pub use bytes::Bytes;
use std::{cell::RefCell, collections::VecDeque, rc::Rc, task::Waker};
pub use utf8_bytes::Utf8Bytes;
use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{CloseEvent, ErrorEvent, MessageEvent, WebSocket};

pub async fn connect(url: &str) -> crate::Result<WebSocketStream> {
    WebSocketStream::new(url).await
}

pub async fn connect_with_protocols(
    url: &str,
    protocols: &[&str],
) -> crate::Result<WebSocketStream> {
    WebSocketStream::new_with_protocols(url, protocols).await
}

pub struct WebSocketStream {
    inner: WebSocket,
    queue: Rc<RefCell<VecDeque<crate::Result<crate::Message>>>>,
    waker: Rc<RefCell<Option<Waker>>>,
    _on_message_callback: Closure<dyn FnMut(MessageEvent)>,
    _on_error_callback: Closure<dyn FnMut(ErrorEvent)>,
    _on_close_callback: Closure<dyn FnMut(CloseEvent)>,
}

impl WebSocketStream {
    async fn new_with_protocols(url: &str, protocols: &[&str]) -> crate::Result<Self> {
        // Can be added as protocol values in multiple headers,
        // or as comma separate values added to a single header
        match web_sys::WebSocket::new_with_str(url, &protocols.join(", ")) {
            Err(_err) => Err(crate::Error::Url(
                crate::error::UrlError::UnsupportedUrlScheme,
            )),
            Ok(ws) => Ok(Self::register_callbacks(ws).await?),
        }
    }

    async fn new(url: &str) -> crate::Result<Self> {
        match web_sys::WebSocket::new(url) {
            Err(_err) => Err(crate::Error::Url(
                crate::error::UrlError::UnsupportedUrlScheme,
            )),
            Ok(ws) => Ok(Self::register_callbacks(ws).await?),
        }
    }

    async fn register_callbacks(ws: WebSocket) -> crate::Result<Self> {
        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

        let (open_sx, open_rx) = futures_channel::oneshot::channel();
        let on_open_callback = {
            let mut open_sx = Some(open_sx);
            Closure::wrap(Box::new(move |_event| {
                open_sx.take().map(|open_sx| open_sx.send(()));
            }) as Box<dyn FnMut(web_sys::Event)>)
        };
        ws.set_onopen(Some(on_open_callback.as_ref().unchecked_ref()));

        let (err_sx, err_rx) = futures_channel::oneshot::channel();
        let on_error_callback = {
            let mut err_sx = Some(err_sx);
            Closure::wrap(Box::new(move |_error_event| {
                err_sx.take().map(|err_sx| err_sx.send(()));
            }) as Box<dyn FnMut(ErrorEvent)>)
        };
        ws.set_onerror(Some(on_error_callback.as_ref().unchecked_ref()));

        let result = futures_util::future::select(open_rx, err_rx).await;
        ws.set_onopen(None);
        ws.set_onerror(None);
        let ws = match result {
            futures_util::future::Either::Left((_, _)) => Ok(ws),
            futures_util::future::Either::Right((_, _)) => Err(crate::Error::ConnectionClosed),
        }?;

        let waker = Rc::new(RefCell::new(Option::<Waker>::None));
        let queue = Rc::new(RefCell::new(VecDeque::new()));
        let on_message_callback = {
            let waker = Rc::clone(&waker);
            let queue = Rc::clone(&queue);
            Closure::wrap(Box::new(move |event: MessageEvent| {
                let payload = std::convert::TryFrom::try_from(event);
                queue.borrow_mut().push_back(payload);
                if let Some(waker) = waker.borrow_mut().take() {
                    waker.wake();
                }
            }) as Box<dyn FnMut(MessageEvent)>)
        };
        ws.set_onmessage(Some(on_message_callback.as_ref().unchecked_ref()));

        let on_error_callback = {
            let waker = Rc::clone(&waker);
            let queue = Rc::clone(&queue);
            Closure::wrap(Box::new(move |_error_event| {
                queue
                    .borrow_mut()
                    .push_back(Err(crate::Error::ConnectionClosed));
                if let Some(waker) = waker.borrow_mut().take() {
                    waker.wake();
                }
            }) as Box<dyn FnMut(ErrorEvent)>)
        };
        ws.set_onerror(Some(on_error_callback.as_ref().unchecked_ref()));

        let on_close_callback = {
            let waker = Rc::clone(&waker);
            let queue = Rc::clone(&queue);
            Closure::wrap(Box::new(move |event: CloseEvent| {
                queue.borrow_mut().push_back(Ok(crate::Message::Close(Some(
                    crate::message::CloseFrame {
                        code: event.code().into(),
                        reason: event.reason().into(),
                    },
                ))));
                if let Some(waker) = waker.borrow_mut().take() {
                    waker.wake();
                }
            }) as Box<dyn FnMut(CloseEvent)>)
        };
        ws.set_onclose(Some(on_close_callback.as_ref().unchecked_ref()));

        Ok(Self {
            inner: ws,
            queue,
            waker,
            _on_message_callback: on_message_callback,
            _on_error_callback: on_error_callback,
            _on_close_callback: on_close_callback,
        })
    }
}

impl Drop for WebSocketStream {
    fn drop(&mut self) {
        let _r = self.inner.close();
        self.inner.set_onmessage(None);
        self.inner.set_onclose(None);
        self.inner.set_onerror(None);
    }
}

enum ReadyState {
    Closed,
    Closing,
    Connecting,
    Open,
}

impl std::convert::TryFrom<u16> for ReadyState {
    type Error = ();

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            web_sys::WebSocket::CLOSED => Ok(Self::Closed),
            web_sys::WebSocket::CLOSING => Ok(Self::Closing),
            web_sys::WebSocket::OPEN => Ok(Self::Open),
            web_sys::WebSocket::CONNECTING => Ok(Self::Connecting),
            _ => Err(()),
        }
    }
}

mod stream {
    use super::ReadyState;
    use std::convert::TryInto;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    impl futures_util::Stream for super::WebSocketStream {
        type Item = crate::Result<crate::Message>;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            if self.queue.borrow().is_empty() {
                *self.waker.borrow_mut() = Some(cx.waker().clone());

                match std::convert::TryFrom::try_from(self.inner.ready_state()) {
                    Ok(ReadyState::Open) => Poll::Pending,
                    _ => None.into(),
                }
            } else {
                self.queue.borrow_mut().pop_front().into()
            }
        }
    }

    impl futures_util::Sink<crate::Message> for super::WebSocketStream {
        type Error = crate::Error;

        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            match std::convert::TryFrom::try_from(self.inner.ready_state()) {
                Ok(ReadyState::Open) => Ok(()).into(),
                _ => Err(crate::Error::ConnectionClosed).into(),
            }
        }

        fn start_send(self: Pin<&mut Self>, item: crate::Message) -> Result<(), Self::Error> {
            match std::convert::TryFrom::try_from(self.inner.ready_state()) {
                Ok(ReadyState::Open) => {
                    match item {
                        crate::Message::Text(text) => self
                            .inner
                            .send_with_str(&text)
                            .map_err(|_| crate::Error::Sending)?,
                        crate::Message::Binary(bin) => {
                            // #[cfg(target_feature = "atomics")]
                            {
                                // When atomics are enabled, WASM memory is backed by SharedArrayBuffer
                                // Copy to a regular Uint8Array to avoid WebSocket compatibility issues
                                let len = bin.len()
                                    .try_into()
                                    .map_err(|_| crate::Error::Sending)?;

                                let array = js_sys::Uint8Array::new_with_length(len);
                                array.copy_from(&bin);
                                self.inner
                                    .send_with_js_u8_array(&array)
                                    .map_err(|_| crate::Error::Sending)?
                            }
                            #[cfg(not(target_feature = "atomics"))]
                            {
                                // Direct send is safe with regular ArrayBuffer
                                self.inner
                                    .send_with_u8_array(&bin)
                                    .map_err(|_| crate::Error::Sending)?
                            }
                        }
                        crate::Message::Close(frame) => match frame {
                            None => self
                                .inner
                                .close()
                                .map_err(|_| crate::Error::AlreadyClosed)?,
                            Some(frame) => self
                                .inner
                                .close_with_code_and_reason(frame.code.into(), &frame.reason)
                                .map_err(|_| crate::Error::AlreadyClosed)?,
                        },
                    }
                    Ok(())
                }
                _ => Err(crate::Error::ConnectionClosed),
            }
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Ok(()).into()
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            self.inner
                .close()
                .map_err(|_| crate::Error::AlreadyClosed)?;
            Ok(()).into()
        }
    }
}

impl std::convert::TryFrom<web_sys::MessageEvent> for crate::Message {
    type Error = crate::Error;

    fn try_from(event: MessageEvent) -> Result<Self, Self::Error> {
        match event.data() {
            payload if payload.is_instance_of::<js_sys::ArrayBuffer>() => {
                let buffer = js_sys::Uint8Array::new(payload.unchecked_ref());
                let mut v = bytes::BytesMut::zeroed(buffer.length() as usize);
                buffer.copy_to(v.as_mut());
                Ok(crate::Message::Binary(v.freeze()))
            }
            payload if payload.is_string() => match payload.as_string() {
                Some(text) => Ok(crate::Message::text(text)),
                None => Err(crate::Error::UnknownFormat),
            },
            payload if payload.is_instance_of::<web_sys::Blob>() => {
                Err(crate::Error::BlobFormatUnsupported)
            }
            _ => Err(crate::Error::UnknownFormat),
        }
    }
}

mod utf8_bytes {
    use bytes::{Bytes, BytesMut};
    use core::str;
    use std::{convert::{TryFrom, TryInto}, fmt::Display};

    /// Utf8 payload.
    #[derive(Debug, Default, Clone, Eq, PartialEq)]
    pub struct Utf8Bytes(Bytes);

    impl Utf8Bytes {
        /// Creates from a static str.
        #[inline]
        pub const fn from_static(str: &'static str) -> Self {
            Self(Bytes::from_static(str.as_bytes()))
        }

        /// Returns as a string slice.
        #[inline]
        #[allow(unsafe_code)]
        pub fn as_str(&self) -> &str {
            // SAFETY: is valid uft8
            unsafe { str::from_utf8_unchecked(&self.0) }
        }
    }

    impl std::ops::Deref for Utf8Bytes {
        type Target = str;

        /// ```
        /// /// Example fn that takes a str slice
        /// fn a(s: &str) {}
        ///
        /// let data = tungstenite::Utf8Bytes::from_static("foo123");
        ///
        /// // auto-deref as arg
        /// a(&data);
        ///
        /// // deref to str methods
        /// assert_eq!(data.len(), 6);
        /// ```
        #[inline]
        fn deref(&self) -> &Self::Target {
            self.as_str()
        }
    }

    impl<T> PartialEq<T> for Utf8Bytes
    where
        for<'a> &'a str: PartialEq<T>,
    {
        /// ```
        /// let payload = tungstenite::Utf8Bytes::from_static("foo123");
        /// assert_eq!(payload, "foo123");
        /// assert_eq!(payload, "foo123".to_string());
        /// assert_eq!(payload, &"foo123".to_string());
        /// assert_eq!(payload, std::borrow::Cow::from("foo123"));
        /// ```
        #[inline]
        fn eq(&self, other: &T) -> bool {
            self.as_str() == *other
        }
    }

    impl Display for Utf8Bytes {
        #[inline]
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.as_str())
        }
    }

    impl TryFrom<Bytes> for Utf8Bytes {
        type Error = str::Utf8Error;

        #[inline]
        fn try_from(bytes: Bytes) -> Result<Self, Self::Error> {
            str::from_utf8(&bytes)?;
            Ok(Self(bytes))
        }
    }

    impl TryFrom<BytesMut> for Utf8Bytes {
        type Error = str::Utf8Error;

        #[inline]
        fn try_from(bytes: BytesMut) -> Result<Self, Self::Error> {
            bytes.freeze().try_into()
        }
    }

    impl TryFrom<Vec<u8>> for Utf8Bytes {
        type Error = str::Utf8Error;

        #[inline]
        fn try_from(v: Vec<u8>) -> Result<Self, Self::Error> {
            Bytes::from(v).try_into()
        }
    }

    impl From<String> for Utf8Bytes {
        #[inline]
        fn from(s: String) -> Self {
            Self(s.into())
        }
    }

    impl From<&str> for Utf8Bytes {
        #[inline]
        fn from(s: &str) -> Self {
            Self(Bytes::copy_from_slice(s.as_bytes()))
        }
    }

    impl From<&String> for Utf8Bytes {
        #[inline]
        fn from(s: &String) -> Self {
            s.as_str().into()
        }
    }

    impl From<Utf8Bytes> for Bytes {
        #[inline]
        fn from(Utf8Bytes(bytes): Utf8Bytes) -> Self {
            bytes
        }
    }
}
