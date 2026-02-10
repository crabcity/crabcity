use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};

use super::executor::run_git;
use super::types::*;
use crate::AppState;

/// GET /api/instances/{id}/git/branches
pub async fn get_git_branches(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let instance = match state.instance_manager.get(&id).await {
        Some(inst) => inst,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let wd = &instance.working_dir;

    // Get current branch
    let current = match run_git(wd, &["rev-parse", "--abbrev-ref", "HEAD"]).await {
        Ok(s) => s.trim().to_string(),
        Err(_) => String::new(),
    };

    // Get branch list with details
    let format = "%(HEAD)%00%(refname:short)%00%(objectname:short)%00%(committerdate:unix)%00%(subject)%00%(upstream:short)%00%(upstream:track,nobracket)%00%(refname)";
    let format_arg = format!("--format={}", format);
    let output = match run_git(wd, &["branch", "-a", &format_arg]).await {
        Ok(o) => o,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
                .into_response();
        }
    };

    let mut branches = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.splitn(8, '\0').collect();
        if fields.len() < 5 {
            continue;
        }

        let is_current = fields[0].trim() == "*";
        let name = fields[1].trim().to_string();
        let hash = fields[2].trim().to_string();
        let date = fields[3].trim().parse::<i64>().unwrap_or(0);
        let message = fields[4].trim().to_string();
        let upstream = fields.get(5).and_then(|s| {
            let s = s.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        });
        let track = fields.get(6).unwrap_or(&"").trim();
        let full_refname = fields.get(7).unwrap_or(&"").trim();
        let is_remote = full_refname.starts_with("refs/remotes/");

        // Parse ahead/behind from track info like "ahead 2, behind 3" or "ahead 1"
        let (ahead, behind) = parse_ahead_behind(track);

        branches.push(GitBranch {
            name,
            current: is_current,
            remote: is_remote,
            last_commit_hash: hash,
            last_commit_date: date,
            last_commit_message: message,
            upstream,
            ahead,
            behind,
        });
    }

    // Get instance branches — which branch is each instance on?
    let all_instances = state.instance_manager.list().await;
    let mut instance_branches = Vec::new();
    for inst in &all_instances {
        if inst.working_dir == instance.working_dir {
            if let Ok(branch) =
                run_git(&inst.working_dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await
            {
                instance_branches.push(InstanceBranchInfo {
                    instance_id: inst.id.clone(),
                    instance_name: inst
                        .custom_name
                        .clone()
                        .unwrap_or_else(|| inst.name.clone()),
                    branch: branch.trim().to_string(),
                });
            }
        }
    }

    // Resolve the remote's default branch from origin/HEAD
    let default_branch = run_git(wd, &["symbolic-ref", "refs/remotes/origin/HEAD"])
        .await
        .ok()
        .and_then(|s| {
            let s = s.trim();
            // "refs/remotes/origin/main" → "main"
            s.strip_prefix("refs/remotes/origin/")
                .map(|b| b.to_string())
        });

    Json(GitBranchesResponse {
        branches,
        current,
        default_branch,
        instance_branches,
    })
    .into_response()
}

pub fn parse_ahead_behind(track: &str) -> (i64, i64) {
    let mut ahead = 0i64;
    let mut behind = 0i64;
    for part in track.split(',') {
        let part = part.trim();
        if part.starts_with("ahead ") {
            ahead = part[6..].trim().parse().unwrap_or(0);
        } else if part.starts_with("behind ") {
            behind = part[7..].trim().parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}
