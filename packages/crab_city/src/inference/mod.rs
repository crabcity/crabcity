//! Claude State Inference
//!
//! This module provides state inference for Claude instances by analyzing
//! signals from multiple sources: terminal I/O and conversation JSONL.
//!
//! # Architecture
//!
//! The unified state manager receives signals from:
//! - Terminal output (for immediate tool detection)
//! - Terminal input (for thinking state)
//! - Conversation watcher (for authoritative turn completion via stop_reason)
//!
//! And maintains a state machine:
//! - `Idle` - Waiting for user input
//! - `Thinking` - User sent input, no output yet
//! - `Responding` - Claude is streaming a response
//! - `ToolExecuting` - Claude is running a tool
//! - `WaitingForInput` - Claude is waiting for user confirmation

#[allow(dead_code)]
mod engine;
mod manager;
mod state;

pub use manager::{StateManagerConfig, spawn_state_manager};
pub use state::{ClaudeState, StateSignal, StateUpdate};
