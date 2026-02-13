use anyhow::Result;
use futures::{SinkExt, StreamExt};
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;

use crate::cli::daemon::{DaemonError, DaemonInfo};
use crate::cli::terminal::{TerminalGuard, get_terminal_size};
use crate::websocket_proxy::WsMessage;

const DETACH_BYTE: u8 = 0x1D; // Ctrl-]

/// What happened when an attach session ended.
pub enum AttachOutcome {
    /// User pressed Ctrl-] to detach; instance is still running.
    Detached,
    /// The remote process exited (WebSocket closed).
    Exited,
}

/// Attach to an instance, forwarding terminal I/O over WebSocket.
pub async fn attach(daemon: &DaemonInfo, instance_id: &str) -> Result<AttachOutcome, DaemonError> {
    // 1. Fetch and display scrollback (best-effort, unchanged)
    let output_url = format!("{}/api/instances/{}/output", daemon.base_url(), instance_id);
    if let Ok(resp) = reqwest::get(&output_url).await {
        if resp.status().is_success() {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                if let Some(lines) = body.get("lines").and_then(|v| v.as_array()) {
                    let mut stdout = std::io::stdout().lock();
                    for line in lines {
                        if let Some(s) = line.as_str() {
                            let _ = stdout.write_all(s.as_bytes());
                        }
                    }
                    let _ = stdout.flush();
                }
            }
        }
    }

    // 2. Connect WebSocket — the Unavailable boundary
    let ws_url = daemon.ws_url(instance_id);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .map_err(DaemonError::from_tungstenite)?;

    // 3. Session phase — internal anyhow, mapped to Other at boundary
    attach_session(ws_stream).await.map_err(Into::into)
}

/// Run the attach session after WebSocket is connected.
async fn attach_session(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> Result<AttachOutcome> {
    let (mut ws_write, mut ws_read) = ws_stream.split();

    // 3. Enter raw mode
    let mut guard = TerminalGuard::new();
    guard.enter_raw_mode();

    // 4. Send initial resize + paint overlay badge
    const OVERLAY_TEXT: &str = "attached -- Ctrl-] to detach";
    let overlay_timer = tokio::time::sleep(std::time::Duration::ZERO);
    tokio::pin!(overlay_timer);
    let mut overlay_armed = false;

    if let Ok((rows, cols)) = get_terminal_size() {
        let msg = WsMessage::Resize { rows, cols };
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
        // Fallback: inline status when terminal size unavailable
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(b"\r[crab: attached -- press Ctrl-] to detach]");
        let _ = stdout.flush();
    }

    // 5. Set up SIGWINCH handler
    #[cfg(unix)]
    let mut sigwinch =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?;

    // 6. Spawn blocking stdin reader thread (with poll so it can shut down cleanly)
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
            // Poll stdin with 100ms timeout so we can check the shutdown flag
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

    // 7. Main select loop
    let mut detached = false;
    loop {
        tokio::select! {
            // Stdin data
            Some(data) = stdin_rx.recv() => {
                // Check for detach byte
                if let Some(pos) = data.iter().position(|&b| b == DETACH_BYTE) {
                    // Send everything before the detach byte
                    if pos > 0 {
                        let msg = WsMessage::Input {
                            data: String::from_utf8_lossy(&data[..pos]).to_string(),
                        };
                        let json = serde_json::to_string(&msg)?;
                        let _ = ws_write.send(tungstenite::Message::Text(json.into())).await;
                    }
                    detached = true;
                    break;
                }

                let msg = WsMessage::Input {
                    data: String::from_utf8_lossy(&data).to_string(),
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
                        if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                            match ws_msg {
                                WsMessage::Output { data } => {
                                    let mut stdout = std::io::stdout().lock();
                                    let _ = stdout.write_all(data.as_bytes());
                                    let _ = stdout.write_all(guard.overlay_paint_bytes());
                                    let _ = stdout.flush();
                                }
                                _ => {
                                    // Ignore conversation updates, state changes, etc.
                                }
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
                    let msg = WsMessage::Resize { rows, cols };
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

    // 8. Clean exit — shut down stdin reader, restore terminal (guard clears overlay on drop)
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
