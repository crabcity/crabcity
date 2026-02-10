use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub session_id: Option<String>,
    pub instance_id: String,
    pub title: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub is_public: bool,
    pub is_deleted: bool,
    pub metadata_json: Option<String>,
    pub file_hash: Option<String>,
    pub file_mtime: Option<i64>,
    /// Import format version - triggers re-import when import logic changes
    pub import_version: Option<i64>,
}

impl Conversation {
    pub fn new(id: String, instance_id: String) -> Self {
        let now = Utc::now().timestamp();
        Self {
            id,
            session_id: None,
            instance_id,
            title: None,
            created_at: now,
            updated_at: now,
            is_public: false,
            is_deleted: false,
            metadata_json: None,
            file_hash: None,
            file_mtime: None,
            import_version: None,
        }
    }

    pub fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ConversationEntry {
    pub id: Option<i64>, // None for new entries
    pub conversation_id: String,
    pub entry_uuid: String,
    pub parent_uuid: Option<String>,
    pub entry_type: String,
    pub role: Option<String>,
    pub content: Option<String>,
    pub timestamp: String,
    pub raw_json: String,
    pub token_count: Option<i32>,
    pub model: Option<String>,
}

impl ConversationEntry {
    pub fn from_claude_entry(
        conversation_id: String,
        entry: &claude_convo::ConversationEntry,
    ) -> Self {
        // Extract role and content from the message if present
        let (role, content, model) = if let Some(msg) = &entry.message {
            let role = match msg.role {
                claude_convo::MessageRole::User => Some("user".to_string()),
                claude_convo::MessageRole::Assistant => Some("assistant".to_string()),
                claude_convo::MessageRole::System => Some("system".to_string()),
            };

            let content = match &msg.content {
                Some(claude_convo::MessageContent::Text(text)) => Some(text.clone()),
                Some(claude_convo::MessageContent::Parts(parts)) => {
                    // Extract text content from parts
                    let texts: Vec<String> = parts
                        .iter()
                        .filter_map(|part| match part {
                            claude_convo::ContentPart::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .collect();
                    if texts.is_empty() {
                        None
                    } else {
                        Some(texts.join("\n"))
                    }
                }
                None => None,
            };

            (role, content, msg.model.clone())
        } else {
            (None, None, None)
        };

        // Serialize the full entry as JSON
        let raw_json = serde_json::to_string(entry).unwrap_or_default();

        Self {
            id: None,
            conversation_id,
            entry_uuid: entry.uuid.clone(),
            parent_uuid: entry.parent_uuid.clone(),
            entry_type: entry.entry_type.clone(),
            role,
            content,
            timestamp: entry.timestamp.clone(),
            raw_json,
            token_count: None, // Could extract from usage if needed
            model,
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Comment {
    pub id: Option<i64>,
    pub conversation_id: String,
    pub entry_uuid: Option<String>,
    pub author: String,
    pub content: String,
    pub created_at: i64,
    pub updated_at: Option<i64>,
}

impl Comment {
    pub fn new(
        conversation_id: String,
        content: String,
        author: Option<String>,
        entry_uuid: Option<String>,
    ) -> Self {
        Self {
            id: None,
            conversation_id,
            entry_uuid,
            author: author.unwrap_or_else(|| "anonymous".to_string()),
            content,
            created_at: Utc::now().timestamp(),
            updated_at: None,
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ConversationShare {
    pub id: Option<i64>,
    pub conversation_id: String,
    pub share_token: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub access_count: i32,
    pub max_access_count: Option<i32>,
    pub password_hash: Option<String>,
}

impl ConversationShare {
    pub fn new(conversation_id: String, expires_in_days: Option<i32>) -> Self {
        let share_token = uuid::Uuid::new_v4().to_string();
        let created_at = Utc::now().timestamp();
        let expires_at = expires_in_days.map(|days| created_at + (days as i64 * 24 * 60 * 60));

        Self {
            id: None,
            conversation_id,
            share_token,
            title: None,
            description: None,
            created_at,
            expires_at,
            access_count: 0,
            max_access_count: None,
            password_hash: None,
        }
    }

    pub fn is_expired(&self) -> bool {
        if let Some(expires) = self.expires_at {
            Utc::now().timestamp() > expires
        } else {
            false
        }
    }

    pub fn is_access_limit_reached(&self) -> bool {
        if let Some(max) = self.max_access_count {
            self.access_count >= max
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub color: Option<String>,
}

/// Lightweight attribution info for a conversation entry (used in REST API responses).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryAttribution {
    pub entry_uuid: String,
    pub user_id: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationWithEntries {
    pub conversation: Conversation,
    pub entries: Vec<ConversationEntry>,
    pub comments: Vec<Comment>,
    pub tags: Vec<Tag>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributions: Vec<EntryAttribution>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub id: String,
    pub title: Option<String>,
    pub instance_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub entry_count: i32,
    pub is_public: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMatchEntry {
    pub entry_uuid: String,
    pub role: Option<String>,
    pub snippet: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResultConversation {
    pub id: String,
    pub title: Option<String>,
    pub instance_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub entry_count: i32,
    pub match_count: i32,
    pub matches: Vec<SearchMatchEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
    pub total_pages: i64,
}

// === Auth models ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub is_admin: bool,
    pub is_disabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Public user info (no password hash)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub is_admin: bool,
}

impl From<User> for UserInfo {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            username: u.username,
            display_name: u.display_name,
            is_admin: u.is_admin,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    pub user_id: String,
    pub csrf_token: String,
    pub expires_at: i64,
    pub last_active_at: i64,
    pub user_agent: Option<String>,
    pub ip_address: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstancePermission {
    pub instance_id: String,
    pub user_id: String,
    pub role: String, // "owner" or "collaborator"
    pub granted_at: i64,
    pub granted_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceInvitation {
    pub invite_token: String,
    pub instance_id: String,
    pub created_by: String,
    pub role: String,
    pub max_uses: Option<i32>,
    pub use_count: i32,
    pub expires_at: Option<i64>,
    pub created_at: i64,
}

impl InstanceInvitation {
    pub fn is_expired(&self) -> bool {
        if let Some(expires) = self.expires_at {
            Utc::now().timestamp() > expires
        } else {
            false
        }
    }

    pub fn is_used_up(&self) -> bool {
        if let Some(max) = self.max_uses {
            self.use_count >= max
        } else {
            false
        }
    }
}

// === Server Invite models ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInvite {
    pub token: String,
    pub created_by: String,
    pub label: Option<String>,
    pub max_uses: Option<i32>,
    pub use_count: i32,
    pub expires_at: Option<i64>,
    pub revoked: bool,
    pub created_at: i64,
}

impl ServerInvite {
    pub fn is_expired(&self) -> bool {
        if let Some(expires) = self.expires_at {
            Utc::now().timestamp() > expires
        } else {
            false
        }
    }

    pub fn is_used_up(&self) -> bool {
        if let Some(max) = self.max_uses {
            self.use_count >= max
        } else {
            false
        }
    }

    pub fn is_valid(&self) -> bool {
        !self.revoked && !self.is_expired() && !self.is_used_up()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInviteWithAcceptors {
    pub invite: ServerInvite,
    pub acceptors: Vec<InviteAcceptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InviteAcceptor {
    pub user_id: String,
    pub username: String,
    pub display_name: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputAttribution {
    pub id: Option<i64>,
    pub instance_id: String,
    pub user_id: String,
    pub display_name: String,
    pub timestamp: i64,
    pub entry_uuid: Option<String>,
    pub content_preview: Option<String>,
    pub task_id: Option<i64>,
}

/// Max characters to compare when content-matching attributions.
/// Both sides may be truncated independently, so we use prefix matching.
pub const ATTRIBUTION_CONTENT_PREFIX_LEN: usize = 100;

/// Normalize content for attribution matching: trim whitespace, take first N chars.
pub fn normalize_attribution_content(content: &str) -> String {
    content
        .trim()
        .chars()
        .take(ATTRIBUTION_CONTENT_PREFIX_LEN)
        .collect()
}

/// Check if two content strings match for attribution purposes.
/// Uses prefix matching because either side may be truncated at different lengths.
///
/// This is the single source of truth for "does this input match this conversation entry?"
/// Both the in-process queue and the DB correlation path use this.
// === Chat models ===

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: Option<i64>,
    pub uuid: String,
    pub scope: String, // "global" or an instance_id
    pub user_id: String,
    pub display_name: String,
    pub content: String,
    pub created_at: i64,
    pub forwarded_from: Option<String>, // original scope if forwarded
    pub topic: Option<String>,          // Zulip-style topic label
}

/// Summary of a chat topic for topic listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTopicSummary {
    pub topic: String,
    pub message_count: i64,
    pub latest_at: i64,
}

// === Task models ===

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Task {
    pub id: Option<i64>,
    pub uuid: String,
    pub title: String,
    pub body: Option<String>,
    pub status: String,
    pub priority: i32,
    pub instance_id: Option<String>,
    pub creator_id: Option<String>,
    pub creator_name: String,
    pub sort_order: f64,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
    pub is_deleted: bool,
    pub sent_text: Option<String>,
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDispatch {
    pub id: Option<i64>,
    pub task_id: i64,
    pub instance_id: String,
    pub sent_text: String,
    pub conversation_id: Option<String>,
    pub sent_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskWithTags {
    #[serde(flatten)]
    pub task: Task,
    pub tags: Vec<Tag>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dispatches: Vec<TaskDispatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    pub title: String,
    pub body: Option<String>,
    pub status: Option<String>,
    pub priority: Option<i32>,
    pub instance_id: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateTaskRequest {
    pub title: Option<String>,
    pub body: Option<String>,
    pub status: Option<String>,
    pub priority: Option<i32>,
    pub instance_id: Option<String>,
    pub sort_order: Option<f64>,
    pub sent_text: Option<String>,
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskListFilters {
    pub status: Option<String>,
    pub instance_id: Option<String>,
    pub tag: Option<String>,
    pub search: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrateTaskItem {
    pub title: String,
    pub instance_id: Option<String>,
    pub created_at: Option<i64>,
}

pub fn attribution_content_matches(stored_content: &str, entry_content: &str) -> bool {
    let stored = normalize_attribution_content(stored_content);
    let entry = normalize_attribution_content(entry_content);
    if stored.is_empty() || entry.is_empty() {
        return false;
    }
    stored.starts_with(&entry) || entry.starts_with(&stored)
}

#[cfg(test)]
mod attribution_tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        assert!(attribution_content_matches(
            "fix the auth bug",
            "fix the auth bug"
        ));
    }

    #[test]
    fn test_prefix_match_stored_longer() {
        // Stored has full content, entry was truncated
        assert!(attribution_content_matches(
            "fix the auth bug in login.rs and also update the tests",
            "fix the auth bug in login.rs"
        ));
    }

    #[test]
    fn test_prefix_match_entry_longer() {
        // Stored was truncated (content_preview is 100 chars), entry has full content
        assert!(attribution_content_matches(
            "fix the auth bug",
            "fix the auth bug in login.rs and also update the tests"
        ));
    }

    #[test]
    fn test_no_match_different_content() {
        assert!(!attribution_content_matches(
            "fix the auth bug",
            "add unit tests"
        ));
    }

    #[test]
    fn test_no_match_empty() {
        assert!(!attribution_content_matches("", "fix the auth bug"));
        assert!(!attribution_content_matches("fix the auth bug", ""));
        assert!(!attribution_content_matches("", ""));
    }

    #[test]
    fn test_whitespace_trimming() {
        assert!(attribution_content_matches(
            "  fix the auth bug  ",
            "fix the auth bug"
        ));
        assert!(attribution_content_matches(
            "fix the auth bug",
            "  fix the auth bug  "
        ));
    }

    #[test]
    fn test_no_false_swap_different_messages() {
        // The critical scenario: two users type different things close together.
        // "fix auth bug" should NOT match "add unit tests".
        let user_a_input = "fix the authentication bug in the login flow";
        let user_b_input = "add unit tests for the payment module";

        // Entry content from Claude's conversation
        let entry_a = "fix the authentication bug in the login flow";
        let entry_b = "add unit tests for the payment module";

        // A matches A, not B
        assert!(attribution_content_matches(user_a_input, entry_a));
        assert!(!attribution_content_matches(user_a_input, entry_b));

        // B matches B, not A
        assert!(attribution_content_matches(user_b_input, entry_b));
        assert!(!attribution_content_matches(user_b_input, entry_a));
    }

    #[test]
    fn test_long_content_truncation() {
        // Content longer than ATTRIBUTION_CONTENT_PREFIX_LEN
        let long_a = "a".repeat(200);
        let long_b = format!("{}bbb", "a".repeat(200));
        // Both truncate to the same first 100 chars
        assert!(attribution_content_matches(&long_a, &long_b));
    }

    #[test]
    fn test_normalize_preserves_meaningful_content() {
        let normalized = normalize_attribution_content("  hello world  ");
        assert_eq!(normalized, "hello world");
    }
}
