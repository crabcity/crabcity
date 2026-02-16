//! Multiplexed WebSocket Handler
//!
//! Single WebSocket connection per client that:
//! - Receives state changes from ALL instances (low bandwidth)
//! - Receives terminal output from ONE focused instance (high bandwidth)
//! - Handles focus switching with history replay

mod conversation_watcher;
pub(crate) mod dispatch;
mod focus;
mod handler;
mod protocol;
mod session_discovery;
mod state_manager;

// Re-export the main types and functions
pub(crate) use focus::Utf8StreamDecoder;
pub use handler::handle_multiplexed_ws;
pub(crate) use protocol::DEFAULT_MAX_HISTORY_BYTES;
pub use protocol::{ClientMessage, ServerMessage, WsUser};
pub use state_manager::{GlobalStateManager, create_state_broadcast};
