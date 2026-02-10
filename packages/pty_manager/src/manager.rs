use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{RwLock, broadcast};
use tracing::debug;

use crate::error::PtyError;
use crate::pty::{PtyActor, PtyConfig, PtyHandle, PtyOutput, PtyState};

/// Unique identifier for a PTY session
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub struct PtyId(pub u64);

impl std::fmt::Display for PtyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pty-{}", self.0)
    }
}

/// Events emitted by managed PTYs
#[derive(Clone, Debug)]
pub enum PtyEvent {
    /// Output from a PTY
    Output { id: PtyId, data: Vec<u8> },
    /// PTY process exited
    Exited { id: PtyId, exit_code: Option<i32> },
}

/// Internal state for a managed PTY
struct ManagedPty {
    handle: PtyHandle,
    output_buffer: VecDeque<Vec<u8>>,
    total_buffer_size: usize,
}

const MAX_BUFFER_SIZE: usize = 1024 * 1024; // 1MB per PTY

/// Manager for multiple PTY sessions
pub struct PtyManager {
    ptys: Arc<RwLock<HashMap<PtyId, ManagedPty>>>,
    next_id: AtomicU64,
    event_tx: broadcast::Sender<PtyEvent>,
}

impl Default for PtyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PtyManager {
    /// Create a new PTY manager
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(1024);
        Self {
            ptys: Arc::new(RwLock::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            event_tx,
        }
    }

    /// Spawn a new PTY with the given configuration
    pub async fn spawn(&self, config: PtyConfig) -> Result<PtyId, PtyError> {
        let id = PtyId(self.next_id.fetch_add(1, Ordering::SeqCst));

        debug!("Spawning PTY {} with command: {}", id, config.command);

        let handle = PtyActor::spawn(config)?;

        // Start a task to forward output to the event channel and buffer
        let mut output_rx = handle.subscribe();
        let ptys = self.ptys.clone();
        let event_tx = self.event_tx.clone();
        let pty_id = id;

        tokio::spawn(async move {
            while let Ok(output) = output_rx.recv().await {
                // Buffer the output
                {
                    let mut ptys_guard = ptys.write().await;
                    if let Some(managed) = ptys_guard.get_mut(&pty_id) {
                        managed.output_buffer.push_back(output.data.clone());
                        managed.total_buffer_size += output.data.len();

                        // Trim buffer if too large
                        while managed.total_buffer_size > MAX_BUFFER_SIZE
                            && !managed.output_buffer.is_empty()
                        {
                            if let Some(removed) = managed.output_buffer.pop_front() {
                                managed.total_buffer_size =
                                    managed.total_buffer_size.saturating_sub(removed.len());
                            }
                        }
                    }
                }

                // Forward to event channel
                let _ = event_tx.send(PtyEvent::Output {
                    id: pty_id,
                    data: output.data,
                });
            }

            // PTY output ended - emit exit event
            let _ = event_tx.send(PtyEvent::Exited {
                id: pty_id,
                exit_code: None,
            });
        });

        // Store the managed PTY
        let managed = ManagedPty {
            handle,
            output_buffer: VecDeque::new(),
            total_buffer_size: 0,
        };

        self.ptys.write().await.insert(id, managed);

        Ok(id)
    }

    /// Write data to a PTY
    pub async fn write(&self, id: PtyId, data: &[u8]) -> Result<usize, PtyError> {
        let ptys = self.ptys.read().await;
        let managed = ptys.get(&id).ok_or(PtyError::NotFound(id.0))?;
        managed.handle.write(data).await
    }

    /// Write a string to a PTY
    pub async fn write_str(&self, id: PtyId, text: &str) -> Result<usize, PtyError> {
        self.write(id, text.as_bytes()).await
    }

    /// Get the state of a PTY
    pub async fn state(&self, id: PtyId) -> Result<PtyState, PtyError> {
        let ptys = self.ptys.read().await;
        let managed = ptys.get(&id).ok_or(PtyError::NotFound(id.0))?;
        managed.handle.state().await
    }

    /// Resize a PTY
    pub async fn resize(&self, id: PtyId, rows: u16, cols: u16) -> Result<(), PtyError> {
        let ptys = self.ptys.read().await;
        let managed = ptys.get(&id).ok_or(PtyError::NotFound(id.0))?;
        managed.handle.resize(rows, cols).await
    }

    /// Kill a PTY process
    pub async fn kill(&self, id: PtyId, signal: Option<&str>) -> Result<(), PtyError> {
        let ptys = self.ptys.read().await;
        let managed = ptys.get(&id).ok_or(PtyError::NotFound(id.0))?;
        managed.handle.kill(signal).await
    }

    /// Remove a PTY from the manager
    pub async fn remove(&self, id: PtyId) -> bool {
        self.ptys.write().await.remove(&id).is_some()
    }

    /// Subscribe to events from all PTYs
    pub fn subscribe(&self) -> broadcast::Receiver<PtyEvent> {
        self.event_tx.subscribe()
    }

    /// Subscribe to output from a specific PTY
    pub async fn subscribe_one(
        &self,
        id: PtyId,
    ) -> Result<broadcast::Receiver<PtyOutput>, PtyError> {
        let ptys = self.ptys.read().await;
        let managed = ptys.get(&id).ok_or(PtyError::NotFound(id.0))?;
        Ok(managed.handle.subscribe())
    }

    /// Get recent output from a PTY (buffered)
    pub async fn recent_output(&self, id: PtyId, max_bytes: usize) -> Result<Vec<u8>, PtyError> {
        let ptys = self.ptys.read().await;
        let managed = ptys.get(&id).ok_or(PtyError::NotFound(id.0))?;

        let mut result = Vec::new();
        let mut remaining = max_bytes;

        // Iterate from the end of the buffer
        for chunk in managed.output_buffer.iter().rev() {
            if remaining == 0 {
                break;
            }
            let take = chunk.len().min(remaining);
            result.extend_from_slice(&chunk[chunk.len() - take..]);
            remaining -= take;
        }

        result.reverse();
        Ok(result)
    }

    /// Get all buffered output from a PTY
    pub async fn full_output(&self, id: PtyId) -> Result<Vec<u8>, PtyError> {
        let ptys = self.ptys.read().await;
        let managed = ptys.get(&id).ok_or(PtyError::NotFound(id.0))?;

        let mut result = Vec::with_capacity(managed.total_buffer_size);
        for chunk in &managed.output_buffer {
            result.extend_from_slice(chunk);
        }
        Ok(result)
    }

    /// List all active PTY IDs
    pub async fn list(&self) -> Vec<PtyId> {
        self.ptys.read().await.keys().copied().collect()
    }

    /// Check if a PTY exists
    pub async fn exists(&self, id: PtyId) -> bool {
        self.ptys.read().await.contains_key(&id)
    }
}
