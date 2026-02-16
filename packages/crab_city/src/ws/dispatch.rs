//! Shared message dispatcher for WebSocket and iroh transports.
//!
//! Both the multiplexed WS handler (`ws/handler.rs`) and the iroh transport
//! (`transport/iroh_transport.rs`) handle the same set of `ClientMessage`
//! variants with identical logic.  This module extracts that dispatch into a
//! single function so changes only need to happen in one place.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use crate::inference::StateSignal;
use crate::instance_manager::InstanceManager;
use crate::repository::ConversationRepository;
use crate::virtual_terminal::ClientType;

use super::focus::{handle_focus, send_conversation_since};
use super::protocol::{ClientMessage, PresenceUser, ServerMessage, WsUser};
use super::state_manager::{GlobalStateManager, TERMINAL_LOCK_TIMEOUT_SECS, TerminalLock};

/// Maximum access-denied errors per connection before silently dropping.
/// Prevents a malicious client from using the error path as an amplification vector.
const MAX_ACCESS_DENIALS: u32 = 10;

/// Check access rights, sending an error message if denied.
/// Returns true if access is granted.
///
/// Anonymous connections (`user: None`) are always denied — loopback connections
/// always carry a `WsUser` with Owner access, so `None` means truly unauthenticated.
async fn require_access(ctx: &ConnectionContext, type_: &str, action: &str) -> bool {
    match &ctx.user {
        Some(user) if user.has_access(type_, action) => true,
        Some(_) => {
            let prev = ctx.deny_count.fetch_add(1, Ordering::Relaxed);
            if prev < MAX_ACCESS_DENIALS {
                let _ = ctx
                    .tx
                    .send(ServerMessage::Error {
                        instance_id: None,
                        message: format!("access denied: requires {}:{}", type_, action),
                    })
                    .await;
            }
            false
        }
        None => {
            let prev = ctx.deny_count.fetch_add(1, Ordering::Relaxed);
            if prev < MAX_ACCESS_DENIALS {
                let _ = ctx
                    .tx
                    .send(ServerMessage::Error {
                        instance_id: None,
                        message: "authentication required".to_string(),
                    })
                    .await;
            }
            false
        }
    }
}

/// Per-connection context shared between the transport layer and the dispatcher.
pub(crate) struct ConnectionContext {
    pub connection_id: String,
    /// `None` for anonymous WebSocket connections; always `Some` for iroh.
    pub user: Option<WsUser>,
    pub focused_instance: Arc<RwLock<Option<String>>>,
    pub focus_cancel: Arc<RwLock<Option<CancellationToken>>>,
    pub tx: mpsc::Sender<ServerMessage>,
    pub state_manager: Arc<GlobalStateManager>,
    pub instance_manager: Arc<InstanceManager>,
    pub repository: Option<Arc<ConversationRepository>>,
    pub max_history_bytes: usize,
    pub session_select_tx: mpsc::Sender<String>,
    pub session_select_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<String>>>,
    pub client_type: ClientType,
    /// Counter for access-denied responses; after MAX_ACCESS_DENIALS, errors are silently dropped.
    pub deny_count: AtomicU32,
}

impl ConnectionContext {
    /// Create a new connection context.
    ///
    /// Allocates the focus state, session-select channel, and deny counter internally
    /// so callers don't duplicate boilerplate.
    pub(crate) fn new(
        connection_id: String,
        user: Option<WsUser>,
        tx: mpsc::Sender<ServerMessage>,
        state_manager: Arc<GlobalStateManager>,
        instance_manager: Arc<InstanceManager>,
        repository: Option<Arc<ConversationRepository>>,
        max_history_bytes: usize,
        client_type: ClientType,
    ) -> Self {
        let (session_select_tx, session_select_rx) = mpsc::channel::<String>(1);
        Self {
            connection_id,
            user,
            focused_instance: Arc::new(RwLock::new(None)),
            focus_cancel: Arc::new(RwLock::new(None)),
            tx,
            state_manager,
            instance_manager,
            repository,
            max_history_bytes,
            session_select_tx,
            session_select_rx: Arc::new(tokio::sync::Mutex::new(session_select_rx)),
            client_type,
            deny_count: AtomicU32::new(0),
        }
    }
}

/// Result of dispatching a `ClientMessage`.
pub(crate) enum DispatchResult {
    /// The message was handled by the shared dispatcher.
    Handled,
    /// The message is an interconnect RPC — the caller must handle it.
    Unhandled(ClientMessage),
}

/// Dispatch a single `ClientMessage` using the shared connection context.
///
/// Returns `DispatchResult::Unhandled` for interconnect RPC variants so that
/// the iroh transport can route them to the RPC handler.
pub(crate) async fn dispatch_client_message(
    ctx: &ConnectionContext,
    msg: ClientMessage,
) -> DispatchResult {
    match msg {
        ClientMessage::Focus {
            instance_id,
            since_uuid,
        } => {
            if !require_access(ctx, "terminals", "read").await {
                return DispatchResult::Handled;
            }
            // Cancel previous focus tasks
            {
                let mut guard = ctx.focus_cancel.write().await;
                if let Some(cancel) = guard.take() {
                    cancel.cancel();
                }
            }

            // Create new cancellation token
            let cancel_token = CancellationToken::new();
            {
                let mut guard = ctx.focus_cancel.write().await;
                *guard = Some(cancel_token.clone());
            }

            // Update focused instance and get previous
            let prev_instance = {
                let mut guard = ctx.focused_instance.write().await;
                let prev = guard.take();
                *guard = Some(instance_id.clone());
                prev
            };

            // Presence operations require an authenticated user
            if let Some(ref user) = ctx.user {
                // Remove from previous instance
                if let Some(ref prev_id) = prev_instance {
                    let users = ctx
                        .state_manager
                        .remove_presence_from_instance(prev_id, &ctx.connection_id)
                        .await;
                    ctx.state_manager
                        .broadcast_lifecycle(ServerMessage::PresenceUpdate {
                            instance_id: prev_id.clone(),
                            users,
                        });
                }

                // Add to new instance
                let users = ctx
                    .state_manager
                    .add_presence(&instance_id, &ctx.connection_id, user)
                    .await;
                ctx.state_manager
                    .broadcast_lifecycle(ServerMessage::PresenceUpdate {
                        instance_id: instance_id.clone(),
                        users,
                    });

                // Reconcile terminal locks
                ctx.state_manager
                    .reconcile_terminal_lock_with_presence(&instance_id)
                    .await;
                if let Some(ref prev_id) = prev_instance {
                    ctx.state_manager
                        .reconcile_terminal_lock_with_presence(prev_id)
                        .await;
                    broadcast_terminal_lock_update(&ctx.state_manager, prev_id).await;
                }

                // Send current lock state to newly focused client
                let lock_msg = build_lock_update_message(
                    &instance_id,
                    ctx.state_manager.get_terminal_lock(&instance_id).await,
                );
                let _ = ctx.tx.send(lock_msg).await;
                // Broadcast to all clients
                broadcast_terminal_lock_update(&ctx.state_manager, &instance_id).await;
            }

            // Spawn focus handler
            let tx_focus = ctx.tx.clone();
            let state_mgr_focus = ctx.state_manager.clone();
            let inst_mgr_focus = ctx.instance_manager.clone();
            let session_rx = ctx.session_select_rx.clone();
            let max_history = ctx.max_history_bytes;
            let repo_focus = ctx.repository.clone();

            tokio::spawn(async move {
                handle_focus(
                    instance_id,
                    since_uuid,
                    cancel_token,
                    state_mgr_focus,
                    inst_mgr_focus,
                    tx_focus,
                    session_rx,
                    max_history,
                    repo_focus,
                )
                .await;
            });

            DispatchResult::Handled
        }
        ClientMessage::ConversationSync { since_uuid } => {
            if !require_access(ctx, "content", "read").await {
                return DispatchResult::Handled;
            }
            if let Some(id) = ctx.focused_instance.read().await.clone() {
                let tx_sync = ctx.tx.clone();
                let state_mgr_sync = ctx.state_manager.clone();
                let repo_sync = ctx.repository.clone();
                tokio::spawn(async move {
                    if let Err(e) = send_conversation_since(
                        &id,
                        since_uuid.as_deref(),
                        &state_mgr_sync,
                        &tx_sync,
                        repo_sync
                            .as_ref()
                            .map(|r| r as &Arc<ConversationRepository>),
                    )
                    .await
                    {
                        error!("Failed to sync conversation: {}", e);
                    }
                });
            }
            DispatchResult::Handled
        }
        ClientMessage::Input {
            instance_id,
            data,
            task_id,
        } => {
            if !require_access(ctx, "terminals", "input").await {
                return DispatchResult::Handled;
            }
            // Send to state manager for tool detection
            ctx.state_manager
                .send_signal(
                    &instance_id,
                    StateSignal::TerminalInput { data: data.clone() },
                )
                .await;

            if let Some(handle) = ctx.state_manager.get_handle(&instance_id).await {
                if let Err(e) = handle.write_input(&data).await {
                    error!(instance = %instance_id, "Failed to write to PTY: {}", e);
                    let _ = ctx
                        .tx
                        .send(ServerMessage::Error {
                            instance_id: Some(instance_id.clone()),
                            message: format!("Failed to send input: {}", e),
                        })
                        .await;
                } else {
                    // Record first input time for session discovery
                    if handle.get_session_id().await.is_none() {
                        ctx.state_manager.mark_first_input(&instance_id).await;
                    }

                    // Keep terminal lock fresh
                    ctx.state_manager
                        .touch_terminal_lock(&instance_id, &ctx.connection_id)
                        .await;

                    // Record input attribution if authenticated
                    if let Some(ref user) = ctx.user {
                        let trimmed = data.trim();
                        if !trimmed.is_empty() && trimmed != "\r" && trimmed != "\n" {
                            ctx.state_manager
                                .push_pending_attribution(
                                    &instance_id,
                                    user.user_id.clone(),
                                    user.display_name.clone(),
                                    trimmed,
                                    task_id,
                                )
                                .await;

                            if let Some(repo) = &ctx.repository {
                                let attr = crate::models::InputAttribution {
                                    id: None,
                                    instance_id: instance_id.clone(),
                                    user_id: user.user_id.clone(),
                                    display_name: user.display_name.clone(),
                                    timestamp: chrono::Utc::now().timestamp(),
                                    entry_uuid: None,
                                    content_preview: Some(trimmed.chars().take(100).collect()),
                                    task_id,
                                };
                                let repo = repo.clone();
                                let inst_id = instance_id.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = repo.record_input_attribution(&attr).await {
                                        warn!(instance = %inst_id, "Failed to record input attribution: {}", e);
                                    }
                                });
                            }
                        }
                    }
                }
            } else {
                warn!(instance = %instance_id, "Instance handle not found for input");
                let _ = ctx
                    .tx
                    .send(ServerMessage::Error {
                        instance_id: Some(instance_id.clone()),
                        message: "Instance no longer available".to_string(),
                    })
                    .await;
            }
            DispatchResult::Handled
        }
        ClientMessage::Resize {
            instance_id,
            rows,
            cols,
        } => {
            if let Some(handle) = ctx.state_manager.get_handle(&instance_id).await {
                if let Err(e) = handle
                    .update_viewport_and_resize(&ctx.connection_id, rows, cols, ctx.client_type)
                    .await
                {
                    warn!("Failed to resize PTY for {}: {}", instance_id, e);
                }
            }
            DispatchResult::Handled
        }
        ClientMessage::TerminalVisible {
            instance_id,
            rows,
            cols,
        } => {
            if let Some(handle) = ctx.state_manager.get_handle(&instance_id).await {
                if let Err(e) = handle
                    .update_viewport_and_resize(&ctx.connection_id, rows, cols, ctx.client_type)
                    .await
                {
                    warn!("Failed to resize PTY for {}: {}", instance_id, e);
                }
            }
            DispatchResult::Handled
        }
        ClientMessage::TerminalHidden { instance_id } => {
            if let Some(handle) = ctx.state_manager.get_handle(&instance_id).await {
                if let Err(e) = handle
                    .set_active_and_resize(&ctx.connection_id, false)
                    .await
                {
                    warn!("Failed to resize PTY for {}: {}", instance_id, e);
                }
            }
            DispatchResult::Handled
        }
        ClientMessage::SessionSelect { session_id } => {
            debug!("Session selected: {}", session_id);
            if ctx
                .session_select_tx
                .send(session_id.clone())
                .await
                .is_err()
            {
                warn!(session = %session_id, "Failed to send session selection - receiver dropped");
            }
            DispatchResult::Handled
        }
        ClientMessage::Lobby { channel, payload } => {
            if !require_access(ctx, "chat", "send").await {
                return DispatchResult::Handled;
            }
            ctx.state_manager
                .broadcast_lifecycle(ServerMessage::LobbyBroadcast {
                    sender_id: ctx.connection_id.clone(),
                    channel,
                    payload,
                });
            DispatchResult::Handled
        }
        ClientMessage::TerminalLockRequest { instance_id } => {
            if !require_access(ctx, "terminals", "input").await {
                return DispatchResult::Handled;
            }
            if let Some(ref user) = ctx.user {
                let acquired = ctx
                    .state_manager
                    .try_acquire_terminal_lock(&instance_id, &ctx.connection_id, user)
                    .await;
                if acquired {
                    broadcast_terminal_lock_update(&ctx.state_manager, &instance_id).await;
                } else {
                    let msg = build_lock_update_message(
                        &instance_id,
                        ctx.state_manager.get_terminal_lock(&instance_id).await,
                    );
                    let _ = ctx.tx.send(msg).await;
                }
            }
            DispatchResult::Handled
        }
        ClientMessage::TerminalLockRelease { instance_id } => {
            let released = ctx
                .state_manager
                .release_terminal_lock(&instance_id, &ctx.connection_id)
                .await;
            if released {
                ctx.state_manager
                    .reconcile_terminal_lock_with_presence(&instance_id)
                    .await;
                broadcast_terminal_lock_update(&ctx.state_manager, &instance_id).await;
            }
            DispatchResult::Handled
        }
        ClientMessage::ChatSend {
            scope,
            content,
            uuid,
            topic,
        } => {
            if !require_access(ctx, "chat", "send").await {
                return DispatchResult::Handled;
            }
            if let (Some(user), Some(repo)) = (&ctx.user, &ctx.repository) {
                let msg = crate::models::ChatMessage {
                    id: None,
                    uuid: uuid.clone(),
                    scope: scope.clone(),
                    user_id: user.user_id.clone(),
                    display_name: user.display_name.clone(),
                    content: content.clone(),
                    created_at: chrono::Utc::now().timestamp(),
                    forwarded_from: None,
                    topic: topic.clone(),
                };
                let repo = repo.clone();
                let state_mgr_chat = ctx.state_manager.clone();
                tokio::spawn(async move {
                    match repo.insert_chat_message(&msg).await {
                        Ok(id) => {
                            state_mgr_chat.broadcast_lifecycle(ServerMessage::ChatMessage {
                                id,
                                uuid: msg.uuid,
                                scope: msg.scope,
                                user_id: msg.user_id,
                                display_name: msg.display_name,
                                content: msg.content,
                                created_at: msg.created_at,
                                forwarded_from: None,
                                topic: msg.topic,
                            });
                        }
                        Err(e) => {
                            warn!("Failed to insert chat message: {}", e);
                        }
                    }
                });
            }
            DispatchResult::Handled
        }
        ClientMessage::ChatHistory {
            scope,
            before_id,
            limit,
            topic,
        } => {
            if !require_access(ctx, "content", "read").await {
                return DispatchResult::Handled;
            }
            if let Some(ref repo) = ctx.repository {
                let repo = repo.clone();
                let tx_chat = ctx.tx.clone();
                let limit = limit.unwrap_or(50).min(100);
                tokio::spawn(async move {
                    match repo
                        .get_chat_history(&scope, before_id, limit, topic.as_deref())
                        .await
                    {
                        Ok((messages, has_more)) => {
                            let msgs: Vec<serde_json::Value> = messages
                                .into_iter()
                                .filter_map(|m| serde_json::to_value(&m).ok())
                                .collect();
                            let _ = tx_chat
                                .send(ServerMessage::ChatHistoryResponse {
                                    scope,
                                    messages: msgs,
                                    has_more,
                                })
                                .await;
                        }
                        Err(e) => {
                            warn!("Failed to get chat history: {}", e);
                        }
                    }
                });
            }
            DispatchResult::Handled
        }
        ClientMessage::ChatForward {
            message_id,
            target_scope,
        } => {
            if !require_access(ctx, "chat", "send").await {
                return DispatchResult::Handled;
            }
            if let (Some(user), Some(repo)) = (&ctx.user, &ctx.repository) {
                let repo = repo.clone();
                let state_mgr_chat = ctx.state_manager.clone();
                let user = user.clone();
                tokio::spawn(async move {
                    if let Ok(Some(original)) = repo.get_chat_message_by_id(message_id).await {
                        let fwd = crate::models::ChatMessage {
                            id: None,
                            uuid: uuid::Uuid::new_v4().to_string(),
                            scope: target_scope.clone(),
                            user_id: user.user_id.clone(),
                            display_name: original.display_name.clone(),
                            content: original.content.clone(),
                            created_at: chrono::Utc::now().timestamp(),
                            forwarded_from: Some(original.scope.clone()),
                            topic: original.topic.clone(),
                        };
                        if let Ok(id) = repo.insert_chat_message(&fwd).await {
                            state_mgr_chat.broadcast_lifecycle(ServerMessage::ChatMessage {
                                id,
                                uuid: fwd.uuid,
                                scope: fwd.scope,
                                user_id: fwd.user_id,
                                display_name: fwd.display_name,
                                content: fwd.content,
                                created_at: fwd.created_at,
                                forwarded_from: fwd.forwarded_from,
                                topic: fwd.topic,
                            });
                        }
                    }
                });
            }
            DispatchResult::Handled
        }
        ClientMessage::ChatTopics { scope } => {
            if !require_access(ctx, "content", "read").await {
                return DispatchResult::Handled;
            }
            if let Some(ref repo) = ctx.repository {
                let repo = repo.clone();
                let tx_chat = ctx.tx.clone();
                tokio::spawn(async move {
                    match repo.get_chat_topics(&scope).await {
                        Ok(topics) => {
                            let _ = tx_chat
                                .send(ServerMessage::ChatTopicsResponse { scope, topics })
                                .await;
                        }
                        Err(e) => {
                            warn!("Failed to get chat topics: {}", e);
                        }
                    }
                });
            }
            DispatchResult::Handled
        }

        // Interconnect RPCs — caller handles these
        msg @ (ClientMessage::CreateInvite { .. }
        | ClientMessage::RedeemInvite { .. }
        | ClientMessage::RevokeInvite { .. }
        | ClientMessage::ListInvites
        | ClientMessage::ListMembers
        | ClientMessage::UpdateMember { .. }
        | ClientMessage::SuspendMember { .. }
        | ClientMessage::ReinstateMember { .. }
        | ClientMessage::RemoveMember { .. }
        | ClientMessage::QueryEvents { .. }
        | ClientMessage::VerifyEvents { .. }
        | ClientMessage::GetEventProof { .. }) => DispatchResult::Unhandled(msg),
    }
}

/// Clean up connection state on disconnect.
///
/// Consolidates the identical cleanup logic from both WS and iroh handlers:
/// cancel focus tasks, remove viewports, remove presence, release terminal locks.
pub(crate) async fn disconnect_cleanup(ctx: &ConnectionContext) {
    // 1. Cancel focus tasks
    {
        let mut guard = ctx.focus_cancel.write().await;
        if let Some(cancel) = guard.take() {
            cancel.cancel();
        }
    }

    // 2. Remove viewports from ALL instances
    for (instance_id, handle) in ctx.state_manager.all_handles().await {
        if let Err(e) = handle.remove_client_and_resize(&ctx.connection_id).await {
            warn!(
                instance = %instance_id,
                "Failed to clean up viewport on disconnect: {}", e
            );
        }
    }

    // 3+4. Presence and terminal lock cleanup (only for authenticated users)
    if ctx.user.is_some() {
        let updates = ctx
            .state_manager
            .remove_presence_all(&ctx.connection_id)
            .await;
        for (instance_id, users) in &updates {
            ctx.state_manager
                .broadcast_lifecycle(ServerMessage::PresenceUpdate {
                    instance_id: instance_id.clone(),
                    users: users.clone(),
                });
        }
        // Release terminal locks and reconcile
        for (instance_id, _) in &updates {
            let released = ctx
                .state_manager
                .release_terminal_lock(instance_id, &ctx.connection_id)
                .await;
            if released {
                ctx.state_manager
                    .reconcile_terminal_lock_with_presence(instance_id)
                    .await;
            }
            broadcast_terminal_lock_update(&ctx.state_manager, instance_id).await;
        }
    }
}

/// Build a `TerminalLockUpdate` message from the current lock state.
pub(crate) fn build_lock_update_message(
    instance_id: &str,
    lock: Option<TerminalLock>,
) -> ServerMessage {
    match lock {
        Some(lock) => {
            let now = chrono::Utc::now();
            let elapsed = (now - lock.last_activity).num_seconds().max(0) as u64;
            let timeout = TERMINAL_LOCK_TIMEOUT_SECS as u64;
            let expires_in = timeout.saturating_sub(elapsed);

            ServerMessage::TerminalLockUpdate {
                instance_id: instance_id.to_string(),
                holder: Some(PresenceUser {
                    user_id: lock.holder_user_id,
                    display_name: lock.holder_display_name,
                }),
                last_activity: Some(lock.last_activity.to_rfc3339()),
                expires_in_secs: Some(expires_in),
            }
        }
        None => ServerMessage::TerminalLockUpdate {
            instance_id: instance_id.to_string(),
            holder: None,
            last_activity: None,
            expires_in_secs: None,
        },
    }
}

/// Broadcast the current terminal lock state for an instance to all connected clients.
pub(crate) async fn broadcast_terminal_lock_update(
    state_mgr: &Arc<GlobalStateManager>,
    instance_id: &str,
) {
    let lock = state_mgr.get_terminal_lock(instance_id).await;
    let msg = build_lock_update_message(instance_id, lock);
    state_mgr.broadcast_lifecycle(msg);
}

/// Convert an `AuthUser` to a `WsUser` for presence/lock APIs.
pub(crate) fn auth_user_to_ws_user(auth_user: &crate::auth::AuthUser) -> WsUser {
    WsUser {
        user_id: auth_user.fingerprint.clone(),
        display_name: auth_user.display_name.clone(),
        access: Some(auth_user.access.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use crab_city_auth::Capability;

    #[test]
    fn build_lock_update_no_lock() {
        let msg = build_lock_update_message("inst-1", None);
        match msg {
            ServerMessage::TerminalLockUpdate {
                instance_id,
                holder,
                last_activity,
                expires_in_secs,
            } => {
                assert_eq!(instance_id, "inst-1");
                assert!(holder.is_none());
                assert!(last_activity.is_none());
                assert!(expires_in_secs.is_none());
            }
            _ => panic!("Expected TerminalLockUpdate"),
        }
    }

    #[test]
    fn build_lock_update_with_recent_lock() {
        let lock = TerminalLock {
            holder_connection_id: "conn-1".to_string(),
            holder_user_id: "user-1".to_string(),
            holder_display_name: "Alice".to_string(),
            last_activity: Utc::now(),
        };

        let msg = build_lock_update_message("inst-1", Some(lock));
        match msg {
            ServerMessage::TerminalLockUpdate {
                instance_id,
                holder,
                last_activity,
                expires_in_secs,
            } => {
                assert_eq!(instance_id, "inst-1");
                let h = holder.unwrap();
                assert_eq!(h.user_id, "user-1");
                assert_eq!(h.display_name, "Alice");
                assert!(last_activity.is_some());
                // Recent lock should have ~full timeout remaining
                let remaining = expires_in_secs.unwrap();
                assert!(remaining > 0);
                assert!(remaining <= TERMINAL_LOCK_TIMEOUT_SECS as u64);
            }
            _ => panic!("Expected TerminalLockUpdate"),
        }
    }

    #[test]
    fn build_lock_update_expired_lock() {
        let lock = TerminalLock {
            holder_connection_id: "conn-1".to_string(),
            holder_user_id: "user-1".to_string(),
            holder_display_name: "Alice".to_string(),
            // Way in the past — well beyond any timeout
            last_activity: Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
        };

        let msg = build_lock_update_message("inst-1", Some(lock));
        match msg {
            ServerMessage::TerminalLockUpdate {
                expires_in_secs, ..
            } => {
                // Expired lock should have 0 remaining (saturating_sub)
                assert_eq!(expires_in_secs.unwrap(), 0);
            }
            _ => panic!("Expected TerminalLockUpdate"),
        }
    }

    #[test]
    fn build_lock_update_last_activity_is_rfc3339() {
        let lock = TerminalLock {
            holder_connection_id: "conn-1".to_string(),
            holder_user_id: "user-1".to_string(),
            holder_display_name: "Bob".to_string(),
            last_activity: Utc::now(),
        };

        let msg = build_lock_update_message("inst-2", Some(lock));
        match msg {
            ServerMessage::TerminalLockUpdate { last_activity, .. } => {
                let ts = last_activity.unwrap();
                // Should parse as valid RFC 3339
                chrono::DateTime::parse_from_rfc3339(&ts).unwrap();
            }
            _ => panic!("Expected TerminalLockUpdate"),
        }
    }

    // === Access gating tests ===

    fn make_ws_user_with_access(cap: Capability) -> WsUser {
        WsUser {
            user_id: "test-user".into(),
            display_name: "Test".into(),
            access: Some(cap.access_rights()),
        }
    }

    fn make_test_ctx(user: Option<WsUser>) -> (ConnectionContext, mpsc::Receiver<ServerMessage>) {
        use crate::ws::state_manager::create_state_broadcast;

        let (tx, rx) = mpsc::channel(16);
        let ctx = ConnectionContext::new(
            "test-conn".into(),
            user,
            tx,
            Arc::new(GlobalStateManager::new(create_state_broadcast())),
            Arc::new(InstanceManager::new("claude".into(), 0, 64 * 1024)),
            None,
            64 * 1024,
            ClientType::Web,
        );
        (ctx, rx)
    }

    #[tokio::test]
    async fn dispatch_input_denied_without_terminal_input() {
        let user = make_ws_user_with_access(Capability::View);
        let (ctx, mut rx) = make_test_ctx(Some(user));

        let result = dispatch_client_message(
            &ctx,
            ClientMessage::Input {
                instance_id: "inst-1".into(),
                data: "hello".into(),
                task_id: None,
            },
        )
        .await;

        assert!(matches!(result, DispatchResult::Handled));
        let msg = rx.try_recv().unwrap();
        match msg {
            ServerMessage::Error { message, .. } => {
                assert!(message.contains("access denied"));
                assert!(message.contains("terminals:input"));
            }
            _ => panic!("Expected Error, got {:?}", msg),
        }
    }

    #[tokio::test]
    async fn dispatch_input_allowed_with_collaborate() {
        let user = make_ws_user_with_access(Capability::Collaborate);
        let (ctx, mut rx) = make_test_ctx(Some(user));

        let result = dispatch_client_message(
            &ctx,
            ClientMessage::Input {
                instance_id: "inst-1".into(),
                data: "hello".into(),
                task_id: None,
            },
        )
        .await;

        assert!(matches!(result, DispatchResult::Handled));
        // Should pass access gating but fail at "instance not found" —
        // that proves the access check passed and execution continued.
        let msg = rx
            .try_recv()
            .expect("expected an error about missing instance");
        match msg {
            ServerMessage::Error { message, .. } => {
                assert!(
                    message.contains("Instance no longer available"),
                    "Expected 'Instance no longer available', got: {}",
                    message
                );
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn dispatch_chat_denied_without_chat_send() {
        let user = make_ws_user_with_access(Capability::View);
        let (ctx, mut rx) = make_test_ctx(Some(user));

        let result = dispatch_client_message(
            &ctx,
            ClientMessage::ChatSend {
                scope: "global".into(),
                content: "hi".into(),
                uuid: "u1".into(),
                topic: None,
            },
        )
        .await;

        assert!(matches!(result, DispatchResult::Handled));
        let msg = rx.try_recv().unwrap();
        match msg {
            ServerMessage::Error { message, .. } => {
                assert!(message.contains("access denied"));
                assert!(message.contains("chat:send"));
            }
            _ => panic!("Expected Error, got {:?}", msg),
        }
    }

    #[tokio::test]
    async fn dispatch_focus_denied_without_terminal_read() {
        // Construct a user with only chat:send (no terminals:read)
        let user = WsUser {
            user_id: "test-user".into(),
            display_name: "Test".into(),
            access: Some(crab_city_auth::AccessRights::single("chat", "send")),
        };
        let (ctx, mut rx) = make_test_ctx(Some(user));

        let result = dispatch_client_message(
            &ctx,
            ClientMessage::Focus {
                instance_id: "inst-1".into(),
                since_uuid: None,
            },
        )
        .await;

        assert!(matches!(result, DispatchResult::Handled));
        let msg = rx.try_recv().unwrap();
        match msg {
            ServerMessage::Error { message, .. } => {
                assert!(message.contains("access denied"));
                assert!(message.contains("terminals:read"));
            }
            _ => panic!("Expected Error, got {:?}", msg),
        }
    }

    #[tokio::test]
    async fn dispatch_lock_denied_without_terminal_input() {
        let user = make_ws_user_with_access(Capability::View);
        let (ctx, mut rx) = make_test_ctx(Some(user));

        let result = dispatch_client_message(
            &ctx,
            ClientMessage::TerminalLockRequest {
                instance_id: "inst-1".into(),
            },
        )
        .await;

        assert!(matches!(result, DispatchResult::Handled));
        let msg = rx.try_recv().unwrap();
        match msg {
            ServerMessage::Error { message, .. } => {
                assert!(message.contains("access denied"));
                assert!(message.contains("terminals:input"));
            }
            _ => panic!("Expected Error, got {:?}", msg),
        }
    }

    #[tokio::test]
    async fn dispatch_resize_always_allowed() {
        let user = make_ws_user_with_access(Capability::View);
        let (ctx, mut rx) = make_test_ctx(Some(user));

        let result = dispatch_client_message(
            &ctx,
            ClientMessage::Resize {
                instance_id: "inst-1".into(),
                rows: 24,
                cols: 80,
            },
        )
        .await;

        assert!(matches!(result, DispatchResult::Handled));
        // View user should NOT get access denied for Resize
        match rx.try_recv() {
            Ok(ServerMessage::Error { message, .. }) => {
                assert!(
                    !message.contains("access denied"),
                    "Resize should be allowed for View: {}",
                    message
                );
            }
            _ => {} // No message is expected — Resize is ungated
        }
    }

    #[tokio::test]
    async fn dispatch_anonymous_user_blocked() {
        let (ctx, mut rx) = make_test_ctx(None);

        let result = dispatch_client_message(
            &ctx,
            ClientMessage::Input {
                instance_id: "inst-1".into(),
                data: "hello".into(),
                task_id: None,
            },
        )
        .await;

        assert!(matches!(result, DispatchResult::Handled));
        let msg = rx.try_recv().unwrap();
        match msg {
            ServerMessage::Error { message, .. } => {
                assert!(message.contains("authentication required"));
            }
            _ => panic!("Expected Error, got {:?}", msg),
        }
    }

    #[tokio::test]
    async fn dispatch_lobby_denied_without_chat_send() {
        let user = make_ws_user_with_access(Capability::View);
        let (ctx, mut rx) = make_test_ctx(Some(user));

        let result = dispatch_client_message(
            &ctx,
            ClientMessage::Lobby {
                channel: "test".into(),
                payload: serde_json::json!({}),
            },
        )
        .await;

        assert!(matches!(result, DispatchResult::Handled));
        let msg = rx.try_recv().unwrap();
        match msg {
            ServerMessage::Error { message, .. } => {
                assert!(message.contains("access denied"));
                assert!(message.contains("chat:send"));
            }
            _ => panic!("Expected Error, got {:?}", msg),
        }
    }

    #[tokio::test]
    async fn dispatch_deny_count_rate_limits_errors() {
        let user = make_ws_user_with_access(Capability::View);
        let (ctx, mut rx) = make_test_ctx(Some(user));

        // Send MAX_ACCESS_DENIALS + 5 denied messages
        for _ in 0..(MAX_ACCESS_DENIALS + 5) {
            dispatch_client_message(
                &ctx,
                ClientMessage::Input {
                    instance_id: "inst-1".into(),
                    data: "x".into(),
                    task_id: None,
                },
            )
            .await;
        }

        // Count error messages received
        let mut error_count = 0u32;
        while let Ok(_msg) = rx.try_recv() {
            error_count += 1;
        }

        // Should have exactly MAX_ACCESS_DENIALS errors, not MAX_ACCESS_DENIALS + 5
        assert_eq!(error_count, MAX_ACCESS_DENIALS);
    }
}
