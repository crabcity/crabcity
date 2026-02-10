use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{error, info, warn};

use crate::error::PtyError;

/// Configuration for spawning a PTY
#[derive(Clone, Debug)]
pub struct PtyConfig {
    pub command: String,
    pub args: Vec<String>,
    pub working_dir: Option<String>,
    pub env: Vec<(String, String)>,
    pub rows: u16,
    pub cols: u16,
}

impl Default for PtyConfig {
    fn default() -> Self {
        Self {
            command: "/bin/bash".to_string(),
            args: Vec::new(),
            working_dir: None,
            env: Vec::new(),
            rows: 24,
            cols: 80,
        }
    }
}

/// State of a PTY session
#[derive(Clone, Debug)]
pub struct PtyState {
    pub running: bool,
    pub pid: Option<u32>,
    pub command: String,
    pub args: Vec<String>,
    pub rows: u16,
    pub cols: u16,
}

/// Output event from a PTY
#[derive(Clone, Debug)]
pub struct PtyOutput {
    pub data: Vec<u8>,
    pub timestamp: i64,
}

/// Messages that can be sent to the PTY actor
pub(crate) enum PtyMessage {
    WriteInput {
        data: Vec<u8>,
        respond_to: oneshot::Sender<Result<usize, PtyError>>,
    },
    Resize {
        rows: u16,
        cols: u16,
        respond_to: oneshot::Sender<Result<(), PtyError>>,
    },
    GetState {
        respond_to: oneshot::Sender<PtyState>,
    },
    Kill {
        signal: Option<String>,
        respond_to: oneshot::Sender<Result<(), PtyError>>,
    },
}

/// Handle to communicate with a PTY actor
#[derive(Clone)]
pub struct PtyHandle {
    sender: mpsc::Sender<PtyMessage>,
    output_tx: broadcast::Sender<PtyOutput>,
}

impl PtyHandle {
    /// Write data to the PTY
    pub async fn write(&self, data: &[u8]) -> Result<usize, PtyError> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(PtyMessage::WriteInput {
                data: data.to_vec(),
                respond_to: tx,
            })
            .await
            .map_err(|_| PtyError::ChannelError("Failed to send write message".into()))?;
        rx.await
            .map_err(|_| PtyError::ChannelError("Failed to receive write response".into()))?
    }

    /// Write a string to the PTY
    pub async fn write_str(&self, text: &str) -> Result<usize, PtyError> {
        self.write(text.as_bytes()).await
    }

    /// Resize the PTY
    pub async fn resize(&self, rows: u16, cols: u16) -> Result<(), PtyError> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(PtyMessage::Resize {
                rows,
                cols,
                respond_to: tx,
            })
            .await
            .map_err(|_| PtyError::ChannelError("Failed to send resize message".into()))?;
        rx.await
            .map_err(|_| PtyError::ChannelError("Failed to receive resize response".into()))?
    }

    /// Get the current state of the PTY
    pub async fn state(&self) -> Result<PtyState, PtyError> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(PtyMessage::GetState { respond_to: tx })
            .await
            .map_err(|_| PtyError::ChannelError("Failed to send state message".into()))?;
        rx.await
            .map_err(|_| PtyError::ChannelError("Failed to receive state response".into()))
    }

    /// Kill the PTY process
    pub async fn kill(&self, signal: Option<&str>) -> Result<(), PtyError> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(PtyMessage::Kill {
                signal: signal.map(|s| s.to_string()),
                respond_to: tx,
            })
            .await
            .map_err(|_| PtyError::ChannelError("Failed to send kill message".into()))?;
        rx.await
            .map_err(|_| PtyError::ChannelError("Failed to receive kill response".into()))?
    }

    /// Subscribe to output from the PTY
    pub fn subscribe(&self) -> broadcast::Receiver<PtyOutput> {
        self.output_tx.subscribe()
    }
}

/// The PTY actor that manages a single PTY session
pub struct PtyActor {
    master: Box<dyn MasterPty + Send>,
    writer: Option<Box<dyn Write + Send>>,
    child: Box<dyn Child + Send + Sync>,
    state: PtyState,
    receiver: mpsc::Receiver<PtyMessage>,
}

impl PtyActor {
    /// Spawn a new PTY and return a handle to it
    pub fn spawn(config: PtyConfig) -> Result<PtyHandle, PtyError> {
        let pty_system = native_pty_system();

        let pair = pty_system
            .openpty(PtySize {
                rows: config.rows,
                cols: config.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY")
            .map_err(PtyError::from)?;

        let mut cmd = CommandBuilder::new(&config.command);
        for arg in &config.args {
            cmd.arg(arg);
        }

        // Set working directory if provided
        if let Some(dir) = &config.working_dir {
            info!("Setting working directory: {}", dir);
            cmd.cwd(dir);
        }

        // Set environment for proper terminal behavior
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        // Inherit PATH and other essential environment variables
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }
        if let Ok(home) = std::env::var("HOME") {
            cmd.env("HOME", home);
        }
        if let Ok(user) = std::env::var("USER") {
            cmd.env("USER", user);
        }

        // Add custom environment variables
        for (key, value) in &config.env {
            cmd.env(key, value);
        }

        // For shells, ensure they know they're interactive
        if config.command.ends_with("bash")
            || config.command.ends_with("sh")
            || config.command.ends_with("zsh")
        {
            cmd.env("PS1", "$ ");
        }

        info!(
            "Spawning PTY command: {} with args: {:?}",
            config.command, config.args
        );

        let child = pair.slave.spawn_command(cmd).map_err(|e| {
            error!("Failed to spawn command '{}': {}", config.command, e);
            PtyError::CreateFailed(e.to_string())
        })?;

        let pid = child.process_id();
        info!("PTY process started with PID: {:?}", pid);

        let state = PtyState {
            running: true,
            pid,
            command: config.command.clone(),
            args: config.args.clone(),
            rows: config.rows,
            cols: config.cols,
        };

        let (output_tx, _) = broadcast::channel(1024);
        let (msg_tx, msg_rx) = mpsc::channel(32);

        let mut actor = Self {
            master: pair.master,
            writer: None,
            child,
            state,
            receiver: msg_rx,
        };

        // Clone for the output reading thread
        let output_tx_clone = output_tx.clone();
        let mut reader = actor
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")
            .map_err(PtyError::from)?;

        // Spawn blocking thread for reading PTY output
        std::thread::spawn(move || {
            let mut buffer = vec![0u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        info!("PTY EOF detected - process has exited");
                        break;
                    }
                    Ok(n) => {
                        let output = PtyOutput {
                            data: buffer[..n].to_vec(),
                            timestamp: chrono::Utc::now().timestamp_millis(),
                        };
                        let _ = output_tx_clone.send(output);
                    }
                    Err(e) => {
                        warn!("Error reading PTY output: {}", e);
                        break;
                    }
                }
            }
            info!("PTY reader thread exiting");
        });

        // Spawn the actor task
        tokio::spawn(async move {
            actor.run().await;
        });

        Ok(PtyHandle {
            sender: msg_tx,
            output_tx,
        })
    }

    async fn run(&mut self) {
        info!(
            "PTY actor started for command: {} with PID: {:?}",
            self.state.command, self.state.pid
        );

        // Take the writer immediately to keep the PTY stdin open
        if self.writer.is_none() {
            match self.master.take_writer() {
                Ok(writer) => {
                    self.writer = Some(writer);
                    info!("PTY writer obtained, stdin will remain open");
                }
                Err(e) => {
                    error!("Failed to get PTY writer: {}", e);
                }
            }
        }

        while let Some(msg) = self.receiver.recv().await {
            match msg {
                PtyMessage::WriteInput { data, respond_to } => {
                    let result = self.handle_write_input(&data);
                    let _ = respond_to.send(result);
                }
                PtyMessage::Resize {
                    rows,
                    cols,
                    respond_to,
                } => {
                    let result = self.handle_resize(rows, cols);
                    let _ = respond_to.send(result);
                }
                PtyMessage::GetState { respond_to } => {
                    let _ = respond_to.send(self.state.clone());
                }
                PtyMessage::Kill { signal, respond_to } => {
                    let result = self.handle_kill(signal);
                    let is_ok = result.is_ok();
                    let _ = respond_to.send(result);
                    if is_ok {
                        break;
                    }
                }
            }

            // Check if child is still running
            if let Ok(Some(status)) = self.child.try_wait() {
                info!("PTY process exited with status: {:?}", status);
                self.state.running = false;
                self.state.pid = None;
                break;
            }
        }

        info!("PTY actor shutting down");
    }

    fn handle_write_input(&mut self, data: &[u8]) -> Result<usize, PtyError> {
        if self.writer.is_none() {
            self.writer = Some(
                self.master
                    .take_writer()
                    .map_err(|e| PtyError::WriteFailed(e.to_string()))?,
            );
        }

        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| PtyError::WriteFailed("No PTY writer available".into()))?;

        writer
            .write_all(data)
            .map_err(|e| PtyError::WriteFailed(e.to_string()))?;
        writer
            .flush()
            .map_err(|e| PtyError::WriteFailed(e.to_string()))?;

        Ok(data.len())
    }

    fn handle_resize(&mut self, rows: u16, cols: u16) -> Result<(), PtyError> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| PtyError::ResizeFailed(e.to_string()))?;

        self.state.rows = rows;
        self.state.cols = cols;
        Ok(())
    }

    fn handle_kill(&mut self, signal: Option<String>) -> Result<(), PtyError> {
        match signal.as_deref() {
            Some("SIGTERM") | None => {
                #[cfg(unix)]
                {
                    use nix::sys::signal::{Signal, kill};
                    use nix::unistd::Pid;

                    if let Some(pid) = self.state.pid {
                        kill(Pid::from_raw(pid as i32), Signal::SIGTERM)
                            .map_err(|e| PtyError::KillFailed(e.to_string()))?;
                    }
                }
                #[cfg(not(unix))]
                {
                    self.child
                        .kill()
                        .map_err(|e| PtyError::KillFailed(e.to_string()))?;
                }
            }
            Some("SIGKILL") => {
                self.child
                    .kill()
                    .map_err(|e| PtyError::KillFailed(e.to_string()))?;
            }
            Some("SIGINT") => {
                // Send Ctrl+C
                self.handle_write_input(b"\x03")?;
                return Ok(());
            }
            Some(sig) => {
                return Err(PtyError::KillFailed(format!("Unsupported signal: {}", sig)));
            }
        }

        self.state.running = false;
        Ok(())
    }
}
