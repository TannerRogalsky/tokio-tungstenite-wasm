use futures_util::{SinkExt, StreamExt};

#[tokio::main]
async fn main() {
    let ws = tokio_tungstenite_wasm::connect("wss://echo.websocket.org/")
        .await
        .unwrap();
    let (mut sender, mut receiver) = ws.split();

    println!("Connected to echo server.");

    let msg = receiver.next().await.unwrap().unwrap();
    assert!(msg.into_text().unwrap().starts_with("Request served by"));

    println!("Recieved initial connection message.");

    let payload = "This is a test message.";
    sender
        .send(tokio_tungstenite_wasm::Message::text(payload))
        .await
        .unwrap();

    println!("Sent payload to echo server. Waiting for response...");

    let msg = receiver.next().await.unwrap().unwrap();
    assert_eq!(msg, tokio_tungstenite_wasm::Message::text(payload));

    println!("Received and validated response.")
}
