use anyhow::Result;
use claude_convo::{ClaudeConvo, ConversationQuery, HistoryQuery};

fn main() -> Result<()> {
    let manager = ClaudeConvo::new();

    if !manager.exists() {
        println!("Claude directory not found at ~/.claude");
        return Ok(());
    }

    println!("Claude directory: {:?}", manager.claude_dir_path()?);
    println!();

    println!("=== Projects with conversations ===");
    let projects = manager.list_projects()?;
    for project in &projects {
        println!("  {}", project);
    }
    println!();

    if let Some(first_project) = projects.first() {
        println!("=== Conversations in {} ===", first_project);
        let metadata = manager.list_conversation_metadata(first_project)?;

        for meta in &metadata {
            println!("  Session: {}", &meta.session_id[..8]);
            println!("    Messages: {}", meta.message_count);
            if let Some(started) = meta.started_at {
                println!("    Started: {}", started.format("%Y-%m-%d %H:%M"));
            }
            if let Some(last) = meta.last_activity {
                println!("    Last activity: {}", last.format("%Y-%m-%d %H:%M"));
            }
            println!();
        }

        if let Some(latest_meta) = metadata.first() {
            println!("=== Latest conversation details ===");
            let convo = manager.read_conversation(first_project, &latest_meta.session_id)?;

            println!("Session ID: {}", convo.session_id);
            println!("Total entries: {}", convo.entries.len());
            println!("User messages: {}", convo.user_messages().len());
            println!("Assistant messages: {}", convo.assistant_messages().len());

            let tool_uses = convo.tool_uses();
            if !tool_uses.is_empty() {
                println!("\nTool uses:");
                let mut tool_counts = std::collections::HashMap::new();
                for (_, tool) in &tool_uses {
                    if let claude_convo::ContentPart::ToolUse { name, .. } = tool {
                        *tool_counts.entry(name.as_str()).or_insert(0) += 1;
                    }
                }
                for (name, count) in tool_counts {
                    println!("  {}: {} times", name, count);
                }
            }

            let query = ConversationQuery::new(&convo);
            let errors = query.errors();
            if !errors.is_empty() {
                println!("\nFound {} errors in conversation", errors.len());
            }
        }
    }

    println!("\n=== Recent history ===");
    let history = manager.read_history()?;
    let query = HistoryQuery::new(&history);
    let recent = query.recent(5);

    for entry in recent {
        println!("  {}", entry.display);
        if let Some(project) = &entry.project {
            println!("    Project: {}", project);
        }
        let timestamp = chrono::DateTime::from_timestamp(entry.timestamp / 1000, 0)
            .unwrap_or(chrono::DateTime::default());
        println!("    Time: {}", timestamp.format("%Y-%m-%d %H:%M"));
        println!();
    }

    Ok(())
}
