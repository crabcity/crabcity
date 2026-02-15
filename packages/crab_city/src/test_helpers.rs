use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::AppState;
use crate::config::{AuthConfig, CrabCityConfig, RuntimeOverrides, ServerConfig, ServerFileConfig};
use crate::db::Database;
use crate::instance_manager::InstanceManager;
use crate::metrics::ServerMetrics;
use crate::persistence::PersistenceService;
use crate::repository::ConversationRepository;

/// Build a fully-wired `AppState` backed by an in-memory SQLite database.
/// Suitable for handler tests that exercise real SQL queries without I/O.
///
/// Returns `(AppState, TempDir)` — callers **must** hold the `TempDir` for
/// the lifetime of the test so that file-backed services (e.g. `NotesStorage`)
/// continue to have a valid directory.
pub async fn test_app_state() -> (AppState, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = CrabCityConfig::new(Some(tmp.path().to_path_buf())).expect("config");

    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory sqlite");

    crate::db::run_migrations(&pool).await.expect("migrations");

    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .expect("pragma");

    let db = Arc::new(Database { pool: pool.clone() });
    let repository = Arc::new(ConversationRepository::new(pool));
    let persistence_service = Arc::new(PersistenceService::new(repository.clone()));
    let notes_storage = Arc::new(crate::notes::NotesStorage::new(&config.data_dir).expect("notes"));
    let state_broadcast = crate::ws::create_state_broadcast();
    let global_state_manager = Arc::new(crate::ws::GlobalStateManager::new(state_broadcast));
    let (restart_tx, _restart_rx) = tokio::sync::watch::channel(());

    let state = AppState {
        instance_manager: Arc::new(InstanceManager::new("echo".into(), 9000, 25 * 1024 * 1024)),
        conversation_watchers: Arc::new(Mutex::new(HashMap::new())),
        config: Arc::new(config),
        server_config: Arc::new(ServerConfig::from_file(&ServerFileConfig::default())),
        auth_config: Arc::new(AuthConfig {
            enabled: false,
            session_ttl_secs: 3600,
            allow_registration: true,
            https: false,
        }),
        metrics: Arc::new(ServerMetrics::new()),
        db,
        repository,
        persistence_service,
        instance_persistors: Arc::new(Mutex::new(HashMap::new())),
        notes_storage,
        global_state_manager,
        runtime_overrides: Arc::new(tokio::sync::RwLock::new(RuntimeOverrides::default())),
        restart_tx: Arc::new(restart_tx),
    };

    (state, tmp)
}

/// Like `test_app_state`, but with auth **enabled** and a pre-created admin user.
///
/// Returns `(AppState, TempDir, AuthUser)` where `AuthUser` can be inserted into
/// request extensions to simulate an authenticated caller.
pub async fn test_app_state_with_auth() -> (AppState, tempfile::TempDir, crate::auth::AuthUser) {
    use crab_city_auth::{Capability, PublicKey};

    let (mut state, tmp) = test_app_state().await;

    // Enable auth
    state.auth_config = Arc::new(AuthConfig {
        enabled: true,
        session_ttl_secs: 3600,
        allow_registration: true,
        https: false,
    });

    // Create an admin AuthUser with a deterministic key
    let pk = PublicKey::from_bytes([0xAA; 32]);
    let auth_user = crate::auth::AuthUser::from_grant(pk, "Admin".into(), Capability::Owner);

    // Insert a User row in the DB to satisfy FK constraints on instance_permissions etc.
    let user = crate::models::User {
        id: auth_user.user_id().to_string(),
        username: "admin".to_string(),
        display_name: "Admin".to_string(),
        password_hash: "unused".to_string(),
        is_admin: true,
        is_disabled: false,
        created_at: chrono::Utc::now().timestamp(),
        updated_at: chrono::Utc::now().timestamp(),
    };
    state.repository.create_user(&user).await.unwrap();

    (state, tmp, auth_user)
}

/// Create a non-admin `AuthUser` and insert the backing user into the database.
///
/// The `seed` parameter is hashed to produce a deterministic public key.
/// The returned AuthUser's `user_id()` is the fingerprint of that key.
pub async fn create_test_user(
    repository: &ConversationRepository,
    seed: &str,
    username: &str,
    display_name: &str,
) -> crate::auth::AuthUser {
    use crab_city_auth::{Capability, PublicKey};

    // Derive a deterministic 32-byte key from the seed string
    let mut key_bytes = [0u8; 32];
    for (i, b) in seed.bytes().enumerate() {
        key_bytes[i % 32] ^= b;
    }
    let pk = PublicKey::from_bytes(key_bytes);
    let auth_user = crate::auth::AuthUser::from_grant(pk, display_name.into(), Capability::View);

    // Insert User row to satisfy FK constraints
    let user = crate::models::User {
        id: auth_user.user_id().to_string(),
        username: username.to_string(),
        display_name: display_name.to_string(),
        password_hash: "unused".to_string(),
        is_admin: false,
        is_disabled: false,
        created_at: chrono::Utc::now().timestamp(),
        updated_at: chrono::Utc::now().timestamp(),
    };
    repository.create_user(&user).await.unwrap();

    auth_user
}
