//! Spike: validate iroh embedded relay + QUIC connections.
//!
//! This throwaway binary answers:
//! 1. Can we embed an iroh relay server in-process?
//! 2. Can an iroh endpoint accept connections and extract EndpointId?
//! 3. Can messages flow bidirectionally?
//!
//! Usage: cargo run -p spike_iroh_relay

use anyhow::Result;
use iroh::{Endpoint, EndpointAddr, RelayMode, RelayUrl, SecretKey};
use tracing::{error, info};

const ALPN: &[u8] = b"crab/1";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    info!("=== Spike: iroh embedded relay + QUIC connections ===");

    // --- Step 1: Start embedded relay ---
    info!("Step 1: Starting embedded relay...");
    let (relay_url, _relay_server) = start_embedded_relay().await?;
    info!("Relay listening at: {}", relay_url);

    // --- Step 2: Create server endpoint ---
    info!("Step 2: Creating server endpoint...");
    let server_secret = SecretKey::generate(&mut rand::rng());
    let server_endpoint_id = server_secret.public();
    info!("Server EndpointId: {}", server_endpoint_id);

    let relay_map = iroh::RelayMap::from(relay_url.clone());
    let server_ep = Endpoint::builder()
        .secret_key(server_secret)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Custom(relay_map.clone()))
        .bind()
        .await?;

    info!("Server endpoint bound, EndpointId = {}", server_endpoint_id);

    // --- Step 3: Create client endpoint ---
    info!("Step 3: Creating client endpoint...");
    let client_secret = SecretKey::generate(&mut rand::rng());
    let client_endpoint_id = client_secret.public();
    info!("Client EndpointId: {}", client_endpoint_id);

    let client_ep = Endpoint::builder()
        .secret_key(client_secret)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Custom(relay_map))
        .bind()
        .await?;

    // --- Step 4: Server accept loop ---
    info!("Step 4: Starting server accept loop...");
    let server_ep_clone = server_ep.clone();
    let accept_handle = tokio::spawn(async move {
        while let Some(incoming) = server_ep_clone.accept().await {
            info!("Server: incoming connection...");
            match incoming.accept() {
                Ok(connecting) => match connecting.await {
                    Ok(conn) => {
                        // Spawn per-connection handler so conn stays alive
                        tokio::spawn(async move {
                            let remote = conn.remote_id();
                            info!("Server: accepted connection from EndpointId = {}", remote);

                            match conn.accept_bi().await {
                                Ok((mut send, mut recv)) => {
                                    // Read full message
                                    let data = match recv.read_to_end(1024).await {
                                        Ok(d) => d,
                                        Err(e) => {
                                            error!("Server: read error: {}", e);
                                            return;
                                        }
                                    };
                                    let msg = String::from_utf8_lossy(&data);
                                    info!("Server: received '{}' from {}", msg, remote);

                                    // Echo back
                                    let reply = format!("echo: {}", msg);
                                    if let Err(e) = send.write_all(reply.as_bytes()).await {
                                        error!("Server: write error: {}", e);
                                        return;
                                    }
                                    let _ = send.finish();
                                    info!("Server: sent echo reply");
                                }
                                Err(e) => error!("Server: accept_bi error: {}", e),
                            }

                            // Keep connection alive until the client closes it
                            conn.closed().await;
                            info!("Server: connection from {} closed", remote);
                        });
                    }
                    Err(e) => error!("Server: connecting error: {}", e),
                },
                Err(e) => error!("Server: accept error: {}", e),
            }
        }
    });

    // --- Step 5: Client connects ---
    info!("Step 5: Client connecting to server...");
    let server_addr = EndpointAddr::new(server_endpoint_id).with_relay_url(relay_url.clone());
    let conn = client_ep.connect(server_addr, ALPN).await?;
    info!(
        "Client: connected! Remote EndpointId = {}",
        conn.remote_id()
    );

    // Verify identity
    assert_eq!(
        conn.remote_id(),
        server_endpoint_id,
        "Remote EndpointId should match server"
    );
    info!("Client: EndpointId verification PASSED");

    // --- Step 6: Bidirectional message exchange ---
    info!("Step 6: Bidirectional message exchange...");
    let (mut send, mut recv) = conn.open_bi().await?;

    // Send ping
    let ping = "hello from crab city!";
    send.write_all(ping.as_bytes()).await?;
    let _ = send.finish();
    info!("Client: sent '{}'", ping);

    // Read echo
    let response = recv.read_to_end(1024).await?;
    let response_str = String::from_utf8_lossy(&response);
    info!("Client: received '{}'", response_str);

    assert_eq!(
        response_str,
        format!("echo: {}", ping),
        "Echo response should match"
    );
    info!("Client: echo verification PASSED");

    // --- Results ---
    info!("");
    info!("=== SPIKE RESULTS ===");
    info!("1. Embedded relay: OK ({})", relay_url);
    info!(
        "2. EndpointId extraction: OK (server={}, client={})",
        server_endpoint_id, client_endpoint_id
    );
    info!("3. Bidirectional messaging: OK");
    info!("");
    info!("All checks passed. iroh is viable for Crab City transport.");

    // Cleanup
    conn.close(0u32.into(), b"done");
    accept_handle.abort();
    client_ep.close().await;
    server_ep.close().await;

    Ok(())
}

async fn start_embedded_relay() -> Result<(RelayUrl, iroh_relay::server::Server)> {
    use iroh_relay::server::{AccessConfig, Server, ServerConfig};

    let cfg: ServerConfig<(), ()> = ServerConfig {
        relay: Some(iroh_relay::server::RelayConfig {
            http_bind_addr: ([127, 0, 0, 1], 0).into(),
            tls: None,
            limits: Default::default(),
            key_cache_capacity: None,
            access: AccessConfig::Everyone,
        }),
        quic: None,
        metrics_addr: None,
    };

    let server = Server::spawn(cfg).await?;
    let addr = server.http_addr().expect("relay should have http addr");
    let url: RelayUrl = format!("http://{}", addr).parse()?;

    Ok((url, server))
}
