//! First-run onboarding.
//!
//! With the migration to keypair-based identity (iroh transport), the old
//! password-based admin setup is no longer needed. Identity is now derived
//! from Ed25519 keys and managed via the `crab_city_auth` crate.
//!
//! This module is retained as a stub — future onboarding (e.g. first-run
//! relay configuration, identity key display) will be added here.

use anyhow::Result;

use crate::config::AuthConfig;
use crate::repository::ConversationRepository;

/// Run first-time setup if needed.
///
/// Currently a no-op — keypair-based identity doesn't require interactive setup.
/// The instance identity key is generated automatically on first run
/// (see `identity.rs`).
pub async fn maybe_run_onboarding(
    _repository: &ConversationRepository,
    _auth_config: &AuthConfig,
) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AuthConfig;
    use crate::repository::test_helpers;

    fn auth_config(enabled: bool) -> AuthConfig {
        AuthConfig {
            enabled,
            session_ttl_secs: 3600,
            allow_registration: true,
            https: false,
        }
    }

    #[tokio::test]
    async fn onboarding_is_noop() {
        let repo = test_helpers::test_repository().await;
        maybe_run_onboarding(&repo, &auth_config(true))
            .await
            .unwrap();
    }
}
