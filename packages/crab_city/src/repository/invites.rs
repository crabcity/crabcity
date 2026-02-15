//! Repository layer for invite tokens.

use anyhow::{Context, Result};
use sqlx::Row;

use super::ConversationRepository;

#[derive(Debug, Clone)]
pub struct StoredInvite {
    pub nonce: Vec<u8>,
    pub issuer: Vec<u8>,
    pub capability: String,
    pub max_uses: i64,
    pub use_count: i64,
    pub expires_at: Option<String>,
    pub chain_blob: Vec<u8>,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

impl StoredInvite {
    pub fn is_valid(&self) -> bool {
        if self.revoked_at.is_some() {
            return false;
        }
        if self.max_uses > 0 && self.use_count >= self.max_uses {
            return false;
        }
        if let Some(ref expires) = self.expires_at {
            // Simple string comparison works for ISO 8601 datetime strings
            let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            if *expires <= now {
                return false;
            }
        }
        true
    }
}

impl ConversationRepository {
    pub async fn store_invite(
        &self,
        nonce: &[u8],
        issuer: &[u8],
        capability: &str,
        max_uses: i64,
        expires_at: Option<&str>,
        chain_blob: &[u8],
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO invites (nonce, issuer, capability, max_uses, expires_at, chain_blob)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(nonce)
        .bind(issuer)
        .bind(capability)
        .bind(max_uses)
        .bind(expires_at)
        .bind(chain_blob)
        .execute(&self.pool)
        .await
        .context("Failed to store invite")?;
        Ok(())
    }

    pub async fn get_invite(&self, nonce: &[u8]) -> Result<Option<StoredInvite>> {
        let row = sqlx::query(
            r#"
            SELECT nonce, issuer, capability, max_uses, use_count,
                   expires_at, chain_blob, created_at, revoked_at
            FROM invites WHERE nonce = ?
            "#,
        )
        .bind(nonce)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(row_to_invite))
    }

    pub async fn increment_use_count(&self, nonce: &[u8]) -> Result<()> {
        sqlx::query("UPDATE invites SET use_count = use_count + 1 WHERE nonce = ?")
            .bind(nonce)
            .execute(&self.pool)
            .await
            .context("Failed to increment invite use count")?;
        Ok(())
    }

    pub async fn revoke_invite(&self, nonce: &[u8]) -> Result<()> {
        sqlx::query(
            "UPDATE invites SET revoked_at = datetime('now') WHERE nonce = ? AND revoked_at IS NULL",
        )
        .bind(nonce)
        .execute(&self.pool)
        .await
        .context("Failed to revoke invite")?;
        Ok(())
    }

    pub async fn list_active_invites(&self) -> Result<Vec<StoredInvite>> {
        let rows = sqlx::query(
            r#"
            SELECT nonce, issuer, capability, max_uses, use_count,
                   expires_at, chain_blob, created_at, revoked_at
            FROM invites
            WHERE revoked_at IS NULL
            ORDER BY created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(row_to_invite).collect())
    }
}

fn row_to_invite(r: sqlx::sqlite::SqliteRow) -> StoredInvite {
    StoredInvite {
        nonce: r.get("nonce"),
        issuer: r.get("issuer"),
        capability: r.get("capability"),
        max_uses: r.get("max_uses"),
        use_count: r.get("use_count"),
        expires_at: r.get("expires_at"),
        chain_blob: r.get("chain_blob"),
        created_at: r.get("created_at"),
        revoked_at: r.get("revoked_at"),
    }
}

#[cfg(test)]
mod tests {
    use crate::repository::test_helpers;

    #[tokio::test]
    async fn store_and_get_invite() {
        let repo = test_helpers::test_repository().await;

        // Use the loopback identity as issuer (seeded by migration)
        let issuer = vec![0u8; 32];
        let nonce = vec![0xaa; 16];

        repo.store_invite(&nonce, &issuer, "collaborate", 5, None, b"chain-data")
            .await
            .unwrap();

        let invite = repo.get_invite(&nonce).await.unwrap().unwrap();
        assert_eq!(invite.capability, "collaborate");
        assert_eq!(invite.max_uses, 5);
        assert_eq!(invite.use_count, 0);
        assert!(invite.revoked_at.is_none());
        assert!(invite.is_valid());
    }

    #[tokio::test]
    async fn invite_not_found() {
        let repo = test_helpers::test_repository().await;
        let result = repo.get_invite(&[0xbb; 16]).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn increment_use_count() {
        let repo = test_helpers::test_repository().await;
        let issuer = vec![0u8; 32];
        let nonce = vec![0xcc; 16];

        repo.store_invite(&nonce, &issuer, "view", 2, None, b"blob")
            .await
            .unwrap();

        repo.increment_use_count(&nonce).await.unwrap();
        let invite = repo.get_invite(&nonce).await.unwrap().unwrap();
        assert_eq!(invite.use_count, 1);
        assert!(invite.is_valid());

        repo.increment_use_count(&nonce).await.unwrap();
        let invite = repo.get_invite(&nonce).await.unwrap().unwrap();
        assert_eq!(invite.use_count, 2);
        assert!(!invite.is_valid()); // max_uses exhausted
    }

    #[tokio::test]
    async fn revoke_invite() {
        let repo = test_helpers::test_repository().await;
        let issuer = vec![0u8; 32];
        let nonce = vec![0xdd; 16];

        repo.store_invite(&nonce, &issuer, "admin", 0, None, b"blob")
            .await
            .unwrap();
        assert!(repo.get_invite(&nonce).await.unwrap().unwrap().is_valid());

        repo.revoke_invite(&nonce).await.unwrap();
        let invite = repo.get_invite(&nonce).await.unwrap().unwrap();
        assert!(invite.revoked_at.is_some());
        assert!(!invite.is_valid());
    }

    #[tokio::test]
    async fn list_active_invites() {
        let repo = test_helpers::test_repository().await;
        let issuer = vec![0u8; 32];

        // Create two active and one revoked
        repo.store_invite(&[1u8; 16], &issuer, "view", 0, None, b"a")
            .await
            .unwrap();
        repo.store_invite(&[2u8; 16], &issuer, "collaborate", 0, None, b"b")
            .await
            .unwrap();
        repo.store_invite(&[3u8; 16], &issuer, "admin", 0, None, b"c")
            .await
            .unwrap();
        repo.revoke_invite(&[3u8; 16]).await.unwrap();

        let active = repo.list_active_invites().await.unwrap();
        assert_eq!(active.len(), 2);
    }

    #[tokio::test]
    async fn unlimited_invite() {
        let repo = test_helpers::test_repository().await;
        let issuer = vec![0u8; 32];
        let nonce = vec![0xee; 16];

        // max_uses = 0 means unlimited
        repo.store_invite(&nonce, &issuer, "view", 0, None, b"blob")
            .await
            .unwrap();

        for _ in 0..10 {
            repo.increment_use_count(&nonce).await.unwrap();
        }

        let invite = repo.get_invite(&nonce).await.unwrap().unwrap();
        assert_eq!(invite.use_count, 10);
        assert!(invite.is_valid()); // still valid because max_uses = 0 (unlimited)
    }
}
