//! Claude-specific ProcessDriver
//!
//! Wraps the inference StateManager for state detection and spawns
//! the server conversation watcher for session discovery + tracking.

use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::inference::{ClaudeState, StateManager, StateManagerConfig, StateSignal};
use crate::process_driver::{
    ConversationEffect, DriverContext, DriverEffect, DriverSignal, ProcessDriver, ProcessState,
};
use crate::ws::ConversationEvent;

/// Claude-specific driver: state detection + conversation tracking.
pub struct ClaudeDriver {
    state_manager: StateManager,
    current_state: ProcessState,
    /// Preserved for backward-compat state broadcasts.
    current_claude_state: ClaudeState,
    instance_id: String,
    conversation_turns: Vec<serde_json::Value>,
    conversation_tx: broadcast::Sender<ConversationEvent>,
    /// Cancellation token for background tasks (conversation watcher).
    cancel: Option<CancellationToken>,
}

impl ClaudeDriver {
    pub fn new() -> Self {
        let (conversation_tx, _) = broadcast::channel(64);
        Self {
            state_manager: StateManager::new(StateManagerConfig::default()),
            current_state: ProcessState::Initializing,
            current_claude_state: ClaudeState::Initializing,
            instance_id: String::new(),
            conversation_turns: Vec::new(),
            conversation_tx,
            cancel: None,
        }
    }

    /// Map ClaudeState → ProcessState.
    fn map_state(claude: &ClaudeState) -> ProcessState {
        match claude {
            ClaudeState::Initializing => ProcessState::Initializing,
            ClaudeState::Starting => ProcessState::Starting,
            ClaudeState::Idle => ProcessState::Idle,
            ClaudeState::Thinking | ClaudeState::Responding => {
                ProcessState::Working { detail: None }
            }
            ClaudeState::ToolExecuting { tool } => ProcessState::Working {
                detail: Some(tool.clone()),
            },
            ClaudeState::WaitingForInput { prompt } => ProcessState::WaitingForInput {
                prompt: prompt.clone(),
            },
        }
    }

    /// Process a StateManager result: update internal state, return ProcessState if changed.
    fn apply_state_change(&mut self, new_claude: Option<ClaudeState>) -> Option<ProcessState> {
        if let Some(ref state) = new_claude {
            self.current_claude_state = state.clone();
            let new_process = Self::map_state(state);
            if new_process != self.current_state {
                self.current_state = new_process.clone();
                return Some(new_process);
            }
        }
        None
    }
}

impl Drop for ClaudeDriver {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
    }
}

impl ProcessDriver for ClaudeDriver {
    fn on_output(&mut self, data: &[u8]) -> Option<ProcessState> {
        let text = String::from_utf8_lossy(data).to_string();
        let result = self
            .state_manager
            .process(StateSignal::TerminalOutput { data: text });
        self.apply_state_change(result)
    }

    fn on_input(&mut self, data: &str) -> Option<ProcessState> {
        let result = self.state_manager.process(StateSignal::TerminalInput {
            data: data.to_string(),
        });
        self.apply_state_change(result)
    }

    fn tick(&mut self) -> Option<ProcessState> {
        let result = self.state_manager.process(StateSignal::Tick);
        self.apply_state_change(result)
    }

    fn start(&mut self, ctx: DriverContext) -> Option<mpsc::Receiver<DriverSignal>> {
        self.instance_id = ctx.instance_id.clone();

        let (driver_tx, driver_rx) = mpsc::channel(100);
        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());

        tokio::spawn(crate::ws::run_driver_conversation_watcher(
            ctx.instance_id,
            ctx.working_dir,
            ctx.instance_created_at,
            cancel,
            driver_tx,
            ctx.claimed_sessions,
            ctx.first_input_data,
            ctx.pending_attributions,
            ctx.repository,
        ));

        Some(driver_rx)
    }

    fn on_signal(&mut self, signal: DriverSignal) -> DriverEffect {
        match signal {
            DriverSignal::ConversationEntry {
                entry_type,
                subtype,
                stop_reason,
                tool_names,
            } => {
                let result = self.state_manager.process(StateSignal::ConversationEntry {
                    entry_type,
                    subtype,
                    stop_reason,
                    tool_names,
                });
                DriverEffect {
                    state_change: self.apply_state_change(result),
                    session_id: None,
                    conversation: None,
                }
            }
            DriverSignal::SessionDiscovered(session_id) => DriverEffect {
                state_change: None,
                session_id: Some(session_id),
                conversation: None,
            },
            DriverSignal::ConversationSnapshot(turns) => {
                self.conversation_turns = turns.clone();
                let _ = self.conversation_tx.send(ConversationEvent::Full {
                    instance_id: self.instance_id.clone(),
                    turns: turns.clone(),
                });
                DriverEffect {
                    state_change: None,
                    session_id: None,
                    conversation: Some(ConversationEffect::Full(turns)),
                }
            }
            DriverSignal::ConversationDelta(turns) => {
                self.conversation_turns.extend(turns.clone());
                let _ = self.conversation_tx.send(ConversationEvent::Update {
                    instance_id: self.instance_id.clone(),
                    turns: turns.clone(),
                });
                DriverEffect {
                    state_change: None,
                    session_id: None,
                    conversation: Some(ConversationEffect::Delta(turns)),
                }
            }
        }
    }

    fn state(&self) -> ProcessState {
        self.current_state.clone()
    }

    fn claude_state(&self) -> Option<&ClaudeState> {
        Some(&self.current_claude_state)
    }

    fn is_terminal_stale(&self) -> bool {
        self.state_manager.is_terminal_stale()
    }

    fn conversation_snapshot(&self) -> &[serde_json::Value] {
        &self.conversation_turns
    }

    fn subscribe_conversation(&self) -> Option<broadcast::Receiver<ConversationEvent>> {
        Some(self.conversation_tx.subscribe())
    }
}
