// Library interface for tty_wrapper
// Exposes PTY management functionality for embedding in other applications

pub mod pty_actor;
pub mod pty_manager;
pub mod websocket;

pub use pty_actor::PtyActor;
pub use pty_manager::{OutputEvent, PtyManager};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{
        Query, State,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

#[derive(Clone)]
pub struct WrapperState {
    pub pty: Arc<PtyManager>,
}

// HTTP API handlers that can be mounted into any axum router
#[allow(dead_code)]
pub fn create_api_routes(state: WrapperState) -> Router {
    Router::new()
        .route("/state", get(get_state))
        .route("/input", post(send_input))
        .route("/output", get(get_output))
        .route("/history", get(get_history))
        .route("/resize", post(resize_terminal))
        .route("/kill", post(kill_process))
        .route("/ws", get(websocket_handler))
        .with_state(state)
}

// Start a PTY session with the given command
pub async fn start_pty_session(
    command: &str,
    args: &[String],
    working_dir: Option<&str>,
    show_output: bool,
) -> Result<Arc<PtyManager>> {
    let pty = PtyManager::spawn(command, args, working_dir, show_output)?;
    Ok(Arc::new(pty))
}

// API handlers
async fn get_state(State(state): State<WrapperState>) -> impl IntoResponse {
    match state.pty.get_state().await {
        Ok(pty_state) => (StatusCode::OK, Json(pty_state)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct InputRequest {
    text: String,
}

async fn send_input(
    State(state): State<WrapperState>,
    Json(req): Json<InputRequest>,
) -> impl IntoResponse {
    match state.pty.write_input(&req.text).await {
        Ok(bytes_written) => (
            StatusCode::OK,
            Json(serde_json::json!({ "bytes": bytes_written })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct OutputQuery {
    lines: Option<usize>,
}

async fn get_output(
    State(state): State<WrapperState>,
    Query(params): Query<OutputQuery>,
) -> impl IntoResponse {
    let lines = params.lines.unwrap_or(100).min(10000);
    let output = state.pty.get_recent_output(lines).await;
    (StatusCode::OK, Json(serde_json::json!({ "lines": output }))).into_response()
}

async fn get_history(State(state): State<WrapperState>) -> impl IntoResponse {
    let history = state.pty.get_full_output().await;
    (
        StatusCode::OK,
        Json(serde_json::json!({ "history": history })),
    )
        .into_response()
}

#[derive(Deserialize)]
struct ResizeRequest {
    rows: u16,
    cols: u16,
}

async fn resize_terminal(
    State(state): State<WrapperState>,
    Json(req): Json<ResizeRequest>,
) -> impl IntoResponse {
    match state.pty.resize(req.rows, req.cols).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "success": true }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct KillRequest {
    signal: Option<String>,
}

async fn kill_process(
    State(state): State<WrapperState>,
    Json(req): Json<KillRequest>,
) -> impl IntoResponse {
    match state.pty.kill(req.signal.as_deref()).await {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "success": true }))).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn websocket_handler(
    State(state): State<WrapperState>,
    ws: axum::extract::WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_websocket(socket, state))
}

#[derive(Serialize, Deserialize)]
struct WsMessage {
    #[serde(rename = "type")]
    msg_type: String,
    data: String,
}

async fn handle_websocket(socket: WebSocket, state: WrapperState) {
    use futures::sink::SinkExt;
    use futures::stream::StreamExt;

    let (mut sender, mut receiver) = socket.split();

    // Subscribe to output events
    let mut output_rx = state.pty.subscribe_output();

    // Task to forward PTY output to WebSocket
    let output_task = async move {
        while let Ok(event) = output_rx.recv().await {
            let msg = WsMessage {
                msg_type: "Output".to_string(),
                data: event.data,
            };
            if sender
                .send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
                .await
                .is_err()
            {
                break;
            }
        }
    };

    // Task to forward WebSocket input to PTY
    let input_task = async move {
        while let Some(msg) = receiver.next().await {
            if let Ok(Message::Text(text)) = msg {
                if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                    if ws_msg.msg_type == "Input" {
                        let _ = state.pty.write_input(&ws_msg.data).await;
                    }
                }
            }
        }
    };

    // Run both tasks concurrently
    tokio::select! {
        _ = output_task => {
            info!("WebSocket output task ended");
        }
        _ = input_task => {
            info!("WebSocket input task ended");
        }
    }
}
