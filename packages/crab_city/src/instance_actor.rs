use anyhow::Result;
use pty_manager::PtyOutput;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, mpsc, oneshot};
use tracing::{debug, info, warn};
use uuid::Uuid;

use pty_manager::{PtyConfig, PtyHandle};

use crate::inference::ClaudeState;
use crate::process_driver::{DriverContext, DriverSignal, ProcessDriver, ProcessState};
use crate::repository::ConversationRepository;
use crate::virtual_terminal::{ClientType, VirtualTerminal, VtRecorder};
use crate::ws::ConversationEvent;
use crate::ws::{FirstInputData, PendingAttribution, StateBroadcast};

/// PTY output enriched with cursor position from the VirtualTerminal.
/// Produced by the VT processing task after feeding output to the parser.
#[derive(Debug, Clone)]
pub struct EnrichedOutput {
    pub data: Vec<u8>,
    pub cursor: (u16, u16),
    pub timestamp: i64,
}

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
        respond_to: oneshot::Sender<broadcast::Receiver<EnrichedOutput>>,
    },
    /// Get recent output with a byte limit
    /// Returns chunks from the end of the buffer up to max_bytes total.
    /// `client_rows` is the receiving terminal's height — used to size the
    /// scrollback-to-keyframe flush so no lines are lost or garbled.
    GetRecentOutput {
        max_bytes: usize,
        client_rows: u16,
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
    GetConversationSnapshot {
        respond_to: oneshot::Sender<Vec<serde_json::Value>>,
    },
    SubscribeConversation {
        respond_to: oneshot::Sender<Option<broadcast::Receiver<ConversationEvent>>>,
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

    pub async fn subscribe_output(&self) -> Result<broadcast::Receiver<EnrichedOutput>> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(InstanceCommand::SubscribeOutput { respond_to: tx })
            .await
            .map_err(|_| anyhow::anyhow!("Instance actor is gone"))?;
        rx.await
            .map_err(|_| anyhow::anyhow!("Instance actor didn't respond"))
    }

    /// Get recent output up to max_bytes total.
    /// `client_rows` is the receiving terminal's row count — used to size
    /// the scrollback flush so the client gets the full history.
    pub async fn get_recent_output(&self, max_bytes: usize, client_rows: u16) -> Vec<String> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .sender
            .send(InstanceCommand::GetRecentOutput {
                max_bytes,
                client_rows,
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

    /// Set the claude state directly on the shared info (used by legacy GSM state manager).
    pub async fn set_claude_state(&self, state: ClaudeState) -> Result<()> {
        self.info.write().await.claude_state = Some(state);
        Ok(())
    }

    pub async fn get_conversation_snapshot(&self) -> Vec<serde_json::Value> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .sender
            .send(InstanceCommand::GetConversationSnapshot { respond_to: tx })
            .await;
        rx.await.unwrap_or_default()
    }

    pub async fn subscribe_conversation(&self) -> Option<broadcast::Receiver<ConversationEvent>> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(InstanceCommand::SubscribeConversation { respond_to: tx })
            .await
            .ok()?;
        rx.await.ok().flatten()
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
    /// Number of scrollback lines the server-side vt100 parser retains
    pub scrollback_lines: usize,
    /// Directory to write VT session recordings. None = recording disabled.
    pub vt_record_dir: Option<std::path::PathBuf>,
    /// Process-type-specific driver. Handles state detection and conversation tracking.
    pub driver: Box<dyn ProcessDriver>,
    /// Channel for broadcasting state changes to WebSocket clients.
    pub state_broadcast_tx: Option<StateBroadcast>,
    /// Channel for broadcasting lifecycle events (SessionRotated, etc.).
    pub lifecycle_tx: Option<broadcast::Sender<crate::ws::ServerMessage>>,
    /// Shared session-claiming map (for driver conversation watcher).
    pub claimed_sessions: Arc<RwLock<HashMap<String, String>>>,
    /// Shared first-input data (for session discovery).
    pub first_input_data: Arc<RwLock<HashMap<String, FirstInputData>>>,
    /// Shared pending attributions (for conversation formatting).
    pub pending_attributions: Arc<RwLock<HashMap<String, VecDeque<PendingAttribution>>>>,
    /// Repository for conversation persistence.
    pub repository: Option<Arc<ConversationRepository>>,
}

/// Handle VT commands.
/// Returns `None` if handled, `Some(cmd)` for PTY/metadata commands.
fn handle_vt_command(vt: &mut VirtualTerminal, cmd: InstanceCommand) -> Option<InstanceCommand> {
    match cmd {
        InstanceCommand::UpdateViewport {
            connection_id,
            rows,
            cols,
            client_type,
            respond_to,
        } => {
            let result = vt.update_viewport(&connection_id, rows, cols, client_type);
            let _ = respond_to.send(result);
            None
        }
        InstanceCommand::SetClientActive {
            connection_id,
            active,
            respond_to,
        } => {
            let result = vt.set_active(&connection_id, active);
            let _ = respond_to.send(result);
            None
        }
        InstanceCommand::RemoveClient {
            connection_id,
            respond_to,
        } => {
            let result = vt.remove_client(&connection_id);
            let _ = respond_to.send(result);
            None
        }
        InstanceCommand::GetRecentOutput {
            max_bytes,
            client_rows,
            respond_to,
        } => {
            let replay = vt.replay(client_rows);
            let data = if replay.len() > max_bytes {
                let mut start = replay.len() - max_bytes;
                // Skip past UTF-8 continuation bytes (10xxxxxx) so we
                // don't slice in the middle of a multi-byte character.
                while start < replay.len() && (replay[start] & 0xC0) == 0x80 {
                    start += 1;
                }
                replay[start..].to_vec()
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
    virtual_terminal: VirtualTerminal,
    enriched_tx: broadcast::Sender<EnrichedOutput>,
    recorder: Option<VtRecorder<std::fs::File>>,
    pty_output_rx: broadcast::Receiver<PtyOutput>,
    driver: Box<dyn ProcessDriver>,
    driver_rx: Option<mpsc::Receiver<DriverSignal>>,
    state_broadcast_tx: Option<StateBroadcast>,
    lifecycle_tx: Option<broadcast::Sender<crate::ws::ServerMessage>>,
    claimed_sessions: Arc<RwLock<HashMap<String, String>>>,
    first_input_data: Arc<RwLock<HashMap<String, FirstInputData>>>,
    pending_attributions: Arc<RwLock<HashMap<String, VecDeque<PendingAttribution>>>>,
    repository: Option<Arc<ConversationRepository>>,
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
            claude_state: Some(ClaudeState::Initializing),
        }));

        let (sender, receiver) = mpsc::channel(32);
        let virtual_terminal =
            VirtualTerminal::new(24, 80, opts.max_buffer_bytes, opts.scrollback_lines);
        let (enriched_tx, _) = broadcast::channel::<EnrichedOutput>(64);

        let recorder = opts.vt_record_dir.as_ref().and_then(|dir| {
            if let Err(e) = std::fs::create_dir_all(dir) {
                tracing::warn!(
                    "VT recording disabled — cannot create {}: {e}",
                    dir.display()
                );
                return None;
            }
            let path = dir.join(format!("{id}.vtr"));
            match VtRecorder::open(&path, 24, 80, opts.scrollback_lines as u32) {
                Ok(r) => {
                    debug!("VT recording → {}", path.display());
                    Some(r)
                }
                Err(e) => {
                    tracing::warn!(
                        "VT recording disabled — cannot open {}: {e}",
                        path.display()
                    );
                    None
                }
            }
        });

        let pty_output_rx = pty.subscribe();

        let actor = InstanceActor {
            info: info.clone(),
            pty: pty.clone(),
            receiver,
            virtual_terminal,
            enriched_tx,
            recorder,
            pty_output_rx,
            driver: opts.driver,
            driver_rx: None,
            state_broadcast_tx: opts.state_broadcast_tx,
            lifecycle_tx: opts.lifecycle_tx,
            claimed_sessions: opts.claimed_sessions,
            first_input_data: opts.first_input_data,
            pending_attributions: opts.pending_attributions,
            repository: opts.repository,
        };

        // Spawn the actor task
        tokio::spawn(async move {
            actor.run().await;
        });

        Ok(InstanceHandle { sender, info })
    }

    /// Process PTY output: feed VT, feed driver, record, and broadcast enriched output.
    async fn process_pty_output(&mut self, event: PtyOutput) {
        if let Some(ref mut rec) = self.recorder {
            rec.output(&event.data);
        }
        self.virtual_terminal.process_output(&event.data);
        let cursor = self.virtual_terminal.cursor_position();

        // Feed driver for state detection
        if let Some(new_state) = self.driver.on_output(&event.data) {
            self.broadcast_state(&new_state).await;
        }

        let _ = self.enriched_tx.send(EnrichedOutput {
            data: event.data,
            cursor,
            timestamp: event.timestamp,
        });
    }

    /// Broadcast a state change: update InstanceInfo and send through state_broadcast_tx.
    async fn broadcast_state(&mut self, _process_state: &ProcessState) {
        // Map to ClaudeState for backward compatibility
        let claude_state = if let Some(cs) = self.driver.claude_state() {
            cs.clone()
        } else {
            // Best-effort mapping for non-Claude drivers
            ClaudeState::Idle
        };

        let instance_id = self.info.read().await.id.clone();
        self.info.write().await.claude_state = Some(claude_state.clone());

        if let Some(ref tx) = self.state_broadcast_tx {
            let terminal_stale = self.driver.is_terminal_stale();
            let _ = tx.send((instance_id, claude_state, terminal_stale));
        }
    }

    /// Apply a DriverEffect returned by the driver's on_signal method.
    async fn apply_effect(&mut self, effect: crate::process_driver::DriverEffect) {
        if let Some(ref state) = effect.state_change {
            self.broadcast_state(state).await;
        }

        if let Some(ref session_id) = effect.session_id {
            let instance_id = self.info.read().await.id.clone();
            let old_session = self.info.read().await.session_id.clone();
            self.info.write().await.session_id = Some(session_id.clone());
            info!("Instance '{}' session set to {}", instance_id, session_id);

            // If there was a previous session, broadcast rotation
            if let Some(old) = old_session {
                if let Some(ref ltx) = self.lifecycle_tx {
                    let _ = ltx.send(crate::ws::ServerMessage::SessionRotated {
                        instance_id,
                        from_session: old,
                        to_session: session_id.clone(),
                    });
                }
            }
        }
    }

    async fn run(mut self) {
        let name = self.info.read().await.name.clone();
        debug!("Instance actor '{}' started", name);

        // Start the driver — this may spawn background tasks and return a signal receiver.
        let driver_ctx = DriverContext {
            working_dir: self.info.read().await.working_dir.clone(),
            instance_id: self.info.read().await.id.clone(),
            instance_created_at: chrono::DateTime::parse_from_rfc3339(
                &self.info.read().await.created_at,
            )
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now()),
            claimed_sessions: Arc::clone(&self.claimed_sessions),
            first_input_data: Arc::clone(&self.first_input_data),
            pending_attributions: Arc::clone(&self.pending_attributions),
            repository: self.repository.clone(),
        };
        self.driver_rx = self.driver.start(driver_ctx);

        let mut tick_interval = tokio::time::interval(std::time::Duration::from_millis(500));

        loop {
            tokio::select! {
                Some(cmd) = self.receiver.recv() => {
                    let cmd = match handle_vt_command(&mut self.virtual_terminal, cmd) {
                        Some(cmd) => cmd,
                        None => continue,
                    };
                    match cmd {
                        InstanceCommand::GetInfo { respond_to } => {
                            let _ = respond_to.send(self.info.read().await.clone());
                        }

                        InstanceCommand::WriteInput { text, respond_to } => {
                            debug!("Writing {} bytes to PTY", text.len());
                            if let Some(ref mut rec) = self.recorder {
                                rec.input(text.as_bytes());
                            }
                            // Feed driver for input-based state detection
                            if let Some(new_state) = self.driver.on_input(&text) {
                                self.broadcast_state(&new_state).await;
                            }
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
                            if let Some(ref mut rec) = self.recorder {
                                rec.resize(rows, cols);
                            }
                            let result = self
                                .pty
                                .resize(rows, cols)
                                .await
                                .map_err(|e| anyhow::anyhow!("{}", e));
                            let _ = respond_to.send(result);
                        }

                        InstanceCommand::SubscribeOutput { respond_to } => {
                            let rx = self.enriched_tx.subscribe();
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

                        InstanceCommand::GetConversationSnapshot { respond_to } => {
                            let snapshot = self.driver.conversation_snapshot().to_vec();
                            let _ = respond_to.send(snapshot);
                        }

                        InstanceCommand::SubscribeConversation { respond_to } => {
                            let rx = self.driver.subscribe_conversation();
                            let _ = respond_to.send(rx);
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
                result = self.pty_output_rx.recv() => {
                    match result {
                        Ok(event) => self.process_pty_output(event).await,
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("PTY output lagged by {n} messages");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                signal = async {
                    match self.driver_rx {
                        Some(ref mut rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match signal {
                        Some(signal) => {
                            let effect = self.driver.on_signal(signal);
                            self.apply_effect(effect).await;
                        }
                        None => {
                            // Driver signal channel closed — driver background task ended
                            debug!("Driver signal channel closed for instance '{}'", name);
                            self.driver_rx = None;
                        }
                    }
                }
                _ = tick_interval.tick() => {
                    if let Some(new_state) = self.driver.tick() {
                        self.broadcast_state(&new_state).await;
                    }
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

#[cfg(test)]
impl InstanceHandle {
    /// Spawn a test actor backed by a VirtualTerminal only (no real PTY).
    /// Returns the handle and an `mpsc::Sender<Vec<u8>>` for injecting output.
    /// The test actor processes injected bytes through its own VT (like real PTY output).
    pub(crate) fn spawn_test(
        rows: u16,
        cols: u16,
        max_delta_bytes: usize,
    ) -> (Self, mpsc::Sender<Vec<u8>>) {
        Self::spawn_test_with_scrollback(rows, cols, max_delta_bytes, 0)
    }

    pub(crate) fn spawn_test_with_scrollback(
        rows: u16,
        cols: u16,
        max_delta_bytes: usize,
        scrollback_lines: usize,
    ) -> (Self, mpsc::Sender<Vec<u8>>) {
        use crate::process_driver::ShellDriver;
        let driver: Box<dyn ProcessDriver> = Box::new(ShellDriver);
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
        let mut vt = VirtualTerminal::new(rows, cols, max_delta_bytes, scrollback_lines);
        let (enriched_tx, _) = broadcast::channel::<EnrichedOutput>(64);
        let enriched_tx_actor = enriched_tx.clone();
        let info_actor = info.clone();
        let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(64);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(cmd) = receiver.recv() => {
                        let cmd = match handle_vt_command(&mut vt, cmd) {
                            Some(cmd) => cmd,
                            None => continue,
                        };
                        match cmd {
                            InstanceCommand::Resize { respond_to, .. } => {
                                let _ = respond_to.send(Ok(()));
                            }
                            InstanceCommand::SubscribeOutput { respond_to } => {
                                let rx = enriched_tx_actor.subscribe();
                                let _ = respond_to.send(rx);
                            }
                            InstanceCommand::Stop { respond_to } => {
                                let _ = respond_to.send(Ok(()));
                                break;
                            }
                            InstanceCommand::SetSessionId {
                                session_id,
                                respond_to,
                            } => {
                                info_actor.write().await.session_id = Some(session_id);
                                let _ = respond_to.send(());
                            }
                            InstanceCommand::GetSessionId { respond_to } => {
                                let sid = info_actor.read().await.session_id.clone();
                                let _ = respond_to.send(sid);
                            }
                            InstanceCommand::WriteInput { text, respond_to } => {
                                // Accept writes silently (no real PTY in test)
                                let _ = respond_to.send(Ok(text.len()));
                            }
                            _ => {}
                        }
                    }
                    Some(data) = output_rx.recv() => {
                        vt.process_output(&data);
                        let cursor = vt.cursor_position();
                        let _ = enriched_tx_actor.send(EnrichedOutput {
                            data,
                            cursor,
                            timestamp: 0,
                        });
                    }
                    else => break,
                }
            }
        });

        (InstanceHandle { sender, info }, output_tx)
    }

    /// Inject output into the test actor and yield so it processes the bytes.
    pub(crate) async fn inject_output(output_tx: &mpsc::Sender<Vec<u8>>, data: &[u8]) {
        let _ = output_tx.send(data.to_vec()).await;
        // Yield to let the actor task process the injected output
        tokio::task::yield_now().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_handle_update_viewport() {
        let (handle, _output_tx) = InstanceHandle::spawn_test(24, 80, 4096);

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
        let (handle, _output_tx) = InstanceHandle::spawn_test(24, 80, 4096);

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
        let (handle, _output_tx) = InstanceHandle::spawn_test(24, 80, 4096);

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
        let (handle, output_tx) = InstanceHandle::spawn_test(24, 80, 4096);

        // Inject output through the actor
        InstanceHandle::inject_output(&output_tx, b"Hello from test\r\nLine 2").await;

        let output = handle.get_recent_output(4096, 24).await;
        assert_eq!(output.len(), 1);
        assert!(output[0].contains("Hello from test"));
        assert!(output[0].contains("Line 2"));
    }

    #[tokio::test]
    async fn test_handle_get_recent_output_truncation() {
        let (handle, output_tx) = InstanceHandle::spawn_test(24, 80, 4096);

        InstanceHandle::inject_output(&output_tx, "A".repeat(200).as_bytes()).await;

        let full = handle.get_recent_output(100_000, 24).await;
        let full_len = full[0].len();
        assert!(full_len > 50);

        let truncated = handle.get_recent_output(50, 24).await;
        assert!(truncated[0].len() <= 50);
        assert!(truncated[0].len() < full_len);
    }

    #[tokio::test]
    async fn test_update_viewport_and_resize() {
        let (handle, _output_tx) = InstanceHandle::spawn_test(24, 80, 4096);

        // Dims change triggers resize (no-op in test actor)
        let result = handle
            .update_viewport_and_resize("conn-1", 40, 120, ClientType::Web)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_set_active_and_resize() {
        let (handle, _output_tx) = InstanceHandle::spawn_test(24, 80, 4096);

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
        let (handle, _output_tx) = InstanceHandle::spawn_test(24, 80, 4096);

        // Remove nonexistent → no dims change, no resize, still Ok
        let result = handle.remove_client_and_resize("nonexistent").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_truncation_never_splits_utf8() {
        let (handle, output_tx) = InstanceHandle::spawn_test(24, 80, 4096);

        // Write box-drawing characters (3 bytes each in UTF-8)
        let content = "─".repeat(40); // 120 bytes
        InstanceHandle::inject_output(&output_tx, content.as_bytes()).await;

        // The replay includes a keyframe prefix (ANSI escapes), so the
        // total is larger than 120 bytes.  Try various max_bytes values
        // that are likely to land mid-character.
        for max_bytes in 1..=150 {
            let output = handle.get_recent_output(max_bytes, 24).await;
            if output.is_empty() {
                continue;
            }
            assert!(
                !output[0].contains('\u{FFFD}'),
                "max_bytes={}: truncation produced replacement character in {:?}",
                max_bytes,
                &output[0][..output[0].len().min(40)],
            );
        }
    }

    #[tokio::test]
    async fn test_truncation_preserves_content() {
        let (handle, output_tx) = InstanceHandle::spawn_test(24, 80, 4096);

        // Mix of ASCII and multi-byte: "hello──world" (5 + 6 + 5 = 16 bytes text)
        let content = "hello──world";
        InstanceHandle::inject_output(&output_tx, content.as_bytes()).await;

        // Full replay should contain the content
        let full = handle.get_recent_output(100_000, 24).await;
        assert!(full[0].contains("hello"));
        assert!(full[0].contains("──"));
        assert!(full[0].contains("world"));

        // Truncated replay should also be valid UTF-8 (no replacement chars)
        for max_bytes in 1..=60 {
            let output = handle.get_recent_output(max_bytes, 24).await;
            if output.is_empty() {
                continue;
            }
            assert!(
                !output[0].contains('\u{FFFD}'),
                "max_bytes={}: got replacement char",
                max_bytes,
            );
        }
    }

    #[tokio::test]
    async fn test_truncation_4byte_chars() {
        let (handle, output_tx) = InstanceHandle::spawn_test(24, 80, 4096);

        // 4-byte emoji characters
        let content = "🦀".repeat(10); // 40 bytes
        InstanceHandle::inject_output(&output_tx, content.as_bytes()).await;

        for max_bytes in 1..=80 {
            let output = handle.get_recent_output(max_bytes, 24).await;
            if output.is_empty() {
                continue;
            }
            assert!(
                !output[0].contains('\u{FFFD}'),
                "max_bytes={}: truncation split a 4-byte character",
                max_bytes,
            );
        }
    }

    // ── Enriched output pipeline tests ───────────────────────────────

    #[tokio::test]
    async fn test_enriched_output_cursor_ascii() {
        let (handle, output_tx) = InstanceHandle::spawn_test(24, 80, 4096);
        let mut rx = handle.subscribe_output().await.unwrap();

        InstanceHandle::inject_output(&output_tx, b"Hello").await;
        let event = rx.recv().await.unwrap();
        assert_eq!(event.data, b"Hello");
        assert_eq!(event.cursor, (0, 5));
    }

    #[tokio::test]
    async fn test_enriched_output_cursor_multiline() {
        let (handle, output_tx) = InstanceHandle::spawn_test(24, 80, 4096);
        let mut rx = handle.subscribe_output().await.unwrap();

        InstanceHandle::inject_output(&output_tx, b"Hello\r\nWorld").await;
        let event = rx.recv().await.unwrap();
        assert_eq!(event.cursor, (1, 5));
    }

    #[tokio::test]
    async fn test_enriched_output_cursor_after_cup() {
        let (handle, output_tx) = InstanceHandle::spawn_test(24, 80, 4096);
        let mut rx = handle.subscribe_output().await.unwrap();

        // CUP moves cursor to row 10, col 20 (1-indexed) → (9, 19) 0-indexed
        InstanceHandle::inject_output(&output_tx, b"\x1b[10;20H").await;
        let event = rx.recv().await.unwrap();
        assert_eq!(event.cursor, (9, 19));
    }

    #[tokio::test]
    async fn test_enriched_output_cursor_tracks_across_chunks() {
        let (handle, output_tx) = InstanceHandle::spawn_test(24, 80, 4096);
        let mut rx = handle.subscribe_output().await.unwrap();

        InstanceHandle::inject_output(&output_tx, b"abc").await;
        let e1 = rx.recv().await.unwrap();
        assert_eq!(e1.cursor, (0, 3));

        InstanceHandle::inject_output(&output_tx, b"def").await;
        let e2 = rx.recv().await.unwrap();
        assert_eq!(e2.cursor, (0, 6));
    }

    #[tokio::test]
    async fn test_subscribe_output_receives_enriched() {
        let (handle, output_tx) = InstanceHandle::spawn_test(24, 80, 4096);
        let mut rx = handle.subscribe_output().await.unwrap();

        InstanceHandle::inject_output(&output_tx, b"Test output").await;
        let event = rx.recv().await.unwrap();
        assert_eq!(event.data, b"Test output");
        assert_eq!(event.cursor, (0, 11));
    }

    /// Simulate the multiplexed WebSocket duplication bug:
    ///
    /// 1. Focus subscribes to PTY output (like focus.rs:231)
    /// 2. PTY output arrives — focus task forwards it to the client
    /// 3. TerminalVisible handler generates a replay that ALSO includes this output
    /// 4. Client processes both — duplication
    ///
    /// This test proves the overlap exists at the instance_actor API level.
    #[tokio::test]
    async fn test_multiplexed_focus_replay_overlap() {
        let (handle, output_tx) = InstanceHandle::spawn_test_with_scrollback(4, 40, 4096, 500);

        // Fill VT with initial content so scrollback has data.
        for i in 0..12 {
            InstanceHandle::inject_output(&output_tx, format!("Init {i:02}\r\n").as_bytes()).await;
        }

        // Step 1: Focus subscribes to output.
        let mut output_rx = handle.subscribe_output().await.unwrap();

        // Step 2: "Late" output arrives after subscribe.
        // These are in BOTH the broadcast receiver AND the VT.
        for i in 0..6 {
            InstanceHandle::inject_output(&output_tx, format!("Late {i:02}\r\n").as_bytes()).await;
        }

        // Step 3: TerminalVisible handler generates replay from VT.
        let replay_chunks = handle.get_recent_output(usize::MAX, 4).await;
        let replay_data = replay_chunks.join("");

        // Collect the broadcast messages the focus task would forward.
        let mut broadcast_bytes = Vec::new();
        while let Ok(event) = output_rx.try_recv() {
            broadcast_bytes.extend_from_slice(&event.data);
        }
        assert!(
            !broadcast_bytes.is_empty(),
            "focus task should have received broadcast output"
        );

        // ── Clean client: replay only (what we want after the fix) ──
        let mut clean = vt100::Parser::new(4, 40, 1000);
        clean.process(replay_data.as_bytes());

        // ── Buggy client: replay THEN broadcast (current broken behavior) ──
        let mut buggy = vt100::Parser::new(4, 40, 1000);
        buggy.process(replay_data.as_bytes());
        buggy.process(&broadcast_bytes);

        // Count "Late 00" occurrences across scrollback + visible screen.
        let clean_count = count_occurrences(&mut clean, 40, "Late 00");
        let buggy_count = count_occurrences(&mut buggy, 40, "Late 00");

        assert_eq!(clean_count, 1, "clean client: 'Late 00' should appear once");
        // BUG: the buggy client sees "Late 00" twice — once from the replay
        // and once from the broadcast overlap pushing content into scrollback.
        assert!(
            buggy_count > 1,
            "buggy client: 'Late 00' should be duplicated, got {buggy_count}"
        );
    }

    /// After draining the broadcast receiver post-replay, the overlap is gone.
    #[tokio::test]
    async fn test_multiplexed_drain_prevents_duplication() {
        let (handle, output_tx) = InstanceHandle::spawn_test_with_scrollback(4, 40, 4096, 500);

        for i in 0..12 {
            InstanceHandle::inject_output(&output_tx, format!("Init {i:02}\r\n").as_bytes()).await;
        }

        let mut output_rx = handle.subscribe_output().await.unwrap();

        for i in 0..6 {
            InstanceHandle::inject_output(&output_tx, format!("Late {i:02}\r\n").as_bytes()).await;
        }

        // Generate replay.
        let replay_chunks = handle.get_recent_output(usize::MAX, 4).await;
        let replay_data = replay_chunks.join("");

        // DRAIN: discard broadcast messages already baked into the replay.
        while output_rx.try_recv().is_ok() {}

        // After the drain, no stale broadcasts remain.
        // Simulate client: write replay only (post-drain no extra output).
        let mut client = vt100::Parser::new(4, 40, 1000);
        client.process(replay_data.as_bytes());

        let count = count_occurrences(&mut client, 40, "Late 00");
        assert_eq!(count, 1, "'Late 00' should appear exactly once after drain");
    }

    /// Count how many times `needle` appears across scrollback + visible screen.
    fn count_occurrences(parser: &mut vt100::Parser, cols: u16, needle: &str) -> usize {
        let mut all_text = String::new();

        // Visible screen
        let (rows, _) = parser.screen().size();
        for r in 0..rows {
            for c in 0..cols {
                if let Some(cell) = parser.screen().cell(r, c) {
                    let s = cell.contents();
                    if s.is_empty() {
                        all_text.push(' ');
                    } else {
                        all_text.push_str(s);
                    }
                }
            }
            all_text.push('\n');
        }

        // Scrollback
        parser.screen_mut().set_scrollback(usize::MAX);
        let depth = parser.screen().scrollback();
        for offset in (1..=depth).rev() {
            parser.screen_mut().set_scrollback(offset);
            for c in 0..cols {
                if let Some(cell) = parser.screen().cell(0, c) {
                    let s = cell.contents();
                    if s.is_empty() {
                        all_text.push(' ');
                    } else {
                        all_text.push_str(s);
                    }
                }
            }
            all_text.push('\n');
        }
        parser.screen_mut().set_scrollback(0);

        all_text.matches(needle).count()
    }
}
