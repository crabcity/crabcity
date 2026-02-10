//! Focus Handling
//!
//! Functions for handling focus switches between instances and sending conversation data.

use claude_convo::ClaudeConvo;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::instance_manager::InstanceManager;
use crate::repository::ConversationRepository;

use super::conversation_watcher::run_conversation_watcher;
use super::protocol::ServerMessage;
use super::state_manager::GlobalStateManager;

/// Send conversation entries since a given UUID (or full conversation if None)
pub async fn send_conversation_since(
    instance_id: &str,
    since_uuid: Option<&str>,
    state_manager: &Arc<GlobalStateManager>,
    tx: &mpsc::Sender<ServerMessage>,
    repository: Option<&ConversationRepository>,
) -> Result<(), String> {
    // Get instance working dir and session info from state manager
    let handle = state_manager
        .get_handle(instance_id)
        .await
        .ok_or_else(|| format!("Instance {} not found", instance_id))?;

    let info = handle.get_info().await;
    let working_dir = info.working_dir;
    let session_id = handle
        .get_session_id()
        .await
        .ok_or_else(|| "No session ID available".to_string())?;

    let manager = ClaudeConvo::new();
    let convo = manager
        .read_conversation(&working_dir, &session_id)
        .map_err(|e| format!("Failed to read conversation: {}", e))?;

    // Filter entries based on since_uuid
    let entries = if let Some(since) = since_uuid {
        convo.entries_since(since)
    } else {
        convo.entries
    };

    let mut turns = Vec::with_capacity(entries.len());
    for e in &entries {
        turns.push(
            crate::handlers::format_entry_with_attribution(
                e,
                instance_id,
                repository,
                Some(state_manager),
            )
            .await,
        );
    }

    if since_uuid.is_some() && !turns.is_empty() {
        // Incremental update
        info!(
            "[CONVO-SYNC {}] Sending ConversationUpdate with {} turns (since {:?})",
            instance_id,
            turns.len(),
            since_uuid
        );
        if tx
            .send(ServerMessage::ConversationUpdate {
                instance_id: instance_id.to_string(),
                turns,
            })
            .await
            .is_err()
        {
            warn!(instance = %instance_id, "Failed to send ConversationUpdate - channel closed");
        }
    } else if since_uuid.is_none() {
        // Full conversation
        info!(
            "[CONVO-SYNC {}] Sending ConversationFull with {} turns",
            instance_id,
            turns.len()
        );
        if tx
            .send(ServerMessage::ConversationFull {
                instance_id: instance_id.to_string(),
                turns,
            })
            .await
            .is_err()
        {
            warn!(instance = %instance_id, "Failed to send ConversationFull - channel closed");
        }
    }
    // If since_uuid is Some but turns is empty, nothing new to send

    Ok(())
}

/// Handle focus switch to a new instance - runs until cancelled or error
pub async fn handle_focus(
    instance_id: String,
    _since_uuid: Option<String>,
    cancel: CancellationToken,
    state_manager: Arc<GlobalStateManager>,
    instance_manager: Arc<InstanceManager>,
    tx: mpsc::Sender<ServerMessage>,
    session_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<String>>>,
    max_history_bytes: usize,
    repository: Option<Arc<ConversationRepository>>,
) {
    debug!("Focusing on instance: {}", instance_id);

    // Get current claude_state to include in FocusAck (prevents race condition)
    let current_state = instance_manager
        .get(&instance_id)
        .await
        .and_then(|inst| inst.claude_state);

    // Send focus acknowledgment with current state
    if tx
        .send(ServerMessage::FocusAck {
            instance_id: instance_id.clone(),
            claude_state: current_state.clone(),
        })
        .await
        .is_err()
    {
        return;
    }

    // Get the instance info
    let (handle, working_dir, created_at, is_claude) = match state_manager
        .get_tracker_info(&instance_id)
        .await
    {
        Some(info) => info,
        None => {
            if tx
                .send(ServerMessage::Error {
                    instance_id: Some(instance_id.clone()),
                    message: format!("Instance {} not found", instance_id),
                })
                .await
                .is_err()
            {
                warn!(instance = %instance_id, "Failed to send error (instance not found) - channel closed");
            }
            return;
        }
    };

    // Send terminal history (bounded by config)
    let history = handle.get_recent_output(max_history_bytes).await;
    if !history.is_empty() {
        if tx
            .send(ServerMessage::OutputHistory {
                instance_id: instance_id.clone(),
                data: history.join(""),
            })
            .await
            .is_err()
        {
            warn!(instance = %instance_id, "Failed to send OutputHistory - channel closed");
            return;
        }
    }

    // Note: claude_state is already sent in FocusAck to prevent race conditions.
    // No need to send a separate StateChange here.

    // Subscribe to PTY output
    let mut output_rx = match handle.subscribe_output().await {
        Ok(rx) => rx,
        Err(e) => {
            error!("Failed to subscribe to PTY output: {}", e);
            return;
        }
    };

    // Start conversation watcher if this is a Claude instance
    let convo_task = if is_claude {
        let tx_convo = tx.clone();
        let cancel_convo = cancel.clone();
        let state_mgr = state_manager.clone();
        let instance_id_convo = instance_id.clone();
        let session_rx = session_rx.clone();

        let repo_clone = repository.clone();
        Some(tokio::spawn(async move {
            run_conversation_watcher(
                instance_id_convo,
                working_dir,
                created_at,
                cancel_convo,
                state_mgr,
                tx_convo,
                session_rx,
                repo_clone,
            )
            .await;
        }))
    } else {
        None
    };

    // Forward PTY output to client until cancelled
    // Note: State tracking is handled by the background PTY reader in InstanceTracker,
    // so we only need to forward output to the focused client here.
    let tx_output = tx.clone();
    let instance_id_output = instance_id.clone();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("Focus cancelled for instance {}", instance_id);
                break;
            }
            result = output_rx.recv() => {
                match result {
                    Ok(event) => {
                        let data = String::from_utf8_lossy(&event.data).to_string();
                        // Send to client only - state tracking is done by background task
                        if tx_output.send(ServerMessage::Output {
                            instance_id: instance_id_output.clone(),
                            data,
                        }).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(instance = %instance_id_output, "PTY output lagged by {} messages", n);
                        // Notify client about the lag so UI can indicate data loss
                        if tx_output.send(ServerMessage::OutputLagged {
                            instance_id: instance_id_output.clone(),
                            dropped_count: n,
                        }).await.is_err() {
                            warn!(instance = %instance_id_output, "Failed to send OutputLagged notification - channel closed");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        debug!("PTY output channel closed");
                        break;
                    }
                }
            }
        }
    }

    // Clean up conversation task
    if let Some(task) = convo_task {
        task.abort();
    }
}
