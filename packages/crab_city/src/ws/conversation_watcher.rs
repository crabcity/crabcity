//! Conversation Watcher
//!
//! Server-owned conversation watcher: exactly one per instance.
//! Discovers the session, maintains formatted conversation data,
//! broadcasts updates to consumers, and feeds state signals for inference.

use chrono::{DateTime, Utc};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use toolpath_claude::ClaudeConvo;
use toolpath_convo::{ConversationProvider, Role, WatcherEvent};
use tracing::{debug, info, warn};

use crate::inference::StateSignal;
use crate::models::attribution_content_matches;
use crate::process_driver::DriverSignal;
use crate::repository::ConversationRepository;
use crate::ws::state_manager::{FirstInputData, PendingAttribution};

/// Determine if auto-claim should proceed based on content matching.
///
/// When `content_prefixes` is empty, returns `true` — the `first_input_at` gate
/// already ensures we only get here after user input, and `claimed_sessions`
/// prevents double-claiming. Content matching adds protection when prefixes ARE
/// available (multi-instance, same directory) but must not block when empty.
fn should_auto_claim(content_prefixes: &[String], candidate_user_texts: &[&str]) -> bool {
    if content_prefixes.is_empty() {
        return true; // fallback: first_input_at + claimed_sessions are sufficient
    }
    if candidate_user_texts.is_empty() {
        return true; // JSONL exists but no user turns yet — trust the timestamp gate
    }
    // Both populated: require a match
    candidate_user_texts.iter().any(|text| {
        content_prefixes
            .iter()
            .any(|prefix| attribution_content_matches(prefix, text))
    })
}

use super::session_discovery::find_candidate_sessions;
use super::state_manager::{ConversationBroadcast, ConversationEvent, GlobalStateManager};

/// Check if a state signal is a text-only assistant entry (no tool uses, no subtype).
/// These can be mid-turn explanatory text and need special handling to avoid
/// green flashes — see `docs/state-inference.md` §"Why Text-Only Assistant
/// Signals Are Deferred".
fn is_text_only_assistant(signal: &StateSignal) -> bool {
    matches!(
        signal,
        StateSignal::ConversationEntry {
            entry_type,
            tool_names,
            subtype: None,
            ..
        } if entry_type == "assistant" && tool_names.is_empty()
    )
}

/// Check if a watcher event is substantive (a conversation Turn or TurnUpdated,
/// as opposed to a Progress metadata event). Used to decide whether a held
/// text-only signal was mid-turn or turn-ending.
fn is_substantive_event(event: &WatcherEvent) -> bool {
    matches!(event, WatcherEvent::Turn(_) | WatcherEvent::TurnUpdated(_))
}

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
        WatcherEvent::TurnUpdated(turn) => {
            // TurnUpdated means a tool_result_only user entry was merged into
            // an assistant turn (cross-poll).
            //
            // Interactive tools (AskUserQuestion, EnterPlanMode, ExitPlanMode)
            // mean the user answered a prompt → emit "user" signal → Thinking.
            //
            // Non-interactive tools (Read, Bash, etc.) are mid-chain merges.
            // Emit "tool_result" so the state manager knows Claude is processing
            // input, but without causing a Thinking flash between tool calls.
            let has_interactive = turn.tool_uses.iter().any(|tu| {
                matches!(
                    tu.name.as_str(),
                    "AskUserQuestion" | "EnterPlanMode" | "ExitPlanMode"
                )
            });

            let entry_type = if has_interactive {
                "user"
            } else {
                "tool_result"
            };

            Some(StateSignal::ConversationEntry {
                entry_type: entry_type.to_string(),
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
                    // No content to verify — fall back to auto-claim.
                    // The first_input_at gate already ensures we only get here
                    // after user input, and claimed_sessions prevents double-claiming.
                    // Content matching adds protection when prefixes ARE available
                    // (multi-instance, same directory) but must not block when empty.
                    true
                } else {
                    // Load the conversation and check if any stored prefix
                    // matches any user turn's text.
                    match manager.load_conversation(&working_dir, &session.id) {
                        Ok(view) => {
                            let user_texts: Vec<&str> = view
                                .turns
                                .iter()
                                .filter(|turn| matches!(turn.role, toolpath_convo::Role::User))
                                .map(|turn| turn.text.as_str())
                                .collect();
                            should_auto_claim(&content_prefixes, &user_texts)
                        }
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
    let pending_attrs = state_manager.pending_attributions_lock();
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
                                Some(pending_attrs),
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

    // Text-only assistant signals are held for one poll cycle before being
    // sent. If the next poll brings substantive events (Turn/TurnUpdated),
    // the text was mid-turn — discard it. If the next poll is empty or has
    // only progress events, it was turn-ending — send it.
    // See docs/state-inference.md §"Why Text-Only Assistant Signals Are Deferred".
    let mut held_text_only: Option<StateSignal> = None;

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

                        // Resolve held text-only signal from previous poll.
                        // If substantive events arrived, the text was mid-turn.
                        // If only progress/metadata events, the text was turn-ending.
                        if let Some(held) = held_text_only.take() {
                            if events.iter().any(is_substantive_event) {
                                debug!(
                                    "[SERVER-CONVO {}] Discarding held text-only signal (new events arrived)",
                                    instance_id
                                );
                            } else if signal_tx.send(held).await.is_err() {
                                warn!("[SERVER-CONVO {}] State signal channel closed", instance_id);
                                return;
                            }
                        }

                        // Signal state manager for each event, with text-only
                        // assistant deferral to prevent mid-turn green flashes.
                        for (i, event) in events.iter().enumerate() {
                            if let Some(signal) = watcher_event_to_signal(event) {
                                if is_text_only_assistant(&signal) {
                                    // Suppress if a substantive event follows in this batch
                                    if events[i + 1..].iter().any(is_substantive_event) {
                                        debug!(
                                            "[SERVER-CONVO {}] Suppressing text-only assistant (followed by more events)",
                                            instance_id
                                        );
                                        continue;
                                    }
                                    // Last in batch — hold for next poll cycle
                                    debug!(
                                        "[SERVER-CONVO {}] Holding text-only assistant for next poll",
                                        instance_id
                                    );
                                    held_text_only = Some(signal);
                                    continue;
                                }
                                if signal_tx.send(signal).await.is_err() {
                                    warn!("[SERVER-CONVO {}] State signal channel closed", instance_id);
                                    return;
                                }
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
                                            Some(pending_attrs),
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
                                        Some(pending_attrs),
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
                    _ => {
                        // Empty poll — if we held a text-only signal, the turn
                        // is over (no more entries coming). Send it now.
                        if let Some(held) = held_text_only.take() {
                            debug!(
                                "[SERVER-CONVO {}] Sending held text-only signal (empty poll)",
                                instance_id
                            );
                            if signal_tx.send(held).await.is_err() {
                                warn!("[SERVER-CONVO {}] State signal channel closed", instance_id);
                                return;
                            }
                        }
                    }
                }
            }
        }
    }
}

// =========================================================================
// Driver-variant helpers (DriverSignal instead of StateSignal)
// =========================================================================

/// Check if a DriverSignal is a text-only assistant entry (same logic as is_text_only_assistant).
fn is_text_only_driver_signal(signal: &DriverSignal) -> bool {
    matches!(
        signal,
        DriverSignal::ConversationEntry {
            entry_type,
            tool_names,
            subtype: None,
            ..
        } if entry_type == "assistant" && tool_names.is_empty()
    )
}

/// Extract a DriverSignal from a WatcherEvent.
fn watcher_event_to_driver_signal(event: &WatcherEvent) -> Option<DriverSignal> {
    match event {
        WatcherEvent::Turn(turn) => {
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

            Some(DriverSignal::ConversationEntry {
                entry_type,
                subtype,
                stop_reason,
                tool_names,
            })
        }
        WatcherEvent::TurnUpdated(turn) => {
            let has_interactive = turn.tool_uses.iter().any(|tu| {
                matches!(
                    tu.name.as_str(),
                    "AskUserQuestion" | "EnterPlanMode" | "ExitPlanMode"
                )
            });
            let entry_type = if has_interactive {
                "user"
            } else {
                "tool_result"
            };
            Some(DriverSignal::ConversationEntry {
                entry_type: entry_type.to_string(),
                subtype: None,
                stop_reason: None,
                tool_names: vec![],
            })
        }
        WatcherEvent::Progress { kind, data } => {
            let subtype = data
                .get("subtype")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            Some(DriverSignal::ConversationEntry {
                entry_type: kind.clone(),
                subtype,
                stop_reason: None,
                tool_names: vec![],
            })
        }
    }
}

// =========================================================================
// Driver-owned conversation watcher
// =========================================================================

/// Driver-owned conversation watcher.
///
/// Same responsibilities as `run_server_conversation_watcher` but sends
/// `DriverSignal`s back to the driver instead of using the GlobalStateManager:
/// - Sends `DriverSignal::ConversationEntry` for state inference
/// - Sends `DriverSignal::SessionDiscovered` when a session is found
/// - Sends `DriverSignal::ConversationSnapshot`/`ConversationDelta` for conversation data
///
/// Session discovery and claiming operate on the shared `claimed_sessions`/
/// `first_input_data` maps directly (no GSM indirection).
#[allow(clippy::too_many_arguments)]
pub async fn run_driver_conversation_watcher(
    instance_id: String,
    working_dir: String,
    created_at: DateTime<Utc>,
    cancel: CancellationToken,
    driver_tx: mpsc::Sender<DriverSignal>,
    claimed_sessions: Arc<RwLock<HashMap<String, String>>>,
    first_input_data: Arc<RwLock<HashMap<String, FirstInputData>>>,
    pending_attributions: Arc<RwLock<HashMap<String, VecDeque<PendingAttribution>>>>,
    repository: Option<Arc<ConversationRepository>>,
) {
    let manager = ClaudeConvo::new();

    // Phase 1: Discover the session.
    let mut discovery_attempts = 0u32;
    let session_id = loop {
        if cancel.is_cancelled() {
            return;
        }

        // Check if a session has already been claimed (by focus path or prior run)
        {
            let claimed = claimed_sessions.read().await;
            if let Some(owner) = claimed.values().find(|v| **v == instance_id) {
                // Find the session_id that maps to this instance
                if let Some((sid, _)) = claimed.iter().find(|(_, v)| *v == owner) {
                    debug!(
                        "[DRIVER-CONVO {}] Using already-claimed session {}",
                        instance_id, sid
                    );
                    break sid.clone();
                }
            }
        }

        discovery_attempts += 1;

        // Only attempt auto-discovery after first input is detected.
        let has_first_input = first_input_data
            .read()
            .await
            .get(&instance_id)
            .map(|d| d.timestamp)
            .is_some();

        if !has_first_input {
            if discovery_attempts.is_multiple_of(30) {
                debug!(
                    "[DRIVER-CONVO {}] Waiting for first input before session discovery (attempt {})",
                    instance_id, discovery_attempts
                );
            }
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            continue;
        }

        let claimed_set: std::collections::HashSet<String> =
            claimed_sessions.read().await.keys().cloned().collect();
        let candidates: Vec<_> = find_candidate_sessions(&manager, &working_dir, created_at)
            .into_iter()
            .filter(|c| !claimed_set.contains(&c.id))
            .collect();

        match candidates.len() {
            0 => {
                if discovery_attempts.is_multiple_of(10) {
                    info!(
                        "[DRIVER-CONVO {}] No candidate sessions after {} attempts",
                        instance_id, discovery_attempts
                    );
                } else {
                    debug!("[DRIVER-CONVO {}] No candidate sessions", instance_id);
                }
            }
            1 => {
                let session = &candidates[0];

                // Content-match gate
                let content_prefixes = first_input_data
                    .read()
                    .await
                    .get(&instance_id)
                    .map(|d| d.content_prefixes.clone())
                    .unwrap_or_default();

                let content_ok = if content_prefixes.is_empty() {
                    true
                } else {
                    match manager.load_conversation(&working_dir, &session.id) {
                        Ok(view) => {
                            let user_texts: Vec<&str> = view
                                .turns
                                .iter()
                                .filter(|turn| matches!(turn.role, toolpath_convo::Role::User))
                                .map(|turn| turn.text.as_str())
                                .collect();
                            should_auto_claim(&content_prefixes, &user_texts)
                        }
                        Err(e) => {
                            debug!(
                                "[DRIVER-CONVO {}] Failed to load candidate session {}: {}",
                                instance_id, session.id, e
                            );
                            false
                        }
                    }
                };

                if content_ok {
                    // Try to claim
                    let mut claimed = claimed_sessions.write().await;
                    if !claimed.contains_key(&session.id) {
                        claimed.insert(session.id.clone(), instance_id.clone());
                        info!(
                            "[DRIVER-CONVO {}] Auto-claimed session {}",
                            instance_id, session.id
                        );
                        drop(claimed);
                        // Tell the driver about the discovered session
                        if driver_tx
                            .send(DriverSignal::SessionDiscovered(session.id.clone()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                        break session.id.clone();
                    }
                }
            }
            n => {
                if discovery_attempts.is_multiple_of(10) {
                    warn!(
                        "[DRIVER-CONVO {}] {} ambiguous candidates, waiting for user selection",
                        instance_id, n
                    );
                } else {
                    info!("[DRIVER-CONVO {}] {} ambiguous candidates", instance_id, n);
                }
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    };

    // Phase 2: Watch the conversation.
    let repo_ref = repository.as_ref();
    let pending_attr_ref = &*pending_attributions;
    let mut watcher = super::merging_watcher::MergingWatcher::new(manager, working_dir, session_id);
    let mut conversation_turns: Vec<serde_json::Value> = Vec::new();

    info!(
        "[DRIVER-CONVO {}] Starting conversation watcher for session {} (project={})",
        instance_id,
        watcher.session_id(),
        watcher.project()
    );

    // Initial poll
    match toolpath_convo::ConversationWatcher::poll(&mut watcher) {
        Ok(events) => {
            info!(
                "[DRIVER-CONVO {}] Initial poll: {} events",
                instance_id,
                events.len()
            );

            let mut turns = Vec::new();
            for event in &events {
                match event {
                    WatcherEvent::Turn(turn) => {
                        turns.push(
                            crate::handlers::format_turn_with_attribution(
                                turn,
                                &instance_id,
                                repo_ref,
                                Some(pending_attr_ref),
                            )
                            .await,
                        );
                    }
                    WatcherEvent::TurnUpdated(_) => {}
                    WatcherEvent::Progress { kind, data } => {
                        if kind != "system" {
                            turns.push(crate::handlers::format_progress_event(kind, data));
                        }
                    }
                }
            }

            conversation_turns = turns.clone();

            if driver_tx
                .send(DriverSignal::ConversationSnapshot(turns))
                .await
                .is_err()
            {
                return;
            }

            // Signal initial state from last event
            if let Some(last) = events.last()
                && let Some(signal) = watcher_event_to_driver_signal(last)
                && driver_tx.send(signal).await.is_err()
            {
                warn!(instance = %instance_id, "Driver signal channel closed on initial poll");
                return;
            }
        }
        Err(e) => {
            warn!("[DRIVER-CONVO {}] Initial poll failed: {}", instance_id, e);
        }
    }

    // Poll loop
    let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(500));
    let mut held_text_only: Option<DriverSignal> = None;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("[DRIVER-CONVO {}] Cancelled", instance_id);
                break;
            }
            _ = interval.tick() => {
                match toolpath_convo::ConversationWatcher::poll(&mut watcher) {
                    Ok(events) if !events.is_empty() => {
                        debug!(
                            "[DRIVER-CONVO {}] {} new events",
                            instance_id, events.len()
                        );

                        // Resolve held text-only signal
                        if let Some(held) = held_text_only.take() {
                            if events.iter().any(is_substantive_event) {
                                debug!(
                                    "[DRIVER-CONVO {}] Discarding held text-only signal (new events arrived)",
                                    instance_id
                                );
                            } else if driver_tx.send(held).await.is_err() {
                                return;
                            }
                        }

                        // Signal driver for each event
                        for (i, event) in events.iter().enumerate() {
                            if let Some(signal) = watcher_event_to_driver_signal(event) {
                                if is_text_only_driver_signal(&signal) {
                                    if events[i + 1..].iter().any(is_substantive_event) {
                                        continue;
                                    }
                                    held_text_only = Some(signal);
                                    continue;
                                }
                                if driver_tx.send(signal).await.is_err() {
                                    return;
                                }
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
                                            Some(pending_attr_ref),
                                        )
                                        .await,
                                    );
                                }
                                WatcherEvent::TurnUpdated(turn) => {
                                    had_update = true;
                                    let formatted =
                                        crate::handlers::format_turn_with_attribution(
                                            turn,
                                            &instance_id,
                                            repo_ref,
                                            Some(pending_attr_ref),
                                        )
                                        .await;
                                    if let Some(pos) = conversation_turns.iter().position(|t| {
                                        t.get("uuid").and_then(|v| v.as_str()) == Some(&turn.id)
                                    }) {
                                        conversation_turns[pos] = formatted;
                                    } else {
                                        conversation_turns.push(formatted);
                                    }
                                }
                                WatcherEvent::Progress { kind, data } => {
                                    if kind != "system" {
                                        new_turns
                                            .push(crate::handlers::format_progress_event(kind, data));
                                    }
                                }
                            }
                        }

                        if !new_turns.is_empty() {
                            conversation_turns.extend(new_turns.clone());
                        }

                        // Send conversation data to driver
                        if had_update {
                            if driver_tx
                                .send(DriverSignal::ConversationSnapshot(
                                    conversation_turns.clone(),
                                ))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        } else if !new_turns.is_empty() {
                            if driver_tx
                                .send(DriverSignal::ConversationDelta(new_turns))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }

                        // Handle session rotations
                        let rotations = watcher.take_pending_rotations();
                        for (_from_session, to_session) in &rotations {
                            info!(
                                "[DRIVER-CONVO {}] Session rotated to {}",
                                instance_id, to_session
                            );
                            // Claim new session
                            claimed_sessions
                                .write()
                                .await
                                .insert(to_session.clone(), instance_id.clone());
                            // Tell driver about the new session
                            if driver_tx
                                .send(DriverSignal::SessionDiscovered(to_session.clone()))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        warn!("[DRIVER-CONVO {}] Poll error: {}", instance_id, e);
                    }
                    _ => {
                        // Empty poll — send held text-only if any
                        if let Some(held) = held_text_only.take() {
                            if driver_tx.send(held).await.is_err() {
                                return;
                            }
                        }
                    }
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
    fn turn_updated_interactive_produces_user_signal() {
        // TurnUpdated with interactive tools means the user answered a
        // prompt (e.g. AskUserQuestion) → emit "user" signal → Thinking.
        let mut turn = make_turn("a1", Role::Assistant);
        turn.tool_uses = vec![toolpath_convo::ToolInvocation {
            id: "tu1".to_string(),
            name: "AskUserQuestion".to_string(),
            input: json!({}),
            result: None,
            category: None,
        }];
        let event = WatcherEvent::TurnUpdated(Box::new(turn));
        let signal = watcher_event_to_signal(&event).unwrap();
        match signal {
            StateSignal::ConversationEntry { entry_type, .. } => {
                assert_eq!(
                    entry_type, "user",
                    "TurnUpdated with interactive tool must produce 'user' signal"
                );
            }
            _ => panic!("Expected ConversationEntry"),
        }
    }

    #[test]
    fn turn_updated_non_interactive_produces_tool_result_signal() {
        // TurnUpdated with only non-interactive tools is a mid-chain
        // tool result merge → "tool_result" signal (not "user", to avoid
        // Thinking flash between tool calls).
        let mut turn = make_turn("a1", Role::Assistant);
        turn.tool_uses = vec![toolpath_convo::ToolInvocation {
            id: "tu1".to_string(),
            name: "Read".to_string(),
            input: json!({}),
            result: None,
            category: None,
        }];
        let event = WatcherEvent::TurnUpdated(Box::new(turn));
        let signal = watcher_event_to_signal(&event).unwrap();
        match signal {
            StateSignal::ConversationEntry { entry_type, .. } => {
                assert_eq!(
                    entry_type, "tool_result",
                    "Non-interactive TurnUpdated must produce 'tool_result' signal"
                );
            }
            _ => panic!("Expected ConversationEntry"),
        }
    }

    // ── is_text_only_assistant helper ─────────────────────────────────

    #[test]
    fn text_only_assistant_detected() {
        let signal = StateSignal::ConversationEntry {
            entry_type: "assistant".to_string(),
            subtype: None,
            stop_reason: Some("end_turn".to_string()),
            tool_names: vec![],
        };
        assert!(is_text_only_assistant(&signal));
    }

    #[test]
    fn assistant_with_tools_not_text_only() {
        let signal = StateSignal::ConversationEntry {
            entry_type: "assistant".to_string(),
            subtype: None,
            stop_reason: Some("end_turn".to_string()),
            tool_names: vec!["Read".to_string()],
        };
        assert!(!is_text_only_assistant(&signal));
    }

    #[test]
    fn user_signal_not_text_only_assistant() {
        let signal = StateSignal::ConversationEntry {
            entry_type: "user".to_string(),
            subtype: None,
            stop_reason: None,
            tool_names: vec![],
        };
        assert!(!is_text_only_assistant(&signal));
    }

    #[test]
    fn system_signal_not_text_only_assistant() {
        let signal = StateSignal::ConversationEntry {
            entry_type: "system".to_string(),
            subtype: Some("turn_duration".to_string()),
            stop_reason: None,
            tool_names: vec![],
        };
        assert!(!is_text_only_assistant(&signal));
    }

    #[test]
    fn terminal_output_not_text_only_assistant() {
        let signal = StateSignal::TerminalOutput {
            data: "hello".to_string(),
        };
        assert!(!is_text_only_assistant(&signal));
    }

    // ── is_substantive_event helper ──────────────────────────────────

    #[test]
    fn turn_is_substantive() {
        let event = WatcherEvent::Turn(Box::new(make_turn("u1", Role::User)));
        assert!(is_substantive_event(&event));
    }

    #[test]
    fn turn_updated_is_substantive() {
        let event = WatcherEvent::TurnUpdated(Box::new(make_turn("a1", Role::Assistant)));
        assert!(is_substantive_event(&event));
    }

    #[test]
    fn progress_is_not_substantive() {
        let event = WatcherEvent::Progress {
            kind: "progress".to_string(),
            data: json!({}),
        };
        assert!(!is_substantive_event(&event));
    }

    #[test]
    fn system_progress_is_not_substantive() {
        let event = WatcherEvent::Progress {
            kind: "system".to_string(),
            data: json!({"subtype": "turn_duration"}),
        };
        assert!(!is_substantive_event(&event));
    }

    // ── should_auto_claim content-match gate ────────────────────────

    #[test]
    fn empty_prefixes_auto_claims() {
        assert!(should_auto_claim(&[], &["anything"]));
    }

    #[test]
    fn matching_prefix_claims() {
        let prefixes = vec!["hello".to_string()];
        assert!(should_auto_claim(&prefixes, &["hello world"]));
    }

    #[test]
    fn no_matching_prefix_defers() {
        let prefixes = vec!["hello".to_string()];
        assert!(!should_auto_claim(&prefixes, &["goodbye"]));
    }

    #[test]
    fn no_user_turns_trusts_timestamp_gate() {
        // JSONL exists but no user turns written yet — trust the timestamp gate.
        // This is the critical fix: previously returned false, causing TUI-started
        // instances to never claim their session when the JSONL hadn't written
        // the first user turn yet at poll time.
        let prefixes = vec!["hello".to_string()];
        assert!(should_auto_claim(&prefixes, &[]));
    }

    #[test]
    fn single_char_prefix_matches() {
        let prefixes = vec!["h".to_string()];
        assert!(should_auto_claim(&prefixes, &["hello"]));
    }

    #[test]
    fn multiple_prefixes_any_match() {
        let prefixes = vec!["x".to_string(), "hello".to_string()];
        assert!(should_auto_claim(&prefixes, &["hello world"]));
    }

    // ── End-to-end TUI vs web content-matching pipeline ───────────────
    //
    // These tests exercise the full pipeline that the conversation watcher
    // uses for session claiming: content prefixes from mark_first_input
    // matched against user turn texts from the JSONL conversation.

    use super::super::state_manager::{GlobalStateManager, create_state_broadcast};

    /// Simulate TUI keystrokes: individual chars then Enter.
    /// Returns the content prefixes produced by mark_first_input.
    async fn tui_keystrokes(chars: &str) -> Vec<String> {
        let broadcast_tx = create_state_broadcast();
        let gsm = GlobalStateManager::new(broadcast_tx);
        for ch in chars.chars() {
            gsm.mark_first_input("inst-1", &ch.to_string()).await;
        }
        gsm.mark_first_input("inst-1", "\r").await;
        gsm.get_discovery_content_prefixes("inst-1").await
    }

    /// Simulate web composed message: full text then \r separately.
    /// This is what sendMessage/sendToInstance does on the web.
    async fn web_composed(text: &str) -> Vec<String> {
        let broadcast_tx = create_state_broadcast();
        let gsm = GlobalStateManager::new(broadcast_tx);
        gsm.mark_first_input("inst-1", text).await;
        gsm.mark_first_input("inst-1", "\r").await;
        gsm.get_discovery_content_prefixes("inst-1").await
    }

    #[tokio::test]
    async fn tui_simple_message_matches_jsonl() {
        let prefixes = tui_keystrokes("hello").await;
        // JSONL has: {"type":"human","message":{"role":"user","content":"hello"}}
        assert!(
            should_auto_claim(&prefixes, &["hello"]),
            "TUI 'hello' should match JSONL 'hello', got prefixes: {:?}",
            prefixes
        );
    }

    #[tokio::test]
    async fn web_composed_message_matches_jsonl() {
        let prefixes = web_composed("hello").await;
        assert!(
            should_auto_claim(&prefixes, &["hello"]),
            "Web composed 'hello' should match JSONL 'hello', got prefixes: {:?}",
            prefixes
        );
    }

    #[tokio::test]
    async fn tui_and_web_produce_same_prefixes() {
        let tui = tui_keystrokes("Fix the bug").await;
        let web = web_composed("Fix the bug").await;
        assert_eq!(
            tui, web,
            "TUI and web should produce identical content prefixes"
        );
    }

    #[tokio::test]
    async fn web_timing_empty_prefixes_before_enter() {
        // Simulates the web timing advantage: the watcher polls between
        // sendRaw("hello") and sendRaw("\r").
        // At poll time, pending_line="hello" but content_prefixes=[].
        let broadcast_tx = create_state_broadcast();
        let gsm = GlobalStateManager::new(broadcast_tx);
        gsm.mark_first_input("inst-1", "hello").await;
        // Watcher polls HERE — before \r arrives
        let prefixes = gsm.get_discovery_content_prefixes("inst-1").await;
        assert!(
            prefixes.is_empty(),
            "Before Enter, content_prefixes should be empty (pending_line not flushed)"
        );
        // Empty prefixes → auto-claim fallback
        assert!(
            should_auto_claim(&prefixes, &["hello"]),
            "Empty prefixes should auto-claim via fallback"
        );
    }

    #[tokio::test]
    async fn tui_prefix_matches_jsonl_with_trailing_context() {
        // Claude Code might append context to the user message.
        // Content prefix "hello" should match "hello\n<additional context>".
        let prefixes = tui_keystrokes("hello").await;
        let jsonl_text = "hello\n<system-reminder>some context</system-reminder>";
        assert!(
            should_auto_claim(&prefixes, &[jsonl_text]),
            "TUI prefix should match JSONL text with trailing context, prefixes: {:?}",
            prefixes
        );
    }

    #[tokio::test]
    async fn tui_prefix_vs_jsonl_with_leading_context() {
        // If Claude Code PREPENDS context to the user message, the prefix
        // won't match because neither is a prefix of the other.
        let prefixes = tui_keystrokes("hello").await;
        let jsonl_text = "<system-reminder>context</system-reminder>\nhello";
        let matches = should_auto_claim(&prefixes, &[jsonl_text]);
        // This documents the failure mode: leading context breaks matching.
        // If this assert fails, it means leading context IS the bug.
        assert!(
            !matches,
            "Leading context in JSONL should NOT match typed prefix — this IS the known failure mode"
        );
    }

    #[tokio::test]
    async fn tui_prefix_no_user_turns_yet() {
        // Watcher polls before Claude writes the user turn to JSONL.
        // The JSONL file exists (session candidate found) but has no user
        // turns yet — trust the timestamp gate and auto-claim.
        let prefixes = tui_keystrokes("hello").await;
        assert!(
            should_auto_claim(&prefixes, &[]),
            "No user turns yet → trust timestamp gate (JSONL still being written)"
        );
    }

    #[tokio::test]
    async fn tui_multiline_input() {
        // User types "line1\nline2" then Enter.
        // In the terminal, the user would type line1, then use some mechanism
        // to insert a newline (e.g., Shift+Enter which might send \n).
        let broadcast_tx = create_state_broadcast();
        let gsm = GlobalStateManager::new(broadcast_tx);
        for ch in "line1".chars() {
            gsm.mark_first_input("inst-1", &ch.to_string()).await;
        }
        gsm.mark_first_input("inst-1", "\n").await; // newline mid-message
        for ch in "line2".chars() {
            gsm.mark_first_input("inst-1", &ch.to_string()).await;
        }
        gsm.mark_first_input("inst-1", "\r").await; // final Enter

        let prefixes = gsm.get_discovery_content_prefixes("inst-1").await;
        // \n flushes "line1" as first prefix, \r flushes "line2" as second
        assert_eq!(
            prefixes.len(),
            2,
            "Expected two prefixes, got: {:?}",
            prefixes
        );
        assert_eq!(prefixes[0], "line1");
        assert_eq!(prefixes[1], "line2");

        // JSONL has the full multiline text
        let jsonl_text = "line1\nline2";
        assert!(
            should_auto_claim(&prefixes, &[jsonl_text]),
            "At least one prefix should match the multiline JSONL text"
        );
    }
}
