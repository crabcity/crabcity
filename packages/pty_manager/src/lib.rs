//! PTY Manager - Pure PTY lifecycle management library
//!
//! This crate provides a clean, minimal API for managing PTY sessions.
//! It has no HTTP dependencies and no domain-specific knowledge (e.g., no Claude awareness).
//!
//! # Example
//!
//! ```no_run
//! use pty_manager::{PtyManager, PtyConfig};
//!
//! #[tokio::main]
//! async fn main() {
//!     let manager = PtyManager::new();
//!
//!     let config = PtyConfig {
//!         command: "/bin/bash".to_string(),
//!         args: vec![],
//!         working_dir: Some("/tmp".to_string()),
//!         ..Default::default()
//!     };
//!
//!     let id = manager.spawn(config).await.unwrap();
//!
//!     // Write to the PTY
//!     manager.write_str(id, "echo hello\n").await.unwrap();
//!
//!     // Subscribe to output
//!     let mut rx = manager.subscribe();
//!     while let Ok(event) = rx.recv().await {
//!         match event {
//!             pty_manager::PtyEvent::Output { id, data } => {
//!                 println!("PTY {}: {:?}", id, String::from_utf8_lossy(&data));
//!             }
//!             pty_manager::PtyEvent::Exited { id, .. } => {
//!                 println!("PTY {} exited", id);
//!                 break;
//!             }
//!         }
//!     }
//! }
//! ```

mod error;
mod manager;
pub mod pty;

pub use error::PtyError;
pub use manager::{PtyEvent, PtyId, PtyManager};
pub use pty::{PtyConfig, PtyHandle, PtyOutput, PtyState};
