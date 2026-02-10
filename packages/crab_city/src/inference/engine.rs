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
}
