//! Federation tunnel protocol: message types for instance-to-instance connections.
//!
//! These wrap the existing `ClientMessage`/`ServerMessage` with per-user routing.
//! The tunnel carries messages for potentially many users over a single QUIC
//! connection; `account_key` tags each message to its originating user.

use serde::{Deserialize, Serialize};

use crate::ws::{ClientMessage, ServerMessage};

/// Messages sent from the connecting (home) instance to the host instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tunnel_type")]
pub enum TunnelClientMessage {
    /// First message: identify the connecting instance.
    Hello { instance_name: String },

    /// Authenticate a specific user within the tunnel.
    /// The host looks up the user's federated account and returns access rights.
    Authenticate {
        /// Hex-encoded ed25519 public key of the user
        account_key: String,
        /// Display name for this user
        display_name: String,
        /// Hex-encoded signature proving ownership of account_key.
        /// Signs the host's node_id bytes (known from the connection token).
        identity_proof: String,
    },

    /// A message from a specific user, forwarded through the tunnel.
    UserMessage {
        /// Hex-encoded ed25519 public key of the originating user
        account_key: String,
        /// The actual client message
        message: ClientMessage,
    },

    /// User disconnected from the home instance (informational).
    UserDisconnected { account_key: String },

    /// Request the host's current instance list (no per-user auth required).
    /// Used when a user switches context to this host.
    RequestInstances,
}

/// Messages sent from the host instance back to the connecting (home) instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tunnel_type")]
pub enum TunnelServerMessage {
    /// Response to Hello: identify the host instance.
    Welcome { instance_name: String },

    /// Response to Authenticate: access granted or denied.
    AuthResult {
        /// Hex-encoded public key this result is for
        account_key: String,
        /// Granted access rights (JSON array), or empty on error
        #[serde(default)]
        access: Vec<serde_json::Value>,
        /// Capability level name (e.g. "collaborate"), or None on error
        #[serde(skip_serializing_if = "Option::is_none")]
        capability: Option<String>,
        /// Error message if authentication failed
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },

    /// A broadcast message from the host, optionally targeted to a specific user.
    /// If `account_key` is None, the message is for all users on this tunnel.
    UserMessage {
        /// Which user this message is for (None = broadcast to all)
        #[serde(skip_serializing_if = "Option::is_none")]
        account_key: Option<String>,
        /// The actual server message
        message: ServerMessage,
    },

    /// Host is closing the tunnel (graceful shutdown or error).
    Goodbye { reason: String },
}

/// Write a `TunnelClientMessage` to a QUIC send stream.
pub async fn write_tunnel_client_message(
    stream: &mut iroh::endpoint::SendStream,
    msg: &TunnelClientMessage,
) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(msg)?;
    let len = (bytes.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

/// Read a `TunnelServerMessage` from a QUIC recv stream.
/// Returns None on clean stream close.
pub async fn read_tunnel_server_message(
    stream: &mut iroh::endpoint::RecvStream,
) -> anyhow::Result<Option<TunnelServerMessage>> {
    use iroh::endpoint::{ReadError, ReadExactError};

    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(()) => {}
        Err(ReadExactError::FinishedEarly(_)) => return Ok(None),
        Err(ReadExactError::ReadError(
            ReadError::Reset(_) | ReadError::ConnectionLost(_) | ReadError::ClosedStream,
        )) => return Ok(None),
        Err(e) => return Err(e.into()),
    }

    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 1024 * 1024 {
        anyhow::bail!("tunnel message too large: {} bytes", len);
    }

    let mut buf = vec![0u8; len];
    match stream.read_exact(&mut buf).await {
        Ok(()) => {}
        Err(ReadExactError::FinishedEarly(_)) => return Ok(None),
        Err(ReadExactError::ReadError(
            ReadError::Reset(_) | ReadError::ConnectionLost(_) | ReadError::ClosedStream,
        )) => return Ok(None),
        Err(e) => return Err(e.into()),
    }

    match serde_json::from_slice(&buf) {
        Ok(msg) => Ok(Some(msg)),
        Err(e) => {
            tracing::warn!(error = %e, "malformed tunnel server message");
            Ok(None)
        }
    }
}

/// Write a `TunnelServerMessage` to a QUIC send stream.
pub async fn write_tunnel_server_message(
    stream: &mut iroh::endpoint::SendStream,
    msg: &TunnelServerMessage,
) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(msg)?;
    let len = (bytes.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

/// Read a `TunnelClientMessage` from a QUIC recv stream.
/// Returns None on clean stream close.
pub async fn read_tunnel_client_message(
    stream: &mut iroh::endpoint::RecvStream,
) -> anyhow::Result<Option<TunnelClientMessage>> {
    use iroh::endpoint::{ReadError, ReadExactError};

    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(()) => {}
        Err(ReadExactError::FinishedEarly(_)) => return Ok(None),
        Err(ReadExactError::ReadError(
            ReadError::Reset(_) | ReadError::ConnectionLost(_) | ReadError::ClosedStream,
        )) => return Ok(None),
        Err(e) => return Err(e.into()),
    }

    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 1024 * 1024 {
        anyhow::bail!("tunnel message too large: {} bytes", len);
    }

    let mut buf = vec![0u8; len];
    match stream.read_exact(&mut buf).await {
        Ok(()) => {}
        Err(ReadExactError::FinishedEarly(_)) => return Ok(None),
        Err(ReadExactError::ReadError(
            ReadError::Reset(_) | ReadError::ConnectionLost(_) | ReadError::ClosedStream,
        )) => return Ok(None),
        Err(e) => return Err(e.into()),
    }

    match serde_json::from_slice(&buf) {
        Ok(msg) => Ok(Some(msg)),
        Err(e) => {
            tracing::warn!(error = %e, "malformed tunnel client message");
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_roundtrip() {
        let msg = TunnelClientMessage::Hello {
            instance_name: "Alice's Lab".into(),
        };
        let bytes = serde_json::to_vec(&msg).unwrap();
        let parsed: TunnelClientMessage = serde_json::from_slice(&bytes).unwrap();
        match parsed {
            TunnelClientMessage::Hello { instance_name } => {
                assert_eq!(instance_name, "Alice's Lab");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn welcome_roundtrip() {
        let msg = TunnelServerMessage::Welcome {
            instance_name: "Bob's Workshop".into(),
        };
        let bytes = serde_json::to_vec(&msg).unwrap();
        let parsed: TunnelServerMessage = serde_json::from_slice(&bytes).unwrap();
        match parsed {
            TunnelServerMessage::Welcome { instance_name } => {
                assert_eq!(instance_name, "Bob's Workshop");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn auth_result_success() {
        let msg = TunnelServerMessage::AuthResult {
            account_key: "aa".repeat(32),
            access: vec![serde_json::json!({"type": "terminals", "actions": ["read"]})],
            capability: Some("collaborate".into()),
            error: None,
        };
        let bytes = serde_json::to_vec(&msg).unwrap();
        let parsed: TunnelServerMessage = serde_json::from_slice(&bytes).unwrap();
        match parsed {
            TunnelServerMessage::AuthResult {
                error, capability, ..
            } => {
                assert!(error.is_none());
                assert_eq!(capability.as_deref(), Some("collaborate"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn auth_result_error() {
        let msg = TunnelServerMessage::AuthResult {
            account_key: "bb".repeat(32),
            access: vec![],
            capability: None,
            error: Some("no federated account".into()),
        };
        let bytes = serde_json::to_vec(&msg).unwrap();
        let parsed: TunnelServerMessage = serde_json::from_slice(&bytes).unwrap();
        match parsed {
            TunnelServerMessage::AuthResult { error, .. } => {
                assert_eq!(error.as_deref(), Some("no federated account"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn user_message_wraps_client_message() {
        let inner = ClientMessage::Focus {
            instance_id: "inst-1".into(),
            since_uuid: None,
        };
        let msg = TunnelClientMessage::UserMessage {
            account_key: "cc".repeat(32),
            message: inner,
        };
        let bytes = serde_json::to_vec(&msg).unwrap();
        let parsed: TunnelClientMessage = serde_json::from_slice(&bytes).unwrap();
        match parsed {
            TunnelClientMessage::UserMessage {
                account_key,
                message: ClientMessage::Focus { instance_id, .. },
            } => {
                assert_eq!(account_key, "cc".repeat(32));
                assert_eq!(instance_id, "inst-1");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn user_message_wraps_server_message() {
        let inner = ServerMessage::InstanceList { instances: vec![] };
        let msg = TunnelServerMessage::UserMessage {
            account_key: None,
            message: inner,
        };
        let bytes = serde_json::to_vec(&msg).unwrap();
        let parsed: TunnelServerMessage = serde_json::from_slice(&bytes).unwrap();
        match parsed {
            TunnelServerMessage::UserMessage {
                account_key,
                message: ServerMessage::InstanceList { instances },
            } => {
                assert!(account_key.is_none());
                assert!(instances.is_empty());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn goodbye_roundtrip() {
        let msg = TunnelServerMessage::Goodbye {
            reason: "shutting down".into(),
        };
        let bytes = serde_json::to_vec(&msg).unwrap();
        let parsed: TunnelServerMessage = serde_json::from_slice(&bytes).unwrap();
        match parsed {
            TunnelServerMessage::Goodbye { reason } => {
                assert_eq!(reason, "shutting down");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn user_disconnected_roundtrip() {
        let msg = TunnelClientMessage::UserDisconnected {
            account_key: "dd".repeat(32),
        };
        let bytes = serde_json::to_vec(&msg).unwrap();
        let parsed: TunnelClientMessage = serde_json::from_slice(&bytes).unwrap();
        match parsed {
            TunnelClientMessage::UserDisconnected { account_key } => {
                assert_eq!(account_key, "dd".repeat(32));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn authenticate_roundtrip() {
        let msg = TunnelClientMessage::Authenticate {
            account_key: "ee".repeat(32),
            display_name: "Alice".into(),
            identity_proof: "ff".repeat(64),
        };
        let bytes = serde_json::to_vec(&msg).unwrap();
        let parsed: TunnelClientMessage = serde_json::from_slice(&bytes).unwrap();
        match parsed {
            TunnelClientMessage::Authenticate {
                account_key,
                display_name,
                identity_proof,
            } => {
                assert_eq!(account_key, "ee".repeat(32));
                assert_eq!(display_name, "Alice");
                assert_eq!(identity_proof, "ff".repeat(64));
            }
            _ => panic!("wrong variant"),
        }
    }

    /// Verify that tunnel messages use `tunnel_type` tag (not `type`),
    /// so they are distinguishable from regular ClientMessage/ServerMessage.
    #[test]
    fn tunnel_tag_is_distinct_from_client_message_tag() {
        // A TunnelClientMessage serializes with "tunnel_type"
        let tunnel = TunnelClientMessage::Hello {
            instance_name: "Test".into(),
        };
        let bytes = serde_json::to_vec(&tunnel).unwrap();
        let obj: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            obj.get("tunnel_type").is_some(),
            "tunnel messages must use tunnel_type tag"
        );
        assert!(
            obj.get("type").is_none(),
            "tunnel messages must NOT use type tag"
        );

        // A regular ClientMessage serializes with "type"
        let client = ClientMessage::Focus {
            instance_id: "inst-1".into(),
            since_uuid: None,
        };
        let client_bytes = serde_json::to_vec(&client).unwrap();
        let client_obj: serde_json::Value = serde_json::from_slice(&client_bytes).unwrap();
        assert!(
            client_obj.get("type").is_some(),
            "client messages must use type tag"
        );
        assert!(
            client_obj.get("tunnel_type").is_none(),
            "client messages must NOT use tunnel_type tag"
        );
    }

    /// Verify cross-deserialization fails: a TunnelClientMessage cannot parse as ClientMessage.
    #[test]
    fn tunnel_message_does_not_parse_as_client_message() {
        let tunnel = TunnelClientMessage::Hello {
            instance_name: "Test".into(),
        };
        let bytes = serde_json::to_vec(&tunnel).unwrap();
        let result = serde_json::from_slice::<ClientMessage>(&bytes);
        assert!(
            result.is_err(),
            "tunnel Hello must not parse as ClientMessage"
        );
    }

    /// Verify the accept loop routing: raw bytes with "tunnel_type" parse as tunnel,
    /// raw bytes with "type" parse as client message.
    #[test]
    fn routing_by_tag_field() {
        // Simulate what the unauthenticated_handler does:
        // read raw bytes, try TunnelClientMessage first, fall back to ClientMessage

        let tunnel_hello = serde_json::to_vec(&TunnelClientMessage::Hello {
            instance_name: "Remote Lab".into(),
        })
        .unwrap();

        let client_redeem = serde_json::json!({
            "type": "RedeemInvite",
            "token": "aa".repeat(16),
            "display_name": "Alice",
            "public_key": "bb".repeat(32),
            "v": 1,
        });
        let client_bytes = serde_json::to_vec(&client_redeem).unwrap();

        // Tunnel Hello should parse as TunnelClientMessage
        let tunnel_result = serde_json::from_slice::<TunnelClientMessage>(&tunnel_hello);
        assert!(tunnel_result.is_ok());
        assert!(matches!(
            tunnel_result.unwrap(),
            TunnelClientMessage::Hello { .. }
        ));

        // Client RedeemInvite should NOT parse as TunnelClientMessage
        let tunnel_fail = serde_json::from_slice::<TunnelClientMessage>(&client_bytes);
        assert!(tunnel_fail.is_err());

        // Client RedeemInvite should parse as ClientMessage (after stripping transport fields)
        let mut obj: serde_json::Value = serde_json::from_slice(&client_bytes).unwrap();
        if let serde_json::Value::Object(ref mut map) = obj {
            map.remove("v");
        }
        let client_result = serde_json::from_value::<ClientMessage>(obj);
        assert!(client_result.is_ok());
        assert!(matches!(
            client_result.unwrap(),
            ClientMessage::RedeemInvite { .. }
        ));
    }
}
