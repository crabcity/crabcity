//! Session Discovery
//!
//! Functions for finding and selecting Claude sessions for instances.

use chrono::{DateTime, Utc};
use toolpath_convo::{ConversationMeta, ConversationProvider};

/// Find candidate sessions that could belong to this instance
pub fn find_candidate_sessions(
    provider: &dyn ConversationProvider,
    working_dir: &str,
    created_at: DateTime<Utc>,
) -> Vec<ConversationMeta> {
    match provider.list_metadata(working_dir) {
        Ok(metadata) => metadata
            .into_iter()
            .filter(|m| m.started_at.map(|s| s >= created_at).unwrap_or(false))
            .collect(),
        Err(_) => vec![],
    }
}
