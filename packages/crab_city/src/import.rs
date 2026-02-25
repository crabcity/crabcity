use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use toolpath_claude::ClaudeConvo;
use tracing::{debug, error, info, warn};

use crate::models::{Conversation, ConversationEntry};
use crate::repository::ConversationRepository;

/// Import format version - increment when import logic extracts more/different data.
/// This triggers re-import of conversations even if the source file hasn't changed.
///
/// Version history:
/// - 1: Initial import format
/// - 2: (future) Extract progress entries, agent metadata, etc.
pub const IMPORT_FORMAT_VERSION: i64 = 1;

/// Result of importing a single session
enum ImportResult {
    /// Newly imported
    Imported,
    /// Updated (re-imported because file changed)
    Updated,
    /// Skipped (file unchanged)
    Skipped,
}

pub struct ConversationImporter {
    repository: ConversationRepository,
    claude_convo: ClaudeConvo,
}

impl ConversationImporter {
    pub fn new(repository: ConversationRepository) -> Self {
        Self {
            repository,
            claude_convo: ClaudeConvo::new(),
        }
    }

    /// Import all conversations from a project directory
    pub async fn import_from_project(&self, project_path: &Path) -> Result<ImportStats> {
        info!(
            "📥 Scanning for conversations in: {}",
            project_path.display()
        );

        let mut stats = ImportStats::default();

        // List all conversation session IDs in this project
        let session_ids = self
            .claude_convo
            .list_conversations(&project_path.to_string_lossy())?;

        info!(
            "Found {} conversation sessions to import",
            session_ids.len()
        );

        for session_id in session_ids {
            match self.import_session(project_path, &session_id).await {
                Ok(ImportResult::Imported) => {
                    stats.imported += 1;
                    info!("✅ Imported session: {}", session_id);
                }
                Ok(ImportResult::Updated) => {
                    stats.updated += 1;
                    info!("🔄 Updated session: {}", session_id);
                }
                Ok(ImportResult::Skipped) => {
                    stats.skipped += 1;
                }
                Err(e) => {
                    error!("Failed to import session {}: {}", session_id, e);
                    stats.failed += 1;
                }
            }
        }

        Ok(stats)
    }

    /// Import all conversations from all known projects
    pub async fn import_all_projects(&self) -> Result<ImportStats> {
        info!("📥 Scanning for all Claude Code conversations...");

        let mut total_stats = ImportStats::default();

        // Use ClaudeConvo to discover all projects with conversations
        let project_paths = match self.claude_convo.list_projects() {
            Ok(paths) => paths,
            Err(e) => {
                warn!("Failed to list projects: {}", e);
                // Fall back to current directory
                vec![
                    std::env::current_dir()
                        .unwrap_or_else(|_| PathBuf::from("."))
                        .to_string_lossy()
                        .to_string(),
                ]
            }
        };

        info!("Found {} projects with conversations", project_paths.len());

        for project_path_str in project_paths {
            let project_path = PathBuf::from(&project_path_str);

            debug!("Checking project: {}", project_path.display());

            match self.import_from_project(&project_path).await {
                Ok(stats) => {
                    if stats.imported > 0 || stats.updated > 0 || stats.skipped > 0 {
                        info!(
                            "Project {}: imported={}, updated={}, skipped={}",
                            project_path.display(),
                            stats.imported,
                            stats.updated,
                            stats.skipped
                        );
                    }
                    total_stats.add(&stats);
                }
                Err(e) => {
                    debug!("No conversations in {}: {}", project_path.display(), e);
                }
            }
        }

        Ok(total_stats)
    }

    /// Get file mtime and size for staleness detection.
    /// Returns (mtime_secs, size_bytes) or None if stat fails.
    fn file_stat(path: &Path) -> Option<(i64, u64)> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime = meta
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_secs() as i64;
        let size = meta.len();
        Some((mtime, size))
    }

    /// Import a single conversation session, with staleness detection.
    async fn import_session(&self, project_path: &Path, session_id: &str) -> Result<ImportResult> {
        // Get the JSONL file path for this session
        let metadata = self
            .claude_convo
            .read_conversation_metadata(&project_path.to_string_lossy(), session_id)?;
        let file_path = &metadata.file_path;

        // Stat the file for mtime + size
        let (file_mtime, file_size) = match Self::file_stat(file_path) {
            Some(v) => v,
            None => {
                debug!("Could not stat file for session {}, skipping", session_id);
                return Ok(ImportResult::Skipped);
            }
        };
        let file_hash = file_size.to_string();

        // Check if this conversation already exists
        let existing = self
            .repository
            .get_conversation_by_session_id(session_id)
            .await?;

        if let Some(existing_conv) = existing {
            // Session exists -- check staleness
            let db_mtime = existing_conv.file_mtime.unwrap_or(0);
            let db_hash = existing_conv.file_hash.as_deref().unwrap_or("");
            let db_import_version = existing_conv.import_version.unwrap_or(0);

            let file_unchanged = db_mtime == file_mtime && db_hash == file_hash;
            let import_version_current = db_import_version >= IMPORT_FORMAT_VERSION;

            if file_unchanged && import_version_current {
                // File unchanged and import version is current, skip
                debug!(
                    "Session {} unchanged (mtime={}, size={}, import_v={}), skipping",
                    session_id, file_mtime, file_size, db_import_version
                );
                return Ok(ImportResult::Skipped);
            }

            // Re-import needed: file changed or import version outdated
            if !file_unchanged {
                info!(
                    "Session {} file changed (mtime {}→{}, size {}→{}), re-importing",
                    session_id, db_mtime, file_mtime, db_hash, file_hash
                );
            } else {
                info!(
                    "Session {} import version outdated ({}→{}), re-importing",
                    session_id, db_import_version, IMPORT_FORMAT_VERSION
                );
            }

            // Delete old entries
            self.repository
                .delete_conversation_entries(&existing_conv.id)
                .await?;

            // Re-read and re-insert entries
            let claude_conversation = self
                .claude_convo
                .read_conversation(&project_path.to_string_lossy(), session_id)?;

            let mut db_entries = Vec::new();
            let mut title = existing_conv.title.clone();
            let mut title_set = title.is_some();

            for entry in &claude_conversation.entries {
                if !title_set {
                    if let Some(extracted) = Self::extract_title(entry) {
                        title = Some(extracted);
                        title_set = true;
                    }
                }
                let db_entry =
                    ConversationEntry::from_claude_entry(existing_conv.id.clone(), entry);
                db_entries.push(db_entry);
            }

            if !db_entries.is_empty() {
                self.repository.add_entries_batch(&db_entries).await?;
            }

            // Update title if we found a new one
            if let Some(ref t) = title {
                if existing_conv.title.as_deref() != Some(t) {
                    self.repository
                        .update_conversation_title(&existing_conv.id, t)
                        .await?;
                }
            }

            // Update file metadata and import version
            let now = chrono::Utc::now().timestamp();
            self.repository
                .update_conversation_file_metadata(
                    session_id,
                    &file_hash,
                    file_mtime,
                    IMPORT_FORMAT_VERSION,
                    now,
                )
                .await?;

            return Ok(ImportResult::Updated);
        }

        // New session -- full import
        let conversation_id = uuid::Uuid::new_v4().to_string();

        // Determine instance_id from project path
        let instance_id = project_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "imported".to_string());

        // Create the conversation record
        let mut conversation = Conversation::new(conversation_id.clone(), instance_id)
            .with_session_id(session_id.to_string());

        // Set timestamps from metadata
        if let Some(started) = metadata.started_at {
            conversation.created_at = started.timestamp();
        }
        if let Some(last_activity) = metadata.last_activity {
            conversation.updated_at = last_activity.timestamp();
        }

        // Store file metadata for staleness detection
        conversation.file_hash = Some(file_hash);
        conversation.file_mtime = Some(file_mtime);
        conversation.import_version = Some(IMPORT_FORMAT_VERSION);

        // Read all conversation entries
        let claude_conversation = self
            .claude_convo
            .read_conversation(&project_path.to_string_lossy(), session_id)?;

        // Extract title from first user message
        let mut title_set = false;
        let mut db_entries = Vec::new();

        for entry in &claude_conversation.entries {
            if !title_set {
                if let Some(extracted) = Self::extract_title(entry) {
                    conversation.title = Some(extracted);
                    title_set = true;
                }
            }

            // Convert to database entry
            let db_entry = ConversationEntry::from_claude_entry(conversation_id.clone(), entry);
            db_entries.push(db_entry);
        }

        // If no title was found, use a default
        if conversation.title.is_none() {
            conversation.title = Some(format!("Imported: {}", session_id));
        }

        // Save to database
        self.repository.create_conversation(&conversation).await?;

        // Add all entries
        if !db_entries.is_empty() {
            self.repository.add_entries_batch(&db_entries).await?;
        }

        Ok(ImportResult::Imported)
    }

    /// Extract a title from a conversation entry (first user message text, truncated to 100 chars)
    fn extract_title(entry: &toolpath_claude::ConversationEntry) -> Option<String> {
        let msg = entry.message.as_ref()?;
        if !msg.is_user() {
            return None;
        }
        let text = entry.text();
        if text.is_empty() {
            return None;
        }
        let truncated: String = text.chars().take(100).collect();
        Some(truncated)
    }
}

#[derive(Debug, Default)]
pub struct ImportStats {
    pub imported: usize,
    pub updated: usize,
    pub skipped: usize,
    pub failed: usize,
}

impl ImportStats {
    pub fn total(&self) -> usize {
        self.imported + self.updated + self.skipped + self.failed
    }

    pub fn add(&mut self, other: &ImportStats) {
        self.imported += other.imported;
        self.updated += other.updated;
        self.skipped += other.skipped;
        self.failed += other.failed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use toolpath_claude::{ConversationEntry, Message, MessageContent, MessageRole};

    /// Helper to create a minimal conversation entry for testing
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

    /// Helper to create a minimal message for testing
    fn make_message(role: MessageRole, content: Option<MessageContent>) -> Message {
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

    #[test]
    fn test_import_stats_total() {
        let stats = ImportStats {
            imported: 5,
            updated: 3,
            skipped: 10,
            failed: 2,
        };
        assert_eq!(stats.total(), 20);
    }

    #[test]
    fn test_import_stats_add() {
        let mut stats1 = ImportStats {
            imported: 5,
            updated: 3,
            skipped: 10,
            failed: 2,
        };
        let stats2 = ImportStats {
            imported: 2,
            updated: 1,
            skipped: 5,
            failed: 0,
        };
        stats1.add(&stats2);
        assert_eq!(stats1.imported, 7);
        assert_eq!(stats1.updated, 4);
        assert_eq!(stats1.skipped, 15);
        assert_eq!(stats1.failed, 2);
    }

    #[test]
    fn test_extract_title_from_user_message() {
        let entry = make_entry(
            "123",
            "user",
            Some(make_message(
                MessageRole::User,
                Some(MessageContent::Text("Help me write a function".to_string())),
            )),
        );

        let title = ConversationImporter::extract_title(&entry);
        assert_eq!(title, Some("Help me write a function".to_string()));
    }

    #[test]
    fn test_extract_title_truncates_long_text() {
        let long_text = "a".repeat(200);
        let entry = make_entry(
            "123",
            "user",
            Some(make_message(
                MessageRole::User,
                Some(MessageContent::Text(long_text)),
            )),
        );

        let title = ConversationImporter::extract_title(&entry);
        assert_eq!(title.as_ref().map(|t| t.len()), Some(100));
    }

    #[test]
    fn test_extract_title_ignores_assistant_messages() {
        let entry = make_entry(
            "123",
            "assistant",
            Some(make_message(
                MessageRole::Assistant,
                Some(MessageContent::Text("I can help with that".to_string())),
            )),
        );

        let title = ConversationImporter::extract_title(&entry);
        assert!(title.is_none());
    }

    #[test]
    fn test_extract_title_handles_empty_content() {
        let entry = make_entry("123", "user", Some(make_message(MessageRole::User, None)));

        let title = ConversationImporter::extract_title(&entry);
        assert!(title.is_none());
    }

    #[test]
    fn test_extract_title_no_message() {
        let entry = make_entry("123", "system", None);

        let title = ConversationImporter::extract_title(&entry);
        assert!(title.is_none());
    }

    #[test]
    fn test_extract_title_from_parts_content() {
        use toolpath_claude::ContentPart;

        let entry = make_entry(
            "123",
            "user",
            Some(make_message(
                MessageRole::User,
                Some(MessageContent::Parts(vec![ContentPart::Text {
                    text: "Hello from parts".to_string(),
                }])),
            )),
        );

        let title = ConversationImporter::extract_title(&entry);
        assert_eq!(title, Some("Hello from parts".to_string()));
    }

    #[test]
    fn test_extract_title_from_parts_no_text() {
        use toolpath_claude::ContentPart;

        let entry = make_entry(
            "123",
            "user",
            Some(make_message(
                MessageRole::User,
                Some(MessageContent::Parts(vec![ContentPart::ToolUse {
                    id: "t1".to_string(),
                    name: "read".to_string(),
                    input: serde_json::json!({}),
                }])),
            )),
        );

        let title = ConversationImporter::extract_title(&entry);
        assert!(
            title.is_none(),
            "ToolUse-only entry should not produce a title"
        );
    }

    #[test]
    fn test_extract_title_parts_truncates() {
        use toolpath_claude::ContentPart;
        let long = "z".repeat(200);

        let entry = make_entry(
            "123",
            "user",
            Some(make_message(
                MessageRole::User,
                Some(MessageContent::Parts(vec![ContentPart::Text {
                    text: long,
                }])),
            )),
        );

        let title = ConversationImporter::extract_title(&entry);
        assert_eq!(title.as_ref().map(|t| t.len()), Some(100));
    }

    #[test]
    fn test_import_stats_default() {
        let stats = ImportStats::default();
        assert_eq!(stats.imported, 0);
        assert_eq!(stats.updated, 0);
        assert_eq!(stats.skipped, 0);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.total(), 0);
    }

    #[test]
    fn test_file_stat_existing_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "hello").unwrap();

        let result = ConversationImporter::file_stat(tmp.path());
        assert!(result.is_some());
        let (mtime, size) = result.unwrap();
        assert!(mtime > 0);
        assert_eq!(size, 5);
    }

    #[test]
    fn test_file_stat_nonexistent() {
        let result = ConversationImporter::file_stat(std::path::Path::new("/nonexistent/file.txt"));
        assert!(result.is_none());
    }

    #[test]
    fn test_import_format_version_is_positive() {
        assert!(IMPORT_FORMAT_VERSION >= 1);
    }
}
