use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};

use super::types::FilePathQuery;
use crate::AppState;

/// Get the content of a file within the instance's working directory
pub async fn get_instance_file_content(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<FilePathQuery>,
) -> Response {
    let instance = match state.instance_manager.get(&id).await {
        Some(inst) => inst,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let working_dir = std::path::Path::new(&instance.working_dir);
    let requested_path = std::path::Path::new(&query.path);

    // Security: ensure the requested path is within working_dir
    let canonical_working = match working_dir.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Invalid working directory: {}", e) })),
            )
                .into_response();
        }
    };

    let target_path = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        working_dir.join(requested_path)
    };

    let canonical_target = match target_path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": format!("File not found: {}", e) })),
            )
                .into_response();
        }
    };

    // Security check: target must be within working_dir
    if !canonical_target.starts_with(&canonical_working) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": "Access denied: symlink points outside project directory" })),
        )
            .into_response();
    }

    if canonical_target.is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Path is a directory, not a file" })),
        )
            .into_response();
    }

    // Check file size (limit to 1MB to prevent memory issues)
    const MAX_FILE_SIZE: u64 = 1024 * 1024; // 1MB
    if let Ok(metadata) = std::fs::metadata(&canonical_target) {
        if metadata.len() > MAX_FILE_SIZE {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("File too large ({}MB). Max size is 1MB.", metadata.len() / (1024 * 1024))
                })),
            )
                .into_response();
        }
    }

    match std::fs::read_to_string(&canonical_target) {
        Ok(content) => Json(serde_json::json!({ "content": content })).into_response(),
        Err(e) => {
            if e.kind() == std::io::ErrorKind::InvalidData {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "File is not valid UTF-8 text" })),
                )
                    .into_response();
            }
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Cannot read file: {}", e) })),
            )
                .into_response()
        }
    }
}
