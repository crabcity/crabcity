use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{error, info};

use crate::AppState;
use crate::auth::AuthUser;

pub async fn get_database_stats(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.db.get_stats().await {
        Ok(stats) => Ok(Json(stats)),
        Err(e) => {
            error!("Failed to get database stats: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Deserialize)]
pub struct ImportRequest {
    project_path: Option<String>,
    import_all: bool,
}

#[derive(Serialize)]
pub struct ImportResponse {
    imported: usize,
    updated: usize,
    skipped: usize,
    failed: usize,
    total: usize,
}

pub async fn trigger_import(
    State(state): State<AppState>,
    Json(request): Json<ImportRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let importer = crate::import::ConversationImporter::new(state.repository.as_ref().clone());

    let stats = if request.import_all {
        info!("API: Starting full system import...");
        match importer.import_all_projects().await {
            Ok(stats) => stats,
            Err(e) => {
                error!("Import failed: {}", e);
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    } else if let Some(project_path) = request.project_path {
        info!("API: Importing from {}", project_path);
        match importer
            .import_from_project(&PathBuf::from(project_path))
            .await
        {
            Ok(stats) => stats,
            Err(e) => {
                error!("Import failed: {}", e);
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    } else {
        return Err(StatusCode::BAD_REQUEST);
    };

    Ok(Json(ImportResponse {
        imported: stats.imported,
        updated: stats.updated,
        skipped: stats.skipped,
        failed: stats.failed,
        total: stats.total(),
    }))
}

// Server invite admin handlers

#[derive(Deserialize)]
pub struct CreateServerInviteRequest {
    label: Option<String>,
    max_uses: Option<i32>,
    expires_in_hours: Option<i64>,
}

pub async fn create_server_invite_handler(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Json(req): Json<CreateServerInviteRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    if !auth_user.is_admin {
        return Err(StatusCode::FORBIDDEN);
    }

    let now = chrono::Utc::now().timestamp();
    let invite = crate::models::ServerInvite {
        token: uuid::Uuid::new_v4().to_string(),
        created_by: auth_user.user_id,
        label: req.label,
        max_uses: req.max_uses,
        use_count: 0,
        expires_at: req.expires_in_hours.map(|h| now + h * 3600),
        revoked: false,
        created_at: now,
    };

    state
        .repository
        .create_server_invite(&invite)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "token": invite.token,
        "label": invite.label,
        "max_uses": invite.max_uses,
        "expires_at": invite.expires_at,
        "created_at": invite.created_at,
    })))
}

pub async fn list_server_invites_handler(
    State(state): State<AppState>,
    auth_user: AuthUser,
) -> Result<impl IntoResponse, StatusCode> {
    if !auth_user.is_admin {
        return Err(StatusCode::FORBIDDEN);
    }

    match state.repository.list_server_invites().await {
        Ok(invites) => Ok(Json(invites)),
        Err(e) => {
            error!("Failed to list server invites: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

pub async fn revoke_server_invite_handler(
    State(state): State<AppState>,
    auth_user: AuthUser,
    Path(token): Path<String>,
) -> StatusCode {
    if !auth_user.is_admin {
        return StatusCode::FORBIDDEN;
    }

    match state.repository.revoke_server_invite(&token).await {
        Ok(true) => StatusCode::NO_CONTENT,
        Ok(false) => StatusCode::NOT_FOUND,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Trigger an HTTP server restart to reload configuration.
/// Accessible from loopback without auth (the CLI calls this after writing config.toml).
pub async fn restart_handler(State(state): State<AppState>) -> impl IntoResponse {
    info!("Admin restart requested — signalling server loop to reload config");
    let _ = state.restart_tx.send(());
    Json(serde_json::json!({ "ok": true, "message": "Server restarting with new config" }))
}

// =============================================================================
// Config read/patch endpoints
// =============================================================================

/// Response shape for GET /api/admin/config
#[derive(Serialize)]
pub struct ConfigResponse {
    pub profile: Option<String>,
    pub host: String,
    pub port: u16,
    pub auth_enabled: bool,
    pub https: bool,
    /// Which values are overridden at runtime (ephemeral)
    pub overrides: OverrideState,
}

#[derive(Serialize)]
pub struct OverrideState {
    pub host: bool,
    pub port: bool,
    pub auth_enabled: bool,
    pub https: bool,
}

/// GET /api/admin/config — return the effective config + which fields are overridden.
pub async fn get_config_handler(State(state): State<AppState>) -> impl IntoResponse {
    let fc: crate::config::FileConfig = crate::config::load_config(&state.config.data_dir, None)
        .extract()
        .unwrap_or_default();
    let overrides = state.runtime_overrides.read().await.clone();

    let effective_host = overrides
        .host
        .as_deref()
        .or(fc.server.host.as_deref())
        .unwrap_or("127.0.0.1")
        .to_string();
    let effective_port = overrides.port.or(fc.server.port).unwrap_or(0);
    let effective_auth = overrides.auth_enabled.unwrap_or(fc.auth.enabled);
    let effective_https = overrides.https.unwrap_or(fc.auth.https);

    let profile_name = fc.profile.as_ref().map(|p| match p {
        crate::config::Profile::Local => "local",
        crate::config::Profile::Tunnel => "tunnel",
        crate::config::Profile::Server => "server",
    });

    Json(ConfigResponse {
        profile: profile_name.map(|s| s.to_string()),
        host: effective_host,
        port: effective_port,
        auth_enabled: effective_auth,
        https: effective_https,
        overrides: OverrideState {
            host: overrides.host.is_some(),
            port: overrides.port.is_some(),
            auth_enabled: overrides.auth_enabled.is_some(),
            https: overrides.https.is_some(),
        },
    })
}

/// Request body for PATCH /api/admin/config
#[derive(Deserialize)]
pub struct ConfigPatchRequest {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub auth_enabled: Option<bool>,
    pub https: Option<bool>,
    /// If true, persist changes to config.toml (in addition to applying at runtime)
    #[serde(default)]
    pub save: bool,
}

/// PATCH /api/admin/config — apply runtime overrides and optionally save to config.toml.
pub async fn patch_config_handler(
    State(state): State<AppState>,
    Json(req): Json<ConfigPatchRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    // If save=true, persist to config.toml first
    if req.save {
        if let Err(e) = save_overrides_to_config(&state.config, &req) {
            error!("Failed to save config: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        info!(
            "Config changes saved to {}",
            state.config.config_toml_path().display()
        );
    }

    // Apply runtime overrides
    {
        let mut overrides = state.runtime_overrides.write().await;
        if let Some(ref host) = req.host {
            overrides.host = Some(host.clone());
        }
        if let Some(port) = req.port {
            overrides.port = Some(port);
        }
        if let Some(auth) = req.auth_enabled {
            overrides.auth_enabled = Some(auth);
        }
        if let Some(https) = req.https {
            overrides.https = Some(https);
        }

        // If saving, clear the runtime overrides for saved fields (they're now in config.toml)
        if req.save {
            if req.host.is_some() {
                overrides.host = None;
            }
            if req.port.is_some() {
                overrides.port = None;
            }
            if req.auth_enabled.is_some() {
                overrides.auth_enabled = None;
            }
            if req.https.is_some() {
                overrides.https = None;
            }
        }
    }

    // Trigger server restart
    info!("Config patch applied — triggering server restart");
    let _ = state.restart_tx.send(());

    Ok(Json(serde_json::json!({
        "ok": true,
        "saved": req.save,
    })))
}

/// Read-modify-write config.toml to persist the given overrides.
fn save_overrides_to_config(
    config: &crate::config::CrabCityConfig,
    req: &ConfigPatchRequest,
) -> anyhow::Result<()> {
    use anyhow::Context;

    let path = config.config_toml_path();
    let mut doc = if path.exists() {
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        contents
            .parse::<toml::Table>()
            .with_context(|| format!("Failed to parse {}", path.display()))?
    } else {
        toml::Table::new()
    };

    if let Some(ref host) = req.host {
        let server = doc
            .entry("server")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .context("[server] is not a table")?;
        server.insert("host".to_string(), toml::Value::String(host.clone()));
    }

    if let Some(port) = req.port {
        let server = doc
            .entry("server")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .context("[server] is not a table")?;
        server.insert("port".to_string(), toml::Value::Integer(port as i64));
    }

    if let Some(auth_enabled) = req.auth_enabled {
        let auth = doc
            .entry("auth")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .context("[auth] is not a table")?;
        auth.insert("enabled".to_string(), toml::Value::Boolean(auth_enabled));
    }

    if let Some(https) = req.https {
        let auth = doc
            .entry("auth")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .context("[auth] is not a table")?;
        auth.insert("https".to_string(), toml::Value::Boolean(https));
    }

    let serialized = toml::to_string_pretty(&doc).context("Failed to serialize config.toml")?;
    std::fs::write(&path, serialized)
        .with_context(|| format!("Failed to write {}", path.display()))?;

    Ok(())
}
