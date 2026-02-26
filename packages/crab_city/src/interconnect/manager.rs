//! ConnectionManager: outbound federation tunnels to remote Crab Cities.
//!
//! The "home side" — your instance connecting to other Crab Cities you've been
//! invited to, on behalf of local users.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use iroh::{Endpoint, EndpointAddr, EndpointId};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::identity::InstanceIdentity;
use crate::repository::ConversationRepository;
use crate::ws::{ClientMessage, ServerMessage};

use super::protocol::{
    TunnelClientMessage, TunnelServerMessage, read_tunnel_server_message,
    write_tunnel_client_message,
};

/// Information about a connected remote Crab City.
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub host_node_id: [u8; 32],
    pub host_name: String,
    pub state: ConnectionState,
    pub authenticated_users: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum ConnectionState {
    Connected,
    Disconnected { since: Instant },
    Reconnecting { attempt: u32 },
}

/// Per-user session state within a tunnel.
struct UserSession {
    display_name: String,
    access: Vec<serde_json::Value>,
    capability: Option<String>,
}

/// A tunnel to a remote Crab City instance.
struct InstanceTunnel {
    host_name: String,
    state: TunnelState,
    authenticated_users: HashMap<String, UserSession>,
    cancel: CancellationToken,
    /// Channel to send messages to the tunnel's writer task
    tx: mpsc::Sender<TunnelClientMessage>,
}

enum TunnelState {
    Connected,
    Disconnected { since: Instant },
    Reconnecting { attempt: u32 },
}

/// Manages outbound iroh connections to remote Crab Cities.
pub struct ConnectionManager {
    tunnels: Arc<Mutex<HashMap<[u8; 32], InstanceTunnel>>>,
    repo: ConversationRepository,
    /// Forward remote events to local clients
    event_tx: broadcast::Sender<(String, ServerMessage)>,
    identity: Arc<InstanceIdentity>,
    instance_name: String,
    endpoint: Endpoint,
    cancel: CancellationToken,
}

/// Events from remote hosts, tagged with the host's name for routing.
pub type RemoteEvent = (String, ServerMessage);

impl ConnectionManager {
    /// Create a new ConnectionManager. Call `start()` to begin auto-connecting.
    pub fn new(
        endpoint: Endpoint,
        identity: Arc<InstanceIdentity>,
        instance_name: String,
        repo: ConversationRepository,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            tunnels: Arc::new(Mutex::new(HashMap::new())),
            repo,
            event_tx,
            identity,
            instance_name,
            endpoint,
            cancel: CancellationToken::new(),
        }
    }

    /// Subscribe to events from remote hosts.
    pub fn subscribe(&self) -> broadcast::Receiver<RemoteEvent> {
        self.event_tx.subscribe()
    }

    /// Start auto-connecting to remote Crab Cities marked `auto_connect = true`.
    pub async fn start(&self) -> Result<()> {
        let remotes = self.repo.list_auto_connect().await?;
        if remotes.is_empty() {
            debug!("no auto-connect remote Crab Cities");
            return Ok(());
        }

        info!(
            count = remotes.len(),
            "auto-connecting to remote Crab Cities"
        );
        for remote in remotes {
            let host_node_id: [u8; 32] = remote
                .host_node_id
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid host_node_id length"))?;

            if let Err(e) = self.connect_to_host(host_node_id, &remote.host_name).await {
                warn!(
                    host = %remote.host_name,
                    error = %e,
                    "failed to auto-connect, will retry"
                );
                // Spawn reconnection task
                self.spawn_reconnect(host_node_id, remote.host_name.clone());
            }
        }
        Ok(())
    }

    /// Connect to a remote host by its node ID and establish a tunnel.
    pub async fn connect(&self, host_node_id: [u8; 32], host_name: &str) -> Result<String> {
        self.connect_to_host(host_node_id, host_name).await
    }

    /// Disconnect from a remote host.
    pub async fn disconnect(&self, host_node_id: &[u8; 32]) -> Result<()> {
        let mut tunnels = self.tunnels.lock().await;
        if let Some(tunnel) = tunnels.remove(host_node_id) {
            tunnel.cancel.cancel();
            info!(host = %tunnel.host_name, "disconnected from remote Crab City");
        }
        Ok(())
    }

    /// List all connections (active and disconnected).
    pub async fn list_connections(&self) -> Vec<ConnectionInfo> {
        let tunnels = self.tunnels.lock().await;
        tunnels
            .iter()
            .map(|(node_id, tunnel)| ConnectionInfo {
                host_node_id: *node_id,
                host_name: tunnel.host_name.clone(),
                state: match &tunnel.state {
                    TunnelState::Connected => ConnectionState::Connected,
                    TunnelState::Disconnected { since } => {
                        ConnectionState::Disconnected { since: *since }
                    }
                    TunnelState::Reconnecting { attempt } => {
                        ConnectionState::Reconnecting { attempt: *attempt }
                    }
                },
                authenticated_users: tunnel
                    .authenticated_users
                    .values()
                    .map(|s| s.display_name.clone())
                    .collect(),
            })
            .collect()
    }

    /// Forward a client message from a local user to a remote host.
    pub async fn forward_message(
        &self,
        host_node_id: &[u8; 32],
        user_pubkey: &str,
        msg: ClientMessage,
    ) -> Result<()> {
        let tunnels = self.tunnels.lock().await;
        let tunnel = tunnels
            .get(host_node_id)
            .ok_or_else(|| anyhow::anyhow!("no tunnel to this host"))?;

        tunnel
            .tx
            .send(TunnelClientMessage::UserMessage {
                account_key: user_pubkey.to_string(),
                message: msg,
            })
            .await
            .context("tunnel send channel closed")?;

        Ok(())
    }

    /// Authenticate a local user on a remote host's tunnel.
    pub async fn authenticate_user(
        &self,
        host_node_id: &[u8; 32],
        user_pubkey: &str,
        display_name: &str,
        identity_proof: &str,
    ) -> Result<()> {
        let tunnels = self.tunnels.lock().await;
        let tunnel = tunnels
            .get(host_node_id)
            .ok_or_else(|| anyhow::anyhow!("no tunnel to this host"))?;

        tunnel
            .tx
            .send(TunnelClientMessage::Authenticate {
                account_key: user_pubkey.to_string(),
                display_name: display_name.to_string(),
                identity_proof: identity_proof.to_string(),
            })
            .await
            .context("tunnel send channel closed")?;

        Ok(())
    }

    /// Request the remote host's instance list. The response comes back via
    /// the event broadcast as a `ServerMessage::InstanceList`.
    pub async fn request_instances(&self, host_node_id: &[u8; 32]) -> Result<()> {
        let tunnels = self.tunnels.lock().await;
        let tunnel = tunnels
            .get(host_node_id)
            .ok_or_else(|| anyhow::anyhow!("no tunnel to this host"))?;

        tunnel
            .tx
            .send(TunnelClientMessage::RequestInstances)
            .await
            .context("tunnel send channel closed")?;

        Ok(())
    }

    /// Graceful shutdown: cancel all tunnels.
    pub async fn shutdown(&self) {
        self.cancel.cancel();
        let mut tunnels = self.tunnels.lock().await;
        for (_, tunnel) in tunnels.drain() {
            tunnel.cancel.cancel();
        }
        info!("connection manager shut down");
    }

    /// Establish a tunnel to a remote host.
    async fn connect_to_host(&self, host_node_id: [u8; 32], host_name: &str) -> Result<String> {
        let node_id = EndpointId::from_bytes(&host_node_id).context("invalid host node ID")?;
        let target = EndpointAddr::new(node_id);

        info!(
            host = %host_name,
            node_id = %node_id.fmt_short(),
            "connecting to remote Crab City"
        );

        let conn = self
            .endpoint
            .connect(target, crate::transport::iroh_transport::ALPN)
            .await
            .context("iroh connect failed")?;

        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .context("failed to open bidirectional stream")?;

        // Send InstanceHello
        write_tunnel_client_message(
            &mut send,
            &TunnelClientMessage::Hello {
                instance_name: self.instance_name.clone(),
            },
        )
        .await
        .context("failed to send InstanceHello")?;

        // Read InstanceWelcome
        let welcome = read_tunnel_server_message(&mut recv)
            .await
            .context("failed to read InstanceWelcome")?
            .ok_or_else(|| anyhow::anyhow!("stream closed before InstanceWelcome"))?;

        let remote_name = match welcome {
            TunnelServerMessage::Welcome { instance_name } => {
                info!(host = %instance_name, "tunnel established");
                instance_name
            }
            TunnelServerMessage::Goodbye { reason } => {
                anyhow::bail!("host rejected connection: {}", reason);
            }
            other => {
                anyhow::bail!("unexpected message, expected Welcome: {:?}", other);
            }
        };

        // Set up tunnel state
        let tunnel_cancel = CancellationToken::new();
        let (tx, mut rx) = mpsc::channel::<TunnelClientMessage>(100);

        let tunnel = InstanceTunnel {
            host_name: remote_name.clone(),
            state: TunnelState::Connected,
            authenticated_users: HashMap::new(),
            cancel: tunnel_cancel.clone(),
            tx,
        };

        self.tunnels.lock().await.insert(host_node_id, tunnel);

        // Spawn writer task: drain channel → write to QUIC stream
        let writer_cancel = tunnel_cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = writer_cancel.cancelled() => break,
                    msg = rx.recv() => {
                        match msg {
                            Some(msg) => {
                                if let Err(e) = write_tunnel_client_message(&mut send, &msg).await {
                                    error!("tunnel write error: {}", e);
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        // Spawn reader task: read from QUIC stream → broadcast events locally
        let reader_cancel = tunnel_cancel.clone();
        let event_tx = self.event_tx.clone();
        let tunnels_ref = self.tunnels.clone();
        let reader_host_name = remote_name.clone();
        let reader_host_id = host_node_id;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = reader_cancel.cancelled() => break,
                    msg = read_tunnel_server_message(&mut recv) => {
                        match msg {
                            Ok(Some(tunnel_msg)) => {
                                match tunnel_msg {
                                    TunnelServerMessage::UserMessage { account_key: _, message } => {
                                        // Forward to local clients
                                        let _ = event_tx.send((reader_host_name.clone(), message));
                                    }
                                    TunnelServerMessage::AuthResult { account_key, access, capability, error } => {
                                        if let Some(ref err) = error {
                                            warn!(
                                                host = %reader_host_name,
                                                user = %account_key,
                                                "auth failed: {}", err
                                            );
                                        } else {
                                            info!(
                                                host = %reader_host_name,
                                                user = %account_key,
                                                capability = ?capability,
                                                "user authenticated on remote host"
                                            );
                                            let mut tunnels = tunnels_ref.lock().await;
                                            if let Some(tunnel) = tunnels.get_mut(&reader_host_id) {
                                                tunnel.authenticated_users.insert(
                                                    account_key.clone(),
                                                    UserSession {
                                                        display_name: account_key.clone(),
                                                        access,
                                                        capability,
                                                    },
                                                );
                                            }
                                        }
                                    }
                                    TunnelServerMessage::Welcome { .. } => {
                                        // Duplicate welcome — ignore
                                    }
                                    TunnelServerMessage::Goodbye { reason } => {
                                        info!(host = %reader_host_name, "host said goodbye: {}", reason);
                                        break;
                                    }
                                }
                            }
                            Ok(None) => {
                                info!(host = %reader_host_name, "tunnel stream closed");
                                break;
                            }
                            Err(e) => {
                                error!(host = %reader_host_name, "tunnel read error: {}", e);
                                break;
                            }
                        }
                    }
                }
            }

            // Mark tunnel as disconnected
            let mut tunnels = tunnels_ref.lock().await;
            if let Some(tunnel) = tunnels.get_mut(&reader_host_id) {
                tunnel.state = TunnelState::Disconnected {
                    since: Instant::now(),
                };
                info!(host = %reader_host_name, "tunnel disconnected");
            }
        });

        // Spawn connection monitoring with reconnection on disconnect
        let monitor_cancel = self.cancel.clone();
        let tunnels_ref = self.tunnels.clone();
        let endpoint = self.endpoint.clone();
        let instance_name = self.instance_name.clone();
        let event_tx = self.event_tx.clone();
        let monitor_host_name = remote_name.clone();
        tokio::spawn(async move {
            // Wait for the connection to close
            conn.closed().await;

            if monitor_cancel.is_cancelled() {
                return;
            }

            info!(host = %monitor_host_name, "connection lost, will attempt reconnection");

            // Mark as reconnecting
            {
                let mut tunnels = tunnels_ref.lock().await;
                if let Some(tunnel) = tunnels.get_mut(&host_node_id) {
                    tunnel.state = TunnelState::Reconnecting { attempt: 0 };
                }
            }

            // Exponential backoff reconnection
            let mut attempt = 0u32;
            let max_delay = Duration::from_secs(60);
            loop {
                if monitor_cancel.is_cancelled() {
                    break;
                }

                let delay = Duration::from_secs(1 << attempt.min(6));
                let delay = delay.min(max_delay);
                tokio::time::sleep(delay).await;

                if monitor_cancel.is_cancelled() {
                    break;
                }

                attempt += 1;
                info!(
                    host = %monitor_host_name,
                    attempt = attempt,
                    "reconnection attempt"
                );

                // Try to reconnect
                let target = EndpointAddr::new(node_id);
                let Ok(new_conn) = endpoint
                    .connect(target, crate::transport::iroh_transport::ALPN)
                    .await
                else {
                    warn!(host = %monitor_host_name, attempt = attempt, "reconnect failed");
                    let mut tunnels = tunnels_ref.lock().await;
                    if let Some(tunnel) = tunnels.get_mut(&host_node_id) {
                        tunnel.state = TunnelState::Reconnecting { attempt };
                    }
                    continue;
                };

                let Ok((mut send, mut recv)) = new_conn.open_bi().await else {
                    warn!(host = %monitor_host_name, "failed to open bidi stream on reconnect");
                    continue;
                };

                // Re-handshake
                if write_tunnel_client_message(
                    &mut send,
                    &TunnelClientMessage::Hello {
                        instance_name: instance_name.clone(),
                    },
                )
                .await
                .is_err()
                {
                    continue;
                }

                match read_tunnel_server_message(&mut recv).await {
                    Ok(Some(TunnelServerMessage::Welcome { .. })) => {
                        info!(host = %monitor_host_name, "reconnected");

                        // Restore tunnel state
                        let (new_tx, mut new_rx) = mpsc::channel::<TunnelClientMessage>(100);
                        let new_cancel = CancellationToken::new();

                        {
                            let mut tunnels = tunnels_ref.lock().await;
                            if let Some(tunnel) = tunnels.get_mut(&host_node_id) {
                                tunnel.state = TunnelState::Connected;
                                tunnel.cancel = new_cancel.clone();
                                tunnel.tx = new_tx;
                                // Clear authenticated users — they'll need to re-auth
                                tunnel.authenticated_users.clear();
                            }
                        }

                        // Spawn new writer
                        let wc = new_cancel.clone();
                        tokio::spawn(async move {
                            loop {
                                tokio::select! {
                                    _ = wc.cancelled() => break,
                                    msg = new_rx.recv() => {
                                        match msg {
                                            Some(msg) => {
                                                if let Err(e) = write_tunnel_client_message(&mut send, &msg).await {
                                                    error!("tunnel write error on reconnect: {}", e);
                                                    break;
                                                }
                                            }
                                            None => break,
                                        }
                                    }
                                }
                            }
                        });

                        // Spawn new reader
                        let rc = new_cancel;
                        let tunnels_r = tunnels_ref.clone();
                        let etx = event_tx.clone();
                        let rhn = monitor_host_name.clone();
                        tokio::spawn(async move {
                            loop {
                                tokio::select! {
                                    _ = rc.cancelled() => break,
                                    msg = read_tunnel_server_message(&mut recv) => {
                                        match msg {
                                            Ok(Some(TunnelServerMessage::UserMessage { message, .. })) => {
                                                let _ = etx.send((rhn.clone(), message));
                                            }
                                            Ok(Some(TunnelServerMessage::Goodbye { reason })) => {
                                                info!(host = %rhn, "host goodbye after reconnect: {}", reason);
                                                break;
                                            }
                                            Ok(Some(_)) => {}
                                            Ok(None) | Err(_) => break,
                                        }
                                    }
                                }
                            }
                            let mut tunnels = tunnels_r.lock().await;
                            if let Some(tunnel) = tunnels.get_mut(&host_node_id) {
                                tunnel.state = TunnelState::Disconnected {
                                    since: Instant::now(),
                                };
                            }
                        });

                        break; // Reconnection successful
                    }
                    _ => {
                        warn!(host = %monitor_host_name, "reconnect handshake failed");
                        continue;
                    }
                }
            }
        });

        Ok(remote_name)
    }

    /// Spawn a background reconnection task for a host that failed initial connect.
    fn spawn_reconnect(&self, host_node_id: [u8; 32], host_name: String) {
        let tunnels = self.tunnels.clone();
        let endpoint = self.endpoint.clone();
        let instance_name = self.instance_name.clone();
        let event_tx = self.event_tx.clone();
        let cancel = self.cancel.clone();

        // Insert a disconnected tunnel placeholder
        let tunnel_cancel = CancellationToken::new();
        let (tx, _rx) = mpsc::channel::<TunnelClientMessage>(1);
        let placeholder = InstanceTunnel {
            host_name: host_name.clone(),
            state: TunnelState::Reconnecting { attempt: 0 },
            authenticated_users: HashMap::new(),
            cancel: tunnel_cancel,
            tx,
        };

        let tunnels_clone = tunnels.clone();
        tokio::spawn(async move {
            tunnels_clone.lock().await.insert(host_node_id, placeholder);

            let mut attempt = 0u32;
            let max_delay = Duration::from_secs(60);

            loop {
                if cancel.is_cancelled() {
                    break;
                }

                let delay = Duration::from_secs(1 << attempt.min(6));
                tokio::time::sleep(delay.min(max_delay)).await;

                if cancel.is_cancelled() {
                    break;
                }

                attempt += 1;
                let node_id = match EndpointId::from_bytes(&host_node_id) {
                    Ok(id) => id,
                    Err(_) => break,
                };
                let target = EndpointAddr::new(node_id);

                let Ok(conn) = endpoint
                    .connect(target, crate::transport::iroh_transport::ALPN)
                    .await
                else {
                    let mut t = tunnels.lock().await;
                    if let Some(tunnel) = t.get_mut(&host_node_id) {
                        tunnel.state = TunnelState::Reconnecting { attempt };
                    }
                    continue;
                };

                let Ok((mut send, mut recv)) = conn.open_bi().await else {
                    continue;
                };

                if write_tunnel_client_message(
                    &mut send,
                    &TunnelClientMessage::Hello {
                        instance_name: instance_name.clone(),
                    },
                )
                .await
                .is_err()
                {
                    continue;
                }

                match read_tunnel_server_message(&mut recv).await {
                    Ok(Some(TunnelServerMessage::Welcome {
                        instance_name: remote_name,
                    })) => {
                        info!(host = %remote_name, "connected after retry");

                        let (new_tx, mut new_rx) = mpsc::channel::<TunnelClientMessage>(100);
                        let new_cancel = CancellationToken::new();

                        {
                            let mut t = tunnels.lock().await;
                            if let Some(tunnel) = t.get_mut(&host_node_id) {
                                tunnel.state = TunnelState::Connected;
                                tunnel.host_name = remote_name.clone();
                                tunnel.cancel = new_cancel.clone();
                                tunnel.tx = new_tx;
                            }
                        }

                        // Spawn writer
                        let wc = new_cancel.clone();
                        tokio::spawn(async move {
                            loop {
                                tokio::select! {
                                    _ = wc.cancelled() => break,
                                    msg = new_rx.recv() => {
                                        match msg {
                                            Some(msg) => {
                                                if write_tunnel_client_message(&mut send, &msg).await.is_err() {
                                                    break;
                                                }
                                            }
                                            None => break,
                                        }
                                    }
                                }
                            }
                        });

                        // Spawn reader
                        let rc = new_cancel;
                        let tr = tunnels.clone();
                        let etx = event_tx.clone();
                        let rhn = remote_name;
                        tokio::spawn(async move {
                            loop {
                                tokio::select! {
                                    _ = rc.cancelled() => break,
                                    msg = read_tunnel_server_message(&mut recv) => {
                                        match msg {
                                            Ok(Some(TunnelServerMessage::UserMessage { message, .. })) => {
                                                let _ = etx.send((rhn.clone(), message));
                                            }
                                            Ok(Some(TunnelServerMessage::Goodbye { .. })) | Ok(None) | Err(_) => break,
                                            Ok(Some(_)) => {}
                                        }
                                    }
                                }
                            }
                            let mut t = tr.lock().await;
                            if let Some(tunnel) = t.get_mut(&host_node_id) {
                                tunnel.state = TunnelState::Disconnected {
                                    since: Instant::now(),
                                };
                            }
                        });

                        break; // Success
                    }
                    _ => continue,
                }
            }
        });
    }
}
