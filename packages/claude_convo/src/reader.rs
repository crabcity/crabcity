use crate::error::{ConvoError, Result};
use crate::types::{Conversation, ConversationEntry, HistoryEntry};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

pub struct ConversationReader;

impl ConversationReader {
    pub fn read_conversation<P: AsRef<Path>>(path: P) -> Result<Conversation> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(ConvoError::ConversationNotFound(path.display().to_string()));
        }

        let file = File::open(path)?;
        let reader = BufReader::new(file);

        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| ConvoError::InvalidFormat(path.to_path_buf()))?
            .to_string();

        let mut conversation = Conversation::new(session_id);

        for (line_num, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            // Try to parse as a conversation entry
            match serde_json::from_str::<ConversationEntry>(&line) {
                Ok(entry) => {
                    // Only add entries with valid UUIDs (skip metadata entries)
                    if !entry.uuid.is_empty() {
                        conversation.add_entry(entry);
                    }
                }
                Err(_) => {
                    // Try to parse as a generic JSON value to check the type
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                        if let Some(entry_type) = value.get("type").and_then(|t| t.as_str()) {
                            // Known metadata types we can safely ignore
                            if entry_type == "file-history-snapshot" {
                                // Silently skip file snapshots
                                continue;
                            }
                        }
                    }

                    // Only warn about truly unexpected parse failures
                    if line_num < 5 || std::env::var("CLAUDE_CLI_DEBUG").is_ok() {
                        eprintln!(
                            "Warning: Failed to parse line {} in {:?}: entry type not recognized",
                            line_num + 1,
                            path.file_name().unwrap_or_default()
                        );
                    }
                }
            }
        }

        Ok(conversation)
    }

    pub fn read_conversation_metadata<P: AsRef<Path>>(
        path: P,
    ) -> Result<crate::types::ConversationMetadata> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(ConvoError::ConversationNotFound(path.display().to_string()));
        }

        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| ConvoError::InvalidFormat(path.to_path_buf()))?
            .to_string();

        let file = File::open(path)?;
        let reader = BufReader::new(file);

        let mut message_count = 0;
        let mut started_at = None;
        let mut last_activity = None;
        let mut project_path = String::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            // Try to parse as ConversationEntry, skip if it fails (likely a metadata entry)
            if let Ok(entry) = serde_json::from_str::<ConversationEntry>(&line) {
                // Only process entries with valid UUIDs
                if !entry.uuid.is_empty() {
                    if entry.message.is_some() {
                        message_count += 1;
                    }

                    if project_path.is_empty() {
                        if let Some(cwd) = entry.cwd {
                            project_path = cwd;
                        }
                    }

                    if !entry.timestamp.is_empty() {
                        if let Ok(timestamp) =
                            entry.timestamp.parse::<chrono::DateTime<chrono::Utc>>()
                        {
                            if started_at.is_none() || Some(timestamp) < started_at {
                                started_at = Some(timestamp);
                            }
                            if last_activity.is_none() || Some(timestamp) > last_activity {
                                last_activity = Some(timestamp);
                            }
                        }
                    }
                }
            }
        }

        Ok(crate::types::ConversationMetadata {
            session_id,
            project_path,
            file_path: path.to_path_buf(),
            message_count,
            started_at,
            last_activity,
        })
    }

    pub fn read_history<P: AsRef<Path>>(path: P) -> Result<Vec<HistoryEntry>> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut history = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<HistoryEntry>(&line) {
                Ok(entry) => history.push(entry),
                Err(e) => {
                    eprintln!("Warning: Failed to parse history line: {}", e);
                }
            }
        }

        Ok(history)
    }

    /// Read conversation entries starting from a byte offset.
    /// Returns the new entries and the new byte offset (end of file position).
    ///
    /// This is used for incremental reading - call with offset=0 initially,
    /// then use the returned offset for subsequent calls to only read new entries.
    pub fn read_from_offset<P: AsRef<Path>>(
        path: P,
        byte_offset: u64,
    ) -> Result<(Vec<ConversationEntry>, u64)> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(ConvoError::ConversationNotFound(path.display().to_string()));
        }

        let mut file = File::open(path)?;
        let file_len = file.metadata()?.len();

        // If offset is beyond file length, file may have been truncated/rotated
        // Return empty with current file length as new offset
        if byte_offset > file_len {
            return Ok((Vec::new(), file_len));
        }

        // Seek to the offset
        file.seek(SeekFrom::Start(byte_offset))?;

        let reader = BufReader::new(file);
        let mut entries = Vec::new();
        let mut current_offset = byte_offset;

        for line in reader.lines() {
            let line = line?;
            // Track offset: line length + newline character
            current_offset += line.len() as u64 + 1;

            if line.trim().is_empty() {
                continue;
            }

            // Try to parse as a conversation entry
            if let Ok(entry) = serde_json::from_str::<ConversationEntry>(&line) {
                // Only add entries with valid UUIDs (skip metadata entries)
                if !entry.uuid.is_empty() {
                    entries.push(entry);
                }
            }
            // Silently skip unparseable lines (metadata, file-history-snapshot, etc.)
        }

        Ok((entries, current_offset))
    }

    /// Get the current file size for a conversation file.
    /// Useful for checking if a file has grown since last read.
    pub fn file_size<P: AsRef<Path>>(path: P) -> Result<u64> {
        let path = path.as_ref();
        if !path.exists() {
            return Err(ConvoError::ConversationNotFound(path.display().to_string()));
        }
        Ok(std::fs::metadata(path)?.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_read_conversation() {
        let mut temp = NamedTempFile::new().unwrap();
        writeln!(
            temp,
            r#"{{"type":"user","uuid":"123","timestamp":"2024-01-01T00:00:00Z","sessionId":"test","message":{{"role":"user","content":"Hello"}}}}"#
        )
        .unwrap();
        writeln!(
            temp,
            r#"{{"type":"assistant","uuid":"456","timestamp":"2024-01-01T00:00:01Z","sessionId":"test","message":{{"role":"assistant","content":"Hi there"}}}}"#
        )
        .unwrap();
        temp.flush().unwrap();

        let convo = ConversationReader::read_conversation(temp.path()).unwrap();
        assert_eq!(convo.entries.len(), 2);
        assert_eq!(convo.message_count(), 2);
        assert_eq!(convo.user_messages().len(), 1);
        assert_eq!(convo.assistant_messages().len(), 1);
    }

    #[test]
    fn test_read_history() {
        let mut temp = NamedTempFile::new().unwrap();
        writeln!(
            temp,
            r#"{{"display":"Test query","pastedContents":{{}},"timestamp":1234567890,"project":"/test/project","sessionId":"session-123"}}"#
        )
        .unwrap();
        temp.flush().unwrap();

        let history = ConversationReader::read_history(temp.path()).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].display, "Test query");
        assert_eq!(history[0].project, Some("/test/project".to_string()));
    }
}
