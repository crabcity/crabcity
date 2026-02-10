use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};

use super::types::*;
use crate::AppState;

/// Fuzzy match a pattern against a string, returning match indices if successful
pub fn fuzzy_match(pattern: &str, text: &str) -> Option<(Vec<usize>, i32)> {
    if pattern.is_empty() {
        return Some((vec![], 0));
    }

    let pattern_lower: Vec<char> = pattern.to_lowercase().chars().collect();
    let text_lower: Vec<char> = text.to_lowercase().chars().collect();
    let text_chars: Vec<char> = text.chars().collect();

    let mut indices = Vec::with_capacity(pattern_lower.len());
    let mut pattern_idx = 0;

    for (i, &c) in text_lower.iter().enumerate() {
        if pattern_idx < pattern_lower.len() && c == pattern_lower[pattern_idx] {
            indices.push(i);
            pattern_idx += 1;
        }
    }

    if pattern_idx != pattern_lower.len() {
        return None;
    }

    // Calculate score (lower is better)
    let mut score: i32 = 0;

    // Prefer matches at start
    if !indices.is_empty() && indices[0] == 0 {
        score -= 15;
    }

    // Prefer consecutive matches
    for i in 1..indices.len() {
        if indices[i] == indices[i - 1] + 1 {
            score -= 5;
        } else {
            score += (indices[i] - indices[i - 1]) as i32;
        }
    }

    // Prefer shorter strings
    score += (text.len() as i32) / 3;

    // Prefer exact case matches
    for (idx, &pattern_char) in pattern.chars().collect::<Vec<_>>().iter().enumerate() {
        if idx < indices.len() && text_chars[indices[idx]] == pattern_char {
            score -= 2;
        }
    }

    Some((indices, score))
}

/// Check if a query looks like a glob pattern
pub fn is_glob_pattern(query: &str) -> bool {
    query.contains('*') || query.contains('?') || query.contains('[')
}

/// Search for files recursively within the instance's working directory
/// Supports both fuzzy matching and glob patterns (e.g., *.rs, src/**/*.ts)
pub async fn search_instance_files(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<FileSearchQuery>,
) -> Response {
    use ignore::WalkBuilder;
    use ignore::overrides::OverrideBuilder;

    let instance = match state.instance_manager.get(&id).await {
        Some(inst) => inst,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let working_dir = std::path::Path::new(&instance.working_dir);

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

    let search_query = query.q.trim();
    if search_query.is_empty() {
        return Json(FileSearchResponse {
            query: search_query.to_string(),
            results: vec![],
            truncated: false,
        })
        .into_response();
    }

    let is_glob = is_glob_pattern(search_query);
    let max_results = query.limit.min(500);

    // Build the glob matcher if needed
    let glob_matcher = if is_glob {
        let mut builder = OverrideBuilder::new(&canonical_working);
        let pattern = if search_query.starts_with('*')
            || search_query.starts_with('/')
            || search_query.contains('/')
        {
            search_query.to_string()
        } else {
            format!("**/{}", search_query)
        };

        if let Err(e) = builder.add(&pattern) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Invalid glob pattern: {}", e) })),
            )
                .into_response();
        }

        match builder.build() {
            Ok(m) => Some(m),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": format!("Invalid glob pattern: {}", e) })),
                )
                    .into_response();
            }
        }
    } else {
        None
    };

    let walker = WalkBuilder::new(&canonical_working)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .follow_links(true)
        .max_depth(Some(25))
        .build();

    let mut results: Vec<FileSearchResult> = Vec::new();
    let mut total_matched = 0;

    for entry in walker.flatten() {
        if entry.path() == canonical_working {
            continue;
        }

        let path = entry.path();
        let file_name = match path.file_name() {
            Some(name) => name.to_string_lossy(),
            None => continue,
        };

        let relative_path = path
            .strip_prefix(&canonical_working)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        let is_directory = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);

        let matched = if let Some(ref matcher) = glob_matcher {
            matcher.matched(path, is_directory).is_whitelist()
        } else {
            fuzzy_match(search_query, &file_name).is_some()
        };

        if matched {
            total_matched += 1;

            let score = if is_glob {
                (relative_path.matches('/').count() as i32) * 10
            } else {
                fuzzy_match(search_query, &file_name)
                    .map(|(_, s)| s)
                    .unwrap_or(0)
            };

            results.push(FileSearchResult {
                name: file_name.to_string(),
                path: path.to_string_lossy().to_string(),
                relative_path,
                is_directory,
                score,
            });

            if results.len() > max_results * 2 {
                results.sort_by_key(|r| r.score);
                results.truncate(max_results);
            }
        }
    }

    results.sort_by_key(|r| r.score);
    let truncated = results.len() > max_results;
    results.truncate(max_results);

    Json(FileSearchResponse {
        query: search_query.to_string(),
        results,
        truncated: truncated || total_matched > max_results,
    })
    .into_response()
}
