//! WebSocket Handler
//!
//! Main multiplexed WebSocket connection handler with auth handshake.

use axum::extract::ws::{Message, WebSocket};
use futures::{sink::SinkExt, stream::StreamExt};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

use crate::auth::AuthUser;
use crate::config::ServerConfig;
use crate::handlers::interconnect::RpcContext;
use crate::identity::InstanceIdentity;
use crate::instance_manager::InstanceManager;
use crate::metrics::ServerMetrics;
use crate::repository::ConversationRepository;

use crate::virtual_terminal::ClientType;

use super::dispatch::{
    ConnectionContext, DispatchResult, auth_user_to_ws_user, disconnect_cleanup,
    dispatch_client_message,
};
use super::protocol::{
    BackpressureStats, ClientMessage, DEFAULT_MAX_HISTORY_BYTES, ServerMessage, WsUser,
};
use super::state_manager::GlobalStateManager;

/// Handle a multiplexed WebSocket connection.
///
/// If `is_loopback` is true, the connection gets Owner access immediately.
/// Otherwise, we run an ed25519 challenge-response handshake before entering
/// the message loop.
#[allow(clippy::too_many_arguments)]
pub async fn handle_multiplexed_ws(
    socket: WebSocket,
    instance_manager: Arc<InstanceManager>,
    state_manager: Arc<GlobalStateManager>,
    server_config: Option<Arc<ServerConfig>>,
    server_metrics: Option<Arc<ServerMetrics>>,
    repository: Arc<ConversationRepository>,
    identity: Option<Arc<InstanceIdentity>>,
    is_loopback: bool,
    connection_manager: Option<Arc<crate::interconnect::manager::ConnectionManager>>,
) {
    // Track connection opened
    if let Some(ref m) = server_metrics {
        m.connection_opened();
    }

    let (mut ws_sender, mut ws_receiver) = socket.split();

    // === Auth handshake phase (always runs, even on loopback) ===
    let (auth_user, ws_user) = match run_auth_handshake(
        &mut ws_sender,
        &mut ws_receiver,
        &repository,
        &state_manager,
        &identity,
        is_loopback,
    )
    .await
    {
        Some((user, ws)) => {
            info!(
                fingerprint = %user.fingerprint,
                display_name = %user.display_name,
                capability = ?user.capability,
                "multiplexed WS connection authenticated"
            );
            (user, ws)
        }
        None => {
            info!("WS connection closed during auth handshake");
            if let Some(ref m) = server_metrics {
                m.connection_closed();
            }
            return;
        }
    };

    // Get max history bytes from config or use default
    let max_history_bytes = server_config
        .as_ref()
        .map(|c| c.websocket.max_history_replay_bytes)
        .unwrap_or(DEFAULT_MAX_HISTORY_BYTES);

    // Per-connection backpressure stats
    let stats = Arc::new(BackpressureStats::new());

    // Unique ID for this connection (for presence tracking)
    let connection_id = uuid::Uuid::new_v4().to_string();

    // Channel for sending messages to the WebSocket
    let (tx, mut rx) = mpsc::channel::<ServerMessage>(100);

    // Build shared connection context
    let ctx = Arc::new(ConnectionContext::new(
        connection_id.clone(),
        Some(ws_user),
        tx.clone(),
        state_manager.clone(),
        instance_manager.clone(),
        Some(repository.clone()),
        max_history_bytes,
        ClientType::Web,
        connection_manager,
    ));

    // Build RPC context for interconnect dispatch
    let rpc_ctx = identity.map(|id| {
        Arc::new(RpcContext {
            repo: (*repository).clone(),
            identity: id,
            broadcast_tx: state_manager.lifecycle_sender(),
        })
    });

    // Send initial instance list with states
    let instances = instance_manager.list().await;
    if tx
        .send(ServerMessage::InstanceList { instances })
        .await
        .is_err()
    {
        warn!(conn_id = %connection_id, "Failed to send initial instance list - channel closed");
    }

    // Subscribe to state broadcasts from all instances
    let mut state_rx = state_manager.subscribe();
    let tx_state = tx.clone();
    let stats_state = stats.clone();
    let state_broadcast_task = async move {
        loop {
            match state_rx.recv().await {
                Ok((instance_id, state, stale)) => {
                    stats_state.record_state_send(1); // At least 1 receiver (us)
                    if tx_state
                        .send(ServerMessage::StateChange {
                            instance_id,
                            state,
                            stale,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    stats_state.record_lag(n);
                    warn!("State broadcast lagged by {} messages", n);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    // Subscribe to instance lifecycle broadcasts (created/stopped)
    let mut lifecycle_rx = state_manager.subscribe_lifecycle();
    let tx_lifecycle = tx.clone();
    let lifecycle_task = async move {
        loop {
            match lifecycle_rx.recv().await {
                Ok(msg) => {
                    if tx_lifecycle.send(msg).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    // Task to send messages to WebSocket
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

    // Task to handle incoming messages
    let ctx_input = ctx.clone();
    let auth_user_for_rpc = auth_user;
    let input_task = async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                        match dispatch_client_message(&ctx_input, client_msg).await {
                            DispatchResult::Handled => {}
                            DispatchResult::Unhandled(msg) => {
                                // Dispatch interconnect RPCs
                                if let Some(ref rpc) = rpc_ctx {
                                    dispatch_ws_rpc(rpc, &auth_user_for_rpc, &ctx_input.tx, msg)
                                        .await;
                                } else {
                                    let _ = ctx_input
                                        .tx
                                        .send(ServerMessage::Error {
                                            instance_id: None,
                                            message: "interconnect not available on this instance"
                                                .into(),
                                        })
                                        .await;
                                }
                            }
                        }
                    }
                }
                Ok(Message::Close(_)) => {
                    debug!("Client closed connection");
                    break;
                }
                Err(e) => {
                    error!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }
    };

    // Run all tasks
    tokio::select! {
        _ = state_broadcast_task => debug!("State broadcast task ended"),
        _ = lifecycle_task => debug!("Lifecycle task ended"),
        _ = sender_task => debug!("Sender task ended"),
        _ = input_task => debug!("Input task ended"),
    }

    // Shared disconnect cleanup (viewports, presence, terminal locks)
    disconnect_cleanup(&ctx).await;

    // Log backpressure stats on connection close
    let snapshot = stats.snapshot();

    // Track connection closed in server metrics
    if let Some(ref m) = server_metrics {
        m.connection_closed();
        // Record dropped messages in global metrics
        if snapshot.total_lagged_count > 0 {
            for _ in 0..snapshot.total_lagged_count {
                m.message_dropped();
            }
        }
    }
    if snapshot.total_lagged_count > 0 || snapshot.output_messages_lagged > 0 {
        warn!(
            "WebSocket connection closed with backpressure issues: state_broadcasts={}, output_lagged={}, total_dropped={}",
            snapshot.state_broadcasts_sent,
            snapshot.output_messages_lagged,
            snapshot.total_lagged_count
        );
    } else {
        info!(
            "Multiplexed WebSocket connection closed (state_broadcasts={})",
            snapshot.state_broadcasts_sent
        );
    }
}

// =============================================================================
// Auth handshake
// =============================================================================

use crab_city_auth::{Capability, PublicKey};
use futures::stream::SplitSink;
use futures::stream::SplitStream;

/// Run the auth handshake on a WebSocket connection.
///
/// 1. Send `Challenge { nonce }`
/// 2. Wait for `ChallengeResponse`, `PasswordAuth`, or `LoopbackAuth`
/// 3. Verify and return `(AuthUser, WsUser)`, or `None` if auth failed/connection closed
async fn run_auth_handshake(
    ws_sender: &mut SplitSink<WebSocket, Message>,
    ws_receiver: &mut SplitStream<WebSocket>,
    repository: &ConversationRepository,
    state_manager: &Arc<GlobalStateManager>,
    identity: &Option<Arc<InstanceIdentity>>,
    is_loopback: bool,
) -> Option<(AuthUser, WsUser)> {
    // Generate challenge nonce (32 random bytes, hex-encoded)
    let nonce_bytes: [u8; 32] = rand::random();
    let nonce_hex = hex_encode(&nonce_bytes);

    // Send challenge
    let challenge = ServerMessage::Challenge {
        nonce: nonce_hex.clone(),
    };
    let json = serde_json::to_string(&challenge).ok()?;
    ws_sender.send(Message::Text(json.into())).await.ok()?;

    // Wait for response (with 30s timeout)
    let response = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    return serde_json::from_str::<ClientMessage>(&text).ok();
                }
                Ok(Message::Close(_)) => return None,
                Err(_) => return None,
                _ => continue, // skip pings etc
            }
        }
        None
    })
    .await
    .ok()
    .flatten()?;

    match response {
        ClientMessage::ChallengeResponse {
            public_key,
            signature,
            display_name,
        } => {
            // Verify signature first
            let verified_pk =
                match verify_challenge_signature(ws_sender, &nonce_bytes, &public_key, &signature)
                    .await
                {
                    Some(pk) => pk,
                    None => return None,
                };

            // Try to authenticate with existing grant
            if let Some(result) = try_authenticate_with_grant(
                ws_sender,
                repository,
                &verified_pk,
                display_name.as_deref(),
            )
            .await
            {
                return Some(result);
            }

            // No grant — send AuthRequired and wait for RedeemInvite
            let msg = ServerMessage::AuthRequired {
                recovery: Some("redeem_invite".into()),
            };
            let json = serde_json::to_string(&msg).ok()?;
            ws_sender.send(Message::Text(json.into())).await.ok()?;

            // Wait for RedeemInvite with the verified key
            wait_for_invite_redeem(
                ws_sender,
                ws_receiver,
                repository,
                state_manager,
                identity,
                &verified_pk,
                display_name.as_deref(),
            )
            .await
        }
        ClientMessage::PasswordAuth {
            username,
            password,
            invite_token,
            display_name,
        } => {
            handle_password_auth(
                ws_sender,
                ws_receiver,
                repository,
                state_manager,
                identity,
                &username,
                &password,
                invite_token.as_deref(),
                display_name.as_deref(),
            )
            .await
        }
        ClientMessage::LoopbackAuth => {
            if !is_loopback {
                send_error(ws_sender, "LoopbackAuth only allowed from loopback").await;
                return None;
            }
            let user = AuthUser::loopback();
            let msg = ServerMessage::Authenticated {
                fingerprint: user.fingerprint.clone(),
                capability: format!("{}", user.capability),
            };
            let json = serde_json::to_string(&msg).ok()?;
            ws_sender.send(Message::Text(json.into())).await.ok()?;
            let ws = auth_user_to_ws_user(&user);
            Some((user, ws))
        }
        _ => {
            let err = ServerMessage::Error {
                instance_id: None,
                message: "expected ChallengeResponse, PasswordAuth, or LoopbackAuth".into(),
            };
            let json = serde_json::to_string(&err).ok()?;
            ws_sender.send(Message::Text(json.into())).await.ok()?;
            None
        }
    }
}

/// Decode and validate a hex-encoded public key from a message.
async fn decode_public_key(
    ws_sender: &mut SplitSink<WebSocket, Message>,
    public_key_hex: &str,
) -> Option<PublicKey> {
    let pk_bytes = hex_decode(public_key_hex)?;
    if pk_bytes.len() != 32 {
        send_error(ws_sender, "invalid public key length").await;
        return None;
    }
    let pk_array: [u8; 32] = pk_bytes.try_into().ok()?;
    let public_key = PublicKey::from_bytes(pk_array);

    if public_key.is_loopback() {
        send_error(ws_sender, "loopback key not allowed remotely").await;
        return None;
    }

    Some(public_key)
}

/// Verify the challenge-response signature and return the validated public key.
async fn verify_challenge_signature(
    ws_sender: &mut SplitSink<WebSocket, Message>,
    nonce_bytes: &[u8; 32],
    public_key_hex: &str,
    signature_hex: &str,
) -> Option<PublicKey> {
    let public_key = decode_public_key(ws_sender, public_key_hex).await?;

    let sig_bytes = hex_decode(signature_hex)?;
    if sig_bytes.len() != 64 {
        send_error(ws_sender, "invalid signature length").await;
        return None;
    }
    let sig_array: [u8; 64] = sig_bytes.try_into().ok()?;
    let signature = crab_city_auth::Signature::from_bytes(sig_array);

    if crab_city_auth::keys::verify(&public_key, nonce_bytes, &signature).is_err() {
        send_error(ws_sender, "signature verification failed").await;
        return None;
    }

    Some(public_key)
}

/// Try to authenticate with an existing grant. Returns None if no grant found (does NOT send error).
async fn try_authenticate_with_grant(
    ws_sender: &mut SplitSink<WebSocket, Message>,
    repository: &ConversationRepository,
    public_key: &PublicKey,
    display_name: Option<&str>,
) -> Option<(AuthUser, WsUser)> {
    let grant = repository
        .get_active_grant(public_key.as_bytes())
        .await
        .ok()
        .flatten()?;

    let cap: Capability = grant.capability.parse().unwrap_or_else(|_| {
        warn!(raw = %grant.capability, "corrupted capability string, falling back to View");
        Capability::View
    });
    let identity = repository
        .get_identity(public_key.as_bytes())
        .await
        .ok()
        .flatten();
    let name = display_name
        .map(|s| s.to_string())
        .or_else(|| identity.map(|i| i.display_name))
        .unwrap_or_else(|| public_key.fingerprint());

    let auth_user = AuthUser::from_grant(public_key.clone(), name, cap);

    let msg = ServerMessage::Authenticated {
        fingerprint: auth_user.fingerprint.clone(),
        capability: format!("{}", auth_user.capability),
    };
    let json = serde_json::to_string(&msg).ok()?;
    ws_sender.send(Message::Text(json.into())).await.ok()?;

    let ws_user = auth_user_to_ws_user(&auth_user);
    Some((auth_user, ws_user))
}

/// Wait for a `RedeemInvite` message after `AuthRequired` was sent.
/// The public key was already verified via challenge-response.
async fn wait_for_invite_redeem(
    ws_sender: &mut SplitSink<WebSocket, Message>,
    ws_receiver: &mut SplitStream<WebSocket>,
    repository: &ConversationRepository,
    state_manager: &Arc<GlobalStateManager>,
    identity: &Option<Arc<InstanceIdentity>>,
    verified_pk: &PublicKey,
    _display_name: Option<&str>,
) -> Option<(AuthUser, WsUser)> {
    let response = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    return serde_json::from_str::<ClientMessage>(&text).ok();
                }
                Ok(Message::Close(_)) => return None,
                Err(_) => return None,
                _ => continue,
            }
        }
        None
    })
    .await
    .ok()
    .flatten();

    match response {
        Some(ClientMessage::RedeemInvite {
            token,
            display_name,
            ..
        }) => {
            // Use the verified public key from the challenge, not the one in the message
            handle_invite_redeem(
                ws_sender,
                repository,
                state_manager,
                identity,
                verified_pk,
                &token,
                &display_name,
            )
            .await
        }
        _ => {
            send_error(ws_sender, "expected RedeemInvite after AuthRequired").await;
            None
        }
    }
}

/// Redeem an invite token for a given public key.
/// The public key must have been verified via challenge-response first.
async fn handle_invite_redeem(
    ws_sender: &mut SplitSink<WebSocket, Message>,
    repository: &ConversationRepository,
    state_manager: &Arc<GlobalStateManager>,
    identity: &Option<Arc<InstanceIdentity>>,
    public_key: &PublicKey,
    token: &str,
    display_name: &str,
) -> Option<(AuthUser, WsUser)> {
    use crate::handlers::interconnect;

    // Build RPC context with real identity and broadcast channel
    let id = match identity {
        Some(id) => id.clone(),
        None => {
            send_error(ws_sender, "instance identity not available").await;
            return None;
        }
    };
    let rpc_ctx = RpcContext {
        repo: repository.clone(),
        identity: id,
        broadcast_tx: state_manager.lifecycle_sender(),
    };

    let resp = interconnect::handle_redeem_invite(&rpc_ctx, public_key, token, display_name).await;

    if matches!(resp, ServerMessage::Error { .. }) {
        let json = serde_json::to_string(&resp).ok()?;
        ws_sender.send(Message::Text(json.into())).await.ok()?;
        return None;
    }

    // Send the InviteRedeemed response
    let json = serde_json::to_string(&resp).ok()?;
    ws_sender.send(Message::Text(json.into())).await.ok()?;

    // Look up the grant we just created
    let grant = match repository.get_active_grant(public_key.as_bytes()).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            send_error(ws_sender, "grant not found after invite redeem").await;
            return None;
        }
        Err(e) => {
            send_error(ws_sender, &format!("failed to look up grant: {e}")).await;
            return None;
        }
    };
    let cap: Capability = grant.capability.parse().unwrap_or_else(|_| {
        warn!(raw = %grant.capability, "corrupted capability string, falling back to View");
        Capability::View
    });
    let auth_user = AuthUser::from_grant(public_key.clone(), display_name.to_string(), cap);

    // Send Authenticated
    let msg = ServerMessage::Authenticated {
        fingerprint: auth_user.fingerprint.clone(),
        capability: format!("{}", auth_user.capability),
    };
    let json = serde_json::to_string(&msg).ok()?;
    ws_sender.send(Message::Text(json.into())).await.ok()?;

    let ws_user = auth_user_to_ws_user(&auth_user);
    Some((auth_user, ws_user))
}

// =============================================================================
// Interconnect RPC dispatch (same as iroh_transport but without disconnect_tx)
// =============================================================================

use crate::handlers::interconnect;

/// Dispatch interconnect RPCs on a WebSocket connection.
///
/// This mirrors `IrohTransport::dispatch_rpc` but without the disconnect channel
/// (WS connections close by dropping the socket, not via a channel).
async fn dispatch_ws_rpc(
    rpc_ctx: &RpcContext,
    auth_user: &AuthUser,
    tx: &mpsc::Sender<ServerMessage>,
    msg: ClientMessage,
) {
    let resp = match msg {
        ClientMessage::CreateInvite {
            capability,
            max_uses,
            expires_in_secs,
            label,
        } => {
            interconnect::handle_create_invite(
                rpc_ctx,
                auth_user,
                &capability,
                max_uses,
                expires_in_secs,
                label.as_deref(),
            )
            .await
        }
        ClientMessage::RedeemInvite {
            token,
            display_name,
            ..
        } => {
            interconnect::handle_redeem_invite(
                rpc_ctx,
                &auth_user.public_key,
                &token,
                &display_name,
            )
            .await
        }
        ClientMessage::RevokeInvite {
            nonce,
            suspend_derived,
        } => interconnect::handle_revoke_invite(rpc_ctx, auth_user, &nonce, suspend_derived).await,
        ClientMessage::ListInvites => interconnect::handle_list_invites(rpc_ctx, auth_user).await,
        ClientMessage::ListMembers => interconnect::handle_list_members(rpc_ctx, auth_user).await,
        ClientMessage::UpdateMember {
            public_key,
            capability,
            display_name,
        } => {
            interconnect::handle_update_member(
                rpc_ctx,
                auth_user,
                &public_key,
                capability.as_deref(),
                display_name.as_deref(),
            )
            .await
        }
        ClientMessage::SuspendMember { public_key } => {
            let (resp, _) =
                interconnect::handle_suspend_member(rpc_ctx, auth_user, &public_key).await;
            resp
        }
        ClientMessage::ReinstateMember { public_key } => {
            interconnect::handle_reinstate_member(rpc_ctx, auth_user, &public_key).await
        }
        ClientMessage::RemoveMember { public_key } => {
            let (resp, _) =
                interconnect::handle_remove_member(rpc_ctx, auth_user, &public_key).await;
            resp
        }
        ClientMessage::QueryEvents {
            target,
            event_type_prefix,
            limit,
            before_id,
        } => {
            interconnect::handle_query_events(
                rpc_ctx,
                auth_user,
                target.as_deref(),
                event_type_prefix.as_deref(),
                limit,
                before_id,
            )
            .await
        }
        ClientMessage::VerifyEvents { from_id, to_id } => {
            interconnect::handle_verify_events(rpc_ctx, auth_user, from_id, to_id).await
        }
        ClientMessage::GetEventProof { event_id } => {
            interconnect::handle_get_event_proof(rpc_ctx, auth_user, event_id).await
        }
        _ => {
            warn!("unexpected non-RPC message in dispatch_ws_rpc");
            return;
        }
    };
    let _ = tx.send(resp).await;
}

// =============================================================================
// Password auth bridge
// =============================================================================

/// Handle password-based auth: either authenticate a returning user or register
/// a new user with an invite token. The server generates a keypair on behalf of
/// the user so the interconnect identity system stays consistent.
#[allow(clippy::too_many_arguments)]
async fn handle_password_auth(
    ws_sender: &mut SplitSink<WebSocket, Message>,
    _ws_receiver: &mut SplitStream<WebSocket>,
    repository: &ConversationRepository,
    state_manager: &Arc<GlobalStateManager>,
    identity: &Option<Arc<InstanceIdentity>>,
    username: &str,
    password: &str,
    invite_token: Option<&str>,
    display_name: Option<&str>,
) -> Option<(AuthUser, WsUser)> {
    use crate::handlers::interconnect;

    // Try to find existing user
    let existing_user = repository
        .get_user_by_username(username)
        .await
        .ok()
        .flatten();

    match existing_user {
        Some(_user) => {
            // Returning user — verify password
            let verified = repository
                .verify_user_password(username, password)
                .await
                .ok()
                .flatten();

            if verified.is_none() {
                send_error(ws_sender, "invalid username or password").await;
                return None;
            }

            let user = verified.unwrap();

            // User has a linked keypair — look up their grant
            if let Some(ref pk_bytes) = user.public_key {
                if pk_bytes.len() == 32 {
                    let pk_arr: [u8; 32] = pk_bytes.as_slice().try_into().ok()?;
                    let public_key = PublicKey::from_bytes(pk_arr);

                    if let Some(result) = try_authenticate_with_grant(
                        ws_sender,
                        repository,
                        &public_key,
                        Some(&user.display_name),
                    )
                    .await
                    {
                        return Some(result);
                    }
                }
            }

            // User exists but has no keypair or no grant — generate one
            let signing_key = crab_city_auth::SigningKey::generate(&mut rand::rng());
            let public_key = signing_key.public_key();

            // Link the keypair to the user
            if let Err(e) = repository
                .link_user_public_key(&user.id, public_key.as_bytes())
                .await
            {
                send_error(ws_sender, &format!("failed to link keypair: {e}")).await;
                return None;
            }

            // Create identity + grant for this keypair
            let dn = display_name.unwrap_or(&user.display_name);
            if let Err(e) = repository.create_identity(public_key.as_bytes(), dn).await {
                warn!("identity may already exist: {e}");
            }

            // Create a collaborate-level grant
            let cap = Capability::Collaborate;
            let access_json = serde_json::to_string(&cap.access_rights()).ok()?;
            if let Err(e) = repository
                .create_grant(
                    public_key.as_bytes(),
                    &cap.to_string(),
                    &access_json,
                    "active",
                    None,
                    None,
                )
                .await
            {
                send_error(ws_sender, &format!("failed to create grant: {e}")).await;
                return None;
            }

            // Authenticate
            let auth_user = AuthUser::from_grant(public_key, dn.to_string(), cap);
            let msg = ServerMessage::Authenticated {
                fingerprint: auth_user.fingerprint.clone(),
                capability: format!("{}", auth_user.capability),
            };
            let json = serde_json::to_string(&msg).ok()?;
            ws_sender.send(Message::Text(json.into())).await.ok()?;

            let ws_user = auth_user_to_ws_user(&auth_user);
            Some((auth_user, ws_user))
        }
        None => {
            // New user — requires invite token
            let token = match invite_token {
                Some(t) => t,
                None => {
                    send_error(
                        ws_sender,
                        "user not found; invite token required for registration",
                    )
                    .await;
                    return None;
                }
            };

            let dn = display_name.unwrap_or(username);

            // Generate keypair for this user
            let signing_key = crab_city_auth::SigningKey::generate(&mut rand::rng());
            let public_key = signing_key.public_key();

            // Hash password
            let password_hash = match ConversationRepository::hash_password(password) {
                Ok(h) => h,
                Err(e) => {
                    send_error(ws_sender, &format!("failed to hash password: {e}")).await;
                    return None;
                }
            };

            // Create user row
            let now = chrono::Utc::now().timestamp();
            let user = crate::models::User {
                id: uuid::Uuid::new_v4().to_string(),
                username: username.to_string(),
                display_name: dn.to_string(),
                password_hash,
                public_key: Some(public_key.as_bytes().to_vec()),
                created_at: now,
                updated_at: now,
            };

            if let Err(e) = repository.create_user(&user).await {
                send_error(ws_sender, &format!("failed to create user: {e}")).await;
                return None;
            }

            // Note: identity is created by handle_redeem_invite, not here,
            // to avoid duplicate INSERT on member_identities.

            // Redeem invite via the real interconnect handler
            let id = match identity {
                Some(id) => id.clone(),
                None => {
                    send_error(ws_sender, "instance identity not available").await;
                    return None;
                }
            };
            let rpc_ctx = RpcContext {
                repo: repository.clone(),
                identity: id,
                broadcast_tx: state_manager.lifecycle_sender(),
            };

            let resp = interconnect::handle_redeem_invite(&rpc_ctx, &public_key, token, dn).await;

            if matches!(resp, ServerMessage::Error { .. }) {
                let json = serde_json::to_string(&resp).ok()?;
                ws_sender.send(Message::Text(json.into())).await.ok()?;
                return None;
            }

            // Send InviteRedeemed
            let json = serde_json::to_string(&resp).ok()?;
            ws_sender.send(Message::Text(json.into())).await.ok()?;

            // Look up the grant we just created
            let grant = repository
                .get_active_grant(public_key.as_bytes())
                .await
                .ok()
                .flatten()?;
            let cap: Capability = grant.capability.parse().unwrap_or_else(|_| {
                warn!(raw = %grant.capability, "corrupted capability, falling back to View");
                Capability::View
            });

            let auth_user = AuthUser::from_grant(public_key, dn.to_string(), cap);

            // Send Authenticated
            let msg = ServerMessage::Authenticated {
                fingerprint: auth_user.fingerprint.clone(),
                capability: format!("{}", auth_user.capability),
            };
            let json = serde_json::to_string(&msg).ok()?;
            ws_sender.send(Message::Text(json.into())).await.ok()?;

            let ws_user = auth_user_to_ws_user(&auth_user);
            Some((auth_user, ws_user))
        }
    }
}

// =============================================================================
// Hex helpers
// =============================================================================

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

async fn send_error(ws_sender: &mut SplitSink<WebSocket, Message>, msg: &str) {
    let err = ServerMessage::Error {
        instance_id: None,
        message: msg.into(),
    };
    if let Ok(json) = serde_json::to_string(&err) {
        let _ = ws_sender.send(Message::Text(json.into())).await;
    }
}
