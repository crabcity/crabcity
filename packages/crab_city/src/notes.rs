use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub session_id: String,
    pub content: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Optional position in conversation (entry UUID)
    pub entry_id: Option<String>,
}

/// Simple file-based notes storage
pub struct NotesStorage {
    notes_dir: PathBuf,
    cache: Arc<RwLock<HashMap<String, Vec<Note>>>>,
}

impl NotesStorage {
    pub fn new(data_dir: &PathBuf) -> Result<Self> {
        let notes_dir = data_dir.join("notes");
        fs::create_dir_all(&notes_dir)?;

        let mut cache = HashMap::new();

        // Load existing notes
        if let Ok(entries) = fs::read_dir(&notes_dir) {
            for entry in entries.flatten() {
                if let Some(session_id) = entry.file_name().to_str() {
                    if session_id.ends_with(".json") {
                        let session_id = session_id.trim_end_matches(".json");
                        if let Ok(content) = fs::read_to_string(entry.path()) {
                            if let Ok(notes) = serde_json::from_str::<Vec<Note>>(&content) {
                                cache.insert(session_id.to_string(), notes);
                            }
                        }
                    }
                }
            }
        }

        info!("Loaded notes for {} conversations", cache.len());

        Ok(Self {
            notes_dir,
            cache: Arc::new(RwLock::new(cache)),
        })
    }

    pub async fn add_note(
        &self,
        session_id: &str,
        content: String,
        entry_id: Option<String>,
    ) -> Result<Note> {
        let note = Note {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            content,
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
            entry_id,
        };

        let mut cache = self.cache.write().await;
        let notes = cache.entry(session_id.to_string()).or_insert_with(Vec::new);
        notes.push(note.clone());

        // Save to disk
        self.save_notes(session_id, notes).await?;

        debug!("Added note {} for session {}", note.id, session_id);
        Ok(note)
    }

    pub async fn update_note(
        &self,
        session_id: &str,
        note_id: &str,
        content: String,
    ) -> Result<()> {
        let mut cache = self.cache.write().await;
        if let Some(notes) = cache.get_mut(session_id) {
            if let Some(note) = notes.iter_mut().find(|n| n.id == note_id) {
                note.content = content;
                note.updated_at = chrono::Utc::now().timestamp();

                // Save to disk
                self.save_notes(session_id, notes).await?;
                debug!("Updated note {} for session {}", note_id, session_id);
            }
        }
        Ok(())
    }

    pub async fn delete_note(&self, session_id: &str, note_id: &str) -> Result<()> {
        let mut cache = self.cache.write().await;
        if let Some(notes) = cache.get_mut(session_id) {
            notes.retain(|n| n.id != note_id);

            // Save to disk
            self.save_notes(session_id, notes).await?;
            debug!("Deleted note {} from session {}", note_id, session_id);
        }
        Ok(())
    }

    pub async fn get_notes(&self, session_id: &str) -> Vec<Note> {
        let cache = self.cache.read().await;
        cache.get(session_id).cloned().unwrap_or_default()
    }

    async fn save_notes(&self, session_id: &str, notes: &[Note]) -> Result<()> {
        let file_path = self.notes_dir.join(format!("{}.json", session_id));
        let content = serde_json::to_string_pretty(notes)?;
        tokio::fs::write(file_path, content).await?;
        Ok(())
    }
}
