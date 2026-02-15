//! iroh QUIC transport: endpoint management, connection accept loop, message dispatch.
//!
//! This is the core transport for Crab City. Each connecting client authenticates
//! via their Ed25519 public key (extracted from the QUIC handshake). Clients with
//! an active `MemberGrant` get full access; clients without a grant can redeem an
//! invite as their first message.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use crab_city_auth::PublicKey;
use iroh::{Endpoint, EndpointAddr, RelayMode, RelayUrl};
use tokio::sync::{Mutex, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::identity::InstanceIdentity;
use crate::repository::ConversationRepository;
use crate::transport::framing;
use crate::transport::replay_buffer::ReplayBuffer;
use crate::ws::ServerMessage;

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
    replay_buffer: Arc<Mutex<ReplayBuffer>>,
    seq: Arc<std::sync::atomic::AtomicU64>,
}

impl IrohTransport {
    /// Start the iroh endpoint and begin accepting connections.
    pub async fn start(
        identity: &InstanceIdentity,
        relay_url: RelayUrl,
        repo: ConversationRepository,
        broadcast_tx: broadcast::Sender<ServerMessage>,
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
        let replay_buffer = Arc::new(Mutex::new(ReplayBuffer::new()));
        let seq = Arc::new(std::sync::atomic::AtomicU64::new(1));

        // Spawn the accept loop
        let ep_clone = endpoint.clone();
        let cancel_clone = cancel.clone();
        let connections_clone = connections.clone();
        let replay_clone = replay_buffer.clone();
        let seq_clone = seq.clone();

        tokio::spawn(async move {
            Self::accept_loop(
                ep_clone,
                cancel_clone,
                connections_clone,
                repo,
                broadcast_tx,
                replay_clone,
                seq_clone,
            )
            .await;
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
            replay_buffer,
            seq,
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

    /// Store a message in the replay buffer and return its sequence number.
    pub async fn buffer_message(&self, msg: &ServerMessage) -> Result<u64> {
        let seq = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let bytes = framing::serialize_envelope(msg, seq)?;
        self.replay_buffer.lock().await.push(seq, bytes);
        Ok(seq)
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
    async fn accept_loop(
        endpoint: Endpoint,
        cancel: CancellationToken,
        connections: Arc<Mutex<HashMap<PublicKey, ConnectionHandle>>>,
        repo: ConversationRepository,
        broadcast_tx: broadcast::Sender<ServerMessage>,
        replay_buffer: Arc<Mutex<ReplayBuffer>>,
        seq: Arc<std::sync::atomic::AtomicU64>,
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
                    let grant = match repo.get_active_grant(public_key.as_bytes()).await {
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
                    let broadcast_rx = broadcast_tx.subscribe();
                    let replay_clone = replay_buffer.clone();
                    let seq_clone = seq.clone();

                    if grant.is_some() {
                        tokio::spawn(Self::connection_handler(
                            conn,
                            public_key,
                            conn_cancel,
                            connections_clone,
                            broadcast_rx,
                            replay_clone,
                            seq_clone,
                        ));
                    } else {
                        // No grant — client must redeem an invite as first message
                        tokio::spawn(Self::invite_handler(
                            conn,
                            public_key,
                            conn_cancel,
                            connections_clone,
                            repo.clone(),
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
        cancel: CancellationToken,
        connections: Arc<Mutex<HashMap<PublicKey, ConnectionHandle>>>,
        mut broadcast_rx: broadcast::Receiver<ServerMessage>,
        replay_buffer: Arc<Mutex<ReplayBuffer>>,
        _seq: Arc<std::sync::atomic::AtomicU64>,
    ) {
        let peer = public_key.fingerprint();

        // Accept the main bidirectional stream from the client
        let (mut send, mut recv) = match conn.accept_bi().await {
            Ok(streams) => streams,
            Err(e) => {
                error!(peer = %peer, "failed to accept bidi stream: {}", e);
                connections.lock().await.remove(&public_key);
                return;
            }
        };

        // Read the client's initial message to get last_seq for replay
        let last_seq = match framing::read_message(&mut recv).await {
            Ok(Some(crate::ws::ClientMessage::Focus { .. })) => {
                // TODO: extract last_seq from a reconnect message
                0u64
            }
            Ok(Some(_)) => 0,
            Ok(None) => {
                info!(peer = %peer, "client disconnected before sending initial message");
                connections.lock().await.remove(&public_key);
                return;
            }
            Err(e) => {
                error!(peer = %peer, "error reading initial message: {}", e);
                connections.lock().await.remove(&public_key);
                return;
            }
        };

        // Replay buffered messages or send snapshot
        {
            let buf = replay_buffer.lock().await;
            match buf.replay_since(last_seq) {
                Some(messages) => {
                    for msg_bytes in messages {
                        if let Err(e) = framing::write_raw(&mut send, msg_bytes).await {
                            error!(peer = %peer, "replay write error: {}", e);
                            connections.lock().await.remove(&public_key);
                            return;
                        }
                    }
                    info!(peer = %peer, "replayed messages since seq {}", last_seq);
                }
                None => {
                    // Too old — would need full snapshot
                    // TODO: send snapshot
                    info!(peer = %peer, "replay buffer too old, sending snapshot");
                }
            }
        }

        // Main message loop: forward broadcasts to client, receive client messages
        let mut seq = 0u64;
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!(peer = %peer, "connection cancelled");
                    break;
                }
                _ = conn.closed() => {
                    info!(peer = %peer, "connection closed by peer");
                    break;
                }
                msg = broadcast_rx.recv() => {
                    match msg {
                        Ok(server_msg) => {
                            if let Err(e) = framing::write_message(&mut send, &server_msg, &mut seq).await {
                                error!(peer = %peer, "write error: {}", e);
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(peer = %peer, dropped = n, "broadcast lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!(peer = %peer, "broadcast channel closed");
                            break;
                        }
                    }
                }
                client_msg = framing::read_message(&mut recv) => {
                    match client_msg {
                        Ok(Some(_msg)) => {
                            // TODO: dispatch to handler (Phase 3)
                        }
                        Ok(None) => {
                            info!(peer = %peer, "client stream closed");
                            break;
                        }
                        Err(e) => {
                            error!(peer = %peer, "read error: {}", e);
                            break;
                        }
                    }
                }
            }
        }

        // Cleanup
        connections.lock().await.remove(&public_key);
        info!(peer = %peer, "connection handler exited");
    }

    /// Handle an unauthenticated connection: expect invite redemption as first message.
    async fn invite_handler(
        conn: iroh::endpoint::Connection,
        public_key: PublicKey,
        cancel: CancellationToken,
        connections: Arc<Mutex<HashMap<PublicKey, ConnectionHandle>>>,
        _repo: ConversationRepository,
    ) {
        let peer = public_key.fingerprint();

        let (_send, mut recv) = match conn.accept_bi().await {
            Ok(streams) => streams,
            Err(e) => {
                error!(peer = %peer, "failed to accept bidi stream for invite: {}", e);
                connections.lock().await.remove(&public_key);
                return;
            }
        };

        // Wait for the invite redemption message
        tokio::select! {
            _ = cancel.cancelled() => {}
            msg = framing::read_message(&mut recv) => {
                match msg {
                    Ok(Some(_client_msg)) => {
                        // TODO: handle RedeemInvite (Phase 4)
                        info!(peer = %peer, "received message on invite handler — invite redemption not yet implemented");
                        conn.close(3u32.into(), b"invite redemption not yet implemented");
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

/// Configuration for the transport layer.
#[derive(Clone, Debug)]
pub struct TransportConfig {
    /// Address for the embedded relay server.
    pub relay_bind_addr: SocketAddr,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            relay_bind_addr: ([127, 0, 0, 1], 4434).into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_config_default() {
        let cfg = TransportConfig::default();
        assert_eq!(cfg.relay_bind_addr, ([127, 0, 0, 1], 4434).into());
    }
}
