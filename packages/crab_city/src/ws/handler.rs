//! WebSocket Handler
//!
//! Main multiplexed WebSocket connection handler.

use axum::extract::ws::{Message, WebSocket};
use futures::{sink::SinkExt, stream::StreamExt};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

use crate::config::ServerConfig;
use crate::instance_manager::InstanceManager;
use crate::metrics::ServerMetrics;
use crate::repository::ConversationRepository;

use crate::virtual_terminal::ClientType;

use super::dispatch::{
    ConnectionContext, DispatchResult, disconnect_cleanup, dispatch_client_message,
};
use super::protocol::{
    BackpressureStats, ClientMessage, DEFAULT_MAX_HISTORY_BYTES, ServerMessage, WsUser,
};
use super::state_manager::GlobalStateManager;

/// Handle a multiplexed WebSocket connection
pub async fn handle_multiplexed_ws(
    socket: WebSocket,
    instance_manager: Arc<InstanceManager>,
    state_manager: Arc<GlobalStateManager>,
    server_config: Option<Arc<ServerConfig>>,
    server_metrics: Option<Arc<ServerMetrics>>,
    ws_user: Option<WsUser>,
    repository: Option<Arc<ConversationRepository>>,
) {
    info!(
        "New multiplexed WebSocket connection (user: {})",
        ws_user
            .as_ref()
            .map(|u| u.display_name.as_str())
            .unwrap_or("anonymous")
    );

    // Track connection opened
    if let Some(ref m) = server_metrics {
        m.connection_opened();
    }

    // Get max history bytes from config or use default
    let max_history_bytes = server_config
        .as_ref()
        .map(|c| c.websocket.max_history_replay_bytes)
        .unwrap_or(DEFAULT_MAX_HISTORY_BYTES);

    // Per-connection backpressure stats
    let stats = Arc::new(BackpressureStats::new());

    // Unique ID for this connection (for presence tracking)
    let connection_id = uuid::Uuid::new_v4().to_string();

    let (mut ws_sender, mut ws_receiver) = socket.split();

    // Channel for sending messages to the WebSocket
    let (tx, mut rx) = mpsc::channel::<ServerMessage>(100);

    // Build shared connection context
    let ctx = Arc::new(ConnectionContext::new(
        connection_id.clone(),
        ws_user.clone(),
        tx.clone(),
        state_manager.clone(),
        instance_manager.clone(),
        repository,
        max_history_bytes,
        ClientType::Web,
    ));

    // Send initial instance list with states
    let instances = instance_manager.list().await;
    if tx
        .send(ServerMessage::InstanceList { instances })
        .await
        .is_err()
    {
        warn!(conn_id = %connection_id, "Failed to send initial instance list - channel closed");
    }

    // Subscribe to state broadcasts from all instances
    let mut state_rx = state_manager.subscribe();
    let tx_state = tx.clone();
    let stats_state = stats.clone();
    let state_broadcast_task = async move {
        loop {
            match state_rx.recv().await {
                Ok((instance_id, state, stale)) => {
                    stats_state.record_state_send(1); // At least 1 receiver (us)
                    if tx_state
                        .send(ServerMessage::StateChange {
                            instance_id,
                            state,
                            stale,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    stats_state.record_lag(n);
                    warn!("State broadcast lagged by {} messages", n);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    // Subscribe to instance lifecycle broadcasts (created/stopped)
    let mut lifecycle_rx = state_manager.subscribe_lifecycle();
    let tx_lifecycle = tx.clone();
    let lifecycle_task = async move {
        loop {
            match lifecycle_rx.recv().await {
                Ok(msg) => {
                    if tx_lifecycle.send(msg).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    // Task to send messages to WebSocket
    let sender_task = async move {
        while let Some(msg) = rx.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(e) => {
                    error!("Failed to serialize message: {}", e);
                    continue;
                }
            };
            if ws_sender.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    };

    // Task to handle incoming messages
    let ctx_input = ctx.clone();
    let input_task = async move {
        while let Some(msg) = ws_receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                        match dispatch_client_message(&ctx_input, client_msg).await {
                            DispatchResult::Handled => {}
                            DispatchResult::Unhandled(_) => {
                                warn!(
                                    "interconnect message received on WebSocket — use iroh transport"
                                );
                            }
                        }
                    }
                }
                Ok(Message::Close(_)) => {
                    debug!("Client closed connection");
                    break;
                }
                Err(e) => {
                    error!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }
    };

    // Run all tasks
    tokio::select! {
        _ = state_broadcast_task => debug!("State broadcast task ended"),
        _ = lifecycle_task => debug!("Lifecycle task ended"),
        _ = sender_task => debug!("Sender task ended"),
        _ = input_task => debug!("Input task ended"),
    }

    // Shared disconnect cleanup (viewports, presence, terminal locks)
    disconnect_cleanup(&ctx).await;

    // Log backpressure stats on connection close
    let snapshot = stats.snapshot();

    // Track connection closed in server metrics
    if let Some(ref m) = server_metrics {
        m.connection_closed();
        // Record dropped messages in global metrics
        if snapshot.total_lagged_count > 0 {
            for _ in 0..snapshot.total_lagged_count {
                m.message_dropped();
            }
        }
    }
    if snapshot.total_lagged_count > 0 || snapshot.output_messages_lagged > 0 {
        warn!(
            "WebSocket connection closed with backpressure issues: state_broadcasts={}, output_lagged={}, total_dropped={}",
            snapshot.state_broadcasts_sent,
            snapshot.output_messages_lagged,
            snapshot.total_lagged_count
        );
    } else {
        info!(
            "Multiplexed WebSocket connection closed (state_broadcasts={})",
            snapshot.state_broadcasts_sent
        );
    }
}
