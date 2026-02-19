use anyhow::Result;
use futures::{SinkExt, StreamExt};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;

use crate::cli::daemon::{DaemonError, DaemonInfo};
use crate::cli::terminal::{TerminalGuard, get_terminal_size};
use crate::ws::{ClientMessage, ServerMessage};

const DETACH_BYTE: u8 = 0x1D; // Ctrl-]

/// Tungstenite client stream (TCP, possibly TLS).
type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// What happened when an attach session ended.
pub enum AttachOutcome {
    /// User pressed Ctrl-] to detach; instance is still running.
    Detached,
    /// The remote process exited (WebSocket closed).
    Exited,
}

/// Attach to an instance, forwarding terminal I/O over the multiplexed WebSocket.
pub async fn attach(daemon: &DaemonInfo, instance_id: &str) -> Result<AttachOutcome, DaemonError> {
    // Connect to multiplexed WS endpoint
    let ws_url = daemon.mux_ws_url();
    let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .map_err(DaemonError::from_tungstenite)?;

    // Session phase — internal anyhow, mapped to Other at boundary
    attach_session(ws_stream, instance_id)
        .await
        .map_err(Into::into)
}

/// Read the next `ServerMessage` from the stream, with a timeout.
///
/// Skips pings/pongs. Returns `Err` on timeout, close, or parse failure.
async fn recv_server_msg(
    ws_read: &mut futures::stream::SplitStream<WsStream>,
    timeout_secs: u64,
    context: &str,
) -> Result<ServerMessage> {
    tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), async {
        while let Some(msg) = ws_read.next().await {
            match msg {
                Ok(tungstenite::Message::Text(text)) => {
                    return serde_json::from_str::<ServerMessage>(&text)
                        .map_err(|e| anyhow::anyhow!("{context}: bad JSON: {e}"));
                }
                Ok(tungstenite::Message::Close(_)) => {
                    anyhow::bail!("{context}: connection closed");
                }
                Err(e) => {
                    anyhow::bail!("{context}: {e}");
                }
                _ => continue, // skip pings
            }
        }
        anyhow::bail!("{context}: stream ended")
    })
    .await
    .map_err(|_| anyhow::anyhow!("timeout: {context}"))?
}

/// Run the auth handshake: wait for Challenge, send LoopbackAuth, wait for Authenticated.
///
/// Timeout matches the server's 30s handshake timeout so slow-start edge cases
/// (e.g. future remote use) don't race against a shorter client deadline.
async fn run_loopback_auth(
    ws_write: &mut futures::stream::SplitSink<WsStream, tungstenite::Message>,
    ws_read: &mut futures::stream::SplitStream<WsStream>,
) -> Result<()> {
    // Wait for Challenge
    let challenge = recv_server_msg(ws_read, 30, "waiting for auth challenge").await?;
    match challenge {
        ServerMessage::Challenge { .. } => {}
        _ => anyhow::bail!("expected Challenge, got {:?}", challenge),
    }

    // Send LoopbackAuth
    let json = serde_json::to_string(&ClientMessage::LoopbackAuth)?;
    ws_write
        .send(tungstenite::Message::Text(json.into()))
        .await?;

    // Wait for Authenticated
    let response = recv_server_msg(ws_read, 30, "waiting for auth response").await?;
    match response {
        ServerMessage::Authenticated { .. } => Ok(()),
        ServerMessage::Error { message, .. } => anyhow::bail!("auth failed: {message}"),
        _ => anyhow::bail!("unexpected auth response: {:?}", response),
    }
}

/// Run the attach session after WebSocket is connected.
async fn attach_session(ws_stream: WsStream, instance_id: &str) -> Result<AttachOutcome> {
    let (mut ws_write, mut ws_read) = ws_stream.split();

    // Auth handshake
    run_loopback_auth(&mut ws_write, &mut ws_read).await?;

    // Enter raw mode
    let mut guard = TerminalGuard::new();
    guard.enter_raw_mode();

    // Send Focus to start receiving output
    let focus = ClientMessage::Focus {
        instance_id: instance_id.to_string(),
        since_uuid: None,
    };
    let json = serde_json::to_string(&focus)?;
    ws_write
        .send(tungstenite::Message::Text(json.into()))
        .await?;

    // Send initial resize + paint overlay badge
    const OVERLAY_TEXT: &str = "attached -- Ctrl-] to detach";
    let overlay_timer = tokio::time::sleep(std::time::Duration::ZERO);
    tokio::pin!(overlay_timer);
    let mut overlay_armed = false;

    if let Ok((rows, cols)) = get_terminal_size() {
        let msg = ClientMessage::Resize {
            instance_id: instance_id.to_string(),
            rows,
            cols,
        };
        let json = serde_json::to_string(&msg)?;
        ws_write
            .send(tungstenite::Message::Text(json.into()))
            .await?;
        guard.show_overlay(OVERLAY_TEXT, cols);
        overlay_timer
            .as_mut()
            .reset(tokio::time::Instant::now() + std::time::Duration::from_secs(5));
        overlay_armed = true;
    } else {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(b"\r[crab: attached -- press Ctrl-] to detach]");
        let _ = stdout.flush();
    }

    // Set up SIGWINCH handler
    #[cfg(unix)]
    let mut sigwinch =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?;

    // Spawn blocking stdin reader thread (with poll so it can shut down cleanly)
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

    let inst_id = instance_id.to_string();

    // Main select loop
    let mut detached = false;
    loop {
        tokio::select! {
            // Stdin data
            Some(data) = stdin_rx.recv() => {
                if let Some(pos) = data.iter().position(|&b| b == DETACH_BYTE) {
                    if pos > 0 {
                        let msg = ClientMessage::Input {
                            instance_id: inst_id.clone(),
                            data: String::from_utf8_lossy(&data[..pos]).to_string(),
                            task_id: None,
                        };
                        let json = serde_json::to_string(&msg)?;
                        let _ = ws_write.send(tungstenite::Message::Text(json.into())).await;
                    }
                    detached = true;
                    break;
                }

                let msg = ClientMessage::Input {
                    instance_id: inst_id.clone(),
                    data: String::from_utf8_lossy(&data).to_string(),
                    task_id: None,
                };
                let json = serde_json::to_string(&msg)?;
                if ws_write.send(tungstenite::Message::Text(json.into())).await.is_err() {
                    break;
                }
            }

            // WebSocket messages
            Some(msg) = ws_read.next() => {
                match msg {
                    Ok(tungstenite::Message::Text(text)) => {
                        if let Ok(server_msg) = serde_json::from_str::<ServerMessage>(&text) {
                            match server_msg {
                                // === Terminal I/O — the reason we're here ===
                                ServerMessage::Output { data, .. }
                                | ServerMessage::OutputHistory { data, .. } => {
                                    let mut stdout = std::io::stdout().lock();
                                    let _ = stdout.write_all(data.as_bytes());
                                    let _ = stdout.write_all(guard.overlay_paint_bytes());
                                    let _ = stdout.flush();
                                }

                                // === Session resolution ===
                                ServerMessage::SessionAmbiguous { candidates, .. } => {
                                    // CLI attach targets a specific instance; auto-select most
                                    // recent session (first candidate) rather than prompting.
                                    if let Some(first) = candidates.first() {
                                        if candidates.len() > 1 {
                                            eprintln!(
                                                "\r[crab: {} sessions found, using most recent]",
                                                candidates.len(),
                                            );
                                        }
                                        let sel = ClientMessage::SessionSelect {
                                            session_id: first.session_id.clone(),
                                        };
                                        let json = serde_json::to_string(&sel)?;
                                        let _ = ws_write.send(tungstenite::Message::Text(json.into())).await;
                                    }
                                }

                                // === Errors ===
                                ServerMessage::Error { message, .. } => {
                                    eprintln!("\r\n[crab: error: {}]", message);
                                }

                                // === Ignored: control / lifecycle / multi-user ===
                                // Exhaustive so the compiler catches new variants.
                                ServerMessage::FocusAck { .. }
                                | ServerMessage::ConnectionEstablished { .. }
                                | ServerMessage::Challenge { .. }
                                | ServerMessage::Authenticated { .. }
                                | ServerMessage::AuthRequired { .. }
                                | ServerMessage::InstanceList { .. }
                                | ServerMessage::InstanceCreated { .. }
                                | ServerMessage::InstanceStopped { .. }
                                | ServerMessage::InstanceRenamed { .. }
                                | ServerMessage::StateChange { .. }
                                | ServerMessage::ConversationFull { .. }
                                | ServerMessage::ConversationUpdate { .. }
                                | ServerMessage::OutputLagged { .. }
                                | ServerMessage::PresenceUpdate { .. }
                                | ServerMessage::LobbyBroadcast { .. }
                                | ServerMessage::ChatMessage { .. }
                                | ServerMessage::ChatHistoryResponse { .. }
                                | ServerMessage::ChatTopicsResponse { .. }
                                | ServerMessage::TaskUpdate { .. }
                                | ServerMessage::TaskDeleted { .. }
                                | ServerMessage::TerminalLockUpdate { .. }
                                | ServerMessage::InviteCreated { .. }
                                | ServerMessage::InviteRedeemed { .. }
                                | ServerMessage::InviteRevoked { .. }
                                | ServerMessage::InviteList { .. }
                                | ServerMessage::MembersList { .. }
                                | ServerMessage::MemberJoined { .. }
                                | ServerMessage::MemberUpdated { .. }
                                | ServerMessage::MemberSuspended { .. }
                                | ServerMessage::MemberReinstated { .. }
                                | ServerMessage::MemberRemoved { .. }
                                | ServerMessage::EventsResponse { .. }
                                | ServerMessage::EventVerification { .. }
                                | ServerMessage::EventProofResponse { .. } => {}
                            }
                        }
                    }
                    Ok(tungstenite::Message::Close(_)) | Err(_) => {
                        break;
                    }
                    _ => {}
                }
            }

            // SIGWINCH
            _ = sigwinch.recv() => {
                if let Ok((rows, cols)) = get_terminal_size() {
                    let msg = ClientMessage::Resize {
                        instance_id: inst_id.clone(),
                        rows,
                        cols,
                    };
                    let json = serde_json::to_string(&msg)?;
                    let _ = ws_write.send(tungstenite::Message::Text(json.into())).await;
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

    // Clean exit — shut down stdin reader, restore terminal (guard clears overlay on drop)
    stdin_shutdown.store(true, Ordering::Relaxed);
    drop(guard);
    if detached {
        eprintln!("\r\n[crab: detached]");
        Ok(AttachOutcome::Detached)
    } else {
        eprintln!("\r\n[crab: exited]");
        Ok(AttachOutcome::Exited)
    }
}
