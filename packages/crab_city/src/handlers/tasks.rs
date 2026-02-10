use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::Deserialize;
use sqlx::Row;
use uuid::Uuid;

use crate::AppState;
use crate::auth::MaybeAuthUser;

pub async fn list_tasks_handler(
    State(state): State<AppState>,
    _maybe_user: MaybeAuthUser,
    Query(filters): Query<crate::models::TaskListFilters>,
) -> Result<Json<Vec<crate::models::TaskWithTags>>, (StatusCode, String)> {
    match state.repository.list_tasks(&filters).await {
        Ok(tasks) => Ok(Json(tasks)),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

pub async fn create_task_handler(
    State(state): State<AppState>,
    maybe_user: MaybeAuthUser,
    Json(req): Json<crate::models::CreateTaskRequest>,
) -> Result<Json<crate::models::TaskWithTags>, (StatusCode, String)> {
    let now = chrono::Utc::now().timestamp();
    let sort_order = state
        .repository
        .get_next_sort_order(req.instance_id.as_deref())
        .await
        .unwrap_or(1.0);

    let (creator_id, creator_name) = match &maybe_user {
        MaybeAuthUser(Some(user)) => (Some(user.user_id.clone()), user.display_name.clone()),
        _ => (None, "anonymous".to_string()),
    };

    let task = crate::models::Task {
        id: None,
        uuid: Uuid::new_v4().to_string(),
        title: req.title.clone(),
        body: req.body.clone(),
        status: req.status.unwrap_or_else(|| "pending".to_string()),
        priority: req.priority.unwrap_or(0),
        instance_id: req.instance_id.clone(),
        creator_id,
        creator_name,
        sort_order,
        created_at: now,
        updated_at: now,
        completed_at: None,
        is_deleted: false,
        sent_text: None,
        conversation_id: None,
    };

    let id = state
        .repository
        .create_task(&task)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Add tags if provided
    if let Some(tags) = &req.tags {
        for tag_name in tags {
            let _ = state.repository.add_task_tag(id, tag_name).await;
        }
    }

    // Get the newly created task
    let task = state
        .repository
        .get_task(id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "Task not found after creation".to_string(),
            )
        })?;

    Ok(Json(crate::models::TaskWithTags {
        task,
        tags: vec![],
        dispatches: vec![],
    }))
}

pub async fn get_task_handler(
    State(state): State<AppState>,
    _maybe_user: MaybeAuthUser,
    Path(id): Path<i64>,
) -> Result<Json<crate::models::TaskWithTags>, (StatusCode, String)> {
    let task = state
        .repository
        .get_task(id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Task not found".to_string()))?;

    // Fetch tags
    let tag_rows = sqlx::query(
        "SELECT tg.id, tg.name, tg.color FROM tags tg JOIN task_tags tt ON tg.id = tt.tag_id WHERE tt.task_id = ?",
    )
    .bind(id)
    .fetch_all(&state.db.pool)
    .await
    .unwrap_or_default();

    let tags: Vec<crate::models::Tag> = tag_rows
        .into_iter()
        .map(|r| crate::models::Tag {
            id: r.get("id"),
            name: r.get("name"),
            color: r.get("color"),
        })
        .collect();

    // Fetch dispatches
    let dispatches = state
        .repository
        .get_dispatches_for_tasks(&[id])
        .await
        .unwrap_or_default();

    Ok(Json(crate::models::TaskWithTags {
        task,
        tags,
        dispatches,
    }))
}

pub async fn update_task_handler(
    State(state): State<AppState>,
    _maybe_user: MaybeAuthUser,
    Path(id): Path<i64>,
    Json(req): Json<crate::models::UpdateTaskRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state
        .repository
        .update_task(id, &req)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn delete_task_handler(
    State(state): State<AppState>,
    _maybe_user: MaybeAuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state
        .repository
        .delete_task(id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn send_task_handler(
    State(state): State<AppState>,
    _maybe_user: MaybeAuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let task = state
        .repository
        .get_task(id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Task not found".to_string()))?;

    let instance_id = task.instance_id.as_ref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Task has no assigned instance".to_string(),
        )
    })?;

    let handle = state
        .instance_manager
        .get_handle(instance_id)
        .await
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "Instance not found or not running".to_string(),
            )
        })?;

    let text = task.body.as_deref().unwrap_or(&task.title);
    handle.write_input(text).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to write to PTY: {}", e),
        )
    })?;

    handle.write_input("\r").await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to send Enter: {}", e),
        )
    })?;

    let _ = state
        .repository
        .create_task_dispatch(id, instance_id, text)
        .await;

    if task.status == "pending" {
        let update = crate::models::UpdateTaskRequest {
            status: Some("in_progress".to_string()),
            ..Default::default()
        };
        let _ = state.repository.update_task(id, &update).await;
    }

    Ok(Json(
        serde_json::json!({ "ok": true, "status": "in_progress" }),
    ))
}

#[derive(Deserialize)]
pub struct CreateDispatchRequest {
    pub instance_id: String,
    pub sent_text: String,
}

pub async fn create_dispatch_handler(
    State(state): State<AppState>,
    _maybe_user: MaybeAuthUser,
    Path(id): Path<i64>,
    Json(req): Json<CreateDispatchRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let task = state
        .repository
        .get_task(id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Task not found".to_string()))?;

    let dispatch = state
        .repository
        .create_task_dispatch(id, &req.instance_id, &req.sent_text)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if task.status == "pending" {
        let update = crate::models::UpdateTaskRequest {
            status: Some("in_progress".to_string()),
            ..Default::default()
        };
        let _ = state.repository.update_task(id, &update).await;
    }

    Ok(Json(serde_json::json!({
        "ok": true,
        "dispatch_id": dispatch.id,
        "status": if task.status == "pending" { "in_progress" } else { &task.status }
    })))
}

#[derive(Deserialize)]
pub struct AddTagRequest {
    pub tag: String,
}

pub async fn add_task_tag_handler(
    State(state): State<AppState>,
    _maybe_user: MaybeAuthUser,
    Path(id): Path<i64>,
    Json(req): Json<AddTagRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state
        .repository
        .add_task_tag(id, &req.tag)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn remove_task_tag_handler(
    State(state): State<AppState>,
    _maybe_user: MaybeAuthUser,
    Path((id, tag_id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    state
        .repository
        .remove_task_tag(id, tag_id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn migrate_tasks_handler(
    State(state): State<AppState>,
    _maybe_user: MaybeAuthUser,
    Json(items): Json<Vec<crate::models::MigrateTaskItem>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let ids = state
        .repository
        .migrate_tasks(&items)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({ "ok": true, "ids": ids })))
}
