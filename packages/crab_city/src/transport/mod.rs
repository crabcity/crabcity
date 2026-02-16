//! iroh-based P2P transport layer.
//!
//! Submodules:
//! - `relay` — in-process iroh relay server
//! - `iroh_transport` — QUIC endpoint, connection accept loop, message dispatch
//! - `framing` — length-prefixed JSON envelope over QUIC streams

pub mod framing;
pub mod iroh_transport;
pub mod relay;
pub mod replay_buffer;
