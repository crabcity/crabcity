use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, mpsc, oneshot};
use tracing::debug;
use uuid::Uuid;

use pty_manager::{PtyConfig, PtyHandle, PtyOutput};

use crate::inference::ClaudeState;
use crate::virtual_terminal::{ClientType, VirtualTerminal};

/// Commands that can be sent to an instance actor
#[derive(Debug)]
#[allow(dead_code)]
pub enum InstanceCommand {
    GetInfo {
        respond_to: oneshot::Sender<InstanceInfo>,
    },
    WriteInput {
        text: String,
        respond_to: oneshot::Sender<Result<usize>>,
    },
    Resize {
        rows: u16,
        cols: u16,
        respond_to: oneshot::Sender<Result<()>>,
    },
    SubscribeOutput {
        respond_to: oneshot::Sender<broadcast::Receiver<PtyOutput>>,
    },
    /// Get recent output with a byte limit
    /// Returns chunks from the end of the buffer up to max_bytes total
    GetRecentOutput {
        max_bytes: usize,
        respond_to: oneshot::Sender<Vec<String>>,
    },
    SetSessionId {
        session_id: String,
        respond_to: oneshot::Sender<()>,
    },
    GetSessionId {
        respond_to: oneshot::Sender<Option<String>>,
    },
    GetPid {
        respond_to: oneshot::Sender<Option<u32>>,
    },
    SetClaudeState {
        state: ClaudeState,
        respond_to: oneshot::Sender<()>,
    },
    SetCustomName {
        name: Option<String>,
        respond_to: oneshot::Sender<()>,
    },
    /// Update a client's viewport in the VirtualTerminal.
    /// Returns Some((rows, cols)) if effective dims changed.
    UpdateViewport {
        connection_id: String,
        rows: u16,
        cols: u16,
        client_type: ClientType,
        respond_to: oneshot::Sender<Option<(u16, u16)>>,
    },
    /// Set a client's terminal visibility.
    /// Returns Some((rows, cols)) if effective dims changed.
    SetClientActive {
        connection_id: String,
        active: bool,
        respond_to: oneshot::Sender<Option<(u16, u16)>>,
    },
    /// Remove a client from the VirtualTerminal.
    /// Returns Some((rows, cols)) if effective dims changed.
    RemoveClient {
        connection_id: String,
        respond_to: oneshot::Sender<Option<(u16, u16)>>,
    },
    Stop {
        respond_to: oneshot::Sender<Result<()>>,
    },
}

/// Information about a running instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceInfo {
    pub id: String,
    pub name: String,
    /// User-set display name (e.g. "Auth refactor"). Falls back to `name` if None.
    pub custom_name: Option<String>,
    pub command: String,
    pub working_dir: String,
    pub running: bool,
    pub created_at: String,
    /// The Claude conversation session ID (detected after instance starts)
    pub session_id: Option<String>,
    /// Current Claude state (for status indicator in sidebar)
    pub claude_state: Option<ClaudeState>,
}

/// Handle to communicate with an instance actor
#[derive(Clone)]
pub struct InstanceHandle {
    sender: mpsc::Sender<InstanceCommand>,
    info: Arc<RwLock<InstanceInfo>>,
}

#[allow(dead_code)]
impl InstanceHandle {
    pub async fn get_info(&self) -> InstanceInfo {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .sender
            .send(InstanceCommand::GetInfo { respond_to: tx })
            .await;
        match rx.await {
            Ok(info) => info,
            Err(_) => self.info.read().await.clone(),
        }
    }

    pub async fn write_input(&self, text: &str) -> Result<usize> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(InstanceCommand::WriteInput {
                text: text.to_string(),
                respond_to: tx,
            })
            .await
            .map_err(|_| anyhow::anyhow!("Instance actor is gone"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Instance actor didn't respond"))?
    }

    pub async fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(InstanceCommand::Resize {
                rows,
                cols,
                respond_to: tx,
            })
            .await
            .map_err(|_| anyhow::anyhow!("Instance actor is gone"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Instance actor didn't respond"))?
    }

    pub async fn subscribe_output(&self) -> Result<broadcast::Receiver<PtyOutput>> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(InstanceCommand::SubscribeOutput { respond_to: tx })
            .await
            .map_err(|_| anyhow::anyhow!("Instance actor is gone"))?;
        Ok(rx
            .await
            .map_err(|_| anyhow::anyhow!("Instance actor didn't respond"))?)
    }

    /// Get recent output up to max_bytes total
    /// Returns chunks from the end of the buffer
    pub async fn get_recent_output(&self, max_bytes: usize) -> Vec<String> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .sender
            .send(InstanceCommand::GetRecentOutput {
                max_bytes,
                respond_to: tx,
            })
            .await;
        rx.await.unwrap_or_default()
    }

    pub async fn stop(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(InstanceCommand::Stop { respond_to: tx })
            .await
            .map_err(|_| anyhow::anyhow!("Instance actor is gone"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Instance actor didn't respond"))?
    }

    pub async fn set_session_id(&self, session_id: String) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(InstanceCommand::SetSessionId {
                session_id,
                respond_to: tx,
            })
            .await
            .map_err(|_| anyhow::anyhow!("Instance actor is gone"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Instance actor didn't respond"))?;
        Ok(())
    }

    pub async fn get_session_id(&self) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(InstanceCommand::GetSessionId { respond_to: tx })
            .await
            .ok()?;
        rx.await.ok().flatten()
    }

    pub async fn get_pid(&self) -> Option<u32> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(InstanceCommand::GetPid { respond_to: tx })
            .await
            .ok()?;
        rx.await.ok().flatten()
    }

    pub async fn set_custom_name(&self, name: Option<String>) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(InstanceCommand::SetCustomName {
                name,
                respond_to: tx,
            })
            .await
            .map_err(|_| anyhow::anyhow!("Instance actor is gone"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Instance actor didn't respond"))?;
        Ok(())
    }

    pub async fn set_claude_state(&self, state: ClaudeState) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(InstanceCommand::SetClaudeState {
                state,
                respond_to: tx,
            })
            .await
            .map_err(|_| anyhow::anyhow!("Instance actor is gone"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Instance actor didn't respond"))?;
        Ok(())
    }

    /// Update a client's viewport in the VirtualTerminal.
    /// Returns Some((rows, cols)) if the effective dimensions changed.
    pub async fn update_viewport(
        &self,
        connection_id: &str,
        rows: u16,
        cols: u16,
        client_type: ClientType,
    ) -> Option<(u16, u16)> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(InstanceCommand::UpdateViewport {
                connection_id: connection_id.to_string(),
                rows,
                cols,
                client_type,
                respond_to: tx,
            })
            .await
            .ok()?;
        rx.await.ok().flatten()
    }

    /// Set a client's terminal visibility.
    /// Returns Some((rows, cols)) if the effective dimensions changed.
    pub async fn set_client_active(&self, connection_id: &str, active: bool) -> Option<(u16, u16)> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(InstanceCommand::SetClientActive {
                connection_id: connection_id.to_string(),
                active,
                respond_to: tx,
            })
            .await
            .ok()?;
        rx.await.ok().flatten()
    }

    /// Remove a client from the VirtualTerminal.
    /// Returns Some((rows, cols)) if the effective dimensions changed.
    pub async fn remove_client(&self, connection_id: &str) -> Option<(u16, u16)> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(InstanceCommand::RemoveClient {
                connection_id: connection_id.to_string(),
                respond_to: tx,
            })
            .await
            .ok()?;
        rx.await.ok().flatten()
    }

    /// Update viewport and resize PTY if effective dimensions changed.
    pub async fn update_viewport_and_resize(
        &self,
        connection_id: &str,
        rows: u16,
        cols: u16,
        client_type: ClientType,
    ) -> Result<()> {
        if let Some((eff_rows, eff_cols)) = self
            .update_viewport(connection_id, rows, cols, client_type)
            .await
        {
            self.resize(eff_rows, eff_cols).await?;
        }
        Ok(())
    }

    /// Set client active/inactive and resize PTY if effective dimensions changed.
    pub async fn set_active_and_resize(&self, connection_id: &str, active: bool) -> Result<()> {
        if let Some((eff_rows, eff_cols)) = self.set_client_active(connection_id, active).await {
            self.resize(eff_rows, eff_cols).await?;
        }
        Ok(())
    }

    /// Remove client and resize PTY if effective dimensions changed.
    pub async fn remove_client_and_resize(&self, connection_id: &str) -> Result<()> {
        if let Some((eff_rows, eff_cols)) = self.remove_client(connection_id).await {
            self.resize(eff_rows, eff_cols).await?;
        }
        Ok(())
    }

    pub fn id(&self) -> String {
        self.info.blocking_read().id.clone()
    }

    pub async fn id_async(&self) -> String {
        self.info.read().await.id.clone()
    }
}

/// Options for spawning a new instance actor
pub struct SpawnOptions {
    pub name: String,
    pub display_command: String,
    pub actual_command: String,
    pub args: Vec<String>,
    pub working_dir: String,
    /// Maximum output ring buffer size in bytes
    pub max_buffer_bytes: usize,
}

/// Handle VT-related commands.
/// Returns `None` if handled, `Some(cmd)` for PTY/metadata commands.
async fn handle_vt_command(
    vt: &RwLock<VirtualTerminal>,
    cmd: InstanceCommand,
) -> Option<InstanceCommand> {
    match cmd {
        InstanceCommand::UpdateViewport {
            connection_id,
            rows,
            cols,
            client_type,
            respond_to,
        } => {
            let mut vt = vt.write().await;
            let result = vt.update_viewport(&connection_id, rows, cols, client_type);
            let _ = respond_to.send(result);
            None
        }
        InstanceCommand::SetClientActive {
            connection_id,
            active,
            respond_to,
        } => {
            let mut vt = vt.write().await;
            let result = vt.set_active(&connection_id, active);
            let _ = respond_to.send(result);
            None
        }
        InstanceCommand::RemoveClient {
            connection_id,
            respond_to,
        } => {
            let mut vt = vt.write().await;
            let result = vt.remove_client(&connection_id);
            let _ = respond_to.send(result);
            None
        }
        InstanceCommand::GetRecentOutput {
            max_bytes,
            respond_to,
        } => {
            let mut vt = vt.write().await;
            let replay = vt.replay();
            let data = if replay.len() > max_bytes {
                replay[replay.len() - max_bytes..].to_vec()
            } else {
                replay
            };
            let _ = respond_to.send(vec![String::from_utf8_lossy(&data).to_string()]);
            None
        }
        other => Some(other),
    }
}

/// The instance actor that manages a single PTY session
struct InstanceActor {
    info: Arc<RwLock<InstanceInfo>>,
    pty: PtyHandle,
    receiver: mpsc::Receiver<InstanceCommand>,
    virtual_terminal: Arc<RwLock<VirtualTerminal>>,
}

impl InstanceActor {
    /// Spawn a new instance actor and return its handle
    pub async fn spawn(opts: SpawnOptions) -> Result<InstanceHandle> {
        let id = Uuid::new_v4().to_string();

        debug!(
            "Starting instance actor '{}' with command '{}' (actual: '{}', args: {:?}, working_dir: '{}')",
            opts.name, opts.display_command, opts.actual_command, opts.args, opts.working_dir
        );

        // Create PTY configuration
        let config = PtyConfig {
            command: opts.actual_command.clone(),
            args: opts.args.clone(),
            working_dir: Some(opts.working_dir.clone()),
            env: Vec::new(),
            rows: 24,
            cols: 80,
        };

        // Start PTY session using pty_manager
        let pty = pty_manager::pty::PtyActor::spawn(config).map_err(|e| {
            tracing::error!(
                "Failed to start PTY for '{}': command='{}' args={:?} working_dir='{}' - {}",
                opts.name,
                opts.actual_command,
                opts.args,
                opts.working_dir,
                e
            );
            anyhow::anyhow!("{}", e)
        })?;

        let info = Arc::new(RwLock::new(InstanceInfo {
            id: id.clone(),
            name: opts.name.clone(),
            custom_name: None,
            command: opts.display_command.clone(),
            working_dir: opts.working_dir,
            running: true,
            created_at: chrono::Utc::now().to_rfc3339(),
            session_id: None,
            claude_state: None,
        }));

        let (sender, receiver) = mpsc::channel(32);
        let virtual_terminal = Arc::new(RwLock::new(VirtualTerminal::new(
            24,
            80,
            opts.max_buffer_bytes,
        )));

        let actor = InstanceActor {
            info: info.clone(),
            pty: pty.clone(),
            receiver,
            virtual_terminal: virtual_terminal.clone(),
        };

        // Start the output collection task - feed PTY output to VirtualTerminal
        let mut output_rx = pty.subscribe();
        let vt_clone = virtual_terminal.clone();
        tokio::spawn(async move {
            while let Ok(event) = output_rx.recv().await {
                let mut vt = vt_clone.write().await;
                vt.process_output(&event.data);
            }
        });

        // Spawn the actor task
        tokio::spawn(async move {
            actor.run().await;
        });

        Ok(InstanceHandle { sender, info })
    }

    async fn run(mut self) {
        let name = self.info.read().await.name.clone();
        debug!("Instance actor '{}' started", name);

        while let Some(cmd) = self.receiver.recv().await {
            let cmd = match handle_vt_command(&self.virtual_terminal, cmd).await {
                Some(cmd) => cmd,
                None => continue,
            };
            match cmd {
                InstanceCommand::GetInfo { respond_to } => {
                    let _ = respond_to.send(self.info.read().await.clone());
                }

                InstanceCommand::WriteInput { text, respond_to } => {
                    debug!("Writing {} bytes to PTY", text.len());
                    let result = self
                        .pty
                        .write_str(&text)
                        .await
                        .map_err(|e| anyhow::anyhow!("{}", e));
                    let _ = respond_to.send(result);
                }

                InstanceCommand::Resize {
                    rows,
                    cols,
                    respond_to,
                } => {
                    let result = self
                        .pty
                        .resize(rows, cols)
                        .await
                        .map_err(|e| anyhow::anyhow!("{}", e));
                    let _ = respond_to.send(result);
                }

                InstanceCommand::SubscribeOutput { respond_to } => {
                    let rx = self.pty.subscribe();
                    let _ = respond_to.send(rx);
                }

                InstanceCommand::SetSessionId {
                    session_id,
                    respond_to,
                } => {
                    debug!("Setting session_id for instance '{}': {}", name, session_id);
                    self.info.write().await.session_id = Some(session_id);
                    let _ = respond_to.send(());
                }

                InstanceCommand::GetSessionId { respond_to } => {
                    let session_id = self.info.read().await.session_id.clone();
                    let _ = respond_to.send(session_id);
                }

                InstanceCommand::GetPid { respond_to } => {
                    let pid = match self.pty.state().await {
                        Ok(state) => state.pid,
                        Err(_) => None,
                    };
                    let _ = respond_to.send(pid);
                }

                InstanceCommand::SetClaudeState { state, respond_to } => {
                    debug!("Setting claude_state for instance '{}': {:?}", name, state);
                    self.info.write().await.claude_state = Some(state);
                    let _ = respond_to.send(());
                }

                InstanceCommand::SetCustomName {
                    name: custom_name,
                    respond_to,
                } => {
                    debug!(
                        "Setting custom_name for instance '{}': {:?}",
                        name, custom_name
                    );
                    self.info.write().await.custom_name = custom_name;
                    let _ = respond_to.send(());
                }

                InstanceCommand::Stop { respond_to } => {
                    debug!("Stopping instance '{}'", name);
                    self.info.write().await.running = false;
                    let result = self
                        .pty
                        .kill(Some("SIGTERM"))
                        .await
                        .map_err(|e| anyhow::anyhow!("{}", e));
                    let _ = respond_to.send(result);
                    break; // Exit the actor loop
                }
                _ => {} // VT commands handled by handle_vt_command
            }
        }

        debug!("Instance actor '{}' stopped", name);
    }
}

/// Create a new instance and return its handle
pub async fn create_instance(opts: SpawnOptions) -> Result<InstanceHandle> {
    InstanceActor::spawn(opts).await
}

#[cfg(test)]
impl InstanceHandle {
    /// Spawn a test actor backed by a VirtualTerminal only (no real PTY).
    /// Returns the handle and a reference to the VT for injecting output.
    fn spawn_test(
        rows: u16,
        cols: u16,
        max_delta_bytes: usize,
    ) -> (Self, Arc<RwLock<VirtualTerminal>>) {
        let info = Arc::new(RwLock::new(InstanceInfo {
            id: "test-instance".to_string(),
            name: "test".to_string(),
            custom_name: None,
            command: "echo test".to_string(),
            working_dir: "/tmp".to_string(),
            running: true,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            session_id: None,
            claude_state: None,
        }));
        let (sender, mut receiver) = mpsc::channel(32);
        let vt = Arc::new(RwLock::new(VirtualTerminal::new(
            rows,
            cols,
            max_delta_bytes,
        )));
        let vt_actor = vt.clone();
        tokio::spawn(async move {
            while let Some(cmd) = receiver.recv().await {
                let cmd = match handle_vt_command(&vt_actor, cmd).await {
                    Some(cmd) => cmd,
                    None => continue,
                };
                match cmd {
                    InstanceCommand::Resize { respond_to, .. } => {
                        let _ = respond_to.send(Ok(()));
                    }
                    InstanceCommand::Stop { respond_to } => {
                        let _ = respond_to.send(Ok(()));
                        break;
                    }
                    _ => {}
                }
            }
        });

        (InstanceHandle { sender, info }, vt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_handle_update_viewport() {
        let (handle, _vt) = InstanceHandle::spawn_test(24, 80, 4096);

        let result = handle
            .update_viewport("client-1", 40, 120, ClientType::Web)
            .await;
        assert_eq!(result, Some((40, 120)));

        // Same dims again → no change
        let result = handle
            .update_viewport("client-1", 40, 120, ClientType::Web)
            .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_handle_set_client_active() {
        let (handle, _vt) = InstanceHandle::spawn_test(24, 80, 4096);

        handle
            .update_viewport("client-1", 40, 120, ClientType::Web)
            .await;
        handle
            .update_viewport("client-2", 24, 80, ClientType::Terminal)
            .await;

        // Hide smaller client → dims go up
        let result = handle.set_client_active("client-2", false).await;
        assert_eq!(result, Some((40, 120)));
    }

    #[tokio::test]
    async fn test_handle_remove_client() {
        let (handle, _vt) = InstanceHandle::spawn_test(24, 80, 4096);

        handle
            .update_viewport("client-1", 40, 120, ClientType::Web)
            .await;
        handle
            .update_viewport("client-2", 24, 80, ClientType::Terminal)
            .await;

        let result = handle.remove_client("client-2").await;
        assert_eq!(result, Some((40, 120)));
    }

    #[tokio::test]
    async fn test_handle_get_recent_output_replay() {
        let (handle, vt) = InstanceHandle::spawn_test(24, 80, 4096);

        // Inject output directly into the VT
        vt.write()
            .await
            .process_output(b"Hello from test\r\nLine 2");

        let output = handle.get_recent_output(4096).await;
        assert_eq!(output.len(), 1);
        assert!(output[0].contains("Hello from test"));
        assert!(output[0].contains("Line 2"));
    }

    #[tokio::test]
    async fn test_handle_get_recent_output_truncation() {
        let (handle, vt) = InstanceHandle::spawn_test(24, 80, 4096);

        vt.write().await.process_output("A".repeat(200).as_bytes());

        let full = handle.get_recent_output(100_000).await;
        let full_len = full[0].len();
        assert!(full_len > 50);

        let truncated = handle.get_recent_output(50).await;
        assert!(truncated[0].len() <= 50);
        assert!(truncated[0].len() < full_len);
    }

    #[tokio::test]
    async fn test_update_viewport_and_resize() {
        let (handle, _vt) = InstanceHandle::spawn_test(24, 80, 4096);

        // Dims change triggers resize (no-op in test actor)
        let result = handle
            .update_viewport_and_resize("conn-1", 40, 120, ClientType::Web)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_set_active_and_resize() {
        let (handle, _vt) = InstanceHandle::spawn_test(24, 80, 4096);

        handle
            .update_viewport("conn-1", 40, 120, ClientType::Web)
            .await;
        handle
            .update_viewport("conn-2", 24, 80, ClientType::Terminal)
            .await;

        // Deactivate smaller → resize to larger
        let result = handle.set_active_and_resize("conn-2", false).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_remove_client_and_resize_no_change() {
        let (handle, _vt) = InstanceHandle::spawn_test(24, 80, 4096);

        // Remove nonexistent → no dims change, no resize, still Ok
        let result = handle.remove_client_and_resize("nonexistent").await;
        assert!(result.is_ok());
    }
}
