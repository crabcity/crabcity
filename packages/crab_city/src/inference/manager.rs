//! Unified State Manager
//!
//! Receives signals from multiple sources (terminal, conversation watcher)
//! and maintains a single source of truth for Claude's state.
//!
//! ## State Detection Strategy
//!
//! State detection uses multiple signal sources, prioritized as follows:
//!
//! 1. **Conversation JSONL** (authoritative):
//!    - `turn_duration` system entry: Definitive turn completion
//!    - `end_turn` stop_reason: Assistant finished responding
//!    - `user` entry type: User sent message → Thinking
//!
//! 2. **Terminal output patterns** (heuristic):
//!    - Tool invocation patterns like "Read(", "Bash(" etc.
//!    - Used to detect tool execution during response
//!    - May have false positives if patterns appear in user messages
//!
//! 3. **Timeout fallback** (safety net):
//!    - 10-second idle timeout as last resort
//!    - Only used when authoritative signals are missed
//!
//! ## Pattern Versioning
//!
//! Terminal patterns may need updating when Claude CLI changes output format.
//! The current patterns are based on Claude Code CLI v1.x spinner output format.

use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::debug;

use super::state::{ClaudeState, StateSignal, StateUpdate};

/// Tool patterns for detecting tool execution from terminal output.
///
/// Format: (pattern_to_match, tool_name)
///
/// These patterns match Claude CLI's spinner output format, e.g.:
/// - "⠋ Read(src/main.rs)" during file read
/// - "⠙ Bash(ls -la)" during command execution
///
/// **Note:** These are heuristic - conversation JSONL is the authoritative source
/// for state transitions. Terminal patterns provide faster feedback during execution.
///
/// **Important:** More specific patterns must come before less specific ones!
/// E.g., "NotebookEdit(" must come before "Edit(" to match correctly.
///
/// Version: 1.0 (Claude Code CLI v1.x format)
/// Last updated: 2026-02-04
const TOOL_PATTERNS_V1: &[(&str, &str)] = &[
    // Notebook operations (before Edit - more specific)
    ("NotebookEdit(", "NotebookEdit"),
    // Todo operations (before Read/Write - more specific)
    ("TodoRead(", "TodoRead"),
    ("TodoWrite(", "TodoWrite"),
    // Web operations (before Search - more specific)
    ("WebFetch(", "WebFetch"),
    ("WebSearch(", "WebSearch"),
    // Agent/task operations
    ("AskUserQuestion(", "AskUserQuestion"),
    ("Task(", "Task"),
    // File operations (general patterns last)
    ("Read(", "Read"),
    ("Write(", "Write"),
    ("Edit(", "Edit"),
    ("Glob(", "Glob"),
    ("Grep(", "Grep"),
    // System operations
    ("Bash(", "Bash"),
];

/// Currently active tool patterns
const TOOL_PATTERNS: &[(&str, &str)] = TOOL_PATTERNS_V1;

/// Configuration for the state manager
pub struct StateManagerConfig {
    /// How long after last activity before considering idle
    pub idle_timeout: Duration,
}

impl Default for StateManagerConfig {
    fn default() -> Self {
        Self {
            // Fallback timeout - authoritative signals (end_turn, turn_duration) are preferred
            // This is a safety net for cases where those signals are missed
            idle_timeout: Duration::from_secs(10),
        }
    }
}

/// Unified state manager that processes signals from multiple sources
pub struct StateManager {
    state: ClaudeState,
    config: StateManagerConfig,
    /// Last conversation entry time (for idle detection)
    last_convo_activity: Instant,
    /// Last terminal output time (for staleness indicator - separate from state)
    last_terminal_activity: Instant,
    current_tool: Option<String>,
    /// Track if we've already sent idle for this quiet period
    sent_idle: bool,
    /// Last entry role from conversation (for idle detection)
    last_convo_role: Option<String>,
}

impl StateManager {
    pub fn new(config: StateManagerConfig) -> Self {
        Self {
            state: ClaudeState::Idle,
            config,
            last_convo_activity: Instant::now(),
            last_terminal_activity: Instant::now(),
            current_tool: None,
            sent_idle: false,
            last_convo_role: None,
        }
    }

    /// Process an incoming signal and return new state if changed
    pub fn process(&mut self, signal: StateSignal) -> Option<ClaudeState> {
        let old_state = self.state.clone();

        match &signal {
            StateSignal::TerminalInput { .. } => {
                // User sent input - transition to Thinking
                // Note: We DON'T reset convo activity here - that's for convo entries only
                self.sent_idle = false;
                if matches!(
                    self.state,
                    ClaudeState::Idle | ClaudeState::WaitingForInput { .. }
                ) {
                    self.state = ClaudeState::Thinking;
                }
            }

            StateSignal::TerminalOutput { data } => {
                // Track terminal activity to prevent false idle during extended thinking
                self.last_terminal_activity = Instant::now();
                self.sent_idle = false;

                // Check for tool patterns
                if let Some(tool) = self.detect_tool(data) {
                    self.current_tool = Some(tool.clone());
                    self.state = ClaudeState::ToolExecuting { tool };
                } else if matches!(self.state, ClaudeState::Thinking) {
                    // First output after thinking -> responding
                    self.state = ClaudeState::Responding;
                }
            }

            StateSignal::ConversationEntry {
                entry_type,
                subtype,
                stop_reason,
            } => {
                debug!(
                    "ConversationEntry signal: type={}, subtype={:?}, stop_reason={:?}",
                    entry_type, subtype, stop_reason
                );
                // Only conversation entries reset the idle timer
                self.last_convo_activity = Instant::now();
                self.sent_idle = false;

                // Check for definitive turn completion signal
                if entry_type == "system" && subtype.as_deref() == Some("turn_duration") {
                    debug!("Got turn_duration -> WaitingForInput (definitive)");
                    self.current_tool = None;
                    self.state = ClaudeState::WaitingForInput { prompt: None };
                    return Some(self.state.clone()); // Early return - this is authoritative
                }

                self.last_convo_role = Some(entry_type.clone());

                if entry_type == "user" {
                    self.state = ClaudeState::Thinking;
                } else if entry_type == "assistant" {
                    if stop_reason.as_deref() == Some("end_turn") {
                        debug!("Got end_turn -> WaitingForInput");
                        self.current_tool = None;
                        self.state = ClaudeState::WaitingForInput { prompt: None };
                    } else if !matches!(self.state, ClaudeState::ToolExecuting { .. }) {
                        self.state = ClaudeState::Responding;
                    }
                }
            }

            StateSignal::Tick => {
                // Tick is now only used for staleness tracking, not state transitions.
                // State transitions rely on authoritative signals:
                // - end_turn: assistant finished responding
                // - turn_duration: definitive turn completion
                // No timeout-based idle detection - it causes false positives.
            }
        }

        if self.state != old_state {
            debug!("State changed: {:?} -> {:?}", old_state, self.state);
            Some(self.state.clone())
        } else {
            None
        }
    }

    /// Get current state
    #[allow(dead_code)]
    pub fn state(&self) -> &ClaudeState {
        &self.state
    }

    /// Check if terminal output is stale (no recent activity)
    /// This is separate from state - indicates confidence in the state
    pub fn is_terminal_stale(&self) -> bool {
        self.last_terminal_activity.elapsed() > self.config.idle_timeout
    }

    /// Check if conversation data is stale (no recent entries)
    #[allow(dead_code)]
    pub fn is_conversation_stale(&self) -> bool {
        self.last_convo_activity.elapsed() > self.config.idle_timeout
    }

    /// Get time since last terminal activity
    #[allow(dead_code)]
    pub fn terminal_idle_duration(&self) -> Duration {
        self.last_terminal_activity.elapsed()
    }

    /// Reset state (e.g., when switching instances)
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.state = ClaudeState::Idle;
        self.last_convo_activity = Instant::now();
        self.last_terminal_activity = Instant::now();
        self.current_tool = None;
        self.sent_idle = false;
        self.last_convo_role = None;
    }

    fn detect_tool(&self, output: &str) -> Option<String> {
        for (pattern, tool) in TOOL_PATTERNS {
            if output.contains(pattern) {
                return Some(tool.to_string());
            }
        }
        None
    }
}

/// Spawn a state manager task that processes signals and emits state changes
pub fn spawn_state_manager(
    mut signal_rx: mpsc::Receiver<StateSignal>,
    state_tx: mpsc::Sender<StateUpdate>,
    config: StateManagerConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut manager = StateManager::new(config);
        let mut tick_interval = tokio::time::interval(Duration::from_millis(500));

        loop {
            tokio::select! {
                Some(signal) = signal_rx.recv() => {
                    if let Some(new_state) = manager.process(signal) {
                        let update = StateUpdate {
                            state: new_state,
                            terminal_stale: manager.is_terminal_stale(),
                        };
                        if state_tx.send(update).await.is_err() {
                            break;
                        }
                    }
                }
                _ = tick_interval.tick() => {
                    if let Some(new_state) = manager.process(StateSignal::Tick) {
                        let update = StateUpdate {
                            state: new_state,
                            terminal_stale: manager.is_terminal_stale(),
                        };
                        if state_tx.send(update).await.is_err() {
                            break;
                        }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_manager() -> StateManager {
        StateManager::new(StateManagerConfig::default())
    }

    #[test]
    fn test_initial_state_is_idle() {
        let manager = default_manager();
        assert_eq!(*manager.state(), ClaudeState::Idle);
    }

    #[test]
    fn test_terminal_input_transitions_to_thinking() {
        let mut manager = default_manager();
        let result = manager.process(StateSignal::TerminalInput {
            data: "hello".to_string(),
        });
        assert_eq!(result, Some(ClaudeState::Thinking));
        assert_eq!(*manager.state(), ClaudeState::Thinking);
    }

    #[test]
    fn test_terminal_output_transitions_thinking_to_responding() {
        let mut manager = default_manager();
        manager.process(StateSignal::TerminalInput {
            data: "hello".to_string(),
        });
        let result = manager.process(StateSignal::TerminalOutput {
            data: "Hi there!".to_string(),
        });
        assert_eq!(result, Some(ClaudeState::Responding));
        assert_eq!(*manager.state(), ClaudeState::Responding);
    }

    #[test]
    fn test_terminal_output_with_tool_pattern() {
        let mut manager = default_manager();
        manager.process(StateSignal::TerminalInput {
            data: "read file".to_string(),
        });
        manager.process(StateSignal::TerminalOutput {
            data: "Starting...".to_string(),
        });
        let result = manager.process(StateSignal::TerminalOutput {
            data: "Read(file.txt)".to_string(),
        });
        assert!(matches!(
            result,
            Some(ClaudeState::ToolExecuting { tool }) if tool == "Read"
        ));
    }

    #[test]
    fn test_conversation_entry_turn_duration_transitions_to_waiting() {
        let mut manager = default_manager();
        // Start in Responding state
        manager.process(StateSignal::TerminalInput {
            data: "hello".to_string(),
        });
        manager.process(StateSignal::TerminalOutput {
            data: "response".to_string(),
        });
        assert_eq!(*manager.state(), ClaudeState::Responding);

        // Send turn_duration signal
        let result = manager.process(StateSignal::ConversationEntry {
            entry_type: "system".to_string(),
            subtype: Some("turn_duration".to_string()),
            stop_reason: None,
        });

        assert_eq!(result, Some(ClaudeState::WaitingForInput { prompt: None }));
        assert_eq!(
            *manager.state(),
            ClaudeState::WaitingForInput { prompt: None }
        );
    }

    #[test]
    fn test_conversation_entry_end_turn_transitions_to_waiting() {
        let mut manager = default_manager();
        // Start in Responding state
        manager.process(StateSignal::TerminalInput {
            data: "hello".to_string(),
        });
        manager.process(StateSignal::TerminalOutput {
            data: "response".to_string(),
        });
        assert_eq!(*manager.state(), ClaudeState::Responding);

        // Send assistant entry with end_turn
        let result = manager.process(StateSignal::ConversationEntry {
            entry_type: "assistant".to_string(),
            subtype: None,
            stop_reason: Some("end_turn".to_string()),
        });

        assert_eq!(result, Some(ClaudeState::WaitingForInput { prompt: None }));
    }

    #[test]
    fn test_conversation_entry_user_transitions_to_thinking() {
        let mut manager = default_manager();
        // Start in Idle
        assert_eq!(*manager.state(), ClaudeState::Idle);

        // User entry should transition to Thinking
        let result = manager.process(StateSignal::ConversationEntry {
            entry_type: "user".to_string(),
            subtype: None,
            stop_reason: None,
        });

        assert_eq!(result, Some(ClaudeState::Thinking));
    }

    #[test]
    fn test_conversation_entry_assistant_without_end_turn() {
        let mut manager = default_manager();
        manager.process(StateSignal::TerminalInput {
            data: "hello".to_string(),
        });
        // Should transition to Responding on assistant entry without end_turn
        let result = manager.process(StateSignal::ConversationEntry {
            entry_type: "assistant".to_string(),
            subtype: None,
            stop_reason: None,
        });

        assert_eq!(result, Some(ClaudeState::Responding));
    }

    #[test]
    fn test_turn_duration_from_any_state() {
        // turn_duration should ALWAYS transition to WaitingForInput
        let states_to_test = vec![
            ClaudeState::Idle,
            ClaudeState::Thinking,
            ClaudeState::Responding,
            ClaudeState::ToolExecuting {
                tool: "Read".to_string(),
            },
        ];

        for initial_state in states_to_test {
            let mut manager = default_manager();

            // Force to initial state
            match &initial_state {
                ClaudeState::Thinking => {
                    manager.process(StateSignal::TerminalInput {
                        data: "x".to_string(),
                    });
                }
                ClaudeState::Responding => {
                    manager.process(StateSignal::TerminalInput {
                        data: "x".to_string(),
                    });
                    manager.process(StateSignal::TerminalOutput {
                        data: "y".to_string(),
                    });
                }
                ClaudeState::ToolExecuting { .. } => {
                    manager.process(StateSignal::TerminalInput {
                        data: "x".to_string(),
                    });
                    manager.process(StateSignal::TerminalOutput {
                        data: "Read(file)".to_string(),
                    });
                }
                _ => {}
            }

            let result = manager.process(StateSignal::ConversationEntry {
                entry_type: "system".to_string(),
                subtype: Some("turn_duration".to_string()),
                stop_reason: None,
            });

            assert_eq!(
                result,
                Some(ClaudeState::WaitingForInput { prompt: None }),
                "turn_duration should transition from {:?} to WaitingForInput",
                initial_state
            );
        }
    }

    #[test]
    fn test_repeated_terminal_output_no_state_change() {
        let mut manager = default_manager();
        manager.process(StateSignal::TerminalInput {
            data: "hello".to_string(),
        });
        manager.process(StateSignal::TerminalOutput {
            data: "response".to_string(),
        });
        assert_eq!(*manager.state(), ClaudeState::Responding);

        // Additional output should NOT emit state change
        let result = manager.process(StateSignal::TerminalOutput {
            data: "more response".to_string(),
        });
        assert_eq!(result, None);
        assert_eq!(*manager.state(), ClaudeState::Responding);
    }

    #[test]
    fn test_terminal_activity_tracked_for_staleness() {
        // Verify that terminal activity is tracked separately from conversation activity
        let mut manager = StateManager::new(StateManagerConfig {
            idle_timeout: Duration::from_millis(50),
        });

        // Initial state - both should be fresh
        assert!(!manager.is_terminal_stale());
        assert!(!manager.is_conversation_stale());

        // Wait for timeout
        std::thread::sleep(Duration::from_millis(100));

        // Both should now be stale
        assert!(manager.is_terminal_stale());
        assert!(manager.is_conversation_stale());

        // Terminal output should refresh terminal staleness
        manager.process(StateSignal::TerminalOutput {
            data: "output".to_string(),
        });
        assert!(!manager.is_terminal_stale());
        assert!(manager.is_conversation_stale()); // Convo still stale

        // Conversation entry should refresh convo staleness
        manager.process(StateSignal::ConversationEntry {
            entry_type: "assistant".to_string(),
            subtype: None,
            stop_reason: None,
        });
        assert!(!manager.is_conversation_stale());
    }

    // ==========================================
    // Real Claude Output Sample Tests
    // ==========================================
    // These test against actual Claude CLI output patterns to catch
    // regressions when Claude CLI format changes.

    /// Real Claude CLI output samples and expected tool detection results
    /// Format: (terminal_output, expected_tool_name_or_none)
    const REAL_CLAUDE_SAMPLES: &[(&str, Option<&str>)] = &[
        // Tool execution with spinner
        ("⠋ Read(src/main.rs)", Some("Read")),
        ("⠙ Read(/path/to/file.txt)", Some("Read")),
        ("⠹ Write(output.json)", Some("Write")),
        ("⠸ Edit(src/lib.rs)", Some("Edit")),
        ("⠼ Bash(ls -la)", Some("Bash")),
        ("⠴ Bash(cargo build)", Some("Bash")),
        ("⠦ Glob(**/*.rs)", Some("Glob")),
        ("⠧ Grep(TODO)", Some("Grep")),
        ("⠇ WebFetch(https://example.com)", Some("WebFetch")),
        ("⠏ WebSearch(rust async)", Some("WebSearch")),
        ("⠋ Task(explore files)", Some("Task")),
        ("⠙ AskUserQuestion(Which option?)", Some("AskUserQuestion")),
        ("⠹ NotebookEdit(notebook.ipynb)", Some("NotebookEdit")),
        // Plain text output (no tool)
        ("Hello! How can I help you today?", None),
        ("Let me analyze this code for you.", None),
        ("I'll help you with that task.", None),
        // Edge cases - tool names in regular text (should NOT match)
        // Note: Current simple pattern matching WILL match these - known limitation
        // The authoritative source (conversation JSONL) prevents false state transitions
        ("I can Read(files) for you", Some("Read")), // Known false positive
        ("Try using Bash(command)", Some("Bash")),   // Known false positive
        // Empty and whitespace
        ("", None),
        ("   ", None),
        ("\n\n", None),
        // Unicode in output
        ("🔍 Searching...", None),
        ("✅ Done!", None),
    ];

    #[test]
    fn test_tool_detection_against_real_samples() {
        let manager = default_manager();

        for (output, expected_tool) in REAL_CLAUDE_SAMPLES {
            let detected = manager.detect_tool(output);
            assert_eq!(
                detected.as_deref(),
                *expected_tool,
                "Tool detection mismatch for output: {:?}\nExpected: {:?}\nGot: {:?}",
                output,
                expected_tool,
                detected
            );
        }
    }

    #[test]
    fn test_all_documented_tools_have_patterns() {
        // List of tools that Claude can use (should have patterns)
        let expected_tools = [
            "Read",
            "Write",
            "Edit",
            "Bash",
            "Glob",
            "Grep",
            "WebFetch",
            "WebSearch",
            "Task",
            "AskUserQuestion",
            "TodoRead",
            "TodoWrite",
            "NotebookEdit",
        ];

        let pattern_tools: std::collections::HashSet<&str> =
            TOOL_PATTERNS.iter().map(|(_, tool)| *tool).collect();

        for tool in expected_tools {
            assert!(
                pattern_tools.contains(tool),
                "Missing pattern for tool: {}",
                tool
            );
        }
    }

    #[test]
    fn test_tool_detection_is_case_sensitive() {
        let manager = default_manager();

        // Should match exact case
        assert_eq!(manager.detect_tool("Read(file)"), Some("Read".to_string()));

        // Should NOT match different case
        assert_eq!(manager.detect_tool("read(file)"), None);
        assert_eq!(manager.detect_tool("READ(file)"), None);
    }

    #[test]
    fn test_tool_detection_requires_open_paren() {
        let manager = default_manager();

        // Should match with open paren
        assert_eq!(manager.detect_tool("Read(file)"), Some("Read".to_string()));

        // Should NOT match without open paren
        assert_eq!(manager.detect_tool("Read file"), None);
        assert_eq!(manager.detect_tool("Reading"), None);
    }
}
