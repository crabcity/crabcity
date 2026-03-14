//! Claude-specific ProcessDriver
//!
//! Wraps the inference StateManager for state detection and spawns
//! the server conversation watcher for session discovery + tracking.

use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::inference::{ClaudeState, StateManager, StateManagerConfig, StateSignal};
use crate::process_driver::{
    DriverContext, DriverEffect, DriverSignal, ProcessDriver, ProcessState,
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
                }
            }
            DriverSignal::SessionDiscovered(session_id) => DriverEffect {
                state_change: None,
                session_id: Some(session_id),
            },
            DriverSignal::ConversationSnapshot(turns) => {
                debug_assert!(!self.instance_id.is_empty());
                self.conversation_turns = turns;
                let _ = self.conversation_tx.send(ConversationEvent::Full {
                    instance_id: self.instance_id.clone(),
                    turns: self.conversation_turns.clone(),
                });
                DriverEffect::none()
            }
            DriverSignal::ConversationDelta(turns) => {
                debug_assert!(!self.instance_id.is_empty());
                let _ = self.conversation_tx.send(ConversationEvent::Update {
                    instance_id: self.instance_id.clone(),
                    turns: turns.clone(),
                });
                self.conversation_turns.extend(turns);
                DriverEffect::none()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn new_driver() -> ClaudeDriver {
        ClaudeDriver::new()
    }

    fn started_driver() -> ClaudeDriver {
        let mut d = ClaudeDriver::new();
        d.instance_id = "test-instance".to_string();
        d
    }

    // ── map_state ──────────────────────────────────────────────────────

    #[test]
    fn map_state_all_variants() {
        assert_eq!(
            ClaudeDriver::map_state(&ClaudeState::Initializing),
            ProcessState::Initializing
        );
        assert_eq!(
            ClaudeDriver::map_state(&ClaudeState::Starting),
            ProcessState::Starting
        );
        assert_eq!(
            ClaudeDriver::map_state(&ClaudeState::Idle),
            ProcessState::Idle
        );
        assert_eq!(
            ClaudeDriver::map_state(&ClaudeState::Thinking),
            ProcessState::Working { detail: None }
        );
        assert_eq!(
            ClaudeDriver::map_state(&ClaudeState::Responding),
            ProcessState::Working { detail: None }
        );
        assert_eq!(
            ClaudeDriver::map_state(&ClaudeState::ToolExecuting {
                tool: "Bash".into()
            }),
            ProcessState::Working {
                detail: Some("Bash".into())
            }
        );
        assert_eq!(
            ClaudeDriver::map_state(&ClaudeState::WaitingForInput {
                prompt: Some("y/n".into())
            }),
            ProcessState::WaitingForInput {
                prompt: Some("y/n".into())
            }
        );
    }

    #[test]
    fn thinking_responding_collapse() {
        // Both map to the same ProcessState — no spurious transition between them.
        let thinking = ClaudeDriver::map_state(&ClaudeState::Thinking);
        let responding = ClaudeDriver::map_state(&ClaudeState::Responding);
        assert_eq!(thinking, responding);

        // Verify the driver doesn't emit a state change for Thinking→Responding.
        let mut d = new_driver();
        d.current_state = ProcessState::Working { detail: None };
        d.current_claude_state = ClaudeState::Thinking;
        let result = d.apply_state_change(Some(ClaudeState::Responding));
        assert!(
            result.is_none(),
            "Thinking→Responding should not emit a state change"
        );
        // But ClaudeState should still update.
        assert_eq!(d.current_claude_state, ClaudeState::Responding);
    }

    // ── apply_state_change ─────────────────────────────────────────────

    #[test]
    fn apply_state_change_returns_some_on_change() {
        let mut d = new_driver();
        assert_eq!(d.current_state, ProcessState::Initializing);
        let result = d.apply_state_change(Some(ClaudeState::Idle));
        assert_eq!(result, Some(ProcessState::Idle));
        assert_eq!(d.current_state, ProcessState::Idle);
    }

    #[test]
    fn apply_state_change_returns_none_on_same() {
        let mut d = new_driver();
        // Force to Idle first.
        d.apply_state_change(Some(ClaudeState::Idle));
        let result = d.apply_state_change(Some(ClaudeState::Idle));
        assert!(result.is_none());
    }

    #[test]
    fn apply_state_change_returns_none_on_none() {
        let mut d = new_driver();
        let result = d.apply_state_change(None);
        assert!(result.is_none());
    }

    // ── ProcessDriver methods ──────────────────────────────────────────

    #[test]
    fn on_output_causes_state_transition() {
        let mut d = new_driver();
        assert_eq!(d.state(), ProcessState::Initializing);
        // Any terminal output should transition from Initializing → Starting.
        let result = d.on_output(b"some output");
        assert_eq!(result, Some(ProcessState::Starting));
        assert_eq!(d.state(), ProcessState::Starting);
    }

    #[test]
    fn on_input_no_state_change() {
        let mut d = new_driver();
        let result = d.on_input("hello");
        assert!(result.is_none());
    }

    #[test]
    fn tick_no_state_change() {
        let mut d = new_driver();
        let result = d.tick();
        assert!(result.is_none());
    }

    // ── on_signal ──────────────────────────────────────────────────────

    #[test]
    fn on_signal_conversation_entry() {
        let mut d = new_driver();
        // Feed a "user" entry — should transition Initializing → Thinking.
        let effect = d.on_signal(DriverSignal::ConversationEntry {
            entry_type: "user".into(),
            subtype: None,
            stop_reason: None,
            tool_names: vec![],
        });
        // StateManager transitions to Idle first (booting → Idle) then to
        // Thinking (user entry), so the net ProcessState change is Working.
        assert_eq!(
            effect.state_change,
            Some(ProcessState::Working { detail: None })
        );
        assert!(effect.session_id.is_none());
    }

    #[test]
    fn on_signal_session_discovered() {
        let mut d = new_driver();
        let effect = d.on_signal(DriverSignal::SessionDiscovered("sess-abc".into()));
        assert_eq!(effect.session_id, Some("sess-abc".into()));
        assert!(effect.state_change.is_none());
    }

    #[test]
    fn on_signal_snapshot_stores_and_broadcasts() {
        let mut d = started_driver();
        let mut rx = d.conversation_tx.subscribe();

        let turns = vec![serde_json::json!({"role": "user", "text": "hi"})];
        let effect = d.on_signal(DriverSignal::ConversationSnapshot(turns.clone()));

        // Effect is none (conversation data is handled via broadcast, not effect).
        assert_eq!(effect, DriverEffect::none());

        // Turns are stored.
        assert_eq!(d.conversation_snapshot(), &turns[..]);

        // Broadcast was sent.
        let event = rx.try_recv().unwrap();
        match event {
            ConversationEvent::Full {
                instance_id,
                turns: t,
            } => {
                assert_eq!(instance_id, "test-instance");
                assert_eq!(t, turns);
            }
            _ => panic!("expected Full event"),
        }
    }

    #[test]
    fn on_signal_delta_extends_and_broadcasts() {
        let mut d = started_driver();
        // Seed with initial turns.
        d.conversation_turns = vec![serde_json::json!({"role": "user", "text": "hi"})];
        let mut rx = d.conversation_tx.subscribe();

        let delta = vec![serde_json::json!({"role": "assistant", "text": "hello"})];
        let effect = d.on_signal(DriverSignal::ConversationDelta(delta.clone()));

        assert_eq!(effect, DriverEffect::none());
        assert_eq!(d.conversation_snapshot().len(), 2);

        let event = rx.try_recv().unwrap();
        match event {
            ConversationEvent::Update {
                instance_id,
                turns: t,
            } => {
                assert_eq!(instance_id, "test-instance");
                assert_eq!(t, delta);
            }
            _ => panic!("expected Update event"),
        }
    }

    // ── conversation / subscribe ───────────────────────────────────────

    #[test]
    fn conversation_snapshot_starts_empty() {
        let d = new_driver();
        assert!(d.conversation_snapshot().is_empty());
    }

    #[test]
    fn subscribe_conversation_returns_receiver() {
        let d = new_driver();
        assert!(d.subscribe_conversation().is_some());
    }

    // ── state consistency ──────────────────────────────────────────────

    #[test]
    fn state_and_claude_state_consistent() {
        let d = new_driver();
        assert_eq!(d.state(), ProcessState::Initializing);
        assert_eq!(d.claude_state(), Some(&ClaudeState::Initializing));
    }

    #[test]
    fn is_terminal_stale_initially_false() {
        let d = new_driver();
        // Just created — last_terminal_activity is now, so not stale.
        assert!(!d.is_terminal_stale());
    }

    // ── full lifecycle ─────────────────────────────────────────────────

    #[test]
    fn full_lifecycle_sequence() {
        let mut d = new_driver();

        // 1. Terminal output → Initializing → Starting
        let s = d.on_output(b"Loading...");
        assert_eq!(s, Some(ProcessState::Starting));

        // 2. Claude banner → Starting → Idle
        //    The StateManager looks for "Claude Code" in terminal output.
        let s = d.on_output(b"Welcome to Claude Code v1.0");
        assert_eq!(s, Some(ProcessState::Idle));

        // 3. User conversation entry → Idle → Thinking (Working)
        let effect = d.on_signal(DriverSignal::ConversationEntry {
            entry_type: "user".into(),
            subtype: None,
            stop_reason: None,
            tool_names: vec![],
        });
        assert_eq!(
            effect.state_change,
            Some(ProcessState::Working { detail: None })
        );

        // 4. Terminal output while Thinking → stays Working (Responding
        //    collapses into same ProcessState).
        let s = d.on_output(b"I'll help you with that.");
        assert!(s.is_none());
    }
}
