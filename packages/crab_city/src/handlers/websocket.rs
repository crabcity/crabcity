use axum::{
    extract::{ConnectInfo, State, WebSocketUpgrade},
    response::Response,
};
use std::net::SocketAddr;

use crate::AppState;
use crate::ws;

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
