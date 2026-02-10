use anyhow::{Context, Result};
use sqlx::Row;

use crate::models::{InputAttribution, attribution_content_matches};

use super::ConversationRepository;

impl ConversationRepository {
    pub async fn record_input_attribution(&self, attr: &InputAttribution) -> Result<i64> {
        let result = sqlx::query(
            r#"
            INSERT INTO input_attributions (instance_id, user_id, display_name, timestamp, entry_uuid, content_preview, task_id)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&attr.instance_id)
        .bind(&attr.user_id)
        .bind(&attr.display_name)
        .bind(attr.timestamp)
        .bind(&attr.entry_uuid)
        .bind(&attr.content_preview)
        .bind(attr.task_id)
        .execute(&self.pool)
        .await
        .context("Failed to record input attribution")?;
        Ok(result.last_insert_rowid())
    }

    pub async fn correlate_attribution(
        &self,
        instance_id: &str,
        entry_uuid: &str,
        timestamp: i64,
        entry_content: Option<&str>,
    ) -> Result<Option<InputAttribution>> {
        // Strategy: content match first (reliable), timestamp fallback (legacy).
        //
        // The timestamp-only path is vulnerable to label swapping when two users
        // type within seconds of each other — the entry with the closest timestamp
        // might match the WRONG attribution. Content matching avoids this entirely.

        // 1. Try content-based match within the time window
        if let Some(content) = entry_content {
            if !content.trim().is_empty() {
                // Fetch candidates within the time window
                let rows = sqlx::query(
                    r#"
                    SELECT id, instance_id, user_id, display_name, timestamp, entry_uuid, content_preview, task_id
                    FROM input_attributions
                    WHERE instance_id = ? AND entry_uuid IS NULL AND ABS(timestamp - ?) <= 30
                    ORDER BY ABS(timestamp - ?) ASC
                    "#,
                )
                .bind(instance_id)
                .bind(timestamp)
                .bind(timestamp)
                .fetch_all(&self.pool)
                .await?;

                // Find the first row whose content_preview matches
                for r in &rows {
                    let preview: Option<String> = r.get("content_preview");
                    if let Some(ref preview) = preview {
                        if attribution_content_matches(preview, content) {
                            let attr_id: i64 = r.get("id");
                            // Claim it
                            sqlx::query(
                                "UPDATE input_attributions SET entry_uuid = ? WHERE id = ?",
                            )
                            .bind(entry_uuid)
                            .bind(attr_id)
                            .execute(&self.pool)
                            .await?;

                            return Ok(Some(InputAttribution {
                                id: r.get("id"),
                                instance_id: r.get("instance_id"),
                                user_id: r.get("user_id"),
                                display_name: r.get("display_name"),
                                timestamp: r.get("timestamp"),
                                entry_uuid: Some(entry_uuid.to_string()),
                                content_preview: r.get("content_preview"),
                                task_id: r.get("task_id"),
                            }));
                        }
                    }
                }
            }
        }

        // No timestamp-only fallback. Without content to match against,
        // a closest-timestamp heuristic can swap labels when two users type
        // concurrently, and the UPDATE permanently poisons future lookups.
        // "Unknown" is more honest than "wrong".
        Ok(None)
    }

    /// Look up an attribution that was already correlated to a specific entry UUID.
    pub async fn get_attribution_by_entry_uuid(
        &self,
        entry_uuid: &str,
    ) -> Result<Option<InputAttribution>> {
        let row = sqlx::query(
            r#"
            SELECT id, instance_id, user_id, display_name, timestamp, entry_uuid, content_preview, task_id
            FROM input_attributions
            WHERE entry_uuid = ?
            LIMIT 1
            "#,
        )
        .bind(entry_uuid)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| InputAttribution {
            id: r.get("id"),
            instance_id: r.get("instance_id"),
            user_id: r.get("user_id"),
            display_name: r.get("display_name"),
            timestamp: r.get("timestamp"),
            entry_uuid: r.get("entry_uuid"),
            content_preview: r.get("content_preview"),
            task_id: r.get("task_id"),
        }))
    }

    /// Batch-fetch attributions for a set of entry UUIDs.
    /// Returns only entries that have a correlated attribution (entry_uuid IS NOT NULL).
    pub async fn get_attributions_for_entry_uuids(
        &self,
        entry_uuids: &[&str],
    ) -> Result<Vec<crate::models::EntryAttribution>> {
        if entry_uuids.is_empty() {
            return Ok(vec![]);
        }

        // SQLite doesn't support array binds, so build a comma-separated placeholder list
        let placeholders: Vec<&str> = entry_uuids.iter().map(|_| "?").collect();
        let query = format!(
            "SELECT entry_uuid, user_id, display_name, task_id FROM input_attributions WHERE entry_uuid IN ({})",
            placeholders.join(", ")
        );

        let mut q = sqlx::query(&query);
        for uuid in entry_uuids {
            q = q.bind(uuid);
        }

        let rows = q.fetch_all(&self.pool).await?;

        Ok(rows
            .into_iter()
            .map(|r| crate::models::EntryAttribution {
                entry_uuid: r.get("entry_uuid"),
                user_id: r.get("user_id"),
                display_name: r.get("display_name"),
                task_id: r.get("task_id"),
            })
            .collect())
    }

    /// Get or correlate attribution for an entry.
    /// First checks for an already-correlated attribution (by entry_uuid),
    /// then falls back to content+timestamp correlation for new entries.
    pub async fn get_or_correlate_attribution(
        &self,
        instance_id: &str,
        entry_uuid: &str,
        timestamp: i64,
        entry_content: Option<&str>,
    ) -> Result<Option<InputAttribution>> {
        // Fast path: already correlated
        if let Some(attr) = self.get_attribution_by_entry_uuid(entry_uuid).await? {
            return Ok(Some(attr));
        }
        // Slow path: correlate by content + timestamp
        self.correlate_attribution(instance_id, entry_uuid, timestamp, entry_content)
            .await
    }
}
