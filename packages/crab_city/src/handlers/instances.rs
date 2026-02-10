use crate::instance_manager::ClaudeInstance;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
use crate::auth::MaybeAuthUser;
use crate::persistence::InstancePersistor;
use crate::ws;

#[derive(Serialize)]
pub struct CreateInstanceResponse {
    id: String,
    name: String,
    wrapper_port: u16,
}

pub async fn list_instances(
    State(state): State<AppState>,
    maybe_user: MaybeAuthUser,
) -> Json<Vec<ClaudeInstance>> {
    let all_instances = state.instance_manager.list().await;

    if state.auth_config.enabled {
        if let MaybeAuthUser(Some(ref user)) = maybe_user {
            if user.is_admin {
                return Json(all_instances);
            }
            let permitted = state
                .repository
                .list_user_instance_ids(&user.user_id)
                .await
                .unwrap_or_default();
            let filtered = all_instances
                .into_iter()
                .filter(|i| permitted.contains(&i.id))
                .collect();
            return Json(filtered);
        }
    }

    Json(all_instances)
}

#[derive(Deserialize)]
pub struct CreateInstanceRequest {
    name: Option<String>,
    working_dir: Option<String>,
    command: Option<String>,
}

pub async fn create_instance(
    State(state): State<AppState>,
    maybe_user: MaybeAuthUser,
    Json(req): Json<CreateInstanceRequest>,
) -> Result<Json<CreateInstanceResponse>, (StatusCode, String)> {
    match state
        .instance_manager
        .create(req.name, req.working_dir, req.command)
        .await
    {
        Ok(instance) => {
            state.metrics.instance_created();

            let is_claude = instance.command.contains("claude");
            let created_at: DateTime<Utc> =
                instance.created_at.parse().unwrap_or_else(|_| Utc::now());

            if let Some(handle) = state.instance_manager.get_handle(&instance.id).await {
                state
                    .global_state_manager
                    .register_instance(
                        instance.id.clone(),
                        handle,
                        instance.working_dir.clone(),
                        created_at,
                        is_claude,
                    )
                    .await;
            }

            if state.auth_config.enabled {
                if let MaybeAuthUser(Some(ref user)) = maybe_user {
                    let perm = crate::models::InstancePermission {
                        instance_id: instance.id.clone(),
                        user_id: user.user_id.clone(),
                        role: "owner".to_string(),
                        granted_at: chrono::Utc::now().timestamp(),
                        granted_by: None,
                    };
                    if let Err(e) = state.repository.create_instance_permission(&perm).await {
                        tracing::warn!("Failed to set instance owner: {}", e);
                    }
                }
            }

            if is_claude {
                let persistor = Arc::new(InstancePersistor::new(
                    instance.id.clone(),
                    instance.working_dir.clone(),
                    state.persistence_service.clone(),
                ));
                persistor.clone().start_monitoring().await;
                state
                    .instance_persistors
                    .lock()
                    .await
                    .insert(instance.id.clone(), persistor);
            }

            state
                .global_state_manager
                .broadcast_lifecycle(ws::ServerMessage::InstanceCreated {
                    instance: instance.clone(),
                });

            Ok(Json(CreateInstanceResponse {
                id: instance.id.clone(),
                name: instance.name.clone(),
                wrapper_port: instance.wrapper_port,
            }))
        }
        Err(e) => {
            tracing::error!("Failed to create instance: {}", e);
            state.metrics.pty_error();
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create instance: {}", e),
            ))
        }
    }
}

pub async fn get_instance(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.instance_manager.get(&id).await {
        Some(instance) => Json(instance).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn delete_instance(
    State(state): State<AppState>,
    maybe_user: MaybeAuthUser,
    Path(id): Path<String>,
) -> StatusCode {
    if state.auth_config.enabled {
        if let MaybeAuthUser(Some(ref user)) = maybe_user {
            if !user.is_admin {
                match state
                    .repository
                    .check_instance_permission(&id, &user.user_id)
                    .await
                {
                    Ok(Some(perm)) if perm.role == "owner" => {}
                    _ => return StatusCode::FORBIDDEN,
                }
            }
        }
    }

    state.instance_persistors.lock().await.remove(&id);
    state.global_state_manager.unregister_instance(&id).await;

    if state.instance_manager.stop(&id).await {
        state.metrics.instance_stopped();

        state
            .global_state_manager
            .broadcast_lifecycle(ws::ServerMessage::InstanceStopped { instance_id: id });

        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
}

#[derive(Deserialize)]
pub struct SetCustomNameRequest {
    custom_name: Option<String>,
}

pub async fn set_custom_name(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SetCustomNameRequest>,
) -> Result<StatusCode, StatusCode> {
    let custom_name = req.custom_name.filter(|n| !n.trim().is_empty());

    state
        .instance_manager
        .set_custom_name(&id, custom_name.clone())
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    state
        .global_state_manager
        .broadcast_lifecycle(ws::ServerMessage::InstanceRenamed {
            instance_id: id,
            custom_name,
        });

    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_instance_output(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    if let Some(handle) = state.instance_manager.get_handle(&id).await {
        let max_bytes = state.server_config.websocket.max_history_replay_bytes;
        let output = handle.get_recent_output(max_bytes).await;
        Json(serde_json::json!({ "lines": output })).into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

// --- Instance invitation handlers ---

#[derive(Deserialize)]
pub struct CreateInvitationRequest {
    role: Option<String>,
    max_uses: Option<i32>,
    expires_in_hours: Option<i64>,
}

pub async fn create_invitation(
    State(state): State<AppState>,
    maybe_user: MaybeAuthUser,
    Path(instance_id): Path<String>,
    Json(req): Json<CreateInvitationRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let user = match maybe_user.0 {
        Some(u) => u,
        None => return Err(StatusCode::UNAUTHORIZED),
    };

    if !user.is_admin {
        match state
            .repository
            .check_instance_permission(&instance_id, &user.user_id)
            .await
        {
            Ok(Some(perm)) if perm.role == "owner" => {}
            _ => return Err(StatusCode::FORBIDDEN),
        }
    }

    let now = chrono::Utc::now().timestamp();
    let invite = crate::models::InstanceInvitation {
        invite_token: uuid::Uuid::new_v4().to_string(),
        instance_id,
        created_by: user.user_id,
        role: req.role.unwrap_or_else(|| "collaborator".to_string()),
        max_uses: req.max_uses,
        use_count: 0,
        expires_at: req.expires_in_hours.map(|h| now + h * 3600),
        created_at: now,
    };

    state
        .repository
        .create_invitation(&invite)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "invite_token": invite.invite_token,
    })))
}

pub async fn accept_invitation(
    State(state): State<AppState>,
    maybe_user: MaybeAuthUser,
    Path(token): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let user = match maybe_user.0 {
        Some(u) => u,
        None => return Err(StatusCode::UNAUTHORIZED),
    };

    let invite = match state.repository.get_invitation(&token).await {
        Ok(Some(i)) => i,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    if invite.is_expired() || invite.is_used_up() {
        return Err(StatusCode::GONE);
    }

    let perm = crate::models::InstancePermission {
        instance_id: invite.instance_id.clone(),
        user_id: user.user_id,
        role: invite.role.clone(),
        granted_at: chrono::Utc::now().timestamp(),
        granted_by: Some(invite.created_by.clone()),
    };

    state
        .repository
        .create_instance_permission(&perm)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    state
        .repository
        .accept_invitation(&token)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(serde_json::json!({
        "instance_id": invite.instance_id,
        "role": invite.role,
    })))
}

pub async fn remove_collaborator(
    State(state): State<AppState>,
    maybe_user: MaybeAuthUser,
    Path((instance_id, target_user_id)): Path<(String, String)>,
) -> StatusCode {
    let user = match maybe_user.0 {
        Some(u) => u,
        None => return StatusCode::UNAUTHORIZED,
    };

    if !user.is_admin {
        match state
            .repository
            .check_instance_permission(&instance_id, &user.user_id)
            .await
        {
            Ok(Some(perm)) if perm.role == "owner" => {}
            _ => return StatusCode::FORBIDDEN,
        }
    }

    match state
        .repository
        .delete_instance_permission(&instance_id, &target_user_id)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
