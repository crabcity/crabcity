use crate::error::Result;
use crate::paths::PathResolver;
use crate::reader::ConversationReader;
use crate::types::{Conversation, ConversationMetadata, HistoryEntry};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ConvoIO {
    resolver: PathResolver,
}

impl ConvoIO {
    pub fn new() -> Self {
        Self {
            resolver: PathResolver::new(),
        }
    }

    pub fn with_resolver(resolver: PathResolver) -> Self {
        Self { resolver }
    }

    pub fn resolver(&self) -> &PathResolver {
        &self.resolver
    }

    pub fn read_conversation(&self, project_path: &str, session_id: &str) -> Result<Conversation> {
        let path = self.resolver.conversation_file(project_path, session_id)?;
        ConversationReader::read_conversation(&path)
    }

    pub fn read_conversation_metadata(
        &self,
        project_path: &str,
        session_id: &str,
    ) -> Result<ConversationMetadata> {
        let path = self.resolver.conversation_file(project_path, session_id)?;
        ConversationReader::read_conversation_metadata(&path)
    }

    pub fn list_conversations(&self, project_path: &str) -> Result<Vec<String>> {
        self.resolver.list_conversations(project_path)
    }

    pub fn list_conversation_metadata(
        &self,
        project_path: &str,
    ) -> Result<Vec<ConversationMetadata>> {
        let sessions = self.list_conversations(project_path)?;
        let mut metadata = Vec::new();

        for session_id in sessions {
            match self.read_conversation_metadata(project_path, &session_id) {
                Ok(meta) => metadata.push(meta),
                Err(e) => {
                    eprintln!("Warning: Failed to read metadata for {}: {}", session_id, e);
                }
            }
        }

        metadata.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
        Ok(metadata)
    }

    pub fn list_projects(&self) -> Result<Vec<String>> {
        self.resolver.list_project_dirs()
    }

    pub fn read_history(&self) -> Result<Vec<HistoryEntry>> {
        let path = self.resolver.history_file()?;
        ConversationReader::read_history(&path)
    }

    pub fn exists(&self) -> bool {
        self.resolver.exists()
    }

    pub fn claude_dir_path(&self) -> Result<PathBuf> {
        self.resolver.claude_dir()
    }

    pub fn conversation_exists(&self, project_path: &str, session_id: &str) -> Result<bool> {
        let path = self.resolver.conversation_file(project_path, session_id)?;
        Ok(path.exists())
    }

    pub fn project_exists(&self, project_path: &str) -> bool {
        self.resolver
            .project_dir(project_path)
            .map(|p| p.exists())
            .unwrap_or(false)
    }
}

impl Default for ConvoIO {
    fn default() -> Self {
        Self::new()
    }
}
