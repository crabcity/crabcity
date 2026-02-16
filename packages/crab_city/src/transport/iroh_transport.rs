//! iroh QUIC transport: endpoint management, connection accept loop, message dispatch.
//!
//! This is the core transport for Crab City. Each connecting client authenticates
//! via their Ed25519 public key (extracted from the QUIC handshake). Clients with
//! an active `MemberGrant` get full access; clients without a grant can redeem an
//! invite as their first message.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use crab_city_auth::{Capability, PublicKey};
use iroh::{Endpoint, EndpointAddr, RelayMode, RelayUrl};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::auth::AuthUser;
use crate::config::ServerConfig;
use crate::handlers::interconnect::{self, RpcContext};
use crate::identity::InstanceIdentity;
use crate::instance_manager::InstanceManager;
use crate::repository::ConversationRepository;
use crate::transport::framing;
use crate::virtual_terminal::ClientType;
use crate::ws::DEFAULT_MAX_HISTORY_BYTES;
use crate::ws::dispatch::{
    ConnectionContext, DispatchResult, auth_user_to_ws_user, disconnect_cleanup,
    dispatch_client_message,
};
use crate::ws::{ClientMessage, GlobalStateManager, ServerMessage};

/// Collapse `Result<ServerMessage, ServerMessage>` into a single response.
fn collapse(r: Result<ServerMessage, ServerMessage>) -> ServerMessage {
    r.unwrap_or_else(|e| e)
}

/// Collapse a handler result that may also request a disconnect.
async fn collapse_with_disconnect(
    r: Result<(ServerMessage, Option<PublicKey>), ServerMessage>,
    disconnect_tx: &mpsc::Sender<(PublicKey, String)>,
    reason: &str,
) -> ServerMessage {
    match r {
        Ok((resp, Some(pk))) => {
            let _ = disconnect_tx.send((pk, reason.into())).await;
            resp
        }
        Ok((resp, None)) => resp,
        Err(err) => err,
    }
}

/// ALPN protocol identifier for Crab City connections.
pub const ALPN: &[u8] = b"crab/1";

/// Handle to a connected client's QUIC connection.
struct ConnectionHandle {
    /// The iroh QUIC connection.
    _conn: iroh::endpoint::Connection,
    /// Token to cancel this connection's handler task.
    cancel: CancellationToken,
}

/// The iroh-based P2P transport layer.
pub struct IrohTransport {
    endpoint: Endpoint,
    relay_url: RelayUrl,
    connections: Arc<Mutex<HashMap<PublicKey, ConnectionHandle>>>,
    cancel: CancellationToken,
}

impl IrohTransport {
    /// Start the iroh endpoint and begin accepting connections.
    pub async fn start(
        identity: Arc<InstanceIdentity>,
        relay_url: RelayUrl,
        repo: ConversationRepository,
        state_manager: Arc<GlobalStateManager>,
        instance_manager: Arc<InstanceManager>,
        server_config: Option<Arc<ServerConfig>>,
    ) -> Result<Self> {
        let relay_map = iroh::RelayMap::from(relay_url.clone());

        // Configure QUIC keepalive: send pings every 30s, timeout after 40s idle
        let transport_config = iroh::endpoint::QuicTransportConfig::builder()
            .keep_alive_interval(Duration::from_secs(30))
            .max_idle_timeout(Some(iroh::endpoint::IdleTimeout::from(
                iroh::endpoint::VarInt::from_u32(40_000),
            )))
            .build();

        let endpoint = Endpoint::builder()
            .secret_key(identity.iroh_secret_key())
            .alpns(vec![ALPN.to_vec()])
            .relay_mode(RelayMode::Custom(relay_map))
            .transport_config(transport_config)
            .bind()
            .await
            .context("failed to bind iroh endpoint")?;

        let cancel = CancellationToken::new();
        let connections: Arc<Mutex<HashMap<PublicKey, ConnectionHandle>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let broadcast_tx = state_manager.lifecycle_sender();

        let rpc_ctx = Arc::new(RpcContext {
            repo,
            identity,
            broadcast_tx: broadcast_tx.clone(),
        });

        let max_history_bytes = server_config
            .as_ref()
            .map(|c| c.websocket.max_history_replay_bytes)
            .unwrap_or(DEFAULT_MAX_HISTORY_BYTES);

        // Disconnect channel: handlers send (public_key, reason) to request disconnection
        let (disconnect_tx, mut disconnect_rx) = mpsc::channel::<(PublicKey, String)>(32);

        // Spawn the accept loop
        let ep_clone = endpoint.clone();
        let cancel_clone = cancel.clone();
        let connections_clone = connections.clone();

        tokio::spawn(async move {
            Self::accept_loop(
                ep_clone,
                cancel_clone,
                connections_clone,
                rpc_ctx,
                disconnect_tx,
                state_manager,
                instance_manager,
                max_history_bytes,
            )
            .await;
        });

        // Spawn disconnect processor
        let connections_for_disconnect = connections.clone();
        let cancel_for_disconnect = cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel_for_disconnect.cancelled() => break,
                    msg = disconnect_rx.recv() => {
                        match msg {
                            Some((pk, reason)) => {
                                let mut conns = connections_for_disconnect.lock().await;
                                if let Some(handle) = conns.remove(&pk) {
                                    handle.cancel.cancel();
                                    info!(
                                        peer = %pk.fingerprint(),
                                        reason = %reason,
                                        "disconnected client via handler request"
                                    );
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        info!(
            "iroh transport started, accepting connections via relay {}",
            relay_url
        );

        Ok(Self {
            endpoint,
            relay_url,
            connections,
            cancel,
        })
    }

    /// The endpoint address that clients use to connect to this instance.
    pub fn endpoint_addr(&self) -> EndpointAddr {
        EndpointAddr::new(self.endpoint.id()).with_relay_url(self.relay_url.clone())
    }

    /// Close a specific client's connection.
    pub async fn disconnect(&self, public_key: &PublicKey, reason: &str) {
        let mut conns = self.connections.lock().await;
        if let Some(handle) = conns.remove(public_key) {
            handle.cancel.cancel();
            info!(
                peer = %public_key.fingerprint(),
                reason = reason,
                "disconnected client"
            );
        }
    }

    /// Number of connected clients.
    pub async fn connection_count(&self) -> usize {
        self.connections.lock().await.len()
    }

    /// Graceful shutdown: cancel accept loop and close all connections.
    pub async fn shutdown(self) {
        info!("shutting down iroh transport");
        self.cancel.cancel();

        // Close all active connections
        let mut conns = self.connections.lock().await;
        for (pk, handle) in conns.drain() {
            handle.cancel.cancel();
            info!(peer = %pk.fingerprint(), "closing connection");
        }

        self.endpoint.close().await;
        info!("iroh transport shut down");
    }

    /// The accept loop runs as a background task, accepting incoming connections
    /// and spawning per-connection handler tasks.
    #[allow(clippy::too_many_arguments)]
    async fn accept_loop(
        endpoint: Endpoint,
        cancel: CancellationToken,
        connections: Arc<Mutex<HashMap<PublicKey, ConnectionHandle>>>,
        rpc_ctx: Arc<RpcContext>,
        disconnect_tx: mpsc::Sender<(PublicKey, String)>,
        state_manager: Arc<GlobalStateManager>,
        instance_manager: Arc<InstanceManager>,
        max_history_bytes: usize,
    ) {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("accept loop cancelled");
                    break;
                }
                incoming = endpoint.accept() => {
                    let Some(incoming) = incoming else {
                        info!("endpoint closed, accept loop exiting");
                        break;
                    };

                    let conn = match incoming.accept() {
                        Ok(connecting) => match connecting.await {
                            Ok(conn) => conn,
                            Err(e) => {
                                error!("connection handshake failed: {}", e);
                                continue;
                            }
                        },
                        Err(e) => {
                            error!("failed to accept incoming connection: {}", e);
                            continue;
                        }
                    };

                    let remote_id = conn.remote_id();
                    let remote_bytes = remote_id.as_bytes();
                    let public_key = PublicKey::from_bytes(*remote_bytes);

                    // Reject spoofed loopback keys from non-local connections
                    if public_key.is_loopback() {
                        warn!("rejected connection with loopback key from remote");
                        conn.close(1u32.into(), b"loopback key not allowed remotely");
                        continue;
                    }

                    info!(
                        peer = %public_key.fingerprint(),
                        "accepted connection"
                    );

                    // Look up the member's grant
                    let grant = match rpc_ctx.repo.get_active_grant(public_key.as_bytes()).await {
                        Ok(g) => g,
                        Err(e) => {
                            error!(
                                peer = %public_key.fingerprint(),
                                "failed to look up grant: {}", e
                            );
                            conn.close(2u32.into(), b"internal error");
                            continue;
                        }
                    };

                    let conn_cancel = CancellationToken::new();
                    let handle = ConnectionHandle {
                        _conn: conn.clone(),
                        cancel: conn_cancel.clone(),
                    };

                    // Register the connection
                    connections.lock().await.insert(public_key, handle);

                    // Spawn per-connection handler
                    let connections_clone = connections.clone();
                    let rpc_clone = rpc_ctx.clone();
                    let disconnect_clone = disconnect_tx.clone();

                    if let Some(ref g) = grant {
                        // Build AuthUser from grant
                        let cap: Capability = g.capability.parse().unwrap_or(Capability::View);
                        let identity = rpc_ctx.repo.get_identity(public_key.as_bytes()).await.ok().flatten();
                        let display_name = identity
                            .map(|i| i.display_name)
                            .unwrap_or_else(|| public_key.fingerprint());
                        let auth_user = AuthUser::from_grant(public_key, display_name, cap);

                        let repo_arc = Some(Arc::new(rpc_ctx.repo.clone()));

                        tokio::spawn(Self::connection_handler(
                            conn,
                            public_key,
                            auth_user,
                            conn_cancel,
                            connections_clone,
                            rpc_clone,
                            disconnect_clone,
                            state_manager.clone(),
                            instance_manager.clone(),
                            repo_arc,
                            max_history_bytes,
                        ));
                    } else {
                        // No grant — client must redeem an invite as first message
                        tokio::spawn(Self::invite_handler(
                            conn,
                            public_key,
                            conn_cancel,
                            connections_clone,
                            rpc_clone,
                        ));
                    }
                }
            }
        }
    }

    /// Handle an authenticated connection: bidirectional message streaming.
    #[allow(clippy::too_many_arguments)]
    async fn connection_handler(
        conn: iroh::endpoint::Connection,
        public_key: PublicKey,
        auth_user: AuthUser,
        cancel: CancellationToken,
        connections: Arc<Mutex<HashMap<PublicKey, ConnectionHandle>>>,
        rpc_ctx: Arc<RpcContext>,
        disconnect_tx: mpsc::Sender<(PublicKey, String)>,
        state_manager: Arc<GlobalStateManager>,
        instance_manager: Arc<InstanceManager>,
        repository: Option<Arc<ConversationRepository>>,
        max_history_bytes: usize,
    ) {
        let peer = public_key.fingerprint();
        let connection_id = uuid::Uuid::new_v4().to_string();

        // Per-connection channel (same pattern as WS handler)
        let (tx, mut rx) = mpsc::channel::<ServerMessage>(100);

        // Accept the main bidirectional stream from the client
        let (mut send, mut recv) = match conn.accept_bi().await {
            Ok(streams) => streams,
            Err(e) => {
                error!(peer = %peer, "failed to accept bidi stream: {}", e);
                connections.lock().await.remove(&public_key);
                return;
            }
        };

        // Send initial instance list (same as WS handler)
        let instances = instance_manager.list().await;
        if tx
            .send(ServerMessage::InstanceList { instances })
            .await
            .is_err()
        {
            warn!(peer = %peer, "failed to send initial instance list");
            connections.lock().await.remove(&public_key);
            return;
        }

        // Build shared connection context
        let ws_user = auth_user_to_ws_user(&auth_user);
        let ctx = Arc::new(ConnectionContext::new(
            connection_id.clone(),
            Some(ws_user),
            tx.clone(),
            state_manager.clone(),
            instance_manager.clone(),
            repository,
            max_history_bytes,
            ClientType::Iroh,
        ));

        // Sender task: drain mpsc rx → write to QUIC send stream
        let sender_task = {
            let peer = peer.clone();
            async move {
                let mut seq = 0u64;
                while let Some(msg) = rx.recv().await {
                    if let Err(e) = framing::write_message(&mut send, &msg, &mut seq, None).await {
                        error!(peer = %peer, "write error: {}", e);
                        break;
                    }
                }
            }
        };

        // State broadcast task: forward state changes to this client
        let mut state_rx = state_manager.subscribe();
        let tx_state = tx.clone();
        let state_broadcast_task = async move {
            loop {
                match state_rx.recv().await {
                    Ok((instance_id, state, stale)) => {
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
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };

        // Lifecycle broadcast task: forward lifecycle events to this client
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

        // Main input loop: read client messages and dispatch
        let ctx_input = ctx.clone();
        let peer_input = peer.clone();
        let input_task = async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        info!(peer = %peer_input, "connection cancelled");
                        break;
                    }
                    _ = conn.closed() => {
                        info!(peer = %peer_input, "connection closed by peer");
                        break;
                    }
                    client_msg = framing::read_message(&mut recv) => {
                        match client_msg {
                            Ok(Some(framing::IncomingMessage { message, request_id: _ })) => {
                                match dispatch_client_message(&ctx_input, message).await {
                                    DispatchResult::Handled => {}
                                    DispatchResult::Unhandled(msg) => {
                                        Self::dispatch_rpc(
                                            &rpc_ctx,
                                            &auth_user,
                                            &ctx_input.tx,
                                            &disconnect_tx,
                                            msg,
                                        ).await;
                                    }
                                }
                            }
                            Ok(None) => {
                                info!(peer = %peer_input, "client stream closed");
                                break;
                            }
                            Err(e) => {
                                error!(peer = %peer_input, "read error: {}", e);
                                break;
                            }
                        }
                    }
                }
            }
        };

        // Run all tasks concurrently
        tokio::select! {
            _ = sender_task => debug!(peer = %peer, "sender task ended"),
            _ = state_broadcast_task => debug!(peer = %peer, "state broadcast task ended"),
            _ = lifecycle_task => debug!(peer = %peer, "lifecycle task ended"),
            _ = input_task => debug!(peer = %peer, "input task ended"),
        }

        // Shared disconnect cleanup (viewports, presence, terminal locks)
        disconnect_cleanup(&ctx).await;

        // Remove from connections map
        connections.lock().await.remove(&public_key);
        info!(peer = %peer, "connection handler exited");
    }

    /// Dispatch interconnect RPC messages (membership, invites, event log).
    ///
    /// These are iroh-only operations that don't belong in the shared dispatcher
    /// because they require `RpcContext`, `AuthUser`, and `disconnect_tx`.
    async fn dispatch_rpc(
        rpc_ctx: &RpcContext,
        auth_user: &AuthUser,
        tx: &mpsc::Sender<ServerMessage>,
        disconnect_tx: &mpsc::Sender<(PublicKey, String)>,
        msg: ClientMessage,
    ) {
        match msg {
            ClientMessage::CreateInvite {
                capability,
                max_uses,
                expires_in_secs,
            } => {
                let resp = collapse(
                    interconnect::handle_create_invite(
                        rpc_ctx,
                        auth_user,
                        &capability,
                        max_uses,
                        expires_in_secs,
                    )
                    .await,
                );
                let _ = tx.send(resp).await;
            }
            ClientMessage::RedeemInvite {
                token,
                display_name,
            } => {
                let resp = collapse(
                    interconnect::handle_redeem_invite(
                        rpc_ctx,
                        &auth_user.public_key,
                        &token,
                        &display_name,
                    )
                    .await,
                );
                let _ = tx.send(resp).await;
            }
            ClientMessage::RevokeInvite {
                nonce,
                suspend_derived,
            } => {
                let resp = collapse(
                    interconnect::handle_revoke_invite(rpc_ctx, auth_user, &nonce, suspend_derived)
                        .await,
                );
                let _ = tx.send(resp).await;
            }
            ClientMessage::ListInvites => {
                let resp = collapse(interconnect::handle_list_invites(rpc_ctx, auth_user).await);
                let _ = tx.send(resp).await;
            }
            ClientMessage::ListMembers => {
                let resp = collapse(interconnect::handle_list_members(rpc_ctx, auth_user).await);
                let _ = tx.send(resp).await;
            }
            ClientMessage::UpdateMember {
                public_key,
                capability,
                display_name,
            } => {
                let resp = collapse(
                    interconnect::handle_update_member(
                        rpc_ctx,
                        auth_user,
                        &public_key,
                        capability.as_deref(),
                        display_name.as_deref(),
                    )
                    .await,
                );
                let _ = tx.send(resp).await;
            }
            ClientMessage::SuspendMember { public_key } => {
                let resp = collapse_with_disconnect(
                    interconnect::handle_suspend_member(rpc_ctx, auth_user, &public_key).await,
                    disconnect_tx,
                    "suspended",
                )
                .await;
                let _ = tx.send(resp).await;
            }
            ClientMessage::ReinstateMember { public_key } => {
                let resp = collapse(
                    interconnect::handle_reinstate_member(rpc_ctx, auth_user, &public_key).await,
                );
                let _ = tx.send(resp).await;
            }
            ClientMessage::RemoveMember { public_key } => {
                let resp = collapse_with_disconnect(
                    interconnect::handle_remove_member(rpc_ctx, auth_user, &public_key).await,
                    disconnect_tx,
                    "removed",
                )
                .await;
                let _ = tx.send(resp).await;
            }
            ClientMessage::QueryEvents {
                target,
                event_type_prefix,
                limit,
                before_id,
            } => {
                let resp = collapse(
                    interconnect::handle_query_events(
                        rpc_ctx,
                        auth_user,
                        target.as_deref(),
                        event_type_prefix.as_deref(),
                        limit,
                        before_id,
                    )
                    .await,
                );
                let _ = tx.send(resp).await;
            }
            ClientMessage::VerifyEvents { from_id, to_id } => {
                let resp = collapse(
                    interconnect::handle_verify_events(rpc_ctx, auth_user, from_id, to_id).await,
                );
                let _ = tx.send(resp).await;
            }
            ClientMessage::GetEventProof { event_id } => {
                let resp = collapse(
                    interconnect::handle_get_event_proof(rpc_ctx, auth_user, event_id).await,
                );
                let _ = tx.send(resp).await;
            }
            // Non-RPC messages should never reach here
            _ => {
                warn!("unexpected non-RPC message in dispatch_rpc");
            }
        }
    }

    /// Handle an unauthenticated connection: expect invite redemption as first message.
    async fn invite_handler(
        conn: iroh::endpoint::Connection,
        public_key: PublicKey,
        cancel: CancellationToken,
        connections: Arc<Mutex<HashMap<PublicKey, ConnectionHandle>>>,
        rpc_ctx: Arc<RpcContext>,
    ) {
        let peer = public_key.fingerprint();

        let (mut send, mut recv) = match conn.accept_bi().await {
            Ok(streams) => streams,
            Err(e) => {
                error!(peer = %peer, "failed to accept bidi stream for invite: {}", e);
                connections.lock().await.remove(&public_key);
                return;
            }
        };

        // Wait for the invite redemption message
        let mut seq = 0u64;
        tokio::select! {
            _ = cancel.cancelled() => {}
            msg = framing::read_message(&mut recv) => {
                match msg {
                    Ok(Some(framing::IncomingMessage { message: ClientMessage::RedeemInvite { token, display_name }, request_id })) => {
                        match interconnect::handle_redeem_invite(&rpc_ctx, &public_key, &token, &display_name).await {
                            Ok(resp) => {
                                // Send success response, then close
                                if let Err(e) = framing::write_message(&mut send, &resp, &mut seq, request_id.as_deref()).await {
                                    error!(peer = %peer, "failed to send redeem response: {}", e);
                                }
                                conn.close(0u32.into(), b"invite redeemed, please reconnect");
                            }
                            Err(err) => {
                                let _ = framing::write_message(&mut send, &err, &mut seq, request_id.as_deref()).await;
                                conn.close(3u32.into(), b"invite redemption failed");
                            }
                        }
                    }
                    Ok(Some(framing::IncomingMessage { request_id, .. })) => {
                        let err = ServerMessage::Error {
                            instance_id: None,
                            message: "expected RedeemInvite as first message".into(),
                        };
                        let _ = framing::write_message(&mut send, &err, &mut seq, request_id.as_deref()).await;
                        conn.close(3u32.into(), b"expected RedeemInvite");
                    }
                    Ok(None) => {
                        info!(peer = %peer, "invite client disconnected");
                    }
                    Err(e) => {
                        error!(peer = %peer, "invite handler read error: {}", e);
                    }
                }
            }
        }

        connections.lock().await.remove(&public_key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TransportConfig;

    #[test]
    fn transport_config_default() {
        let cfg = TransportConfig::from_file(&crate::config::TransportFileConfig::default());
        assert_eq!(cfg.relay_bind_addr, ([127, 0, 0, 1], 4434).into());
    }
}
