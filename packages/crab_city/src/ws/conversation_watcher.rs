//! Conversation Watcher
//!
//! Server-owned conversation watcher: exactly one per instance.
//! Discovers the session, maintains formatted conversation data,
//! broadcasts updates to consumers, and feeds state signals for inference.

use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use toolpath_claude::ClaudeConvo;
use toolpath_convo::{ConversationProvider, Role, WatcherEvent};
use tracing::{debug, info, warn};

use crate::inference::StateSignal;
use crate::models::attribution_content_matches;
use crate::repository::ConversationRepository;

use super::session_discovery::find_candidate_sessions;
use super::state_manager::{ConversationBroadcast, ConversationEvent, GlobalStateManager};

/// Extract a state signal from a WatcherEvent.
fn watcher_event_to_signal(event: &WatcherEvent) -> Option<StateSignal> {
    match event {
        WatcherEvent::Turn(turn) => {
            // Map roles to the entry_type strings the StateManager expects.
            // Claude Code JSONL uses "human" for user entries, but our state
            // manager checks for "user" (the conceptually clearer name).
            let entry_type = match &turn.role {
                Role::User => "user".to_string(),
                Role::Assistant => "assistant".to_string(),
                Role::System => "system".to_string(),
                Role::Other(s) => s.clone(),
            };
            let subtype = turn
                .extra
                .get("subtype")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            // Infer stop_reason when not explicitly set.
            // Claude Code JSONL always writes stop_reason: null (the API streaming
            // field isn't populated at write time).  Every assistant entry means
            // an API call completed — the model is no longer generating.  The
            // inference manager uses this to set tentative WaitingForInput, which
            // terminal heuristics can override for non-interactive tools.
            let stop_reason = turn.stop_reason.clone().or_else(|| {
                if matches!(turn.role, Role::Assistant) {
                    Some("end_turn".to_string())
                } else {
                    None
                }
            });

            let tool_names: Vec<String> = if matches!(turn.role, Role::Assistant) {
                turn.tool_uses.iter().map(|tu| tu.name.clone()).collect()
            } else {
                vec![]
            };

            Some(StateSignal::ConversationEntry {
                entry_type,
                subtype,
                stop_reason,
                tool_names,
            })
        }
        WatcherEvent::TurnUpdated(_) => {
            // TurnUpdated means a tool_result_only user entry was merged into
            // an assistant turn (cross-poll). This IS user input — emit a `user`
            // signal so the state machine transitions to Thinking.
            Some(StateSignal::ConversationEntry {
                entry_type: "user".to_string(),
                subtype: None,
                stop_reason: None,
                tool_names: vec![],
            })
        }
        WatcherEvent::Progress { kind, data } => {
            // Claude Code JSONL entries have "type" (→ entry_type → kind) and
            // "subtype" (→ entry.extra → data bag).  The top-level "type" field
            // was consumed into entry_type during parsing; what remains in the
            // Progress data is the "subtype" key from entry.extra.
            let subtype = data
                .get("subtype")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            Some(StateSignal::ConversationEntry {
                entry_type: kind.clone(),
                subtype,
                stop_reason: None,
                tool_names: vec![],
            })
        }
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
#[allow(clippy::too_many_arguments)]
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
    // Auto-discovery requires first_input_at — without input, Claude can't have
    // created a session for this instance, so any candidates would belong to
    // other instances sharing the same working directory.
    // The focus path can still override via set_session_id at any time.
    let mut discovery_attempts = 0u32;
    let session_id = loop {
        if cancel.is_cancelled() {
            return;
        }

        // Check if a session has already been claimed (by focus path or prior run)
        if let Some(handle) = state_manager.get_handle(&instance_id).await
            && let Some(sid) = handle.get_session_id().await
        {
            debug!(
                "[SERVER-CONVO {}] Using claimed session {}",
                instance_id, sid
            );
            break sid;
        }

        discovery_attempts += 1;

        // Only attempt auto-discovery after first input is detected.
        // Without input, Claude can't have created a session for this instance,
        // so any candidates found would belong to other instances.
        if state_manager
            .get_first_input_at(&instance_id)
            .await
            .is_none()
        {
            if discovery_attempts.is_multiple_of(30) {
                debug!(
                    "[SERVER-CONVO {}] Waiting for first input before session discovery (attempt {})",
                    instance_id, discovery_attempts
                );
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            continue;
        }

        // Use created_at as search_after: the session file may have been created
        // between instance creation and first input (e.g. Claude startup entries).
        let claimed_sessions = state_manager.get_claimed_sessions().await;
        let candidates: Vec<_> = find_candidate_sessions(&manager, &working_dir, created_at)
            .into_iter()
            .filter(|c| !claimed_sessions.contains(&c.id))
            .collect();

        match candidates.len() {
            0 => {
                if discovery_attempts.is_multiple_of(10) {
                    info!(
                        "[SERVER-CONVO {}] No candidate sessions after {} attempts (search_after={}, working_dir={})",
                        instance_id, discovery_attempts, created_at, working_dir
                    );
                } else {
                    debug!(
                        "[SERVER-CONVO {}] No candidate sessions (search_after={}, working_dir={})",
                        instance_id, created_at, working_dir
                    );
                }
            }
            1 => {
                let session = &candidates[0];

                // Content-match gate: verify this session contains input
                // we actually sent to *this* instance before auto-claiming.
                let content_prefixes = state_manager
                    .get_discovery_content_prefixes(&instance_id)
                    .await;

                let content_ok = if content_prefixes.is_empty() {
                    // No content recorded (unauthenticated user, control chars
                    // only, etc.) — fall back to timestamp-only auto-claim.
                    true
                } else {
                    // Load the conversation and check if any stored prefix
                    // matches any user turn's text.
                    match manager.load_conversation(&working_dir, &session.id) {
                        Ok(view) => view.turns.iter().any(|turn| {
                            matches!(turn.role, toolpath_convo::Role::User)
                                && content_prefixes
                                    .iter()
                                    .any(|prefix| attribution_content_matches(prefix, &turn.text))
                        }),
                        Err(e) => {
                            debug!(
                                "[SERVER-CONVO {}] Failed to load candidate session {} for content check: {}",
                                instance_id, session.id, e
                            );
                            // Can't verify — skip this cycle, retry next poll.
                            false
                        }
                    }
                };

                if content_ok
                    && state_manager
                        .try_claim_session(&session.id, &instance_id)
                        .await
                {
                    info!(
                        "[SERVER-CONVO {}] Auto-claimed session {} (content_verified={})",
                        instance_id,
                        session.id,
                        !content_prefixes.is_empty()
                    );
                    if let Some(handle) = state_manager.get_handle(&instance_id).await
                        && let Err(e) = handle.set_session_id(session.id.clone()).await
                    {
                        warn!(instance = %instance_id, session = %session.id, "Failed to set session ID: {}", e);
                    }
                    break session.id.clone();
                } else if !content_ok {
                    debug!(
                        "[SERVER-CONVO {}] Content mismatch for candidate session {}, retrying next poll",
                        instance_id, session.id
                    );
                }
            }
            n => {
                // Multiple candidates — let the user break the tie via the
                // focus path (SessionAmbiguous). Don't auto-pick.
                if discovery_attempts.is_multiple_of(10) {
                    warn!(
                        "[SERVER-CONVO {}] {} ambiguous candidates after {} attempts, waiting for user selection",
                        instance_id, n, discovery_attempts
                    );
                } else {
                    info!(
                        "[SERVER-CONVO {}] {} ambiguous candidates, waiting for user selection",
                        instance_id, n
                    );
                }
            }
        }

        // No session yet — wait and retry
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    };

    // Phase 2: Watch the conversation.
    let repo_ref = repository.as_ref();
    let mut watcher = super::merging_watcher::MergingWatcher::new(manager, working_dir, session_id);

    info!(
        "[SERVER-CONVO {}] Starting conversation watcher for session {} (project={})",
        instance_id,
        watcher.session_id(),
        watcher.project()
    );

    // Initial poll — load existing events (turns + progress)
    match toolpath_convo::ConversationWatcher::poll(&mut watcher) {
        Ok(events) => {
            info!(
                "[SERVER-CONVO {}] Initial poll: {} events",
                instance_id,
                events.len()
            );

            // Format events into JSON turns
            let mut turns = Vec::new();
            for event in &events {
                match event {
                    WatcherEvent::Turn(turn) => {
                        turns.push(
                            crate::handlers::format_turn_with_attribution(
                                turn,
                                &instance_id,
                                repo_ref,
                                Some(&state_manager),
                            )
                            .await,
                        );
                    }
                    WatcherEvent::TurnUpdated(_) => {
                        // Skip on initial poll — no prior state to update
                    }
                    WatcherEvent::Progress { kind, data } => {
                        // System metadata (turn_duration, init, etc.) is for state
                        // inference only — not conversation display.
                        if kind != "system" {
                            turns.push(crate::handlers::format_progress_event(kind, data));
                        }
                    }
                }
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

            // Signal initial state from last event
            if let Some(last) = events.last()
                && let Some(signal) = watcher_event_to_signal(last)
                && signal_tx.send(signal).await.is_err()
            {
                warn!(instance = %instance_id, "State signal channel closed on initial poll");
                return;
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
                match toolpath_convo::ConversationWatcher::poll(&mut watcher) {
                    Ok(events) if !events.is_empty() => {
                        debug!(
                            "[SERVER-CONVO {}] {} new events",
                            instance_id, events.len()
                        );

                        // Signal state manager for each event
                        for event in &events {
                            if let Some(signal) = watcher_event_to_signal(event)
                                && signal_tx.send(signal).await.is_err()
                            {
                                warn!("[SERVER-CONVO {}] State signal channel closed", instance_id);
                                return;
                            }
                        }

                        // Format new events into turns
                        let mut new_turns = Vec::new();
                        let mut had_update = false;
                        for event in &events {
                            match event {
                                WatcherEvent::Turn(turn) => {
                                    new_turns.push(
                                        crate::handlers::format_turn_with_attribution(
                                            turn,
                                            &instance_id,
                                            repo_ref,
                                            Some(&state_manager),
                                        )
                                        .await,
                                    );
                                }
                                WatcherEvent::TurnUpdated(turn) => {
                                    // Replace matching turn in store by id
                                    had_update = true;
                                    let formatted = crate::handlers::format_turn_with_attribution(
                                        turn,
                                        &instance_id,
                                        repo_ref,
                                        Some(&state_manager),
                                    )
                                    .await;
                                    let mut store = conversation_turns.write().await;
                                    if let Some(pos) = store.iter().position(|t| t.get("uuid").and_then(|v| v.as_str()) == Some(&turn.id)) {
                                        store[pos] = formatted;
                                    } else {
                                        // Turn not found in store — append
                                        store.push(formatted);
                                    }
                                }
                                WatcherEvent::Progress { kind, data } => {
                                    // System metadata → state inference only, not UI.
                                    if kind != "system" {
                                        new_turns.push(crate::handlers::format_progress_event(kind, data));
                                    }
                                }
                            }
                        }

                        // Append new turns to shared store
                        if !new_turns.is_empty() {
                            let mut store = conversation_turns.write().await;
                            store.extend(new_turns.clone());
                        }

                        // Broadcast: Full if we had an update (replace), Update for new turns only
                        if had_update {
                            let store = conversation_turns.read().await;
                            let _ = conversation_tx.send(ConversationEvent::Full {
                                instance_id: instance_id.clone(),
                                turns: store.clone(),
                            });
                        } else if !new_turns.is_empty() {
                            let _ = conversation_tx.send(ConversationEvent::Update {
                                instance_id: instance_id.clone(),
                                turns: new_turns,
                            });
                        }

                        // Handle session rotations (plan-mode exit, context overflow)
                        let rotations = watcher.take_pending_rotations();
                        for (from_session, to_session) in &rotations {
                            info!(
                                "[SERVER-CONVO {}] Session rotated: {} → {}",
                                instance_id, from_session, to_session
                            );

                            // Claim new session, release old
                            state_manager.try_claim_session(to_session, &instance_id).await;

                            // Update session_id on the instance handle
                            if let Some(handle) = state_manager.get_handle(&instance_id).await
                                && let Err(e) = handle.set_session_id(to_session.clone()).await
                            {
                                warn!(instance = %instance_id, "Failed to update session ID on rotation: {}", e);
                            }

                            // Broadcast rotation event to clients
                            state_manager.broadcast_lifecycle(
                                super::protocol::ServerMessage::SessionRotated {
                                    instance_id: instance_id.clone(),
                                    from_session: from_session.clone(),
                                    to_session: to_session.clone(),
                                },
                            );
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
        if let Some(handle) = state_manager.get_handle(&instance_id).await
            && handle.get_session_id().await.is_some()
        {
            debug!(
                "[SESSION-DISCOVERY {}] Session already claimed by server watcher",
                instance_id
            );
            return;
        }

        // Same guard as the server watcher: no input → no session to discover
        if state_manager
            .get_first_input_at(&instance_id)
            .await
            .is_none()
        {
            debug!(
                "[SESSION-DISCOVERY {}] No first input yet, waiting...",
                instance_id
            );
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            continue;
        }

        let search_after = created_at;

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
                            if let Some(handle) = state_manager.get_handle(&instance_id).await
                                && let Err(e) = handle.set_session_id(selected_id.clone()).await {
                                    warn!(instance = %instance_id, session = %selected_id, "Failed to set session ID: {}", e);
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
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use toolpath_convo::{Role, Turn};

    fn make_turn(id: &str, role: Role) -> Turn {
        Turn {
            id: id.to_string(),
            parent_id: None,
            role,
            timestamp: "2024-06-01T12:00:00Z".to_string(),
            text: String::new(),
            thinking: None,
            tool_uses: vec![],
            model: None,
            stop_reason: None,
            token_usage: None,
            environment: None,
            delegations: vec![],
            extra: HashMap::new(),
        }
    }

    // ── watcher_event_to_signal: Turn path ──────────────────────────

    #[test]
    fn signal_from_user_turn() {
        let event = WatcherEvent::Turn(Box::new(make_turn("u1", Role::User)));
        let signal = watcher_event_to_signal(&event).unwrap();
        match signal {
            StateSignal::ConversationEntry {
                entry_type,
                subtype,
                stop_reason,
                ..
            } => {
                assert_eq!(
                    entry_type, "user",
                    "Role::User must map to 'user' for the state manager"
                );
                assert!(subtype.is_none());
                assert!(stop_reason.is_none());
            }
            _ => panic!("Expected ConversationEntry"),
        }
    }

    #[test]
    fn signal_from_assistant_turn_with_end_turn() {
        let mut turn = make_turn("a1", Role::Assistant);
        turn.stop_reason = Some("end_turn".to_string());
        let event = WatcherEvent::Turn(Box::new(turn));
        let signal = watcher_event_to_signal(&event).unwrap();
        match signal {
            StateSignal::ConversationEntry {
                entry_type,
                stop_reason,
                ..
            } => {
                assert_eq!(entry_type, "assistant");
                assert_eq!(stop_reason.as_deref(), Some("end_turn"));
            }
            _ => panic!("Expected ConversationEntry"),
        }
    }

    #[test]
    fn signal_from_system_turn_with_subtype() {
        let mut turn = make_turn("s1", Role::System);
        turn.extra.insert(
            "subtype".to_string(),
            serde_json::Value::String("turn_duration".to_string()),
        );
        let event = WatcherEvent::Turn(Box::new(turn));
        let signal = watcher_event_to_signal(&event).unwrap();
        match signal {
            StateSignal::ConversationEntry {
                entry_type,
                subtype,
                ..
            } => {
                assert_eq!(entry_type, "system");
                assert_eq!(subtype.as_deref(), Some("turn_duration"));
            }
            _ => panic!("Expected ConversationEntry"),
        }
    }

    // ── watcher_event_to_signal: Progress path ──────────────────────
    //
    // This is the critical path: system metadata entries (turn_duration,
    // init, etc.) have no message field, so merging_watcher emits them
    // as Progress events.  The subtype must be extracted from the data
    // bag's "subtype" key — NOT "type" (which was consumed during parsing).

    #[test]
    fn signal_from_system_progress_turn_duration() {
        let event = WatcherEvent::Progress {
            kind: "system".to_string(),
            data: json!({
                "subtype": "turn_duration",
                "uuid": "td1",
                "timestamp": "2024-06-01T12:00:00Z",
                "durationMs": 1234,
            }),
        };
        let signal = watcher_event_to_signal(&event).unwrap();
        match signal {
            StateSignal::ConversationEntry {
                entry_type,
                subtype,
                stop_reason,
                ..
            } => {
                assert_eq!(entry_type, "system");
                assert_eq!(
                    subtype.as_deref(),
                    Some("turn_duration"),
                    "turn_duration subtype must be extracted from Progress data"
                );
                assert!(stop_reason.is_none());
            }
            _ => panic!("Expected ConversationEntry"),
        }
    }

    #[test]
    fn signal_from_system_progress_init() {
        let event = WatcherEvent::Progress {
            kind: "system".to_string(),
            data: json!({"subtype": "init", "uuid": "i1", "timestamp": "2024-06-01T12:00:00Z"}),
        };
        let signal = watcher_event_to_signal(&event).unwrap();
        match signal {
            StateSignal::ConversationEntry {
                entry_type,
                subtype,
                ..
            } => {
                assert_eq!(entry_type, "system");
                assert_eq!(subtype.as_deref(), Some("init"));
            }
            _ => panic!("Expected ConversationEntry"),
        }
    }

    #[test]
    fn signal_from_agent_progress_has_no_subtype() {
        let event = WatcherEvent::Progress {
            kind: "agent_progress".to_string(),
            data: json!({
                "uuid": "ap1",
                "timestamp": "2024-06-01T12:00:00Z",
                "data": {"type": "agent_progress", "agentId": "a1"},
            }),
        };
        let signal = watcher_event_to_signal(&event).unwrap();
        match signal {
            StateSignal::ConversationEntry {
                entry_type,
                subtype,
                ..
            } => {
                assert_eq!(entry_type, "agent_progress");
                assert!(subtype.is_none(), "agent_progress has no top-level subtype");
            }
            _ => panic!("Expected ConversationEntry"),
        }
    }

    // ── stop_reason inference from Turn content ────────────────────
    //
    // Claude Code JSONL always writes stop_reason: null (the API streaming
    // field isn't populated at write time).  We infer it from structure:
    // - Assistant with no tool_uses → end_turn
    // - Assistant with tool_uses → None (let terminal heuristics track)

    #[test]
    fn signal_from_assistant_null_stop_reason_no_tools_infers_end_turn() {
        // Real Claude Code format: stop_reason is always None in JSONL
        let turn = make_turn("a1", Role::Assistant);
        assert!(turn.stop_reason.is_none());
        assert!(turn.tool_uses.is_empty());

        let event = WatcherEvent::Turn(Box::new(turn));
        let signal = watcher_event_to_signal(&event).unwrap();
        match signal {
            StateSignal::ConversationEntry {
                entry_type,
                stop_reason,
                ..
            } => {
                assert_eq!(entry_type, "assistant");
                assert_eq!(
                    stop_reason.as_deref(),
                    Some("end_turn"),
                    "Assistant turn with no tool_uses and no stop_reason must infer end_turn"
                );
            }
            _ => panic!("Expected ConversationEntry"),
        }
    }

    #[test]
    fn signal_from_assistant_with_tools_passes_tool_names() {
        // Tool names from assistant entries are passed through for interactive
        // tool detection (AskUserQuestion → WaitingForInput).
        let mut turn = make_turn("a1", Role::Assistant);
        turn.tool_uses = vec![toolpath_convo::ToolInvocation {
            id: "tu1".to_string(),
            name: "Read".to_string(),
            input: json!({"path": "/foo"}),
            result: None,
            category: None,
        }];

        let event = WatcherEvent::Turn(Box::new(turn));
        let signal = watcher_event_to_signal(&event).unwrap();
        match signal {
            StateSignal::ConversationEntry {
                entry_type,
                tool_names,
                ..
            } => {
                assert_eq!(entry_type, "assistant");
                assert_eq!(tool_names, vec!["Read".to_string()]);
            }
            _ => panic!("Expected ConversationEntry"),
        }
    }

    #[test]
    fn signal_from_assistant_with_ask_user_question_passes_tool_name() {
        let mut turn = make_turn("a1", Role::Assistant);
        turn.tool_uses = vec![toolpath_convo::ToolInvocation {
            id: "tu1".to_string(),
            name: "AskUserQuestion".to_string(),
            input: json!({"question": "Which?"}),
            result: None,
            category: None,
        }];

        let event = WatcherEvent::Turn(Box::new(turn));
        let signal = watcher_event_to_signal(&event).unwrap();
        match signal {
            StateSignal::ConversationEntry { tool_names, .. } => {
                assert_eq!(tool_names, vec!["AskUserQuestion".to_string()]);
            }
            _ => panic!("Expected ConversationEntry"),
        }
    }

    #[test]
    fn signal_from_user_turn_does_not_infer_stop_reason() {
        // User turns should never get inferred stop_reason
        let turn = make_turn("u1", Role::User);
        let event = WatcherEvent::Turn(Box::new(turn));
        let signal = watcher_event_to_signal(&event).unwrap();
        match signal {
            StateSignal::ConversationEntry { stop_reason, .. } => {
                assert!(stop_reason.is_none());
            }
            _ => panic!("Expected ConversationEntry"),
        }
    }

    #[test]
    fn explicit_stop_reason_takes_precedence_over_inference() {
        // If stop_reason IS set (hypothetical future JSONL format), use it as-is
        let mut turn = make_turn("a1", Role::Assistant);
        turn.stop_reason = Some("max_tokens".to_string());
        // Even with no tool_uses, explicit stop_reason wins
        let event = WatcherEvent::Turn(Box::new(turn));
        let signal = watcher_event_to_signal(&event).unwrap();
        match signal {
            StateSignal::ConversationEntry { stop_reason, .. } => {
                assert_eq!(stop_reason.as_deref(), Some("max_tokens"));
            }
            _ => panic!("Expected ConversationEntry"),
        }
    }

    // ── TurnUpdated produces user signal (tool_result merge) ───────

    #[test]
    fn turn_updated_produces_user_signal() {
        // TurnUpdated means a tool_result_only user entry was merged into
        // an assistant turn. This IS user input, so emit a user signal.
        let event = WatcherEvent::TurnUpdated(Box::new(make_turn("a1", Role::Assistant)));
        let signal = watcher_event_to_signal(&event).unwrap();
        match signal {
            StateSignal::ConversationEntry {
                entry_type,
                subtype,
                stop_reason,
                ..
            } => {
                assert_eq!(
                    entry_type, "user",
                    "TurnUpdated must produce a 'user' signal (tool_result merge)"
                );
                assert!(subtype.is_none());
                assert!(stop_reason.is_none());
            }
            _ => panic!("Expected ConversationEntry"),
        }
    }
}
