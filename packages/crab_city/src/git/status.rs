use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};

use super::executor::run_git;
use super::types::*;
use crate::AppState;

/// GET /api/instances/{id}/git/status
pub async fn get_git_status(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let instance = match state.instance_manager.get(&id).await {
        Some(inst) => inst,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let wd = &instance.working_dir;
    let output = match run_git(wd, &["status", "--porcelain=v2", "--branch"]).await {
        Ok(o) => o,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
                .into_response();
        }
    };

    let mut branch = String::new();
    let mut ahead_behind: Option<(i64, i64)> = None;
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();

    for line in output.lines() {
        if line.starts_with("# branch.head ") {
            branch = line[15..].to_string();
        } else if line.starts_with("# branch.ab ") {
            // Parse "# branch.ab +N -M"
            let ab = &line[13..];
            let parts: Vec<&str> = ab.split_whitespace().collect();
            if parts.len() == 2 {
                let a = parts[0].trim_start_matches('+').parse::<i64>().unwrap_or(0);
                let b = parts[1].trim_start_matches('-').parse::<i64>().unwrap_or(0);
                ahead_behind = Some((a, b));
            }
        } else if line.starts_with("? ") {
            // Untracked
            let path = line[2..].to_string();
            untracked.push(GitFileStatus {
                path,
                status: "untracked".to_string(),
                old_path: None,
            });
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            // Changed entry: "1 XY ..." or rename "2 XY ..."
            let is_rename = line.starts_with("2 ");
            let parts: Vec<&str> = line.splitn(if is_rename { 10 } else { 9 }, ' ').collect();
            if parts.len() < 2 {
                continue;
            }
            let xy = parts[1];
            let x = xy.chars().next().unwrap_or('.');
            let y = xy.chars().nth(1).unwrap_or('.');

            let file_path = if is_rename {
                parts.last().unwrap_or(&"").to_string()
            } else {
                parts.last().unwrap_or(&"").to_string()
            };

            let old_path = if is_rename {
                let last = *parts.last().unwrap_or(&"");
                let rename_parts: Vec<&str> = last.splitn(2, '\t').collect();
                if rename_parts.len() == 2 {
                    Some(rename_parts[1].to_string())
                } else {
                    None
                }
            } else {
                None
            };

            // X = staged status
            if x != '.' && x != '?' {
                staged.push(GitFileStatus {
                    path: file_path.clone(),
                    status: porcelain_status_to_string(x),
                    old_path: old_path.clone(),
                });
            }
            // Y = unstaged (working tree) status
            if y != '.' && y != '?' {
                unstaged.push(GitFileStatus {
                    path: file_path,
                    status: porcelain_status_to_string(y),
                    old_path,
                });
            }
        }
    }

    Json(GitStatusResponse {
        branch,
        staged,
        unstaged,
        untracked,
        ahead_behind,
    })
    .into_response()
}

pub fn porcelain_status_to_string(c: char) -> String {
    match c {
        'M' => "modified".to_string(),
        'A' => "added".to_string(),
        'D' => "deleted".to_string(),
        'R' => "renamed".to_string(),
        'C' => "copied".to_string(),
        'T' => "type_changed".to_string(),
        'U' => "unmerged".to_string(),
        _ => format!("unknown({})", c),
    }
}
