//! Claude State Types
//!
//! Defines the state machine for tracking Claude's activity.
//! Uses a unified state manager that receives signals from multiple sources.

use serde::{Deserialize, Serialize};

/// The current state of a Claude instance
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ClaudeState {
    /// Claude is waiting for user input (prompt visible)
    Idle,

    /// User sent input, Claude is processing but no output yet
    Thinking,

    /// Claude is actively streaming a response
    Responding,

    /// Claude is executing a tool
    ToolExecuting { tool: String },

    /// Claude is waiting for user confirmation or additional input
    WaitingForInput { prompt: Option<String> },
}

impl Default for ClaudeState {
    fn default() -> Self {
        ClaudeState::Idle
    }
}

/// Events emitted during state transitions (for external consumers)
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
pub enum StateEvent {
    /// State has changed
    StateChanged { state: ClaudeState },

    /// A tool execution has started
    ToolStarted { tool: String },

    /// A tool execution has completed
    ToolCompleted { tool: String },
}

/// Input signals to the unified state manager (from multiple sources)
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum StateSignal {
    /// Terminal output received (for tool detection)
    TerminalOutput { data: String },

    /// User sent input to terminal
    TerminalInput { data: String },

    /// Conversation entry from JSONL watcher
    ConversationEntry {
        entry_type: String,
        subtype: Option<String>,
        stop_reason: Option<String>,
    },

    /// Periodic tick for timeout detection (fallback)
    Tick,
}

impl ClaudeState {
    /// Returns true if Claude is actively working (not waiting for input)
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            ClaudeState::Thinking | ClaudeState::Responding | ClaudeState::ToolExecuting { .. }
        )
    }

    /// Returns the tool name if currently executing a tool
    pub fn current_tool(&self) -> Option<&str> {
        match self {
            ClaudeState::ToolExecuting { tool } => Some(tool),
            _ => None,
        }
    }
}

/// State update with metadata (sent through channels)
#[derive(Clone, Debug)]
pub struct StateUpdate {
    pub state: ClaudeState,
    /// True if terminal output is stale (no recent activity)
    /// Indicates lower confidence in state accuracy during extended thinking
    pub terminal_stale: bool,
}
