//! Session Discovery
//!
//! Functions for finding and selecting Claude sessions for instances.

use chrono::{DateTime, Utc};
use claude_convo::ClaudeConvo;

/// Find candidate sessions that could belong to this instance
pub fn find_candidate_sessions(
    working_dir: &str,
    created_at: DateTime<Utc>,
) -> Vec<claude_convo::ConversationMetadata> {
    let manager = ClaudeConvo::new();

    match manager.list_conversation_metadata(working_dir) {
        Ok(metadata) => metadata
            .into_iter()
            .filter(|m| m.started_at.map(|s| s >= created_at).unwrap_or(false))
            .collect(),
        Err(_) => vec![],
    }
}
