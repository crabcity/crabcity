use axum::{
    extract::{ConnectInfo, Path, State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use std::net::SocketAddr;

use crate::AppState;
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

    let global_state_manager = state.global_state_manager.clone();

    ws.on_upgrade(move |socket| {
        crate::websocket_proxy::handle_proxy(
            socket,
            instance_id,
            handle,
            convo_config,
            Some(global_state_manager),
        )
    })
}

/// Multiplexed WebSocket handler - single connection for all instances.
///
/// Auth is handled inside the WS connection via challenge-response handshake,
/// so this endpoint is in the public routes list (no middleware auth needed).
pub async fn multiplexed_websocket_handler(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Response {
    let instance_manager = state.instance_manager.clone();
    let global_state_manager = state.global_state_manager.clone();
    let server_config = state.server_config.clone();
    let metrics = state.metrics.clone();
    let repository = state.repository.clone();
    let identity = state.identity.clone();

    let is_loopback = addr.ip().is_loopback();

    ws.on_upgrade(move |socket| {
        ws::handle_multiplexed_ws(
            socket,
            instance_manager,
            global_state_manager,
            Some(server_config),
            Some(metrics),
            repository,
            identity,
            is_loopback,
        )
    })
}
