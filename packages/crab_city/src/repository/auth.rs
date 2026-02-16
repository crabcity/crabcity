//! Password auth bridge: argon2-hashed user accounts that map to keypair identities.
//!
//! This is a convenience layer for username/password sign-up. Under the hood,
//! the server generates an ed25519 keypair and creates a `MemberIdentity` +
//! `MemberGrant`, so the interconnect system remains the single source of truth.

use anyhow::Result;
use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};

use super::ConversationRepository;
use crate::models::User;

/// Hash a password with Argon2id and a random salt.
fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("failed to hash password: {e}"))?;
    Ok(hash.to_string())
}

/// Verify a password against a stored Argon2id hash.
fn verify_password(password: &str, hash: &str) -> Result<bool> {
    let parsed =
        PasswordHash::new(hash).map_err(|e| anyhow::anyhow!("invalid password hash: {e}"))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

impl ConversationRepository {
    /// Create a user with an argon2-hashed password and optional server-generated keypair.
    pub async fn create_user(&self, user: &User) -> Result<()> {
        sqlx::query(
            "INSERT INTO users (id, username, display_name, password_hash, public_key, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&user.id)
        .bind(&user.username)
        .bind(&user.display_name)
        .bind(&user.password_hash)
        .bind(&user.public_key)
        .bind(user.created_at)
        .bind(user.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Look up a user by username.
    pub async fn get_user_by_username(&self, username: &str) -> Result<Option<User>> {
        let user = sqlx::query_as::<_, User>(
            "SELECT id, username, display_name, password_hash, public_key, created_at, updated_at
             FROM users WHERE username = ?",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        Ok(user)
    }

    /// Verify password against stored argon2 hash. Returns the User on success.
    pub async fn verify_user_password(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Option<User>> {
        let user = match self.get_user_by_username(username).await? {
            Some(u) => u,
            None => return Ok(None),
        };
        if verify_password(password, &user.password_hash)? {
            Ok(Some(user))
        } else {
            Ok(None)
        }
    }

    /// Link a public key to an existing user (for migration from password-only to keypair).
    pub async fn link_user_public_key(&self, user_id: &str, public_key: &[u8]) -> Result<()> {
        sqlx::query("UPDATE users SET public_key = ?, updated_at = ? WHERE id = ?")
            .bind(public_key)
            .bind(chrono::Utc::now().timestamp())
            .bind(user_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Hash a password with argon2 for user creation.
    pub fn hash_password(password: &str) -> Result<String> {
        hash_password(password)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::test_helpers;

    #[test]
    fn test_hash_and_verify_password() {
        let hash = hash_password("hunter2").unwrap();
        assert!(hash.starts_with("$argon2"));
        assert!(verify_password("hunter2", &hash).unwrap());
        assert!(!verify_password("wrong", &hash).unwrap());
    }

    #[test]
    fn test_different_passwords_different_hashes() {
        let h1 = hash_password("password1").unwrap();
        let h2 = hash_password("password2").unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_same_password_different_salts() {
        let h1 = hash_password("same").unwrap();
        let h2 = hash_password("same").unwrap();
        // Random salt means different hashes
        assert_ne!(h1, h2);
        // Both verify correctly
        assert!(verify_password("same", &h1).unwrap());
        assert!(verify_password("same", &h2).unwrap());
    }

    #[tokio::test]
    async fn test_create_and_get_user() {
        let repo = test_helpers::test_repository().await;
        let hash = hash_password("secret").unwrap();
        let now = chrono::Utc::now().timestamp();
        let user = User {
            id: "u-1".into(),
            username: "alice".into(),
            display_name: "Alice".into(),
            password_hash: hash,
            public_key: None,
            created_at: now,
            updated_at: now,
        };
        repo.create_user(&user).await.unwrap();

        let found = repo.get_user_by_username("alice").await.unwrap().unwrap();
        assert_eq!(found.id, "u-1");
        assert_eq!(found.display_name, "Alice");
        assert!(found.public_key.is_none());
    }

    #[tokio::test]
    async fn test_verify_user_password() {
        let repo = test_helpers::test_repository().await;
        let hash = hash_password("mypass").unwrap();
        let now = chrono::Utc::now().timestamp();
        let user = User {
            id: "u-2".into(),
            username: "bob".into(),
            display_name: "Bob".into(),
            password_hash: hash,
            public_key: None,
            created_at: now,
            updated_at: now,
        };
        repo.create_user(&user).await.unwrap();

        // Correct password
        let ok = repo.verify_user_password("bob", "mypass").await.unwrap();
        assert!(ok.is_some());

        // Wrong password
        let fail = repo.verify_user_password("bob", "wrong").await.unwrap();
        assert!(fail.is_none());

        // Non-existent user
        let missing = repo.verify_user_password("nobody", "x").await.unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn test_link_user_public_key() {
        let repo = test_helpers::test_repository().await;
        let hash = hash_password("pw").unwrap();
        let now = chrono::Utc::now().timestamp();
        let user = User {
            id: "u-3".into(),
            username: "charlie".into(),
            display_name: "Charlie".into(),
            password_hash: hash,
            public_key: None,
            created_at: now,
            updated_at: now,
        };
        repo.create_user(&user).await.unwrap();

        let pk = [42u8; 32];
        repo.link_user_public_key("u-3", &pk).await.unwrap();

        let updated = repo.get_user_by_username("charlie").await.unwrap().unwrap();
        assert_eq!(updated.public_key.as_deref(), Some(pk.as_slice()));
    }
}
