use claude_convo::{ContentPart, MessageContent, MessageRole};
use tracing::warn;

use crate::repository;
use crate::ws;

/// Format a conversation entry for the frontend.
/// Returns a formatted JSON value for ALL entries - no filtering.
/// Unknown or unrecognized entries are included with type "unknown".
pub fn format_entry(entry: &claude_convo::ConversationEntry) -> serde_json::Value {
    // Helper to normalize whitespace (preserves indentation)
    fn normalize_whitespace(s: &str) -> String {
        let lines: Vec<&str> = s.lines().collect();
        let mut result = Vec::new();
        let mut last_was_empty = true;
        for line in lines {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                if !last_was_empty {
                    result.push("");
                    last_was_empty = true;
                }
            } else {
                result.push(line);
                last_was_empty = false;
            }
        }
        while result.last().map(|s| s.trim().is_empty()).unwrap_or(false) {
            result.pop();
        }
        result.join("\n")
    }

    // Check if we have a message with User or Assistant role
    if let Some(msg) = &entry.message {
        match msg.role {
            MessageRole::User | MessageRole::Assistant => {
                let content_text = match &msg.content {
                    Some(MessageContent::Text(text)) => text.clone(),
                    Some(MessageContent::Parts(parts)) => parts
                        .iter()
                        .filter_map(|part| match part {
                            ContentPart::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                    None => String::new(),
                };

                let normalized = normalize_whitespace(&content_text);

                // Extract thinking content (extended thinking blocks from Claude)
                let thinking_text: Option<String> =
                    if let Some(MessageContent::Parts(parts)) = &msg.content {
                        let thinking_parts: Vec<String> = parts
                            .iter()
                            .filter_map(|p| match p {
                                ContentPart::Thinking { thinking, .. } => Some(thinking.clone()),
                                _ => None,
                            })
                            .collect();
                        if thinking_parts.is_empty() {
                            None
                        } else {
                            Some(thinking_parts.join("\n\n"))
                        }
                    } else {
                        None
                    };

                let tool_names: Vec<String> =
                    if let Some(MessageContent::Parts(parts)) = &msg.content {
                        parts
                            .iter()
                            .filter_map(|p| match p {
                                ContentPart::ToolUse { name, .. } => Some(name.clone()),
                                _ => None,
                            })
                            .collect()
                    } else {
                        vec![]
                    };

                let mut json = serde_json::json!({
                    "uuid": entry.uuid,
                    "role": format!("{:?}", msg.role),
                    "content": normalized,
                    "timestamp": entry.timestamp.clone(),
                    "tools": tool_names
                });

                if let Some(thinking) = thinking_text {
                    if let Some(obj) = json.as_object_mut() {
                        obj.insert("thinking".to_string(), serde_json::Value::String(thinking));
                    }
                }

                return json;
            }
            MessageRole::System => {
                let content_text = match &msg.content {
                    Some(MessageContent::Text(text)) => text.clone(),
                    Some(MessageContent::Parts(parts)) => parts
                        .iter()
                        .filter_map(|part| match part {
                            ContentPart::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                    None => String::new(),
                };

                let empty_tools: Vec<String> = vec![];
                return serde_json::json!({
                    "uuid": entry.uuid,
                    "role": "System",
                    "content": normalize_whitespace(&content_text),
                    "timestamp": entry.timestamp.clone(),
                    "tools": empty_tools,
                    "entry_type": entry.entry_type.clone()
                });
            }
        }
    }

    // Handle progress entries specially
    if entry.entry_type == "progress" {
        if let Some(data) = entry.extra.get("data") {
            let progress_type = data.get("type").and_then(|t| t.as_str()).unwrap_or("");

            match progress_type {
                "hook_progress" => {
                    let hook_name = data
                        .get("hookName")
                        .and_then(|h| h.as_str())
                        .unwrap_or("hook");
                    let hook_event = data.get("hookEvent").and_then(|e| e.as_str()).unwrap_or("");

                    let empty_tools: Vec<String> = vec![];
                    return serde_json::json!({
                        "uuid": entry.uuid,
                        "role": "Progress",
                        "content": hook_name,
                        "timestamp": entry.timestamp.clone(),
                        "tools": empty_tools,
                        "entry_type": "progress",
                        "progress_type": "hook",
                        "hook_event": hook_event
                    });
                }
                "agent_progress" => {
                    let agent_id = data
                        .get("agentId")
                        .and_then(|a| a.as_str())
                        .unwrap_or("unknown");
                    let prompt = data.get("prompt").and_then(|p| p.as_str()).unwrap_or("");

                    let mut content = String::new();
                    let mut msg_role = "agent";

                    if let Some(message_data) = data.get("message") {
                        let msg_type = message_data
                            .get("type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        msg_role = match msg_type {
                            "user" => "agent_user",
                            "assistant" => "agent_assistant",
                            _ => "agent",
                        };

                        if let Some(inner_msg) = message_data.get("message") {
                            if let Some(inner_content) = inner_msg.get("content") {
                                if let Some(text) = inner_content.as_str() {
                                    content = text.to_string();
                                } else if let Some(parts) = inner_content.as_array() {
                                    let texts: Vec<String> = parts
                                        .iter()
                                        .filter_map(|p| {
                                            if p.get("type").and_then(|t| t.as_str())
                                                == Some("text")
                                            {
                                                p.get("text")
                                                    .and_then(|t| t.as_str())
                                                    .map(|s| s.to_string())
                                            } else if p.get("type").and_then(|t| t.as_str())
                                                == Some("tool_use")
                                            {
                                                let name = p
                                                    .get("name")
                                                    .and_then(|n| n.as_str())
                                                    .unwrap_or("tool");
                                                Some(format!("[{}]", name))
                                            } else if p.get("type").and_then(|t| t.as_str())
                                                == Some("tool_result")
                                            {
                                                let tool_content = p
                                                    .get("content")
                                                    .and_then(|c| c.as_str())
                                                    .unwrap_or("");
                                                let preview = if tool_content.len() > 100 {
                                                    format!("{}...", &tool_content[..100])
                                                } else {
                                                    tool_content.to_string()
                                                };
                                                Some(format!("[result: {}]", preview))
                                            } else {
                                                None
                                            }
                                        })
                                        .collect();
                                    content = texts.join(" ");
                                }
                            }
                        }

                        if let Some(tool_result) =
                            message_data.get("toolUseResult").and_then(|t| t.as_str())
                        {
                            if content.is_empty() {
                                content = tool_result.to_string();
                            }
                        }
                    }

                    if content.len() > 500 {
                        content = format!("{}...", &content[..500]);
                    }

                    let empty_tools: Vec<String> = vec![];
                    return serde_json::json!({
                        "uuid": entry.uuid,
                        "role": "AgentProgress",
                        "content": content,
                        "timestamp": entry.timestamp.clone(),
                        "tools": empty_tools,
                        "entry_type": "progress",
                        "agent_id": agent_id,
                        "agent_prompt": if prompt.len() > 200 {
                            format!("{}...", &prompt[..200])
                        } else {
                            prompt.to_string()
                        },
                        "agent_msg_role": msg_role
                    });
                }
                _ => {
                    // Unknown progress type - fall through to generic handling
                }
            }
        }
    }

    // For entries without a message or with unknown structure
    let empty_tools: Vec<String> = vec![];
    let mut json = serde_json::json!({
        "uuid": entry.uuid,
        "role": "Unknown",
        "content": "",
        "timestamp": entry.timestamp.clone(),
        "tools": empty_tools,
        "entry_type": entry.entry_type.clone(),
        "unknown": true
    });

    let obj = json.as_object_mut().unwrap();

    if let Some(tool_result) = &entry.tool_use_result {
        obj.insert("tool_result".to_string(), tool_result.clone());
        if let Some(content) = tool_result.get("content").and_then(|c| c.as_str()) {
            let preview = if content.len() > 200 {
                format!("{}...", &content[..200])
            } else {
                content.to_string()
            };
            obj.insert(
                "content".to_string(),
                serde_json::Value::String(format!("[Tool Result] {}", preview)),
            );
        }
    }

    if !entry.extra.is_empty() {
        obj.insert("extra".to_string(), serde_json::json!(entry.extra));
    }

    json
}

/// Format a conversation entry with user attribution.
///
/// Attribution sources (checked in order):
/// 1. In-process pending attribution queue (content-matched, for live sessions)
/// 2. DB-based InputAttribution (for historical entries / conversation reload)
/// 3. "Terminal" fallback (auth enabled but no attribution — external input)
///
/// If no auth/repository configured, leaves unattributed (frontend shows "You").
pub async fn format_entry_with_attribution(
    entry: &claude_convo::ConversationEntry,
    instance_id: &str,
    repo: Option<&repository::ConversationRepository>,
    state_manager: Option<&std::sync::Arc<ws::GlobalStateManager>>,
) -> serde_json::Value {
    let mut json = format_entry(entry);

    let is_user = entry
        .message
        .as_ref()
        .is_some_and(|m| m.role == MessageRole::User);

    if !is_user {
        return json;
    }

    let content_text = entry.message.as_ref().and_then(|msg| match &msg.content {
        Some(MessageContent::Text(text)) => Some(text.as_str()),
        Some(MessageContent::Parts(parts)) => parts.iter().find_map(|p| match p {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        }),
        None => None,
    });

    // 1. Try in-process content-matched attribution (fast path, no DB)
    if let (Some(content), Some(sm)) = (content_text, state_manager) {
        if let Some(attr) = sm.consume_pending_attribution(instance_id, content).await {
            if let Some(obj) = json.as_object_mut() {
                obj.insert(
                    "attributed_to".to_string(),
                    serde_json::json!({
                        "user_id": attr.user_id,
                        "display_name": attr.display_name,
                    }),
                );
                if let Some(task_id) = attr.task_id {
                    obj.insert("task_id".to_string(), serde_json::json!(task_id));
                }
            }
            return json;
        }
    }

    // 2. Fall back to DB-based attribution
    if let Some(repo) = repo {
        let unix_ts = chrono::DateTime::parse_from_rfc3339(&entry.timestamp)
            .map(|dt| dt.timestamp())
            .unwrap_or(0);

        match repo
            .get_or_correlate_attribution(instance_id, &entry.uuid, unix_ts, content_text)
            .await
        {
            Ok(Some(attr)) => {
                if let Some(obj) = json.as_object_mut() {
                    obj.insert(
                        "attributed_to".to_string(),
                        serde_json::json!({
                            "user_id": attr.user_id,
                            "display_name": attr.display_name,
                        }),
                    );
                    if let Some(task_id) = attr.task_id {
                        obj.insert("task_id".to_string(), serde_json::json!(task_id));
                    }
                }
                return json;
            }
            Ok(None) => {
                if let Some(obj) = json.as_object_mut() {
                    obj.insert(
                        "attributed_to".to_string(),
                        serde_json::json!({
                            "user_id": "terminal",
                            "display_name": "Terminal",
                        }),
                    );
                }
            }
            Err(e) => {
                warn!(
                    "Failed to look up attribution for entry {}: {}",
                    entry.uuid, e
                );
            }
        }
    }

    json
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_convo::{ContentPart, ConversationEntry, Message, MessageContent, MessageRole};
    use serde_json::json;
    use std::collections::HashMap;

    fn make_entry(uuid: &str, entry_type: &str, message: Option<Message>) -> ConversationEntry {
        ConversationEntry {
            uuid: uuid.to_string(),
            parent_uuid: None,
            is_sidechain: false,
            entry_type: entry_type.to_string(),
            session_id: Some("session".to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            cwd: None,
            git_branch: None,
            version: None,
            user_type: None,
            request_id: None,
            tool_use_result: None,
            snapshot: None,
            message_id: None,
            message,
            extra: HashMap::new(),
        }
    }

    fn make_msg(role: MessageRole, content: Option<MessageContent>) -> Message {
        Message {
            role,
            content,
            model: None,
            id: None,
            message_type: None,
            stop_reason: None,
            stop_sequence: None,
            usage: None,
        }
    }

    // ── User message with simple text ─────────────────────────────

    #[test]
    fn user_message_simple_text() {
        let entry = make_entry(
            "u1",
            "user",
            Some(make_msg(
                MessageRole::User,
                Some(MessageContent::Text("Hello world".into())),
            )),
        );
        let result = format_entry(&entry);

        assert_eq!(result["uuid"], "u1");
        assert_eq!(result["role"], "User");
        assert_eq!(result["content"], "Hello world");
        assert_eq!(result["tools"], json!([]));
        assert!(result.get("thinking").is_none());
    }

    // ── User message with parts ───────────────────────────────────

    #[test]
    fn user_message_parts_extracts_text_only() {
        let entry = make_entry(
            "u2",
            "user",
            Some(make_msg(
                MessageRole::User,
                Some(MessageContent::Parts(vec![
                    ContentPart::Text {
                        text: "first".into(),
                    },
                    ContentPart::Text {
                        text: "second".into(),
                    },
                ])),
            )),
        );
        let result = format_entry(&entry);

        assert_eq!(result["content"], "first second");
        assert_eq!(result["tools"], json!([]));
    }

    // ── Assistant message with thinking + tools ───────────────────

    #[test]
    fn assistant_message_with_thinking_and_tools() {
        let entry = make_entry(
            "a1",
            "assistant",
            Some(make_msg(
                MessageRole::Assistant,
                Some(MessageContent::Parts(vec![
                    ContentPart::Thinking {
                        thinking: "Let me think...".into(),
                        signature: None,
                    },
                    ContentPart::Text {
                        text: "Here is my answer".into(),
                    },
                    ContentPart::ToolUse {
                        id: "tu1".into(),
                        name: "Read".into(),
                        input: json!({"path": "/foo"}),
                    },
                ])),
            )),
        );
        let result = format_entry(&entry);

        assert_eq!(result["role"], "Assistant");
        assert_eq!(result["content"], "Here is my answer");
        assert_eq!(result["thinking"], "Let me think...");
        assert_eq!(result["tools"], json!(["Read"]));
    }

    // ── Assistant message with multiple thinking blocks ───────────

    #[test]
    fn assistant_multiple_thinking_blocks_joined() {
        let entry = make_entry(
            "a2",
            "assistant",
            Some(make_msg(
                MessageRole::Assistant,
                Some(MessageContent::Parts(vec![
                    ContentPart::Thinking {
                        thinking: "First thought".into(),
                        signature: Some("sig1".into()),
                    },
                    ContentPart::Thinking {
                        thinking: "Second thought".into(),
                        signature: None,
                    },
                    ContentPart::Text {
                        text: "output".into(),
                    },
                ])),
            )),
        );
        let result = format_entry(&entry);

        assert_eq!(result["thinking"], "First thought\n\nSecond thought");
    }

    // ── Assistant with no thinking → no thinking field ────────────

    #[test]
    fn assistant_no_thinking_omits_field() {
        let entry = make_entry(
            "a3",
            "assistant",
            Some(make_msg(
                MessageRole::Assistant,
                Some(MessageContent::Parts(vec![ContentPart::Text {
                    text: "just text".into(),
                }])),
            )),
        );
        let result = format_entry(&entry);

        assert!(result.get("thinking").is_none());
    }

    // ── System message includes entry_type ────────────────────────

    #[test]
    fn system_message_includes_entry_type() {
        let entry = make_entry(
            "s1",
            "system",
            Some(make_msg(
                MessageRole::System,
                Some(MessageContent::Text("System prompt".into())),
            )),
        );
        let result = format_entry(&entry);

        assert_eq!(result["role"], "System");
        assert_eq!(result["content"], "System prompt");
        assert_eq!(result["entry_type"], "system");
        assert_eq!(result["tools"], json!([]));
    }

    // ── Message with None content → empty string ──────────────────

    #[test]
    fn message_with_none_content() {
        let entry = make_entry("n1", "user", Some(make_msg(MessageRole::User, None)));
        let result = format_entry(&entry);

        assert_eq!(result["content"], "");
    }

    // ── Whitespace normalization ──────────────────────────────────

    #[test]
    fn whitespace_normalization_collapses_blanks() {
        let text = "Hello\n\n\n\nWorld\n\n\n";
        let entry = make_entry(
            "w1",
            "user",
            Some(make_msg(
                MessageRole::User,
                Some(MessageContent::Text(text.into())),
            )),
        );
        let result = format_entry(&entry);

        // Consecutive blank lines collapsed to one, trailing blanks stripped
        assert_eq!(result["content"], "Hello\n\nWorld");
    }

    #[test]
    fn whitespace_preserves_indentation() {
        let text = "def foo():\n    return 42";
        let entry = make_entry(
            "w2",
            "user",
            Some(make_msg(
                MessageRole::User,
                Some(MessageContent::Text(text.into())),
            )),
        );
        let result = format_entry(&entry);

        assert_eq!(result["content"], "def foo():\n    return 42");
    }

    // ── Progress: hook_progress ───────────────────────────────────

    #[test]
    fn progress_hook() {
        let mut extra = HashMap::new();
        extra.insert(
            "data".to_string(),
            json!({
                "type": "hook_progress",
                "hookName": "pre-commit",
                "hookEvent": "start"
            }),
        );

        let mut entry = make_entry("p1", "progress", None);
        entry.extra = extra;

        let result = format_entry(&entry);

        assert_eq!(result["role"], "Progress");
        assert_eq!(result["content"], "pre-commit");
        assert_eq!(result["entry_type"], "progress");
        assert_eq!(result["progress_type"], "hook");
        assert_eq!(result["hook_event"], "start");
    }

    // ── Progress: agent_progress with text message ────────────────

    #[test]
    fn progress_agent_with_text_content() {
        let mut extra = HashMap::new();
        extra.insert(
            "data".to_string(),
            json!({
                "type": "agent_progress",
                "agentId": "agent-42",
                "prompt": "Do the thing",
                "message": {
                    "type": "assistant",
                    "message": {
                        "content": "I'll handle it"
                    }
                }
            }),
        );

        let mut entry = make_entry("p2", "progress", None);
        entry.extra = extra;

        let result = format_entry(&entry);

        assert_eq!(result["role"], "AgentProgress");
        assert_eq!(result["content"], "I'll handle it");
        assert_eq!(result["agent_id"], "agent-42");
        assert_eq!(result["agent_prompt"], "Do the thing");
        assert_eq!(result["agent_msg_role"], "agent_assistant");
    }

    // ── Progress: agent_progress with array content (tool_use + text)

    #[test]
    fn progress_agent_with_parts_content() {
        let mut extra = HashMap::new();
        extra.insert(
            "data".to_string(),
            json!({
                "type": "agent_progress",
                "agentId": "a1",
                "prompt": "test",
                "message": {
                    "type": "user",
                    "message": {
                        "content": [
                            {"type": "text", "text": "hello"},
                            {"type": "tool_use", "name": "Bash"},
                            {"type": "tool_result", "content": "ok done"}
                        ]
                    }
                }
            }),
        );

        let mut entry = make_entry("p3", "progress", None);
        entry.extra = extra;

        let result = format_entry(&entry);

        assert_eq!(result["content"], "hello [Bash] [result: ok done]");
        assert_eq!(result["agent_msg_role"], "agent_user");
    }

    // ── Progress: agent content truncated at 500 chars ────────────

    #[test]
    fn progress_agent_content_truncation() {
        let long_content = "x".repeat(600);
        let mut extra = HashMap::new();
        extra.insert(
            "data".to_string(),
            json!({
                "type": "agent_progress",
                "agentId": "a2",
                "prompt": "",
                "message": {
                    "type": "assistant",
                    "message": {
                        "content": long_content
                    }
                }
            }),
        );

        let mut entry = make_entry("p4", "progress", None);
        entry.extra = extra;

        let result = format_entry(&entry);

        let content = result["content"].as_str().unwrap();
        assert!(content.ends_with("..."));
        assert_eq!(content.len(), 503); // 500 + "..."
    }

    // ── Progress: agent prompt truncated at 200 chars ─────────────

    #[test]
    fn progress_agent_prompt_truncation() {
        let long_prompt = "p".repeat(300);
        let mut extra = HashMap::new();
        extra.insert(
            "data".to_string(),
            json!({
                "type": "agent_progress",
                "agentId": "a3",
                "prompt": long_prompt,
                "message": {
                    "type": "assistant",
                    "message": { "content": "hi" }
                }
            }),
        );

        let mut entry = make_entry("p5", "progress", None);
        entry.extra = extra;

        let result = format_entry(&entry);

        let prompt = result["agent_prompt"].as_str().unwrap();
        assert!(prompt.ends_with("..."));
        assert_eq!(prompt.len(), 203); // 200 + "..."
    }

    // ── Progress: agent with toolUseResult fallback ───────────────

    #[test]
    fn progress_agent_tool_use_result_fallback() {
        let mut extra = HashMap::new();
        extra.insert(
            "data".to_string(),
            json!({
                "type": "agent_progress",
                "agentId": "a4",
                "prompt": "",
                "message": {
                    "type": "assistant",
                    "message": {},
                    "toolUseResult": "tool output here"
                }
            }),
        );

        let mut entry = make_entry("p6", "progress", None);
        entry.extra = extra;

        let result = format_entry(&entry);

        assert_eq!(result["content"], "tool output here");
    }

    // ── Unknown entry with no message ─────────────────────────────

    #[test]
    fn unknown_entry_no_message() {
        let entry = make_entry("x1", "some_unknown_type", None);
        let result = format_entry(&entry);

        assert_eq!(result["role"], "Unknown");
        assert_eq!(result["content"], "");
        assert_eq!(result["entry_type"], "some_unknown_type");
        assert_eq!(result["unknown"], true);
    }

    // ── Unknown entry with tool_use_result ────────────────────────

    #[test]
    fn unknown_entry_with_tool_result() {
        let mut entry = make_entry("x2", "tool_result", None);
        entry.tool_use_result = Some(json!({
            "content": "File written successfully"
        }));

        let result = format_entry(&entry);

        assert_eq!(result["role"], "Unknown");
        assert_eq!(result["content"], "[Tool Result] File written successfully");
        assert!(result.get("tool_result").is_some());
    }

    // ── Unknown entry with long tool_use_result truncated ─────────

    #[test]
    fn unknown_entry_tool_result_truncation() {
        let long = "y".repeat(300);
        let mut entry = make_entry("x3", "tool_result", None);
        entry.tool_use_result = Some(json!({ "content": long }));

        let result = format_entry(&entry);

        let content = result["content"].as_str().unwrap();
        assert!(content.starts_with("[Tool Result] "));
        assert!(content.ends_with("..."));
        // "[Tool Result] " (14 chars) + 200 chars + "..." (3 chars) = 217
        assert_eq!(content.len(), 217);
    }

    // ── Unknown entry with extra data ─────────────────────────────

    #[test]
    fn unknown_entry_includes_extra() {
        let mut entry = make_entry("x4", "custom", None);
        entry
            .extra
            .insert("data".to_string(), json!({"key": "value"}));

        let result = format_entry(&entry);

        assert!(result.get("extra").is_some());
        assert_eq!(result["extra"]["data"]["key"], "value");
    }

    // ── Unknown progress type falls through to generic ────────────

    #[test]
    fn unknown_progress_type_falls_through() {
        let mut entry = make_entry("p7", "progress", None);
        entry.extra.insert(
            "data".to_string(),
            json!({"type": "some_new_progress_type"}),
        );

        let result = format_entry(&entry);

        assert_eq!(result["role"], "Unknown");
        assert_eq!(result["entry_type"], "progress");
        assert_eq!(result["unknown"], true);
    }

    // ── Progress entry with no data falls through ─────────────────

    #[test]
    fn progress_no_data_falls_through() {
        let entry = make_entry("p8", "progress", None);

        let result = format_entry(&entry);

        assert_eq!(result["role"], "Unknown");
        assert_eq!(result["entry_type"], "progress");
    }

    // ── Agent progress: tool_result content preview truncation ────

    #[test]
    fn progress_agent_tool_result_part_truncated() {
        let long_result = "z".repeat(200);
        let mut extra = HashMap::new();
        extra.insert(
            "data".to_string(),
            json!({
                "type": "agent_progress",
                "agentId": "a5",
                "prompt": "",
                "message": {
                    "type": "assistant",
                    "message": {
                        "content": [
                            {"type": "tool_result", "content": long_result}
                        ]
                    }
                }
            }),
        );

        let mut entry = make_entry("p9", "progress", None);
        entry.extra = extra;

        let result = format_entry(&entry);

        let content = result["content"].as_str().unwrap();
        // tool_result content > 100 chars gets truncated to 100 + "..."
        assert!(content.contains("..."));
        assert!(content.starts_with("[result: "));
    }
}
