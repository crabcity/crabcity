use axum::{
    extract::{Path, State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};

use crate::AppState;
use crate::auth::MaybeAuthUser;
use crate::ws;

pub async fn websocket_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    let handle = match state.instance_manager.get_handle(&id).await {
        Some(h) => h,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let instance = match state.instance_manager.get(&id).await {
        Some(i) => i,
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let instance_created_at: DateTime<Utc> =
        instance.created_at.parse().unwrap_or_else(|_| Utc::now());

    let instance_id = id.clone();
    let is_claude = instance.command.contains("claude");
    let convo_config = if is_claude {
        Some(crate::websocket_proxy::ConversationConfig {
            working_dir: instance.working_dir.clone(),
            session_id: instance.session_id.clone(),
            is_claude,
            instance_created_at,
        })
    } else {
        None
    };

    ws.on_upgrade(move |socket| {
        crate::websocket_proxy::handle_proxy(socket, instance_id, handle, convo_config)
    })
}

/// Multiplexed WebSocket handler - single connection for all instances
pub async fn multiplexed_websocket_handler(
    State(state): State<AppState>,
    maybe_user: MaybeAuthUser,
    ws: WebSocketUpgrade,
) -> Response {
    let instance_manager = state.instance_manager.clone();
    let global_state_manager = state.global_state_manager.clone();
    let server_config = state.server_config.clone();
    let metrics = state.metrics.clone();
    let repository = state.repository.clone();
    let auth_enabled = state.auth_config.enabled;

    let ws_user = maybe_user.0.map(|u| ws::WsUser {
        user_id: u.user_id,
        display_name: u.display_name,
    });

    ws.on_upgrade(move |socket| {
        ws::handle_multiplexed_ws(
            socket,
            instance_manager,
            global_state_manager,
            Some(server_config),
            Some(metrics),
            ws_user,
            if auth_enabled { Some(repository) } else { None },
        )
    })
}
