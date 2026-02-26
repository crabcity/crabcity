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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use toolpath_convo::{ConversationView, ConvoError};

    /// Mock provider that returns a fixed list of metadata.
    struct MockProvider {
        metadata: Vec<ConversationMeta>,
    }

    impl ConversationProvider for MockProvider {
        fn list_conversations(&self, _project: &str) -> toolpath_convo::Result<Vec<String>> {
            Ok(self.metadata.iter().map(|m| m.id.clone()).collect())
        }

        fn load_conversation(
            &self,
            _project: &str,
            _conversation_id: &str,
        ) -> toolpath_convo::Result<ConversationView> {
            Err(ConvoError::Provider("not implemented".into()))
        }

        fn load_metadata(
            &self,
            _project: &str,
            _conversation_id: &str,
        ) -> toolpath_convo::Result<ConversationMeta> {
            Err(ConvoError::Provider("not implemented".into()))
        }

        fn list_metadata(&self, _project: &str) -> toolpath_convo::Result<Vec<ConversationMeta>> {
            Ok(self.metadata.clone())
        }
    }

    /// Mock provider that always errors.
    struct ErrorProvider;

    impl ConversationProvider for ErrorProvider {
        fn list_conversations(&self, _: &str) -> toolpath_convo::Result<Vec<String>> {
            Err(ConvoError::Provider("boom".into()))
        }
        fn load_conversation(&self, _: &str, _: &str) -> toolpath_convo::Result<ConversationView> {
            Err(ConvoError::Provider("boom".into()))
        }
        fn load_metadata(&self, _: &str, _: &str) -> toolpath_convo::Result<ConversationMeta> {
            Err(ConvoError::Provider("boom".into()))
        }
        fn list_metadata(&self, _: &str) -> toolpath_convo::Result<Vec<ConversationMeta>> {
            Err(ConvoError::Provider("boom".into()))
        }
    }

    fn meta(id: &str, started_at: Option<DateTime<Utc>>) -> ConversationMeta {
        ConversationMeta {
            id: id.to_string(),
            started_at,
            last_activity: None,
            message_count: 1,
            file_path: None,
        }
    }

    #[test]
    fn filters_by_created_at() {
        let t1 = Utc.with_ymd_and_hms(2024, 6, 15, 10, 0, 0).unwrap();
        let t2 = Utc.with_ymd_and_hms(2024, 6, 15, 10, 0, 5).unwrap();
        let t3 = Utc.with_ymd_and_hms(2024, 6, 15, 10, 0, 10).unwrap();

        let provider = MockProvider {
            metadata: vec![meta("old-session", Some(t1)), meta("new-session", Some(t3))],
        };

        // created_at = t2, so only t3 (>= t2) should match
        let results = find_candidate_sessions(&provider, "/test", t2);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "new-session");
    }

    #[test]
    fn exact_timestamp_match_included() {
        let t = Utc.with_ymd_and_hms(2024, 6, 15, 10, 0, 0).unwrap();

        let provider = MockProvider {
            metadata: vec![meta("exact", Some(t))],
        };

        let results = find_candidate_sessions(&provider, "/test", t);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "exact");
    }

    #[test]
    fn none_started_at_excluded() {
        let t = Utc.with_ymd_and_hms(2024, 6, 15, 10, 0, 0).unwrap();

        let provider = MockProvider {
            metadata: vec![meta("no-timestamp", None)],
        };

        let results = find_candidate_sessions(&provider, "/test", t);
        assert!(results.is_empty());
    }

    #[test]
    fn provider_error_returns_empty() {
        let t = Utc.with_ymd_and_hms(2024, 6, 15, 10, 0, 0).unwrap();
        let results = find_candidate_sessions(&ErrorProvider, "/test", t);
        assert!(results.is_empty());
    }

    #[test]
    fn empty_metadata_returns_empty() {
        let t = Utc.with_ymd_and_hms(2024, 6, 15, 10, 0, 0).unwrap();
        let provider = MockProvider { metadata: vec![] };
        let results = find_candidate_sessions(&provider, "/test", t);
        assert!(results.is_empty());
    }

    #[test]
    fn returns_convo_meta_id_field() {
        // Regression: old code used .session_id, new uses .id
        let t = Utc.with_ymd_and_hms(2024, 6, 15, 10, 0, 0).unwrap();
        let provider = MockProvider {
            metadata: vec![meta("abc-123-def", Some(t))],
        };
        let results = find_candidate_sessions(&provider, "/test", t);
        assert_eq!(results[0].id, "abc-123-def");
    }
}
