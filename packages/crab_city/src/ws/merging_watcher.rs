//! Cross-poll tool result merging wrapper for ConversationWatcher.
//!
//! The upstream `toolpath-claude` ConversationWatcher trait impl only merges
//! tool results within a single poll batch. If a tool_result entry arrives in
//! poll N+1 but the tool_use was emitted in poll N, the result is silently
//! dropped.
//!
//! This wrapper uses the inherent `poll()` to get raw `ConversationEntry`
//! objects, performs its own Turn conversion via `toolpath_claude::provider::to_turn`,
//! and tracks pending tool_uses across poll boundaries to emit `TurnUpdated`
//! when late-arriving results are merged.

use std::collections::HashMap;
use toolpath_claude::{ConversationEntry, MessageRole};
use toolpath_convo::{ConvoError, ToolResult, Turn, WatcherEvent};

/// A ConversationWatcher wrapper that properly merges tool results across
/// poll boundaries.
///
/// Internally tracks all emitted turns and pending tool_use IDs so that
/// tool_result entries arriving in a later poll can be merged into the
/// correct assistant turn, emitting `WatcherEvent::TurnUpdated`.
pub struct MergingWatcher {
    inner: toolpath_claude::ConversationWatcher,
    /// All turns emitted so far, keyed by turn ID.
    /// Used for cross-poll tool result merging.
    emitted_turns: HashMap<String, Turn>,
    /// Maps tool_use_id → turn_id for tool uses still awaiting results.
    pending_tool_uses: HashMap<String, String>,
}

impl MergingWatcher {
    pub fn new(manager: toolpath_claude::ClaudeConvo, project: String, session_id: String) -> Self {
        Self {
            inner: toolpath_claude::ConversationWatcher::new(manager, project, session_id),
            emitted_turns: HashMap::new(),
            pending_tool_uses: HashMap::new(),
        }
    }

    pub fn session_id(&self) -> &str {
        self.inner.session_id()
    }

    pub fn project(&self) -> &str {
        self.inner.project()
    }

    /// Drain any rotation events detected during the last poll.
    /// Returns Vec<(from_session_id, to_session_id)>.
    pub fn take_pending_rotations(&mut self) -> Vec<(String, String)> {
        self.inner.take_pending_rotations()
    }

    /// Check if an entry is a tool-result-only user message
    /// (no human-authored text, only tool_result parts).
    fn is_tool_result_only(entry: &ConversationEntry) -> bool {
        let Some(msg) = &entry.message else {
            return false;
        };
        msg.role == MessageRole::User && msg.text().is_empty() && !msg.tool_results().is_empty()
    }

    /// Merge a single tool result into in-flight events (same-batch).
    /// Returns true if the result was merged.
    fn merge_into_batch(
        &mut self,
        events: &mut [WatcherEvent],
        tool_use_id: &str,
        result: ToolResult,
    ) -> bool {
        for event in events.iter_mut().rev() {
            if let WatcherEvent::Turn(turn) | WatcherEvent::TurnUpdated(turn) = event
                && let Some(inv) = turn
                    .tool_uses
                    .iter_mut()
                    .find(|tu| tu.id == tool_use_id && tu.result.is_none())
            {
                inv.result = Some(result.clone());
                // Keep emitted_turns in sync
                if let Some(stored) = self.emitted_turns.get_mut(&turn.id)
                    && let Some(sinv) = stored.tool_uses.iter_mut().find(|tu| tu.id == tool_use_id)
                {
                    sinv.result = Some(result);
                }
                self.pending_tool_uses.remove(tool_use_id);
                return true;
            }
        }
        false
    }

    /// Re-derive delegation results for a turn after tool results were merged.
    fn update_delegation_results(turn: &mut Turn) {
        for delegation in &mut turn.delegations {
            if delegation.result.is_none()
                && let Some(tu) = turn
                    .tool_uses
                    .iter()
                    .find(|tu| tu.id == delegation.agent_id)
            {
                delegation.result = tu.result.as_ref().map(|r| r.content.clone());
            }
        }
    }
}

impl toolpath_convo::ConversationWatcher for MergingWatcher {
    fn poll(&mut self) -> toolpath_convo::Result<Vec<WatcherEvent>> {
        // Get raw entries from the inherent poll (re-reads file, returns unseen)
        let entries = self
            .inner
            .poll()
            .map_err(|e| ConvoError::Provider(e.to_string()))?;

        let mut events: Vec<WatcherEvent> = Vec::new();
        // Track which stored turns were updated cross-poll in this batch,
        // so we emit one TurnUpdated per turn rather than per tool result.
        let mut cross_poll_updated: Vec<String> = Vec::new();

        for entry in &entries {
            // No message → progress event (preserve extra fields)
            if entry.message.is_none() {
                let mut data = serde_json::json!({});
                if let Some(obj) = data.as_object_mut() {
                    for (key, value) in &entry.extra {
                        obj.insert(key.clone(), value.clone());
                    }
                    obj.insert("uuid".to_string(), serde_json::json!(entry.uuid));
                    obj.insert("timestamp".to_string(), serde_json::json!(entry.timestamp));
                }
                events.push(WatcherEvent::Progress {
                    kind: entry.entry_type.clone(),
                    data,
                });
                continue;
            }

            // Tool-result-only entries get merged, not emitted as turns
            if Self::is_tool_result_only(entry) {
                let msg = entry.message.as_ref().unwrap();

                for tr in msg.tool_results() {
                    let tool_use_id = tr.tool_use_id.to_string();
                    let result = ToolResult {
                        content: tr.content.text(),
                        is_error: tr.is_error,
                    };

                    // First try same-batch merge
                    if self.merge_into_batch(&mut events, &tool_use_id, result.clone()) {
                        continue;
                    }

                    // Cross-poll merge: find the turn that had this tool_use
                    if let Some(turn_id) = self.pending_tool_uses.remove(&tool_use_id)
                        && let Some(stored) = self.emitted_turns.get_mut(&turn_id)
                    {
                        if let Some(inv) =
                            stored.tool_uses.iter_mut().find(|tu| tu.id == tool_use_id)
                        {
                            inv.result = Some(result);
                        }
                        Self::update_delegation_results(stored);
                        if !cross_poll_updated.contains(&turn_id) {
                            cross_poll_updated.push(turn_id);
                        }
                    }
                    // If no match found anywhere, silently drop
                }
                continue;
            }

            // Regular entry → convert to Turn
            match toolpath_claude::provider::to_turn(entry) {
                Some(turn) => {
                    // Track pending tool uses
                    for tu in &turn.tool_uses {
                        if tu.result.is_none() {
                            self.pending_tool_uses
                                .insert(tu.id.clone(), turn.id.clone());
                        }
                    }
                    // Store for cross-poll merging
                    self.emitted_turns.insert(turn.id.clone(), turn.clone());
                    events.push(WatcherEvent::Turn(Box::new(turn)));
                }
                None => {
                    // Entry has message but conversion failed (shouldn't happen)
                    let mut data = serde_json::json!({});
                    if let Some(obj) = data.as_object_mut() {
                        for (key, value) in &entry.extra {
                            obj.insert(key.clone(), value.clone());
                        }
                        obj.insert("uuid".to_string(), serde_json::json!(entry.uuid));
                        obj.insert("timestamp".to_string(), serde_json::json!(entry.timestamp));
                    }
                    events.push(WatcherEvent::Progress {
                        kind: entry.entry_type.clone(),
                        data,
                    });
                }
            }
        }

        // Emit TurnUpdated for any cross-poll merges
        for turn_id in cross_poll_updated {
            if let Some(turn) = self.emitted_turns.get(&turn_id) {
                events.push(WatcherEvent::TurnUpdated(Box::new(turn.clone())));
            }
        }

        Ok(events)
    }

    fn seen_count(&self) -> usize {
        self.inner.seen_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use toolpath_claude::{ClaudeConvo, PathResolver};
    use toolpath_convo::Role;

    fn create_test_jsonl(claude_dir: &std::path::Path, session_id: &str, entries: &[&str]) {
        let project_dir = claude_dir.join("projects/-test-project");
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(
            project_dir.join(format!("{session_id}.jsonl")),
            entries.join("\n") + "\n",
        )
        .unwrap();
    }

    fn test_manager(claude_dir: &std::path::Path) -> ClaudeConvo {
        let resolver = PathResolver::new().with_claude_dir(claude_dir);
        ClaudeConvo::with_resolver(resolver)
    }

    #[test]
    fn same_batch_tool_results_merged_into_turn() {
        let temp = tempfile::TempDir::new().unwrap();
        let claude_dir = temp.path().join(".claude");

        let entries = vec![
            r#"{"uuid":"u1","type":"user","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"Read file"}}"#,
            r#"{"uuid":"u2","type":"assistant","timestamp":"2024-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"Reading..."},{"type":"tool_use","id":"t1","name":"Read","input":{"path":"test.rs"}}]}}"#,
            r#"{"uuid":"u3","type":"user","timestamp":"2024-01-01T00:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"fn main() {}","is_error":false}]}}"#,
        ];
        create_test_jsonl(&claude_dir, "s1", &entries);

        let mut watcher = MergingWatcher::new(
            test_manager(&claude_dir),
            "/test/project".into(),
            "s1".into(),
        );

        let events = toolpath_convo::ConversationWatcher::poll(&mut watcher).unwrap();

        // user Turn + assistant Turn (with result merged in-place)
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], WatcherEvent::Turn(t) if t.role == Role::User));

        if let WatcherEvent::Turn(t) = &events[1] {
            assert_eq!(t.role, Role::Assistant);
            assert_eq!(t.tool_uses.len(), 1);
            let result = t.tool_uses[0]
                .result
                .as_ref()
                .expect("result should be merged");
            assert_eq!(result.content, "fn main() {}");
            assert!(!result.is_error);
        } else {
            panic!("Expected Turn, got {:?}", events[1]);
        }
    }

    #[test]
    fn cross_poll_tool_results_emit_turn_updated() {
        let temp = tempfile::TempDir::new().unwrap();
        let claude_dir = temp.path().join(".claude");

        // Phase 1: user + assistant with tool_use (no result yet)
        let entries_phase1 = vec![
            r#"{"uuid":"u1","type":"user","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"Read file"}}"#,
            r#"{"uuid":"u2","type":"assistant","timestamp":"2024-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"Reading..."},{"type":"tool_use","id":"t1","name":"Read","input":{"path":"test.rs"}}]}}"#,
        ];
        create_test_jsonl(&claude_dir, "s1", &entries_phase1);

        let mut watcher = MergingWatcher::new(
            test_manager(&claude_dir),
            "/test/project".into(),
            "s1".into(),
        );

        // First poll: 2 turns, assistant has no result yet
        let events1 = toolpath_convo::ConversationWatcher::poll(&mut watcher).unwrap();
        assert_eq!(events1.len(), 2);
        if let WatcherEvent::Turn(t) = &events1[1] {
            assert!(
                t.tool_uses[0].result.is_none(),
                "result should be None initially"
            );
        } else {
            panic!("Expected Turn");
        }

        // Phase 2: append tool result to the file
        use std::io::Write;
        let project_dir = claude_dir.join("projects/-test-project");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(project_dir.join("s1.jsonl"))
            .unwrap();
        writeln!(file, r#"{{"uuid":"u3","type":"user","timestamp":"2024-01-01T00:00:02Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t1","content":"fn main() {{}}","is_error":false}}]}}}}"#).unwrap();

        // Second poll: should get TurnUpdated with the result merged
        let events2 = toolpath_convo::ConversationWatcher::poll(&mut watcher).unwrap();
        assert_eq!(events2.len(), 1, "Expected exactly one TurnUpdated event");

        match &events2[0] {
            WatcherEvent::TurnUpdated(turn) => {
                assert_eq!(turn.id, "u2");
                assert_eq!(turn.tool_uses.len(), 1);
                let result = turn.tool_uses[0]
                    .result
                    .as_ref()
                    .expect("result should be merged cross-poll");
                assert_eq!(result.content, "fn main() {}");
                assert!(!result.is_error);
            }
            other => panic!("Expected TurnUpdated, got {:?}", other),
        }
    }

    #[test]
    fn cross_poll_multiple_results_single_turn_updated() {
        let temp = tempfile::TempDir::new().unwrap();
        let claude_dir = temp.path().join(".claude");

        // Phase 1: assistant with two tool_uses
        let entries_phase1 = vec![
            r#"{"uuid":"u1","type":"user","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"Check files"}}"#,
            r#"{"uuid":"u2","type":"assistant","timestamp":"2024-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"Checking..."},{"type":"tool_use","id":"t1","name":"Read","input":{"path":"a.rs"}},{"type":"tool_use","id":"t2","name":"Read","input":{"path":"b.rs"}}]}}"#,
        ];
        create_test_jsonl(&claude_dir, "s1", &entries_phase1);

        let mut watcher = MergingWatcher::new(
            test_manager(&claude_dir),
            "/test/project".into(),
            "s1".into(),
        );

        let events1 = toolpath_convo::ConversationWatcher::poll(&mut watcher).unwrap();
        assert_eq!(events1.len(), 2);

        // Phase 2: both results arrive in one entry
        use std::io::Write;
        let project_dir = claude_dir.join("projects/-test-project");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(project_dir.join("s1.jsonl"))
            .unwrap();
        writeln!(file, r#"{{"uuid":"u3","type":"user","timestamp":"2024-01-01T00:00:02Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t1","content":"file a","is_error":false}},{{"type":"tool_result","tool_use_id":"t2","content":"file b","is_error":false}}]}}}}"#).unwrap();

        let events2 = toolpath_convo::ConversationWatcher::poll(&mut watcher).unwrap();
        // Should get exactly one TurnUpdated (both results merged into same turn)
        assert_eq!(events2.len(), 1);

        if let WatcherEvent::TurnUpdated(turn) = &events2[0] {
            assert_eq!(turn.id, "u2");
            assert_eq!(turn.tool_uses[0].result.as_ref().unwrap().content, "file a");
            assert_eq!(turn.tool_uses[1].result.as_ref().unwrap().content, "file b");
        } else {
            panic!("Expected TurnUpdated");
        }
    }

    #[test]
    fn progress_entries_emitted_as_progress() {
        let temp = tempfile::TempDir::new().unwrap();
        let claude_dir = temp.path().join(".claude");

        let entries = vec![
            r#"{"uuid":"u1","type":"user","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"Hello"}}"#,
            r#"{"uuid":"p1","type":"progress","timestamp":"2024-01-01T00:00:01Z"}"#,
        ];
        create_test_jsonl(&claude_dir, "s1", &entries);

        let mut watcher = MergingWatcher::new(
            test_manager(&claude_dir),
            "/test/project".into(),
            "s1".into(),
        );

        let events = toolpath_convo::ConversationWatcher::poll(&mut watcher).unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], WatcherEvent::Turn(_)));
        assert!(matches!(&events[1], WatcherEvent::Progress { .. }));
    }

    #[test]
    fn progress_entries_preserve_extra_data() {
        let temp = tempfile::TempDir::new().unwrap();
        let claude_dir = temp.path().join(".claude");

        // agent_progress entry with extra fields (agentId, data, etc.)
        let entries = vec![
            r#"{"uuid":"u1","type":"user","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"Hello"}}"#,
            r#"{"uuid":"p1","type":"agent_progress","timestamp":"2024-01-01T00:00:01Z","agentId":"agent-123","data":{"type":"agent_progress","agentId":"agent-123","message":{"role":"assistant","content":"Working..."}}}"#,
        ];
        create_test_jsonl(&claude_dir, "s1", &entries);

        let mut watcher = MergingWatcher::new(
            test_manager(&claude_dir),
            "/test/project".into(),
            "s1".into(),
        );

        let events = toolpath_convo::ConversationWatcher::poll(&mut watcher).unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], WatcherEvent::Turn(_)));

        if let WatcherEvent::Progress { kind, data } = &events[1] {
            assert_eq!(kind, "agent_progress");
            assert_eq!(data["uuid"], "p1");
            assert_eq!(data["timestamp"], "2024-01-01T00:00:01Z");
            // Extra fields should be preserved
            assert_eq!(data["agentId"], "agent-123");
            assert_eq!(data["data"]["type"], "agent_progress");
            assert_eq!(data["data"]["agentId"], "agent-123");
            assert_eq!(data["data"]["message"]["content"], "Working...");
        } else {
            panic!("Expected Progress, got {:?}", events[1]);
        }
    }

    #[test]
    fn seen_count_tracks_entries() {
        let temp = tempfile::TempDir::new().unwrap();
        let claude_dir = temp.path().join(".claude");

        let entries = vec![
            r#"{"uuid":"u1","type":"user","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"Hello"}}"#,
            r#"{"uuid":"u2","type":"assistant","timestamp":"2024-01-01T00:00:01Z","message":{"role":"assistant","content":"Hi"}}"#,
        ];
        create_test_jsonl(&claude_dir, "s1", &entries);

        let mut watcher = MergingWatcher::new(
            test_manager(&claude_dir),
            "/test/project".into(),
            "s1".into(),
        );

        assert_eq!(toolpath_convo::ConversationWatcher::seen_count(&watcher), 0);
        let _ = toolpath_convo::ConversationWatcher::poll(&mut watcher).unwrap();
        assert_eq!(toolpath_convo::ConversationWatcher::seen_count(&watcher), 2);
    }

    #[test]
    fn accessors_delegate_to_inner() {
        let temp = tempfile::TempDir::new().unwrap();
        let claude_dir = temp.path().join(".claude");
        create_test_jsonl(&claude_dir, "s1", &[]);

        let watcher = MergingWatcher::new(
            test_manager(&claude_dir),
            "/test/project".into(),
            "s1".into(),
        );

        assert_eq!(watcher.project(), "/test/project");
        assert_eq!(watcher.session_id(), "s1");
    }

    #[test]
    fn error_result_preserved_cross_poll() {
        let temp = tempfile::TempDir::new().unwrap();
        let claude_dir = temp.path().join(".claude");

        let entries_phase1 = vec![
            r#"{"uuid":"u1","type":"user","timestamp":"2024-01-01T00:00:00Z","message":{"role":"user","content":"Read"}}"#,
            r#"{"uuid":"u2","type":"assistant","timestamp":"2024-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"Reading..."},{"type":"tool_use","id":"t1","name":"Read","input":{"path":"missing.rs"}}]}}"#,
        ];
        create_test_jsonl(&claude_dir, "s1", &entries_phase1);

        let mut watcher = MergingWatcher::new(
            test_manager(&claude_dir),
            "/test/project".into(),
            "s1".into(),
        );
        let _ = toolpath_convo::ConversationWatcher::poll(&mut watcher).unwrap();

        // Append error result
        use std::io::Write;
        let project_dir = claude_dir.join("projects/-test-project");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(project_dir.join("s1.jsonl"))
            .unwrap();
        writeln!(file, r#"{{"uuid":"u3","type":"user","timestamp":"2024-01-01T00:00:02Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"t1","content":"File not found","is_error":true}}]}}}}"#).unwrap();

        let events2 = toolpath_convo::ConversationWatcher::poll(&mut watcher).unwrap();
        assert_eq!(events2.len(), 1);

        if let WatcherEvent::TurnUpdated(turn) = &events2[0] {
            let result = turn.tool_uses[0].result.as_ref().unwrap();
            assert!(result.is_error);
            assert_eq!(result.content, "File not found");
        } else {
            panic!("Expected TurnUpdated");
        }
    }
}
