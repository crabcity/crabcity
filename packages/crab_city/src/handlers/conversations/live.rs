use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use toolpath_claude::ClaudeConvo;
use toolpath_convo::ConversationProvider;
use tracing::{debug, info, warn};

use super::format::{format_entry_with_attribution, format_turn_with_attribution};
use crate::AppState;

pub async fn get_conversation(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let instance = match state.instance_manager.get(&id).await {
        Some(inst) => inst,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    if !instance.command.contains("claude") {
        return Json(serde_json::json!({
            "error": "Not a Claude instance",
            "turns": []
        }))
        .into_response();
    }

    let convo_manager = ClaudeConvo::new();
    let working_dir = &instance.working_dir;

    let session_id = if let Some(sid) = &instance.session_id {
        info!("Using cached session_id: {}", sid);
        sid.clone()
    } else {
        info!(
            "Detecting session for instance {} (created_at: {})",
            instance.id, instance.created_at
        );

        let instance_created: DateTime<Utc> = match instance.created_at.parse() {
            Ok(dt) => dt,
            Err(e) => {
                tracing::error!("Failed to parse instance created_at: {}", e);
                return Json(serde_json::json!({
                    "error": "Failed to parse instance timestamp",
                    "turns": []
                }))
                .into_response();
            }
        };

        let metadata = match convo_manager.list_conversation_metadata(working_dir) {
            Ok(m) => m,
            Err(e) => {
                tracing::error!("Failed to list conversations: {}", e);
                return Json(serde_json::json!({
                    "error": format!("Failed to list conversations: {}", e),
                    "turns": []
                }))
                .into_response();
            }
        };

        info!(
            "Found {} conversations in {}, filtering by created_at >= {}",
            metadata.len(),
            working_dir,
            instance_created
        );

        let candidates: Vec<_> = metadata
            .iter()
            .filter(|m| {
                if let Some(started) = m.started_at {
                    started >= instance_created
                } else {
                    false
                }
            })
            .collect();

        info!("Found {} candidate conversations", candidates.len());

        if candidates.is_empty() {
            debug!("No conversation found for instance yet");
            return Json(serde_json::json!({
                "turns": []
            }))
            .into_response();
        }

        let detected_session = &candidates[0].session_id;
        info!("Detected session_id: {}", detected_session);

        if let Some(handle) = state.instance_manager.get_handle(&id).await {
            if let Err(e) = handle.set_session_id(detected_session.clone()).await {
                tracing::warn!("Failed to cache session_id: {}", e);
            }
        }

        detected_session.clone()
    };

    match ConversationProvider::load_conversation(&convo_manager, working_dir, &session_id) {
        Ok(view) => {
            let mut turns = Vec::with_capacity(view.turns.len());
            for turn in &view.turns {
                turns.push(
                    format_turn_with_attribution(
                        turn,
                        &id,
                        Some(&state.repository),
                        Some(&state.global_state_manager),
                    )
                    .await,
                );
            }

            info!(
                "Found conversation {} with {} turns",
                session_id,
                turns.len()
            );
            Json(serde_json::json!({
                "conversation_id": view.id,
                "turns": turns,
                "files_changed": view.files_changed,
                "total_usage": view.total_usage,
                "provider_id": view.provider_id,
            }))
            .into_response()
        }
        Err(e) => {
            tracing::error!("Failed to read conversation {}: {}", session_id, e);
            Json(serde_json::json!({
                "error": format!("Failed to read conversation: {}", e),
                "turns": []
            }))
            .into_response()
        }
    }
}

/// Poll for new conversation entries (uses watcher to return only unseen entries)
pub async fn poll_conversation(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let instance = match state.instance_manager.get(&id).await {
        Some(inst) => inst,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    if !instance.command.contains("claude") {
        return Json(serde_json::json!({
            "error": "Not a Claude instance",
            "new_turns": []
        }))
        .into_response();
    }

    let session_id = match &instance.session_id {
        Some(sid) => sid.clone(),
        None => {
            let convo_manager = ClaudeConvo::new();
            let instance_created: DateTime<Utc> = match instance.created_at.parse() {
                Ok(dt) => dt,
                Err(_) => {
                    return Json(serde_json::json!({
                        "new_turns": [],
                        "waiting": true
                    }))
                    .into_response();
                }
            };

            let metadata = match convo_manager.list_conversation_metadata(&instance.working_dir) {
                Ok(m) => m,
                Err(_) => {
                    return Json(serde_json::json!({
                        "new_turns": [],
                        "waiting": true
                    }))
                    .into_response();
                }
            };

            let candidates: Vec<_> = metadata
                .iter()
                .filter(|m| m.started_at.map(|s| s >= instance_created).unwrap_or(false))
                .collect();

            if candidates.is_empty() {
                return Json(serde_json::json!({
                    "new_turns": [],
                    "waiting": true
                }))
                .into_response();
            }

            let detected = candidates[0].session_id.clone();

            if let Some(handle) = state.instance_manager.get_handle(&id).await {
                if let Err(e) = handle.set_session_id(detected.clone()).await {
                    warn!(instance = %id, session = %detected, "Failed to cache session ID: {}", e);
                }
            }

            detected
        }
    };

    let mut watchers = state.conversation_watchers.lock().await;
    let watcher = watchers.entry(id.clone()).or_insert_with(|| {
        toolpath_claude::ConversationWatcher::new(
            ClaudeConvo::new(),
            instance.working_dir.clone(),
            session_id.clone(),
        )
    });

    match watcher.poll() {
        Ok(new_entries) => {
            let mut turns = Vec::with_capacity(new_entries.len());
            for entry in &new_entries {
                if super::format::is_tool_result_only(entry) {
                    continue;
                }
                if let Some(turn) = toolpath_claude::provider::to_turn(entry) {
                    turns.push(
                        format_turn_with_attribution(
                            &turn,
                            &id,
                            Some(&state.repository),
                            Some(&state.global_state_manager),
                        )
                        .await,
                    );
                } else {
                    // Progress/unknown entries — fall back to entry-based formatting
                    turns.push(
                        format_entry_with_attribution(
                            entry,
                            &id,
                            Some(&state.repository),
                            Some(&state.global_state_manager),
                        )
                        .await,
                    );
                }
            }

            Json(serde_json::json!({
                "new_turns": turns,
                "total_seen": watcher.seen_count()
            }))
            .into_response()
        }
        Err(e) => {
            tracing::error!("Failed to poll conversation: {}", e);
            Json(serde_json::json!({
                "error": format!("Failed to poll: {}", e),
                "new_turns": []
            }))
            .into_response()
        }
    }
}
