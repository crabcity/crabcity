use claude_convo::{ContentPart, MessageContent, MessageRole};
use tracing::warn;

use std::sync::Arc;

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
    repo: Option<&Arc<repository::ConversationRepository>>,
    state_manager: Option<&Arc<ws::GlobalStateManager>>,
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
            // Persist entry_uuid link to DB so historical queries find it
            if let Some(repo) = repo {
                let repo = Arc::clone(repo);
                let instance_id = instance_id.to_string();
                let entry_uuid = entry.uuid.clone();
                let entry_content = content.to_string();
                let unix_ts = chrono::DateTime::parse_from_rfc3339(&entry.timestamp)
                    .map(|dt| dt.timestamp())
                    .unwrap_or(0);
                tokio::spawn(async move {
                    let _ = repo
                        .correlate_attribution(
                            &instance_id,
                            &entry_uuid,
                            unix_ts,
                            Some(&entry_content),
                        )
                        .await;
                });
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

/// Tests for `format_entry_with_attribution` — the full attribution pipeline.
///
/// These exercise the three-tier fallback (in-process queue → DB → Terminal)
/// and the DB write-back that was added to persist entry_uuid after a queue hit.
#[cfg(test)]
mod attribution_integration_tests {
    use super::*;
    use claude_convo::{ConversationEntry, Message, MessageContent, MessageRole};
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::models::{InputAttribution, User};
    use crate::repository::test_helpers::test_repository;
    use crate::ws::{GlobalStateManager, create_state_broadcast};

    fn make_user(id: &str, name: &str) -> User {
        let now = chrono::Utc::now().timestamp();
        User {
            id: id.to_string(),
            username: name.to_string(),
            display_name: name.to_string(),
            password_hash: "hashed".to_string(),
            is_admin: false,
            is_disabled: false,
            created_at: now,
            updated_at: now,
        }
    }

    fn make_user_entry(uuid: &str, content: &str, ts: &str) -> ConversationEntry {
        ConversationEntry {
            uuid: uuid.to_string(),
            parent_uuid: None,
            is_sidechain: false,
            entry_type: "user".to_string(),
            session_id: Some("sess".to_string()),
            timestamp: ts.to_string(),
            cwd: None,
            git_branch: None,
            version: None,
            user_type: None,
            request_id: None,
            tool_use_result: None,
            snapshot: None,
            message_id: None,
            message: Some(Message {
                role: MessageRole::User,
                content: Some(MessageContent::Text(content.to_string())),
                model: None,
                id: None,
                message_type: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
            }),
            extra: HashMap::new(),
        }
    }

    fn make_assistant_entry(uuid: &str, content: &str, ts: &str) -> ConversationEntry {
        ConversationEntry {
            uuid: uuid.to_string(),
            parent_uuid: None,
            is_sidechain: false,
            entry_type: "assistant".to_string(),
            session_id: Some("sess".to_string()),
            timestamp: ts.to_string(),
            cwd: None,
            git_branch: None,
            version: None,
            user_type: None,
            request_id: None,
            tool_use_result: None,
            snapshot: None,
            message_id: None,
            message: Some(Message {
                role: MessageRole::Assistant,
                content: Some(MessageContent::Text(content.to_string())),
                model: None,
                id: None,
                message_type: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
            }),
            extra: HashMap::new(),
        }
    }

    /// Ensure user exists in DB (idempotent).
    async fn ensure_user(repo: &crate::repository::ConversationRepository, id: &str, name: &str) {
        // Ignore error if already exists
        let _ = repo.create_user(&make_user(id, name)).await;
    }

    /// Return an RFC-3339 timestamp string for "now", suitable for conversation entries
    /// whose DB rows are written with `Utc::now().timestamp()`.
    fn now_rfc3339() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    /// Simulate what handler.rs does: push to in-process queue AND insert DB row.
    async fn simulate_handler_input(
        sm: &GlobalStateManager,
        repo: &crate::repository::ConversationRepository,
        instance_id: &str,
        user_id: &str,
        display_name: &str,
        raw_input: &str,
        task_id: Option<i64>,
    ) {
        let trimmed = raw_input.trim();
        if trimmed.is_empty() || trimmed == "\r" || trimmed == "\n" {
            return;
        }
        // In-process queue (sync in handler)
        sm.push_pending_attribution(
            instance_id,
            user_id.to_string(),
            display_name.to_string(),
            trimmed,
            task_id,
        )
        .await;
        // DB row (spawned in handler, but we await here for determinism)
        ensure_user(repo, user_id, display_name).await;
        let ts = chrono::Utc::now().timestamp();
        repo.record_input_attribution(&InputAttribution {
            id: None,
            instance_id: instance_id.to_string(),
            user_id: user_id.to_string(),
            display_name: display_name.to_string(),
            timestamp: ts,
            entry_uuid: None,
            content_preview: Some(trimmed.chars().take(100).collect()),
            task_id,
        })
        .await
        .unwrap();
    }

    // ── Tier-1: in-process queue match ──────────────────────────────

    #[tokio::test]
    async fn queue_match_returns_attribution() {
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        sm.push_pending_attribution("i1", "u1".into(), "Alice".into(), "fix the bug", None)
            .await;

        let entry = make_user_entry("e1", "fix the bug", "2024-06-01T12:00:00Z");
        let result =
            format_entry_with_attribution(&entry, "i1", None, Some(&sm)).await;

        assert_eq!(result["attributed_to"]["user_id"], "u1");
        assert_eq!(result["attributed_to"]["display_name"], "Alice");
    }

    // ── Tier-2: DB fallback (uncorrelated row) ──────────────────────

    #[tokio::test]
    async fn db_fallback_correlates_unclaimed_row() {
        let repo = Arc::new(test_repository().await);
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));

        ensure_user(&repo, "u1", "Alice").await;
        let ts: i64 = chrono::DateTime::parse_from_rfc3339("2024-06-01T12:00:00Z")
            .unwrap()
            .timestamp();
        repo.record_input_attribution(&InputAttribution {
            id: None,
            instance_id: "i1".into(),
            user_id: "u1".into(),
            display_name: "Alice".into(),
            timestamp: ts,
            entry_uuid: None,
            content_preview: Some("fix the bug".into()),
            task_id: None,
        })
        .await
        .unwrap();

        // Queue is empty — should fall through to DB
        let entry = make_user_entry("e1", "fix the bug", "2024-06-01T12:00:00Z");
        let result =
            format_entry_with_attribution(&entry, "i1", Some(&repo), Some(&sm)).await;

        assert_eq!(result["attributed_to"]["user_id"], "u1");
    }

    // ── Tier-3: Terminal fallback ───────────────────────────────────

    #[tokio::test]
    async fn terminal_fallback_when_nothing_matches() {
        let repo = Arc::new(test_repository().await);
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));

        let entry = make_user_entry("e1", "fix the bug", "2024-06-01T12:00:00Z");
        let result =
            format_entry_with_attribution(&entry, "i1", Some(&repo), Some(&sm)).await;

        assert_eq!(result["attributed_to"]["user_id"], "terminal");
        assert_eq!(result["attributed_to"]["display_name"], "Terminal");
    }

    // ── No repo/state → no attributed_to field at all ───────────────

    #[tokio::test]
    async fn no_repo_no_state_manager_leaves_unattributed() {
        let entry = make_user_entry("e1", "hello", "2024-06-01T12:00:00Z");
        let result =
            format_entry_with_attribution(&entry, "i1", None, None).await;

        assert!(
            result.get("attributed_to").is_none(),
            "Without repo or state_manager, no attribution should be set"
        );
    }

    // ── Assistant entries are never attributed ───────────────────────

    #[tokio::test]
    async fn assistant_entry_skips_attribution() {
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        sm.push_pending_attribution("i1", "u1".into(), "Alice".into(), "anything", None)
            .await;

        let entry = make_assistant_entry("e1", "Sure, I can help", "2024-06-01T12:00:00Z");
        let result =
            format_entry_with_attribution(&entry, "i1", None, Some(&sm)).await;

        assert!(result.get("attributed_to").is_none());
    }

    // ── Queue hit persists entry_uuid to DB ─────────────────────────

    #[tokio::test]
    async fn queue_match_writes_entry_uuid_to_db() {
        let repo = Arc::new(test_repository().await);
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        let ts = now_rfc3339();

        // Simulate handler: queue push + DB insert
        simulate_handler_input(&sm, &repo, "i1", "u1", "Alice", "fix the bug", None).await;

        let entry = make_user_entry("e1", "fix the bug", &ts);

        // First call — consumes queue, spawns write-back
        let r1 =
            format_entry_with_attribution(&entry, "i1", Some(&repo), Some(&sm)).await;
        assert_eq!(r1["attributed_to"]["user_id"], "u1");

        // Let the spawned write-back complete
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // DB should now have entry_uuid populated
        let db_attr = repo.get_attribution_by_entry_uuid("e1").await.unwrap();
        assert!(
            db_attr.is_some(),
            "After queue match + write-back, DB row should have entry_uuid set"
        );
        assert_eq!(db_attr.unwrap().user_id, "u1");
    }

    // ── Second caller (REST) gets attribution from DB after queue drained ─

    #[tokio::test]
    async fn second_caller_gets_attribution_from_db() {
        let repo = Arc::new(test_repository().await);
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        let ts = now_rfc3339();

        simulate_handler_input(&sm, &repo, "i1", "u1", "Alice", "fix the bug", None).await;

        let entry = make_user_entry("e1", "fix the bug", &ts);

        // First call (conversation watcher) — consumes queue
        let r1 =
            format_entry_with_attribution(&entry, "i1", Some(&repo), Some(&sm)).await;
        assert_eq!(r1["attributed_to"]["user_id"], "u1");

        // Let write-back finish
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Second call (REST endpoint) — queue empty, must come from DB
        let r2 =
            format_entry_with_attribution(&entry, "i1", Some(&repo), Some(&sm)).await;
        assert_eq!(
            r2["attributed_to"]["user_id"], "u1",
            "Second caller should get attribution from DB after queue was consumed"
        );
    }

    // ── Keystroke-by-keystroke: first char prefix-matches full entry ─

    #[tokio::test]
    async fn single_keystroke_prefix_matches_full_message() {
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        sm.push_pending_attribution("i1", "u1".into(), "Alice".into(), "h", None)
            .await;

        let entry = make_user_entry("e1", "hello world", "2024-06-01T12:00:00Z");
        let result =
            format_entry_with_attribution(&entry, "i1", None, Some(&sm)).await;

        assert_eq!(
            result["attributed_to"]["user_id"], "u1",
            "'h' should prefix-match 'hello world'"
        );
    }

    // ── Keystroke residuals don't false-match unrelated messages ─────

    #[tokio::test]
    async fn keystroke_residuals_dont_false_match() {
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        for ch in "hello".chars() {
            sm.push_pending_attribution("i1", "u1".into(), "Alice".into(), &ch.to_string(), None)
                .await;
        }

        // First entry consumes "h"
        let e1 = make_user_entry("e1", "hello", "2024-06-01T12:00:00Z");
        let r1 = format_entry_with_attribution(&e1, "i1", None, Some(&sm)).await;
        assert_eq!(r1["attributed_to"]["user_id"], "u1");

        // Second entry "goodbye" — residual "e","l","l","o" should NOT match
        let e2 = make_user_entry("e2", "goodbye", "2024-06-01T12:00:01Z");
        let r2 = format_entry_with_attribution(&e2, "i1", None, Some(&sm)).await;
        assert!(
            r2.get("attributed_to").is_none(),
            "Leftover single-char residuals should not match unrelated message"
        );
    }

    // ── Full handler → watcher → REST round-trip ────────────────────

    #[tokio::test]
    async fn full_round_trip_handler_then_watcher_then_rest() {
        let repo = Arc::new(test_repository().await);
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        let ts = now_rfc3339();

        // 1. Handler receives input
        simulate_handler_input(&sm, &repo, "i1", "u1", "Alice", "fix the bug\r\n", None)
            .await;

        // 2. Conversation watcher picks up the entry (uses queue)
        let entry = make_user_entry("e1", "fix the bug", &ts);
        let watcher_result =
            format_entry_with_attribution(&entry, "i1", Some(&repo), Some(&sm)).await;
        assert_eq!(
            watcher_result["attributed_to"]["user_id"], "u1",
            "Watcher should attribute via in-process queue"
        );

        // 3. Let DB write-back complete
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 4. REST endpoint reads same entry (queue already drained)
        let rest_result =
            format_entry_with_attribution(&entry, "i1", Some(&repo), Some(&sm)).await;
        assert_eq!(
            rest_result["attributed_to"]["user_id"], "u1",
            "REST endpoint should attribute via DB after queue was drained by watcher"
        );
    }

    // ── Two users, two messages, correct attribution ────────────────

    #[tokio::test]
    async fn two_users_attributed_correctly() {
        let repo = Arc::new(test_repository().await);
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        let ts = now_rfc3339();

        simulate_handler_input(&sm, &repo, "i1", "u1", "Alice", "hello from alice", None)
            .await;
        simulate_handler_input(&sm, &repo, "i1", "u2", "Bob", "hello from bob", None)
            .await;

        let e1 = make_user_entry("e1", "hello from alice", &ts);
        let e2 = make_user_entry("e2", "hello from bob", &ts);

        let r1 =
            format_entry_with_attribution(&e1, "i1", Some(&repo), Some(&sm)).await;
        let r2 =
            format_entry_with_attribution(&e2, "i1", Some(&repo), Some(&sm)).await;

        assert_eq!(r1["attributed_to"]["user_id"], "u1");
        assert_eq!(r2["attributed_to"]["user_id"], "u2");
    }

    // ── task_id is propagated through attribution ───────────────────

    #[tokio::test]
    async fn task_id_propagated() {
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        sm.push_pending_attribution("i1", "u1".into(), "Alice".into(), "do the thing", Some(42))
            .await;

        let entry = make_user_entry("e1", "do the thing", "2024-06-01T12:00:00Z");
        let result =
            format_entry_with_attribution(&entry, "i1", None, Some(&sm)).await;

        assert_eq!(result["attributed_to"]["user_id"], "u1");
        assert_eq!(result["task_id"], 42);
    }

    // ── Multiline with \r\n in input vs \n in entry ─────────────────

    #[tokio::test]
    async fn multiline_crlf_input_matches_lf_entry() {
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        // Terminal sends \r\n line endings
        sm.push_pending_attribution(
            "i1",
            "u1".into(),
            "Alice".into(),
            "line one\r\nline two",
            None,
        )
        .await;

        // But Claude writes \n line endings to JSONL
        let entry = make_user_entry("e1", "line one\nline two", "2024-06-01T12:00:00Z");
        let result =
            format_entry_with_attribution(&entry, "i1", None, Some(&sm)).await;

        assert_eq!(
            result["attributed_to"]["user_id"], "u1",
            "\\r\\n in input should still match \\n in entry"
        );
    }

    // ── Input with trailing \\r should match clean entry ────────────

    #[tokio::test]
    async fn trailing_cr_stripped_by_trim() {
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        sm.push_pending_attribution("i1", "u1".into(), "Alice".into(), "hello\r", None)
            .await;

        let entry = make_user_entry("e1", "hello", "2024-06-01T12:00:00Z");
        let result =
            format_entry_with_attribution(&entry, "i1", None, Some(&sm)).await;

        assert_eq!(
            result["attributed_to"]["user_id"], "u1",
            "Trailing \\r should be handled by trim in push_pending_attribution"
        );
    }
}

/// Diagnostic tests: trace the exact attribution pipeline to find where it breaks.
///
/// The live bug: user sends "Test" from the web UI, JSONL records `Text("Test")`,
/// but the entry shows `attributed_to: Terminal` instead of the user's name.
#[cfg(test)]
mod attribution_pipeline_diagnostic_tests {
    use super::*;
    use claude_convo::{ConversationEntry, Message, MessageContent, MessageRole};
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::models::{InputAttribution, User, attribution_content_matches, normalize_attribution_content};
    use crate::repository::test_helpers::test_repository;
    use crate::ws::{GlobalStateManager, create_state_broadcast};

    fn make_user(id: &str, name: &str) -> User {
        let now = chrono::Utc::now().timestamp();
        User {
            id: id.to_string(),
            username: name.to_string(),
            display_name: name.to_string(),
            password_hash: "hashed".to_string(),
            is_admin: false,
            is_disabled: false,
            created_at: now,
            updated_at: now,
        }
    }

    fn make_user_entry(uuid: &str, content: &str, ts: &str) -> ConversationEntry {
        ConversationEntry {
            uuid: uuid.to_string(),
            parent_uuid: None,
            is_sidechain: false,
            entry_type: "user".to_string(),
            session_id: Some("sess".to_string()),
            timestamp: ts.to_string(),
            cwd: None,
            git_branch: None,
            version: None,
            user_type: None,
            request_id: None,
            tool_use_result: None,
            snapshot: None,
            message_id: None,
            message: Some(Message {
                role: MessageRole::User,
                content: Some(MessageContent::Text(content.to_string())),
                model: None,
                id: None,
                message_type: None,
                stop_reason: None,
                stop_sequence: None,
                usage: None,
            }),
            extra: HashMap::new(),
        }
    }

    fn now_rfc3339() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    // ─────────────────────────────────────────────────────────────────
    // Step 0: Sanity — does "Test" match "Test"?
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn sanity_test_matches_test() {
        assert!(
            attribution_content_matches("Test", "Test"),
            "Exact match 'Test' vs 'Test' should succeed"
        );
    }

    #[test]
    fn sanity_normalize_test() {
        let n = normalize_attribution_content("Test");
        assert_eq!(n, "Test", "Normalizing 'Test' should produce 'Test'");
    }

    // ─────────────────────────────────────────────────────────────────
    // Step 1: Does push_pending_attribution actually store the entry?
    // ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn queue_stores_entry_and_consumes_it() {
        let sm = GlobalStateManager::new(create_state_broadcast());

        // Simulate handler push (what handler.rs does on Input)
        let trimmed = "Test".trim();
        sm.push_pending_attribution("inst1", "u1".into(), "Alice".into(), trimmed, None)
            .await;

        // Simulate watcher consume (what format_entry_with_attribution does)
        let result = sm.consume_pending_attribution("inst1", "Test").await;

        assert!(
            result.is_some(),
            "Queue should have an entry for 'Test' and consume_pending_attribution should find it"
        );
        let attr = result.unwrap();
        assert_eq!(attr.user_id, "u1");
        assert_eq!(attr.display_name, "Alice");
    }

    // ─────────────────────────────────────────────────────────────────
    // Step 2: Does consume work when called via format_entry_with_attribution?
    // (tests the content_text extraction path)
    // ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn format_entry_consumes_from_queue() {
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        sm.push_pending_attribution("inst1", "u1".into(), "Alice".into(), "Test", None)
            .await;

        let entry = make_user_entry("e1", "Test", "2024-06-01T12:00:00Z");
        let result =
            format_entry_with_attribution(&entry, "inst1", None, Some(&sm)).await;

        assert_eq!(
            result.get("attributed_to").and_then(|a| a.get("user_id")).and_then(|v| v.as_str()),
            Some("u1"),
            "format_entry_with_attribution should consume queue and attribute to u1"
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // Step 3: Queue is consumed only once — second caller gets nothing
    // (simulates watcher + REST endpoint racing)
    // ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn queue_consumed_once_second_caller_gets_terminal() {
        let repo = Arc::new(test_repository().await);
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));

        // Push attribution (handler)
        sm.push_pending_attribution("inst1", "u1".into(), "Alice".into(), "Test", None)
            .await;
        // NOTE: handler also writes a DB record — simulate that
        let _ = repo.create_user(&make_user("u1", "Alice")).await;
        let ts = chrono::Utc::now().timestamp();
        repo.record_input_attribution(&InputAttribution {
            id: None,
            instance_id: "inst1".into(),
            user_id: "u1".into(),
            display_name: "Alice".into(),
            timestamp: ts,
            entry_uuid: None,
            content_preview: Some("Test".into()),
            task_id: None,
        })
        .await
        .unwrap();

        let entry = make_user_entry("e1", "Test", &now_rfc3339());

        // First caller (e.g. conversation watcher) — should get attribution from queue
        let r1 =
            format_entry_with_attribution(&entry, "inst1", Some(&repo), Some(&sm)).await;
        assert_eq!(
            r1["attributed_to"]["user_id"], "u1",
            "First caller should consume queue successfully"
        );

        // Let write-back complete
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Second caller (e.g. REST endpoint) — queue is empty, should fall through to DB
        let r2 =
            format_entry_with_attribution(&entry, "inst1", Some(&repo), Some(&sm)).await;

        let r2_user = r2.get("attributed_to")
            .and_then(|a| a.get("user_id"))
            .and_then(|v| v.as_str());
        assert_eq!(
            r2_user,
            Some("u1"),
            "Second caller should find attribution in DB (not fall through to Terminal). \
             Got attributed_to = {:?}",
            r2.get("attributed_to")
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // Step 4: What happens when there's NO DB record?
    // (handler didn't write to DB — auth disabled or repo is None)
    // ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn queue_consumed_no_db_record_second_caller_gets_terminal() {
        let repo = Arc::new(test_repository().await);
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));

        // Push to queue but DON'T write DB record
        sm.push_pending_attribution("inst1", "u1".into(), "Alice".into(), "Test", None)
            .await;

        let entry = make_user_entry("e1", "Test", &now_rfc3339());

        // First caller consumes queue
        let r1 =
            format_entry_with_attribution(&entry, "inst1", Some(&repo), Some(&sm)).await;
        assert_eq!(r1["attributed_to"]["user_id"], "u1");

        // Write-back spawned by format_entry_with_attribution — but there's no DB
        // attribution record to correlate against (handler didn't write one).
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Second caller — queue empty, DB has no attribution record for this instance
        let r2 =
            format_entry_with_attribution(&entry, "inst1", Some(&repo), Some(&sm)).await;
        let r2_user = r2.get("attributed_to")
            .and_then(|a| a.get("user_id"))
            .and_then(|v| v.as_str());

        // This WILL be "terminal" — documenting the failure mode
        assert_eq!(
            r2_user,
            Some("terminal"),
            "Without DB record, second caller falls through to Terminal"
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // Step 5: What if the REST endpoint runs BEFORE the watcher?
    // (REST consumes queue, watcher falls through)
    // ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn rest_consumes_queue_before_watcher() {
        let repo = Arc::new(test_repository().await);
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));

        let _ = repo.create_user(&make_user("u1", "Alice")).await;

        // Handler pushes queue + DB
        sm.push_pending_attribution("inst1", "u1".into(), "Alice".into(), "Test", None)
            .await;
        let ts = chrono::Utc::now().timestamp();
        repo.record_input_attribution(&InputAttribution {
            id: None,
            instance_id: "inst1".into(),
            user_id: "u1".into(),
            display_name: "Alice".into(),
            timestamp: ts,
            entry_uuid: None,
            content_preview: Some("Test".into()),
            task_id: None,
        })
        .await
        .unwrap();

        let entry = make_user_entry("e1", "Test", &now_rfc3339());

        // REST endpoint processes the entry FIRST (user loaded the page)
        let rest_result =
            format_entry_with_attribution(&entry, "inst1", Some(&repo), Some(&sm)).await;
        assert_eq!(
            rest_result["attributed_to"]["user_id"], "u1",
            "REST endpoint should consume queue and attribute correctly"
        );

        // Let write-back from REST complete
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Now the watcher processes the same entry (queue is drained)
        let watcher_result =
            format_entry_with_attribution(&entry, "inst1", Some(&repo), Some(&sm)).await;
        let watcher_user = watcher_result.get("attributed_to")
            .and_then(|a| a.get("user_id"))
            .and_then(|v| v.as_str());

        assert_eq!(
            watcher_user,
            Some("u1"),
            "Watcher should find attribution in DB even though queue was consumed by REST. \
             Got attributed_to = {:?}",
            watcher_result.get("attributed_to")
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // Step 6: What happens with NO state_manager?
    // (repo is Some but state_manager is None — skips queue entirely)
    // ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn no_state_manager_with_repo_falls_to_terminal() {
        let repo = Arc::new(test_repository().await);

        let entry = make_user_entry("e1", "Test", &now_rfc3339());
        let result =
            format_entry_with_attribution(&entry, "inst1", Some(&repo), None).await;

        let user = result.get("attributed_to")
            .and_then(|a| a.get("user_id"))
            .and_then(|v| v.as_str());

        // With repo but no state_manager: queue is skipped, DB has nothing → Terminal
        assert_eq!(
            user,
            Some("terminal"),
            "No state_manager + empty DB → Terminal fallback"
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // Step 7: instance_id mismatch between push and consume
    // ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn instance_id_mismatch_misses_queue() {
        let repo = Arc::new(test_repository().await);
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));

        // Push to "inst1"
        sm.push_pending_attribution("inst1", "u1".into(), "Alice".into(), "Test", None)
            .await;

        // But watcher processes for "inst2"
        let entry = make_user_entry("e1", "Test", &now_rfc3339());
        let result =
            format_entry_with_attribution(&entry, "inst2", Some(&repo), Some(&sm)).await;

        assert_eq!(
            result["attributed_to"]["user_id"], "terminal",
            "Wrong instance_id should miss the queue and fall through to Terminal"
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // Step 8: THE ACTUAL BUG — loopback auth exemption means ws_user
    // is None, so handler never pushes attribution. Queue is always
    // empty. DB has no records. Everything falls to Terminal.
    //
    // This simulates what happens when connecting from localhost:
    // auth middleware returns early → no AuthUser in extensions →
    // MaybeAuthUser.0 is None → ws_user is None → no push.
    // ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn loopback_no_ws_user_never_pushes_attribution() {
        let repo = Arc::new(test_repository().await);
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));

        // === Handler receives Input, but ws_user is None (loopback exemption) ===
        // handler.rs line 327: if let Some(user) = &ws_user_clone { ... }
        // ws_user_clone is None → this block is skipped entirely
        // NO queue push, NO DB record
        let _data = "Test";

        // === Claude processes and writes JSONL ===
        let entry = make_user_entry("e1", "Test", &now_rfc3339());

        // === Watcher processes entry ===
        // repo is Some (auth is enabled), state_manager is Some
        let result = format_entry_with_attribution(
            &entry,
            "inst1",
            Some(&repo),
            Some(&sm),
        )
        .await;

        // Queue is empty (nothing pushed), DB is empty (nothing written)
        // → Falls through to Terminal
        assert_eq!(
            result["attributed_to"]["user_id"], "terminal",
            "BUG CONFIRMED: When ws_user is None (loopback auth exemption), \
             no attribution is ever pushed, and every user message falls to Terminal"
        );
        assert_eq!(
            result["attributed_to"]["display_name"], "Terminal",
        );
    }

    // ─────────────────────────────────────────────────────────────────
    // Step 9: Full end-to-end with ws_user present (working case)
    // ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn exact_real_flow_web_ui_message() {
        let repo = Arc::new(test_repository().await);
        let sm = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        let ts = now_rfc3339();

        let _ = repo.create_user(&make_user("u1", "Alice")).await;

        // === Step A: WebSocket handler receives Input{data: "Test"} ===
        // (from handler.rs lines 327-368)
        let data = "Test";
        let trimmed = data.trim();
        // Push to in-process queue
        sm.push_pending_attribution(
            "inst1",
            "u1".to_string(),
            "Alice".to_string(),
            trimmed,
            None,
        )
        .await;
        // Write DB attribution (spawned in handler, awaited here)
        let attr_ts = chrono::Utc::now().timestamp();
        repo.record_input_attribution(&InputAttribution {
            id: None,
            instance_id: "inst1".into(),
            user_id: "u1".into(),
            display_name: "Alice".into(),
            timestamp: attr_ts,
            entry_uuid: None,
            content_preview: Some(trimmed.chars().take(100).collect()),
            task_id: None,
        })
        .await
        .unwrap();

        // === Step B: Claude processes and writes JSONL ===
        let entry = make_user_entry("e1", "Test", &ts);

        // === Step C: Conversation watcher picks up entry ===
        let watcher_result = format_entry_with_attribution(
            &entry,
            "inst1",
            Some(&repo),
            Some(&sm),
        )
        .await;
        assert_eq!(
            watcher_result["attributed_to"]["user_id"], "u1",
            "Watcher (first consumer) should attribute to Alice"
        );

        // Let DB write-back complete
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // === Step D: REST endpoint also processes the entry ===
        let rest_result = format_entry_with_attribution(
            &entry,
            "inst1",
            Some(&repo),
            Some(&sm),
        )
        .await;
        assert_eq!(
            rest_result["attributed_to"]["user_id"], "u1",
            "REST endpoint (second consumer) should find attribution in DB. \
             Got: {:?}",
            rest_result.get("attributed_to")
        );
    }
}
