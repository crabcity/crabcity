//! HostHandler: inbound federation tunnel handler.
//!
//! Accepts connections from other Crab City instances and dispatches per-user
//! messages. This is the "host side" — another instance connecting to yours
//! on behalf of its users.

use std::collections::HashMap;
use std::sync::Arc;

use crab_city_auth::{Capability, PublicKey, Signature, keys};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::auth::AuthUser;
use crate::handlers::interconnect::RpcContext;
use crate::instance_manager::InstanceManager;
use crate::repository::ConversationRepository;
use crate::virtual_terminal::ClientType;
use crate::ws::dispatch::{
    ConnectionContext, DispatchResult, auth_user_to_ws_user, disconnect_cleanup,
    dispatch_client_message,
};
use crate::ws::{GlobalStateManager, ServerMessage};

use super::protocol::{
    TunnelClientMessage, TunnelServerMessage, read_tunnel_client_message,
    write_tunnel_server_message,
};

/// Context needed to handle an instance tunnel.
pub struct TunnelContext {
    pub repo: ConversationRepository,
    pub rpc_ctx: Arc<RpcContext>,
    pub state_manager: Arc<GlobalStateManager>,
    pub instance_manager: Arc<InstanceManager>,
    pub instance_name: String,
    pub max_history_bytes: usize,
    /// The host's node_id (ed25519 public key bytes). Used to verify
    /// identity_proof signatures from authenticating users.
    pub host_node_id: [u8; 32],
    /// Name of the remote (connecting) instance. Used to annotate federated
    /// user presence, e.g. "Alice (via Bob's Lab)".
    pub remote_instance_name: String,
}

/// Per-user session state within a tunnel.
struct TunnelUserSession {
    auth_user: AuthUser,
    conn_ctx: Arc<ConnectionContext>,
}

/// Handle an inbound instance tunnel connection.
///
/// Called when the accept loop receives a `TunnelClientMessage::Hello` as the
/// first message on a connection (instead of the usual `RedeemInvite` or
/// normal client message).
pub async fn handle_tunnel(
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    remote_instance_name: String,
    cancel: CancellationToken,
    ctx: Arc<TunnelContext>,
) {
    info!(
        remote = %remote_instance_name,
        "instance tunnel opened"
    );

    // Send Welcome
    if let Err(e) = write_tunnel_server_message(
        &mut send,
        &TunnelServerMessage::Welcome {
            instance_name: ctx.instance_name.clone(),
        },
    )
    .await
    {
        error!(remote = %remote_instance_name, "failed to send Welcome: {}", e);
        return;
    }

    // Per-user sessions within this tunnel, keyed by hex-encoded account_key
    let mut users: HashMap<String, TunnelUserSession> = HashMap::new();

    // Channel for outbound messages (from dispatch handlers back to the tunnel)
    let (out_tx, mut out_rx) = mpsc::channel::<TunnelServerMessage>(100);

    // Spawn sender task
    let send_cancel = cancel.clone();
    let sender_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = send_cancel.cancelled() => break,
                msg = out_rx.recv() => {
                    match msg {
                        Some(msg) => {
                            if let Err(e) = write_tunnel_server_message(&mut send, &msg).await {
                                error!("tunnel send error: {}", e);
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    });

    // Main receive loop
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!(remote = %remote_instance_name, "tunnel cancelled");
                break;
            }
            msg = read_tunnel_client_message(&mut recv) => {
                match msg {
                    Ok(Some(tunnel_msg)) => {
                        match tunnel_msg {
                            TunnelClientMessage::Hello { .. } => {
                                // Duplicate hello — ignore
                                warn!(remote = %remote_instance_name, "duplicate Hello");
                            }
                            TunnelClientMessage::Authenticate {
                                account_key,
                                display_name,
                                identity_proof,
                            } => {
                                handle_authenticate(
                                    &ctx,
                                    &account_key,
                                    &display_name,
                                    &identity_proof,
                                    &out_tx,
                                    &mut users,
                                ).await;
                            }
                            TunnelClientMessage::UserMessage {
                                account_key,
                                message,
                            } => {
                                handle_user_message(
                                    &account_key,
                                    message,
                                    &users,
                                    &out_tx,
                                    &ctx,
                                ).await;
                            }
                            TunnelClientMessage::UserDisconnected { account_key } => {
                                if let Some(session) = users.remove(&account_key) {
                                    disconnect_cleanup(&session.conn_ctx).await;
                                    info!(
                                        remote = %remote_instance_name,
                                        user = %account_key,
                                        "federated user disconnected"
                                    );
                                }
                            }
                            TunnelClientMessage::RequestInstances => {
                                let instances = ctx.instance_manager.list().await;
                                let _ = out_tx
                                    .send(TunnelServerMessage::UserMessage {
                                        account_key: None,
                                        message: ServerMessage::InstanceList { instances },
                                    })
                                    .await;
                            }
                        }
                    }
                    Ok(None) => {
                        info!(remote = %remote_instance_name, "tunnel stream closed");
                        break;
                    }
                    Err(e) => {
                        error!(remote = %remote_instance_name, "tunnel read error: {}", e);
                        break;
                    }
                }
            }
        }
    }

    // Cleanup all user sessions
    for (key, session) in users.drain() {
        disconnect_cleanup(&session.conn_ctx).await;
        debug!(user = %key, "cleaned up federated user session");
    }

    // Drop out_tx to signal sender to exit
    drop(out_tx);
    let _ = sender_task.await;

    info!(remote = %remote_instance_name, "tunnel handler exited");
}

/// Authenticate a user within the tunnel by looking up their federated account.
///
/// The `identity_proof` is a hex-encoded ed25519 signature over the host's
/// `node_id` bytes, proving the caller owns the `account_key`.
async fn handle_authenticate(
    ctx: &TunnelContext,
    account_key_hex: &str,
    display_name: &str,
    identity_proof_hex: &str,
    out_tx: &mpsc::Sender<TunnelServerMessage>,
    users: &mut HashMap<String, TunnelUserSession>,
) {
    // Decode the account key (hex → bytes)
    let account_key = match hex_to_bytes(account_key_hex) {
        Some(bytes) if bytes.len() == 32 => bytes,
        _ => {
            let _ = out_tx
                .send(TunnelServerMessage::AuthResult {
                    account_key: account_key_hex.to_string(),
                    access: vec![],
                    capability: None,
                    error: Some("invalid account key".into()),
                })
                .await;
            return;
        }
    };

    // Verify identity proof: the signature must be a valid ed25519 signature
    // over the host's node_id, made by the key claimed in account_key.
    let key_arr: [u8; 32] = account_key
        .clone()
        .try_into()
        .expect("already validated as 32 bytes");
    let claimed_pk = PublicKey::from_bytes(key_arr);

    match hex_to_bytes(identity_proof_hex) {
        Some(sig_bytes) if sig_bytes.len() == 64 => {
            let sig_arr: [u8; 64] = sig_bytes.try_into().unwrap();
            let sig = Signature::from_bytes(sig_arr);
            if keys::verify(&claimed_pk, &ctx.host_node_id, &sig).is_err() {
                warn!(
                    user = %account_key_hex,
                    "identity proof verification failed"
                );
                let _ = out_tx
                    .send(TunnelServerMessage::AuthResult {
                        account_key: account_key_hex.to_string(),
                        access: vec![],
                        capability: None,
                        error: Some("identity proof verification failed".into()),
                    })
                    .await;
                return;
            }
        }
        _ => {
            let _ = out_tx
                .send(TunnelServerMessage::AuthResult {
                    account_key: account_key_hex.to_string(),
                    access: vec![],
                    capability: None,
                    error: Some("invalid identity proof".into()),
                })
                .await;
            return;
        }
    }

    // Look up federated account
    let federated = match ctx.repo.get_active_federated_account(&account_key).await {
        Ok(Some(acct)) => acct,
        Ok(None) => {
            // No federated account — check if they have a local member grant
            // (for backward compatibility during transition)
            let _ = out_tx
                .send(TunnelServerMessage::AuthResult {
                    account_key: account_key_hex.to_string(),
                    access: vec![],
                    capability: None,
                    error: Some("no federated account on this instance".into()),
                })
                .await;
            return;
        }
        Err(e) => {
            error!("failed to look up federated account: {}", e);
            let _ = out_tx
                .send(TunnelServerMessage::AuthResult {
                    account_key: account_key_hex.to_string(),
                    access: vec![],
                    capability: None,
                    error: Some("internal error".into()),
                })
                .await;
            return;
        }
    };

    // Parse the access rights and capability
    let access: Vec<serde_json::Value> =
        serde_json::from_str(&federated.access).unwrap_or_default();

    // Determine capability from access rights
    let capability = determine_capability(&access);

    // Build AuthUser for this federated user (reuse claimed_pk from proof verification).
    // Annotate the display name with the home instance name so presence shows provenance,
    // e.g. "Alice (via Bob's Lab)".
    let annotated_name = format!("{} (via {})", display_name, ctx.remote_instance_name);
    let auth_user = AuthUser::from_grant(claimed_pk, annotated_name, capability);

    // Create a ConnectionContext for message dispatch
    let (user_tx, mut user_rx) = mpsc::channel::<ServerMessage>(100);
    let conn_id = format!("tunnel-{}", uuid::Uuid::new_v4());

    let conn_ctx = Arc::new(ConnectionContext::new(
        conn_id,
        Some(auth_user_to_ws_user(&auth_user)),
        user_tx,
        ctx.state_manager.clone(),
        ctx.instance_manager.clone(),
        Some(Arc::new(ctx.repo.clone())),
        ctx.max_history_bytes,
        ClientType::Iroh,
        None,
    ));

    // Spawn a task to forward this user's outbound messages through the tunnel
    let forward_tx = out_tx.clone();
    let user_key = account_key_hex.to_string();
    tokio::spawn(async move {
        while let Some(msg) = user_rx.recv().await {
            let _ = forward_tx
                .send(TunnelServerMessage::UserMessage {
                    account_key: Some(user_key.clone()),
                    message: msg,
                })
                .await;
        }
    });

    // Send initial instance list to this user
    let instances = ctx.instance_manager.list().await;
    let _ = conn_ctx
        .tx
        .send(ServerMessage::InstanceList { instances })
        .await;

    users.insert(
        account_key_hex.to_string(),
        TunnelUserSession {
            auth_user,
            conn_ctx,
        },
    );

    info!(
        user = %account_key_hex,
        display_name = %display_name,
        capability = %capability,
        "federated user authenticated"
    );

    let _ = out_tx
        .send(TunnelServerMessage::AuthResult {
            account_key: account_key_hex.to_string(),
            access,
            capability: Some(capability.to_string()),
            error: None,
        })
        .await;
}

/// Dispatch a message from a specific federated user.
async fn handle_user_message(
    account_key_hex: &str,
    message: crate::ws::ClientMessage,
    users: &HashMap<String, TunnelUserSession>,
    out_tx: &mpsc::Sender<TunnelServerMessage>,
    ctx: &TunnelContext,
) {
    let session = match users.get(account_key_hex) {
        Some(s) => s,
        None => {
            let _ = out_tx
                .send(TunnelServerMessage::UserMessage {
                    account_key: Some(account_key_hex.to_string()),
                    message: ServerMessage::Error {
                        instance_id: None,
                        message: "not authenticated".into(),
                    },
                })
                .await;
            return;
        }
    };

    match dispatch_client_message(&session.conn_ctx, message).await {
        DispatchResult::Handled => {}
        DispatchResult::Unhandled(msg) => {
            // Interconnect RPCs from federated users
            let resp = dispatch_tunnel_rpc(ctx, &session.auth_user, msg).await;
            let _ = out_tx
                .send(TunnelServerMessage::UserMessage {
                    account_key: Some(account_key_hex.to_string()),
                    message: resp,
                })
                .await;
        }
    }
}

/// Handle interconnect RPC messages from federated users.
/// Similar to `IrohTransport::dispatch_rpc` but without disconnect_tx
/// (federated users disconnect via the tunnel, not individual connections).
async fn dispatch_tunnel_rpc(
    ctx: &TunnelContext,
    auth_user: &AuthUser,
    msg: crate::ws::ClientMessage,
) -> ServerMessage {
    use crate::handlers::interconnect;
    use crate::ws::ClientMessage;

    match msg {
        ClientMessage::ListMembers => {
            interconnect::handle_list_members(&ctx.rpc_ctx, auth_user).await
        }
        ClientMessage::ListInvites => {
            interconnect::handle_list_invites(&ctx.rpc_ctx, auth_user).await
        }
        ClientMessage::QueryEvents {
            target,
            event_type_prefix,
            limit,
            before_id,
        } => {
            interconnect::handle_query_events(
                &ctx.rpc_ctx,
                auth_user,
                target.as_deref(),
                event_type_prefix.as_deref(),
                limit,
                before_id,
            )
            .await
        }
        // Federated users cannot create invites, modify members, etc. on the host
        _ => ServerMessage::Error {
            instance_id: None,
            message: "operation not permitted for federated users".into(),
        },
    }
}

/// Decode a hex string to bytes. Returns None on invalid hex.
fn hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// Determine capability level from JSON access rights.
fn determine_capability(access: &[serde_json::Value]) -> Capability {
    let has_action = |resource: &str, action: &str| -> bool {
        access.iter().any(|entry| {
            entry.get("type").and_then(|t| t.as_str()) == Some(resource)
                && entry
                    .get("actions")
                    .and_then(|a| a.as_array())
                    .is_some_and(|actions| actions.iter().any(|a| a.as_str() == Some(action)))
        })
    };

    if has_action("members", "invite") || has_action("instance", "manage") {
        Capability::Admin
    } else if has_action("terminals", "input") {
        Capability::Collaborate
    } else {
        Capability::View
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn determine_capability_view() {
        let access: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"type":"content","actions":["read"]},{"type":"terminals","actions":["read"]}]"#,
        )
        .unwrap();
        assert_eq!(determine_capability(&access), Capability::View);
    }

    #[test]
    fn determine_capability_collaborate() {
        let access: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"type":"terminals","actions":["read","input"]},{"type":"chat","actions":["send"]}]"#,
        )
        .unwrap();
        assert_eq!(determine_capability(&access), Capability::Collaborate);
    }

    #[test]
    fn determine_capability_admin() {
        let access: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"type":"members","actions":["invite","read"]},{"type":"terminals","actions":["read","input"]}]"#,
        )
        .unwrap();
        assert_eq!(determine_capability(&access), Capability::Admin);
    }

    #[test]
    fn determine_capability_empty() {
        assert_eq!(determine_capability(&[]), Capability::View);
    }

    #[test]
    fn determine_capability_instance_manage_is_admin() {
        let access: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"type":"instance","actions":["manage"]},{"type":"terminals","actions":["read"]}]"#,
        )
        .unwrap();
        assert_eq!(determine_capability(&access), Capability::Admin);
    }

    #[test]
    fn determine_capability_input_without_chat_is_collaborate() {
        let access: Vec<serde_json::Value> =
            serde_json::from_str(r#"[{"type":"terminals","actions":["read","input"]}]"#).unwrap();
        assert_eq!(determine_capability(&access), Capability::Collaborate);
    }

    #[test]
    fn determine_capability_read_only_no_input_is_view() {
        let access: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"type":"terminals","actions":["read"]},{"type":"chat","actions":["read"]}]"#,
        )
        .unwrap();
        assert_eq!(determine_capability(&access), Capability::View);
    }

    #[test]
    fn hex_to_bytes_valid() {
        assert_eq!(hex_to_bytes("deadbeef"), Some(vec![0xde, 0xad, 0xbe, 0xef]));
    }

    #[test]
    fn hex_to_bytes_empty() {
        assert_eq!(hex_to_bytes(""), Some(vec![]));
    }

    #[test]
    fn hex_to_bytes_odd_length() {
        assert_eq!(hex_to_bytes("abc"), None);
    }

    #[test]
    fn hex_to_bytes_invalid_chars() {
        assert_eq!(hex_to_bytes("zzzz"), None);
    }

    #[test]
    fn hex_to_bytes_32_byte_key() {
        let hex = "aa".repeat(32);
        let result = hex_to_bytes(&hex);
        assert!(result.is_some());
        let bytes = result.unwrap();
        assert_eq!(bytes.len(), 32);
        assert!(bytes.iter().all(|&b| b == 0xaa));
    }

    // =========================================================================
    // Integration tests: exercises the full auth + dispatch flow
    // =========================================================================

    use crab_city_auth::SigningKey;

    use crate::identity::InstanceIdentity;
    use crate::repository::test_helpers::test_repository;
    use crate::ws::create_state_broadcast;
    use tokio::sync::broadcast;

    /// A known host_node_id for all tests.
    const TEST_HOST_NODE_ID: [u8; 32] = [0x42; 32];

    fn bytes_to_hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Generate a valid hex-encoded identity proof for a given signing key.
    /// The proof is an ed25519 signature of `TEST_HOST_NODE_ID`.
    fn make_proof(signing_key: &SigningKey) -> String {
        let sig = signing_key.sign(&TEST_HOST_NODE_ID);
        bytes_to_hex(sig.as_bytes())
    }

    /// Drain all pending messages from the channel, yielding to let spawned
    /// forwarder tasks flush their messages first.
    async fn drain(rx: &mut mpsc::Receiver<TunnelServerMessage>) {
        // Yield a few times to let the spawned per-user forwarder tasks run
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        while rx.try_recv().is_ok() {}
    }

    /// Receive the next message, waiting briefly for spawned forwarders.
    async fn recv_next(rx: &mut mpsc::Receiver<TunnelServerMessage>) -> TunnelServerMessage {
        tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for message")
            .expect("channel closed")
    }

    /// Receive messages until we find an AuthResult, returning it.
    /// Other messages (like InstanceList) are discarded.
    async fn recv_auth_result(rx: &mut mpsc::Receiver<TunnelServerMessage>) -> TunnelServerMessage {
        for _ in 0..10 {
            let msg = recv_next(rx).await;
            if matches!(msg, TunnelServerMessage::AuthResult { .. }) {
                return msg;
            }
        }
        panic!("did not receive AuthResult within 10 messages");
    }

    /// Receive messages until we find a UserMessage wrapping a specific
    /// ServerMessage variant, returning it. Other messages are discarded.
    async fn recv_user_message(
        rx: &mut mpsc::Receiver<TunnelServerMessage>,
    ) -> TunnelServerMessage {
        for _ in 0..10 {
            let msg = recv_next(rx).await;
            if matches!(msg, TunnelServerMessage::UserMessage { .. }) {
                return msg;
            }
        }
        panic!("did not receive UserMessage within 10 messages");
    }

    /// Build a test TunnelContext backed by an in-memory DB.
    async fn test_tunnel_ctx() -> (Arc<TunnelContext>, ConversationRepository) {
        let repo = test_repository().await;
        let identity = Arc::new(InstanceIdentity::generate());
        let (broadcast_tx, _rx) = broadcast::channel(16);
        let rpc_ctx = Arc::new(RpcContext {
            repo: repo.clone(),
            identity,
            broadcast_tx,
        });
        let state_manager = Arc::new(GlobalStateManager::new(create_state_broadcast()));
        let instance_manager = Arc::new(InstanceManager::new("echo".into(), 0, 64 * 1024));

        let ctx = Arc::new(TunnelContext {
            repo: repo.clone(),
            rpc_ctx,
            state_manager,
            instance_manager,
            instance_name: "Test Instance".into(),
            max_history_bytes: 64 * 1024,
            host_node_id: TEST_HOST_NODE_ID,
            remote_instance_name: "Remote Lab".into(),
        });
        (ctx, repo)
    }

    /// Seed a federated account with a real ed25519 key and return (hex_key, signing_key).
    /// The signing key can be used with `make_proof()` to create a valid identity proof.
    async fn seed_federated_account(
        repo: &ConversationRepository,
        access: &str,
    ) -> (String, SigningKey) {
        let signing_key = SigningKey::generate(&mut rand::rng());
        let account_key = *signing_key.public_key().as_bytes();
        let admin_key = [0xffu8; 32];
        repo.create_federated_account(
            &account_key,
            "TestUser",
            None,
            Some("Remote Lab"),
            access,
            &admin_key,
        )
        .await
        .unwrap();
        (bytes_to_hex(&account_key), signing_key)
    }

    #[tokio::test]
    async fn authenticate_known_user_succeeds() {
        let (ctx, repo) = test_tunnel_ctx().await;
        let access = r#"[{"type":"terminals","actions":["read","input"]},{"type":"chat","actions":["send"]}]"#;
        let (account_hex, sk) = seed_federated_account(&repo, access).await;
        let proof = make_proof(&sk);

        let (out_tx, mut out_rx) = mpsc::channel::<TunnelServerMessage>(16);
        let mut users = HashMap::new();

        handle_authenticate(&ctx, &account_hex, "Alice", &proof, &out_tx, &mut users).await;

        let msg = out_rx.try_recv().unwrap();
        match msg {
            TunnelServerMessage::AuthResult {
                account_key,
                capability,
                error,
                access: granted_access,
            } => {
                assert_eq!(account_key, account_hex);
                assert!(error.is_none(), "expected no error, got: {:?}", error);
                assert_eq!(capability.as_deref(), Some("collaborate"));
                assert!(!granted_access.is_empty());
            }
            other => panic!("expected AuthResult, got: {:?}", other),
        }

        assert!(users.contains_key(&account_hex));
    }

    #[tokio::test]
    async fn authenticate_unknown_user_fails() {
        let (ctx, _repo) = test_tunnel_ctx().await;
        // Generate a key that has no federated account
        let sk = SigningKey::generate(&mut rand::rng());
        let unknown_hex = bytes_to_hex(sk.public_key().as_bytes());
        let proof = make_proof(&sk);

        let (out_tx, mut out_rx) = mpsc::channel::<TunnelServerMessage>(16);
        let mut users = HashMap::new();

        handle_authenticate(&ctx, &unknown_hex, "Ghost", &proof, &out_tx, &mut users).await;

        let msg = out_rx.try_recv().unwrap();
        match msg {
            TunnelServerMessage::AuthResult { error, .. } => {
                assert!(error.is_some());
                assert!(
                    error.as_ref().unwrap().contains("no federated account"),
                    "unexpected error: {:?}",
                    error,
                );
            }
            other => panic!("expected AuthResult error, got: {:?}", other),
        }

        assert!(!users.contains_key(&unknown_hex));
    }

    #[tokio::test]
    async fn authenticate_invalid_hex_key_fails() {
        let (ctx, _repo) = test_tunnel_ctx().await;

        let (out_tx, mut out_rx) = mpsc::channel::<TunnelServerMessage>(16);
        let mut users = HashMap::new();

        handle_authenticate(
            &ctx,
            "not-valid-hex",
            "Bad",
            "deadbeef",
            &out_tx,
            &mut users,
        )
        .await;

        let msg = out_rx.try_recv().unwrap();
        match msg {
            TunnelServerMessage::AuthResult { error, .. } => {
                assert!(error.is_some());
                assert!(error.as_ref().unwrap().contains("invalid account key"));
            }
            other => panic!("expected AuthResult error, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn authenticate_bad_proof_rejected() {
        let (ctx, repo) = test_tunnel_ctx().await;
        let access = r#"[{"type":"terminals","actions":["read"]}]"#;
        let (account_hex, _sk) = seed_federated_account(&repo, access).await;

        // Use a WRONG key to sign the proof (not the one matching account_key)
        let wrong_sk = SigningKey::generate(&mut rand::rng());
        let bad_proof = make_proof(&wrong_sk);

        let (out_tx, mut out_rx) = mpsc::channel::<TunnelServerMessage>(16);
        let mut users = HashMap::new();

        handle_authenticate(&ctx, &account_hex, "Faker", &bad_proof, &out_tx, &mut users).await;

        let msg = out_rx.try_recv().unwrap();
        match msg {
            TunnelServerMessage::AuthResult { error, .. } => {
                assert!(error.is_some());
                assert!(
                    error.as_ref().unwrap().contains("verification failed"),
                    "unexpected error: {:?}",
                    error,
                );
            }
            other => panic!("expected AuthResult error, got: {:?}", other),
        }
        assert!(!users.contains_key(&account_hex));
    }

    #[tokio::test]
    async fn authenticate_suspended_user_fails() {
        let (ctx, repo) = test_tunnel_ctx().await;
        let access = r#"[{"type":"terminals","actions":["read"]}]"#;
        let (account_hex, sk) = seed_federated_account(&repo, access).await;
        let proof = make_proof(&sk);

        // Suspend the account
        let key_bytes: [u8; 32] = hex_to_bytes(&account_hex).unwrap().try_into().unwrap();
        repo.update_federated_state(&key_bytes, "suspended")
            .await
            .unwrap();

        let (out_tx, mut out_rx) = mpsc::channel::<TunnelServerMessage>(16);
        let mut users = HashMap::new();

        handle_authenticate(&ctx, &account_hex, "Suspended", &proof, &out_tx, &mut users).await;

        let msg = out_rx.try_recv().unwrap();
        match msg {
            TunnelServerMessage::AuthResult { error, .. } => {
                assert!(error.is_some());
                assert!(error.as_ref().unwrap().contains("no federated account"));
            }
            other => panic!("expected AuthResult error, got: {:?}", other),
        }
        assert!(!users.contains_key(&account_hex));
    }

    #[tokio::test]
    async fn authenticate_grants_correct_capability_view() {
        let (ctx, repo) = test_tunnel_ctx().await;
        let access =
            r#"[{"type":"terminals","actions":["read"]},{"type":"content","actions":["read"]}]"#;
        let (account_hex, sk) = seed_federated_account(&repo, access).await;
        let proof = make_proof(&sk);

        let (out_tx, mut out_rx) = mpsc::channel::<TunnelServerMessage>(16);
        let mut users = HashMap::new();

        handle_authenticate(&ctx, &account_hex, "Viewer", &proof, &out_tx, &mut users).await;

        let msg = out_rx.try_recv().unwrap();
        match msg {
            TunnelServerMessage::AuthResult { capability, .. } => {
                assert_eq!(capability.as_deref(), Some("view"));
            }
            other => panic!("expected AuthResult, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn authenticate_grants_correct_capability_admin() {
        let (ctx, repo) = test_tunnel_ctx().await;
        let access = r#"[{"type":"members","actions":["invite","read"]},{"type":"terminals","actions":["read","input"]}]"#;
        let (account_hex, sk) = seed_federated_account(&repo, access).await;
        let proof = make_proof(&sk);

        let (out_tx, mut out_rx) = mpsc::channel::<TunnelServerMessage>(16);
        let mut users = HashMap::new();

        handle_authenticate(&ctx, &account_hex, "Admin", &proof, &out_tx, &mut users).await;

        let msg = out_rx.try_recv().unwrap();
        match msg {
            TunnelServerMessage::AuthResult { capability, .. } => {
                assert_eq!(capability.as_deref(), Some("admin"));
            }
            other => panic!("expected AuthResult, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn unauthenticated_user_message_rejected() {
        let (ctx, _repo) = test_tunnel_ctx().await;
        let unknown_hex = bytes_to_hex(&[0xee; 32]);

        let (out_tx, mut out_rx) = mpsc::channel::<TunnelServerMessage>(16);
        let users = HashMap::new();

        let msg = crate::ws::ClientMessage::ListMembers;
        handle_user_message(&unknown_hex, msg, &users, &out_tx, &ctx).await;

        let response = out_rx.try_recv().unwrap();
        match response {
            TunnelServerMessage::UserMessage {
                account_key,
                message: ServerMessage::Error { message, .. },
            } => {
                assert_eq!(account_key.as_deref(), Some(unknown_hex.as_str()));
                assert!(message.contains("not authenticated"));
            }
            other => panic!("expected Error UserMessage, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn authenticated_user_message_dispatched() {
        let (ctx, repo) = test_tunnel_ctx().await;
        let access = r#"[{"type":"terminals","actions":["read","input"]},{"type":"chat","actions":["send"]}]"#;
        let (account_hex, sk) = seed_federated_account(&repo, access).await;
        let proof = make_proof(&sk);

        let (out_tx, mut out_rx) = mpsc::channel::<TunnelServerMessage>(16);
        let mut users = HashMap::new();

        handle_authenticate(&ctx, &account_hex, "Alice", &proof, &out_tx, &mut users).await;
        drain(&mut out_rx).await;

        let msg = crate::ws::ClientMessage::Focus {
            instance_id: "nonexistent".into(),
            since_uuid: None,
        };
        handle_user_message(&account_hex, msg, &users, &out_tx, &ctx).await;

        let response = recv_user_message(&mut out_rx).await;
        match response {
            TunnelServerMessage::UserMessage { account_key, .. } => {
                assert_eq!(account_key.as_deref(), Some(account_hex.as_str()));
            }
            other => panic!("expected UserMessage, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn two_users_authenticate_independently() {
        let (ctx, repo) = test_tunnel_ctx().await;
        let collab_access = r#"[{"type":"terminals","actions":["read","input"]}]"#;
        let view_access = r#"[{"type":"terminals","actions":["read"]}]"#;

        let (alice_hex, alice_sk) = seed_federated_account(&repo, collab_access).await;
        let (bob_hex, bob_sk) = seed_federated_account(&repo, view_access).await;

        let (out_tx, mut out_rx) = mpsc::channel::<TunnelServerMessage>(16);
        let mut users = HashMap::new();

        handle_authenticate(
            &ctx,
            &alice_hex,
            "Alice",
            &make_proof(&alice_sk),
            &out_tx,
            &mut users,
        )
        .await;
        let alice_result = recv_auth_result(&mut out_rx).await;
        match &alice_result {
            TunnelServerMessage::AuthResult {
                capability, error, ..
            } => {
                assert!(error.is_none());
                assert_eq!(capability.as_deref(), Some("collaborate"));
            }
            _ => unreachable!(),
        }
        drain(&mut out_rx).await;

        handle_authenticate(
            &ctx,
            &bob_hex,
            "Bob",
            &make_proof(&bob_sk),
            &out_tx,
            &mut users,
        )
        .await;
        let bob_result = recv_auth_result(&mut out_rx).await;
        match &bob_result {
            TunnelServerMessage::AuthResult {
                capability, error, ..
            } => {
                assert!(error.is_none());
                assert_eq!(capability.as_deref(), Some("view"));
            }
            _ => unreachable!(),
        }

        assert_eq!(users.len(), 2);
        assert!(users.contains_key(&alice_hex));
        assert!(users.contains_key(&bob_hex));
        assert_eq!(
            users[&alice_hex].auth_user.capability,
            Capability::Collaborate
        );
        assert_eq!(users[&bob_hex].auth_user.capability, Capability::View);
    }

    #[tokio::test]
    async fn suspend_one_user_other_unaffected() {
        let (ctx, repo) = test_tunnel_ctx().await;
        let access = r#"[{"type":"terminals","actions":["read","input"]}]"#;
        let (alice_hex, alice_sk) = seed_federated_account(&repo, access).await;
        let (bob_hex, bob_sk) = seed_federated_account(&repo, access).await;

        let (out_tx, mut out_rx) = mpsc::channel::<TunnelServerMessage>(16);
        let mut users = HashMap::new();

        handle_authenticate(
            &ctx,
            &alice_hex,
            "Alice",
            &make_proof(&alice_sk),
            &out_tx,
            &mut users,
        )
        .await;
        drain(&mut out_rx).await;
        handle_authenticate(
            &ctx,
            &bob_hex,
            "Bob",
            &make_proof(&bob_sk),
            &out_tx,
            &mut users,
        )
        .await;
        drain(&mut out_rx).await;
        assert_eq!(users.len(), 2);

        // Suspend Alice at the DB level
        let alice_bytes: [u8; 32] = hex_to_bytes(&alice_hex).unwrap().try_into().unwrap();
        repo.update_federated_state(&alice_bytes, "suspended")
            .await
            .unwrap();

        let alice_session = users.remove(&alice_hex).unwrap();
        disconnect_cleanup(&alice_session.conn_ctx).await;

        assert_eq!(users.len(), 1);
        assert!(users.contains_key(&bob_hex));

        let msg = crate::ws::ClientMessage::ListMembers;
        handle_user_message(&bob_hex, msg, &users, &out_tx, &ctx).await;
        let bob_resp = recv_user_message(&mut out_rx).await;
        match bob_resp {
            TunnelServerMessage::UserMessage {
                message: ServerMessage::MembersList { .. },
                ..
            } => {}
            other => panic!("expected MembersList for Bob, got: {:?}", other),
        }

        // Alice cannot re-authenticate (suspended)
        handle_authenticate(
            &ctx,
            &alice_hex,
            "Alice",
            &make_proof(&alice_sk),
            &out_tx,
            &mut users,
        )
        .await;
        let result = recv_auth_result(&mut out_rx).await;
        match result {
            TunnelServerMessage::AuthResult { error, .. } => {
                assert!(error.is_some());
            }
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn federated_rpc_list_members_allowed() {
        let (ctx, repo) = test_tunnel_ctx().await;
        let access = r#"[{"type":"terminals","actions":["read"]}]"#;
        let (account_hex, sk) = seed_federated_account(&repo, access).await;
        let proof = make_proof(&sk);

        let (out_tx, mut out_rx) = mpsc::channel::<TunnelServerMessage>(16);
        let mut users = HashMap::new();

        handle_authenticate(&ctx, &account_hex, "Viewer", &proof, &out_tx, &mut users).await;
        drain(&mut out_rx).await;

        let msg = crate::ws::ClientMessage::ListMembers;
        handle_user_message(&account_hex, msg, &users, &out_tx, &ctx).await;

        let response = recv_user_message(&mut out_rx).await;
        match response {
            TunnelServerMessage::UserMessage {
                message: ServerMessage::MembersList { .. },
                ..
            } => {}
            other => panic!("expected MemberList, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn federated_rpc_disallowed_operations_rejected() {
        let (ctx, repo) = test_tunnel_ctx().await;
        let access = r#"[{"type":"terminals","actions":["read","input"]}]"#;
        let (account_hex, sk) = seed_federated_account(&repo, access).await;
        let proof = make_proof(&sk);

        let (out_tx, mut out_rx) = mpsc::channel::<TunnelServerMessage>(16);
        let mut users = HashMap::new();

        handle_authenticate(&ctx, &account_hex, "Collab", &proof, &out_tx, &mut users).await;
        drain(&mut out_rx).await;

        let msg = crate::ws::ClientMessage::SuspendMember {
            public_key: "aa".repeat(32),
        };
        handle_user_message(&account_hex, msg, &users, &out_tx, &ctx).await;

        let response = recv_user_message(&mut out_rx).await;
        match response {
            TunnelServerMessage::UserMessage {
                message: ServerMessage::Error { message, .. },
                ..
            } => {
                assert!(
                    message.contains("not permitted"),
                    "unexpected error: {}",
                    message
                );
            }
            other => panic!("expected Error, got: {:?}", other),
        }
    }
}
