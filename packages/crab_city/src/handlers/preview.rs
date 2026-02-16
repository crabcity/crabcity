//! Unauthenticated preview WebSocket at `/api/preview`.
//!
//! Sends periodic `PreviewActivity` snapshots so the join page can show
//! live instance/user counts without requiring authentication.

use axum::{
    extract::ws::{Message, WebSocket},
    extract::{ConnectInfo, State, WebSocketUpgrade},
    response::Response,
};
use futures::SinkExt;
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::debug;

use crate::AppState;

/// Global counter of active preview connections (rate limit).
static PREVIEW_CONNECTIONS: AtomicUsize = AtomicUsize::new(0);
const MAX_PREVIEW_CONNECTIONS_PER_IP: usize = 5;
const MAX_PREVIEW_CONNECTIONS_TOTAL: usize = 50;

#[derive(Serialize)]
struct PreviewActivity {
    terminal_count: usize,
    user_count: usize,
    instance_name: String,
    uptime_secs: u64,
}

static START_TIME: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

fn uptime_secs() -> u64 {
    START_TIME
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs()
}

pub async fn preview_websocket_handler(
    State(state): State<AppState>,
    ConnectInfo(_addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> Response {
    let current = PREVIEW_CONNECTIONS.load(Ordering::Relaxed);
    if current >= MAX_PREVIEW_CONNECTIONS_TOTAL {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "too many preview connections",
        )
            .into_response();
    }

    let instance_manager = state.instance_manager.clone();
    let state_manager = state.global_state_manager.clone();

    ws.on_upgrade(move |socket| handle_preview(socket, instance_manager, state_manager))
}

use crate::instance_manager::InstanceManager;
use crate::ws::GlobalStateManager;
use axum::response::IntoResponse;

async fn handle_preview(
    socket: WebSocket,
    instance_manager: Arc<InstanceManager>,
    state_manager: Arc<GlobalStateManager>,
) {
    PREVIEW_CONNECTIONS.fetch_add(1, Ordering::Relaxed);

    let (mut sender, mut _receiver) = socket.split();
    use futures::StreamExt;

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let instances = instance_manager.list().await;
                let terminal_count = instances.iter().filter(|i| i.running).count();
                let user_count = state_manager.total_connected_users().await;

                let activity = PreviewActivity {
                    terminal_count,
                    user_count,
                    instance_name: "Crab City".into(),
                    uptime_secs: uptime_secs(),
                };

                let json = match serde_json::to_string(&activity) {
                    Ok(j) => j,
                    Err(_) => break,
                };

                if sender.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
            msg = futures::StreamExt::next(&mut _receiver) => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {} // ignore client messages
                }
            }
        }
    }

    PREVIEW_CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
    debug!("preview connection closed");
}
