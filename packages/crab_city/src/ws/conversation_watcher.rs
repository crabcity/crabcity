//! Conversation Watcher
//!
//! Server-owned conversation watcher: exactly one per instance.
//! Discovers the session, maintains formatted conversation data,
//! broadcasts updates to consumers, and feeds state signals for inference.

use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use toolpath_claude::{ClaudeConvo, ConversationWatcher};
use toolpath_convo::ConversationProvider;
use tracing::{debug, info, warn};

use crate::inference::StateSignal;
use crate::repository::ConversationRepository;

use super::session_discovery::find_candidate_sessions;
use super::state_manager::{ConversationBroadcast, ConversationEvent, GlobalStateManager};

/// Helper to extract a state signal from a conversation entry.
fn entry_state_signal(entry: &toolpath_claude::ConversationEntry) -> StateSignal {
    let subtype = entry
        .extra
        .get("subtype")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    StateSignal::ConversationEntry {
        entry_type: entry.entry_type.clone(),
        subtype,
        stop_reason: entry.message.as_ref().and_then(|m| m.stop_reason.clone()),
    }
}

/// Server-owned conversation watcher for an instance.
///
/// This is the **single** watcher per instance. It:
/// 1. Waits for the session to be discovered (by the focus path or auto-discovery)
/// 2. Polls the JSONL conversation log
/// 3. Formats entries with attribution
/// 4. Stores formatted turns in the shared conversation store
/// 5. Broadcasts `ConversationEvent::Full`/`Update` for consumers
/// 6. Sends `StateSignal::ConversationEntry` for inference
///
/// Runs for the lifetime of the instance (cancelled when unregistered).
pub async fn run_server_conversation_watcher(
    instance_id: String,
    working_dir: String,
    created_at: DateTime<Utc>,
    cancel: CancellationToken,
    signal_tx: mpsc::Sender<StateSignal>,
    state_manager: Arc<GlobalStateManager>,
    conversation_turns: Arc<RwLock<Vec<serde_json::Value>>>,
    conversation_tx: ConversationBroadcast,
    repository: Option<Arc<ConversationRepository>>,
) {
    let manager = ClaudeConvo::new();

    // Phase 1: Discover the session.
    // First try auto-discovery (single unclaimed candidate). If ambiguous,
    // the focus path (run_session_discovery) will claim a session and set
    // session_id on the handle — we just wait for it.
    let session_id = loop {
        if cancel.is_cancelled() {
            return;
        }

        // Check if a session has already been claimed (by focus path or prior run)
        if let Some(handle) = state_manager.get_handle(&instance_id).await {
            if let Some(sid) = handle.get_session_id().await {
                debug!(
                    "[SERVER-CONVO {}] Using claimed session {}",
                    instance_id, sid
                );
                break sid;
            }
        }

        // Attempt auto-discovery: if exactly one unclaimed candidate, claim it
        let search_after = state_manager
            .get_first_input_at(&instance_id)
            .await
            .unwrap_or(created_at);

        let claimed_sessions = state_manager.get_claimed_sessions().await;
        let candidates: Vec<_> = find_candidate_sessions(&manager, &working_dir, search_after)
            .into_iter()
            .filter(|c| !claimed_sessions.contains(&c.id))
            .collect();

        match candidates.len() {
            0 => {
                info!(
                    "[SERVER-CONVO {}] No candidate sessions (search_after={}, working_dir={})",
                    instance_id, search_after, working_dir
                );
            }
            1 => {
                let session = &candidates[0];
                if state_manager
                    .try_claim_session(&session.id, &instance_id)
                    .await
                {
                    info!(
                        "[SERVER-CONVO {}] Auto-claimed session {}",
                        instance_id, session.id
                    );
                    if let Some(handle) = state_manager.get_handle(&instance_id).await {
                        if let Err(e) = handle.set_session_id(session.id.clone()).await {
                            warn!(instance = %instance_id, session = %session.id, "Failed to set session ID: {}", e);
                        }
                    }
                    break session.id.clone();
                }
            }
            n => {
                debug!(
                    "[SERVER-CONVO {}] {} ambiguous candidates, waiting for focus-path resolution",
                    instance_id, n
                );
            }
        }

        // No session yet — wait and retry
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    };

    // Phase 2: Watch the conversation.
    let repo_ref = repository.as_ref();
    let mut watcher = ConversationWatcher::new(manager, working_dir, session_id);

    info!(
        "[SERVER-CONVO {}] Starting conversation watcher for session {} (project={})",
        instance_id,
        watcher.session_id(),
        watcher.project()
    );

    // Initial poll — load existing entries
    match watcher.poll() {
        Ok(entries) => {
            info!(
                "[SERVER-CONVO {}] Initial poll: {} entries",
                instance_id,
                entries.len()
            );

            // Format entries (skip phantom tool-result-only entries)
            let mut turns = Vec::with_capacity(entries.len());
            for e in &entries {
                if crate::handlers::is_tool_result_only(e) {
                    continue;
                }
                turns.push(
                    crate::handlers::format_entry_with_attribution(
                        e,
                        &instance_id,
                        repo_ref,
                        Some(&state_manager),
                    )
                    .await,
                );
            }

            // Store in shared state
            {
                let mut store = conversation_turns.write().await;
                *store = turns.clone();
            }

            // Broadcast full conversation to any current subscribers
            let _ = conversation_tx.send(ConversationEvent::Full {
                instance_id: instance_id.clone(),
                turns,
            });

            // Signal initial state from last entry
            if let Some(last) = entries.last() {
                if signal_tx.send(entry_state_signal(last)).await.is_err() {
                    warn!(instance = %instance_id, "State signal channel closed on initial poll");
                    return;
                }
            }
        }
        Err(e) => {
            warn!("[SERVER-CONVO {}] Initial poll failed: {}", instance_id, e);
        }
    }

    // Poll loop
    let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(500));

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("[SERVER-CONVO {}] Cancelled", instance_id);
                break;
            }
            _ = interval.tick() => {
                match watcher.poll() {
                    Ok(new_entries) if !new_entries.is_empty() => {
                        debug!(
                            "[SERVER-CONVO {}] {} new entries",
                            instance_id, new_entries.len()
                        );

                        // Signal state manager for each entry
                        for entry in &new_entries {
                            if signal_tx.send(entry_state_signal(entry)).await.is_err() {
                                warn!("[SERVER-CONVO {}] State signal channel closed", instance_id);
                                return;
                            }
                        }

                        // Format new entries (skip phantom tool-result-only entries)
                        let mut turns = Vec::with_capacity(new_entries.len());
                        for e in &new_entries {
                            if crate::handlers::is_tool_result_only(e) {
                                continue;
                            }
                            turns.push(
                                crate::handlers::format_entry_with_attribution(
                                    e,
                                    &instance_id,
                                    repo_ref,
                                    Some(&state_manager),
                                )
                                .await,
                            );
                        }

                        // Append to shared store
                        {
                            let mut store = conversation_turns.write().await;
                            store.extend(turns.clone());
                        }

                        // Broadcast update
                        if !turns.is_empty() {
                            let _ = conversation_tx.send(ConversationEvent::Update {
                                instance_id: instance_id.clone(),
                                turns,
                            });
                        }
                    }
                    Err(e) => {
                        warn!("[SERVER-CONVO {}] Poll error: {}", instance_id, e);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Session discovery for the focus path.
///
/// Called when a client focuses on an instance that doesn't have a session yet.
/// Discovers candidates, handles the ambiguous case (sends `SessionAmbiguous`
/// to the client, waits for selection), claims the session, and sets session_id
/// on the handle. The server-owned watcher then picks it up.
///
/// Does NOT watch the conversation — that's the server watcher's job.
pub async fn run_session_discovery(
    instance_id: String,
    working_dir: String,
    created_at: DateTime<Utc>,
    cancel: CancellationToken,
    state_manager: Arc<GlobalStateManager>,
    tx: mpsc::Sender<super::protocol::ServerMessage>,
    session_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<String>>>,
) {
    // Wait briefly for session to be created
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    if cancel.is_cancelled() {
        return;
    }

    let provider = ClaudeConvo::new();

    loop {
        if cancel.is_cancelled() {
            return;
        }

        // Check if the server watcher already found a session (auto-discovery)
        if let Some(handle) = state_manager.get_handle(&instance_id).await {
            if handle.get_session_id().await.is_some() {
                debug!(
                    "[SESSION-DISCOVERY {}] Session already claimed by server watcher",
                    instance_id
                );
                return;
            }
        }

        let search_after = state_manager
            .get_first_input_at(&instance_id)
            .await
            .unwrap_or(created_at);

        let claimed_sessions = state_manager.get_claimed_sessions().await;
        let candidates: Vec<_> = find_candidate_sessions(&provider, &working_dir, search_after)
            .into_iter()
            .filter(|c| !claimed_sessions.contains(&c.id))
            .collect();

        match candidates.len() {
            0 => {
                debug!(
                    "[SESSION-DISCOVERY {}] No candidates yet, waiting...",
                    instance_id
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
            1 => {
                // Single candidate — the server watcher will auto-claim it.
                // Just wait for it to pick it up.
                debug!(
                    "[SESSION-DISCOVERY {}] Single candidate, letting server watcher auto-claim",
                    instance_id
                );
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
            n => {
                // Multiple candidates — need user to select
                debug!(
                    "[SESSION-DISCOVERY {}] {} ambiguous candidates, asking user",
                    instance_id, n
                );

                let candidate_info: Vec<super::protocol::SessionCandidate> = candidates
                    .iter()
                    .map(|c| {
                        let preview = provider
                            .load_conversation(&working_dir, &c.id)
                            .ok()
                            .and_then(|view| view.title(100));

                        super::protocol::SessionCandidate {
                            session_id: c.id.clone(),
                            started_at: c.started_at.map(|s| s.to_rfc3339()),
                            message_count: c.message_count,
                            preview,
                        }
                    })
                    .collect();

                if tx
                    .send(super::protocol::ServerMessage::SessionAmbiguous {
                        instance_id: instance_id.clone(),
                        candidates: candidate_info,
                    })
                    .await
                    .is_err()
                {
                    warn!(instance = %instance_id, "Failed to send SessionAmbiguous - channel closed");
                    return;
                }

                // Wait for selection
                let mut rx = session_rx.lock().await;
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    selected = rx.recv() => {
                        if let Some(selected_id) = selected {
                            debug!("[SESSION-DISCOVERY {}] User selected session: {}", instance_id, selected_id);

                            // Claim and set session_id — server watcher will pick it up
                            state_manager.try_claim_session(&selected_id, &instance_id).await;
                            if let Some(handle) = state_manager.get_handle(&instance_id).await {
                                if let Err(e) = handle.set_session_id(selected_id.clone()).await {
                                    warn!(instance = %instance_id, session = %selected_id, "Failed to set session ID: {}", e);
                                }
                            }
                            return;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use toolpath_claude::ConversationEntry;

    #[test]
    fn test_filter_entries_since_uuid() {
        // Create test entries
        let entries: Vec<ConversationEntry> = vec![
            serde_json::from_str(
                r#"{"uuid":"uuid-1","type":"user","timestamp":"2024-01-01T00:00:00Z"}"#,
            )
            .unwrap(),
            serde_json::from_str(
                r#"{"uuid":"uuid-2","type":"assistant","timestamp":"2024-01-01T00:00:01Z"}"#,
            )
            .unwrap(),
            serde_json::from_str(
                r#"{"uuid":"uuid-3","type":"user","timestamp":"2024-01-01T00:00:02Z"}"#,
            )
            .unwrap(),
            serde_json::from_str(
                r#"{"uuid":"uuid-4","type":"assistant","timestamp":"2024-01-01T00:00:03Z"}"#,
            )
            .unwrap(),
        ];

        // Filter since uuid-2 (should return uuid-3, uuid-4)
        let since_uuid = "uuid-2";
        let since_idx = entries.iter().position(|e| e.uuid == since_uuid);
        let filtered: Vec<_> = match since_idx {
            Some(idx) => entries.into_iter().skip(idx + 1).collect(),
            None => entries,
        };

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].uuid, "uuid-3");
        assert_eq!(filtered[1].uuid, "uuid-4");
    }

    #[test]
    fn test_filter_entries_since_unknown_uuid() {
        let entries: Vec<ConversationEntry> = vec![
            serde_json::from_str(
                r#"{"uuid":"uuid-1","type":"user","timestamp":"2024-01-01T00:00:00Z"}"#,
            )
            .unwrap(),
            serde_json::from_str(
                r#"{"uuid":"uuid-2","type":"assistant","timestamp":"2024-01-01T00:00:01Z"}"#,
            )
            .unwrap(),
        ];

        // Filter since unknown UUID (should return all entries)
        let since_uuid = "unknown-uuid";
        let since_idx = entries.iter().position(|e| e.uuid == since_uuid);
        let filtered: Vec<_> = match since_idx {
            Some(idx) => entries.clone().into_iter().skip(idx + 1).collect(),
            None => entries.clone(),
        };

        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_filter_entries_since_last_uuid() {
        let entries: Vec<ConversationEntry> = vec![
            serde_json::from_str(
                r#"{"uuid":"uuid-1","type":"user","timestamp":"2024-01-01T00:00:00Z"}"#,
            )
            .unwrap(),
            serde_json::from_str(
                r#"{"uuid":"uuid-2","type":"assistant","timestamp":"2024-01-01T00:00:01Z"}"#,
            )
            .unwrap(),
        ];

        // Filter since last UUID (should return empty)
        let since_uuid = "uuid-2";
        let since_idx = entries.iter().position(|e| e.uuid == since_uuid);
        let filtered: Vec<_> = match since_idx {
            Some(idx) => entries.into_iter().skip(idx + 1).collect(),
            None => entries,
        };

        assert_eq!(filtered.len(), 0);
    }
}
