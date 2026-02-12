use axum::extract::ws::{Message, WebSocket};
use chrono::{DateTime, Utc};
use claude_convo::{ClaudeConvo, ConversationWatcher};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::inference::{
    ClaudeState, StateManagerConfig, StateSignal, StateUpdate, spawn_state_manager,
};
use crate::instance_actor::InstanceHandle;
use crate::virtual_terminal::ClientType;
use crate::ws::GlobalStateManager;

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

/// Find candidate sessions that could belong to this instance.
/// Returns sessions that started after the instance was created.
fn find_candidate_sessions(
    working_dir: &str,
    created_at: DateTime<Utc>,
) -> Vec<claude_convo::ConversationMetadata> {
    let manager = ClaudeConvo::new();

    match manager.list_conversation_metadata(working_dir) {
        Ok(metadata) => metadata
            .into_iter()
            .filter(|m| m.started_at.map(|s| s >= created_at).unwrap_or(false))
            .collect(),
        Err(_) => vec![],
    }
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
    _global_state_manager: Option<Arc<GlobalStateManager>>,
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

    // Channel for session selection (when ambiguous)
    let (session_select_tx, mut session_select_rx) = mpsc::channel::<String>(1);

    let is_claude = convo_config.as_ref().map(|c| c.is_claude).unwrap_or(false);

    // Unified state manager: one channel for all state signals
    let (signal_tx, signal_rx) = mpsc::channel::<StateSignal>(100);
    let (state_tx, mut state_rx) = mpsc::channel::<StateUpdate>(100);

    // Spawn the unified state manager (handles tick internally)
    let state_manager_handle = if is_claude {
        Some(spawn_state_manager(
            signal_rx,
            state_tx,
            StateManagerConfig::default(),
        ))
    } else {
        None
    };

    // Task to forward state changes to WebSocket
    let tx_state = tx.clone();
    let state_forward_task = async move {
        while let Some(update) = state_rx.recv().await {
            debug!(
                "Forwarding state change to WebSocket: {:?} (stale={})",
                update.state, update.terminal_stale
            );
            if tx_state
                .send(WsMessage::StateChange {
                    state: update.state,
                    stale: update.terminal_stale,
                })
                .await
                .is_err()
            {
                break;
            }
        }
    };

    // Subscribe to PTY output
    let mut output_rx = match handle.subscribe_output().await {
        Ok(rx) => rx,
        Err(e) => {
            error!("Failed to subscribe to output: {}", e);
            return;
        }
    };

    // Task to forward PTY output to WebSocket (and signal state manager)
    let tx_output = tx.clone();
    let signal_tx_output = signal_tx.clone();
    let output_task = async move {
        let mut decoder = crate::ws::Utf8StreamDecoder::new();
        loop {
            match output_rx.recv().await {
                Ok(event) => {
                    let data = decoder.decode(&event.data);
                    if data.is_empty() {
                        continue;
                    }

                    // Send signal to state manager (for tool detection)
                    if is_claude {
                        if signal_tx_output
                            .send(StateSignal::TerminalOutput { data: data.clone() })
                            .await
                            .is_err()
                        {
                            warn!("Failed to send terminal output signal - state manager closed");
                        }
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

    // Task to watch conversation and send updates (and signal state manager)
    let tx_convo = tx.clone();
    let signal_tx_convo = signal_tx.clone();
    let handle_for_convo = handle.clone();
    let has_convo_config = convo_config.is_some();
    let convo_task = async move {
        let handle = handle_for_convo;
        let Some(config) = convo_config else {
            return;
        };

        if !config.is_claude {
            return;
        }

        // Wait a bit for the session to be created
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        let manager = ClaudeConvo::new();

        // Try to find the session ID if not provided
        let session_id = match config.session_id {
            Some(sid) => {
                debug!("Using cached session ID: {}", sid);
                sid
            }
            None => {
                debug!(
                    "Looking for sessions in {} created after {}",
                    config.working_dir, config.instance_created_at
                );

                loop {
                    let candidates =
                        find_candidate_sessions(&config.working_dir, config.instance_created_at);

                    match candidates.len() {
                        0 => {
                            debug!("No candidate sessions found yet, waiting...");
                        }
                        1 => {
                            let session = &candidates[0];
                            debug!("Found unique session: {}", session.session_id);
                            if let Err(e) = handle.set_session_id(session.session_id.clone()).await
                            {
                                warn!(session = %session.session_id, "Failed to set session ID: {}", e);
                            }
                            break session.session_id.clone();
                        }
                        n => {
                            debug!("Found {} candidate sessions, asking user to select", n);

                            let candidate_info: Vec<SessionCandidate> = candidates
                                .iter()
                                .map(|c| {
                                    let preview = manager
                                        .read_conversation(&config.working_dir, &c.session_id)
                                        .ok()
                                        .and_then(|convo| {
                                            convo.user_messages().first().and_then(|entry| {
                                                entry.message.as_ref().and_then(|msg| {
                                                    match &msg.content {
                                                        Some(
                                                            claude_convo::MessageContent::Text(t),
                                                        ) => Some(t.chars().take(100).collect()),
                                                        Some(
                                                            claude_convo::MessageContent::Parts(
                                                                parts,
                                                            ),
                                                        ) => parts.iter().find_map(|p| match p {
                                                            claude_convo::ContentPart::Text {
                                                                text,
                                                            } => Some(
                                                                text.chars().take(100).collect(),
                                                            ),
                                                            _ => None,
                                                        }),
                                                        None => None,
                                                    }
                                                })
                                            })
                                        });

                                    SessionCandidate {
                                        session_id: c.session_id.clone(),
                                        started_at: c.started_at.map(|s| s.to_rfc3339()),
                                        message_count: c.message_count,
                                        preview,
                                    }
                                })
                                .collect();

                            if tx_convo
                                .send(WsMessage::SessionAmbiguous {
                                    candidates: candidate_info,
                                })
                                .await
                                .is_err()
                            {
                                warn!("Failed to send SessionAmbiguous - channel closed");
                                return;
                            }

                            if let Some(selected) = session_select_rx.recv().await {
                                debug!("User selected session: {}", selected);
                                if let Err(e) = handle.set_session_id(selected.clone()).await {
                                    warn!(session = %selected, "Failed to set session ID: {}", e);
                                }
                                break selected;
                            }
                        }
                    }

                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            }
        };

        debug!("Starting conversation watcher for session {}", session_id);

        let mut watcher = ConversationWatcher::new(manager, config.working_dir, session_id);

        // Send initial full conversation
        if let Ok(entries) = watcher.poll() {
            let turns: Vec<serde_json::Value> =
                entries.iter().map(crate::handlers::format_entry).collect();
            if tx_convo
                .send(WsMessage::ConversationFull { turns })
                .await
                .is_err()
            {
                warn!("Failed to send ConversationFull - channel closed");
                return;
            }

            // Signal initial state from last entry
            if let Some(last) = entries.last() {
                let subtype = last
                    .extra
                    .get("subtype")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                if signal_tx_convo
                    .send(StateSignal::ConversationEntry {
                        entry_type: last.entry_type.clone(),
                        subtype,
                        stop_reason: last.message.as_ref().and_then(|m| m.stop_reason.clone()),
                    })
                    .await
                    .is_err()
                {
                    warn!("Failed to send initial state signal - channel closed");
                }
            }
        }

        // Poll for updates every 500ms
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            match watcher.poll() {
                Ok(new_entries) if !new_entries.is_empty() => {
                    debug!("Conversation watcher got {} new entries", new_entries.len());
                    // Signal state manager for each entry
                    for entry in &new_entries {
                        // Extract subtype from extra fields (for system entries like turn_duration)
                        let subtype = entry
                            .extra
                            .get("subtype")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());

                        debug!(
                            "Sending ConversationEntry signal: type={}, subtype={:?}, stop_reason={:?}",
                            entry.entry_type,
                            subtype,
                            entry.message.as_ref().and_then(|m| m.stop_reason.clone())
                        );
                        if signal_tx_convo
                            .send(StateSignal::ConversationEntry {
                                entry_type: entry.entry_type.clone(),
                                subtype,
                                stop_reason: entry
                                    .message
                                    .as_ref()
                                    .and_then(|m| m.stop_reason.clone()),
                            })
                            .await
                            .is_err()
                        {
                            warn!(
                                "Failed to send conversation entry signal - state manager closed"
                            );
                        }
                    }

                    let turns: Vec<serde_json::Value> = new_entries
                        .iter()
                        .map(crate::handlers::format_entry)
                        .collect();
                    if !turns.is_empty() {
                        if tx_convo
                            .send(WsMessage::ConversationUpdate { turns })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                Err(e) => {
                    debug!("Conversation poll error: {}", e);
                }
                _ => {}
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

    // Task to forward WebSocket input to PTY (and signal state manager)
    let handle_clone = handle.clone();
    let signal_tx_input = signal_tx.clone();
    let connection_id_input = connection_id.clone();
    let input_task = async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    debug!("Client message: {}", text);
                    if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                        match ws_msg {
                            WsMessage::Input { data } => {
                                // Signal state manager
                                if is_claude {
                                    if signal_tx_input
                                        .send(StateSignal::TerminalInput { data: data.clone() })
                                        .await
                                        .is_err()
                                    {
                                        warn!(
                                            "Failed to send terminal input signal - state manager closed"
                                        );
                                    }
                                }

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
                            WsMessage::SessionSelect { session_id } => {
                                debug!("User selected session: {}", session_id);
                                if session_select_tx.send(session_id.clone()).await.is_err() {
                                    warn!(session = %session_id, "Failed to send session selection - receiver dropped");
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
    if has_convo_config {
        tokio::select! {
            _ = output_task => debug!("Output task ended"),
            _ = convo_task => debug!("Conversation task ended"),
            _ = sender_task => debug!("Sender task ended"),
            _ = input_task => debug!("Input task ended"),
            _ = state_forward_task => debug!("State forward task ended"),
        }
    } else {
        tokio::select! {
            _ = output_task => debug!("Output task ended"),
            _ = sender_task => debug!("Sender task ended"),
            _ = input_task => debug!("Input task ended"),
        }
        let _ = convo_task.await;
    }

    // Clean up VirtualTerminal viewport on disconnect
    if let Err(e) = handle.remove_client_and_resize(&connection_id).await {
        warn!(
            "Failed to resize PTY for {} on CLI disconnect: {}",
            instance_id, e
        );
    }

    // Clean up state manager
    if let Some(handle) = state_manager_handle {
        handle.abort();
    }

    debug!("WebSocket proxy closed for instance {}", instance_id);
}
