//! Repository layer for federation: federated accounts (host side) and remote
//! Crab Cities (home side).

use anyhow::{Context, Result};
use sqlx::Row;

use super::ConversationRepository;

// --- Row types ---

/// A federated account on this server for an identity from another server.
#[derive(Debug, Clone)]
pub struct FederatedAccount {
    pub account_key: Vec<u8>,
    pub display_name: String,
    pub home_node_id: Option<Vec<u8>>,
    pub home_name: Option<String>,
    pub access: String,
    pub state: String,
    pub created_by: Vec<u8>,
    pub created_at: String,
    pub updated_at: String,
}

/// A remote Crab City that a local user can connect to.
#[derive(Debug, Clone)]
pub struct RemoteCrabCity {
    pub host_node_id: Vec<u8>,
    pub account_key: Vec<u8>,
    pub host_name: String,
    pub granted_access: String,
    pub auto_connect: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl ConversationRepository {
    // =========================================================================
    // Federated Accounts (host side)
    // =========================================================================

    pub async fn create_federated_account(
        &self,
        account_key: &[u8],
        display_name: &str,
        home_node_id: Option<&[u8]>,
        home_name: Option<&str>,
        access: &str,
        created_by: &[u8],
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO federated_accounts
                (account_key, display_name, home_node_id, home_name, access, created_by)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(account_key)
        .bind(display_name)
        .bind(home_node_id)
        .bind(home_name)
        .bind(access)
        .bind(created_by)
        .execute(&self.pool)
        .await
        .context("Failed to create federated account")?;
        Ok(())
    }

    pub async fn get_federated_account(
        &self,
        account_key: &[u8],
    ) -> Result<Option<FederatedAccount>> {
        let row = sqlx::query(
            r#"
            SELECT account_key, display_name, home_node_id, home_name,
                   access, state, created_by, created_at, updated_at
            FROM federated_accounts WHERE account_key = ?
            "#,
        )
        .bind(account_key)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(row_to_federated_account))
    }

    pub async fn get_active_federated_account(
        &self,
        account_key: &[u8],
    ) -> Result<Option<FederatedAccount>> {
        let row = sqlx::query(
            r#"
            SELECT account_key, display_name, home_node_id, home_name,
                   access, state, created_by, created_at, updated_at
            FROM federated_accounts WHERE account_key = ? AND state = 'active'
            "#,
        )
        .bind(account_key)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(row_to_federated_account))
    }

    pub async fn list_federated_accounts(&self) -> Result<Vec<FederatedAccount>> {
        let rows = sqlx::query(
            r#"
            SELECT account_key, display_name, home_node_id, home_name,
                   access, state, created_by, created_at, updated_at
            FROM federated_accounts
            ORDER BY created_at ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(row_to_federated_account).collect())
    }

    pub async fn update_federated_access(&self, account_key: &[u8], access: &str) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE federated_accounts
            SET access = ?, updated_at = datetime('now')
            WHERE account_key = ?
            "#,
        )
        .bind(access)
        .bind(account_key)
        .execute(&self.pool)
        .await
        .context("Failed to update federated account access")?;
        Ok(())
    }

    pub async fn update_federated_state(&self, account_key: &[u8], state: &str) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE federated_accounts
            SET state = ?, updated_at = datetime('now')
            WHERE account_key = ?
            "#,
        )
        .bind(state)
        .bind(account_key)
        .execute(&self.pool)
        .await
        .context("Failed to update federated account state")?;
        Ok(())
    }

    pub async fn delete_federated_account(&self, account_key: &[u8]) -> Result<()> {
        sqlx::query("DELETE FROM federated_accounts WHERE account_key = ?")
            .bind(account_key)
            .execute(&self.pool)
            .await
            .context("Failed to delete federated account")?;
        Ok(())
    }

    // =========================================================================
    // Remote Crab Cities (home side)
    // =========================================================================

    pub async fn add_remote_crab_city(
        &self,
        host_node_id: &[u8],
        account_key: &[u8],
        host_name: &str,
        granted_access: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO remote_crab_cities
                (host_node_id, account_key, host_name, granted_access)
            VALUES (?, ?, ?, ?)
            ON CONFLICT (host_node_id, account_key) DO UPDATE SET
                host_name = excluded.host_name,
                granted_access = excluded.granted_access,
                updated_at = datetime('now')
            "#,
        )
        .bind(host_node_id)
        .bind(account_key)
        .bind(host_name)
        .bind(granted_access)
        .execute(&self.pool)
        .await
        .context("Failed to add remote crab city")?;
        Ok(())
    }

    pub async fn get_remote_crab_city(
        &self,
        host_node_id: &[u8],
        account_key: &[u8],
    ) -> Result<Option<RemoteCrabCity>> {
        let row = sqlx::query(
            r#"
            SELECT host_node_id, account_key, host_name, granted_access,
                   auto_connect, created_at, updated_at
            FROM remote_crab_cities
            WHERE host_node_id = ? AND account_key = ?
            "#,
        )
        .bind(host_node_id)
        .bind(account_key)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(row_to_remote_crab_city))
    }

    pub async fn list_remote_crab_cities(&self, account_key: &[u8]) -> Result<Vec<RemoteCrabCity>> {
        let rows = sqlx::query(
            r#"
            SELECT host_node_id, account_key, host_name, granted_access,
                   auto_connect, created_at, updated_at
            FROM remote_crab_cities
            WHERE account_key = ?
            ORDER BY host_name ASC
            "#,
        )
        .bind(account_key)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(row_to_remote_crab_city).collect())
    }

    pub async fn list_auto_connect(&self) -> Result<Vec<RemoteCrabCity>> {
        let rows = sqlx::query(
            r#"
            SELECT host_node_id, account_key, host_name, granted_access,
                   auto_connect, created_at, updated_at
            FROM remote_crab_cities
            WHERE auto_connect = 1
            ORDER BY host_name ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(row_to_remote_crab_city).collect())
    }

    pub async fn update_remote_access(
        &self,
        host_node_id: &[u8],
        account_key: &[u8],
        granted_access: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE remote_crab_cities
            SET granted_access = ?, updated_at = datetime('now')
            WHERE host_node_id = ? AND account_key = ?
            "#,
        )
        .bind(granted_access)
        .bind(host_node_id)
        .bind(account_key)
        .execute(&self.pool)
        .await
        .context("Failed to update remote crab city access")?;
        Ok(())
    }

    pub async fn set_auto_connect(
        &self,
        host_node_id: &[u8],
        account_key: &[u8],
        auto_connect: bool,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE remote_crab_cities
            SET auto_connect = ?, updated_at = datetime('now')
            WHERE host_node_id = ? AND account_key = ?
            "#,
        )
        .bind(auto_connect)
        .bind(host_node_id)
        .bind(account_key)
        .execute(&self.pool)
        .await
        .context("Failed to update auto_connect")?;
        Ok(())
    }

    pub async fn remove_remote_crab_city(
        &self,
        host_node_id: &[u8],
        account_key: &[u8],
    ) -> Result<()> {
        sqlx::query("DELETE FROM remote_crab_cities WHERE host_node_id = ? AND account_key = ?")
            .bind(host_node_id)
            .bind(account_key)
            .execute(&self.pool)
            .await
            .context("Failed to remove remote crab city")?;
        Ok(())
    }
}

fn row_to_federated_account(r: sqlx::sqlite::SqliteRow) -> FederatedAccount {
    FederatedAccount {
        account_key: r.get("account_key"),
        display_name: r.get("display_name"),
        home_node_id: r.get("home_node_id"),
        home_name: r.get("home_name"),
        access: r.get("access"),
        state: r.get("state"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

fn row_to_remote_crab_city(r: sqlx::sqlite::SqliteRow) -> RemoteCrabCity {
    RemoteCrabCity {
        host_node_id: r.get("host_node_id"),
        account_key: r.get("account_key"),
        host_name: r.get("host_name"),
        granted_access: r.get("granted_access"),
        auto_connect: r.get::<bool, _>("auto_connect"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

#[cfg(test)]
mod tests {
    use crate::repository::test_helpers;

    #[tokio::test]
    async fn federated_account_crud() {
        let repo = test_helpers::test_repository().await;
        let key = vec![0xAA; 32];
        let admin = vec![0xBB; 32];

        repo.create_federated_account(
            &key,
            "Alice",
            Some(&[0xCC; 32]),
            Some("Alice's Lab"),
            r#"[{"type":"terminals","actions":["read"]}]"#,
            &admin,
        )
        .await
        .unwrap();

        let acct = repo.get_federated_account(&key).await.unwrap().unwrap();
        assert_eq!(acct.display_name, "Alice");
        assert_eq!(acct.home_name.as_deref(), Some("Alice's Lab"));
        assert_eq!(acct.state, "active");
        assert_eq!(acct.created_by, admin);
    }

    #[tokio::test]
    async fn federated_account_not_found() {
        let repo = test_helpers::test_repository().await;
        let result = repo.get_federated_account(&[0xFF; 32]).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn federated_account_state_transitions() {
        let repo = test_helpers::test_repository().await;
        let key = vec![0xA1; 32];
        let admin = vec![0xB1; 32];

        repo.create_federated_account(&key, "Bob", None, None, "[]", &admin)
            .await
            .unwrap();

        // Suspend
        repo.update_federated_state(&key, "suspended")
            .await
            .unwrap();
        let acct = repo.get_federated_account(&key).await.unwrap().unwrap();
        assert_eq!(acct.state, "suspended");

        // get_active should return None
        let active = repo.get_active_federated_account(&key).await.unwrap();
        assert!(active.is_none());

        // Reactivate
        repo.update_federated_state(&key, "active").await.unwrap();
        let active = repo.get_active_federated_account(&key).await.unwrap();
        assert!(active.is_some());
    }

    #[tokio::test]
    async fn federated_account_update_access() {
        let repo = test_helpers::test_repository().await;
        let key = vec![0xA2; 32];
        let admin = vec![0xB2; 32];

        repo.create_federated_account(&key, "Charlie", None, None, "[]", &admin)
            .await
            .unwrap();

        let new_access = r#"[{"type":"chat","actions":["send"]}]"#;
        repo.update_federated_access(&key, new_access)
            .await
            .unwrap();

        let acct = repo.get_federated_account(&key).await.unwrap().unwrap();
        assert!(acct.access.contains("chat"));
    }

    #[tokio::test]
    async fn federated_account_delete() {
        let repo = test_helpers::test_repository().await;
        let key = vec![0xA3; 32];
        let admin = vec![0xB3; 32];

        repo.create_federated_account(&key, "Dave", None, None, "[]", &admin)
            .await
            .unwrap();

        repo.delete_federated_account(&key).await.unwrap();
        let result = repo.get_federated_account(&key).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn list_federated_accounts() {
        let repo = test_helpers::test_repository().await;
        let admin = vec![0xB4; 32];

        repo.create_federated_account(&[0xC1; 32], "Eve", None, None, "[]", &admin)
            .await
            .unwrap();
        repo.create_federated_account(&[0xC2; 32], "Frank", None, None, "[]", &admin)
            .await
            .unwrap();

        let accounts = repo.list_federated_accounts().await.unwrap();
        assert_eq!(accounts.len(), 2);
    }

    #[tokio::test]
    async fn remote_crab_city_crud() {
        let repo = test_helpers::test_repository().await;
        let host = vec![0xD1; 32];
        let user = vec![0xE1; 32];

        repo.add_remote_crab_city(&host, &user, "Bob's Workshop", "[]")
            .await
            .unwrap();

        let remote = repo
            .get_remote_crab_city(&host, &user)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(remote.host_name, "Bob's Workshop");
        assert!(remote.auto_connect);
    }

    #[tokio::test]
    async fn remote_crab_city_not_found() {
        let repo = test_helpers::test_repository().await;
        let result = repo
            .get_remote_crab_city(&[0xFF; 32], &[0xFE; 32])
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn remote_crab_city_upsert() {
        let repo = test_helpers::test_repository().await;
        let host = vec![0xD2; 32];
        let user = vec![0xE2; 32];

        repo.add_remote_crab_city(&host, &user, "Old Name", "[]")
            .await
            .unwrap();

        // Upsert with new name
        repo.add_remote_crab_city(&host, &user, "New Name", r#"["view"]"#)
            .await
            .unwrap();

        let remote = repo
            .get_remote_crab_city(&host, &user)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(remote.host_name, "New Name");
        assert!(remote.granted_access.contains("view"));
    }

    #[tokio::test]
    async fn list_remote_crab_cities_by_user() {
        let repo = test_helpers::test_repository().await;
        let user = vec![0xE3; 32];

        repo.add_remote_crab_city(&[0xD3; 32], &user, "Host A", "[]")
            .await
            .unwrap();
        repo.add_remote_crab_city(&[0xD4; 32], &user, "Host B", "[]")
            .await
            .unwrap();

        // Different user
        repo.add_remote_crab_city(&[0xD5; 32], &[0xE4; 32], "Host C", "[]")
            .await
            .unwrap();

        let remotes = repo.list_remote_crab_cities(&user).await.unwrap();
        assert_eq!(remotes.len(), 2);
    }

    #[tokio::test]
    async fn list_auto_connect() {
        let repo = test_helpers::test_repository().await;
        let user = vec![0xE5; 32];

        repo.add_remote_crab_city(&[0xD6; 32], &user, "Auto", "[]")
            .await
            .unwrap();
        repo.add_remote_crab_city(&[0xD7; 32], &user, "Manual", "[]")
            .await
            .unwrap();

        // Disable auto_connect on second
        repo.set_auto_connect(&[0xD7; 32], &user, false)
            .await
            .unwrap();

        let auto = repo.list_auto_connect().await.unwrap();
        assert_eq!(auto.len(), 1);
        assert_eq!(auto[0].host_name, "Auto");
    }

    #[tokio::test]
    async fn update_remote_access() {
        let repo = test_helpers::test_repository().await;
        let host = vec![0xD8; 32];
        let user = vec![0xE6; 32];

        repo.add_remote_crab_city(&host, &user, "Test", "[]")
            .await
            .unwrap();

        repo.update_remote_access(&host, &user, r#"["collaborate"]"#)
            .await
            .unwrap();

        let remote = repo
            .get_remote_crab_city(&host, &user)
            .await
            .unwrap()
            .unwrap();
        assert!(remote.granted_access.contains("collaborate"));
    }

    #[tokio::test]
    async fn remove_remote_crab_city() {
        let repo = test_helpers::test_repository().await;
        let host = vec![0xD9; 32];
        let user = vec![0xE7; 32];

        repo.add_remote_crab_city(&host, &user, "Gone", "[]")
            .await
            .unwrap();

        repo.remove_remote_crab_city(&host, &user).await.unwrap();
        let result = repo.get_remote_crab_city(&host, &user).await.unwrap();
        assert!(result.is_none());
    }
}
