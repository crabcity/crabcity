//! Conversation Watcher
//!
//! Functions for watching Claude conversation files and sending updates to clients.

use chrono::{DateTime, Utc};
use claude_convo::{ClaudeConvo, ConversationWatcher};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::inference::StateSignal;
use crate::repository::ConversationRepository;

use super::protocol::{ServerMessage, SessionCandidate};
use super::session_discovery::find_candidate_sessions;
use super::state_manager::GlobalStateManager;

/// Run conversation watcher for an instance - runs until cancelled
pub async fn run_conversation_watcher(
    instance_id: String,
    working_dir: String,
    created_at: DateTime<Utc>,
    cancel: CancellationToken,
    state_manager: Arc<GlobalStateManager>,
    tx: mpsc::Sender<ServerMessage>,
    session_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<String>>>,
    repository: Option<Arc<ConversationRepository>>,
) {
    // Check if we already have a cached session_id from previous focus
    if let Some(handle) = state_manager.get_handle(&instance_id).await {
        if let Some(cached_session_id) = handle.get_session_id().await {
            debug!(
                "Using cached session_id for instance {}: {}",
                instance_id, cached_session_id
            );
            // Skip session discovery, go straight to watching
            run_conversation_watcher_with_session(
                instance_id,
                working_dir,
                cached_session_id,
                cancel,
                state_manager,
                tx,
                repository,
            )
            .await;
            return;
        }
    }

    // No cached session - need to discover it
    // Wait for session to be created
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    if cancel.is_cancelled() {
        return;
    }

    let manager = ClaudeConvo::new();

    // Find or select session
    let session_id = loop {
        if cancel.is_cancelled() {
            return;
        }

        // Use first_input_at for tighter causation-based session matching.
        // The Input message is the causal event that triggers Claude to create
        // a session, so sessions created after that timestamp are the right candidates.
        // Fall back to created_at if no input has been sent yet (user focused
        // but hasn't typed).
        let search_after = match state_manager.get_first_input_at(&instance_id).await {
            Some(first_input) => {
                debug!(
                    "Using first_input_at ({}) for session discovery on instance {}",
                    first_input, instance_id
                );
                first_input
            }
            None => {
                debug!(
                    "No first_input_at yet for instance {}, using created_at ({})",
                    instance_id, created_at
                );
                created_at
            }
        };

        let all_candidates = find_candidate_sessions(&working_dir, search_after);

        // Filter out sessions that are already claimed by OTHER instances
        // This prevents instance #1 from seeing instance #2's claimed session
        let claimed_sessions = state_manager.get_claimed_sessions().await;
        let candidates: Vec<_> = all_candidates
            .into_iter()
            .filter(|c| !claimed_sessions.contains(&c.session_id))
            .collect();

        debug!(
            "Instance {} found {} candidate sessions ({} after filtering claimed)",
            instance_id,
            candidates.len() + claimed_sessions.len(),
            candidates.len()
        );

        match candidates.len() {
            0 => {
                debug!("No unclaimed candidate sessions found yet, waiting...");
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
            1 => {
                let session = &candidates[0];

                // Try to claim this session (should succeed since we filtered)
                if !state_manager
                    .try_claim_session(&session.session_id, &instance_id)
                    .await
                {
                    // Race condition - another instance claimed it between filter and claim
                    debug!(
                        "Session {} was claimed by another instance (race), retrying...",
                        session.session_id
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    continue;
                }

                debug!("Found and claimed unique session: {}", session.session_id);

                // Update the instance handle with the session ID
                if let Some(handle) = state_manager.get_handle(&instance_id).await {
                    if let Err(e) = handle.set_session_id(session.session_id.clone()).await {
                        warn!(instance = %instance_id, session = %session.session_id, "Failed to set session ID: {}", e);
                    }
                }

                break session.session_id.clone();
            }
            n => {
                // Multiple UNCLAIMED sessions - this is truly ambiguous
                debug!(
                    "Found {} unclaimed candidate sessions for {}, asking user to select",
                    n, instance_id
                );

                let candidate_info: Vec<SessionCandidate> = candidates
                    .iter()
                    .map(|c| {
                        let preview = manager
                            .read_conversation(&working_dir, &c.session_id)
                            .ok()
                            .and_then(|convo| {
                                convo.user_messages().first().and_then(|entry| {
                                    entry.message.as_ref().and_then(|msg| match &msg.content {
                                        Some(claude_convo::MessageContent::Text(t)) => {
                                            Some(t.chars().take(100).collect())
                                        }
                                        Some(claude_convo::MessageContent::Parts(parts)) => {
                                            parts.iter().find_map(|p| match p {
                                                claude_convo::ContentPart::Text { text } => {
                                                    Some(text.chars().take(100).collect())
                                                }
                                                _ => None,
                                            })
                                        }
                                        None => None,
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

                if tx
                    .send(ServerMessage::SessionAmbiguous {
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
                            let selected_id: String = selected_id;
                            debug!("User selected session: {}", selected_id);

                            // Update the instance handle
                            if let Some(handle) = state_manager.get_handle(&instance_id).await {
                                if let Err(e) = handle.set_session_id(selected_id.clone()).await {
                                    warn!(instance = %instance_id, session = %selected_id, "Failed to set session ID: {}", e);
                                }
                            }

                            break selected_id;
                        }
                    }
                }
            }
        }
    };

    // Use the helper function to do the actual watching
    run_conversation_watcher_with_session(
        instance_id,
        working_dir,
        session_id,
        cancel,
        state_manager,
        tx,
        repository,
    )
    .await;
}

/// Helper function that runs the conversation watcher given a known session_id
pub async fn run_conversation_watcher_with_session(
    instance_id: String,
    working_dir: String,
    session_id: String,
    cancel: CancellationToken,
    state_manager: Arc<GlobalStateManager>,
    tx: mpsc::Sender<ServerMessage>,
    repository: Option<Arc<ConversationRepository>>,
) {
    debug!("Starting conversation watcher for session {}", session_id);

    let repo_ref = repository.as_deref();
    let manager = ClaudeConvo::new();
    let mut watcher = ConversationWatcher::new(manager, working_dir, session_id);

    // Send initial full conversation
    if let Ok(entries) = watcher.poll() {
        let mut turns = Vec::with_capacity(entries.len());
        for e in &entries {
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
        info!(
            "[CONVO-WATCHER {}] Sending ConversationFull with {} turns",
            instance_id,
            turns.len()
        );
        if tx
            .send(ServerMessage::ConversationFull {
                instance_id: instance_id.clone(),
                turns,
            })
            .await
            .is_err()
        {
            warn!(instance = %instance_id, "Failed to send initial ConversationFull - channel closed");
            return;
        }

        // Signal initial state from last entry
        if let Some(last) = entries.last() {
            let subtype = last
                .extra
                .get("subtype")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            state_manager
                .send_signal(
                    &instance_id,
                    StateSignal::ConversationEntry {
                        entry_type: last.entry_type.clone(),
                        subtype,
                        stop_reason: last.message.as_ref().and_then(|m| m.stop_reason.clone()),
                    },
                )
                .await;
        }
    }

    // Poll for updates
    let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(500));

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("Conversation watcher cancelled");
                break;
            }
            _ = interval.tick() => {
                match watcher.poll() {
                    Ok(new_entries) if !new_entries.is_empty() => {
                        debug!("Conversation watcher got {} new entries", new_entries.len());

                        // Signal state manager for each entry
                        for entry in &new_entries {
                            let subtype = entry
                                .extra
                                .get("subtype")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());

                            state_manager
                                .send_signal(
                                    &instance_id,
                                    StateSignal::ConversationEntry {
                                        entry_type: entry.entry_type.clone(),
                                        subtype,
                                        stop_reason: entry.message.as_ref().and_then(|m| m.stop_reason.clone()),
                                    },
                                )
                                .await;
                        }

                        let mut turns = Vec::with_capacity(new_entries.len());
                        for e in &new_entries {
                            turns.push(
                                crate::handlers::format_entry_with_attribution(e, &instance_id, repo_ref, Some(&state_manager)).await,
                            );
                        }

                        if !turns.is_empty() {
                            if tx.send(ServerMessage::ConversationUpdate {
                                instance_id: instance_id.clone(),
                                turns,
                            }).await.is_err() {
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
        }
    }
}

/// Background conversation watcher for state tracking only.
/// This runs continuously for each instance, independent of focus.
/// It only sends ConversationEntry signals - no UI messages.
///
/// IMPORTANT: This watcher does NOT claim sessions. It only watches sessions
/// that have already been claimed by the focused conversation watcher.
/// This prevents race conditions where the wrong instance claims a session.
pub async fn run_background_conversation_watcher(
    instance_id: String,
    working_dir: String,
    _created_at: DateTime<Utc>,
    cancel: CancellationToken,
    signal_tx: mpsc::Sender<StateSignal>,
    state_manager: Arc<GlobalStateManager>,
) {
    // Wait for the focused conversation watcher to claim a session for this instance
    // We check the instance handle's session_id which is set when a session is claimed
    let session_id = loop {
        if cancel.is_cancelled() {
            return;
        }

        // Check if this instance has a claimed session
        if let Some(handle) = state_manager.get_handle(&instance_id).await {
            if let Some(sid) = handle.get_session_id().await {
                debug!(
                    "Background state watcher using claimed session {} for instance {}",
                    sid, instance_id
                );
                break sid;
            }
        }

        // No session claimed yet, wait and retry
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    };

    debug!(
        "Starting background conversation watcher for instance {} session {}",
        instance_id, session_id
    );

    let manager = ClaudeConvo::new();
    let mut watcher = ConversationWatcher::new(manager, working_dir, session_id);

    // Process initial entries to set up state
    match watcher.poll() {
        Ok(entries) => {
            info!(
                "[BG-CONVO {}] Initial poll got {} entries",
                instance_id,
                entries.len()
            );
            if let Some(last) = entries.last() {
                let subtype = last
                    .extra
                    .get("subtype")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                info!(
                    "[BG-CONVO {}] Signaling initial entry: type={}, subtype={:?}, stop_reason={:?}",
                    instance_id,
                    last.entry_type,
                    subtype,
                    last.message.as_ref().and_then(|m| m.stop_reason.clone())
                );

                if signal_tx
                    .send(StateSignal::ConversationEntry {
                        entry_type: last.entry_type.clone(),
                        subtype,
                        stop_reason: last.message.as_ref().and_then(|m| m.stop_reason.clone()),
                    })
                    .await
                    .is_err()
                {
                    warn!(instance = %instance_id, "Failed to send initial state signal - channel closed");
                    return;
                }
            }
        }
        Err(e) => {
            warn!("[BG-CONVO {}] Initial poll failed: {}", instance_id, e);
        }
    }

    // Poll for updates continuously
    let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(500));

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("Background conversation watcher cancelled for instance {}", instance_id);
                break;
            }
            _ = interval.tick() => {
                match watcher.poll() {
                    Ok(new_entries) if !new_entries.is_empty() => {
                        info!(
                            "[BG-CONVO {}] Got {} new entries",
                            instance_id, new_entries.len()
                        );

                        // Send ConversationEntry signal for each entry
                        for entry in &new_entries {
                            let subtype = entry
                                .extra
                                .get("subtype")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());

                            info!(
                                "[BG-CONVO {}] Signaling: type={}, subtype={:?}, stop_reason={:?}",
                                instance_id,
                                entry.entry_type,
                                subtype,
                                entry.message.as_ref().and_then(|m| m.stop_reason.clone())
                            );

                            if signal_tx
                                .send(StateSignal::ConversationEntry {
                                    entry_type: entry.entry_type.clone(),
                                    subtype,
                                    stop_reason: entry.message.as_ref().and_then(|m| m.stop_reason.clone()),
                                })
                                .await
                                .is_err()
                            {
                                warn!("[BG-CONVO {}] Signal channel closed!", instance_id);
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        debug!("[BG-CONVO {}] Poll error: {}", instance_id, e);
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use claude_convo::ConversationEntry;

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
