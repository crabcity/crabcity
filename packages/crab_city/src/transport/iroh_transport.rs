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
use crate::transport::replay_buffer::ReplayBuffer;
use crate::virtual_terminal::ClientType;
use crate::ws::DEFAULT_MAX_HISTORY_BYTES;
use crate::ws::dispatch::{
    ConnectionContext, DispatchResult, auth_user_to_ws_user, disconnect_cleanup,
    dispatch_client_message,
};
use crate::ws::{ClientMessage, GlobalStateManager, ServerMessage};

/// Send a disconnect request if the handler indicated one is needed.
async fn maybe_disconnect(
    resp: ServerMessage,
    disconnect_pk: Option<PublicKey>,
    disconnect_tx: &mpsc::Sender<(PublicKey, String)>,
    reason: &str,
) -> ServerMessage {
    if let Some(pk) = disconnect_pk {
        let _ = disconnect_tx.send((pk, reason.into())).await;
    }
    resp
}

/// ALPN protocol identifier for Crab City connections.
pub const ALPN: &[u8] = b"crab/1";

/// Handle to a connected client's QUIC connection.
struct ConnectionHandle {
    /// The iroh QUIC connection.
    _conn: iroh::endpoint::Connection,
    /// Token to cancel this connection's handler task.
    cancel: CancellationToken,
    /// The identity (public key) of the connected client.
    public_key: PublicKey,
}

/// Multi-connection registry: supports multiple connections per identity.
///
/// Primary index: `connection_id → ConnectionHandle`
/// Secondary index: `PublicKey → [connection_id, ...]`
struct ConnectionRegistry {
    by_id: HashMap<String, ConnectionHandle>,
    by_key: HashMap<PublicKey, Vec<String>>,
}

impl ConnectionRegistry {
    fn new() -> Self {
        Self {
            by_id: HashMap::new(),
            by_key: HashMap::new(),
        }
    }

    /// Register a connection. Multiple connections per identity are allowed.
    fn insert(&mut self, connection_id: String, handle: ConnectionHandle) {
        let pk = handle.public_key;
        self.by_key
            .entry(pk)
            .or_default()
            .push(connection_id.clone());
        self.by_id.insert(connection_id, handle);
    }

    /// Remove a single connection by its ID.
    fn remove(&mut self, connection_id: &str) -> Option<ConnectionHandle> {
        if let Some(handle) = self.by_id.remove(connection_id) {
            if let Some(ids) = self.by_key.get_mut(&handle.public_key) {
                ids.retain(|id| id != connection_id);
                if ids.is_empty() {
                    self.by_key.remove(&handle.public_key);
                }
            }
            Some(handle)
        } else {
            None
        }
    }

    /// Cancel and remove all connections for a given identity.
    fn disconnect_all(&mut self, public_key: &PublicKey) -> usize {
        let ids = match self.by_key.remove(public_key) {
            Some(ids) => ids,
            None => return 0,
        };
        let count = ids.len();
        for id in ids {
            if let Some(handle) = self.by_id.remove(&id) {
                handle.cancel.cancel();
            }
        }
        count
    }

    /// Total number of active connections.
    fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Drain all connections (for shutdown).
    fn drain(&mut self) -> impl Iterator<Item = (String, ConnectionHandle)> + '_ {
        self.by_key.clear();
        self.by_id.drain()
    }
}

/// Shared per-transport state passed to connection handlers.
///
/// Bundles the common arguments that `accept_loop`, `connection_handler`,
/// and `invite_handler` all need, eliminating 8-11 argument functions.
struct TransportContext {
    rpc_ctx: Arc<RpcContext>,
    disconnect_tx: mpsc::Sender<(PublicKey, String)>,
    state_manager: Arc<GlobalStateManager>,
    instance_manager: Arc<InstanceManager>,
    connections: Arc<Mutex<ConnectionRegistry>>,
    /// Per-connection replay buffers for reconnection support.
    /// Keyed by `connection_id` so that multi-device users don't get
    /// cross-connection duplicates. Each buffer is independently locked so
    /// the sender hot path never contends on the global map. Buffers outlive
    /// their connection handler and are cleaned up by the periodic eviction sweep.
    replay_buffers: Arc<Mutex<HashMap<String, Arc<Mutex<ReplayBuffer>>>>>,
    max_history_bytes: usize,
}

/// The iroh-based P2P transport layer.
pub struct IrohTransport {
    endpoint: Endpoint,
    relay_url: RelayUrl,
    connections: Arc<Mutex<ConnectionRegistry>>,
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
        let connections: Arc<Mutex<ConnectionRegistry>> =
            Arc::new(Mutex::new(ConnectionRegistry::new()));

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

        let transport_ctx = Arc::new(TransportContext {
            rpc_ctx,
            disconnect_tx,
            state_manager,
            instance_manager,
            connections: connections.clone(),
            replay_buffers: Arc::new(Mutex::new(HashMap::new())),
            max_history_bytes,
        });

        // Spawn the accept loop
        let ep_clone = endpoint.clone();
        let cancel_clone = cancel.clone();
        let ctx_clone = transport_ctx.clone();

        tokio::spawn(async move {
            Self::accept_loop(ep_clone, cancel_clone, ctx_clone).await;
        });

        // Spawn replay buffer eviction sweep (every 60s)
        let replay_buffers_evict = transport_ctx.replay_buffers.clone();
        let cancel_evict = cancel.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                tokio::select! {
                    _ = cancel_evict.cancelled() => break,
                    _ = interval.tick() => {
                        let mut buffers = replay_buffers_evict.lock().await;
                        for buf in buffers.values() {
                            buf.lock().await.evict_expired();
                        }
                        // Collect keys to remove while holding the outer lock briefly
                        let empty_keys: Vec<String> = {
                            let mut keys = Vec::new();
                            for (k, buf) in buffers.iter() {
                                if buf.lock().await.len() == 0 {
                                    keys.push(k.clone());
                                }
                            }
                            keys
                        };
                        for k in empty_keys {
                            buffers.remove(&k);
                        }
                    }
                }
            }
        });

        // Spawn disconnect processor
        let connections_for_disconnect = transport_ctx.connections.clone();
        let cancel_for_disconnect = cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel_for_disconnect.cancelled() => break,
                    msg = disconnect_rx.recv() => {
                        match msg {
                            Some((pk, reason)) => {
                                let mut conns = connections_for_disconnect.lock().await;
                                let count = conns.disconnect_all(&pk);
                                if count > 0 {
                                    info!(
                                        peer = %pk.fingerprint(),
                                        reason = %reason,
                                        count = count,
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

    /// Close all connections for a specific identity.
    pub async fn disconnect(&self, public_key: &PublicKey, reason: &str) {
        let mut conns = self.connections.lock().await;
        let count = conns.disconnect_all(public_key);
        if count > 0 {
            info!(
                peer = %public_key.fingerprint(),
                reason = reason,
                count = count,
                "disconnected client"
            );
        }
    }

    /// Number of active connections.
    pub async fn connection_count(&self) -> usize {
        self.connections.lock().await.len()
    }

    /// Graceful shutdown: cancel accept loop and close all connections.
    pub async fn shutdown(self) {
        info!("shutting down iroh transport");
        self.cancel.cancel();

        // Close all active connections
        let mut conns = self.connections.lock().await;
        for (id, handle) in conns.drain() {
            handle.cancel.cancel();
            info!(peer = %handle.public_key.fingerprint(), conn_id = %id, "closing connection");
        }

        self.endpoint.close().await;
        info!("iroh transport shut down");
    }

    /// The accept loop runs as a background task, accepting incoming connections
    /// and spawning per-connection handler tasks.
    async fn accept_loop(
        endpoint: Endpoint,
        cancel: CancellationToken,
        ctx: Arc<TransportContext>,
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
                    let grant = match ctx.rpc_ctx.repo.get_active_grant(public_key.as_bytes()).await {
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

                    let conn_id = uuid::Uuid::new_v4().to_string();
                    let conn_cancel = CancellationToken::new();
                    let handle = ConnectionHandle {
                        _conn: conn.clone(),
                        cancel: conn_cancel.clone(),
                        public_key,
                    };

                    // Register the connection (multi-device: keeps all alive)
                    ctx.connections.lock().await.insert(conn_id.clone(), handle);

                    if let Some(ref g) = grant {
                        // Build AuthUser from grant
                        let cap: Capability = g.capability.parse().unwrap_or(Capability::View);
                        let identity = ctx.rpc_ctx.repo.get_identity(public_key.as_bytes()).await.ok().flatten();
                        let display_name = identity
                            .map(|i| i.display_name)
                            .unwrap_or_else(|| public_key.fingerprint());
                        let auth_user = AuthUser::from_grant(public_key, display_name, cap);

                        tokio::spawn(Self::connection_handler(
                            conn,
                            public_key,
                            auth_user,
                            conn_id,
                            conn_cancel,
                            ctx.clone(),
                        ));
                    } else {
                        // No grant — client must redeem an invite as first message
                        tokio::spawn(Self::invite_handler(
                            conn,
                            public_key,
                            conn_id,
                            conn_cancel,
                            ctx.clone(),
                        ));
                    }
                }
            }
        }
    }

    /// Handle an authenticated connection: bidirectional message streaming.
    async fn connection_handler(
        conn: iroh::endpoint::Connection,
        public_key: PublicKey,
        auth_user: AuthUser,
        connection_id: String,
        cancel: CancellationToken,
        transport: Arc<TransportContext>,
    ) {
        let peer = public_key.fingerprint();

        // Per-connection channel (same pattern as WS handler)
        let (tx, mut rx) = mpsc::channel::<ServerMessage>(100);

        // Accept the main bidirectional stream from the client
        let (mut send, mut recv) = match conn.accept_bi().await {
            Ok(streams) => streams,
            Err(e) => {
                error!(peer = %peer, "failed to accept bidi stream: {}", e);
                transport.connections.lock().await.remove(&connection_id);
                return;
            }
        };

        // Reconnect handshake: read the first message to check for Reconnect.
        // If the client sends Reconnect { last_seq, connection_id }, replay
        // missed messages from that connection's buffer directly on the send
        // stream before starting the normal message loop.
        let mut seq = 0u64;
        let mut first_message = None;
        match framing::read_message(&mut recv).await {
            Ok(Some(framing::IncomingMessage {
                message:
                    ClientMessage::Reconnect {
                        last_seq,
                        connection_id: old_conn_id,
                    },
                ..
            })) => {
                // Collect replay data into owned bytes. We take the global map
                // lock briefly to look up the per-connection buffer, then lock
                // the buffer itself to collect entries, then drop both before
                // doing async writes.
                let replay_data = {
                    let buffers = transport.replay_buffers.lock().await;
                    if let Some(buf_arc) = buffers.get(&old_conn_id) {
                        let buf = buf_arc.lock().await;
                        match buf.replay_since(last_seq) {
                            Some(entries) => {
                                let owned: Vec<Vec<u8>> = entries
                                    .into_iter()
                                    .map(|(_seq, data)| data.to_vec())
                                    .collect();
                                Some(Ok(owned))
                            }
                            None => Some(Err(())), // gap too large
                        }
                    } else {
                        None // no buffer for this connection
                    }
                };
                // Both locks are dropped here

                match replay_data {
                    Some(Ok(entries)) => {
                        info!(
                            peer = %peer,
                            old_conn_id = %old_conn_id,
                            last_seq = last_seq,
                            replay_count = entries.len(),
                            "replaying missed messages for reconnecting client"
                        );
                        for data in &entries {
                            // Write each replayed message with length-prefixed framing
                            let len = (data.len() as u32).to_be_bytes();
                            if let Err(e) = send.write_all(&len).await {
                                error!(peer = %peer, "replay write error (len): {}", e);
                                transport.connections.lock().await.remove(&connection_id);
                                return;
                            }
                            if let Err(e) = send.write_all(&data).await {
                                error!(peer = %peer, "replay write error (data): {}", e);
                                transport.connections.lock().await.remove(&connection_id);
                                return;
                            }
                            seq += 1;
                        }
                        // Clean up the old connection's buffer now that replay is done
                        transport.replay_buffers.lock().await.remove(&old_conn_id);
                    }
                    Some(Err(())) => {
                        // Gap too large — client must do a full resync
                        warn!(
                            peer = %peer,
                            old_conn_id = %old_conn_id,
                            last_seq = last_seq,
                            "replay buffer gap: client must full-resync"
                        );
                        let err = ServerMessage::Error {
                            instance_id: None,
                            message: "replay gap: full resync required".into(),
                        };
                        let _ = framing::write_message(&mut send, &err, &mut seq, None).await;
                        // Clean up the stale buffer
                        transport.replay_buffers.lock().await.remove(&old_conn_id);
                    }
                    None => {
                        // No replay buffer for this connection (expired or unknown)
                        debug!(
                            peer = %peer,
                            old_conn_id = %old_conn_id,
                            "no replay buffer found, proceeding normally"
                        );
                    }
                }
            }
            Ok(Some(msg)) => {
                // Not a Reconnect — stash it for dispatch in the main loop
                first_message = Some(msg);
            }
            Ok(None) => {
                info!(peer = %peer, "client stream closed before first message");
                transport.connections.lock().await.remove(&connection_id);
                return;
            }
            Err(e) => {
                error!(peer = %peer, "read error on first message: {}", e);
                transport.connections.lock().await.remove(&connection_id);
                return;
            }
        }

        // Tell the client its connection_id so it can reconnect later
        if tx
            .send(ServerMessage::ConnectionEstablished {
                connection_id: connection_id.clone(),
            })
            .await
            .is_err()
        {
            warn!(peer = %peer, "failed to send ConnectionEstablished");
            transport.connections.lock().await.remove(&connection_id);
            return;
        }

        // Send initial instance list (same as WS handler)
        let instances = transport.instance_manager.list().await;
        if tx
            .send(ServerMessage::InstanceList { instances })
            .await
            .is_err()
        {
            warn!(peer = %peer, "failed to send initial instance list");
            transport.connections.lock().await.remove(&connection_id);
            return;
        }

        // Build shared connection context
        let ws_user = auth_user_to_ws_user(&auth_user);
        let repository = Some(Arc::new(transport.rpc_ctx.repo.clone()));
        let conn_ctx = Arc::new(ConnectionContext::new(
            connection_id.clone(),
            Some(ws_user),
            tx.clone(),
            transport.state_manager.clone(),
            transport.instance_manager.clone(),
            repository,
            transport.max_history_bytes,
            ClientType::Iroh,
        ));

        // Create this connection's replay buffer and register it in the global
        // map. The sender task holds a direct Arc to the buffer so the hot path
        // (every outgoing message) only locks the per-connection mutex, never the
        // global map. The global map is only touched on reconnect lookup and
        // eviction sweep — both cold paths.
        let my_replay_buffer = Arc::new(Mutex::new(ReplayBuffer::new()));
        transport
            .replay_buffers
            .lock()
            .await
            .insert(connection_id.clone(), my_replay_buffer.clone());

        // Sender task: drain mpsc rx → write to QUIC send stream, pushing
        // serialized bytes into the per-connection replay buffer.
        // `seq` continues from where the replay handshake left off.
        let sender_task = {
            let peer = peer.clone();
            let replay_buf = my_replay_buffer;
            async move {
                let mut seq = seq;
                while let Some(msg) = rx.recv().await {
                    // Serialize once for both the wire and the replay buffer
                    if let Err(e) = framing::write_message(&mut send, &msg, &mut seq, None).await {
                        error!(peer = %peer, "write error: {}", e);
                        break;
                    }
                    // Push raw JSON into this connection's replay buffer (no global lock)
                    if let Ok(bytes) = serde_json::to_vec(&msg) {
                        replay_buf.lock().await.push(&bytes);
                    }
                }
            }
        };

        // State broadcast task: forward state changes to this client
        let mut state_rx = transport.state_manager.subscribe();
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
        let mut lifecycle_rx = transport.state_manager.subscribe_lifecycle();
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

        // Main input loop: read client messages and dispatch.
        // If the first message wasn't Reconnect, it's stashed in `first_message`
        // and dispatched here before entering the read loop.
        let conn_ctx_input = conn_ctx.clone();
        let peer_input = peer.clone();
        let rpc_ctx = transport.rpc_ctx.clone();
        let disconnect_tx = transport.disconnect_tx.clone();
        let input_task = async move {
            // Dispatch stashed first message (non-Reconnect)
            if let Some(framing::IncomingMessage {
                message,
                request_id: _,
            }) = first_message
            {
                match dispatch_client_message(&conn_ctx_input, message).await {
                    DispatchResult::Handled => {}
                    DispatchResult::Unhandled(msg) => {
                        Self::dispatch_rpc(
                            &rpc_ctx,
                            &auth_user,
                            &conn_ctx_input.tx,
                            &disconnect_tx,
                            msg,
                        )
                        .await;
                    }
                }
            }

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
                                match dispatch_client_message(&conn_ctx_input, message).await {
                                    DispatchResult::Handled => {}
                                    DispatchResult::Unhandled(msg) => {
                                        Self::dispatch_rpc(
                                            &rpc_ctx,
                                            &auth_user,
                                            &conn_ctx_input.tx,
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
        disconnect_cleanup(&conn_ctx).await;

        // Remove this connection from the registry
        transport.connections.lock().await.remove(&connection_id);
        info!(peer = %peer, conn_id = %connection_id, "connection handler exited");
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
        let resp = match msg {
            ClientMessage::CreateInvite {
                capability,
                max_uses,
                expires_in_secs,
            } => {
                interconnect::handle_create_invite(
                    rpc_ctx,
                    auth_user,
                    &capability,
                    max_uses,
                    expires_in_secs,
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
            } => {
                interconnect::handle_revoke_invite(rpc_ctx, auth_user, &nonce, suspend_derived)
                    .await
            }
            ClientMessage::ListInvites => {
                interconnect::handle_list_invites(rpc_ctx, auth_user).await
            }
            ClientMessage::ListMembers => {
                interconnect::handle_list_members(rpc_ctx, auth_user).await
            }
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
                let (resp, pk) =
                    interconnect::handle_suspend_member(rpc_ctx, auth_user, &public_key).await;
                maybe_disconnect(resp, pk, disconnect_tx, "suspended").await
            }
            ClientMessage::ReinstateMember { public_key } => {
                interconnect::handle_reinstate_member(rpc_ctx, auth_user, &public_key).await
            }
            ClientMessage::RemoveMember { public_key } => {
                let (resp, pk) =
                    interconnect::handle_remove_member(rpc_ctx, auth_user, &public_key).await;
                maybe_disconnect(resp, pk, disconnect_tx, "removed").await
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
            // Non-RPC messages should never reach here
            _ => {
                warn!("unexpected non-RPC message in dispatch_rpc");
                return;
            }
        };
        let _ = tx.send(resp).await;
    }

    /// Handle an unauthenticated connection: expect invite redemption as first message.
    async fn invite_handler(
        conn: iroh::endpoint::Connection,
        public_key: PublicKey,
        connection_id: String,
        cancel: CancellationToken,
        transport: Arc<TransportContext>,
    ) {
        let peer = public_key.fingerprint();

        let (mut send, mut recv) = match conn.accept_bi().await {
            Ok(streams) => streams,
            Err(e) => {
                error!(peer = %peer, "failed to accept bidi stream for invite: {}", e);
                transport.connections.lock().await.remove(&connection_id);
                return;
            }
        };
        let rpc_ctx = &transport.rpc_ctx;

        // Wait for the invite redemption message
        let mut seq = 0u64;
        tokio::select! {
            _ = cancel.cancelled() => {}
            msg = framing::read_message(&mut recv) => {
                match msg {
                    Ok(Some(framing::IncomingMessage { message: ClientMessage::RedeemInvite { token, display_name, .. }, request_id })) => {
                        let resp = interconnect::handle_redeem_invite(rpc_ctx, &public_key, &token, &display_name).await;
                        let is_error = matches!(resp, ServerMessage::Error { .. });
                        if let Err(e) = framing::write_message(&mut send, &resp, &mut seq, request_id.as_deref()).await {
                            error!(peer = %peer, "failed to send redeem response: {}", e);
                        }
                        // Finish the send stream to flush the response before closing.
                        // Without this, conn.close() may race the frame delivery.
                        if let Err(e) = send.finish() {
                            debug!(peer = %peer, "send.finish error (non-fatal): {}", e);
                        }
                        // Brief yield to allow the QUIC stack to flush
                        tokio::task::yield_now().await;
                        if is_error {
                            conn.close(3u32.into(), b"invite redemption failed");
                        } else {
                            conn.close(0u32.into(), b"invite redeemed, please reconnect");
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

        transport.connections.lock().await.remove(&connection_id);
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
