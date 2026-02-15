//! In-process iroh relay server for NAT traversal.
//!
//! Starts an HTTP relay on a separate port. Both native iroh clients
//! and browser clients (via iroh WASM) connect through this relay.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use iroh::RelayUrl;
use iroh_relay::server::{AccessConfig, Server, ServerConfig};
use tracing::info;

/// Handle to an in-process iroh relay server.
pub struct EmbeddedRelay {
    server: Server,
    url: RelayUrl,
}

impl EmbeddedRelay {
    /// Start the relay, binding to the given address.
    ///
    /// Returns the relay URL that clients use to connect.
    /// The relay runs plain HTTP (no TLS) — TLS termination is handled
    /// by an external reverse proxy in production.
    pub async fn start(bind_addr: SocketAddr) -> Result<Self> {
        let cfg: ServerConfig<(), ()> = ServerConfig {
            relay: Some(iroh_relay::server::RelayConfig {
                http_bind_addr: bind_addr,
                tls: None,
                limits: Default::default(),
                key_cache_capacity: None,
                access: AccessConfig::Everyone,
            }),
            quic: None,
            metrics_addr: None,
        };

        let server = Server::spawn(cfg)
            .await
            .context("failed to start embedded relay")?;

        let actual_addr = server.http_addr().context("relay has no HTTP address")?;

        let url: RelayUrl = format!("http://{}", actual_addr)
            .parse()
            .context("invalid relay URL")?;

        info!("Embedded relay listening at {}", url);

        Ok(Self { server, url })
    }

    /// The relay URL for clients to connect through.
    pub fn url(&self) -> &RelayUrl {
        &self.url
    }

    /// Graceful shutdown.
    pub async fn shutdown(self) {
        info!("Shutting down embedded relay");
        let _ = self.server.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn relay_starts_and_provides_url() {
        let relay = EmbeddedRelay::start(([127, 0, 0, 1], 0).into())
            .await
            .unwrap();
        let url = relay.url().to_string();
        assert!(url.starts_with("http://127.0.0.1:"));
        relay.shutdown().await;
    }
}
