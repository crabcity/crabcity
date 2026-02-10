use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use claude_convo::{
    ClaudeConvo, ContentPart, ConversationQuery, HistoryQuery, Message, MessageContent, MessageRole,
};
use std::collections::{HashMap, HashSet};
use std::thread;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "claude-cli")]
#[command(version = "1.0")]
#[command(about = "CLI tool for exploring Claude conversation data")]
#[command(long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output format
    #[arg(short, long, value_enum, global = true, default_value = "text")]
    format: OutputFormat,

    /// Suppress informational output
    #[arg(short, long, global = true)]
    quiet: bool,
}

#[derive(ValueEnum, Clone, Debug)]
enum OutputFormat {
    Text,
    Json,
    Markdown,
}

#[derive(Subcommand)]
enum Commands {
    /// List projects or conversations
    List {
        #[command(subcommand)]
        subcommand: ListCommands,
    },

    /// Display conversation content
    Show {
        /// Project path (e.g., /Users/alex/project)
        project: String,

        /// Session ID or "latest" for most recent
        session: String,

        /// Show first N messages
        #[arg(long, conflicts_with_all = &["tail", "range_start"])]
        head: Option<usize>,

        /// Show last N messages (default: 10 if flag is used without value)
        #[arg(long, conflicts_with_all = &["head", "range_start"], num_args = 0..=1, default_missing_value = "10")]
        tail: Option<usize>,

        /// Follow mode - continuously watch for new messages (like tail -f)
        #[arg(short = 'F', long)]
        follow: bool,

        /// Start of range (0-indexed)
        #[arg(long, requires = "range_end", conflicts_with = "tail")]
        range_start: Option<usize>,

        /// End of range (exclusive)
        #[arg(long, requires = "range_start", conflicts_with = "tail")]
        range_end: Option<usize>,

        /// Show only messages from this role
        #[arg(long, value_enum)]
        role: Option<MessageRole>,

        /// Show raw JSON entries
        #[arg(long)]
        raw: bool,
    },

    /// Search within conversations
    Search {
        /// Search pattern
        pattern: String,

        /// Limit to specific project
        #[arg(long)]
        project: Option<String>,

        /// Search for tool usage instead of text
        #[arg(long, conflicts_with = "errors")]
        tool: bool,

        /// Search for errors only
        #[arg(long, conflicts_with = "tool")]
        errors: bool,

        /// Show context lines around matches
        #[arg(short = 'C', long, default_value = "0")]
        context: usize,
    },

    /// Show statistics
    Stats {
        /// Project path
        project: Option<String>,

        /// Session ID (if not provided, shows project stats)
        session: Option<String>,

        /// Show global statistics across all projects
        #[arg(long, conflicts_with_all = &["project", "session"])]
        all: bool,
    },

    /// Query global history
    History {
        /// Number of recent entries to show
        #[arg(long, default_value = "10")]
        last: usize,

        /// Filter by pattern
        #[arg(long)]
        grep: Option<String>,

        /// Filter by project
        #[arg(long)]
        project: Option<String>,
    },

    /// Export conversation data
    Export {
        /// Project path
        project: String,

        /// Session ID
        session: String,

        /// Include metadata
        #[arg(long)]
        with_metadata: bool,
    },

    /// Show a quick overview of Claude data
    Overview,
}

#[derive(Subcommand, Clone)]
enum ListCommands {
    /// List all projects with conversations
    Projects {
        /// Show conversation count for each project
        #[arg(long)]
        with_counts: bool,
    },

    /// List conversations in a project
    Convos {
        /// Project path (optional, shows all if not specified)
        project: Option<String>,

        /// Show detailed metadata
        #[arg(long)]
        detailed: bool,
    },

    /// Show recent conversations across all projects
    Recent {
        /// Number of conversations to show
        #[arg(long, default_value = "10")]
        count: usize,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let manager = ClaudeConvo::new();

    if !manager.exists() {
        eprintln!("Error: Claude directory not found at ~/.claude");
        std::process::exit(1);
    }

    match cli.command {
        Commands::List { ref subcommand } => handle_list(subcommand.clone(), &manager, &cli)?,
        Commands::Show {
            ref project,
            ref session,
            head,
            tail,
            follow,
            range_start,
            range_end,
            role,
            raw,
        } => handle_show(
            &manager,
            project,
            session,
            head,
            tail,
            follow,
            range_start,
            range_end,
            role,
            raw,
            &cli,
        )?,
        Commands::Search {
            ref pattern,
            ref project,
            tool,
            errors,
            context,
        } => handle_search(
            &manager,
            pattern,
            project.clone(),
            tool,
            errors,
            context,
            &cli,
        )?,
        Commands::Stats {
            ref project,
            ref session,
            all,
        } => handle_stats(&manager, project.clone(), session.clone(), all, &cli)?,
        Commands::History {
            last,
            ref grep,
            ref project,
        } => handle_history(&manager, last, grep.clone(), project.clone(), &cli)?,
        Commands::Export {
            ref project,
            ref session,
            with_metadata,
        } => handle_export(&manager, project, session, with_metadata, &cli)?,
        Commands::Overview => handle_overview(&manager, &cli)?,
    }

    Ok(())
}

fn handle_list(subcommand: ListCommands, manager: &ClaudeConvo, cli: &Cli) -> Result<()> {
    match subcommand {
        ListCommands::Projects { with_counts } => {
            let projects = manager.list_projects()?;

            if matches!(cli.format, OutputFormat::Json) {
                println!("{}", serde_json::to_string_pretty(&projects)?);
                return Ok(());
            }

            for project in projects {
                if with_counts {
                    let count = manager.list_conversations(&project)?.len();
                    println!("{} ({} conversations)", project, count);
                } else {
                    println!("{}", project);
                }
            }
        }
        ListCommands::Convos { project, detailed } => {
            // If no project specified, list conversations from all projects
            let projects = if let Some(proj) = project {
                vec![proj]
            } else {
                manager.list_projects()?
            };

            let mut all_metadata = Vec::new();
            for proj in &projects {
                let metadata = manager.list_conversation_metadata(proj)?;
                for meta in metadata {
                    all_metadata.push((proj.clone(), meta));
                }
            }

            // Sort by last activity
            all_metadata.sort_by(|a, b| b.1.last_activity.cmp(&a.1.last_activity));

            if matches!(cli.format, OutputFormat::Json) {
                println!("{}", serde_json::to_string_pretty(&all_metadata)?);
                return Ok(());
            }

            for (proj, meta) in all_metadata {
                if detailed {
                    println!("Session: {}", meta.session_id);
                    println!("  Project: {}", proj);
                    println!("  Messages: {}", meta.message_count);
                    if let Some(started) = meta.started_at {
                        println!("  Started: {}", started.format("%Y-%m-%d %H:%M:%S"));
                    }
                    if let Some(last) = meta.last_activity {
                        println!("  Last: {}", last.format("%Y-%m-%d %H:%M:%S"));
                    }
                    println!();
                } else {
                    let short_id = &meta.session_id[..8.min(meta.session_id.len())];
                    let short_proj = if projects.len() > 1 {
                        // Show abbreviated project path when listing multiple projects
                        let parts: Vec<&str> = proj.split('/').collect();
                        if parts.len() > 2 {
                            format!(".../{}", parts[parts.len() - 1])
                        } else {
                            proj.clone()
                        }
                    } else {
                        String::new()
                    };

                    let last = meta
                        .last_activity
                        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| "unknown".to_string());

                    if !short_proj.is_empty() {
                        println!(
                            "{} | {} | {} msgs | {}",
                            short_id, short_proj, meta.message_count, last
                        );
                    } else {
                        println!("{} | {} msgs | {}", short_id, meta.message_count, last);
                    }
                }
            }
        }
        ListCommands::Recent { count } => {
            let projects = manager.list_projects()?;
            let mut all_metadata = Vec::new();

            for project in projects {
                let metadata = manager.list_conversation_metadata(&project)?;
                for meta in metadata {
                    all_metadata.push((project.clone(), meta));
                }
            }

            all_metadata.sort_by(|a, b| b.1.last_activity.cmp(&a.1.last_activity));
            all_metadata.truncate(count);

            if matches!(cli.format, OutputFormat::Json) {
                println!("{}", serde_json::to_string_pretty(&all_metadata)?);
                return Ok(());
            }

            for (project, meta) in all_metadata {
                let short_id = &meta.session_id[..8.min(meta.session_id.len())];
                let last = meta
                    .last_activity
                    .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                println!(
                    "{} | {} | {} msgs | {}",
                    project, short_id, meta.message_count, last
                );
            }
        }
    }

    Ok(())
}

// Helper function to resolve session ID from a prefix
fn resolve_session_id(manager: &ClaudeConvo, project: &str, session: &str) -> Result<String> {
    if session == "latest" {
        let metadata = manager.list_conversation_metadata(project)?;
        metadata
            .first()
            .map(|m| m.session_id.clone())
            .ok_or_else(|| anyhow::anyhow!("No conversations found in project"))
    } else {
        // Try to find a session that matches the prefix
        let sessions = manager.list_conversations(project)?;
        let matching_sessions: Vec<_> =
            sessions.iter().filter(|s| s.starts_with(session)).collect();

        match matching_sessions.len() {
            0 => Err(anyhow::anyhow!(
                "No session found matching prefix: {}",
                session
            )),
            1 => Ok(matching_sessions[0].clone()),
            _ => {
                // Multiple matches - show them and ask user to be more specific
                eprintln!("Multiple sessions found matching prefix '{}':", session);
                for s in &matching_sessions {
                    eprintln!("  {}", s);
                }
                Err(anyhow::anyhow!(
                    "Please provide a more specific session prefix"
                ))
            }
        }
    }
}

fn handle_show(
    manager: &ClaudeConvo,
    project: &str,
    session: &str,
    head: Option<usize>,
    tail: Option<usize>,
    follow: bool,
    range_start: Option<usize>,
    range_end: Option<usize>,
    role: Option<MessageRole>,
    raw: bool,
    cli: &Cli,
) -> Result<()> {
    let session_id = resolve_session_id(manager, project, session)?;

    if follow {
        // Follow mode - tail and watch for new messages (like tail -f)
        // If tail isn't specified with follow, default to 10
        let tail_count = tail.unwrap_or(10);
        handle_tail_follow(manager, project, &session_id, tail_count, role, raw, cli)?;
    } else {
        // Normal mode - show and exit
        let convo = manager
            .read_conversation(project, &session_id)
            .context("Failed to read conversation")?;

        let mut entries = convo.entries.clone();

        // Filter by role if specified
        if let Some(role_filter) = role {
            entries.retain(|e| {
                e.message
                    .as_ref()
                    .map(|m| m.role == role_filter)
                    .unwrap_or(false)
            });
        }

        // Apply range selection
        let entries_to_show = if let Some(n) = head {
            entries.into_iter().take(n).collect()
        } else if let Some(n) = tail {
            let len = entries.len();
            entries.into_iter().skip(len.saturating_sub(n)).collect()
        } else if let (Some(start), Some(end)) = (range_start, range_end) {
            entries.into_iter().skip(start).take(end - start).collect()
        } else {
            entries
        };

        if raw || matches!(cli.format, OutputFormat::Json) {
            println!("{}", serde_json::to_string_pretty(&entries_to_show)?);
        } else {
            print_entries(&entries_to_show, &cli.format)?;
        }
    }

    Ok(())
}

fn handle_tail_follow(
    manager: &ClaudeConvo,
    project: &str,
    session_id: &str,
    tail_count: usize,
    role: Option<MessageRole>,
    raw: bool,
    cli: &Cli,
) -> Result<()> {
    // Track which UUIDs we've already displayed
    let mut seen_uuids: HashSet<String> = HashSet::new();

    // First, show the tail if requested
    let convo = manager
        .read_conversation(project, session_id)
        .context("Failed to read conversation")?;

    let mut entries = convo.entries.clone();

    // Filter by role if specified
    if let Some(role_filter) = role {
        entries.retain(|e| {
            e.message
                .as_ref()
                .map(|m| m.role == role_filter)
                .unwrap_or(false)
        });
    }

    // Show initial tail
    let len = entries.len();
    let entries_to_show: Vec<_> = entries
        .into_iter()
        .skip(len.saturating_sub(tail_count))
        .collect();

    // Mark these as seen and display them
    for entry in &entries_to_show {
        seen_uuids.insert(entry.uuid.clone());
    }

    if raw || matches!(cli.format, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&entries_to_show)?);
    } else {
        print_entries(&entries_to_show, &cli.format)?;
    }

    println!("\n=== Following conversation (press Ctrl+C to stop) ===\n");

    // Now poll for new messages
    loop {
        thread::sleep(Duration::from_secs(1));

        // Re-read the conversation
        match manager.read_conversation(project, session_id) {
            Ok(convo) => {
                let mut new_entries = Vec::new();

                for entry in &convo.entries {
                    if !seen_uuids.contains(&entry.uuid) {
                        // Filter by role if specified
                        if let Some(role_filter) = role {
                            if let Some(msg) = &entry.message {
                                if msg.role != role_filter {
                                    seen_uuids.insert(entry.uuid.clone());
                                    continue;
                                }
                            } else {
                                seen_uuids.insert(entry.uuid.clone());
                                continue;
                            }
                        }

                        new_entries.push(entry.clone());
                        seen_uuids.insert(entry.uuid.clone());
                    }
                }

                // Display new entries
                if !new_entries.is_empty() {
                    if raw || matches!(cli.format, OutputFormat::Json) {
                        println!("{}", serde_json::to_string_pretty(&new_entries)?);
                    } else {
                        print_entries(&new_entries, &cli.format)?;
                    }
                }
            }
            Err(e) => {
                eprintln!("Error reading conversation: {}", e);
                break;
            }
        }
    }

    Ok(())
}

fn print_entries(entries: &[claude_convo::ConversationEntry], format: &OutputFormat) -> Result<()> {
    for (i, entry) in entries.iter().enumerate() {
        if matches!(format, OutputFormat::Markdown) {
            if let Some(msg) = &entry.message {
                match msg.role {
                    MessageRole::User => println!("\n### User ({})", i),
                    MessageRole::Assistant => println!("\n### Assistant ({})", i),
                    MessageRole::System => println!("\n### System ({})", i),
                }
            } else {
                println!("\n### Entry {} ({})", i, entry.entry_type);
            }
        } else {
            println!("\n[{}] {} - {}", i, entry.entry_type, &entry.uuid[..8]);
            if let Ok(timestamp) = entry.timestamp.parse::<DateTime<Utc>>() {
                println!("Time: {}", timestamp.format("%Y-%m-%d %H:%M:%S"));
            }
        }

        if let Some(msg) = &entry.message {
            print_message(msg, format)?;
        }
    }
    Ok(())
}

fn print_message(msg: &Message, format: &OutputFormat) -> Result<()> {
    match &msg.content {
        Some(MessageContent::Text(text)) => {
            if matches!(format, OutputFormat::Markdown) {
                println!("{}", text);
            } else {
                println!("{}: {}", msg.role_str(), text);
            }
        }
        Some(MessageContent::Parts(parts)) => {
            for part in parts {
                match part {
                    ContentPart::Text { text } => {
                        if matches!(format, OutputFormat::Markdown) {
                            println!("{}", text);
                        } else {
                            println!("{}: {}", msg.role_str(), text);
                        }
                    }
                    ContentPart::ToolUse { name, .. } => {
                        if matches!(format, OutputFormat::Markdown) {
                            println!("**Tool Use**: `{}`", name);
                        } else {
                            println!("Tool Use: {}", name);
                        }
                    }
                    ContentPart::ToolResult {
                        content, is_error, ..
                    } => {
                        let prefix = if *is_error { "Error" } else { "Result" };
                        if matches!(format, OutputFormat::Markdown) {
                            println!("**Tool {}**:\n```\n{}\n```", prefix, content);
                        } else {
                            println!("Tool {}: {}", prefix, content);
                        }
                    }
                    ContentPart::Thinking { thinking, .. } => {
                        if matches!(format, OutputFormat::Markdown) {
                            println!(
                                "*Thinking*: {}",
                                thinking.chars().take(100).collect::<String>()
                            );
                        }
                        // Skip thinking in other formats
                    }
                    ContentPart::Unknown => {
                        // Skip unknown content types
                    }
                }
            }
        }
        None => {}
    }
    Ok(())
}

fn handle_search(
    manager: &ClaudeConvo,
    pattern: &str,
    project: Option<String>,
    tool: bool,
    errors: bool,
    context: usize,
    cli: &Cli,
) -> Result<()> {
    let projects = if let Some(p) = project {
        vec![p]
    } else {
        manager.list_projects()?
    };

    let mut results = Vec::new();

    for project in projects {
        let conversations = manager.read_all_conversations(&project)?;

        for convo in conversations {
            let query = ConversationQuery::new(&convo);

            let matches = if errors {
                query.errors()
            } else if tool {
                query.tool_uses_by_name(pattern)
            } else {
                query.contains_text(pattern)
            };

            if !matches.is_empty() {
                results.push((project.clone(), convo.session_id.clone(), matches.len()));

                if !cli.quiet {
                    println!(
                        "\n{} | {} ({} matches)",
                        project,
                        &convo.session_id[..8.min(convo.session_id.len())],
                        matches.len()
                    );

                    if context > 0 {
                        for entry in matches.iter().take(3) {
                            if let Some(msg) = &entry.message {
                                print_message_snippet(msg, pattern)?;
                            }
                        }
                    }
                }
            }
        }
    }

    if matches!(cli.format, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else if !cli.quiet {
        println!("\nTotal: {} conversations with matches", results.len());
    }

    Ok(())
}

fn print_message_snippet(msg: &Message, highlight: &str) -> Result<()> {
    let content_text = match &msg.content {
        Some(MessageContent::Text(text)) => text.clone(),
        Some(MessageContent::Parts(parts)) => parts
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
        None => String::new(),
    };

    if let Some(pos) = content_text.to_lowercase().find(&highlight.to_lowercase()) {
        let start = pos.saturating_sub(50);
        let end = (pos + highlight.len() + 50).min(content_text.len());
        let snippet = &content_text[start..end];
        println!("  ...{}...", snippet);
    }

    Ok(())
}

fn handle_stats(
    manager: &ClaudeConvo,
    project: Option<String>,
    session: Option<String>,
    all: bool,
    _cli: &Cli,
) -> Result<()> {
    if all {
        let projects = manager.list_projects()?;
        let mut total_convos = 0;
        let mut total_messages = 0;
        let mut total_user_messages = 0;
        let mut total_assistant_messages = 0;
        let mut tool_counts: HashMap<String, usize> = HashMap::new();

        for project in &projects {
            let metadata = manager.list_conversation_metadata(project)?;
            total_convos += metadata.len();

            for meta in metadata {
                total_messages += meta.message_count;

                if let Ok(convo) = manager.read_conversation(project, &meta.session_id) {
                    total_user_messages += convo.user_messages().len();
                    total_assistant_messages += convo.assistant_messages().len();

                    for (_, tool) in convo.tool_uses() {
                        if let ContentPart::ToolUse { name, .. } = tool {
                            *tool_counts.entry(name.clone()).or_insert(0) += 1;
                        }
                    }
                }
            }
        }

        println!("=== Global Statistics ===");
        println!("Projects: {}", projects.len());
        println!("Total conversations: {}", total_convos);
        println!("Total messages: {}", total_messages);
        println!("  User turns: {}", total_user_messages);
        println!("  Assistant responses: {}", total_assistant_messages);
        println!("\nTop tools used:");
        let mut tools: Vec<_> = tool_counts.into_iter().collect();
        tools.sort_by(|a, b| b.1.cmp(&a.1));
        for (name, count) in tools.iter().take(10) {
            println!("  {}: {} times", name, count);
        }
    } else if let Some(project_path) = project {
        if let Some(session_str) = session {
            let session_id = resolve_session_id(manager, &project_path, &session_str)?;
            let convo = manager.read_conversation(&project_path, &session_id)?;
            print_conversation_stats(&convo)?;
        } else {
            print_project_stats(manager, &project_path)?;
        }
    } else {
        println!("Please specify --all, or provide a project path");
    }

    Ok(())
}

fn print_conversation_stats(convo: &claude_convo::Conversation) -> Result<()> {
    println!("=== Conversation Statistics ===");
    println!("Session ID: {}", convo.session_id);
    println!("Total entries: {}", convo.entries.len());
    println!("User messages: {}", convo.user_messages().len());
    println!("Assistant messages: {}", convo.assistant_messages().len());

    if let Some(duration) = convo.duration() {
        let minutes = duration.num_minutes();
        let hours = minutes / 60;
        let mins = minutes % 60;
        println!("Duration: {}h {}m", hours, mins);
    }

    let tool_uses = convo.tool_uses();
    if !tool_uses.is_empty() {
        println!("\nTool usage:");
        let mut tool_counts: HashMap<&str, usize> = HashMap::new();
        for (_, tool) in &tool_uses {
            if let ContentPart::ToolUse { name, .. } = tool {
                *tool_counts.entry(name.as_str()).or_insert(0) += 1;
            }
        }
        for (name, count) in tool_counts {
            println!("  {}: {} times", name, count);
        }
    }

    Ok(())
}

fn print_project_stats(manager: &ClaudeConvo, project: &str) -> Result<()> {
    let metadata = manager.list_conversation_metadata(project)?;

    println!("=== Project Statistics: {} ===", project);
    println!("Total conversations: {}", metadata.len());

    let total_messages: usize = metadata.iter().map(|m| m.message_count).sum();
    println!("Total messages: {}", total_messages);

    // Calculate user and assistant message counts
    let mut user_messages = 0;
    let mut assistant_messages = 0;
    let mut tool_counts: HashMap<String, usize> = HashMap::new();

    for meta in &metadata {
        if let Ok(convo) = manager.read_conversation(project, &meta.session_id) {
            user_messages += convo.user_messages().len();
            assistant_messages += convo.assistant_messages().len();

            for (_, tool) in convo.tool_uses() {
                if let ContentPart::ToolUse { name, .. } = tool {
                    *tool_counts.entry(name.clone()).or_insert(0) += 1;
                }
            }
        }
    }

    println!("  User turns: {}", user_messages);
    println!("  Assistant responses: {}", assistant_messages);

    if let Some(earliest) = metadata.iter().filter_map(|m| m.started_at).min() {
        println!("\nFirst conversation: {}", earliest.format("%Y-%m-%d"));
    }

    if let Some(latest) = metadata.iter().filter_map(|m| m.last_activity).max() {
        println!("Latest activity: {}", latest.format("%Y-%m-%d %H:%M"));
    }

    if !tool_counts.is_empty() {
        println!("\nTop tools used:");
        let mut tools: Vec<_> = tool_counts.into_iter().collect();
        tools.sort_by(|a, b| b.1.cmp(&a.1));
        for (name, count) in tools.iter().take(5) {
            println!("  {}: {} times", name, count);
        }
    }

    Ok(())
}

fn handle_history(
    manager: &ClaudeConvo,
    last: usize,
    grep: Option<String>,
    project: Option<String>,
    cli: &Cli,
) -> Result<()> {
    let history = manager.read_history()?;
    let query = HistoryQuery::new(&history);

    let entries = if let Some(pattern) = grep {
        query.contains_text(&pattern)
    } else if let Some(proj) = project {
        query.by_project(&proj)
    } else {
        query.recent(last)
    };

    if matches!(cli.format, OutputFormat::Json) {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    for entry in entries {
        let timestamp =
            DateTime::from_timestamp(entry.timestamp / 1000, 0).unwrap_or(DateTime::default());

        println!("{}", entry.display);
        if let Some(proj) = &entry.project {
            println!("  Project: {}", proj);
        }
        if let Some(session) = &entry.session_id {
            println!("  Session: {}", &session[..8.min(session.len())]);
        }
        println!("  Time: {}", timestamp.format("%Y-%m-%d %H:%M"));
        println!();
    }

    Ok(())
}

fn handle_export(
    manager: &ClaudeConvo,
    project: &str,
    session: &str,
    with_metadata: bool,
    cli: &Cli,
) -> Result<()> {
    let convo = manager.read_conversation(project, session)?;

    match cli.format {
        OutputFormat::Json => {
            if with_metadata {
                println!("{}", serde_json::to_string_pretty(&convo)?);
            } else {
                println!("{}", serde_json::to_string_pretty(&convo.entries)?);
            }
        }
        OutputFormat::Markdown => {
            println!("# Claude Conversation Export\n");
            println!("**Project**: {}", project);
            println!("**Session**: {}\n", session);

            if with_metadata {
                if let Some(started) = convo.started_at {
                    println!("**Started**: {}", started.format("%Y-%m-%d %H:%M"));
                }
                if let Some(last) = convo.last_activity {
                    println!("**Last Activity**: {}", last.format("%Y-%m-%d %H:%M"));
                }
                println!("**Total Messages**: {}\n", convo.message_count());
                println!("---\n");
            }

            print_entries(&convo.entries, &OutputFormat::Markdown)?;
        }
        OutputFormat::Text => {
            print_entries(&convo.entries, &OutputFormat::Text)?;
        }
    }

    Ok(())
}

fn handle_overview(manager: &ClaudeConvo, _cli: &Cli) -> Result<()> {
    let projects = manager.list_projects()?;
    let mut total_convos = 0;
    let mut total_messages = 0;

    for project in &projects {
        let metadata = manager.list_conversation_metadata(project)?;
        total_convos += metadata.len();
        total_messages += metadata.iter().map(|m| m.message_count).sum::<usize>();
    }

    println!("=== Claude Data Overview ===");
    println!("Claude directory: {:?}", manager.claude_dir_path()?);
    println!("\nProjects: {}", projects.len());
    println!("Total conversations: {}", total_convos);
    println!("Total messages: {}", total_messages);

    let history = manager.read_history()?;
    println!("History entries: {}", history.len());

    println!("\nRecent activity:");
    let query = HistoryQuery::new(&history);
    let recent = query.recent(3);
    for entry in recent {
        println!("  - {}", entry.display);
        if let Some(proj) = &entry.project {
            println!("    in {}", proj);
        }
    }

    println!("\nUse 'claude-cli list projects' to see all projects");
    println!("Use 'claude-cli show <project> latest' to view recent conversations");

    Ok(())
}

// Helper trait to add role_str method
trait MessageRoleExt {
    fn role_str(&self) -> &str;
}

impl MessageRoleExt for Message {
    fn role_str(&self) -> &str {
        match self.role {
            MessageRole::User => "User",
            MessageRole::Assistant => "Assistant",
            MessageRole::System => "System",
        }
    }
}
