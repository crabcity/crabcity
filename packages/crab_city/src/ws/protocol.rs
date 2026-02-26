//! WebSocket Protocol Types
//!
//! Message types for client-server communication over the multiplexed WebSocket.

use crab_city_auth::AccessRights;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::inference::ClaudeState;
use crate::instance_manager::ClaudeInstance;

/// User info passed from the auth layer into WebSocket connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsUser {
    pub user_id: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<AccessRights>,
}

impl WsUser {
    /// Check if this user has the given access right.
    /// Returns true if access is None (legacy/loopback — full access).
    pub fn has_access(&self, type_: &str, action: &str) -> bool {
        match &self.access {
            Some(rights) => rights.contains(type_, action),
            None => true,
        }
    }
}

/// User presence information broadcast to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresenceUser {
    pub user_id: String,
    pub display_name: String,
}

/// Statistics for tracking backpressure and message drops
#[derive(Debug, Default)]
pub struct BackpressureStats {
    /// Number of state broadcasts sent successfully
    pub state_broadcasts_sent: AtomicU64,
    /// Number of state broadcasts that had no receivers
    pub state_broadcasts_no_receivers: AtomicU64,
    /// Number of output messages sent
    pub output_messages_sent: AtomicU64,
    /// Number of output messages that lagged (receiver was slow)
    pub output_messages_lagged: AtomicU64,
    /// Total messages dropped due to lag
    pub total_lagged_count: AtomicU64,
}

impl BackpressureStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_state_send(&self, receiver_count: usize) {
        self.state_broadcasts_sent.fetch_add(1, Ordering::Relaxed);
        if receiver_count == 0 {
            self.state_broadcasts_no_receivers
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    #[allow(dead_code)]
    pub fn record_output_send(&self) {
        self.output_messages_sent.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_lag(&self, lag_count: u64) {
        self.output_messages_lagged.fetch_add(1, Ordering::Relaxed);
        self.total_lagged_count
            .fetch_add(lag_count, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> BackpressureSnapshot {
        BackpressureSnapshot {
            state_broadcasts_sent: self.state_broadcasts_sent.load(Ordering::Relaxed),
            state_broadcasts_no_receivers: self
                .state_broadcasts_no_receivers
                .load(Ordering::Relaxed),
            output_messages_sent: self.output_messages_sent.load(Ordering::Relaxed),
            output_messages_lagged: self.output_messages_lagged.load(Ordering::Relaxed),
            total_lagged_count: self.total_lagged_count.load(Ordering::Relaxed),
        }
    }
}

/// Snapshot of backpressure stats (for serialization/logging)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackpressureSnapshot {
    pub state_broadcasts_sent: u64,
    pub state_broadcasts_no_receivers: u64,
    pub output_messages_sent: u64,
    pub output_messages_lagged: u64,
    pub total_lagged_count: u64,
}

/// Messages sent FROM the client TO the server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    // === Auth handshake ===
    /// Ed25519 challenge-response: client signs the server's nonce
    ChallengeResponse {
        /// Hex-encoded 32-byte Ed25519 public key
        public_key: String,
        /// Hex-encoded signature of the nonce
        signature: String,
        /// Human-readable display name
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
    },

    /// Password-based auth bridge: server generates keypair on behalf of the user.
    /// If invite_token is present and user is new, also redeems the invite.
    PasswordAuth {
        username: String,
        password: String,
        /// Invite token for new user registration (optional for returning users)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invite_token: Option<String>,
        /// Display name for new user registration
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
    },

    /// Loopback-only auth: accepted only from 127.0.0.1/::1 connections.
    /// Clients without a keypair send this to get Owner access on localhost.
    LoopbackAuth,

    /// Reconnect with replay: client sends last seen sequence number
    /// and the connection_id of the previous session to replay from.
    Reconnect {
        last_seq: u64,
        connection_id: String,
    },

    /// Switch focus to a different instance (triggers history replay)
    /// If `since_uuid` is provided, only returns entries after that UUID.
    /// If `since_uuid` is None, returns full conversation.
    Focus {
        instance_id: String,
        /// Optional: only return conversation entries after this UUID
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since_uuid: Option<String>,
    },
    /// Request conversation sync without changing focus
    /// Useful for catching up when tab becomes visible
    ConversationSync {
        /// Only return entries after this UUID (if None, returns full)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since_uuid: Option<String>,
    },
    /// Send input to a specific instance
    /// instance_id is required to ensure correct routing regardless of focus state
    Input {
        instance_id: String,
        data: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task_id: Option<i64>,
    },
    /// Resize a specific instance's terminal
    Resize {
        instance_id: String,
        rows: u16,
        cols: u16,
    },
    /// Select a session when ambiguous
    SessionSelect { session_id: String },
    /// Lobby relay message: broadcast to all connected clients on a named channel.
    /// The server treats the payload as opaque — all game logic lives client-side.
    Lobby {
        channel: String,
        payload: serde_json::Value,
    },
    /// Request terminal lock for an instance
    TerminalLockRequest { instance_id: String },
    /// Release terminal lock for an instance
    TerminalLockRelease { instance_id: String },
    /// Send a chat message
    ChatSend {
        scope: String,
        content: String,
        uuid: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topic: Option<String>,
    },
    /// Terminal panel became visible — include this client in dimension negotiation
    TerminalVisible {
        instance_id: String,
        rows: u16,
        cols: u16,
    },
    /// Terminal panel was hidden — exclude this client from dimension negotiation
    TerminalHidden { instance_id: String },
    /// Request paginated chat history
    ChatHistory {
        scope: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_id: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topic: Option<String>,
    },
    /// Forward a chat message to another scope
    ChatForward {
        message_id: i64,
        target_scope: String,
    },
    /// Request list of topics for a scope
    ChatTopics { scope: String },

    // === Interconnect (membership + invites + event log) ===
    /// Create an invite token
    CreateInvite {
        capability: String,
        max_uses: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_in_secs: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// Redeem an invite token (public_key is hex-encoded 32-byte ed25519 key)
    RedeemInvite {
        token: String,
        display_name: String,
        /// Hex-encoded 32-byte Ed25519 public key to bind to this invite
        public_key: String,
    },
    /// Revoke an invite by nonce (hex-encoded)
    RevokeInvite {
        nonce: String,
        #[serde(default)]
        suspend_derived: bool,
    },
    /// List active invites
    ListInvites,
    /// List all members
    ListMembers,
    /// Update a member's capability or display name
    UpdateMember {
        public_key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capability: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
    },
    /// Suspend a member
    SuspendMember { public_key: String },
    /// Reinstate a suspended member
    ReinstateMember { public_key: String },
    /// Remove a member permanently
    RemoveMember { public_key: String },
    /// Query the event log
    QueryEvents {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_type_prefix: Option<String>,
        limit: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_id: Option<i64>,
    },
    /// Verify hash chain integrity over a range
    VerifyEvents { from_id: i64, to_id: i64 },
    /// Get an event with its inclusion proof
    GetEventProof { event_id: i64 },

    // === Context switching ===
    /// Switch viewing context to a remote Crab City (or back to local).
    /// When host_node_id is None, switches back to local context.
    /// When host_node_id is provided (hex-encoded 32 bytes), switches to that remote.
    SwitchContext {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_node_id: Option<String>,
    },
}

/// Messages sent FROM the server TO the client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    // === Auth handshake ===
    /// Server sends a challenge nonce for the client to sign
    Challenge {
        /// Hex-encoded 32-byte random nonce
        nonce: String,
    },
    /// Auth succeeded — connection is authenticated
    Authenticated {
        /// The user's fingerprint (crab_XXXXXXXX)
        fingerprint: String,
        /// Capability level
        capability: String,
    },
    /// Auth required — no grant found for this identity
    AuthRequired {
        /// Hint for recovery (e.g. "redeem_invite")
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recovery: Option<String>,
    },

    /// Connection established — sent as the first message to inform the client
    /// of its connection_id (needed for reconnect replay).
    ConnectionEstablished { connection_id: String },

    // === High-resolution messages (focused instance only) ===
    // All content messages include instance_id for proper client-side routing
    /// Terminal output from focused instance
    Output { instance_id: String, data: String },
    /// Terminal history replay on focus switch
    OutputHistory { instance_id: String, data: String },
    /// Full conversation on focus switch
    ConversationFull {
        instance_id: String,
        turns: Vec<serde_json::Value>,
    },
    /// Incremental conversation update
    ConversationUpdate {
        instance_id: String,
        turns: Vec<serde_json::Value>,
    },
    /// Multiple candidate sessions found
    SessionAmbiguous {
        instance_id: String,
        candidates: Vec<SessionCandidate>,
    },

    // === Low-resolution messages (all instances) ===
    /// State change for any instance
    StateChange {
        instance_id: String,
        state: ClaudeState,
        /// True if terminal output is stale (no recent activity)
        /// Indicates we're less confident in the state accuracy
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        stale: bool,
    },
    /// New instance was created
    InstanceCreated { instance: ClaudeInstance },
    /// Instance was stopped/removed
    InstanceStopped { instance_id: String },
    /// Instance custom name was changed
    InstanceRenamed {
        instance_id: String,
        custom_name: Option<String>,
    },
    /// Initial list of all instances with their states
    InstanceList { instances: Vec<ClaudeInstance> },

    // === Control messages ===
    /// Acknowledge focus switch (sent before history replay)
    /// Includes current claude_state to prevent race conditions on focus switch
    FocusAck {
        instance_id: String,
        /// Current Claude state at time of focus (avoids reading stale store)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claude_state: Option<ClaudeState>,
    },
    /// Error message
    Error {
        instance_id: Option<String>,
        message: String,
    },

    // === Backpressure notifications ===
    /// Terminal output was dropped due to slow client
    /// UI can show an indicator that some output was missed
    OutputLagged {
        instance_id: String,
        dropped_count: u64,
    },

    // === Multi-user presence ===
    /// Presence update for an instance (who is viewing it)
    PresenceUpdate {
        instance_id: String,
        users: Vec<PresenceUser>,
    },

    // === Lobby relay ===
    /// Lobby relay broadcast: forwarded from another client.
    /// sender_id is the server-assigned connection UUID (not user identity).
    LobbyBroadcast {
        sender_id: String,
        channel: String,
        payload: serde_json::Value,
    },

    // === Chat ===
    /// A single chat message broadcast
    ChatMessage {
        id: i64,
        uuid: String,
        scope: String,
        user_id: String,
        display_name: String,
        content: String,
        created_at: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        forwarded_from: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        topic: Option<String>,
    },
    /// Chat history response
    ChatHistoryResponse {
        scope: String,
        messages: Vec<serde_json::Value>,
        has_more: bool,
    },
    /// Chat topics list response
    ChatTopicsResponse {
        scope: String,
        topics: Vec<crate::models::ChatTopicSummary>,
    },

    // === Tasks ===
    /// A task was created or updated — full snapshot for idempotent merge
    TaskUpdate { task: serde_json::Value },
    /// A task was deleted
    TaskDeleted { task_id: i64 },

    // === Interconnect (membership + invites + event log) ===
    /// Invite was successfully created
    InviteCreated {
        token: String,
        nonce: String,
        capability: String,
        max_uses: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        expires_at: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// Invite was redeemed by a new member
    InviteRedeemed {
        public_key: String,
        fingerprint: String,
        display_name: String,
        capability: String,
    },
    /// Invite was revoked
    InviteRevoked { nonce: String },
    /// List of active invites
    InviteList { invites: Vec<serde_json::Value> },
    /// List of members
    MembersList { members: Vec<serde_json::Value> },
    /// Broadcast: new member joined
    MemberJoined { member: serde_json::Value },
    /// Broadcast: member details updated
    MemberUpdated { member: serde_json::Value },
    /// Broadcast: member suspended
    MemberSuspended {
        public_key: String,
        fingerprint: String,
        display_name: String,
    },
    /// Broadcast: member reinstated
    MemberReinstated {
        public_key: String,
        fingerprint: String,
        display_name: String,
    },
    /// Broadcast: member removed
    MemberRemoved {
        public_key: String,
        fingerprint: String,
        display_name: String,
    },
    /// Event log query response
    EventsResponse { events: Vec<serde_json::Value> },
    /// Event chain verification result
    EventVerification {
        valid: bool,
        events_checked: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Event with inclusion proof
    EventProofResponse {
        event: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        nearest_checkpoint: Option<serde_json::Value>,
    },

    // === Terminal lock ===
    /// Terminal lock state update (idempotent snapshot — covers acquire/release/expire/steal)
    TerminalLockUpdate {
        instance_id: String,
        /// Current lock holder, or None if unclaimed
        #[serde(skip_serializing_if = "Option::is_none")]
        holder: Option<PresenceUser>,
        /// ISO 8601 timestamp of last terminal activity by holder
        #[serde(skip_serializing_if = "Option::is_none")]
        last_activity: Option<String>,
        /// Seconds until lock expires (for UI countdown)
        #[serde(skip_serializing_if = "Option::is_none")]
        expires_in_secs: Option<u64>,
    },

    // === Context switching ===
    /// Confirmation that viewing context was switched.
    /// Sent after a successful SwitchContext client message.
    ContextSwitched {
        /// None = local, Some(hex) = remote host_node_id
        #[serde(skip_serializing_if = "Option::is_none")]
        host_node_id: Option<String>,
        /// Human-readable name (e.g. "Bob's Workshop" or "local")
        context_name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCandidate {
    pub session_id: String,
    pub started_at: Option<String>,
    pub message_count: usize,
    pub preview: Option<String>,
}

/// Default max history bytes if no config provided
pub const DEFAULT_MAX_HISTORY_BYTES: usize = 64 * 1024; // 64KB

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_message_focus_without_since_uuid() {
        let json = r#"{"type":"Focus","instance_id":"abc123"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();

        match msg {
            ClientMessage::Focus {
                instance_id,
                since_uuid,
            } => {
                assert_eq!(instance_id, "abc123");
                assert!(since_uuid.is_none());
            }
            _ => panic!("Expected Focus message"),
        }
    }

    #[test]
    fn test_client_message_focus_with_since_uuid() {
        let json = r#"{"type":"Focus","instance_id":"abc123","since_uuid":"uuid-42"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();

        match msg {
            ClientMessage::Focus {
                instance_id,
                since_uuid,
            } => {
                assert_eq!(instance_id, "abc123");
                assert_eq!(since_uuid, Some("uuid-42".to_string()));
            }
            _ => panic!("Expected Focus message"),
        }
    }

    #[test]
    fn test_client_message_conversation_sync() {
        let json = r#"{"type":"ConversationSync","since_uuid":"uuid-99"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();

        match msg {
            ClientMessage::ConversationSync { since_uuid } => {
                assert_eq!(since_uuid, Some("uuid-99".to_string()));
            }
            _ => panic!("Expected ConversationSync message"),
        }
    }

    #[test]
    fn test_client_message_conversation_sync_without_uuid() {
        let json = r#"{"type":"ConversationSync"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();

        match msg {
            ClientMessage::ConversationSync { since_uuid } => {
                assert!(since_uuid.is_none());
            }
            _ => panic!("Expected ConversationSync message"),
        }
    }

    #[test]
    fn test_client_message_input() {
        let json = r#"{"type":"Input","instance_id":"inst-123","data":"hello world\n"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();

        match msg {
            ClientMessage::Input {
                instance_id,
                data,
                task_id,
            } => {
                assert_eq!(instance_id, "inst-123");
                assert_eq!(data, "hello world\n");
                assert!(task_id.is_none());
            }
            _ => panic!("Expected Input message"),
        }
    }

    #[test]
    fn test_client_message_input_with_task_id() {
        let json =
            r#"{"type":"Input","instance_id":"inst-123","data":"fix the bug\n","task_id":42}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();

        match msg {
            ClientMessage::Input {
                instance_id,
                data,
                task_id,
            } => {
                assert_eq!(instance_id, "inst-123");
                assert_eq!(data, "fix the bug\n");
                assert_eq!(task_id, Some(42));
            }
            _ => panic!("Expected Input message"),
        }
    }

    #[test]
    fn test_client_message_resize() {
        let json = r#"{"type":"Resize","instance_id":"inst-456","rows":40,"cols":120}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();

        match msg {
            ClientMessage::Resize {
                instance_id,
                rows,
                cols,
            } => {
                assert_eq!(instance_id, "inst-456");
                assert_eq!(rows, 40);
                assert_eq!(cols, 120);
            }
            _ => panic!("Expected Resize message"),
        }
    }

    #[test]
    fn test_client_message_session_select() {
        let json = r#"{"type":"SessionSelect","session_id":"abc-123"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();

        match msg {
            ClientMessage::SessionSelect { session_id } => {
                assert_eq!(session_id, "abc-123");
            }
            _ => panic!("Expected SessionSelect message"),
        }
    }

    #[test]
    fn test_server_message_output_serialization() {
        let msg = ServerMessage::Output {
            instance_id: "inst-1".to_string(),
            data: "Hello from Claude!".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("Output"));
        assert!(json.contains("inst-1"));
        assert!(json.contains("Hello from Claude!"));
    }

    #[test]
    fn test_server_message_state_change_serialization() {
        let msg = ServerMessage::StateChange {
            instance_id: "inst-1".to_string(),
            state: ClaudeState::Thinking,
            stale: false,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("StateChange"));
        assert!(json.contains("inst-1"));
        assert!(json.contains("Thinking"));
    }

    #[test]
    fn test_server_message_state_change_with_stale_serialization() {
        let msg = ServerMessage::StateChange {
            instance_id: "inst-1".to_string(),
            state: ClaudeState::Responding,
            stale: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("stale"));
    }

    #[test]
    fn test_server_message_output_lagged_serialization() {
        let msg = ServerMessage::OutputLagged {
            instance_id: "inst-1".to_string(),
            dropped_count: 42,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("OutputLagged"));
        assert!(json.contains("42"));
    }

    #[test]
    fn test_server_message_error_serialization() {
        let msg = ServerMessage::Error {
            instance_id: Some("inst-1".to_string()),
            message: "Something went wrong".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("Error"));
        assert!(json.contains("Something went wrong"));
    }

    #[test]
    fn test_server_message_focus_ack_serialization() {
        let msg = ServerMessage::FocusAck {
            instance_id: "inst-123".to_string(),
            claude_state: Some(ClaudeState::Idle),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("FocusAck"));
        assert!(json.contains("inst-123"));
        assert!(json.contains("Idle"));
    }

    #[test]
    fn test_server_message_focus_ack_without_state() {
        let msg = ServerMessage::FocusAck {
            instance_id: "inst-456".to_string(),
            claude_state: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("FocusAck"));
        assert!(json.contains("inst-456"));
        // claude_state should be skipped when None
        assert!(!json.contains("claude_state"));
    }

    #[test]
    fn test_backpressure_stats_initial_state() {
        let stats = BackpressureStats::new();
        let snapshot = stats.snapshot();

        assert_eq!(snapshot.state_broadcasts_sent, 0);
        assert_eq!(snapshot.output_messages_sent, 0);
        assert_eq!(snapshot.output_messages_lagged, 0);
        assert_eq!(snapshot.total_lagged_count, 0);
    }

    #[test]
    fn test_backpressure_stats_tracking() {
        let stats = BackpressureStats::new();

        stats.record_state_send(1);
        stats.record_state_send(2);
        stats.record_output_send();
        stats.record_lag(5);
        stats.record_lag(10);

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.state_broadcasts_sent, 2);
        assert_eq!(snapshot.output_messages_sent, 1);
        assert_eq!(snapshot.output_messages_lagged, 2);
        assert_eq!(snapshot.total_lagged_count, 15);
    }

    #[test]
    fn test_backpressure_stats_no_receivers() {
        let stats = BackpressureStats::new();

        stats.record_state_send(0); // No receivers
        stats.record_state_send(1); // Has receivers

        let snapshot = stats.snapshot();

        assert_eq!(snapshot.state_broadcasts_sent, 2);
        assert_eq!(snapshot.state_broadcasts_no_receivers, 1);
    }

    #[test]
    fn test_client_message_roundtrip_focus() {
        let original = ClientMessage::Focus {
            instance_id: "test-instance".to_string(),
            since_uuid: Some("uuid-abc".to_string()),
        };

        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();

        match decoded {
            ClientMessage::Focus {
                instance_id,
                since_uuid,
            } => {
                assert_eq!(instance_id, "test-instance");
                assert_eq!(since_uuid, Some("uuid-abc".to_string()));
            }
            _ => panic!("Round-trip failed"),
        }
    }

    #[test]
    fn test_server_message_roundtrip_tool_executing() {
        let original = ServerMessage::StateChange {
            instance_id: "inst".to_string(),
            state: ClaudeState::ToolExecuting {
                tool: "Bash".to_string(),
            },
            stale: false,
        };

        let json = serde_json::to_string(&original).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();

        match decoded {
            ServerMessage::StateChange {
                instance_id,
                state,
                stale,
            } => {
                assert_eq!(instance_id, "inst");
                assert!(!stale);
                match state {
                    ClaudeState::ToolExecuting { tool } => {
                        assert_eq!(tool, "Bash");
                    }
                    _ => panic!("Wrong state type"),
                }
            }
            _ => panic!("Round-trip failed"),
        }
    }

    #[test]
    fn test_client_message_invalid_json() {
        let invalid_json = r#"{"type":"Focus"}"#; // Missing required instance_id
        let result: Result<ClientMessage, _> = serde_json::from_str(invalid_json);
        assert!(result.is_err());
    }

    #[test]
    fn test_client_message_unknown_type() {
        let json = r#"{"type":"UnknownCommand","data":"test"}"#;
        let result: Result<ClientMessage, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_client_message_empty_instance_id() {
        let json = r#"{"type":"Focus","instance_id":""}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Focus { instance_id, .. } => {
                assert_eq!(instance_id, "");
            }
            _ => panic!("Expected Focus message"),
        }
    }

    #[test]
    fn test_client_message_input_with_special_chars() {
        let json = r#"{"type":"Input","instance_id":"inst-1","data":"hello\nworld\t\u0000"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Input {
                instance_id, data, ..
            } => {
                assert_eq!(instance_id, "inst-1");
                assert!(data.contains('\n'));
                assert!(data.contains('\t'));
                assert!(data.contains('\0'));
            }
            _ => panic!("Expected Input message"),
        }
    }

    #[test]
    fn test_client_message_resize_edge_values() {
        let json = r#"{"type":"Resize","instance_id":"inst-1","rows":1,"cols":1}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Resize {
                instance_id,
                rows,
                cols,
            } => {
                assert_eq!(instance_id, "inst-1");
                assert_eq!(rows, 1);
                assert_eq!(cols, 1);
            }
            _ => panic!("Expected Resize message"),
        }

        let json = r#"{"type":"Resize","instance_id":"inst-2","rows":65535,"cols":65535}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Resize {
                instance_id,
                rows,
                cols,
            } => {
                assert_eq!(instance_id, "inst-2");
                assert_eq!(rows, 65535);
                assert_eq!(cols, 65535);
            }
            _ => panic!("Expected Resize message"),
        }
    }

    #[test]
    fn test_backpressure_snapshot_serialization() {
        let stats = BackpressureStats::new();
        stats.record_state_send(1);
        stats.record_output_send();
        stats.record_lag(10);

        let snapshot = stats.snapshot();
        let json = serde_json::to_string(&snapshot).unwrap();
        let decoded: BackpressureSnapshot = serde_json::from_str(&json).unwrap();

        assert_eq!(
            decoded.state_broadcasts_sent,
            snapshot.state_broadcasts_sent
        );
        assert_eq!(decoded.output_messages_sent, snapshot.output_messages_sent);
        assert_eq!(decoded.total_lagged_count, snapshot.total_lagged_count);
    }

    #[test]
    fn test_backpressure_snapshot_json_structure() {
        let stats = BackpressureStats::new();
        stats.record_state_send(5);
        stats.record_output_send();
        stats.record_output_send();

        let snapshot = stats.snapshot();
        let json = serde_json::to_string(&snapshot).unwrap();

        assert!(json.contains("state_broadcasts_sent"));
        assert!(json.contains("output_messages_sent"));
        assert!(json.contains("total_lagged_count"));
    }

    #[test]
    fn test_server_message_conversation_full_serialization() {
        let turns: Vec<serde_json::Value> = vec![
            serde_json::json!({"role": "user", "content": "Hello"}),
            serde_json::json!({"role": "assistant", "content": "Hi there!"}),
        ];

        let msg = ServerMessage::ConversationFull {
            instance_id: "inst-1".to_string(),
            turns,
        };
        let json = serde_json::to_string(&msg).unwrap();

        assert!(json.contains("ConversationFull"));
        assert!(json.contains("inst-1"));
        assert!(json.contains("Hello"));
        assert!(json.contains("Hi there!"));
    }

    #[test]
    fn test_server_message_conversation_update_serialization() {
        let turns: Vec<serde_json::Value> =
            vec![serde_json::json!({"role": "assistant", "content": "New response"})];

        let msg = ServerMessage::ConversationUpdate {
            instance_id: "inst-1".to_string(),
            turns,
        };
        let json = serde_json::to_string(&msg).unwrap();

        assert!(json.contains("ConversationUpdate"));
        assert!(json.contains("inst-1"));
        assert!(json.contains("New response"));
    }

    #[test]
    fn test_server_message_output_history_serialization() {
        let msg = ServerMessage::OutputHistory {
            instance_id: "inst-1".to_string(),
            data: "terminal output data".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();

        assert!(json.contains("OutputHistory"));
        assert!(json.contains("inst-1"));
        assert!(json.contains("terminal output data"));
    }

    #[test]
    fn test_backpressure_stats_concurrent_updates() {
        use std::sync::Arc;
        use std::thread;

        let stats = Arc::new(BackpressureStats::new());
        let mut handles = vec![];

        for _ in 0..10 {
            let stats_clone = stats.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    stats_clone.record_state_send(1);
                    stats_clone.record_output_send();
                    stats_clone.record_lag(1);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.state_broadcasts_sent, 1000);
        assert_eq!(snapshot.output_messages_sent, 1000);
        assert_eq!(snapshot.total_lagged_count, 1000);
    }

    #[test]
    fn test_client_message_lobby_roundtrip() {
        let original = ClientMessage::Lobby {
            channel: "snake".to_string(),
            payload: serde_json::json!({"x": 10, "y": 20}),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();

        match decoded {
            ClientMessage::Lobby { channel, payload } => {
                assert_eq!(channel, "snake");
                assert_eq!(payload, serde_json::json!({"x": 10, "y": 20}));
            }
            _ => panic!("Expected Lobby message"),
        }
    }

    #[test]
    fn test_server_message_lobby_broadcast_roundtrip() {
        let original = ServerMessage::LobbyBroadcast {
            sender_id: "conn-abc-123".to_string(),
            channel: "snake".to_string(),
            payload: serde_json::json!({"direction": "up"}),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();

        match decoded {
            ServerMessage::LobbyBroadcast {
                sender_id,
                channel,
                payload,
            } => {
                assert_eq!(sender_id, "conn-abc-123");
                assert_eq!(channel, "snake");
                assert_eq!(payload, serde_json::json!({"direction": "up"}));
            }
            _ => panic!("Expected LobbyBroadcast message"),
        }
    }

    #[test]
    fn test_client_message_terminal_visible_roundtrip() {
        let original = ClientMessage::TerminalVisible {
            instance_id: "inst-1".to_string(),
            rows: 40,
            cols: 120,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();

        match decoded {
            ClientMessage::TerminalVisible {
                instance_id,
                rows,
                cols,
            } => {
                assert_eq!(instance_id, "inst-1");
                assert_eq!(rows, 40);
                assert_eq!(cols, 120);
            }
            _ => panic!("Expected TerminalVisible message"),
        }
    }

    #[test]
    fn test_client_message_terminal_hidden_roundtrip() {
        let original = ClientMessage::TerminalHidden {
            instance_id: "inst-1".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();

        match decoded {
            ClientMessage::TerminalHidden { instance_id } => {
                assert_eq!(instance_id, "inst-1");
            }
            _ => panic!("Expected TerminalHidden message"),
        }
    }

    #[test]
    fn test_default_max_history_bytes_constant() {
        assert_eq!(DEFAULT_MAX_HISTORY_BYTES, 64 * 1024);
    }

    #[test]
    fn test_claude_state_all_variants_serialize() {
        let states: Vec<ClaudeState> = vec![
            ClaudeState::Idle,
            ClaudeState::Thinking,
            ClaudeState::Responding,
            ClaudeState::ToolExecuting {
                tool: "Read".to_string(),
            },
            ClaudeState::WaitingForInput {
                prompt: Some("Continue?".to_string()),
            },
            ClaudeState::WaitingForInput { prompt: None },
        ];

        for state in states {
            let msg = ServerMessage::StateChange {
                instance_id: "test".to_string(),
                state: state.clone(),
                stale: false,
            };
            let json = serde_json::to_string(&msg).unwrap();
            let decoded: ServerMessage = serde_json::from_str(&json).unwrap();

            match decoded {
                ServerMessage::StateChange {
                    state: decoded_state,
                    ..
                } => {
                    let original_json = serde_json::to_string(&state).unwrap();
                    let decoded_json = serde_json::to_string(&decoded_state).unwrap();
                    assert_eq!(original_json, decoded_json);
                }
                _ => panic!("Wrong message type"),
            }
        }
    }

    #[test]
    fn test_client_message_terminal_visible_from_raw_json() {
        let json = r#"{"type":"TerminalVisible","instance_id":"inst-1","rows":40,"cols":120}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();

        match msg {
            ClientMessage::TerminalVisible {
                instance_id,
                rows,
                cols,
            } => {
                assert_eq!(instance_id, "inst-1");
                assert_eq!(rows, 40);
                assert_eq!(cols, 120);
            }
            _ => panic!("Expected TerminalVisible message"),
        }
    }

    #[test]
    fn test_client_message_terminal_hidden_from_raw_json() {
        let json = r#"{"type":"TerminalHidden","instance_id":"inst-1"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();

        match msg {
            ClientMessage::TerminalHidden { instance_id } => {
                assert_eq!(instance_id, "inst-1");
            }
            _ => panic!("Expected TerminalHidden message"),
        }
    }

    #[test]
    fn test_client_message_chat_send_roundtrip() {
        let original = ClientMessage::ChatSend {
            scope: "global".to_string(),
            content: "Hello chat!".to_string(),
            uuid: "msg-uuid-1".to_string(),
            topic: Some("general".to_string()),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::ChatSend {
                scope,
                content,
                uuid,
                topic,
            } => {
                assert_eq!(scope, "global");
                assert_eq!(content, "Hello chat!");
                assert_eq!(uuid, "msg-uuid-1");
                assert_eq!(topic, Some("general".to_string()));
            }
            _ => panic!("Expected ChatSend"),
        }
    }

    #[test]
    fn test_client_message_chat_send_no_topic() {
        let json = r#"{"type":"ChatSend","scope":"inst-1","content":"hi","uuid":"u1"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::ChatSend { topic, .. } => assert!(topic.is_none()),
            _ => panic!("Expected ChatSend"),
        }
    }

    #[test]
    fn test_client_message_chat_history_roundtrip() {
        let original = ClientMessage::ChatHistory {
            scope: "inst-1".to_string(),
            before_id: Some(42),
            limit: Some(20),
            topic: None,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::ChatHistory {
                scope,
                before_id,
                limit,
                topic,
            } => {
                assert_eq!(scope, "inst-1");
                assert_eq!(before_id, Some(42));
                assert_eq!(limit, Some(20));
                assert!(topic.is_none());
            }
            _ => panic!("Expected ChatHistory"),
        }
    }

    #[test]
    fn test_client_message_chat_forward_roundtrip() {
        let original = ClientMessage::ChatForward {
            message_id: 99,
            target_scope: "inst-2".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::ChatForward {
                message_id,
                target_scope,
            } => {
                assert_eq!(message_id, 99);
                assert_eq!(target_scope, "inst-2");
            }
            _ => panic!("Expected ChatForward"),
        }
    }

    #[test]
    fn test_client_message_chat_topics_roundtrip() {
        let original = ClientMessage::ChatTopics {
            scope: "global".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::ChatTopics { scope } => assert_eq!(scope, "global"),
            _ => panic!("Expected ChatTopics"),
        }
    }

    #[test]
    fn test_client_message_terminal_lock_request() {
        let json = r#"{"type":"TerminalLockRequest","instance_id":"inst-1"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::TerminalLockRequest { instance_id } => {
                assert_eq!(instance_id, "inst-1");
            }
            _ => panic!("Expected TerminalLockRequest"),
        }
    }

    #[test]
    fn test_client_message_terminal_lock_release() {
        let json = r#"{"type":"TerminalLockRelease","instance_id":"inst-1"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::TerminalLockRelease { instance_id } => {
                assert_eq!(instance_id, "inst-1");
            }
            _ => panic!("Expected TerminalLockRelease"),
        }
    }

    #[test]
    fn test_server_message_presence_update_serialization() {
        let msg = ServerMessage::PresenceUpdate {
            instance_id: "inst-1".to_string(),
            users: vec![
                PresenceUser {
                    user_id: "u-1".to_string(),
                    display_name: "Alice".to_string(),
                },
                PresenceUser {
                    user_id: "u-2".to_string(),
                    display_name: "Bob".to_string(),
                },
            ],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("PresenceUpdate"));
        assert!(json.contains("Alice"));
        assert!(json.contains("Bob"));

        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::PresenceUpdate { users, .. } => assert_eq!(users.len(), 2),
            _ => panic!("Expected PresenceUpdate"),
        }
    }

    #[test]
    fn test_server_message_terminal_lock_update_with_holder() {
        let msg = ServerMessage::TerminalLockUpdate {
            instance_id: "inst-1".to_string(),
            holder: Some(PresenceUser {
                user_id: "u-1".to_string(),
                display_name: "Alice".to_string(),
            }),
            last_activity: Some("2025-01-01T00:00:00Z".to_string()),
            expires_in_secs: Some(120),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("TerminalLockUpdate"));
        assert!(json.contains("Alice"));
        assert!(json.contains("120"));

        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::TerminalLockUpdate {
                holder,
                expires_in_secs,
                ..
            } => {
                assert!(holder.is_some());
                assert_eq!(expires_in_secs, Some(120));
            }
            _ => panic!("Expected TerminalLockUpdate"),
        }
    }

    #[test]
    fn test_server_message_terminal_lock_update_no_holder() {
        let msg = ServerMessage::TerminalLockUpdate {
            instance_id: "inst-1".to_string(),
            holder: None,
            last_activity: None,
            expires_in_secs: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("holder"));
        assert!(!json.contains("last_activity"));
        assert!(!json.contains("expires_in_secs"));
    }

    #[test]
    fn test_server_message_chat_message_serialization() {
        let msg = ServerMessage::ChatMessage {
            id: 42,
            uuid: "uuid-1".to_string(),
            scope: "global".to_string(),
            user_id: "u-1".to_string(),
            display_name: "Alice".to_string(),
            content: "Hello!".to_string(),
            created_at: 1700000000,
            forwarded_from: Some("inst-1".to_string()),
            topic: Some("general".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("ChatMessage"));
        assert!(json.contains("Hello!"));
        assert!(json.contains("forwarded_from"));

        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::ChatMessage {
                id,
                content,
                forwarded_from,
                topic,
                ..
            } => {
                assert_eq!(id, 42);
                assert_eq!(content, "Hello!");
                assert_eq!(forwarded_from, Some("inst-1".to_string()));
                assert_eq!(topic, Some("general".to_string()));
            }
            _ => panic!("Expected ChatMessage"),
        }
    }

    #[test]
    fn test_server_message_chat_message_no_optional_fields() {
        let msg = ServerMessage::ChatMessage {
            id: 1,
            uuid: "u".to_string(),
            scope: "s".to_string(),
            user_id: "uid".to_string(),
            display_name: "dn".to_string(),
            content: "c".to_string(),
            created_at: 0,
            forwarded_from: None,
            topic: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("forwarded_from"));
        assert!(!json.contains("topic"));
    }

    #[test]
    fn test_server_message_chat_history_response() {
        let msg = ServerMessage::ChatHistoryResponse {
            scope: "global".to_string(),
            messages: vec![serde_json::json!({"content": "hi"})],
            has_more: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::ChatHistoryResponse {
                scope,
                messages,
                has_more,
            } => {
                assert_eq!(scope, "global");
                assert_eq!(messages.len(), 1);
                assert!(has_more);
            }
            _ => panic!("Expected ChatHistoryResponse"),
        }
    }

    #[test]
    fn test_server_message_session_ambiguous() {
        let msg = ServerMessage::SessionAmbiguous {
            instance_id: "inst-1".to_string(),
            candidates: vec![SessionCandidate {
                session_id: "sess-1".to_string(),
                started_at: Some("2025-01-01T00:00:00Z".to_string()),
                message_count: 10,
                preview: Some("Hello...".to_string()),
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::SessionAmbiguous { candidates, .. } => {
                assert_eq!(candidates.len(), 1);
                assert_eq!(candidates[0].session_id, "sess-1");
                assert_eq!(candidates[0].message_count, 10);
            }
            _ => panic!("Expected SessionAmbiguous"),
        }
    }

    #[test]
    fn test_server_message_instance_created() {
        let msg = ServerMessage::InstanceCreated {
            instance: ClaudeInstance {
                id: "inst-1".to_string(),
                name: "test".to_string(),
                custom_name: None,
                wrapper_port: 0,
                working_dir: "/tmp".to_string(),
                command: "claude".to_string(),
                running: true,
                created_at: "2025-01-01T00:00:00Z".to_string(),
                session_id: None,
                claude_state: None,
            },
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("InstanceCreated"));
        assert!(json.contains("inst-1"));
    }

    #[test]
    fn test_server_message_instance_stopped() {
        let msg = ServerMessage::InstanceStopped {
            instance_id: "inst-1".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::InstanceStopped { instance_id } => {
                assert_eq!(instance_id, "inst-1");
            }
            _ => panic!("Expected InstanceStopped"),
        }
    }

    #[test]
    fn test_server_message_instance_renamed() {
        let msg = ServerMessage::InstanceRenamed {
            instance_id: "inst-1".to_string(),
            custom_name: Some("My Crab".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::InstanceRenamed {
                instance_id,
                custom_name,
            } => {
                assert_eq!(instance_id, "inst-1");
                assert_eq!(custom_name, Some("My Crab".to_string()));
            }
            _ => panic!("Expected InstanceRenamed"),
        }
    }

    #[test]
    fn test_session_candidate_serde() {
        let candidate = SessionCandidate {
            session_id: "sess-1".to_string(),
            started_at: Some("2025-01-01T00:00:00Z".to_string()),
            message_count: 42,
            preview: Some("Hello world...".to_string()),
        };
        let json = serde_json::to_value(&candidate).unwrap();
        assert_eq!(json["session_id"], "sess-1");
        assert_eq!(json["message_count"], 42);
        assert_eq!(json["preview"], "Hello world...");

        let rt: SessionCandidate = serde_json::from_value(json).unwrap();
        assert_eq!(rt.session_id, "sess-1");
        assert_eq!(rt.message_count, 42);
    }

    #[test]
    fn test_session_candidate_none_fields() {
        let candidate = SessionCandidate {
            session_id: "s".to_string(),
            started_at: None,
            message_count: 0,
            preview: None,
        };
        let json = serde_json::to_value(&candidate).unwrap();
        assert!(json["started_at"].is_null());
        assert!(json["preview"].is_null());
    }

    #[test]
    fn test_ws_user_serde() {
        let user = WsUser {
            user_id: "u-1".to_string(),
            display_name: "Alice".to_string(),
            access: None,
        };
        let json = serde_json::to_value(&user).unwrap();
        assert_eq!(json["user_id"], "u-1");
        assert_eq!(json["display_name"], "Alice");
        // access: None should be skipped
        assert!(json.get("access").is_none());
        let rt: WsUser = serde_json::from_value(json).unwrap();
        assert_eq!(rt.user_id, "u-1");
        assert!(rt.access.is_none());
    }

    #[test]
    fn test_ws_user_has_access_with_none() {
        let user = WsUser {
            user_id: "u-1".to_string(),
            display_name: "Alice".to_string(),
            access: None,
        };
        // None means full access (legacy/loopback)
        assert!(user.has_access("terminals", "input"));
        assert!(user.has_access("chat", "send"));
    }

    #[test]
    fn test_ws_user_has_access_with_rights() {
        use crab_city_auth::Capability;
        let user = WsUser {
            user_id: "u-1".to_string(),
            display_name: "Alice".to_string(),
            access: Some(Capability::View.access_rights()),
        };
        // View can read content and terminals
        assert!(user.has_access("content", "read"));
        assert!(user.has_access("terminals", "read"));
        // View cannot input or send chat
        assert!(!user.has_access("terminals", "input"));
        assert!(!user.has_access("chat", "send"));
    }

    #[test]
    fn test_presence_user_serde() {
        let user = PresenceUser {
            user_id: "u-2".to_string(),
            display_name: "Bob".to_string(),
        };
        let json = serde_json::to_value(&user).unwrap();
        assert_eq!(json["user_id"], "u-2");
        let rt: PresenceUser = serde_json::from_value(json).unwrap();
        assert_eq!(rt.display_name, "Bob");
    }

    #[test]
    fn test_claude_state_tool_names_preserved() {
        let tools = vec![
            "Bash", "Read", "Write", "Edit", "Glob", "Grep", "WebFetch", "Task",
        ];

        for tool in tools {
            let state = ClaudeState::ToolExecuting {
                tool: tool.to_string(),
            };
            let json = serde_json::to_string(&state).unwrap();
            let decoded: ClaudeState = serde_json::from_str(&json).unwrap();

            match decoded {
                ClaudeState::ToolExecuting { tool: decoded_tool } => {
                    assert_eq!(decoded_tool, tool);
                }
                _ => panic!("Wrong state type"),
            }
        }
    }

    #[test]
    fn test_server_message_task_update_serialization() {
        let task_value = serde_json::json!({
            "id": 42,
            "title": "Fix the bug",
            "status": "pending",
            "tags": [{"id": 1, "name": "urgent", "color": null}],
        });
        let msg = ServerMessage::TaskUpdate {
            task: task_value.clone(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("TaskUpdate"));
        assert!(json.contains("Fix the bug"));

        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::TaskUpdate { task } => {
                assert_eq!(task["id"], 42);
                assert_eq!(task["title"], "Fix the bug");
                assert_eq!(task["tags"][0]["name"], "urgent");
            }
            _ => panic!("Expected TaskUpdate"),
        }
    }

    #[test]
    fn test_server_message_task_deleted_serialization() {
        let msg = ServerMessage::TaskDeleted { task_id: 99 };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("TaskDeleted"));
        assert!(json.contains("99"));

        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::TaskDeleted { task_id } => {
                assert_eq!(task_id, 99);
            }
            _ => panic!("Expected TaskDeleted"),
        }
    }

    #[test]
    fn test_server_message_task_update_from_raw_json() {
        let json = r#"{"type":"TaskUpdate","task":{"id":1,"title":"Test"}}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        match msg {
            ServerMessage::TaskUpdate { task } => {
                assert_eq!(task["id"], 1);
                assert_eq!(task["title"], "Test");
            }
            _ => panic!("Expected TaskUpdate"),
        }
    }

    #[test]
    fn test_server_message_task_deleted_from_raw_json() {
        let json = r#"{"type":"TaskDeleted","task_id":42}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        match msg {
            ServerMessage::TaskDeleted { task_id } => {
                assert_eq!(task_id, 42);
            }
            _ => panic!("Expected TaskDeleted"),
        }
    }

    // === Interconnect protocol roundtrip tests ===

    #[test]
    fn test_client_message_create_invite_roundtrip() {
        let original = ClientMessage::CreateInvite {
            capability: "collaborate".to_string(),
            max_uses: 5,
            expires_in_secs: Some(3600),
            label: Some("For Alice".to_string()),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::CreateInvite {
                capability,
                max_uses,
                expires_in_secs,
                label,
            } => {
                assert_eq!(capability, "collaborate");
                assert_eq!(max_uses, 5);
                assert_eq!(expires_in_secs, Some(3600));
                assert_eq!(label, Some("For Alice".to_string()));
            }
            _ => panic!("Expected CreateInvite"),
        }
    }

    #[test]
    fn test_client_message_create_invite_no_expiry() {
        let json = r#"{"type":"CreateInvite","capability":"view","max_uses":0}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::CreateInvite {
                expires_in_secs, ..
            } => assert!(expires_in_secs.is_none()),
            _ => panic!("Expected CreateInvite"),
        }
    }

    #[test]
    fn test_client_message_redeem_invite_roundtrip() {
        let original = ClientMessage::RedeemInvite {
            token: "ABCDEF123456".to_string(),
            display_name: "Alice".to_string(),
            public_key: "aa".repeat(32),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::RedeemInvite {
                token,
                display_name,
                public_key,
            } => {
                assert_eq!(token, "ABCDEF123456");
                assert_eq!(display_name, "Alice");
                assert_eq!(public_key, "aa".repeat(32));
            }
            _ => panic!("Expected RedeemInvite"),
        }
    }

    #[test]
    fn test_client_message_revoke_invite_roundtrip() {
        let original = ClientMessage::RevokeInvite {
            nonce: "aa".repeat(16),
            suspend_derived: true,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::RevokeInvite {
                nonce,
                suspend_derived,
            } => {
                assert_eq!(nonce, "aa".repeat(16));
                assert!(suspend_derived);
            }
            _ => panic!("Expected RevokeInvite"),
        }
    }

    #[test]
    fn test_client_message_list_invites_roundtrip() {
        let json = r#"{"type":"ListInvites"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, ClientMessage::ListInvites));
    }

    #[test]
    fn test_client_message_list_members_roundtrip() {
        let json = r#"{"type":"ListMembers"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, ClientMessage::ListMembers));
    }

    #[test]
    fn test_client_message_update_member_roundtrip() {
        let original = ClientMessage::UpdateMember {
            public_key: "pk-hex".to_string(),
            capability: Some("admin".to_string()),
            display_name: Some("New Name".to_string()),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::UpdateMember {
                public_key,
                capability,
                display_name,
            } => {
                assert_eq!(public_key, "pk-hex");
                assert_eq!(capability, Some("admin".to_string()));
                assert_eq!(display_name, Some("New Name".to_string()));
            }
            _ => panic!("Expected UpdateMember"),
        }
    }

    #[test]
    fn test_client_message_suspend_member_roundtrip() {
        let original = ClientMessage::SuspendMember {
            public_key: "deadbeef".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::SuspendMember { public_key } => {
                assert_eq!(public_key, "deadbeef");
            }
            _ => panic!("Expected SuspendMember"),
        }
    }

    #[test]
    fn test_client_message_reinstate_member_roundtrip() {
        let original = ClientMessage::ReinstateMember {
            public_key: "cafe".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::ReinstateMember { public_key } => assert_eq!(public_key, "cafe"),
            _ => panic!("Expected ReinstateMember"),
        }
    }

    #[test]
    fn test_client_message_remove_member_roundtrip() {
        let original = ClientMessage::RemoveMember {
            public_key: "babe".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::RemoveMember { public_key } => assert_eq!(public_key, "babe"),
            _ => panic!("Expected RemoveMember"),
        }
    }

    #[test]
    fn test_client_message_query_events_roundtrip() {
        let original = ClientMessage::QueryEvents {
            target: Some("abcd".to_string()),
            event_type_prefix: Some("member.".to_string()),
            limit: 50,
            before_id: Some(100),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::QueryEvents {
                target,
                event_type_prefix,
                limit,
                before_id,
            } => {
                assert_eq!(target, Some("abcd".to_string()));
                assert_eq!(event_type_prefix, Some("member.".to_string()));
                assert_eq!(limit, 50);
                assert_eq!(before_id, Some(100));
            }
            _ => panic!("Expected QueryEvents"),
        }
    }

    #[test]
    fn test_client_message_verify_events_roundtrip() {
        let original = ClientMessage::VerifyEvents {
            from_id: 1,
            to_id: 100,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::VerifyEvents { from_id, to_id } => {
                assert_eq!(from_id, 1);
                assert_eq!(to_id, 100);
            }
            _ => panic!("Expected VerifyEvents"),
        }
    }

    #[test]
    fn test_client_message_get_event_proof_roundtrip() {
        let original = ClientMessage::GetEventProof { event_id: 42 };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::GetEventProof { event_id } => assert_eq!(event_id, 42),
            _ => panic!("Expected GetEventProof"),
        }
    }

    #[test]
    fn test_server_message_invite_created_roundtrip() {
        let original = ServerMessage::InviteCreated {
            token: "CROCK32TOKEN".to_string(),
            nonce: "aabbccdd".to_string(),
            capability: "collaborate".to_string(),
            max_uses: 5,
            expires_at: Some("2025-12-31 23:59:59".to_string()),
            label: Some("QA team".to_string()),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::InviteCreated {
                token,
                nonce,
                capability,
                max_uses,
                expires_at,
                label,
            } => {
                assert_eq!(token, "CROCK32TOKEN");
                assert_eq!(nonce, "aabbccdd");
                assert_eq!(capability, "collaborate");
                assert_eq!(max_uses, 5);
                assert!(expires_at.is_some());
                assert_eq!(label, Some("QA team".to_string()));
            }
            _ => panic!("Expected InviteCreated"),
        }
    }

    #[test]
    fn test_server_message_invite_redeemed_roundtrip() {
        let original = ServerMessage::InviteRedeemed {
            public_key: "pk123".to_string(),
            fingerprint: "crab_ABCD1234".to_string(),
            display_name: "Bob".to_string(),
            capability: "view".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::InviteRedeemed {
                public_key,
                fingerprint,
                display_name,
                capability,
            } => {
                assert_eq!(public_key, "pk123");
                assert_eq!(fingerprint, "crab_ABCD1234");
                assert_eq!(display_name, "Bob");
                assert_eq!(capability, "view");
            }
            _ => panic!("Expected InviteRedeemed"),
        }
    }

    #[test]
    fn test_server_message_invite_revoked_roundtrip() {
        let original = ServerMessage::InviteRevoked {
            nonce: "deadbeef".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::InviteRevoked { nonce } => assert_eq!(nonce, "deadbeef"),
            _ => panic!("Expected InviteRevoked"),
        }
    }

    #[test]
    fn test_server_message_invite_list_roundtrip() {
        let original = ServerMessage::InviteList {
            invites: vec![serde_json::json!({"nonce": "aa", "capability": "view"})],
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::InviteList { invites } => {
                assert_eq!(invites.len(), 1);
                assert_eq!(invites[0]["capability"], "view");
            }
            _ => panic!("Expected InviteList"),
        }
    }

    #[test]
    fn test_server_message_members_list_roundtrip() {
        let original = ServerMessage::MembersList {
            members: vec![serde_json::json!({"display_name": "Alice", "capability": "admin"})],
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::MembersList { members } => {
                assert_eq!(members.len(), 1);
                assert_eq!(members[0]["display_name"], "Alice");
            }
            _ => panic!("Expected MembersList"),
        }
    }

    #[test]
    fn test_server_message_member_joined_roundtrip() {
        let original = ServerMessage::MemberJoined {
            member: serde_json::json!({"public_key": "pk", "display_name": "New"}),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::MemberJoined { member } => {
                assert_eq!(member["display_name"], "New");
            }
            _ => panic!("Expected MemberJoined"),
        }
    }

    #[test]
    fn test_server_message_member_updated_roundtrip() {
        let original = ServerMessage::MemberUpdated {
            member: serde_json::json!({"public_key": "pk", "capability": "admin"}),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::MemberUpdated { member } => {
                assert_eq!(member["capability"], "admin");
            }
            _ => panic!("Expected MemberUpdated"),
        }
    }

    #[test]
    fn test_server_message_member_suspended_roundtrip() {
        let original = ServerMessage::MemberSuspended {
            public_key: "pk1".to_string(),
            fingerprint: "crab_AAAA1111".to_string(),
            display_name: "Suspended User".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::MemberSuspended {
                public_key,
                fingerprint,
                display_name,
            } => {
                assert_eq!(public_key, "pk1");
                assert_eq!(fingerprint, "crab_AAAA1111");
                assert_eq!(display_name, "Suspended User");
            }
            _ => panic!("Expected MemberSuspended"),
        }
    }

    #[test]
    fn test_server_message_member_reinstated_roundtrip() {
        let original = ServerMessage::MemberReinstated {
            public_key: "pk2".to_string(),
            fingerprint: "crab_BBBB2222".to_string(),
            display_name: "Reinstated User".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::MemberReinstated {
                public_key,
                fingerprint,
                display_name,
            } => {
                assert_eq!(public_key, "pk2");
                assert_eq!(fingerprint, "crab_BBBB2222");
                assert_eq!(display_name, "Reinstated User");
            }
            _ => panic!("Expected MemberReinstated"),
        }
    }

    #[test]
    fn test_server_message_member_removed_roundtrip() {
        let original = ServerMessage::MemberRemoved {
            public_key: "pk3".to_string(),
            fingerprint: "crab_CCCC3333".to_string(),
            display_name: "Removed User".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::MemberRemoved {
                public_key,
                fingerprint,
                display_name,
            } => {
                assert_eq!(public_key, "pk3");
                assert_eq!(fingerprint, "crab_CCCC3333");
                assert_eq!(display_name, "Removed User");
            }
            _ => panic!("Expected MemberRemoved"),
        }
    }

    #[test]
    fn test_server_message_events_response_roundtrip() {
        let original = ServerMessage::EventsResponse {
            events: vec![serde_json::json!({"id": 1, "event_type": "member.joined"})],
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::EventsResponse { events } => {
                assert_eq!(events.len(), 1);
                assert_eq!(events[0]["id"], 1);
            }
            _ => panic!("Expected EventsResponse"),
        }
    }

    #[test]
    fn test_server_message_event_verification_roundtrip() {
        let original = ServerMessage::EventVerification {
            valid: true,
            events_checked: 50,
            error: None,
        };
        let json = serde_json::to_string(&original).unwrap();
        assert!(!json.contains("error")); // None should be skipped
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::EventVerification {
                valid,
                events_checked,
                error,
            } => {
                assert!(valid);
                assert_eq!(events_checked, 50);
                assert!(error.is_none());
            }
            _ => panic!("Expected EventVerification"),
        }
    }

    #[test]
    fn test_server_message_event_verification_with_error() {
        let original = ServerMessage::EventVerification {
            valid: false,
            events_checked: 10,
            error: Some("hash mismatch at event 5".to_string()),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::EventVerification { valid, error, .. } => {
                assert!(!valid);
                assert!(error.is_some());
            }
            _ => panic!("Expected EventVerification"),
        }
    }

    #[test]
    fn test_server_message_event_proof_response_roundtrip() {
        let original = ServerMessage::EventProofResponse {
            event: serde_json::json!({"id": 42, "event_type": "invite.created"}),
            nearest_checkpoint: Some(serde_json::json!({"event_id": 50, "signature": "sig"})),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::EventProofResponse {
                event,
                nearest_checkpoint,
            } => {
                assert_eq!(event["id"], 42);
                assert!(nearest_checkpoint.is_some());
            }
            _ => panic!("Expected EventProofResponse"),
        }
    }

    #[test]
    fn test_server_message_event_proof_no_checkpoint() {
        let original = ServerMessage::EventProofResponse {
            event: serde_json::json!({"id": 1}),
            nearest_checkpoint: None,
        };
        let json = serde_json::to_string(&original).unwrap();
        assert!(!json.contains("nearest_checkpoint"));
    }

    #[test]
    fn test_client_message_password_auth_roundtrip() {
        let original = ClientMessage::PasswordAuth {
            username: "alice".to_string(),
            password: "hunter2".to_string(),
            invite_token: Some("ABCDEF123456".to_string()),
            display_name: Some("Alice".to_string()),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::PasswordAuth {
                username,
                password,
                invite_token,
                display_name,
            } => {
                assert_eq!(username, "alice");
                assert_eq!(password, "hunter2");
                assert_eq!(invite_token, Some("ABCDEF123456".to_string()));
                assert_eq!(display_name, Some("Alice".to_string()));
            }
            _ => panic!("Expected PasswordAuth"),
        }
    }

    #[test]
    fn test_client_message_password_auth_no_optional_fields() {
        let json = r#"{"type":"PasswordAuth","username":"bob","password":"pw123"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::PasswordAuth {
                username,
                invite_token,
                display_name,
                ..
            } => {
                assert_eq!(username, "bob");
                assert!(invite_token.is_none());
                assert!(display_name.is_none());
            }
            _ => panic!("Expected PasswordAuth"),
        }
    }

    #[test]
    fn test_client_message_password_auth_skip_serializing_none() {
        let msg = ClientMessage::PasswordAuth {
            username: "x".to_string(),
            password: "y".to_string(),
            invite_token: None,
            display_name: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(!json.contains("invite_token"));
        assert!(!json.contains("display_name"));
    }

    #[test]
    fn test_client_message_loopback_auth_roundtrip() {
        let original = ClientMessage::LoopbackAuth;
        let json = serde_json::to_string(&original).unwrap();
        assert_eq!(json, r#"{"type":"LoopbackAuth"}"#);
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, ClientMessage::LoopbackAuth));
    }

    #[test]
    fn test_client_message_loopback_auth_from_raw_json() {
        let json = r#"{"type":"LoopbackAuth"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, ClientMessage::LoopbackAuth));
    }
}
