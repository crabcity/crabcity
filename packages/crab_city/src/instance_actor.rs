use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, mpsc, oneshot};
use tracing::debug;
use uuid::Uuid;

use pty_manager::{PtyConfig, PtyHandle, PtyOutput};

use crate::inference::ClaudeState;

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

/// The instance actor that manages a single PTY session
struct InstanceActor {
    info: Arc<RwLock<InstanceInfo>>,
    pty: PtyHandle,
    receiver: mpsc::Receiver<InstanceCommand>,
    output_buffer: Arc<RwLock<Vec<String>>>,
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
        let output_buffer = Arc::new(RwLock::new(Vec::new()));

        let actor = InstanceActor {
            info: info.clone(),
            pty: pty.clone(),
            receiver,
            output_buffer: output_buffer.clone(),
        };

        // Start the output collection task - store raw output chunks
        let mut output_rx = pty.subscribe();
        let buffer_clone = output_buffer.clone();
        let max_buffer = opts.max_buffer_bytes;
        tokio::spawn(async move {
            let mut total_size = 0usize;

            while let Ok(event) = output_rx.recv().await {
                let mut buffer = buffer_clone.write().await;

                // Store complete chunks (keep raw formatting)
                // Convert bytes to string for storage
                let data_str = String::from_utf8_lossy(&event.data).to_string();
                total_size += data_str.len();
                buffer.push(data_str);

                // Limit by total size rather than chunk count
                while total_size > max_buffer && !buffer.is_empty() {
                    if let Some(removed) = buffer.first() {
                        total_size = total_size.saturating_sub(removed.len());
                    }
                    buffer.remove(0);
                }
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

                InstanceCommand::GetRecentOutput {
                    max_bytes,
                    respond_to,
                } => {
                    let buffer = self.output_buffer.read().await;

                    // Walk backwards from end until we hit max_bytes
                    let mut total_bytes = 0usize;
                    let mut start_idx = buffer.len();

                    for (i, chunk) in buffer.iter().enumerate().rev() {
                        let chunk_len = chunk.len();
                        if total_bytes + chunk_len > max_bytes {
                            break;
                        }
                        total_bytes += chunk_len;
                        start_idx = i;
                    }

                    // Return chunks from start_idx to end
                    let recent: Vec<String> = buffer[start_idx..].to_vec();
                    let _ = respond_to.send(recent);
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
            }
        }

        debug!("Instance actor '{}' stopped", name);
    }
}

/// Create a new instance and return its handle
pub async fn create_instance(opts: SpawnOptions) -> Result<InstanceHandle> {
    InstanceActor::spawn(opts).await
}
