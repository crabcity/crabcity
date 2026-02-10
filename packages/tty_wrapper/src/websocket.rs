use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::pty_manager::PtyManager;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
enum WsMessage {
    Input { data: String },
    Output { data: String },
    State { state: crate::pty_manager::PtyState },
    Error { message: String },
    Connected { session_id: String },
}

pub async fn handle_websocket(socket: WebSocket, pty: Arc<PtyManager>) {
    let (mut sender, mut receiver) = socket.split();

    // Send initial connection message
    let connected_msg = WsMessage::Connected {
        session_id: uuid::Uuid::new_v4().to_string(),
    };
    if let Ok(msg) = serde_json::to_string(&connected_msg) {
        let _ = sender.send(Message::Text(msg.into())).await;
    }

    // Send initial state
    if let Ok(state) = pty.get_state().await {
        let state_msg = WsMessage::State { state };
        if let Ok(msg) = serde_json::to_string(&state_msg) {
            let _ = sender.send(Message::Text(msg.into())).await;
        }
    }

    // Subscribe to output events
    let mut output_rx = pty.subscribe_output();

    // Channel for output forwarding
    let (output_tx, mut output_rx_ws) = tokio::sync::mpsc::channel::<String>(100);

    // Spawn task to forward output events to channel
    let output_task = tokio::spawn(async move {
        while let Ok(event) = output_rx.recv().await {
            let msg = WsMessage::Output { data: event.data };
            if let Ok(json) = serde_json::to_string(&msg) {
                let _ = output_tx.send(json).await;
            }
        }
    });

    // Handle incoming messages and output forwarding
    loop {
        tokio::select! {
            Some(json) = output_rx_ws.recv() => {
                if sender.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
            Some(Ok(msg)) = receiver.next() => {
        match msg {
            Message::Text(text) => {
                if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                    match ws_msg {
                        WsMessage::Input { data } => {
                            if let Err(e) = pty.write_input(&data).await {
                                warn!("Failed to write input: {}", e);
                                let error_msg = WsMessage::Error {
                                    message: e.to_string(),
                                };
                                if let Ok(json) = serde_json::to_string(&error_msg) {
                                    let _ = sender.send(Message::Text(json.into())).await;
                                }
                            }
                        }
                        _ => {
                            debug!("Unexpected message type from client");
                        }
                    }
                }
            }
            Message::Binary(_) => {
                debug!("Binary messages not supported");
            }
            Message::Close(_) => {
                info!("WebSocket connection closed");
                break;
            }
            _ => {}
        }
            }
            else => break,
        }
    }

    // Cleanup
    output_task.abort();
}
