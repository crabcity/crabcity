use anyhow::Result;
use sqlx::Row;

use super::ConversationRepository;

impl ConversationRepository {
    pub async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let row = sqlx::query("SELECT value FROM server_settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.get("value")))
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO server_settings (key, value, updated_at) VALUES (?, ?, unixepoch())",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::repository::test_helpers;

    #[tokio::test]
    async fn get_nonexistent_setting() {
        let repo = test_helpers::test_repository().await;
        let result = repo.get_setting("nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn set_and_get_setting() {
        let repo = test_helpers::test_repository().await;
        repo.set_setting("theme", "dark").await.unwrap();

        let value = repo.get_setting("theme").await.unwrap().unwrap();
        assert_eq!(value, "dark");
    }

    #[tokio::test]
    async fn update_setting() {
        let repo = test_helpers::test_repository().await;
        repo.set_setting("theme", "dark").await.unwrap();
        repo.set_setting("theme", "light").await.unwrap();

        let value = repo.get_setting("theme").await.unwrap().unwrap();
        assert_eq!(value, "light");
    }
}
