use anyhow::{Context, Result};
use chrono;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{error, info, warn};

use crate::pty_manager::{OutputEvent, PtyState};

/// Messages that can be sent to the PTY actor
pub enum PtyMessage {
    WriteInput {
        text: String,
        respond_to: oneshot::Sender<Result<usize>>,
    },
    Resize {
        rows: u16,
        cols: u16,
        respond_to: oneshot::Sender<Result<()>>,
    },
    GetState {
        respond_to: oneshot::Sender<PtyState>,
    },
    Kill {
        signal: Option<String>,
        respond_to: oneshot::Sender<Result<()>>,
    },
}

/// The PTY actor that runs in a separate task
pub struct PtyActor {
    master: Box<dyn MasterPty + Send>,
    writer: Option<Box<dyn Write + Send>>,
    child: Box<dyn Child + Send + Sync>,
    state: PtyState,
    _output_tx: broadcast::Sender<OutputEvent>,
    receiver: mpsc::Receiver<PtyMessage>,
}

impl PtyActor {
    pub fn spawn(
        command: &str,
        args: &[String],
        working_dir: Option<&str>,
        show_output: bool,
    ) -> Result<(mpsc::Sender<PtyMessage>, broadcast::Receiver<OutputEvent>)> {
        let pty_system = native_pty_system();

        // Default terminal size
        let rows = 24;
        let cols = 80;

        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY")?;

        let mut cmd = CommandBuilder::new(command);
        for arg in args {
            cmd.arg(arg);
        }

        // Set working directory if provided
        if let Some(dir) = working_dir {
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

        // For shells, ensure they know they're interactive
        if command.ends_with("bash") || command.ends_with("sh") || command.ends_with("zsh") {
            cmd.env("PS1", "$ "); // Basic prompt to indicate interactive mode
        }

        // Spawn the command - the PTY slave provides stdin/stdout/stderr
        info!("Spawning PTY command: {} with args: {:?}", command, args);
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| {
                error!("Failed to spawn command '{}': {}", command, e);
                e
            })
            .context("Failed to spawn command")?;

        let pid = child.process_id();
        info!("PTY process started with PID: {:?}", pid);

        let state = PtyState {
            running: true,
            pid,
            command: command.to_string(),
            args: args.to_vec(),
            rows,
            cols,
            output_lines: 0,
        };

        let (output_tx, output_rx) = broadcast::channel(1024);
        let (msg_tx, msg_rx) = mpsc::channel(32);

        let mut actor = Self {
            master: pair.master,
            writer: None,
            child,
            state,
            _output_tx: output_tx.clone(),
            receiver: msg_rx,
        };

        // Clone for the output reading thread
        let output_tx_clone = output_tx.clone();
        let mut reader = actor
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;

        // Spawn blocking thread for reading PTY output
        std::thread::spawn(move || {
            let mut buffer = vec![0u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        // EOF - process exited
                        info!("PTY EOF detected - process has exited");
                        break;
                    }
                    Ok(n) => {
                        let data_bytes = &buffer[..n];

                        // Write raw bytes directly to stdout when showing output
                        // This preserves the exact PTY output including control sequences
                        if show_output {
                            use std::io::Write;
                            let mut stdout = std::io::stdout();
                            let _ = stdout.write_all(data_bytes);
                            let _ = stdout.flush();
                        }

                        let event = OutputEvent {
                            data: String::from_utf8_lossy(data_bytes).to_string(),
                            timestamp: chrono::Utc::now().timestamp_millis(),
                        };
                        // Ignore send errors (no receivers)
                        let _ = output_tx_clone.send(event);
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

        Ok((msg_tx, output_rx))
    }

    async fn run(&mut self) {
        info!(
            "PTY actor started for command: {} with PID: {:?}",
            self.state.command, self.state.pid
        );

        // Take the writer immediately to keep the PTY stdin open
        // This prevents shells from seeing immediate EOF
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
                PtyMessage::WriteInput { text, respond_to } => {
                    let result = self.handle_write_input(text);
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
                        break; // Exit the actor after killing
                    }
                }
            }

            // Check if child is still running
            if let Ok(Some(status)) = self.child.try_wait() {
                info!("PTY process exited with status: {:?}", status);
                self.state.running = false;
                self.state.pid = None;
                break; // Exit the actor loop when process dies
            }
        }

        info!("PTY actor shutting down");
    }

    fn handle_write_input(&mut self, text: String) -> Result<usize> {
        // Get the writer if we don't have it yet (though we should already have it from run())
        if self.writer.is_none() {
            self.writer = Some(
                self.master
                    .take_writer()
                    .context("Failed to get PTY writer")?,
            );
        }

        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("No PTY writer available"))?;
        writer
            .write_all(text.as_bytes())
            .context("Failed to write to PTY")?;
        writer.flush().context("Failed to flush PTY writer")?;
        Ok(text.len())
    }

    fn handle_resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to resize PTY")?;

        self.state.rows = rows;
        self.state.cols = cols;
        Ok(())
    }

    fn handle_kill(&mut self, signal: Option<String>) -> Result<()> {
        match signal.as_deref() {
            Some("SIGTERM") | None => {
                #[cfg(unix)]
                {
                    use nix::sys::signal::{Signal, kill};
                    use nix::unistd::Pid;

                    if let Some(pid) = self.state.pid {
                        kill(Pid::from_raw(pid as i32), Signal::SIGTERM)?;
                    }
                }
                #[cfg(not(unix))]
                {
                    self.child.kill()?;
                }
            }
            Some("SIGKILL") => {
                self.child.kill()?;
            }
            Some("SIGINT") => {
                // Send Ctrl+C
                self.handle_write_input("\x03".to_string())?;
                return Ok(()); // Don't exit actor on SIGINT
            }
            Some(sig) => {
                return Err(anyhow::anyhow!("Unsupported signal: {}", sig));
            }
        }

        self.state.running = false;
        Ok(())
    }
}
