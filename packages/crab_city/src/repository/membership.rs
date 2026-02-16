//! Repository layer for member identities and grants.

use anyhow::{Context, Result};
use sqlx::Row;

use super::ConversationRepository;

// --- Row types (DB-layer, not crab_city_auth types) ---

#[derive(Debug, Clone)]
pub struct MemberIdentity {
    pub public_key: Vec<u8>,
    pub display_name: String,
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
    pub registry_account_id: Option<String>,
    pub resolved_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct MemberGrant {
    pub public_key: Vec<u8>,
    pub capability: String,
    pub access: String,
    pub state: String,
    pub org_id: Option<String>,
    pub invited_by: Option<Vec<u8>>,
    pub invited_via: Option<Vec<u8>>,
    pub replaces: Option<Vec<u8>>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct Member {
    pub identity: MemberIdentity,
    pub grant: MemberGrant,
}

impl ConversationRepository {
    // =========================================================================
    // Identity CRUD
    // =========================================================================

    pub async fn create_identity(&self, public_key: &[u8], display_name: &str) -> Result<()> {
        sqlx::query("INSERT INTO member_identities (public_key, display_name) VALUES (?, ?)")
            .bind(public_key)
            .bind(display_name)
            .execute(&self.pool)
            .await
            .context("Failed to create member identity")?;
        Ok(())
    }

    pub async fn get_identity(&self, public_key: &[u8]) -> Result<Option<MemberIdentity>> {
        let row = sqlx::query(
            r#"
            SELECT public_key, display_name, handle, avatar_url,
                   registry_account_id, resolved_at, created_at, updated_at
            FROM member_identities WHERE public_key = ?
            "#,
        )
        .bind(public_key)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| MemberIdentity {
            public_key: r.get("public_key"),
            display_name: r.get("display_name"),
            handle: r.get("handle"),
            avatar_url: r.get("avatar_url"),
            registry_account_id: r.get("registry_account_id"),
            resolved_at: r.get("resolved_at"),
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
        }))
    }

    pub async fn update_identity(
        &self,
        public_key: &[u8],
        display_name: &str,
        handle: Option<&str>,
        avatar_url: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE member_identities
            SET display_name = ?, handle = ?, avatar_url = ?,
                updated_at = datetime('now')
            WHERE public_key = ?
            "#,
        )
        .bind(display_name)
        .bind(handle)
        .bind(avatar_url)
        .bind(public_key)
        .execute(&self.pool)
        .await
        .context("Failed to update member identity")?;
        Ok(())
    }

    // =========================================================================
    // Grant CRUD
    // =========================================================================

    pub async fn create_grant(
        &self,
        public_key: &[u8],
        capability: &str,
        access: &str,
        state: &str,
        invited_by: Option<&[u8]>,
        invited_via: Option<&[u8]>,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO member_grants (public_key, capability, access, state, invited_by, invited_via)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(public_key)
        .bind(capability)
        .bind(access)
        .bind(state)
        .bind(invited_by)
        .bind(invited_via)
        .execute(&self.pool)
        .await
        .context("Failed to create member grant")?;
        Ok(())
    }

    pub async fn get_grant(&self, public_key: &[u8]) -> Result<Option<MemberGrant>> {
        let row = sqlx::query(
            r#"
            SELECT public_key, capability, access, state, org_id,
                   invited_by, invited_via, replaces, created_at, updated_at
            FROM member_grants WHERE public_key = ?
            "#,
        )
        .bind(public_key)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(row_to_grant))
    }

    pub async fn get_active_grant(&self, public_key: &[u8]) -> Result<Option<MemberGrant>> {
        let row = sqlx::query(
            r#"
            SELECT public_key, capability, access, state, org_id,
                   invited_by, invited_via, replaces, created_at, updated_at
            FROM member_grants WHERE public_key = ? AND state = 'active'
            "#,
        )
        .bind(public_key)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(row_to_grant))
    }

    pub async fn get_member(&self, public_key: &[u8]) -> Result<Option<Member>> {
        let row = sqlx::query(
            r#"
            SELECT
                i.public_key, i.display_name, i.handle, i.avatar_url,
                i.registry_account_id, i.resolved_at,
                i.created_at as i_created_at, i.updated_at as i_updated_at,
                g.capability, g.access, g.state, g.org_id,
                g.invited_by, g.invited_via, g.replaces,
                g.created_at as g_created_at, g.updated_at as g_updated_at
            FROM member_identities i
            JOIN member_grants g ON i.public_key = g.public_key
            WHERE i.public_key = ?
            "#,
        )
        .bind(public_key)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| {
            let pk: Vec<u8> = r.get("public_key");
            Member {
                identity: MemberIdentity {
                    public_key: pk.clone(),
                    display_name: r.get("display_name"),
                    handle: r.get("handle"),
                    avatar_url: r.get("avatar_url"),
                    registry_account_id: r.get("registry_account_id"),
                    resolved_at: r.get("resolved_at"),
                    created_at: r.get("i_created_at"),
                    updated_at: r.get("i_updated_at"),
                },
                grant: MemberGrant {
                    public_key: pk,
                    capability: r.get("capability"),
                    access: r.get("access"),
                    state: r.get("state"),
                    org_id: r.get("org_id"),
                    invited_by: r.get("invited_by"),
                    invited_via: r.get("invited_via"),
                    replaces: r.get("replaces"),
                    created_at: r.get("g_created_at"),
                    updated_at: r.get("g_updated_at"),
                },
            }
        }))
    }

    pub async fn list_members(&self) -> Result<Vec<Member>> {
        let rows = sqlx::query(
            r#"
            SELECT
                i.public_key, i.display_name, i.handle, i.avatar_url,
                i.registry_account_id, i.resolved_at,
                i.created_at as i_created_at, i.updated_at as i_updated_at,
                g.capability, g.access, g.state, g.org_id,
                g.invited_by, g.invited_via, g.replaces,
                g.created_at as g_created_at, g.updated_at as g_updated_at
            FROM member_identities i
            JOIN member_grants g ON i.public_key = g.public_key
            ORDER BY g.created_at ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let pk: Vec<u8> = r.get("public_key");
                Member {
                    identity: MemberIdentity {
                        public_key: pk.clone(),
                        display_name: r.get("display_name"),
                        handle: r.get("handle"),
                        avatar_url: r.get("avatar_url"),
                        registry_account_id: r.get("registry_account_id"),
                        resolved_at: r.get("resolved_at"),
                        created_at: r.get("i_created_at"),
                        updated_at: r.get("i_updated_at"),
                    },
                    grant: MemberGrant {
                        public_key: pk,
                        capability: r.get("capability"),
                        access: r.get("access"),
                        state: r.get("state"),
                        org_id: r.get("org_id"),
                        invited_by: r.get("invited_by"),
                        invited_via: r.get("invited_via"),
                        replaces: r.get("replaces"),
                        created_at: r.get("g_created_at"),
                        updated_at: r.get("g_updated_at"),
                    },
                }
            })
            .collect())
    }

    pub async fn update_grant_state(&self, public_key: &[u8], new_state: &str) -> Result<()> {
        sqlx::query(
            "UPDATE member_grants SET state = ?, updated_at = datetime('now') WHERE public_key = ?",
        )
        .bind(new_state)
        .bind(public_key)
        .execute(&self.pool)
        .await
        .context("Failed to update grant state")?;
        Ok(())
    }

    pub async fn update_grant_capability(
        &self,
        public_key: &[u8],
        capability: &str,
        access: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE member_grants
            SET capability = ?, access = ?, updated_at = datetime('now')
            WHERE public_key = ?
            "#,
        )
        .bind(capability)
        .bind(access)
        .bind(public_key)
        .execute(&self.pool)
        .await
        .context("Failed to update grant capability")?;
        Ok(())
    }

    pub async fn update_grant_access(&self, public_key: &[u8], access: &str) -> Result<()> {
        sqlx::query(
            "UPDATE member_grants SET access = ?, updated_at = datetime('now') WHERE public_key = ?",
        )
        .bind(access)
        .bind(public_key)
        .execute(&self.pool)
        .await
        .context("Failed to update grant access")?;
        Ok(())
    }

    pub async fn replace_grant(&self, new_pubkey: &[u8], old_pubkey: &[u8]) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        // Get the old grant's capability + access
        let old = sqlx::query(
            "SELECT capability, access, invited_by, invited_via FROM member_grants WHERE public_key = ?",
        )
        .bind(old_pubkey)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Old grant not found"))?;

        let cap: String = old.get("capability");
        let access: String = old.get("access");
        let invited_by: Option<Vec<u8>> = old.get("invited_by");
        let invited_via: Option<Vec<u8>> = old.get("invited_via");

        // Create new grant with same capability, referencing old
        sqlx::query(
            r#"
            INSERT INTO member_grants (public_key, capability, access, state, invited_by, invited_via, replaces)
            VALUES (?, ?, ?, 'active', ?, ?, ?)
            "#,
        )
        .bind(new_pubkey)
        .bind(&cap)
        .bind(&access)
        .bind(&invited_by)
        .bind(&invited_via)
        .bind(old_pubkey)
        .execute(&mut *tx)
        .await?;

        // Mark old grant as removed
        sqlx::query(
            "UPDATE member_grants SET state = 'removed', updated_at = datetime('now') WHERE public_key = ?",
        )
        .bind(old_pubkey)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// List all grants (any state).
    pub async fn list_grants(&self) -> Result<Vec<MemberGrant>> {
        let rows = sqlx::query(
            r#"
            SELECT public_key, capability, access, state, org_id,
                   invited_by, invited_via, replaces, created_at, updated_at
            FROM member_grants
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(row_to_grant).collect())
    }

    pub async fn list_grants_by_invite(&self, invite_nonce: &[u8]) -> Result<Vec<MemberGrant>> {
        let rows = sqlx::query(
            r#"
            SELECT public_key, capability, access, state, org_id,
                   invited_by, invited_via, replaces, created_at, updated_at
            FROM member_grants WHERE invited_via = ?
            "#,
        )
        .bind(invite_nonce)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(row_to_grant).collect())
    }
}

fn row_to_grant(r: sqlx::sqlite::SqliteRow) -> MemberGrant {
    MemberGrant {
        public_key: r.get("public_key"),
        capability: r.get("capability"),
        access: r.get("access"),
        state: r.get("state"),
        org_id: r.get("org_id"),
        invited_by: r.get("invited_by"),
        invited_via: r.get("invited_via"),
        replaces: r.get("replaces"),
        created_at: r.get("created_at"),
        updated_at: r.get("updated_at"),
    }
}

#[cfg(test)]
mod tests {
    use crate::repository::test_helpers;

    #[tokio::test]
    async fn identity_crud() {
        let repo = test_helpers::test_repository().await;
        let pk = vec![1u8; 32];

        repo.create_identity(&pk, "Alice").await.unwrap();

        let identity = repo.get_identity(&pk).await.unwrap().unwrap();
        assert_eq!(identity.display_name, "Alice");
        assert!(identity.handle.is_none());

        repo.update_identity(&pk, "Alice B", Some("@alice"), None)
            .await
            .unwrap();

        let updated = repo.get_identity(&pk).await.unwrap().unwrap();
        assert_eq!(updated.display_name, "Alice B");
        assert_eq!(updated.handle.as_deref(), Some("@alice"));
    }

    #[tokio::test]
    async fn identity_not_found() {
        let repo = test_helpers::test_repository().await;
        let result = repo.get_identity(&[99u8; 32]).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn grant_crud() {
        let repo = test_helpers::test_repository().await;
        let pk = vec![2u8; 32];

        repo.create_identity(&pk, "Bob").await.unwrap();
        repo.create_grant(&pk, "collaborate", "[]", "active", None, None)
            .await
            .unwrap();

        let grant = repo.get_grant(&pk).await.unwrap().unwrap();
        assert_eq!(grant.capability, "collaborate");
        assert_eq!(grant.state, "active");

        let active = repo.get_active_grant(&pk).await.unwrap().unwrap();
        assert_eq!(active.capability, "collaborate");
    }

    #[tokio::test]
    async fn get_active_grant_excludes_suspended() {
        let repo = test_helpers::test_repository().await;
        let pk = vec![3u8; 32];

        repo.create_identity(&pk, "Charlie").await.unwrap();
        repo.create_grant(&pk, "view", "[]", "suspended", None, None)
            .await
            .unwrap();

        let active = repo.get_active_grant(&pk).await.unwrap();
        assert!(active.is_none());

        let grant = repo.get_grant(&pk).await.unwrap();
        assert!(grant.is_some());
    }

    #[tokio::test]
    async fn state_transitions() {
        let repo = test_helpers::test_repository().await;
        let pk = vec![4u8; 32];

        repo.create_identity(&pk, "Dave").await.unwrap();
        repo.create_grant(&pk, "collaborate", "[]", "active", None, None)
            .await
            .unwrap();

        repo.update_grant_state(&pk, "suspended").await.unwrap();
        let g = repo.get_grant(&pk).await.unwrap().unwrap();
        assert_eq!(g.state, "suspended");

        repo.update_grant_state(&pk, "active").await.unwrap();
        let g = repo.get_grant(&pk).await.unwrap().unwrap();
        assert_eq!(g.state, "active");

        repo.update_grant_state(&pk, "removed").await.unwrap();
        let g = repo.get_grant(&pk).await.unwrap().unwrap();
        assert_eq!(g.state, "removed");
    }

    #[tokio::test]
    async fn update_capability() {
        let repo = test_helpers::test_repository().await;
        let pk = vec![5u8; 32];

        repo.create_identity(&pk, "Eve").await.unwrap();
        repo.create_grant(&pk, "view", "[]", "active", None, None)
            .await
            .unwrap();

        repo.update_grant_capability(
            &pk,
            "admin",
            "[{\"type\":\"members\",\"actions\":[\"read\"]}]",
        )
        .await
        .unwrap();

        let g = repo.get_grant(&pk).await.unwrap().unwrap();
        assert_eq!(g.capability, "admin");
        assert!(g.access.contains("members"));
    }

    #[tokio::test]
    async fn list_members() {
        let repo = test_helpers::test_repository().await;

        // Loopback is seeded by migration
        let members = repo.list_members().await.unwrap();
        assert!(members.iter().any(|m| m.identity.display_name == "Local"));

        let pk = vec![6u8; 32];
        repo.create_identity(&pk, "Frank").await.unwrap();
        repo.create_grant(&pk, "collaborate", "[]", "active", None, None)
            .await
            .unwrap();

        let members = repo.list_members().await.unwrap();
        assert!(members.len() >= 2);
        assert!(members.iter().any(|m| m.identity.display_name == "Frank"));
    }

    #[tokio::test]
    async fn replace_grant() {
        let repo = test_helpers::test_repository().await;
        let old_pk = vec![7u8; 32];
        let new_pk = vec![8u8; 32];

        repo.create_identity(&old_pk, "Old Key").await.unwrap();
        repo.create_grant(&old_pk, "admin", "[\"test\"]", "active", None, None)
            .await
            .unwrap();

        repo.create_identity(&new_pk, "New Key").await.unwrap();
        repo.replace_grant(&new_pk, &old_pk).await.unwrap();

        let old = repo.get_grant(&old_pk).await.unwrap().unwrap();
        assert_eq!(old.state, "removed");

        let new = repo.get_grant(&new_pk).await.unwrap().unwrap();
        assert_eq!(new.state, "active");
        assert_eq!(new.capability, "admin");
        assert_eq!(new.replaces.as_deref(), Some(old_pk.as_slice()));
    }

    #[tokio::test]
    async fn list_grants_by_invite() {
        let repo = test_helpers::test_repository().await;
        let nonce = vec![0xaa; 16];

        let pk1 = vec![9u8; 32];
        let pk2 = vec![10u8; 32];

        repo.create_identity(&pk1, "G1").await.unwrap();
        repo.create_identity(&pk2, "G2").await.unwrap();

        repo.create_grant(&pk1, "view", "[]", "active", None, Some(&nonce))
            .await
            .unwrap();
        repo.create_grant(&pk2, "view", "[]", "active", None, Some(&nonce))
            .await
            .unwrap();

        let grants = repo.list_grants_by_invite(&nonce).await.unwrap();
        assert_eq!(grants.len(), 2);
    }

    #[tokio::test]
    async fn get_member_found() {
        let repo = test_helpers::test_repository().await;
        let pk = vec![13u8; 32];

        repo.create_identity(&pk, "GetMe").await.unwrap();
        repo.create_grant(&pk, "collaborate", "[]", "active", None, None)
            .await
            .unwrap();

        let member = repo.get_member(&pk).await.unwrap().unwrap();
        assert_eq!(member.identity.display_name, "GetMe");
        assert_eq!(member.grant.capability, "collaborate");
        assert_eq!(member.grant.state, "active");
    }

    #[tokio::test]
    async fn get_member_not_found() {
        let repo = test_helpers::test_repository().await;
        let result = repo.get_member(&[99u8; 32]).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn grant_with_invited_by() {
        let repo = test_helpers::test_repository().await;
        let inviter_pk = vec![11u8; 32];
        let invitee_pk = vec![12u8; 32];

        repo.create_identity(&inviter_pk, "Inviter").await.unwrap();
        repo.create_identity(&invitee_pk, "Invitee").await.unwrap();
        repo.create_grant(
            &invitee_pk,
            "collaborate",
            "[]",
            "active",
            Some(&inviter_pk),
            None,
        )
        .await
        .unwrap();

        let g = repo.get_grant(&invitee_pk).await.unwrap().unwrap();
        assert_eq!(g.invited_by.as_deref(), Some(inviter_pk.as_slice()));
    }
}
