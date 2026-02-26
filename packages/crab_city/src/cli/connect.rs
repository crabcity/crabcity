//! `crab connect` — connect to a remote crab instance via iroh transport.
//!
//! Parses a connection token (or --node/--invite/--relay flags), redeems the
//! invite, then enters a terminal I/O loop.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use iroh::Endpoint;
use sqlx::sqlite::SqlitePoolOptions;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

use crate::config::CrabCityConfig;
use crate::identity::InstanceIdentity;
use crate::repository::ConversationRepository;
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

/// Format a capability byte as a human-readable string with access details.
fn capability_display(cap: u8) -> &'static str {
    match cap {
        0 => "view",
        1 => "collaborate",
        2 => "admin",
        3 => "owner",
        _ => "unknown",
    }
}

/// Format access rights for a capability level.
fn capability_access_summary(cap: u8) -> &'static str {
    match cap {
        0 => "terminals: read",
        1 => "terminals: read, input | chat: send | tasks: read, create",
        2 => "terminals: read, input | chat: send | tasks: read, create | members: manage",
        3 => "full access",
        _ => "",
    }
}

/// Format an 8-byte inviter fingerprint as crab_XXXXXXXX.
fn format_fingerprint(fp: &[u8; 8]) -> String {
    use crab_city_auth::encoding::crockford_encode;
    let encoded = crockford_encode(fp);
    format!("crab_{}", &encoded[..8.min(encoded.len())])
}

/// Show invite metadata and ask for confirmation. Returns false if user declines.
fn confirm_join(token: &ConnectionToken, skip_confirm: bool) -> bool {
    // Show metadata if available (v2 token)
    if let Some(ref name) = token.instance_name {
        eprintln!();
        eprintln!("  {}", name);
        if let Some(ref fp) = token.inviter_fingerprint {
            eprintln!("  Invited by: {}", format_fingerprint(fp));
        }
        if let Some(cap) = token.capability {
            eprintln!("  Access: {}", capability_display(cap));
            let summary = capability_access_summary(cap);
            if !summary.is_empty() {
                eprintln!("    {}", summary);
            }
        }
        eprintln!();
    }

    if skip_confirm {
        return true;
    }

    eprint!("  Join this workspace? [Y/n] ");
    let _ = std::io::stderr().flush();
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    let trimmed = input.trim().to_ascii_lowercase();
    trimmed.is_empty() || trimmed == "y" || trimmed == "yes"
}

/// Translate server error messages into user-friendly text with recovery actions.
fn format_server_error(message: &str, instance_name: Option<&str>) -> String {
    let name = instance_name.unwrap_or("the remote host");

    if message.contains("expired") {
        return format!("This invite has expired. Ask {} for a new one.", name);
    }
    if message.contains("exhausted") || message.contains("max_uses") {
        return format!(
            "This invite has been fully used. Ask {} for a new one.",
            name
        );
    }
    if message.contains("revoked") {
        return format!("This invite was revoked. Contact {}.", name);
    }
    if message.contains("already have a grant") {
        if let Some(n) = instance_name {
            return format!(
                "You already have access to {}. Switch with: crab switch '{}'",
                n, n
            );
        }
        return "You already have access. Use `crab switch` to connect.".to_string();
    }
    if message.contains("suspended") {
        return format!(
            "Your access to {} has been suspended. Contact the admin.",
            name
        );
    }
    if message.contains("not_a_member") || message.contains("no grant") {
        return format!("No access to {}. You need an invite token to join.", name);
    }

    // Fall through: return original message
    message.to_string()
}

/// Main entry point for `crab connect`.
pub async fn connect_command(
    config: &CrabCityConfig,
    token: Option<String>,
    node_hex: Option<String>,
    invite_hex: Option<String>,
    relay: Option<String>,
    name: Option<String>,
    skip_confirm: bool,
) -> Result<()> {
    // Parse connection info from token or flags
    let (ct, node_id, invite_nonce, relay_url) = if let Some(ref token_str) = token {
        let ct = ConnectionToken::from_base32(token_str)
            .map_err(|e| anyhow::anyhow!("invalid token: {e}"))?;
        let node_id = ct.node_id;
        let invite_nonce = ct.invite_nonce;
        let relay_url = ct.relay_url.clone();
        (Some(ct), node_id, invite_nonce, relay_url)
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

        (None, node_id, invite_nonce, relay)
    };

    // Show metadata and confirm before connecting
    if let Some(ref ct) = ct {
        if !confirm_join(ct, skip_confirm) {
            eprintln!("Cancelled.");
            return Ok(());
        }
    }

    // Load or generate local identity
    let identity = InstanceIdentity::load_or_generate(&config.data_dir)?;
    let display_name = name.unwrap_or_else(|| identity.public_key.fingerprint());

    let instance_name = ct.as_ref().and_then(|t| t.instance_name.clone());

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

    let granted_capability = match &response {
        ServerMessage::InviteRedeemed {
            capability,
            fingerprint,
            ..
        } => {
            eprintln!("Joined as {} ({})", fingerprint, capability);
            Some(capability.clone())
        }
        ServerMessage::Error { message, .. } => {
            if message.contains("already have a grant") {
                eprintln!("Already a member, reconnecting...");
                None
            } else {
                let friendly = format_server_error(message, instance_name.as_deref());
                bail!("{}", friendly);
            }
        }
        other => {
            bail!("Unexpected response during invite redemption: {:?}", other);
        }
    };

    // Persist this remote so the daemon can auto-connect on next startup
    let host_name = instance_name.clone().unwrap_or_else(|| {
        iroh::EndpointId::from_bytes(&node_id)
            .map(|id| id.fmt_short().to_string())
            .unwrap_or_else(|_| "unknown".into())
    });
    let granted_access = granted_capability.as_deref().unwrap_or("view");

    if let Err(e) = persist_remote(
        config,
        &node_id,
        identity.public_key.as_bytes(),
        &host_name,
        granted_access,
    )
    .await
    {
        eprintln!(
            "Warning: failed to save remote (will need to reconnect manually): {}",
            e
        );
    } else {
        info!(host = %host_name, "remote saved — will auto-connect on next daemon start");
    }

    // Wait for server to close the connection (it sends "please reconnect")
    drop(send);
    drop(recv);
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
            let friendly = format_server_error(&message, instance_name.as_deref());
            bail!("{}", friendly);
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

    // Print instance list
    if let Some(ref name) = instance_name {
        eprintln!("Connected to {}", name);
    }
    eprintln!("{} terminal(s) available:", instances.len());
    for inst in &instances {
        let name = inst.custom_name.as_deref().unwrap_or(&inst.name);
        let status = if inst.running { "running" } else { "stopped" };
        eprintln!(
            "  {} {} ({})",
            if inst.running { "►" } else { " " },
            name,
            status,
        );
    }
    eprintln!();

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
        eprintln!("Select instance:");
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
                                let friendly = format_server_error(&message, instance_name.as_deref());
                                eprintln!("\r\n[crab: error: {}]", friendly);
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

/// Open the shared SQLite database and persist a remote Crab City entry.
/// This allows the daemon's ConnectionManager to auto-connect on next startup.
async fn persist_remote(
    config: &CrabCityConfig,
    host_node_id: &[u8; 32],
    account_key: &[u8],
    host_name: &str,
    granted_access: &str,
) -> Result<()> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&config.db_url())
        .await
        .context("failed to open database for remote persistence")?;

    let repo = ConversationRepository::new(pool.clone());
    repo.add_remote_crab_city(host_node_id, account_key, host_name, granted_access)
        .await?;

    pool.close().await;
    Ok(())
}
