# tokio-tungstenite-wasm
A wrapper around websys and tokio-tungstenite that makes it easy to use websockets cross-platform.

## Features

As with tungstenite-rs TLS is supported on all platforms using native-tls or rustls through feature flags: native-tls, rustls-tls-native-roots or rustls-tls-webpki-roots feature flags. Neither is enabled by default. See the Cargo.toml for more information. If you require support for secure WebSockets (wss://) enable one of them.

These are, at time of writing, at parity with [tokio-tungstenite](https://github.com/snapview/tokio-tungstenite/tree/master?tab=readme-ov-file#features).