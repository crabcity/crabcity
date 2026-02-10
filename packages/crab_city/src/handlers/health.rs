use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::AppState;
use crate::metrics;

/// Health check endpoint - returns server status
pub async fn health_handler(State(state): State<AppState>) -> impl IntoResponse {
    let instances = state.instance_manager.list().await;
    let active_instances = instances.iter().filter(|i| i.running).count() as u64;
    let metrics = state.metrics.snapshot();

    let status = if metrics.errors.pty == 0 && metrics.errors.websocket == 0 {
        "healthy"
    } else {
        "degraded"
    };

    Json(metrics::HealthStatus {
        status: status.to_string(),
        instances: metrics::InstanceHealth {
            total: instances.len() as u64,
            active: active_instances,
        },
        connections: metrics.connections.active,
        uptime_secs: metrics.uptime_secs,
    })
}

/// Metrics endpoint - returns detailed server metrics
pub async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.metrics.snapshot())
}

/// Liveness probe - returns 200 if the server is running
pub async fn health_live_handler() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "alive" }))
}

/// Readiness probe - returns 200 if the server is ready to accept requests
pub async fn health_ready_handler(State(state): State<AppState>) -> Response {
    let db_ok = state.db.pool.acquire().await.is_ok();

    if db_ok {
        Json(serde_json::json!({
            "status": "ready",
            "database": "connected"
        }))
        .into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "not_ready",
                "database": "disconnected"
            })),
        )
            .into_response()
    }
}
