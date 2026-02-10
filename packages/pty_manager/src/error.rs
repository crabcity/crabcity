use std::fmt;

/// Errors that can occur during PTY operations
#[derive(Debug)]
pub enum PtyError {
    /// Failed to create PTY
    CreateFailed(String),
    /// PTY not found
    NotFound(u64),
    /// Failed to write to PTY
    WriteFailed(String),
    /// Failed to read from PTY
    ReadFailed(String),
    /// Failed to resize PTY
    ResizeFailed(String),
    /// Failed to kill PTY process
    KillFailed(String),
    /// PTY process has exited
    ProcessExited,
    /// Channel communication error
    ChannelError(String),
}

impl fmt::Display for PtyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PtyError::CreateFailed(msg) => write!(f, "Failed to create PTY: {}", msg),
            PtyError::NotFound(id) => write!(f, "PTY not found: {}", id),
            PtyError::WriteFailed(msg) => write!(f, "Failed to write to PTY: {}", msg),
            PtyError::ReadFailed(msg) => write!(f, "Failed to read from PTY: {}", msg),
            PtyError::ResizeFailed(msg) => write!(f, "Failed to resize PTY: {}", msg),
            PtyError::KillFailed(msg) => write!(f, "Failed to kill PTY: {}", msg),
            PtyError::ProcessExited => write!(f, "PTY process has exited"),
            PtyError::ChannelError(msg) => write!(f, "Channel error: {}", msg),
        }
    }
}

impl std::error::Error for PtyError {}

impl From<anyhow::Error> for PtyError {
    fn from(err: anyhow::Error) -> Self {
        PtyError::CreateFailed(err.to_string())
    }
}
