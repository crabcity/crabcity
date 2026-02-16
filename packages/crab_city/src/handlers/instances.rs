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
use crate::persistence::InstancePersistor;
use crate::ws;

#[derive(Serialize)]
pub struct CreateInstanceResponse {
    id: String,
    name: String,
    wrapper_port: u16,
}

pub async fn list_instances(State(state): State<AppState>) -> Json<Vec<ClaudeInstance>> {
    Json(state.instance_manager.list().await)
}

#[derive(Deserialize)]
pub struct CreateInstanceRequest {
    name: Option<String>,
    working_dir: Option<String>,
    command: Option<String>,
}

pub async fn create_instance(
    State(state): State<AppState>,
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

pub async fn delete_instance(State(state): State<AppState>, Path(id): Path<String>) -> StatusCode {
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::Request,
        routing::{delete, get, patch, post},
    };
    use tower::ServiceExt;

    async fn test_router() -> (Router, tempfile::TempDir) {
        let (state, tmp) = crate::test_helpers::test_app_state().await;
        let router = Router::new()
            .route("/instances", get(list_instances))
            .route("/instances/{id}", get(get_instance))
            .route("/instances/{id}", delete(delete_instance))
            .route("/instances/{id}/name", patch(set_custom_name))
            .route("/instances/{id}/output", get(get_instance_output))
            .with_state(state);
        (router, tmp)
    }

    #[tokio::test]
    async fn test_list_instances_empty() {
        let (app, _tmp) = test_router().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/instances")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let instances: Vec<ClaudeInstance> = serde_json::from_slice(&body).unwrap();
        assert!(instances.is_empty());
    }

    #[tokio::test]
    async fn test_get_instance_not_found() {
        let (app, _tmp) = test_router().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/instances/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_delete_instance_not_found() {
        let (app, _tmp) = test_router().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/instances/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_get_instance_output_not_found() {
        let (app, _tmp) = test_router().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/instances/nonexistent/output")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_create_instance_request_deserialization() {
        let json = r#"{"name": "test", "working_dir": "/tmp", "command": "echo hello"}"#;
        let req: CreateInstanceRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name.as_deref(), Some("test"));
        assert_eq!(req.working_dir.as_deref(), Some("/tmp"));
        assert_eq!(req.command.as_deref(), Some("echo hello"));
    }

    #[tokio::test]
    async fn test_create_instance_request_all_optional() {
        let json = r#"{}"#;
        let req: CreateInstanceRequest = serde_json::from_str(json).unwrap();
        assert!(req.name.is_none());
        assert!(req.working_dir.is_none());
        assert!(req.command.is_none());
    }

    #[tokio::test]
    async fn test_set_custom_name_request_deserialization() {
        let json = r#"{"custom_name": "My Crab"}"#;
        let req: SetCustomNameRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.custom_name.as_deref(), Some("My Crab"));

        let json_null = r#"{"custom_name": null}"#;
        let req2: SetCustomNameRequest = serde_json::from_str(json_null).unwrap();
        assert!(req2.custom_name.is_none());
    }

    #[tokio::test]
    async fn test_create_and_get_instance() {
        let (state, _tmp) = crate::test_helpers::test_app_state().await;
        let router = Router::new()
            .route("/instances", get(list_instances).post(create_instance))
            .route("/instances/{id}", get(get_instance))
            .route("/instances/{id}", delete(delete_instance))
            .route("/instances/{id}/name", patch(set_custom_name))
            .route("/instances/{id}/output", get(get_instance_output))
            .with_state(state);

        // Create an instance with a short-lived command
        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"command":"echo hello","working_dir":"/tmp"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let instance_id = created["id"].as_str().unwrap().to_string();
        assert!(!instance_id.is_empty());

        // Get instance by ID
        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/instances/{}", instance_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let instance: ClaudeInstance = serde_json::from_slice(&body).unwrap();
        assert_eq!(instance.id, instance_id);
        assert_eq!(instance.command, "echo hello");

        // List instances - should have at least one
        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/instances")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let instances: Vec<ClaudeInstance> = serde_json::from_slice(&body).unwrap();
        assert!(!instances.is_empty());
        assert!(instances.iter().any(|i| i.id == instance_id));

        // Get instance output
        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/instances/{}/output", instance_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Set custom name
        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/instances/{}/name", instance_id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"custom_name":"My Echo"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Delete instance
        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/instances/{}", instance_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Verify it's gone
        let resp = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/instances/{}", instance_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_create_instance_with_custom_name() {
        let (state, _tmp) = crate::test_helpers::test_app_state().await;
        let router = Router::new()
            .route("/instances", post(create_instance))
            .with_state(state);

        // Create with custom name, default command (echo)
        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/instances")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"my-crab","working_dir":"/tmp"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(created["name"], "my-crab");
    }

    #[tokio::test]
    async fn test_create_instance_response_serialization() {
        let resp = CreateInstanceResponse {
            id: "inst-1".to_string(),
            name: "swift-azure-falcon".to_string(),
            wrapper_port: 9001,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["id"], "inst-1");
        assert_eq!(json["name"], "swift-azure-falcon");
        assert_eq!(json["wrapper_port"], 9001);
    }
}
