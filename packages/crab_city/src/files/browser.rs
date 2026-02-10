use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};

use super::types::*;
use crate::AppState;

/// List files in a directory within the instance's working directory
pub async fn list_instance_files(
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
            return Json(DirectoryListing {
                path: query.path,
                entries: vec![],
                error: Some(format!("Invalid working directory: {}", e)),
            })
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
            return Json(DirectoryListing {
                path: query.path,
                entries: vec![],
                error: Some(format!("Path not found: {}", e)),
            })
            .into_response();
        }
    };

    // Security check: target must be within working_dir
    if !canonical_target.starts_with(&canonical_working) {
        return Json(DirectoryListing {
            path: query.path,
            entries: vec![],
            error: Some("Access denied: path outside working directory".to_string()),
        })
        .into_response();
    }

    // Read directory
    let entries = match std::fs::read_dir(&canonical_target) {
        Ok(entries) => entries,
        Err(e) => {
            return Json(DirectoryListing {
                path: query.path,
                entries: vec![],
                error: Some(format!("Cannot read directory: {}", e)),
            })
            .into_response();
        }
    };

    let mut file_entries: Vec<FileEntry> = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files (starting with .)
        if name.starts_with('.') {
            continue;
        }

        let path = entry.path();

        let symlink_meta = std::fs::symlink_metadata(&path).ok();
        let is_symlink = symlink_meta
            .as_ref()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);

        let symlink_target = if is_symlink {
            std::fs::read_link(&path)
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        } else {
            None
        };

        let metadata = entry.metadata().ok();

        let is_directory = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = metadata
            .as_ref()
            .and_then(|m| if m.is_file() { Some(m.len()) } else { None });
        let modified_at = metadata.and_then(|m| {
            m.modified().ok().map(|t| {
                let datetime: DateTime<Utc> = t.into();
                datetime.to_rfc3339()
            })
        });

        file_entries.push(FileEntry {
            name,
            path: path.to_string_lossy().to_string(),
            is_directory,
            is_symlink,
            symlink_target,
            size,
            modified_at,
        });
    }

    // Sort: directories first, then alphabetically
    file_entries.sort_by(|a, b| match (a.is_directory, b.is_directory) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Json(DirectoryListing {
        path: canonical_target.to_string_lossy().to_string(),
        entries: file_entries,
        error: None,
    })
    .into_response()
}
