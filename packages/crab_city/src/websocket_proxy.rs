use axum::extract::ws::{Message, WebSocket};
use chrono::{DateTime, Utc};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, warn};

use crate::inference::ClaudeState;
use crate::instance_actor::InstanceHandle;
use crate::virtual_terminal::ClientType;
use crate::ws::{ConversationEvent, GlobalStateManager};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WsMessage {
    Output {
        data: String,
    },
    Input {
        data: String,
    },
    Resize {
        rows: u16,
        cols: u16,
    },
    ConversationUpdate {
        turns: Vec<serde_json::Value>,
    },
    ConversationFull {
        turns: Vec<serde_json::Value>,
    },
    /// Claude state has changed
    StateChange {
        state: ClaudeState,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        stale: bool,
    },
    /// Multiple candidate sessions found - user must pick one
    SessionAmbiguous {
        candidates: Vec<SessionCandidate>,
    },
    /// User selected a session from the ambiguous list
    SessionSelect {
        session_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCandidate {
    pub session_id: String,
    pub started_at: Option<String>,
    pub message_count: usize,
    /// First user message preview (to help identify)
    pub preview: Option<String>,
}

/// Configuration for conversation watching
pub struct ConversationConfig {
    pub working_dir: String,
    pub session_id: Option<String>,
    pub is_claude: bool,
    /// When the instance was created - used to narrow down candidate sessions
    pub instance_created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_message_output_serde() {
        let msg = WsMessage::Output {
            data: "hello".to_string(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "Output");
        assert_eq!(json["data"], "hello");
        let rt: WsMessage = serde_json::from_value(json).unwrap();
        match rt {
            WsMessage::Output { data } => assert_eq!(data, "hello"),
            _ => panic!("Expected Output"),
        }
    }

    #[test]
    fn ws_message_input_serde() {
        let msg = WsMessage::Input {
            data: "ls\n".to_string(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "Input");
        assert_eq!(json["data"], "ls\n");
    }

    #[test]
    fn ws_message_resize_serde() {
        let msg = WsMessage::Resize { rows: 24, cols: 80 };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "Resize");
        assert_eq!(json["rows"], 24);
        assert_eq!(json["cols"], 80);
        let rt: WsMessage = serde_json::from_value(json).unwrap();
        match rt {
            WsMessage::Resize { rows, cols } => {
                assert_eq!(rows, 24);
                assert_eq!(cols, 80);
            }
            _ => panic!("Expected Resize"),
        }
    }

    #[test]
    fn ws_message_conversation_update_serde() {
        let turns = vec![serde_json::json!({"role": "user", "text": "hi"})];
        let msg = WsMessage::ConversationUpdate {
            turns: turns.clone(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "ConversationUpdate");
        assert_eq!(json["turns"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn ws_message_conversation_full_serde() {
        let msg = WsMessage::ConversationFull { turns: vec![] };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "ConversationFull");
        assert!(json["turns"].as_array().unwrap().is_empty());
    }

    #[test]
    fn ws_message_state_change_serde() {
        let msg = WsMessage::StateChange {
            state: ClaudeState::Idle,
            stale: false,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "StateChange");
        // stale=false should be skipped by skip_serializing_if
        assert!(json.get("stale").is_none());
    }

    #[test]
    fn ws_message_state_change_stale_serde() {
        let msg = WsMessage::StateChange {
            state: ClaudeState::Thinking,
            stale: true,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["stale"], true);
    }

    #[test]
    fn ws_message_session_ambiguous_serde() {
        let msg = WsMessage::SessionAmbiguous {
            candidates: vec![SessionCandidate {
                session_id: "sess-1".to_string(),
                started_at: Some("2024-01-01T00:00:00Z".to_string()),
                message_count: 5,
                preview: Some("Hello Claude".to_string()),
            }],
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "SessionAmbiguous");
        assert_eq!(json["candidates"][0]["session_id"], "sess-1");
        assert_eq!(json["candidates"][0]["message_count"], 5);
        assert_eq!(json["candidates"][0]["preview"], "Hello Claude");
    }

    #[test]
    fn ws_message_session_select_serde() {
        let msg = WsMessage::SessionSelect {
            session_id: "sess-2".to_string(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "SessionSelect");
        assert_eq!(json["session_id"], "sess-2");
    }

    #[test]
    fn session_candidate_none_fields() {
        let c = SessionCandidate {
            session_id: "s".to_string(),
            started_at: None,
            message_count: 0,
            preview: None,
        };
        let json = serde_json::to_value(&c).unwrap();
        assert!(json["started_at"].is_null());
        assert!(json["preview"].is_null());
        assert_eq!(json["message_count"], 0);
    }

    #[test]
    fn ws_message_roundtrip_all_variants() {
        let variants: Vec<WsMessage> = vec![
            WsMessage::Output { data: "x".into() },
            WsMessage::Input { data: "y".into() },
            WsMessage::Resize { rows: 10, cols: 20 },
            WsMessage::ConversationUpdate { turns: vec![] },
            WsMessage::ConversationFull { turns: vec![] },
            WsMessage::StateChange {
                state: ClaudeState::Idle,
                stale: false,
            },
            WsMessage::SessionAmbiguous { candidates: vec![] },
            WsMessage::SessionSelect {
                session_id: "s".into(),
            },
        ];
        for msg in variants {
            let json_str = serde_json::to_string(&msg).unwrap();
            let _: WsMessage = serde_json::from_str(&json_str).unwrap();
        }
    }
}

pub async fn handle_proxy(
    socket: WebSocket,
    instance_id: String,
    handle: InstanceHandle,
    convo_config: Option<ConversationConfig>,
    global_state_manager: Option<Arc<GlobalStateManager>>,
) {
    debug!(
        "WebSocket connection established for instance {}",
        instance_id
    );

    // Generate a unique connection ID for VT dimension negotiation
    let connection_id = uuid::Uuid::new_v4().to_string();

    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Create a channel for sending messages to the WebSocket
    let (tx, mut rx) = mpsc::channel::<WsMessage>(100);

    let is_claude = convo_config.as_ref().map(|c| c.is_claude).unwrap_or(false);

    // Subscribe to global state broadcast for state changes
    // (the server-owned conversation watcher feeds the global state manager)
    let mut state_rx: Option<broadcast::Receiver<(String, ClaudeState, bool)>> =
        global_state_manager.as_ref().map(|gsm| gsm.subscribe());

    // Subscribe to conversation broadcast from the server-owned watcher
    let mut convo_rx: Option<broadcast::Receiver<ConversationEvent>> = if is_claude {
        if let Some(ref gsm) = global_state_manager {
            gsm.subscribe_conversation(&instance_id).await
        } else {
            None
        }
    } else {
        None
    };

    // Send current conversation snapshot from server store
    if is_claude {
        if let Some(ref gsm) = global_state_manager {
            let turns = gsm.get_conversation_snapshot(&instance_id).await;
            if !turns.is_empty() {
                let _ = tx.send(WsMessage::ConversationFull { turns }).await;
            }
        }
    }

    // Subscribe to PTY output
    let mut output_rx = match handle.subscribe_output().await {
        Ok(rx) => rx,
        Err(e) => {
            error!("Failed to subscribe to output: {}", e);
            return;
        }
    };

    // Task to forward PTY output to WebSocket
    let tx_output = tx.clone();
    let output_task = async move {
        let mut decoder = crate::ws::Utf8StreamDecoder::new();
        loop {
            match output_rx.recv().await {
                Ok(event) => {
                    let data = decoder.decode(&event.data);
                    if data.is_empty() {
                        continue;
                    }
                    if tx_output.send(WsMessage::Output { data }).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    decoder.clear();
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    // Task to forward state changes and conversation events to WebSocket
    let tx_events = tx.clone();
    let instance_id_events = instance_id.clone();
    let gsm_for_events = global_state_manager.clone();
    let events_task = async move {
        loop {
            tokio::select! {
                // Forward state changes from global broadcast
                state_event = async {
                    if let Some(ref mut rx) = state_rx {
                        rx.recv().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    match state_event {
                        Ok((ref iid, ref state, stale)) if iid == &instance_id_events => {
                            debug!("Forwarding state change to TUI WebSocket: {:?} (stale={})", state, stale);
                            if tx_events.send(WsMessage::StateChange {
                                state: state.clone(),
                                stale,
                            }).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        _ => {} // Different instance
                    }
                }
                // Forward conversation events from server-owned watcher
                convo_event = async {
                    if let Some(ref mut rx) = convo_rx {
                        rx.recv().await
                    } else {
                        std::future::pending().await
                    }
                } => {
                    match convo_event {
                        Ok(ConversationEvent::Full { instance_id: ref iid, turns }) if iid == &instance_id_events => {
                            if tx_events.send(WsMessage::ConversationFull { turns }).await.is_err() {
                                break;
                            }
                        }
                        Ok(ConversationEvent::Update { instance_id: ref iid, turns }) if iid == &instance_id_events => {
                            if tx_events.send(WsMessage::ConversationUpdate { turns }).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // Re-sync from server store
                            if let Some(ref gsm) = gsm_for_events {
                                let turns = gsm.get_conversation_snapshot(&instance_id_events).await;
                                if !turns.is_empty() {
                                    let _ = tx_events.send(WsMessage::ConversationFull { turns }).await;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                        _ => {} // Different instance
                    }
                }
            }
        }
    };

    // Task to send messages from channel to WebSocket
    let sender_task = async move {
        while let Some(msg) = rx.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(e) => {
                    error!("Failed to serialize message: {}", e);
                    continue;
                }
            };
            if ws_sender.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    };

    // Task to forward WebSocket input to PTY
    let handle_clone = handle.clone();
    let connection_id_input = connection_id.clone();
    let input_task = async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    debug!("Client message: {}", text);
                    if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                        match ws_msg {
                            WsMessage::Input { data } => {
                                if let Err(e) = handle_clone.write_input(&data).await {
                                    error!("Failed to write to PTY: {}", e);
                                }
                            }
                            WsMessage::Resize { rows, cols } => {
                                if let Err(e) = handle_clone
                                    .update_viewport_and_resize(
                                        &connection_id_input,
                                        rows,
                                        cols,
                                        ClientType::Terminal,
                                    )
                                    .await
                                {
                                    error!("Failed to resize PTY: {}", e);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Ok(Message::Close(_)) => {
                    debug!("Client closed connection");
                    break;
                }
                Err(e) => {
                    error!("WebSocket error from client: {}", e);
                    break;
                }
                _ => {}
            }
        }
    };

    // Run all tasks concurrently
    tokio::select! {
        _ = output_task => debug!("Output task ended"),
        _ = events_task => debug!("Events task ended"),
        _ = sender_task => debug!("Sender task ended"),
        _ = input_task => debug!("Input task ended"),
    }

    // Clean up VirtualTerminal viewport on disconnect
    if let Err(e) = handle.remove_client_and_resize(&connection_id).await {
        warn!(
            "Failed to resize PTY for {} on CLI disconnect: {}",
            instance_id, e
        );
    }

    debug!("WebSocket proxy closed for instance {}", instance_id);
}
