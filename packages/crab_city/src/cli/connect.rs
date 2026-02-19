//! `crab connect` — connect to a remote crab instance via iroh transport.
//!
//! Parses a connection token (or --node/--invite/--relay flags), redeems the
//! invite, then enters a terminal I/O loop.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use iroh::Endpoint;
use tokio::sync::mpsc;
use tracing::{debug, error};

use crate::config::CrabCityConfig;
use crate::identity::InstanceIdentity;
use crate::transport::connection_token::ConnectionToken;
use crate::transport::framing;
use crate::transport::iroh_transport::ALPN;
use crate::ws::{ClientMessage, ServerMessage};

use super::terminal::{TerminalGuard, get_terminal_size};

const DETACH_BYTE: u8 = 0x1D; // Ctrl-]

fn hex_to_bytes(hex: &str) -> Result<Vec<u8>> {
    if hex.len() % 2 != 0 {
        bail!("hex string must have even length");
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex[i..i + 2], 16)
                .with_context(|| format!("invalid hex at position {i}"))
        })
        .collect()
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Main entry point for `crab connect`.
pub async fn connect_command(
    config: &CrabCityConfig,
    token: Option<String>,
    node_hex: Option<String>,
    invite_hex: Option<String>,
    relay: Option<String>,
    name: Option<String>,
) -> Result<()> {
    // Parse connection info from token or flags
    let (node_id, invite_nonce, relay_url) = if let Some(ref token_str) = token {
        let ct = ConnectionToken::from_base32(token_str)
            .map_err(|e| anyhow::anyhow!("invalid token: {e}"))?;
        (ct.node_id, ct.invite_nonce, ct.relay_url)
    } else {
        let node_hex = node_hex.ok_or_else(|| anyhow::anyhow!("provide a token or --node"))?;
        let invite_hex =
            invite_hex.ok_or_else(|| anyhow::anyhow!("provide a token or --invite"))?;

        let node_bytes = hex_to_bytes(&node_hex)?;
        let invite_bytes = hex_to_bytes(&invite_hex)?;

        let node_id: [u8; 32] = node_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("--node must be 32 bytes (64 hex chars)"))?;
        let invite_nonce: [u8; 16] = invite_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("--invite must be 16 bytes (32 hex chars)"))?;

        (node_id, invite_nonce, relay)
    };

    // Load or generate local identity
    let identity = InstanceIdentity::load_or_generate(&config.data_dir)?;
    let display_name = name.unwrap_or_else(|| identity.public_key.fingerprint());

    eprintln!("Connecting as {}...", display_name);

    // Create client-side iroh endpoint (uses public relays by default)
    let endpoint = Endpoint::builder()
        .secret_key(identity.iroh_secret_key())
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(iroh::RelayMode::Default)
        .bind()
        .await
        .context("failed to bind iroh endpoint")?;

    // Build target address
    let remote_node_id = iroh::EndpointId::from_bytes(&node_id).context("invalid node ID")?;
    let mut target = iroh::EndpointAddr::new(remote_node_id);
    if let Some(ref url) = relay_url {
        let relay_parsed: iroh::RelayUrl = url.parse().context("invalid relay URL")?;
        target = target.with_relay_url(relay_parsed);
    }

    eprintln!("Connecting to {}...", remote_node_id.fmt_short());

    // Phase 1: Redeem invite
    let conn = endpoint
        .connect(target.clone(), ALPN)
        .await
        .context("failed to connect to remote")?;

    let (mut send, mut recv) = conn.open_bi().await.context("failed to open bidi stream")?;

    // Send RedeemInvite
    let redeem_msg = ClientMessage::RedeemInvite {
        token: bytes_to_hex(&invite_nonce),
        display_name: display_name.clone(),
        public_key: bytes_to_hex(identity.public_key.as_bytes()),
    };
    framing::write_client_message(&mut send, &redeem_msg, None).await?;

    // Read response
    let response = framing::read_server_message(&mut recv)
        .await?
        .ok_or_else(|| anyhow::anyhow!("connection closed during invite redemption"))?;

    match &response {
        ServerMessage::InviteRedeemed {
            capability,
            fingerprint,
            ..
        } => {
            eprintln!(
                "Invite redeemed! Joined as {} ({})",
                fingerprint, capability
            );
        }
        ServerMessage::Error { message, .. } => {
            // Might already have a grant (reconnecting)
            if message.contains("already have a grant") {
                eprintln!("Already a member, reconnecting...");
            } else {
                bail!("Invite redemption failed: {}", message);
            }
        }
        other => {
            bail!("Unexpected response during invite redemption: {:?}", other);
        }
    }

    // Wait for server to close the connection (it sends "please reconnect")
    drop(send);
    drop(recv);
    // The connection should be closed by the server after invite redemption
    // Brief wait for the close frame
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Phase 2: Reconnect as authenticated member
    eprintln!("Reconnecting...");
    let conn = endpoint
        .connect(target, ALPN)
        .await
        .context("failed to reconnect after invite redemption")?;

    let (mut send, mut recv) = conn
        .accept_bi()
        .await
        .context("failed to accept bidi stream on reconnect")?;

    // Read ConnectionEstablished
    let msg = framing::read_server_message(&mut recv)
        .await?
        .ok_or_else(|| anyhow::anyhow!("connection closed before ConnectionEstablished"))?;

    let _connection_id = match msg {
        ServerMessage::ConnectionEstablished { connection_id } => {
            debug!("connection established: {}", connection_id);
            connection_id
        }
        ServerMessage::Error { message, .. } => {
            bail!("Server error: {}", message);
        }
        other => {
            bail!("Expected ConnectionEstablished, got: {:?}", other);
        }
    };

    // Read InstanceList
    let msg = framing::read_server_message(&mut recv)
        .await?
        .ok_or_else(|| anyhow::anyhow!("connection closed before InstanceList"))?;

    let instances = match msg {
        ServerMessage::InstanceList { instances } => instances,
        other => {
            bail!("Expected InstanceList, got: {:?}", other);
        }
    };

    if instances.is_empty() {
        eprintln!("No running instances on remote host.");
        endpoint.close().await;
        return Ok(());
    }

    // Select instance
    let instance_id = if instances.len() == 1 {
        let inst = &instances[0];
        let name = inst.custom_name.as_deref().unwrap_or(&inst.name);
        eprintln!(
            "Attaching to {} ({})",
            name,
            &inst.id[..8.min(inst.id.len())]
        );
        inst.id.clone()
    } else {
        eprintln!("\nAvailable instances:");
        for (i, inst) in instances.iter().enumerate() {
            let name = inst.custom_name.as_deref().unwrap_or(&inst.name);
            let status = if inst.running { "running" } else { "stopped" };
            eprintln!(
                "  [{}] {} ({}) - {}",
                i + 1,
                name,
                &inst.id[..8.min(inst.id.len())],
                status,
            );
        }
        eprint!("\nSelect instance [1]: ");
        std::io::stderr().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let idx: usize = input.trim().parse().unwrap_or(1);
        let idx = idx.saturating_sub(1).min(instances.len() - 1);
        let inst = &instances[idx];
        let name = inst.custom_name.as_deref().unwrap_or(&inst.name);
        eprintln!(
            "Attaching to {} ({})",
            name,
            &inst.id[..8.min(inst.id.len())]
        );
        inst.id.clone()
    };

    // Send Focus
    let focus = ClientMessage::Focus {
        instance_id: instance_id.clone(),
        since_uuid: None,
    };
    framing::write_client_message(&mut send, &focus, None).await?;

    // Enter raw terminal mode
    let mut guard = TerminalGuard::new();
    guard.enter_raw_mode();

    // Send initial resize + show overlay badge
    const OVERLAY_TEXT: &str = "remote -- Ctrl-] to detach";
    let overlay_timer = tokio::time::sleep(std::time::Duration::ZERO);
    tokio::pin!(overlay_timer);
    let mut overlay_armed = false;

    if let Ok((rows, cols)) = get_terminal_size() {
        let msg = ClientMessage::Resize {
            instance_id: instance_id.clone(),
            rows,
            cols,
        };
        framing::write_client_message(&mut send, &msg, None).await?;
        guard.show_overlay(OVERLAY_TEXT, cols);
        overlay_timer
            .as_mut()
            .reset(tokio::time::Instant::now() + std::time::Duration::from_secs(5));
        overlay_armed = true;
    }

    // SIGWINCH handler
    #[cfg(unix)]
    let mut sigwinch =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?;

    // Stdin reader thread (same pattern as attach.rs)
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(64);
    let stdin_shutdown = Arc::new(AtomicBool::new(false));
    let stdin_shutdown_thread = stdin_shutdown.clone();
    std::thread::spawn(move || {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        let stdin = std::io::stdin();
        let stdin_fd = stdin.as_raw_fd();
        let mut buf = [0u8; 4096];
        loop {
            if stdin_shutdown_thread.load(Ordering::Relaxed) {
                break;
            }
            let mut pfd = nix::libc::pollfd {
                fd: stdin_fd,
                events: nix::libc::POLLIN,
                revents: 0,
            };
            let ret = unsafe { nix::libc::poll(&mut pfd, 1, 100) };
            if ret <= 0 {
                continue;
            }
            let mut handle = stdin.lock();
            match handle.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    drop(handle);
                    if stdin_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Main I/O loop
    let mut detached = false;
    loop {
        tokio::select! {
            // Stdin data → send as Input
            Some(data) = stdin_rx.recv() => {
                if let Some(pos) = data.iter().position(|&b| b == DETACH_BYTE) {
                    if pos > 0 {
                        let msg = ClientMessage::Input {
                            instance_id: instance_id.clone(),
                            data: String::from_utf8_lossy(&data[..pos]).to_string(),
                            task_id: None,
                        };
                        let _ = framing::write_client_message(&mut send, &msg, None).await;
                    }
                    detached = true;
                    break;
                }

                let msg = ClientMessage::Input {
                    instance_id: instance_id.clone(),
                    data: String::from_utf8_lossy(&data).to_string(),
                    task_id: None,
                };
                if framing::write_client_message(&mut send, &msg, None).await.is_err() {
                    break;
                }
            }

            // Messages from server
            server_msg = framing::read_server_message(&mut recv) => {
                match server_msg {
                    Ok(Some(msg)) => {
                        match msg {
                            ServerMessage::Output { data, .. }
                            | ServerMessage::OutputHistory { data, .. } => {
                                let mut stdout = std::io::stdout().lock();
                                let _ = stdout.write_all(data.as_bytes());
                                let _ = stdout.write_all(guard.overlay_paint_bytes());
                                let _ = stdout.flush();
                            }
                            ServerMessage::Error { message, .. } => {
                                eprintln!("\r\n[crab: error: {}]", message);
                            }
                            ServerMessage::InstanceStopped { instance_id: ref id } if *id == instance_id => {
                                eprintln!("\r\n[crab: remote instance stopped]");
                                break;
                            }
                            // Ignore all other messages
                            _ => {}
                        }
                    }
                    Ok(None) => {
                        // Stream closed
                        break;
                    }
                    Err(e) => {
                        error!("read error: {}", e);
                        break;
                    }
                }
            }

            // SIGWINCH
            _ = sigwinch.recv() => {
                if let Ok((rows, cols)) = get_terminal_size() {
                    let msg = ClientMessage::Resize {
                        instance_id: instance_id.clone(),
                        rows,
                        cols,
                    };
                    let _ = framing::write_client_message(&mut send, &msg, None).await;
                    guard.repaint_overlay(cols);
                }
            }

            // Overlay auto-clear timer
            () = &mut overlay_timer, if overlay_armed => {
                guard.clear_overlay();
                overlay_armed = false;
            }
        }
    }

    // Cleanup
    stdin_shutdown.store(true, Ordering::Relaxed);
    drop(guard);
    endpoint.close().await;

    if detached {
        eprintln!("\r\n[crab: detached from remote]");
    } else {
        eprintln!("\r\n[crab: disconnected]");
    }

    Ok(())
}
