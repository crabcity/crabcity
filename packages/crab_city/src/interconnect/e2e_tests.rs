//! End-to-end federation tests: two real iroh endpoints with a shared relay.
//!
//! These tests prove that the full tunnel pipeline works over real QUIC connections:
//! relay → iroh endpoint → accept loop → tunnel handler → auth + dispatch.

use std::sync::Arc;
use std::time::Duration;

use crab_city_auth::SigningKey;
use iroh::{Endpoint, RelayMode, RelayUrl};
use tokio::time::timeout;

use crate::identity::InstanceIdentity;
use crate::instance_manager::InstanceManager;
use crate::interconnect::protocol::{
    TunnelClientMessage, TunnelServerMessage, read_tunnel_server_message,
    write_tunnel_client_message,
};
use crate::repository::ConversationRepository;
use crate::repository::test_helpers::test_repository;
use crate::transport::iroh_transport::{ALPN, IrohTransport};
use crate::transport::relay::EmbeddedRelay;
use crate::ws::{GlobalStateManager, create_state_broadcast};

/// Timeout for each async operation in tests.
const TEST_TIMEOUT: Duration = Duration::from_secs(15);

fn bytes_to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Start an embedded relay on a random port, return (relay, url).
async fn start_relay() -> (EmbeddedRelay, RelayUrl) {
    let relay = EmbeddedRelay::start(([127, 0, 0, 1], 0).into())
        .await
        .expect("failed to start relay");
    let url = relay.url().clone();
    (relay, url)
}

/// Create a full IrohTransport host and return it along with the repo and node_id.
async fn create_test_host(
    relay_url: RelayUrl,
) -> (IrohTransport, ConversationRepository, [u8; 32]) {
    let repo = test_repository().await;
    let identity = Arc::new(InstanceIdentity::generate());
    let state_manager = Arc::new(GlobalStateManager::new(create_state_broadcast()));
    let instance_manager = Arc::new(InstanceManager::new("echo".into(), 0, 64 * 1024));

    let transport = IrohTransport::start(
        identity,
        Some(relay_url),
        repo.clone(),
        state_manager,
        instance_manager,
        None,
        "Test Host".into(),
    )
    .await
    .expect("failed to start IrohTransport");

    let node_id = transport.node_id();
    (transport, repo, node_id)
}

/// Create a bare iroh client endpoint using the same relay.
async fn create_test_client(relay_url: &RelayUrl) -> Endpoint {
    let secret_key = iroh::SecretKey::generate(&mut rand::rng());
    Endpoint::builder()
        .secret_key(secret_key)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Custom(iroh::RelayMap::from(relay_url.clone())))
        .bind()
        .await
        .expect("failed to bind client endpoint")
}

/// Seed a federated account and return (hex_pubkey, signing_key).
async fn seed_federated_user(
    repo: &ConversationRepository,
    access_json: &str,
) -> (String, SigningKey) {
    let signing_key = SigningKey::generate(&mut rand::rng());
    let account_key = *signing_key.public_key().as_bytes();
    let admin_key = [0xFFu8; 32];
    repo.create_federated_account(
        &account_key,
        "E2E Test User",
        None,
        Some("Client Lab"),
        access_json,
        &admin_key,
    )
    .await
    .expect("failed to seed federated account");
    (bytes_to_hex(&account_key), signing_key)
}

/// Sign the host's node_id with the user's signing key → hex-encoded proof.
fn sign_challenge(signing_key: &SigningKey, host_node_id: &[u8; 32]) -> String {
    let sig = signing_key.sign(host_node_id);
    bytes_to_hex(sig.as_bytes())
}

/// Read tunnel server messages until `predicate` returns true, with a timeout.
/// Discards non-matching messages (e.g. InstanceList before AuthResult).
async fn read_until(
    recv: &mut iroh::endpoint::RecvStream,
    predicate: impl Fn(&TunnelServerMessage) -> bool,
    max_attempts: usize,
) -> Option<TunnelServerMessage> {
    for _ in 0..max_attempts {
        let msg = timeout(TEST_TIMEOUT, read_tunnel_server_message(recv))
            .await
            .expect("timed out reading tunnel message")
            .expect("read error")
            .expect("stream closed");
        if predicate(&msg) {
            return Some(msg);
        }
    }
    None
}

#[tokio::test]
async fn federation_tunnel_full_flow() {
    // 1. Start relay + host + client
    let (relay, relay_url) = start_relay().await;
    let (transport, repo, host_node_id) = create_test_host(relay_url.clone()).await;
    let client_ep = create_test_client(&relay_url).await;

    // 2. Seed federated account
    let access =
        r#"[{"type":"terminals","actions":["read","input"]},{"type":"chat","actions":["send"]}]"#;
    let (account_hex, signing_key) = seed_federated_user(&repo, access).await;

    // 3. Connect client to host
    let host_endpoint_id =
        iroh::EndpointId::from_bytes(&host_node_id).expect("invalid host node id");
    let target = iroh::EndpointAddr::new(host_endpoint_id).with_relay_url(relay_url.clone());

    let conn = timeout(TEST_TIMEOUT, client_ep.connect(target, ALPN))
        .await
        .expect("connect timed out")
        .expect("connection failed");

    let (mut send, mut recv) = timeout(TEST_TIMEOUT, conn.open_bi())
        .await
        .expect("open_bi timed out")
        .expect("open_bi failed");

    // 4. Hello → Welcome
    write_tunnel_client_message(
        &mut send,
        &TunnelClientMessage::Hello {
            instance_name: "Client Lab".into(),
        },
    )
    .await
    .expect("failed to send Hello");

    let welcome = timeout(TEST_TIMEOUT, read_tunnel_server_message(&mut recv))
        .await
        .expect("timed out reading Welcome")
        .expect("read error")
        .expect("stream closed");

    match welcome {
        TunnelServerMessage::Welcome { instance_name } => {
            assert_eq!(instance_name, "Test Host");
        }
        other => panic!("expected Welcome, got: {:?}", other),
    }

    // 5. Authenticate → AuthResult
    let proof = sign_challenge(&signing_key, &host_node_id);
    write_tunnel_client_message(
        &mut send,
        &TunnelClientMessage::Authenticate {
            account_key: account_hex.clone(),
            display_name: "Alice".into(),
            identity_proof: proof,
        },
    )
    .await
    .expect("failed to send Authenticate");

    let auth_result = read_until(
        &mut recv,
        |msg| matches!(msg, TunnelServerMessage::AuthResult { .. }),
        10,
    )
    .await
    .expect("never received AuthResult");

    match auth_result {
        TunnelServerMessage::AuthResult {
            account_key,
            capability,
            error,
            access: granted,
        } => {
            assert_eq!(account_key, account_hex);
            assert!(error.is_none(), "unexpected auth error: {:?}", error);
            assert_eq!(capability.as_deref(), Some("collaborate"));
            assert!(!granted.is_empty());
        }
        _ => unreachable!(),
    }

    // 6. UserMessage { ListMembers } → UserMessage { MembersList }
    write_tunnel_client_message(
        &mut send,
        &TunnelClientMessage::UserMessage {
            account_key: account_hex.clone(),
            message: crate::ws::ClientMessage::ListMembers,
        },
    )
    .await
    .expect("failed to send ListMembers");

    let members_resp = read_until(
        &mut recv,
        |msg| {
            matches!(
                msg,
                TunnelServerMessage::UserMessage {
                    message: crate::ws::ServerMessage::MembersList { .. },
                    ..
                }
            )
        },
        10,
    )
    .await
    .expect("never received MembersList");

    match members_resp {
        TunnelServerMessage::UserMessage {
            message: crate::ws::ServerMessage::MembersList { .. },
            ..
        } => {}
        _ => unreachable!(),
    }

    // 7. UserDisconnected → clean shutdown
    write_tunnel_client_message(
        &mut send,
        &TunnelClientMessage::UserDisconnected {
            account_key: account_hex.clone(),
        },
    )
    .await
    .expect("failed to send UserDisconnected");

    // Give the host time to process
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Cleanup
    send.finish().ok();
    drop(conn);
    client_ep.close().await;
    transport.shutdown().await;
    relay.shutdown().await;
}

#[tokio::test]
async fn federation_tunnel_wrong_proof_rejected() {
    let (relay, relay_url) = start_relay().await;
    let (transport, repo, host_node_id) = create_test_host(relay_url.clone()).await;
    let client_ep = create_test_client(&relay_url).await;

    // Seed user with key A
    let access = r#"[{"type":"terminals","actions":["read"]}]"#;
    let (account_hex, _correct_sk) = seed_federated_user(&repo, access).await;

    // Sign challenge with key B (wrong key)
    let wrong_sk = SigningKey::generate(&mut rand::rng());
    let bad_proof = sign_challenge(&wrong_sk, &host_node_id);

    // Connect
    let host_id = iroh::EndpointId::from_bytes(&host_node_id).unwrap();
    let target = iroh::EndpointAddr::new(host_id).with_relay_url(relay_url.clone());
    let conn = timeout(TEST_TIMEOUT, client_ep.connect(target, ALPN))
        .await
        .expect("connect timed out")
        .expect("connection failed");
    let (mut send, mut recv) = timeout(TEST_TIMEOUT, conn.open_bi())
        .await
        .expect("open_bi timed out")
        .expect("open_bi failed");

    // Hello → Welcome
    write_tunnel_client_message(
        &mut send,
        &TunnelClientMessage::Hello {
            instance_name: "Bad Client".into(),
        },
    )
    .await
    .unwrap();
    let _ = timeout(TEST_TIMEOUT, read_tunnel_server_message(&mut recv))
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // Authenticate with wrong proof
    write_tunnel_client_message(
        &mut send,
        &TunnelClientMessage::Authenticate {
            account_key: account_hex.clone(),
            display_name: "Faker".into(),
            identity_proof: bad_proof,
        },
    )
    .await
    .unwrap();

    let auth_result = read_until(
        &mut recv,
        |msg| matches!(msg, TunnelServerMessage::AuthResult { .. }),
        10,
    )
    .await
    .expect("never received AuthResult");

    match auth_result {
        TunnelServerMessage::AuthResult { error, .. } => {
            let err = error.expect("expected an error");
            assert!(
                err.contains("verification failed"),
                "unexpected error: {err}"
            );
        }
        _ => unreachable!(),
    }

    // Cleanup
    send.finish().ok();
    drop(conn);
    client_ep.close().await;
    transport.shutdown().await;
    relay.shutdown().await;
}

#[tokio::test]
async fn federation_tunnel_unknown_user_rejected() {
    let (relay, relay_url) = start_relay().await;
    let (transport, _repo, host_node_id) = create_test_host(relay_url.clone()).await;
    let client_ep = create_test_client(&relay_url).await;

    // Don't seed any federated account — use a random key
    let sk = SigningKey::generate(&mut rand::rng());
    let account_hex = bytes_to_hex(sk.public_key().as_bytes());
    let proof = sign_challenge(&sk, &host_node_id);

    // Connect
    let host_id = iroh::EndpointId::from_bytes(&host_node_id).unwrap();
    let target = iroh::EndpointAddr::new(host_id).with_relay_url(relay_url.clone());
    let conn = timeout(TEST_TIMEOUT, client_ep.connect(target, ALPN))
        .await
        .expect("connect timed out")
        .expect("connection failed");
    let (mut send, mut recv) = timeout(TEST_TIMEOUT, conn.open_bi())
        .await
        .expect("open_bi timed out")
        .expect("open_bi failed");

    // Hello → Welcome
    write_tunnel_client_message(
        &mut send,
        &TunnelClientMessage::Hello {
            instance_name: "Ghost Client".into(),
        },
    )
    .await
    .unwrap();
    let _ = timeout(TEST_TIMEOUT, read_tunnel_server_message(&mut recv))
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    // Authenticate — valid proof but no account
    write_tunnel_client_message(
        &mut send,
        &TunnelClientMessage::Authenticate {
            account_key: account_hex.clone(),
            display_name: "Ghost".into(),
            identity_proof: proof,
        },
    )
    .await
    .unwrap();

    let auth_result = read_until(
        &mut recv,
        |msg| matches!(msg, TunnelServerMessage::AuthResult { .. }),
        10,
    )
    .await
    .expect("never received AuthResult");

    match auth_result {
        TunnelServerMessage::AuthResult { error, .. } => {
            let err = error.expect("expected an error");
            assert!(
                err.contains("no federated account"),
                "unexpected error: {err}"
            );
        }
        _ => unreachable!(),
    }

    // Cleanup
    send.finish().ok();
    drop(conn);
    client_ep.close().await;
    transport.shutdown().await;
    relay.shutdown().await;
}

// =========================================================================
// Helpers for Phase 5 tests
// =========================================================================

/// Run Hello → Welcome on a connected tunnel, return host's instance name.
async fn handshake(
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    instance_name: &str,
) -> String {
    write_tunnel_client_message(
        send,
        &TunnelClientMessage::Hello {
            instance_name: instance_name.into(),
        },
    )
    .await
    .expect("failed to send Hello");

    let welcome = timeout(TEST_TIMEOUT, read_tunnel_server_message(recv))
        .await
        .expect("timed out")
        .expect("read error")
        .expect("stream closed");

    match welcome {
        TunnelServerMessage::Welcome { instance_name } => instance_name,
        other => panic!("expected Welcome, got: {:?}", other),
    }
}

/// Authenticate a user on the tunnel, assert success, return capability string.
async fn authenticate_user(
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    account_hex: &str,
    display_name: &str,
    signing_key: &SigningKey,
    host_node_id: &[u8; 32],
) -> String {
    let proof = sign_challenge(signing_key, host_node_id);
    write_tunnel_client_message(
        send,
        &TunnelClientMessage::Authenticate {
            account_key: account_hex.into(),
            display_name: display_name.into(),
            identity_proof: proof,
        },
    )
    .await
    .expect("failed to send Authenticate");

    let auth_result = read_until(
        recv,
        |msg| matches!(msg, TunnelServerMessage::AuthResult { .. }),
        10,
    )
    .await
    .expect("never received AuthResult");

    match auth_result {
        TunnelServerMessage::AuthResult {
            capability, error, ..
        } => {
            assert!(error.is_none(), "unexpected auth error: {:?}", error);
            capability.expect("expected capability")
        }
        _ => unreachable!(),
    }
}

/// Connect a client to a host and complete Hello/Welcome.
async fn connect_and_handshake(
    client_ep: &Endpoint,
    host_node_id: &[u8; 32],
    relay_url: &RelayUrl,
    client_name: &str,
) -> (
    iroh::endpoint::Connection,
    iroh::endpoint::SendStream,
    iroh::endpoint::RecvStream,
) {
    let host_id = iroh::EndpointId::from_bytes(host_node_id).unwrap();
    let target = iroh::EndpointAddr::new(host_id).with_relay_url(relay_url.clone());
    let conn = timeout(TEST_TIMEOUT, client_ep.connect(target, ALPN))
        .await
        .expect("connect timed out")
        .expect("connection failed");
    let (mut send, mut recv) = timeout(TEST_TIMEOUT, conn.open_bi())
        .await
        .expect("open_bi timed out")
        .expect("open_bi failed");
    handshake(&mut send, &mut recv, client_name).await;
    (conn, send, recv)
}

// =========================================================================
// Phase 5: multi-user, access gating, suspend, RequestInstances
// =========================================================================

#[tokio::test]
async fn two_users_independent_access_on_same_tunnel() {
    let (relay, relay_url) = start_relay().await;
    let (transport, repo, host_node_id) = create_test_host(relay_url.clone()).await;
    let client_ep = create_test_client(&relay_url).await;

    // Seed two users with different access levels
    let collab_access =
        r#"[{"type":"terminals","actions":["read","input"]},{"type":"chat","actions":["send"]}]"#;
    let view_access =
        r#"[{"type":"terminals","actions":["read"]},{"type":"content","actions":["read"]}]"#;
    let (alice_hex, alice_sk) = seed_federated_user(&repo, collab_access).await;
    let (bob_hex, bob_sk) = seed_federated_user(&repo, view_access).await;

    // Connect
    let (conn, mut send, mut recv) =
        connect_and_handshake(&client_ep, &host_node_id, &relay_url, "Multi-User Lab").await;

    // Authenticate both users
    let alice_cap = authenticate_user(
        &mut send,
        &mut recv,
        &alice_hex,
        "Alice",
        &alice_sk,
        &host_node_id,
    )
    .await;
    assert_eq!(alice_cap, "collaborate");

    let bob_cap = authenticate_user(
        &mut send,
        &mut recv,
        &bob_hex,
        "Bob",
        &bob_sk,
        &host_node_id,
    )
    .await;
    assert_eq!(bob_cap, "view");

    // Both can ListMembers
    for (hex, name) in [(&alice_hex, "Alice"), (&bob_hex, "Bob")] {
        write_tunnel_client_message(
            &mut send,
            &TunnelClientMessage::UserMessage {
                account_key: hex.clone(),
                message: crate::ws::ClientMessage::ListMembers,
            },
        )
        .await
        .unwrap();

        let resp = read_until(
            &mut recv,
            |msg| {
                matches!(
                    msg,
                    TunnelServerMessage::UserMessage {
                        message: crate::ws::ServerMessage::MembersList { .. },
                        ..
                    }
                )
            },
            10,
        )
        .await;
        assert!(resp.is_some(), "{name} should be able to ListMembers");
    }

    // Cleanup
    send.finish().ok();
    drop(conn);
    client_ep.close().await;
    transport.shutdown().await;
    relay.shutdown().await;
}

#[tokio::test]
async fn suspend_one_user_other_continues() {
    let (relay, relay_url) = start_relay().await;
    let (transport, repo, host_node_id) = create_test_host(relay_url.clone()).await;
    let client_ep = create_test_client(&relay_url).await;

    let access =
        r#"[{"type":"terminals","actions":["read","input"]},{"type":"chat","actions":["send"]}]"#;
    let (alice_hex, alice_sk) = seed_federated_user(&repo, access).await;
    let (bob_hex, bob_sk) = seed_federated_user(&repo, access).await;

    let (conn, mut send, mut recv) =
        connect_and_handshake(&client_ep, &host_node_id, &relay_url, "Suspend Lab").await;

    // Authenticate both
    authenticate_user(
        &mut send,
        &mut recv,
        &alice_hex,
        "Alice",
        &alice_sk,
        &host_node_id,
    )
    .await;
    authenticate_user(
        &mut send,
        &mut recv,
        &bob_hex,
        "Bob",
        &bob_sk,
        &host_node_id,
    )
    .await;

    // Suspend Alice at the DB level
    let alice_bytes: Vec<u8> = (0..alice_hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&alice_hex[i..i + 2], 16).unwrap())
        .collect();
    repo.update_federated_state(&alice_bytes, "suspended")
        .await
        .unwrap();

    // Disconnect Alice from tunnel
    write_tunnel_client_message(
        &mut send,
        &TunnelClientMessage::UserDisconnected {
            account_key: alice_hex.clone(),
        },
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Bob can still dispatch messages
    write_tunnel_client_message(
        &mut send,
        &TunnelClientMessage::UserMessage {
            account_key: bob_hex.clone(),
            message: crate::ws::ClientMessage::ListMembers,
        },
    )
    .await
    .unwrap();

    let bob_resp = read_until(
        &mut recv,
        |msg| {
            matches!(
                msg,
                TunnelServerMessage::UserMessage {
                    message: crate::ws::ServerMessage::MembersList { .. },
                    ..
                }
            )
        },
        10,
    )
    .await;
    assert!(
        bob_resp.is_some(),
        "Bob should still be able to ListMembers"
    );

    // Alice cannot re-authenticate (suspended)
    let proof = sign_challenge(&alice_sk, &host_node_id);
    write_tunnel_client_message(
        &mut send,
        &TunnelClientMessage::Authenticate {
            account_key: alice_hex.clone(),
            display_name: "Alice".into(),
            identity_proof: proof,
        },
    )
    .await
    .unwrap();

    let alice_reauth = read_until(
        &mut recv,
        |msg| matches!(msg, TunnelServerMessage::AuthResult { .. }),
        10,
    )
    .await
    .expect("never received AuthResult for re-auth");

    match alice_reauth {
        TunnelServerMessage::AuthResult { error, .. } => {
            assert!(error.is_some(), "suspended Alice should fail re-auth");
        }
        _ => unreachable!(),
    }

    // Cleanup
    send.finish().ok();
    drop(conn);
    client_ep.close().await;
    transport.shutdown().await;
    relay.shutdown().await;
}

#[tokio::test]
async fn view_user_input_denied_through_tunnel() {
    let (relay, relay_url) = start_relay().await;
    let (transport, repo, host_node_id) = create_test_host(relay_url.clone()).await;
    let client_ep = create_test_client(&relay_url).await;

    // View-only access
    let access =
        r#"[{"type":"terminals","actions":["read"]},{"type":"content","actions":["read"]}]"#;
    let (user_hex, user_sk) = seed_federated_user(&repo, access).await;

    let (conn, mut send, mut recv) =
        connect_and_handshake(&client_ep, &host_node_id, &relay_url, "View Lab").await;

    let cap = authenticate_user(
        &mut send,
        &mut recv,
        &user_hex,
        "Viewer",
        &user_sk,
        &host_node_id,
    )
    .await;
    assert_eq!(cap, "view");

    // Send Input — should be denied
    write_tunnel_client_message(
        &mut send,
        &TunnelClientMessage::UserMessage {
            account_key: user_hex.clone(),
            message: crate::ws::ClientMessage::Input {
                instance_id: "some-instance".into(),
                data: "hello".into(),
                task_id: None,
            },
        },
    )
    .await
    .unwrap();

    // Should get an access denied error back
    let error_resp = read_until(
        &mut recv,
        |msg| {
            matches!(
                msg,
                TunnelServerMessage::UserMessage {
                    message: crate::ws::ServerMessage::Error { .. },
                    ..
                }
            )
        },
        10,
    )
    .await
    .expect("never received Error for denied Input");

    match error_resp {
        TunnelServerMessage::UserMessage {
            message: crate::ws::ServerMessage::Error { message, .. },
            ..
        } => {
            assert!(
                message.contains("access denied"),
                "expected 'access denied', got: {message}"
            );
        }
        _ => unreachable!(),
    }

    // Cleanup
    send.finish().ok();
    drop(conn);
    client_ep.close().await;
    transport.shutdown().await;
    relay.shutdown().await;
}

#[tokio::test]
async fn request_instances_returns_host_list() {
    let (relay, relay_url) = start_relay().await;
    let (transport, _repo, host_node_id) = create_test_host(relay_url.clone()).await;
    let client_ep = create_test_client(&relay_url).await;

    let (conn, mut send, mut recv) =
        connect_and_handshake(&client_ep, &host_node_id, &relay_url, "Instance Lab").await;

    // Send RequestInstances (no per-user auth required)
    write_tunnel_client_message(&mut send, &TunnelClientMessage::RequestInstances)
        .await
        .expect("failed to send RequestInstances");

    // Should get back a UserMessage wrapping InstanceList
    let resp = read_until(
        &mut recv,
        |msg| {
            matches!(
                msg,
                TunnelServerMessage::UserMessage {
                    message: crate::ws::ServerMessage::InstanceList { .. },
                    ..
                }
            )
        },
        10,
    )
    .await
    .expect("never received InstanceList response");

    match resp {
        TunnelServerMessage::UserMessage {
            account_key,
            message: crate::ws::ServerMessage::InstanceList { instances },
        } => {
            // account_key is None for broadcast messages
            assert!(account_key.is_none());
            // Host has no running instances in test — list should be empty
            assert!(instances.is_empty());
        }
        _ => unreachable!(),
    }

    // Cleanup
    send.finish().ok();
    drop(conn);
    client_ep.close().await;
    transport.shutdown().await;
    relay.shutdown().await;
}
