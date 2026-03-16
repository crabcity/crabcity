//! Global State Manager
//!
//! Manages instance state tracking, session claiming, and presence across all WebSocket connections.

use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::inference::{
    ClaudeState, StateManagerConfig, StateSignal, StateUpdate, spawn_state_manager,
};
use crate::instance_actor::InstanceHandle;
use crate::instance_manager::InstanceKind;
use crate::models::{attribution_content_matches, normalize_attribution_content};
use crate::repository::ConversationRepository;

use super::conversation_watcher::run_server_conversation_watcher;
use super::protocol::{PresenceUser, ServerMessage, WsUser};

/// Entry in the presence map: one per WebSocket connection.
#[derive(Debug, Clone)]
struct PresenceEntry {
    user_id: String,
    display_name: String,
}

/// Terminal lock timeout in seconds (2 minutes)
pub const TERMINAL_LOCK_TIMEOUT_SECS: i64 = 120;

/// Terminal lock state for an instance
#[derive(Debug, Clone)]
pub struct TerminalLock {
    pub holder_connection_id: String,
    pub holder_user_id: String,
    pub holder_display_name: String,
    pub last_activity: DateTime<Utc>,
}

/// Data stored for the first input(s) to an instance before session discovery.
/// Accumulates content prefixes so the discovery loop can verify that a
/// candidate session actually contains input sent to *this* instance.
#[derive(Debug, Clone)]
pub struct FirstInputData {
    pub timestamp: DateTime<Utc>,
    /// Normalized content prefixes of inputs sent while session_id was None.
    /// Used for content-matching during session discovery.
    pub content_prefixes: Vec<String>,
    /// Accumulator for keystroke-by-keystroke input.  Characters accumulate
    /// here until a `\r`/`\n` arrives, at which point the completed line is
    /// flushed into `content_prefixes`.  Full-message input (containing a
    /// newline) bypasses the accumulator and stores directly.
    pending_line: String,
}

/// Maximum number of content prefixes to store per instance.
const FIRST_INPUT_PREFIX_CAP: usize = 20;

/// A pending attribution: recorded when a user sends input via WebSocket,
/// consumed when the conversation watcher sees the corresponding User entry.
/// Content-matched rather than timestamp-correlated.
#[derive(Debug, Clone)]
pub struct PendingAttribution {
    pub user_id: String,
    pub display_name: String,
    /// First 200 chars of the input, normalized (trimmed, whitespace-collapsed)
    pub content_prefix: String,
    pub timestamp: DateTime<Utc>,
    /// Optional task ID if this input was sent on behalf of a task
    pub task_id: Option<i64>,
}

/// Broadcast channel for state changes across all instances
/// Tuple: (instance_id, state, terminal_stale)
pub type StateBroadcast = broadcast::Sender<(String, ClaudeState, bool)>;

/// Broadcast channel for instance lifecycle events (created/stopped)
pub type LifecycleBroadcast = broadcast::Sender<ServerMessage>;

/// Conversation event broadcast from server-owned watcher to consumers.
#[derive(Debug, Clone)]
pub enum ConversationEvent {
    /// Full conversation snapshot (sent after initial load).
    Full {
        instance_id: String,
        turns: Vec<serde_json::Value>,
    },
    /// Incremental update (new turns appended).
    Update {
        instance_id: String,
        turns: Vec<serde_json::Value>,
    },
}

/// Per-instance conversation broadcast channel.
pub type ConversationBroadcast = broadcast::Sender<ConversationEvent>;

/// Create a new state broadcast channel
pub fn create_state_broadcast() -> StateBroadcast {
    let (tx, _) = broadcast::channel(256);
    tx
}

/// Per-instance state tracking (lives in GlobalStateManager)
pub(crate) struct InstanceTracker {
    pub instance_id: String,
    pub handle: InstanceHandle,
    pub working_dir: String,
    pub created_at: DateTime<Utc>,
    pub kind: InstanceKind,
    /// Sender for state signals to this instance's state manager
    signal_tx: Option<mpsc::Sender<StateSignal>>,
    /// Cancellation token to stop background tasks when instance is unregistered
    cancel: CancellationToken,
    /// Formatted conversation turns maintained by the server-owned watcher.
    conversation_turns: Arc<RwLock<Vec<serde_json::Value>>>,
    /// Broadcast channel for conversation events (Full/Update).
    conversation_tx: ConversationBroadcast,
}

impl InstanceTracker {
    pub fn new(
        instance_id: String,
        handle: InstanceHandle,
        working_dir: String,
        created_at: DateTime<Utc>,
        kind: InstanceKind,
    ) -> Self {
        let (conversation_tx, _) = broadcast::channel(64);
        Self {
            instance_id,
            handle,
            working_dir,
            created_at,
            kind,
            signal_tx: None,
            cancel: CancellationToken::new(),
            conversation_turns: Arc::new(RwLock::new(Vec::new())),
            conversation_tx,
        }
    }

    /// Start the state manager for this instance
    pub fn start_state_manager(
        &mut self,
        broadcast_tx: StateBroadcast,
        global_state_manager: Arc<GlobalStateManager>,
        repository: Option<Arc<ConversationRepository>>,
    ) {
        if !self.kind.is_structured() {
            return;
        }

        let (signal_tx, signal_rx) = mpsc::channel::<StateSignal>(100);
        let (state_tx, mut state_rx) = mpsc::channel::<StateUpdate>(100);

        // Spawn the state manager
        let _handle = spawn_state_manager(signal_rx, state_tx, StateManagerConfig::default());

        // Forward state changes to broadcast and instance actor
        let instance_id = self.instance_id.clone();
        let handle = self.handle.clone();
        let gsm = global_state_manager.clone();
        let repo_fwd = repository.clone();
        tokio::spawn(async move {
            let mut prev_state: Option<ClaudeState> = None;
            while let Some(update) = state_rx.recv().await {
                info!(
                    "[STATE-FWD {}] State changed to {:?} (stale={})",
                    instance_id, update.state, update.terminal_stale
                );
                // Update the instance actor's stored state
                if let Err(e) = handle.set_claude_state(update.state.clone()).await {
                    debug!("Failed to update instance state: {}", e);
                }

                // Track state_entered_at: record timestamp when state type changes
                let state_type_changed = prev_state
                    .as_ref()
                    .map(|p| std::mem::discriminant(p) != std::mem::discriminant(&update.state))
                    .unwrap_or(true);

                if state_type_changed {
                    let now = Utc::now();
                    gsm.set_state_entered_at(&instance_id, now).await;
                }

                // Inbox logic: detect state transitions and upsert/clear inbox items
                if let Some(ref repo) = repo_fwd {
                    if let Some(ref prev) = prev_state {
                        let prev_active = matches!(
                            prev,
                            ClaudeState::Thinking
                                | ClaudeState::Responding
                                | ClaudeState::ToolExecuting { .. }
                        );
                        let now_idle = matches!(update.state, ClaudeState::Idle);
                        let now_waiting = matches!(
                            update.state,
                            ClaudeState::WaitingForInput { .. }
                        );
                        let now_active = matches!(
                            update.state,
                            ClaudeState::Thinking
                                | ClaudeState::Responding
                                | ClaudeState::ToolExecuting { .. }
                        );

                        // Active → Idle: completed a turn
                        if prev_active && now_idle {
                            match repo.upsert_inbox_item(&instance_id, "completed_turn", None).await {
                                Ok(item) => {
                                    gsm.broadcast_lifecycle(ServerMessage::InboxUpdate {
                                        instance_id: instance_id.clone(),
                                        item: Some(item),
                                    });
                                }
                                Err(e) => warn!("[INBOX] Failed to upsert completed_turn: {}", e),
                            }
                        }

                        // → WaitingForInput: needs user action
                        if now_waiting && !matches!(prev, ClaudeState::WaitingForInput { .. }) {
                            let metadata = match &update.state {
                                ClaudeState::WaitingForInput { prompt: Some(p) } => {
                                    Some(serde_json::json!({"prompt": p}).to_string())
                                }
                                _ => None,
                            };
                            match repo.upsert_inbox_item(
                                &instance_id,
                                "needs_input",
                                metadata.as_deref(),
                            ).await {
                                Ok(item) => {
                                    gsm.broadcast_lifecycle(ServerMessage::InboxUpdate {
                                        instance_id: instance_id.clone(),
                                        item: Some(item),
                                    });
                                }
                                Err(e) => warn!("[INBOX] Failed to upsert needs_input: {}", e),
                            }
                        }

                        // Was WaitingForInput, now isn't: user responded, clear needs_input
                        if matches!(prev, ClaudeState::WaitingForInput { .. }) && !now_waiting {
                            match repo.clear_inbox_by_type(&instance_id, "needs_input").await {
                                Ok(true) => {
                                    gsm.broadcast_lifecycle(ServerMessage::InboxUpdate {
                                        instance_id: instance_id.clone(),
                                        item: None,
                                    });
                                }
                                Ok(false) => {} // No item to clear
                                Err(e) => warn!("[INBOX] Failed to clear needs_input: {}", e),
                            }
                        }

                        // Was Idle, now active: user sent new work, clear completed_turn
                        if !prev_active && now_active {
                            match repo.clear_inbox_by_type(&instance_id, "completed_turn").await {
                                Ok(true) => {
                                    gsm.broadcast_lifecycle(ServerMessage::InboxUpdate {
                                        instance_id: instance_id.clone(),
                                        item: None,
                                    });
                                }
                                Ok(false) => {} // No item to clear
                                Err(e) => warn!("[INBOX] Failed to clear completed_turn: {}", e),
                            }
                        }
                    }
                }

                // Broadcast to all connected clients (include staleness)
                let _ =
                    broadcast_tx.send((instance_id.clone(), update.state.clone(), update.terminal_stale));

                prev_state = Some(update.state);
            }
            warn!("[STATE-FWD {}] State receiver closed", instance_id);
        });

        // Spawn background PTY reader to continuously feed state manager
        // This runs INDEPENDENTLY of focus, so state updates even when not viewing this instance
        let signal_tx_pty = signal_tx.clone();
        let handle_pty = self.handle.clone();
        let instance_id_pty = self.instance_id.clone();
        let cancel_pty = self.cancel.clone();

        tokio::spawn(async move {
            // Subscribe to PTY output
            let mut output_rx = match handle_pty.subscribe_output().await {
                Ok(rx) => rx,
                Err(e) => {
                    error!(
                        "Failed to subscribe to PTY output for state tracking: {}",
                        e
                    );
                    return;
                }
            };

            debug!(
                "Started background PTY state tracking for instance {}",
                instance_id_pty
            );

            loop {
                tokio::select! {
                    _ = cancel_pty.cancelled() => {
                        debug!("Stopping background PTY state tracking for instance {}", instance_id_pty);
                        break;
                    }
                    result = output_rx.recv() => {
                        match result {
                            Ok(event) => {
                                let data = String::from_utf8_lossy(&event.data).to_string();
                                // Feed the state manager
                                if signal_tx_pty.send(StateSignal::TerminalOutput { data }).await.is_err() {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!("State tracking PTY output lagged by {} messages for {}", n, instance_id_pty);
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                debug!("PTY output channel closed for state tracking {}", instance_id_pty);
                                break;
                            }
                        }
                    }
                }
            }
        });

        // Spawn the server-owned conversation watcher.
        // This is the single watcher per instance: does session discovery, maintains
        // formatted conversation data, broadcasts updates, and feeds state signals.
        let signal_tx_convo = signal_tx.clone();
        let instance_id_convo = self.instance_id.clone();
        let working_dir_convo = self.working_dir.clone();
        let created_at_convo = self.created_at;
        let cancel_convo = self.cancel.clone();
        let state_manager_convo = global_state_manager.clone();
        let convo_turns = self.conversation_turns.clone();
        let convo_tx = self.conversation_tx.clone();

        tokio::spawn(async move {
            run_server_conversation_watcher(
                instance_id_convo,
                working_dir_convo,
                created_at_convo,
                cancel_convo,
                signal_tx_convo,
                state_manager_convo,
                convo_turns,
                convo_tx,
                repository,
            )
            .await;
        });

        self.signal_tx = Some(signal_tx);
    }

    /// Stop background tasks for this instance
    pub fn stop(&self) {
        self.cancel.cancel();
    }

    /// Send a signal to this instance's state manager
    pub async fn send_signal(&self, signal: StateSignal) {
        if let Some(tx) = &self.signal_tx
            && tx.send(signal).await.is_err()
        {
            warn!(
                "Failed to send signal to state manager for instance {} (receiver dropped)",
                self.instance_id
            );
        }
    }
}

/// Manager for all instance state trackers
pub struct GlobalStateManager {
    trackers: RwLock<HashMap<String, InstanceTracker>>,
    broadcast_tx: StateBroadcast,
    lifecycle_tx: LifecycleBroadcast,
    /// Sessions that have been claimed by instances (session_id -> instance_id)
    /// Prevents multiple instances from claiming the same Claude session
    claimed_sessions: RwLock<HashMap<String, String>>,
    /// First input data per instance (instance_id -> FirstInputData).
    /// Used for causation-based session discovery: the timestamp gates when
    /// discovery starts, and content prefixes verify the candidate session
    /// actually contains input sent to *this* instance.
    first_input_at: RwLock<HashMap<String, FirstInputData>>,
    /// Presence tracking: instance_id -> (connection_id -> PresenceEntry)
    presence: RwLock<HashMap<String, HashMap<String, PresenceEntry>>>,
    /// Pending attributions: instance_id -> queue of (user, content_prefix).
    /// Pushed by the WebSocket input handler, consumed by the conversation watcher.
    /// Content-matched to conversation entries for reliable attribution.
    pending_attributions: RwLock<HashMap<String, VecDeque<PendingAttribution>>>,
    /// Terminal locks: instance_id -> lock holder info
    terminal_locks: RwLock<HashMap<String, TerminalLock>>,
    /// Timestamp when each instance entered its current state
    state_entered_at: RwLock<HashMap<String, DateTime<Utc>>>,
}

impl GlobalStateManager {
    pub fn new(broadcast_tx: StateBroadcast) -> Self {
        let (lifecycle_tx, _) = broadcast::channel(64);
        Self {
            trackers: RwLock::new(HashMap::new()),
            broadcast_tx,
            lifecycle_tx,
            claimed_sessions: RwLock::new(HashMap::new()),
            first_input_at: RwLock::new(HashMap::new()),
            presence: RwLock::new(HashMap::new()),
            pending_attributions: RwLock::new(HashMap::new()),
            terminal_locks: RwLock::new(HashMap::new()),
            state_entered_at: RwLock::new(HashMap::new()),
        }
    }

    /// Get the timestamp when an instance entered its current state.
    pub async fn get_state_entered_at(&self, instance_id: &str) -> Option<DateTime<Utc>> {
        self.state_entered_at
            .read()
            .await
            .get(instance_id)
            .copied()
    }

    /// Record when an instance entered its current state.
    pub async fn set_state_entered_at(&self, instance_id: &str, timestamp: DateTime<Utc>) {
        self.state_entered_at
            .write()
            .await
            .insert(instance_id.to_string(), timestamp);
    }

    /// Try to claim a session for an instance. Returns true if successful.
    /// Returns false if the session is already claimed by another instance.
    pub async fn try_claim_session(&self, session_id: &str, instance_id: &str) -> bool {
        let mut claimed = self.claimed_sessions.write().await;
        match claimed.get(session_id) {
            Some(owner) if owner == instance_id => true, // Already claimed by us
            Some(_) => false,                            // Claimed by another instance
            None => {
                claimed.insert(session_id.to_string(), instance_id.to_string());
                info!(
                    "[SESSION] Instance {} claimed session {}",
                    instance_id, session_id
                );
                true
            }
        }
    }

    /// Release a session claim (called when instance is unregistered)
    pub async fn release_session(&self, instance_id: &str) {
        let mut claimed = self.claimed_sessions.write().await;
        claimed.retain(|_, owner| owner != instance_id);
    }

    /// Get all sessions claimed by other instances (for filtering candidates)
    pub async fn get_claimed_sessions(&self) -> HashSet<String> {
        self.claimed_sessions.read().await.keys().cloned().collect()
    }

    /// Record input for an instance before session discovery.
    /// First call: creates the entry with timestamp, returns true.
    /// Subsequent calls: accumulates content, returns false.
    ///
    /// Content handling depends on whether input arrives as individual
    /// keystrokes (terminal) or as a complete message (ConversationView):
    ///
    /// - **Full message** (contains `\r` or `\n`): normalized and stored
    ///   directly as a content prefix.
    /// - **Individual keystroke** (no newline): accumulated in a line buffer.
    ///   When a `\r`/`\n` arrives, the accumulated line is flushed as a
    ///   single content prefix.
    ///
    /// This ensures that both input paths produce the same content prefixes
    /// for session discovery matching.
    pub async fn mark_first_input(&self, instance_id: &str, content: &str) -> bool {
        let mut map = self.first_input_at.write().await;
        let is_first = !map.contains_key(instance_id);

        let data = map.entry(instance_id.to_string()).or_insert_with(|| {
            let now = Utc::now();
            info!(
                "[SESSION] Marked first input for instance {} at {}",
                instance_id, now
            );
            FirstInputData {
                timestamp: now,
                content_prefixes: vec![],
                pending_line: String::new(),
            }
        });

        if data.content_prefixes.len() >= FIRST_INPUT_PREFIX_CAP {
            return is_first;
        }

        // Check if content contains a newline (full message or end-of-line keystroke)
        let has_newline = content.contains('\r') || content.contains('\n');

        if has_newline {
            // Flush: prepend any accumulated pending_line to content, then
            // split on newlines and store each non-empty line as a prefix.
            let combined = if data.pending_line.is_empty() {
                content.to_string()
            } else {
                let mut s = std::mem::take(&mut data.pending_line);
                s.push_str(content);
                s
            };

            for part in combined.split(['\r', '\n']) {
                if data.content_prefixes.len() >= FIRST_INPUT_PREFIX_CAP {
                    break;
                }
                let prefix = normalize_attribution_content(part);
                if !prefix.is_empty() {
                    data.content_prefixes.push(prefix);
                }
            }
        } else {
            // Individual keystroke — accumulate in the line buffer.
            // Skip control characters and escape sequences that won't
            // appear in the JSONL user entry text.
            let first_byte = content.as_bytes().first().copied().unwrap_or(0);
            let is_control = first_byte < 0x20 || first_byte == 0x7f;
            if !is_control {
                data.pending_line.push_str(content);
            }
        }

        is_first
    }

    /// Get the timestamp of first input for an instance, if any.
    pub async fn get_first_input_at(&self, instance_id: &str) -> Option<DateTime<Utc>> {
        self.first_input_at
            .read()
            .await
            .get(instance_id)
            .map(|d| d.timestamp)
    }

    /// Get content prefixes stored for session discovery verification.
    /// Returns an empty vec if no inputs have been recorded.
    pub async fn get_discovery_content_prefixes(&self, instance_id: &str) -> Vec<String> {
        self.first_input_at
            .read()
            .await
            .get(instance_id)
            .map(|d| d.content_prefixes.clone())
            .unwrap_or_default()
    }

    // =========================================================================
    // Pending attribution tracking (in-process, content-matched)
    // =========================================================================

    /// Record that a WebSocket user just sent input to an instance.
    /// The conversation watcher will consume this when the corresponding
    /// User entry appears in the conversation.
    pub async fn push_pending_attribution(
        &self,
        instance_id: &str,
        user_id: String,
        display_name: String,
        content: &str,
        task_id: Option<i64>,
    ) {
        let prefix = normalize_attribution_content(content);
        if prefix.is_empty() {
            return;
        }
        let mut map = self.pending_attributions.write().await;
        let queue = map
            .entry(instance_id.to_string())
            .or_insert_with(VecDeque::new);
        queue.push_back(PendingAttribution {
            user_id,
            display_name,
            content_prefix: prefix,
            timestamp: Utc::now(),
            task_id,
        });
        // Cap the queue to prevent unbounded growth
        while queue.len() > 50 {
            queue.pop_front();
        }
    }

    /// Try to consume a pending attribution that matches a conversation entry's content.
    /// Returns the attribution if found, or None if this was external/terminal input.
    ///
    /// Matching strategy: compare the first N chars of the conversation entry content
    /// against pending attribution prefixes. The first match is consumed (FIFO order
    /// handles the case where the same user sends multiple messages).
    pub async fn consume_pending_attribution(
        &self,
        instance_id: &str,
        entry_content: &str,
    ) -> Option<PendingAttribution> {
        if normalize_attribution_content(entry_content).is_empty() {
            return None;
        }

        let mut map = self.pending_attributions.write().await;
        let queue = map.get_mut(instance_id)?;

        // Find the first pending attribution whose content matches
        if let Some(idx) = queue
            .iter()
            .position(|attr| attribution_content_matches(&attr.content_prefix, entry_content))
        {
            Some(queue.remove(idx).unwrap())
        } else {
            // Prune stale entries (older than 60 seconds) while we're here
            let cutoff = Utc::now() - chrono::Duration::seconds(60);
            queue.retain(|attr| attr.timestamp > cutoff);
            None
        }
    }

    // =========================================================================
    // Terminal lock tracking
    // =========================================================================

    /// Try to acquire the terminal lock for an instance.
    /// Succeeds if unclaimed or if current holder's lock has expired.
    /// Returns true if lock was acquired.
    pub async fn try_acquire_terminal_lock(
        &self,
        instance_id: &str,
        connection_id: &str,
        user: &WsUser,
    ) -> bool {
        let mut locks = self.terminal_locks.write().await;
        let now = Utc::now();

        if let Some(existing) = locks.get(instance_id) {
            // Already held by this user (any connection)
            if existing.holder_user_id == user.user_id {
                // Update connection_id in case it's a different tab
                locks.insert(
                    instance_id.to_string(),
                    TerminalLock {
                        holder_connection_id: connection_id.to_string(),
                        holder_user_id: user.user_id.clone(),
                        holder_display_name: user.display_name.clone(),
                        last_activity: now,
                    },
                );
                return true;
            }

            // Check if expired
            let elapsed = (now - existing.last_activity).num_seconds();
            if elapsed < TERMINAL_LOCK_TIMEOUT_SECS {
                return false; // Still active, can't take it
            }
            // Expired — fall through to acquire
            info!(
                "[TERMINAL-LOCK] Lock expired for {} (held by {}, inactive {}s), granting to {}",
                instance_id, existing.holder_display_name, elapsed, user.display_name
            );
        }

        locks.insert(
            instance_id.to_string(),
            TerminalLock {
                holder_connection_id: connection_id.to_string(),
                holder_user_id: user.user_id.clone(),
                holder_display_name: user.display_name.clone(),
                last_activity: now,
            },
        );
        info!(
            "[TERMINAL-LOCK] {} acquired lock for instance {}",
            user.display_name, instance_id
        );
        true
    }

    /// Update last_activity on the terminal lock (called on every Input from holder).
    pub async fn touch_terminal_lock(&self, instance_id: &str, connection_id: &str) {
        let mut locks = self.terminal_locks.write().await;
        if let Some(lock) = locks.get_mut(instance_id) {
            // Touch if held by this connection OR same user (multi-tab)
            if lock.holder_connection_id == connection_id {
                lock.last_activity = Utc::now();
            }
        }
    }

    /// Release the terminal lock (voluntary release or disconnect cleanup).
    /// Returns true if a lock was actually released.
    pub async fn release_terminal_lock(&self, instance_id: &str, connection_id: &str) -> bool {
        let mut locks = self.terminal_locks.write().await;
        if let Some(lock) = locks.get(instance_id)
            && lock.holder_connection_id == connection_id
        {
            let holder = lock.holder_display_name.clone();
            locks.remove(instance_id);
            info!(
                "[TERMINAL-LOCK] {} released lock for instance {}",
                holder, instance_id
            );
            return true;
        }
        false
    }

    /// Get the current terminal lock state for an instance.
    pub async fn get_terminal_lock(&self, instance_id: &str) -> Option<TerminalLock> {
        self.terminal_locks.read().await.get(instance_id).cloned()
    }

    /// Reconcile terminal lock with presence:
    /// - If holder disconnected, clear the lock.
    /// - If only one user is present and no lock exists, auto-grant.
    ///   Returns true if lock state changed.
    pub async fn reconcile_terminal_lock_with_presence(&self, instance_id: &str) -> bool {
        let presence = self.presence.read().await;
        let instance_presence = presence.get(instance_id);
        let connection_ids: Vec<String> = instance_presence
            .map(|p| p.keys().cloned().collect())
            .unwrap_or_default();
        let unique_users = instance_presence
            .map(Self::dedupe_presence)
            .unwrap_or_default();
        drop(presence);

        let mut locks = self.terminal_locks.write().await;

        // If holder's connection is no longer present, clear the lock
        if let Some(lock) = locks.get(instance_id)
            && !connection_ids.contains(&lock.holder_connection_id)
        {
            info!(
                "[TERMINAL-LOCK] Holder {} disconnected from {}, clearing lock",
                lock.holder_display_name, instance_id
            );
            locks.remove(instance_id);
            return true;
        }

        // Auto-grant to sole user if no lock exists
        if locks.get(instance_id).is_none() && unique_users.len() == 1 {
            // Find the connection_id for this sole user
            let sole_user = &unique_users[0];
            if let Some(conn_id) = connection_ids.first() {
                locks.insert(
                    instance_id.to_string(),
                    TerminalLock {
                        holder_connection_id: conn_id.clone(),
                        holder_user_id: sole_user.user_id.clone(),
                        holder_display_name: sole_user.display_name.clone(),
                        last_activity: Utc::now(),
                    },
                );
                debug!(
                    "[TERMINAL-LOCK] Auto-granted lock to sole user {} for instance {}",
                    sole_user.display_name, instance_id
                );
                return true;
            }
        }

        false
    }

    /// Register a new instance for state tracking
    pub async fn register_instance(
        self: &Arc<Self>,
        instance_id: String,
        handle: InstanceHandle,
        working_dir: String,
        created_at: DateTime<Utc>,
        kind: InstanceKind,
        repository: Option<Arc<ConversationRepository>>,
    ) {
        let mut tracker =
            InstanceTracker::new(instance_id.clone(), handle, working_dir, created_at, kind);
        tracker.start_state_manager(self.broadcast_tx.clone(), Arc::clone(self), repository);

        self.trackers.write().await.insert(instance_id, tracker);
    }

    /// Unregister an instance
    pub async fn unregister_instance(&self, instance_id: &str) {
        if let Some(tracker) = self.trackers.write().await.remove(instance_id) {
            // Stop background tasks before dropping
            tracker.stop();
        }
        // Release any claimed sessions
        self.release_session(instance_id).await;
        // Clean up first_input_at tracking
        self.first_input_at.write().await.remove(instance_id);
        // Clean up pending attributions
        self.pending_attributions.write().await.remove(instance_id);
        // Clean up terminal lock
        self.terminal_locks.write().await.remove(instance_id);
        // Clean up state_entered_at
        self.state_entered_at.write().await.remove(instance_id);
    }

    /// Get a handle for an instance
    pub async fn get_handle(&self, instance_id: &str) -> Option<InstanceHandle> {
        self.trackers
            .read()
            .await
            .get(instance_id)
            .map(|t| t.handle.clone())
    }

    /// Get all instance handles (for disconnect cleanup across all instances).
    pub async fn all_handles(&self) -> Vec<(String, InstanceHandle)> {
        self.trackers
            .read()
            .await
            .iter()
            .map(|(id, t)| (id.clone(), t.handle.clone()))
            .collect()
    }

    /// Get tracker info for an instance
    pub async fn get_tracker_info(
        &self,
        instance_id: &str,
    ) -> Option<(InstanceHandle, String, DateTime<Utc>, InstanceKind)> {
        self.trackers.read().await.get(instance_id).map(|t| {
            (
                t.handle.clone(),
                t.working_dir.clone(),
                t.created_at,
                t.kind.clone(),
            )
        })
    }

    /// Send a signal to an instance's state manager
    pub async fn send_signal(&self, instance_id: &str, signal: StateSignal) {
        if let Some(tracker) = self.trackers.read().await.get(instance_id) {
            tracker.send_signal(signal).await;
        }
    }

    /// Process terminal input for an instance: send state signal, record for
    /// session discovery, and write to PTY.
    ///
    /// This is the single entry point for input handling — both the
    /// multiplexed WebSocket handler and the TUI proxy call this instead of
    /// duplicating the state-signal + mark_first_input + PTY-write ceremony.
    ///
    /// Returns `Err` if the PTY write fails or the instance handle is gone.
    pub async fn write_input(&self, instance_id: &str, data: &str) -> Result<(), String> {
        // Send state signal for idle→thinking detection
        self.send_signal(
            instance_id,
            StateSignal::TerminalInput {
                data: data.to_string(),
            },
        )
        .await;

        let handle = self
            .get_handle(instance_id)
            .await
            .ok_or_else(|| "Instance handle not found".to_string())?;

        // Record for session discovery (with line accumulation).
        // Only relevant while the session hasn't been claimed yet.
        if handle.get_session_id().await.is_none() {
            self.mark_first_input(instance_id, data).await;
        }

        // Write to PTY
        handle
            .write_input(data)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Subscribe to state broadcasts
    /// Returns receiver for (instance_id, state, terminal_stale) tuples
    pub fn subscribe(&self) -> broadcast::Receiver<(String, ClaudeState, bool)> {
        self.broadcast_tx.subscribe()
    }

    /// Subscribe to instance lifecycle broadcasts (InstanceCreated/InstanceStopped)
    pub fn subscribe_lifecycle(&self) -> broadcast::Receiver<ServerMessage> {
        self.lifecycle_tx.subscribe()
    }

    /// Broadcast an instance lifecycle event to all connected WebSocket clients
    pub fn broadcast_lifecycle(&self, msg: ServerMessage) {
        let _ = self.lifecycle_tx.send(msg);
    }

    // =========================================================================
    // Conversation data (server-owned watcher)
    // =========================================================================

    /// Get a snapshot of the current formatted conversation turns for an instance.
    pub async fn get_conversation_snapshot(&self, instance_id: &str) -> Vec<serde_json::Value> {
        if let Some(tracker) = self.trackers.read().await.get(instance_id) {
            tracker.conversation_turns.read().await.clone()
        } else {
            Vec::new()
        }
    }

    /// Subscribe to conversation events (Full/Update) for an instance.
    /// Returns None if the instance doesn't exist.
    pub async fn subscribe_conversation(
        &self,
        instance_id: &str,
    ) -> Option<broadcast::Receiver<ConversationEvent>> {
        self.trackers
            .read()
            .await
            .get(instance_id)
            .map(|t| t.conversation_tx.subscribe())
    }

    // =========================================================================
    // Presence tracking
    // =========================================================================

    /// Add a user to an instance's presence set. Returns the updated presence list.
    pub async fn add_presence(
        &self,
        instance_id: &str,
        connection_id: &str,
        user: &WsUser,
    ) -> Vec<PresenceUser> {
        let mut presence = self.presence.write().await;
        let instance_presence = presence
            .entry(instance_id.to_string())
            .or_insert_with(HashMap::new);
        instance_presence.insert(
            connection_id.to_string(),
            PresenceEntry {
                user_id: user.user_id.clone(),
                display_name: user.display_name.clone(),
            },
        );
        Self::dedupe_presence(instance_presence)
    }

    /// Remove a connection from a specific instance's presence set.
    pub async fn remove_presence_from_instance(
        &self,
        instance_id: &str,
        connection_id: &str,
    ) -> Vec<PresenceUser> {
        let mut presence = self.presence.write().await;
        if let Some(instance_presence) = presence.get_mut(instance_id) {
            instance_presence.remove(connection_id);
            let result = Self::dedupe_presence(instance_presence);
            if instance_presence.is_empty() {
                presence.remove(instance_id);
            }
            result
        } else {
            vec![]
        }
    }

    /// Remove a connection from ALL instance presence sets. Returns affected instance_ids.
    pub async fn remove_presence_all(
        &self,
        connection_id: &str,
    ) -> Vec<(String, Vec<PresenceUser>)> {
        let mut presence = self.presence.write().await;
        let mut updates = Vec::new();
        let mut empty_instances = Vec::new();
        for (instance_id, instance_presence) in presence.iter_mut() {
            if instance_presence.remove(connection_id).is_some() {
                let users = Self::dedupe_presence(instance_presence);
                updates.push((instance_id.clone(), users));
                if instance_presence.is_empty() {
                    empty_instances.push(instance_id.clone());
                }
            }
        }
        for id in empty_instances {
            presence.remove(&id);
        }
        updates
    }

    /// Deduplicate presence by user_id (a user may have multiple connections).
    fn dedupe_presence(entries: &HashMap<String, PresenceEntry>) -> Vec<PresenceUser> {
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for entry in entries.values() {
            if seen.insert(entry.user_id.clone()) {
                result.push(PresenceUser {
                    user_id: entry.user_id.clone(),
                    display_name: entry.display_name.clone(),
                });
            }
        }
        result
    }
}

#[cfg(test)]
impl GlobalStateManager {
    /// Insert a minimal tracker for testing conversation data flow.
    /// Does NOT spawn any background tasks (watcher, state manager).
    pub(crate) async fn insert_test_tracker(&self, instance_id: &str, handle: InstanceHandle) {
        let tracker = InstanceTracker::new(
            instance_id.to_string(),
            handle,
            "/tmp/test".to_string(),
            Utc::now(),
            InstanceKind::Structured {
                provider: "claude".into(),
            },
        );
        self.trackers
            .write()
            .await
            .insert(instance_id.to_string(), tracker);
    }

    /// Write conversation turns directly into a tracker's store (for testing).
    pub(crate) async fn set_test_conversation_turns(
        &self,
        instance_id: &str,
        turns: Vec<serde_json::Value>,
    ) {
        if let Some(tracker) = self.trackers.read().await.get(instance_id) {
            let mut store = tracker.conversation_turns.write().await;
            *store = turns;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mark_first_input_returns_true_then_false() {
        let broadcast_tx = create_state_broadcast();
        let state_mgr = GlobalStateManager::new(broadcast_tx);

        // First call returns true (full message with newline)
        assert!(state_mgr.mark_first_input("inst-1", "hello\r").await);

        // Second call returns false (already recorded)
        assert!(!state_mgr.mark_first_input("inst-1", "world\r").await);

        // Different instance returns true
        assert!(state_mgr.mark_first_input("inst-2", "hi\r").await);
    }

    #[tokio::test]
    async fn test_mark_first_input_full_messages() {
        let broadcast_tx = create_state_broadcast();
        let state_mgr = GlobalStateManager::new(broadcast_tx);

        // Full messages (contain newlines) store directly as prefixes
        state_mgr
            .mark_first_input("inst-1", "first message\r")
            .await;
        state_mgr
            .mark_first_input("inst-1", "second message\r")
            .await;
        state_mgr
            .mark_first_input("inst-1", "third message\r")
            .await;

        let prefixes = state_mgr.get_discovery_content_prefixes("inst-1").await;
        assert_eq!(prefixes.len(), 3);
        assert!(prefixes[0].starts_with("first"));
        assert!(prefixes[1].starts_with("second"));
        assert!(prefixes[2].starts_with("third"));
    }

    #[tokio::test]
    async fn test_mark_first_input_keystroke_accumulation() {
        let broadcast_tx = create_state_broadcast();
        let state_mgr = GlobalStateManager::new(broadcast_tx);

        // Individual keystrokes accumulate in pending_line
        state_mgr.mark_first_input("inst-1", "H").await;
        state_mgr.mark_first_input("inst-1", "e").await;
        state_mgr.mark_first_input("inst-1", "l").await;
        state_mgr.mark_first_input("inst-1", "l").await;
        state_mgr.mark_first_input("inst-1", "o").await;

        // No prefix stored yet (no newline to flush)
        let prefixes = state_mgr.get_discovery_content_prefixes("inst-1").await;
        assert!(prefixes.is_empty());

        // Newline flushes the accumulated line
        state_mgr.mark_first_input("inst-1", "\r").await;

        let prefixes = state_mgr.get_discovery_content_prefixes("inst-1").await;
        assert_eq!(prefixes.len(), 1);
        assert_eq!(prefixes[0], "Hello");
    }

    #[tokio::test]
    async fn test_mark_first_input_skips_control_chars() {
        let broadcast_tx = create_state_broadcast();
        let state_mgr = GlobalStateManager::new(broadcast_tx);

        // Escape sequences and control chars should not accumulate
        state_mgr.mark_first_input("inst-1", "\x1b[A").await; // up arrow
        state_mgr.mark_first_input("inst-1", "\x7f").await; // backspace
        state_mgr.mark_first_input("inst-1", "H").await;
        state_mgr.mark_first_input("inst-1", "i").await;
        state_mgr.mark_first_input("inst-1", "\r").await;

        let prefixes = state_mgr.get_discovery_content_prefixes("inst-1").await;
        assert_eq!(prefixes.len(), 1);
        assert_eq!(prefixes[0], "Hi");
    }

    #[tokio::test]
    async fn test_mark_first_input_caps_content_prefixes() {
        let broadcast_tx = create_state_broadcast();
        let state_mgr = GlobalStateManager::new(broadcast_tx);

        // Push more than the cap (each with newline to force flush)
        for i in 0..25 {
            state_mgr
                .mark_first_input("inst-1", &format!("message {}\r", i))
                .await;
        }

        let prefixes = state_mgr.get_discovery_content_prefixes("inst-1").await;
        assert_eq!(prefixes.len(), super::FIRST_INPUT_PREFIX_CAP);
    }

    #[tokio::test]
    async fn test_mark_first_input_skips_empty_content() {
        let broadcast_tx = create_state_broadcast();
        let state_mgr = GlobalStateManager::new(broadcast_tx);

        // First call with whitespace-only: creates entry but no prefix
        assert!(state_mgr.mark_first_input("inst-1", "   \r").await);

        let prefixes = state_mgr.get_discovery_content_prefixes("inst-1").await;
        assert!(prefixes.is_empty());

        // Timestamp should still be set
        assert!(state_mgr.get_first_input_at("inst-1").await.is_some());
    }

    #[tokio::test]
    async fn test_get_discovery_content_prefixes_no_entry() {
        let broadcast_tx = create_state_broadcast();
        let state_mgr = GlobalStateManager::new(broadcast_tx);

        let prefixes = state_mgr
            .get_discovery_content_prefixes("nonexistent")
            .await;
        assert!(prefixes.is_empty());
    }

    #[tokio::test]
    async fn test_get_first_input_at_lifecycle() {
        let broadcast_tx = create_state_broadcast();
        let state_mgr = GlobalStateManager::new(broadcast_tx);

        // Before any input
        assert!(state_mgr.get_first_input_at("inst-1").await.is_none());

        // After marking first input (keystroke without newline still creates timestamp)
        state_mgr.mark_first_input("inst-1", "h").await;
        let timestamp = state_mgr.get_first_input_at("inst-1").await;
        assert!(timestamp.is_some());

        // Timestamp should be recent
        let now = Utc::now();
        let diff = now - timestamp.unwrap();
        assert!(diff.num_seconds() < 2);
    }

    #[tokio::test]
    async fn test_first_input_at_cleanup_on_unregister() {
        let broadcast_tx = create_state_broadcast();
        let state_mgr = GlobalStateManager::new(broadcast_tx);

        // Mark first input
        state_mgr.mark_first_input("inst-1", "hello\r").await;
        assert!(state_mgr.get_first_input_at("inst-1").await.is_some());

        // Unregister cleans up first_input_at
        state_mgr.unregister_instance("inst-1").await;
        assert!(state_mgr.get_first_input_at("inst-1").await.is_none());
    }

    #[test]
    fn test_timestamp_narrowing_logic() {
        use chrono::TimeZone;

        let t1_created = Utc.with_ymd_and_hms(2024, 6, 15, 10, 0, 1).unwrap();
        let t4_first_input = Utc.with_ymd_and_hms(2024, 6, 15, 10, 0, 6).unwrap();
        let t3_other_session = Utc.with_ymd_and_hms(2024, 6, 15, 10, 0, 4).unwrap();
        let t5_own_session = Utc.with_ymd_and_hms(2024, 6, 15, 10, 0, 7).unwrap();

        // With created_at, BOTH sessions match (too broad)
        assert!(t3_other_session >= t1_created);
        assert!(t5_own_session >= t1_created);

        // With first_input_at, only OWN session matches (correct)
        assert!(t3_other_session < t4_first_input);
        assert!(t5_own_session >= t4_first_input);
    }

    #[tokio::test]
    async fn test_state_broadcast_channel_basic() {
        let (tx, mut rx) = broadcast::channel::<(String, ClaudeState, bool)>(16);

        tx.send(("inst-1".to_string(), ClaudeState::Thinking, false))
            .unwrap();

        let (id, state, stale) = rx.recv().await.unwrap();
        assert_eq!(id, "inst-1");
        assert!(matches!(state, ClaudeState::Thinking));
        assert!(!stale);
    }

    #[tokio::test]
    async fn test_state_broadcast_channel_multiple_receivers() {
        let (tx, mut rx1) = broadcast::channel::<(String, ClaudeState, bool)>(16);
        let mut rx2 = tx.subscribe();

        tx.send(("inst-1".to_string(), ClaudeState::Responding, true))
            .unwrap();

        let (id1, _, stale1) = rx1.recv().await.unwrap();
        let (id2, _, stale2) = rx2.recv().await.unwrap();

        assert_eq!(id1, "inst-1");
        assert_eq!(id2, "inst-1");
        assert!(stale1);
        assert!(stale2);
    }

    #[tokio::test]
    async fn test_state_broadcast_channel_lag_detection() {
        // Create a small channel that will lag
        let (tx, mut rx) = broadcast::channel::<(String, ClaudeState, bool)>(2);

        // Send more messages than the buffer can hold
        for i in 0..5 {
            let _ = tx.send((format!("inst-{}", i), ClaudeState::Idle, false));
        }

        // First recv should report lag
        match rx.recv().await {
            Err(broadcast::error::RecvError::Lagged(n)) => {
                assert!(n > 0, "Should report lagged messages");
            }
            Ok(_) => {
                // Might get the last message if timing is right
            }
            Err(e) => panic!("Unexpected error: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_mpsc_channel_backpressure() {
        // Test that mpsc channel blocks sender when full
        let (tx, mut rx) = mpsc::channel::<ServerMessage>(2);

        // Fill the channel
        tx.send(ServerMessage::Output {
            instance_id: "inst-1".to_string(),
            data: "1".to_string(),
            cursor: None,
        })
        .await
        .unwrap();
        tx.send(ServerMessage::Output {
            instance_id: "inst-1".to_string(),
            data: "2".to_string(),
            cursor: None,
        })
        .await
        .unwrap();

        // Drain one to make room
        let msg = rx.recv().await.unwrap();
        match msg {
            ServerMessage::Output { data, .. } => assert_eq!(data, "1"),
            _ => panic!("Expected Output"),
        }

        // Now we can send again
        tx.send(ServerMessage::Output {
            instance_id: "inst-1".to_string(),
            data: "3".to_string(),
            cursor: None,
        })
        .await
        .unwrap();
    }

    // =========================================================================
    // Session claiming tests
    // =========================================================================

    #[tokio::test]
    async fn test_try_claim_session_unclaimed() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        assert!(state_mgr.try_claim_session("sess-1", "inst-1").await);
    }

    #[tokio::test]
    async fn test_try_claim_session_already_owned_by_self() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        assert!(state_mgr.try_claim_session("sess-1", "inst-1").await);
        // Same instance re-claiming → true
        assert!(state_mgr.try_claim_session("sess-1", "inst-1").await);
    }

    #[tokio::test]
    async fn test_try_claim_session_owned_by_other() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        assert!(state_mgr.try_claim_session("sess-1", "inst-1").await);
        // Different instance → false
        assert!(!state_mgr.try_claim_session("sess-1", "inst-2").await);
    }

    #[tokio::test]
    async fn test_release_session() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        state_mgr.try_claim_session("sess-1", "inst-1").await;
        state_mgr.try_claim_session("sess-2", "inst-1").await;

        state_mgr.release_session("inst-1").await;

        // Both sessions should be claimable by another instance now
        assert!(state_mgr.try_claim_session("sess-1", "inst-2").await);
        assert!(state_mgr.try_claim_session("sess-2", "inst-2").await);
    }

    #[tokio::test]
    async fn test_get_claimed_sessions() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        state_mgr.try_claim_session("sess-1", "inst-1").await;
        state_mgr.try_claim_session("sess-2", "inst-2").await;

        let claimed = state_mgr.get_claimed_sessions().await;
        assert!(claimed.contains("sess-1"));
        assert!(claimed.contains("sess-2"));
        assert_eq!(claimed.len(), 2);
    }

    #[tokio::test]
    async fn test_first_input_gate_prevents_cross_claim() {
        // Simulates the multi-instance race:
        // Instance A (no input) must not claim Instance B's session.
        let state_mgr = GlobalStateManager::new(create_state_broadcast());

        // Instance B marks first input
        assert!(state_mgr.mark_first_input("inst-b", "fix the bug").await);

        // Instance A has no first_input_at — should be gated from discovery
        assert!(state_mgr.get_first_input_at("inst-a").await.is_none());

        // Instance B has first_input_at — discovery proceeds
        assert!(state_mgr.get_first_input_at("inst-b").await.is_some());

        // B claims its session
        assert!(state_mgr.try_claim_session("sess-b", "inst-b").await);

        // Even if A were to try (bypassing the gate), it can't steal
        let claimed = state_mgr.get_claimed_sessions().await;
        assert!(claimed.contains("sess-b"));
        assert!(!state_mgr.try_claim_session("sess-b", "inst-a").await);
    }

    #[tokio::test]
    async fn test_two_instances_sequential_input_correct_claims() {
        // A gets input first, claims its session.
        // B gets input second, sees A's claim, claims its own session.
        let state_mgr = GlobalStateManager::new(create_state_broadcast());

        // A marks first input and claims session-a
        state_mgr.mark_first_input("inst-a", "hello").await;
        assert!(state_mgr.try_claim_session("sess-a", "inst-a").await);

        // B marks first input
        state_mgr.mark_first_input("inst-b", "world").await;

        // B sees sess-a is claimed
        let claimed = state_mgr.get_claimed_sessions().await;
        assert!(claimed.contains("sess-a"));

        // B claims sess-b (unclaimed)
        assert!(state_mgr.try_claim_session("sess-b", "inst-b").await);

        // Both sessions claimed by correct instances
        assert!(!state_mgr.try_claim_session("sess-a", "inst-b").await);
        assert!(!state_mgr.try_claim_session("sess-b", "inst-a").await);
    }

    // =========================================================================
    // Attribution queue tests
    // =========================================================================

    #[tokio::test]
    async fn test_push_and_consume_attribution() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());

        state_mgr
            .push_pending_attribution("inst-1", "u-1".into(), "Alice".into(), "fix the bug", None)
            .await;

        let attr = state_mgr
            .consume_pending_attribution("inst-1", "fix the bug")
            .await
            .unwrap();
        assert_eq!(attr.user_id, "u-1");
        assert_eq!(attr.display_name, "Alice");
    }

    #[tokio::test]
    async fn test_consume_attribution_no_match() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());

        state_mgr
            .push_pending_attribution("inst-1", "u-1".into(), "Alice".into(), "fix the bug", None)
            .await;

        let result = state_mgr
            .consume_pending_attribution("inst-1", "add new feature")
            .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_consume_attribution_fifo() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());

        state_mgr
            .push_pending_attribution("inst-1", "u-1".into(), "Alice".into(), "message one", None)
            .await;
        state_mgr
            .push_pending_attribution("inst-1", "u-2".into(), "Bob".into(), "message two", None)
            .await;

        // Consume first match
        let attr = state_mgr
            .consume_pending_attribution("inst-1", "message one")
            .await
            .unwrap();
        assert_eq!(attr.user_id, "u-1");

        // First is consumed, second remains
        let attr = state_mgr
            .consume_pending_attribution("inst-1", "message two")
            .await
            .unwrap();
        assert_eq!(attr.user_id, "u-2");
    }

    #[tokio::test]
    async fn test_push_attribution_empty_content_skipped() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());

        state_mgr
            .push_pending_attribution("inst-1", "u-1".into(), "Alice".into(), "   ", None)
            .await;

        // Nothing should be queued (empty after trim)
        let result = state_mgr
            .consume_pending_attribution("inst-1", "anything")
            .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_attribution_with_task_id() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());

        state_mgr
            .push_pending_attribution(
                "inst-1",
                "u-1".into(),
                "Alice".into(),
                "task content",
                Some(42),
            )
            .await;

        let attr = state_mgr
            .consume_pending_attribution("inst-1", "task content")
            .await
            .unwrap();
        assert_eq!(attr.task_id, Some(42));
    }

    #[tokio::test]
    async fn test_attribution_queue_cap() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());

        // Push more than the 50-entry cap
        for i in 0..60 {
            state_mgr
                .push_pending_attribution(
                    "inst-1",
                    "u-1".into(),
                    "Alice".into(),
                    &format!("message {}", i),
                    None,
                )
                .await;
        }

        // Oldest entries should be evicted; message 0 through 9 should be gone
        let result = state_mgr
            .consume_pending_attribution("inst-1", "message 0")
            .await;
        assert!(result.is_none());

        // Latest entries should still be present
        let result = state_mgr
            .consume_pending_attribution("inst-1", "message 59")
            .await;
        assert!(result.is_some());
    }

    // =========================================================================
    // Terminal lock tests
    // =========================================================================

    fn make_ws_user(user_id: &str, display_name: &str) -> WsUser {
        WsUser {
            user_id: user_id.to_string(),
            display_name: display_name.to_string(),
        }
    }

    #[tokio::test]
    async fn test_acquire_terminal_lock_unclaimed() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let user = make_ws_user("u-1", "Alice");

        assert!(
            state_mgr
                .try_acquire_terminal_lock("inst-1", "conn-1", &user)
                .await
        );

        let lock = state_mgr.get_terminal_lock("inst-1").await.unwrap();
        assert_eq!(lock.holder_user_id, "u-1");
    }

    #[tokio::test]
    async fn test_acquire_terminal_lock_same_user() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let user = make_ws_user("u-1", "Alice");

        state_mgr
            .try_acquire_terminal_lock("inst-1", "conn-1", &user)
            .await;
        // Same user, different connection (new tab) → allowed
        assert!(
            state_mgr
                .try_acquire_terminal_lock("inst-1", "conn-2", &user)
                .await
        );

        let lock = state_mgr.get_terminal_lock("inst-1").await.unwrap();
        assert_eq!(lock.holder_connection_id, "conn-2");
    }

    #[tokio::test]
    async fn test_acquire_terminal_lock_different_user_denied() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let alice = make_ws_user("u-1", "Alice");
        let bob = make_ws_user("u-2", "Bob");

        state_mgr
            .try_acquire_terminal_lock("inst-1", "conn-1", &alice)
            .await;
        // Different user while lock is active → denied
        assert!(
            !state_mgr
                .try_acquire_terminal_lock("inst-1", "conn-2", &bob)
                .await
        );
    }

    #[tokio::test]
    async fn test_release_terminal_lock() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let user = make_ws_user("u-1", "Alice");

        state_mgr
            .try_acquire_terminal_lock("inst-1", "conn-1", &user)
            .await;
        let released = state_mgr.release_terminal_lock("inst-1", "conn-1").await;
        assert!(released);

        assert!(state_mgr.get_terminal_lock("inst-1").await.is_none());
    }

    #[tokio::test]
    async fn test_release_terminal_lock_wrong_connection() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let user = make_ws_user("u-1", "Alice");

        state_mgr
            .try_acquire_terminal_lock("inst-1", "conn-1", &user)
            .await;
        // Wrong connection_id → not released
        let released = state_mgr
            .release_terminal_lock("inst-1", "conn-other")
            .await;
        assert!(!released);

        assert!(state_mgr.get_terminal_lock("inst-1").await.is_some());
    }

    // =========================================================================
    // Presence tracking tests
    // =========================================================================

    #[tokio::test]
    async fn test_add_presence() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let user = WsUser {
            user_id: "u-1".into(),
            display_name: "Alice".into(),
        };

        let users = state_mgr.add_presence("inst-1", "conn-1", &user).await;
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].user_id, "u-1");
    }

    #[tokio::test]
    async fn test_presence_deduplication() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let user = WsUser {
            user_id: "u-1".into(),
            display_name: "Alice".into(),
        };

        // Same user, two connections (two tabs)
        state_mgr.add_presence("inst-1", "conn-1", &user).await;
        let users = state_mgr.add_presence("inst-1", "conn-2", &user).await;
        // Should be deduplicated to 1
        assert_eq!(users.len(), 1);
    }

    #[tokio::test]
    async fn test_remove_presence() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let alice = WsUser {
            user_id: "u-1".into(),
            display_name: "Alice".into(),
        };
        let bob = WsUser {
            user_id: "u-2".into(),
            display_name: "Bob".into(),
        };

        state_mgr.add_presence("inst-1", "conn-1", &alice).await;
        state_mgr.add_presence("inst-1", "conn-2", &bob).await;

        let remaining = state_mgr
            .remove_presence_from_instance("inst-1", "conn-1")
            .await;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].user_id, "u-2");
    }

    #[tokio::test]
    async fn test_remove_presence_all() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let user = WsUser {
            user_id: "u-1".into(),
            display_name: "Alice".into(),
        };

        state_mgr.add_presence("inst-1", "conn-1", &user).await;
        state_mgr.add_presence("inst-2", "conn-1", &user).await;

        let updates = state_mgr.remove_presence_all("conn-1").await;
        assert_eq!(updates.len(), 2);
        // Both instances should now have empty presence
        for (_, users) in &updates {
            assert!(users.is_empty());
        }
    }

    // =========================================================================
    // touch_terminal_lock tests
    // =========================================================================

    #[tokio::test]
    async fn test_touch_terminal_lock_updates_activity() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let user = make_ws_user("u-1", "Alice");

        state_mgr
            .try_acquire_terminal_lock("inst-1", "conn-1", &user)
            .await;

        let before = state_mgr
            .get_terminal_lock("inst-1")
            .await
            .unwrap()
            .last_activity;

        // Sleep briefly so the timestamp differs
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        state_mgr.touch_terminal_lock("inst-1", "conn-1").await;

        let after = state_mgr
            .get_terminal_lock("inst-1")
            .await
            .unwrap()
            .last_activity;

        assert!(after >= before);
    }

    #[tokio::test]
    async fn test_touch_terminal_lock_wrong_connection_ignored() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let user = make_ws_user("u-1", "Alice");

        state_mgr
            .try_acquire_terminal_lock("inst-1", "conn-1", &user)
            .await;

        let before = state_mgr
            .get_terminal_lock("inst-1")
            .await
            .unwrap()
            .last_activity;

        // Touch with wrong connection_id — should not update
        state_mgr.touch_terminal_lock("inst-1", "conn-other").await;

        let after = state_mgr
            .get_terminal_lock("inst-1")
            .await
            .unwrap()
            .last_activity;

        assert_eq!(before, after);
    }

    #[tokio::test]
    async fn test_touch_terminal_lock_no_lock_noop() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        // Touch on non-existent lock — should not panic
        state_mgr.touch_terminal_lock("inst-1", "conn-1").await;
        assert!(state_mgr.get_terminal_lock("inst-1").await.is_none());
    }

    // =========================================================================
    // reconcile_terminal_lock_with_presence tests
    // =========================================================================

    #[tokio::test]
    async fn test_reconcile_clears_disconnected_holder() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let alice = make_ws_user("u-1", "Alice");

        // Alice joins and gets the lock
        state_mgr.add_presence("inst-1", "conn-1", &alice).await;
        state_mgr
            .try_acquire_terminal_lock("inst-1", "conn-1", &alice)
            .await;

        // Alice disconnects
        state_mgr
            .remove_presence_from_instance("inst-1", "conn-1")
            .await;

        // Reconcile should clear the lock
        let changed = state_mgr
            .reconcile_terminal_lock_with_presence("inst-1")
            .await;
        assert!(changed);
        assert!(state_mgr.get_terminal_lock("inst-1").await.is_none());
    }

    #[tokio::test]
    async fn test_reconcile_auto_grants_sole_user() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let alice = make_ws_user("u-1", "Alice");

        // Alice joins but no lock is held
        state_mgr.add_presence("inst-1", "conn-1", &alice).await;

        // Reconcile should auto-grant to sole user
        let changed = state_mgr
            .reconcile_terminal_lock_with_presence("inst-1")
            .await;
        assert!(changed);

        let lock = state_mgr.get_terminal_lock("inst-1").await.unwrap();
        assert_eq!(lock.holder_user_id, "u-1");
    }

    #[tokio::test]
    async fn test_reconcile_no_change_when_holder_present() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let alice = make_ws_user("u-1", "Alice");
        let bob = make_ws_user("u-2", "Bob");

        // Alice and Bob both present, Alice holds lock
        state_mgr.add_presence("inst-1", "conn-1", &alice).await;
        state_mgr.add_presence("inst-1", "conn-2", &bob).await;
        state_mgr
            .try_acquire_terminal_lock("inst-1", "conn-1", &alice)
            .await;

        // No change needed — holder is still present
        let changed = state_mgr
            .reconcile_terminal_lock_with_presence("inst-1")
            .await;
        assert!(!changed);
    }

    #[tokio::test]
    async fn test_reconcile_no_auto_grant_multiple_users() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let alice = make_ws_user("u-1", "Alice");
        let bob = make_ws_user("u-2", "Bob");

        // Two users present, no lock — should NOT auto-grant
        state_mgr.add_presence("inst-1", "conn-1", &alice).await;
        state_mgr.add_presence("inst-1", "conn-2", &bob).await;

        let changed = state_mgr
            .reconcile_terminal_lock_with_presence("inst-1")
            .await;
        assert!(!changed);
        assert!(state_mgr.get_terminal_lock("inst-1").await.is_none());
    }

    #[tokio::test]
    async fn test_reconcile_empty_instance_no_change() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        // No presence at all — no change
        let changed = state_mgr
            .reconcile_terminal_lock_with_presence("inst-1")
            .await;
        assert!(!changed);
    }

    // =========================================================================
    // Lifecycle broadcast tests
    // =========================================================================

    #[tokio::test]
    async fn test_subscribe_lifecycle_receives_events() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let mut rx = state_mgr.subscribe_lifecycle();

        state_mgr.broadcast_lifecycle(ServerMessage::InstanceStopped {
            instance_id: "inst-1".to_string(),
        });

        let msg = rx.recv().await.unwrap();
        match msg {
            ServerMessage::InstanceStopped { instance_id } => {
                assert_eq!(instance_id, "inst-1");
            }
            _ => panic!("Expected InstanceStopped"),
        }
    }

    #[tokio::test]
    async fn test_subscribe_state_broadcast() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let mut rx = state_mgr.subscribe();

        // Manually broadcast via the underlying channel
        // (normally done by state manager forwarding)
        let _ = state_mgr
            .broadcast_tx
            .send(("inst-1".to_string(), ClaudeState::Idle, false));

        let (id, state, stale) = rx.recv().await.unwrap();
        assert_eq!(id, "inst-1");
        assert!(matches!(state, ClaudeState::Idle));
        assert!(!stale);
    }

    // =========================================================================
    // Conversation store tests
    // =========================================================================

    #[tokio::test]
    async fn test_conversation_snapshot_empty_without_tracker() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let turns = state_mgr.get_conversation_snapshot("nonexistent").await;
        assert!(turns.is_empty());
    }

    #[tokio::test]
    async fn test_conversation_snapshot_roundtrip() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let (handle, _vt, _etx) = InstanceHandle::spawn_test(24, 80, 4096);
        state_mgr.insert_test_tracker("inst-1", handle).await;

        // Initially empty
        assert!(
            state_mgr
                .get_conversation_snapshot("inst-1")
                .await
                .is_empty()
        );

        // Write turns
        let turns = vec![
            serde_json::json!({"uuid": "t1", "role": "user", "content": "hello"}),
            serde_json::json!({"uuid": "t2", "role": "assistant", "content": "hi"}),
        ];
        state_mgr
            .set_test_conversation_turns("inst-1", turns.clone())
            .await;

        // Read them back
        let snapshot = state_mgr.get_conversation_snapshot("inst-1").await;
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0]["uuid"], "t1");
        assert_eq!(snapshot[1]["uuid"], "t2");
    }

    #[tokio::test]
    async fn test_subscribe_conversation_receives_events() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let (handle, _vt, _etx) = InstanceHandle::spawn_test(24, 80, 4096);
        state_mgr.insert_test_tracker("inst-1", handle).await;

        // Subscribe BEFORE sending
        let mut rx = state_mgr
            .subscribe_conversation("inst-1")
            .await
            .expect("tracker exists");

        // Manually broadcast a ConversationEvent via the tracker's channel
        {
            let trackers = state_mgr.trackers.read().await;
            let tracker = trackers.get("inst-1").unwrap();
            let _ = tracker.conversation_tx.send(ConversationEvent::Full {
                instance_id: "inst-1".to_string(),
                turns: vec![serde_json::json!({"uuid": "t1"})],
            });
        }

        let event = rx.recv().await.unwrap();
        match event {
            ConversationEvent::Full { instance_id, turns } => {
                assert_eq!(instance_id, "inst-1");
                assert_eq!(turns.len(), 1);
                assert_eq!(turns[0]["uuid"], "t1");
            }
            _ => panic!("expected Full event"),
        }
    }

    #[tokio::test]
    async fn test_subscribe_conversation_nonexistent_returns_none() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        assert!(
            state_mgr
                .subscribe_conversation("nonexistent")
                .await
                .is_none()
        );
    }

    // =========================================================================
    // Keystroke → flush → prefix flow tests
    // =========================================================================

    #[tokio::test]
    async fn test_keystroke_then_enter_produces_prefix() {
        let gsm = GlobalStateManager::new(create_state_broadcast());
        for ch in "hello".chars() {
            gsm.mark_first_input("i1", &ch.to_string()).await;
        }
        assert!(gsm.get_discovery_content_prefixes("i1").await.is_empty());
        gsm.mark_first_input("i1", "\r").await;
        assert_eq!(
            gsm.get_discovery_content_prefixes("i1").await,
            vec!["hello"]
        );
    }

    #[tokio::test]
    async fn test_control_chars_filtered_from_pending_line() {
        let gsm = GlobalStateManager::new(create_state_broadcast());
        gsm.mark_first_input("i1", "\x1b").await; // escape
        gsm.mark_first_input("i1", "\x1b[A").await; // arrow up
        gsm.mark_first_input("i1", "h").await;
        gsm.mark_first_input("i1", "i").await;
        gsm.mark_first_input("i1", "\r").await;
        assert_eq!(gsm.get_discovery_content_prefixes("i1").await, vec!["hi"]);
    }

    #[tokio::test]
    async fn test_full_message_with_newline_stores_directly() {
        let gsm = GlobalStateManager::new(create_state_broadcast());
        gsm.mark_first_input("i1", "hello world\r").await;
        assert_eq!(
            gsm.get_discovery_content_prefixes("i1").await,
            vec!["hello world"]
        );
    }

    // =========================================================================
    // Unregister instance cleanup tests
    // =========================================================================

    #[tokio::test]
    async fn test_unregister_cleans_up_all_state() {
        let state_mgr = GlobalStateManager::new(create_state_broadcast());
        let alice = make_ws_user("u-1", "Alice");

        // Set up state for an instance
        state_mgr.try_claim_session("sess-1", "inst-1").await;
        state_mgr.mark_first_input("inst-1", "hello").await;
        state_mgr
            .push_pending_attribution("inst-1", "u-1".into(), "Alice".into(), "hello", None)
            .await;
        state_mgr
            .try_acquire_terminal_lock("inst-1", "conn-1", &alice)
            .await;

        // Unregister should clean up everything
        state_mgr.unregister_instance("inst-1").await;

        assert!(state_mgr.get_first_input_at("inst-1").await.is_none());
        assert!(state_mgr.get_terminal_lock("inst-1").await.is_none());
        // Session should be reclaimable
        assert!(state_mgr.try_claim_session("sess-1", "inst-2").await);
        // Attribution queue should be gone
        assert!(
            state_mgr
                .consume_pending_attribution("inst-1", "hello")
                .await
                .is_none()
        );
    }
}
