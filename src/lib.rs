pub mod error;
mod message;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;

pub use error::{Error, Result};
pub use message::coding::*;
pub use message::CloseFrame;
pub use message::Message;
#[cfg(not(target_arch = "wasm32"))]
use native as ws;
#[cfg(target_arch = "wasm32")]
use web as ws;
pub use ws::{Bytes, Utf8Bytes, WebSocketStream};

pub async fn connect<S: AsRef<str>>(url: S) -> Result<WebSocketStream> {
    ws::connect(url.as_ref()).await
}

pub async fn connect_with_protocols<S: AsRef<str>>(
    url: S,
    protocols: &[&str],
) -> Result<WebSocketStream> {
    ws::connect_with_protocols(url.as_ref(), protocols).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assert_impls() {
        use assert_impl::assert_impl;

        assert_impl!(futures_util::Stream<Item = Result<Message>>: WebSocketStream);
        assert_impl!(futures_util::Sink<Message>: WebSocketStream);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod native_tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};

    #[tokio::test]
    async fn ping_test() {
        let port = rand::random();
        let ip_addr = std::net::Ipv4Addr::from([127, 0, 0, 1]);
        let listener = tokio::net::TcpListener::bind((ip_addr, port))
            .await
            .unwrap();

        let payload = vec![0, 1, 2];
        let pong = tokio_tungstenite::tungstenite::Message::Pong(payload.clone().into());
        let handle: tokio::task::JoinHandle<crate::Result<_>> = tokio::spawn(async move {
            let (stream, _) = listener.accept().await?;
            let ping = tokio_tungstenite::tungstenite::Message::Ping(payload.into());

            let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let (mut write, mut read) = ws.split();
            write.send(ping).await?;
            let timeout = std::time::Duration::from_millis(500);
            tokio::time::timeout(timeout, read.next())
                .await
                .map_err(|_| crate::Error::AlreadyClosed)
        });

        let mut client = connect(format!("ws://127.0.0.1:{}", port)).await.unwrap();
        assert!(client.next().await.is_some());
        assert!(client.next().await.is_none());
        let result = handle.await.unwrap().unwrap().unwrap().unwrap();
        assert_eq!(result, pong);
    }
}
