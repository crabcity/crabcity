//! State Inference Engine
//!
//! Analyzes PTY I/O timing and patterns to infer Claude's current state.

use std::time::{Duration, Instant};

use super::state::{ClaudeState, StateEvent};

/// Patterns that indicate tool execution in Claude's output
const TOOL_PATTERNS: &[(&str, &str)] = &[
    ("Read(", "Read"),
    ("Write(", "Write"),
    ("Edit(", "Edit"),
    ("Bash(", "Bash"),
    ("Glob(", "Glob"),
    ("Grep(", "Grep"),
    ("WebFetch(", "WebFetch"),
    ("WebSearch(", "WebSearch"),
    ("Task(", "Task"),
    ("AskUserQuestion(", "AskUserQuestion"),
];

/// Patterns that indicate Claude is waiting for input
const INPUT_PROMPT_PATTERNS: &[&str] = &[
    "> ",         // Standard prompt
    "? ",         // Question prompt
    "(y/n)",      // Yes/no prompt
    "[y/N]",      // Default no prompt
    "[Y/n]",      // Default yes prompt
    "Continue? ", // Continue prompt
];

/// State inference engine that tracks Claude's activity state
pub struct StateInferrer {
    state: ClaudeState,
    buffer: String,
    last_input_time: Option<Instant>,
    last_output_time: Option<Instant>,
    current_tool: Option<String>,

    // Configurable timeouts
    thinking_timeout: Duration,
    idle_timeout: Duration,
    tool_timeout: Duration,
}

impl Default for StateInferrer {
    fn default() -> Self {
        Self::new()
    }
}

impl StateInferrer {
    pub fn new() -> Self {
        Self {
            state: ClaudeState::Idle,
            buffer: String::new(),
            last_input_time: None,
            last_output_time: None,
            current_tool: None,
            thinking_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(2),
            tool_timeout: Duration::from_secs(60),
        }
    }

    /// Get the current state
    pub fn state(&self) -> &ClaudeState {
        &self.state
    }

    /// Called when user sends input to the PTY
    pub fn on_input(&mut self, input: &str) -> Vec<StateEvent> {
        self.last_input_time = Some(Instant::now());
        self.buffer.clear();

        let mut events = vec![];

        // If we were waiting for input and got it, transition to Thinking
        if matches!(
            self.state,
            ClaudeState::Idle | ClaudeState::WaitingForInput { .. }
        ) {
            // Check if this is a tool confirmation (y/n response)
            let is_confirmation = input.trim().len() <= 3
                && (input.to_lowercase().contains('y') || input.to_lowercase().contains('n'));

            if !is_confirmation || !matches!(self.state, ClaudeState::WaitingForInput { .. }) {
                self.state = ClaudeState::Thinking;
                events.push(StateEvent::StateChanged {
                    state: self.state.clone(),
                });
            }
        }

        events
    }

    /// Called when PTY produces output
    pub fn on_output(&mut self, output: &str) -> Vec<StateEvent> {
        self.last_output_time = Some(Instant::now());
        self.buffer.push_str(output);

        // Limit buffer size to prevent memory issues
        const MAX_BUFFER_SIZE: usize = 64 * 1024; // 64KB
        if self.buffer.len() > MAX_BUFFER_SIZE {
            let drain_to = self.buffer.len() - MAX_BUFFER_SIZE / 2;
            self.buffer.drain(..drain_to);
        }

        let mut events = vec![];

        // Transition from Thinking -> Responding on first output
        if matches!(self.state, ClaudeState::Thinking) {
            self.state = ClaudeState::Responding;
            events.push(StateEvent::StateChanged {
                state: self.state.clone(),
            });
        }

        // Detect tool execution start
        if let Some(tool) = self.detect_tool_start(output) {
            if self.current_tool.as_ref() != Some(&tool) {
                // New tool started
                if let Some(old_tool) = self.current_tool.take() {
                    events.push(StateEvent::ToolCompleted { tool: old_tool });
                }
                self.current_tool = Some(tool.clone());
                self.state = ClaudeState::ToolExecuting { tool: tool.clone() };
                events.push(StateEvent::ToolStarted { tool: tool.clone() });
                events.push(StateEvent::StateChanged {
                    state: self.state.clone(),
                });
            }
        }

        // Detect tool completion (looking for result patterns)
        if self.current_tool.is_some() && self.detect_tool_complete(output) {
            if let Some(tool) = self.current_tool.take() {
                events.push(StateEvent::ToolCompleted { tool });
            }
            self.state = ClaudeState::Responding;
            events.push(StateEvent::StateChanged {
                state: self.state.clone(),
            });
        }

        // Detect input prompt (Claude waiting for user)
        if self.detect_input_prompt(&self.buffer) {
            // Complete any ongoing tool
            if let Some(tool) = self.current_tool.take() {
                events.push(StateEvent::ToolCompleted { tool });
            }

            let prompt = self.extract_prompt(&self.buffer);
            self.state = ClaudeState::WaitingForInput { prompt };
            events.push(StateEvent::StateChanged {
                state: self.state.clone(),
            });
        }

        events
    }

    /// Called periodically to check for timeouts
    pub fn tick(&mut self) -> Vec<StateEvent> {
        let now = Instant::now();
        let mut events = vec![];

        match &self.state {
            ClaudeState::Responding => {
                // If no output for idle_timeout, consider Claude done
                if let Some(last) = self.last_output_time {
                    if now.duration_since(last) > self.idle_timeout {
                        // Check if buffer ends with a prompt
                        if self.detect_input_prompt(&self.buffer) {
                            let prompt = self.extract_prompt(&self.buffer);
                            self.state = ClaudeState::WaitingForInput { prompt };
                        } else {
                            self.state = ClaudeState::Idle;
                        }
                        events.push(StateEvent::StateChanged {
                            state: self.state.clone(),
                        });
                    }
                }
            }
            ClaudeState::Thinking => {
                // If thinking for too long with no output, something might be wrong
                if let Some(last) = self.last_input_time {
                    if now.duration_since(last) > self.thinking_timeout {
                        // Stay in thinking but could emit a warning event
                    }
                }
            }
            ClaudeState::ToolExecuting { tool } => {
                // If no output for idle_timeout while executing tool, tool is probably done
                if let Some(last) = self.last_output_time {
                    if now.duration_since(last) > self.idle_timeout {
                        let tool_name = tool.clone();
                        events.push(StateEvent::ToolCompleted { tool: tool_name });
                        self.current_tool = None;
                        // Check if buffer ends with a prompt
                        if self.detect_input_prompt(&self.buffer) {
                            let prompt = self.extract_prompt(&self.buffer);
                            self.state = ClaudeState::WaitingForInput { prompt };
                        } else {
                            self.state = ClaudeState::Idle;
                        }
                        events.push(StateEvent::StateChanged {
                            state: self.state.clone(),
                        });
                    }
                }
            }
            _ => {}
        }

        events
    }

    /// Reset state (e.g., when switching instances)
    pub fn reset(&mut self) {
        self.state = ClaudeState::Idle;
        self.buffer.clear();
        self.last_input_time = None;
        self.last_output_time = None;
        self.current_tool = None;
    }

    fn detect_tool_start(&self, output: &str) -> Option<String> {
        for (pattern, tool) in TOOL_PATTERNS {
            if output.contains(pattern) {
                return Some(tool.to_string());
            }
        }
        None
    }

    fn detect_tool_complete(&self, output: &str) -> bool {
        // Tool completion indicators
        output.contains("✓") || output.contains("✔") || output.contains("Done")
    }

    fn detect_input_prompt(&self, buffer: &str) -> bool {
        // Check last 100 characters for prompt patterns
        let check_range = if buffer.len() > 100 {
            &buffer[buffer.len() - 100..]
        } else {
            buffer
        };

        // Check for prompt patterns at the end
        let trimmed = check_range.trim_end();
        for pattern in INPUT_PROMPT_PATTERNS {
            if trimmed.ends_with(pattern) {
                return true;
            }
        }

        false
    }

    fn extract_prompt(&self, buffer: &str) -> Option<String> {
        // Extract the last line as the prompt
        let last_line = buffer.lines().last()?;
        let trimmed = last_line.trim();
        if !trimmed.is_empty() {
            Some(trimmed.to_string())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_is_idle() {
        let inferrer = StateInferrer::new();
        assert_eq!(*inferrer.state(), ClaudeState::Idle);
    }

    #[test]
    fn test_input_transitions_to_thinking() {
        let mut inferrer = StateInferrer::new();
        let events = inferrer.on_input("hello");
        assert_eq!(*inferrer.state(), ClaudeState::Thinking);
        assert!(!events.is_empty());
    }

    #[test]
    fn test_output_transitions_to_responding() {
        let mut inferrer = StateInferrer::new();
        inferrer.on_input("hello");
        let events = inferrer.on_output("Hi there!");
        assert_eq!(*inferrer.state(), ClaudeState::Responding);
        assert!(!events.is_empty());
    }

    #[test]
    fn test_tool_detection() {
        let mut inferrer = StateInferrer::new();
        inferrer.on_input("read file.txt");
        inferrer.on_output("Starting...");
        let events = inferrer.on_output("Read(file.txt)");

        assert!(matches!(
            inferrer.state(),
            ClaudeState::ToolExecuting { tool } if tool == "Read"
        ));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StateEvent::ToolStarted { tool } if tool == "Read"))
        );
    }

    #[test]
    fn test_default_is_same_as_new() {
        let d = StateInferrer::default();
        assert_eq!(*d.state(), ClaudeState::Idle);
    }

    // ── Tool pattern coverage ──────────────────────────────────────

    #[test]
    fn test_all_tool_patterns_detected() {
        let tools = [
            ("Write(", "Write"),
            ("Edit(", "Edit"),
            ("Bash(", "Bash"),
            ("Glob(", "Glob"),
            ("Grep(", "Grep"),
            ("WebFetch(", "WebFetch"),
            ("WebSearch(", "WebSearch"),
            ("Task(", "Task"),
            ("AskUserQuestion(", "AskUserQuestion"),
        ];

        for (pattern, expected_tool) in tools {
            let mut inf = StateInferrer::new();
            inf.on_input("cmd");
            inf.on_output("starting");
            inf.on_output(pattern);
            assert!(
                matches!(
                    inf.state(),
                    ClaudeState::ToolExecuting { tool } if tool == expected_tool
                ),
                "Expected ToolExecuting({}) for pattern '{}'",
                expected_tool,
                pattern
            );
        }
    }

    #[test]
    fn test_detect_tool_start_no_match() {
        let inf = StateInferrer::new();
        assert!(inf.detect_tool_start("just some text").is_none());
    }

    // ── Tool completion ────────────────────────────────────────────

    #[test]
    fn test_tool_completion_checkmark() {
        let mut inf = StateInferrer::new();
        inf.on_input("cmd");
        inf.on_output("Bash(ls)");
        assert!(matches!(inf.state(), ClaudeState::ToolExecuting { .. }));

        inf.on_output("✓");
        assert_eq!(*inf.state(), ClaudeState::Responding);
    }

    #[test]
    fn test_tool_completion_done() {
        let mut inf = StateInferrer::new();
        inf.on_input("cmd");
        inf.on_output("Edit(file)");
        inf.on_output("Done");
        assert_eq!(*inf.state(), ClaudeState::Responding);
    }

    #[test]
    fn test_tool_completion_heavy_checkmark() {
        let mut inf = StateInferrer::new();
        inf.on_input("cmd");
        inf.on_output("Glob(*.rs)");
        inf.on_output("✔");
        assert_eq!(*inf.state(), ClaudeState::Responding);
    }

    #[test]
    fn test_detect_tool_complete_no_match() {
        let inf = StateInferrer::new();
        assert!(!inf.detect_tool_complete("just regular output"));
    }

    // ── Tool switching ─────────────────────────────────────────────

    #[test]
    fn test_tool_switch_emits_completed_and_started() {
        let mut inf = StateInferrer::new();
        inf.on_input("cmd");
        inf.on_output("Read(a.txt)");
        assert!(matches!(
            inf.state(),
            ClaudeState::ToolExecuting { tool } if tool == "Read"
        ));

        let events = inf.on_output("Write(b.txt)");
        assert!(matches!(
            inf.state(),
            ClaudeState::ToolExecuting { tool } if tool == "Write"
        ));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StateEvent::ToolCompleted { tool } if tool == "Read"))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StateEvent::ToolStarted { tool } if tool == "Write"))
        );
    }

    // ── Input prompt detection ─────────────────────────────────────

    #[test]
    fn test_detect_input_prompt_patterns() {
        // Patterns without trailing whitespace work with trim_end()
        let patterns = ["(y/n)", "[y/N]", "[Y/n]"];
        for pat in patterns {
            let mut inf = StateInferrer::new();
            inf.on_input("cmd");
            inf.on_output("some output ");
            inf.on_output(pat);
            assert!(
                matches!(inf.state(), ClaudeState::WaitingForInput { .. }),
                "Should detect '{}' as input prompt",
                pat
            );
        }
    }

    #[test]
    fn test_detect_input_prompt_long_buffer() {
        let inf = StateInferrer::new();
        // Prompt pattern in the last 100 chars (no trailing space issue)
        let mut buf = "x".repeat(200);
        buf.push_str("Continue? [Y/n]");
        assert!(inf.detect_input_prompt(&buf));
    }

    #[test]
    fn test_detect_input_prompt_no_match() {
        let inf = StateInferrer::new();
        assert!(!inf.detect_input_prompt("regular text without a prompt"));
    }

    #[test]
    fn test_prompt_detection_clears_tool() {
        let mut inf = StateInferrer::new();
        inf.on_input("cmd");
        inf.on_output("Bash(ls)");
        assert!(matches!(inf.state(), ClaudeState::ToolExecuting { .. }));

        let events = inf.on_output("Allow? (y/n)");
        assert!(matches!(inf.state(), ClaudeState::WaitingForInput { .. }));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StateEvent::ToolCompleted { tool } if tool == "Bash"))
        );
    }

    // ── extract_prompt ─────────────────────────────────────────────

    #[test]
    fn test_extract_prompt_last_line() {
        let inf = StateInferrer::new();
        assert_eq!(
            inf.extract_prompt("some output\nDo you want to continue? "),
            Some("Do you want to continue?".to_string())
        );
    }

    #[test]
    fn test_extract_prompt_empty_last_line() {
        let inf = StateInferrer::new();
        // Empty last line returns None
        assert!(inf.extract_prompt("some output\n   ").is_none());
    }

    #[test]
    fn test_extract_prompt_empty_buffer() {
        let inf = StateInferrer::new();
        assert!(inf.extract_prompt("").is_none());
    }

    // ── Tick / timeout behavior ────────────────────────────────────

    #[test]
    fn test_tick_responding_to_idle() {
        let mut inf = StateInferrer::new();
        inf.on_input("cmd");
        inf.on_output("some output");
        assert_eq!(*inf.state(), ClaudeState::Responding);

        // Force last_output_time into the past
        inf.last_output_time = Some(Instant::now() - Duration::from_secs(10));
        let events = inf.tick();
        assert_eq!(*inf.state(), ClaudeState::Idle);
        assert!(!events.is_empty());
    }

    #[test]
    fn test_tick_responding_with_prompt_to_waiting() {
        let mut inf = StateInferrer::new();
        inf.on_input("cmd");
        inf.on_output("Allow? (y/n)");
        // If not already detected, push to buffer and expire
        inf.state = ClaudeState::Responding;
        inf.last_output_time = Some(Instant::now() - Duration::from_secs(10));
        inf.tick();
        assert!(matches!(inf.state(), ClaudeState::WaitingForInput { .. }));
    }

    #[test]
    fn test_tick_tool_executing_to_idle() {
        let mut inf = StateInferrer::new();
        inf.state = ClaudeState::ToolExecuting {
            tool: "Bash".to_string(),
        };
        inf.current_tool = Some("Bash".to_string());
        inf.last_output_time = Some(Instant::now() - Duration::from_secs(10));

        let events = inf.tick();
        assert_eq!(*inf.state(), ClaudeState::Idle);
        assert!(inf.current_tool.is_none());
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StateEvent::ToolCompleted { tool } if tool == "Bash"))
        );
    }

    #[test]
    fn test_tick_thinking_no_transition() {
        let mut inf = StateInferrer::new();
        inf.on_input("cmd");
        assert_eq!(*inf.state(), ClaudeState::Thinking);
        // Even after a long time thinking, state doesn't change
        inf.last_input_time = Some(Instant::now() - Duration::from_secs(60));
        let events = inf.tick();
        assert_eq!(*inf.state(), ClaudeState::Thinking);
        assert!(events.is_empty());
    }

    #[test]
    fn test_tick_idle_no_transition() {
        let mut inf = StateInferrer::new();
        let events = inf.tick();
        assert_eq!(*inf.state(), ClaudeState::Idle);
        assert!(events.is_empty());
    }

    // ── Reset ──────────────────────────────────────────────────────

    #[test]
    fn test_reset_clears_state() {
        let mut inf = StateInferrer::new();
        inf.on_input("cmd");
        inf.on_output("Bash(ls)");
        assert!(matches!(inf.state(), ClaudeState::ToolExecuting { .. }));

        inf.reset();
        assert_eq!(*inf.state(), ClaudeState::Idle);
        assert!(inf.current_tool.is_none());
        assert!(inf.last_input_time.is_none());
        assert!(inf.last_output_time.is_none());
        assert!(inf.buffer.is_empty());
    }

    // ── Buffer overflow ────────────────────────────────────────────

    #[test]
    fn test_buffer_overflow_protection() {
        let mut inf = StateInferrer::new();
        inf.on_input("cmd");

        // Write 100KB to exceed 64KB limit
        let chunk = "x".repeat(10_000);
        for _ in 0..11 {
            inf.on_output(&chunk);
        }

        // Buffer should be capped at ~32KB (half of 64KB max)
        assert!(
            inf.buffer.len() <= 64 * 1024,
            "Buffer should be capped, got {} bytes",
            inf.buffer.len()
        );
    }

    // ── Confirmation input ─────────────────────────────────────────

    #[test]
    fn test_confirmation_input_while_waiting() {
        let mut inf = StateInferrer::new();
        inf.state = ClaudeState::WaitingForInput {
            prompt: Some("Continue? (y/n)".to_string()),
        };

        // 'y' is a confirmation — should NOT transition to Thinking
        let events = inf.on_input("y");
        assert!(matches!(inf.state(), ClaudeState::WaitingForInput { .. }));
        assert!(events.is_empty());
    }

    #[test]
    fn test_non_confirmation_input_while_waiting() {
        let mut inf = StateInferrer::new();
        inf.state = ClaudeState::WaitingForInput {
            prompt: Some("Enter filename:".to_string()),
        };

        // A longer input is not a confirmation
        let events = inf.on_input("my_file.txt");
        assert_eq!(*inf.state(), ClaudeState::Thinking);
        assert!(!events.is_empty());
    }

    // ── Output while already responding (no duplicate transition) ──

    #[test]
    fn test_output_while_responding_stays_responding() {
        let mut inf = StateInferrer::new();
        inf.on_input("cmd");
        inf.on_output("first output");
        assert_eq!(*inf.state(), ClaudeState::Responding);

        let events = inf.on_output("more output");
        assert_eq!(*inf.state(), ClaudeState::Responding);
        // No StateChanged event since we're already Responding
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StateEvent::StateChanged { .. }))
        );
    }

    // ── Same tool detected again — no duplicate events ─────────────

    #[test]
    fn test_same_tool_again_no_duplicate() {
        let mut inf = StateInferrer::new();
        inf.on_input("cmd");
        inf.on_output("Read(a.txt)");
        assert!(matches!(
            inf.state(),
            ClaudeState::ToolExecuting { tool } if tool == "Read"
        ));

        let events = inf.on_output("Read(b.txt)");
        // Same tool — no ToolStarted/ToolCompleted events
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StateEvent::ToolStarted { .. }))
        );
    }

    // ── Input during Responding transitions to Thinking ─────────────

    #[test]
    fn test_input_during_responding_no_transition() {
        let mut inf = StateInferrer::new();
        inf.on_input("first");
        inf.on_output("response");
        assert_eq!(*inf.state(), ClaudeState::Responding);

        // Input during Responding doesn't transition (not Idle or WaitingForInput)
        let events = inf.on_input("second");
        assert_eq!(*inf.state(), ClaudeState::Responding);
        assert!(events.is_empty());
    }
}
