use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, mpsc, oneshot};

use crate::pty_actor::{PtyActor, PtyMessage};

const OUTPUT_BUFFER_LIMIT: usize = 10000; // Keep last 10k lines

pub struct PtyManager {
    sender: mpsc::Sender<PtyMessage>,
    output_buffer: Arc<RwLock<VecDeque<String>>>,
    full_output: Arc<RwLock<String>>,
    _output_rx: broadcast::Receiver<OutputEvent>,
    output_tx: broadcast::Sender<OutputEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PtyState {
    pub running: bool,
    pub pid: Option<u32>,
    pub command: String,
    pub args: Vec<String>,
    pub rows: u16,
    pub cols: u16,
    pub output_lines: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputEvent {
    pub data: String,
    pub timestamp: i64,
}

impl PtyManager {
    pub fn spawn(
        command: &str,
        args: &[String],
        working_dir: Option<&str>,
        show_output: bool,
    ) -> Result<Self> {
        // Start the actor
        let (sender, mut output_rx) = PtyActor::spawn(command, args, working_dir, show_output)?;

        // Create broadcast channel for output distribution
        let (output_tx, _) = broadcast::channel(1024);

        let output_buffer = Arc::new(RwLock::new(VecDeque::new()));
        let full_output = Arc::new(RwLock::new(String::new()));

        // Clone for the output processing task
        let buffer_clone = output_buffer.clone();
        let full_clone = full_output.clone();
        let tx_clone = output_tx.clone();

        // Spawn task to process output events
        tokio::spawn(async move {
            while let Ok(event) = output_rx.recv().await {
                // Update buffers
                {
                    let mut buffer = buffer_clone.write().await;
                    let mut full = full_clone.write().await;

                    full.push_str(&event.data);

                    for line in event.data.lines() {
                        buffer.push_back(line.to_string());
                        if buffer.len() > OUTPUT_BUFFER_LIMIT {
                            buffer.pop_front();
                        }
                    }
                }

                // Rebroadcast for websocket clients
                let _ = tx_clone.send(event);
            }
        });

        Ok(Self {
            sender,
            output_buffer,
            full_output,
            _output_rx: output_tx.subscribe(),
            output_tx,
        })
    }

    pub async fn write_input(&self, text: &str) -> Result<usize> {
        let (respond_to, response) = oneshot::channel();

        self.sender
            .send(PtyMessage::WriteInput {
                text: text.to_string(),
                respond_to,
            })
            .await
            .context("Failed to send write message")?;

        response.await.context("Failed to get write response")?
    }

    pub async fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        let (respond_to, response) = oneshot::channel();

        self.sender
            .send(PtyMessage::Resize {
                rows,
                cols,
                respond_to,
            })
            .await
            .context("Failed to send resize message")?;

        response.await.context("Failed to get resize response")?
    }

    pub async fn kill(&self, signal: Option<&str>) -> Result<()> {
        let (respond_to, response) = oneshot::channel();

        self.sender
            .send(PtyMessage::Kill {
                signal: signal.map(|s| s.to_string()),
                respond_to,
            })
            .await
            .context("Failed to send kill message")?;

        response.await.context("Failed to get kill response")?
    }

    pub async fn get_state(&self) -> Result<PtyState> {
        let (respond_to, response) = oneshot::channel();

        self.sender
            .send(PtyMessage::GetState { respond_to })
            .await
            .context("Failed to send get_state message")?;

        let mut state = response.await.context("Failed to get state response")?;

        // Update output line count from buffer
        state.output_lines = self.output_buffer.read().await.len();

        Ok(state)
    }

    pub async fn get_recent_output(&self, lines: usize) -> Vec<String> {
        let buffer = self.output_buffer.read().await;
        buffer.iter().rev().take(lines).rev().cloned().collect()
    }

    pub async fn get_full_output(&self) -> String {
        self.full_output.read().await.clone()
    }

    pub async fn get_output_line_count(&self) -> usize {
        self.output_buffer.read().await.len()
    }

    pub fn subscribe_output(&self) -> broadcast::Receiver<OutputEvent> {
        self.output_tx.subscribe()
    }
}
