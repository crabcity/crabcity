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
