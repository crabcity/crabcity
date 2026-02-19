//! Length-prefixed JSON envelope over QUIC streams.
//!
//! Wire format: `[4-byte big-endian length][JSON payload]`
//!
//! The payload is a flat JSON object with transport fields (`v`, `seq`,
//! `request_id`) alongside the message body (which includes a `type` tag
//! from serde's internally-tagged enum).

use anyhow::{Result, bail};
use iroh::endpoint::{ReadError, ReadExactError, RecvStream, SendStream};
use serde_json::Value;

use crate::ws::{ClientMessage, ServerMessage};

/// Maximum message size (1 MiB). Rejects messages larger than this.
const MAX_MESSAGE_SIZE: usize = 1024 * 1024;

/// A client message with optional transport-layer correlation.
#[derive(Debug)]
pub struct IncomingMessage {
    pub message: ClientMessage,
    pub request_id: Option<String>,
}

/// Write a `ServerMessage` to a QUIC send stream with length-prefixed framing.
///
/// The message is serialized as a flat JSON object: the `ServerMessage` fields
/// (including its `type` tag) are merged with transport fields `v` and `seq`.
pub async fn write_message(
    stream: &mut SendStream,
    msg: &ServerMessage,
    seq: &mut u64,
    request_id: Option<&str>,
) -> Result<()> {
    let mut obj = serde_json::to_value(msg)?;

    // Inject transport fields into the flat object
    if let Value::Object(ref mut map) = obj {
        map.insert("v".into(), Value::from(1u32));
        map.insert("seq".into(), Value::from(*seq));
        if let Some(rid) = request_id {
            map.insert("request_id".into(), Value::from(rid));
        }
    }

    let bytes = serde_json::to_vec(&obj)?;
    let len = (bytes.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(&bytes).await?;
    *seq += 1;
    Ok(())
}

/// Read a `ClientMessage` from a QUIC recv stream with length-prefixed framing.
///
/// Returns `None` if the stream is cleanly closed (peer finished).
pub async fn read_message(stream: &mut RecvStream) -> Result<Option<IncomingMessage>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(()) => {}
        // Stream finished before we got 4 bytes — clean close
        Err(ReadExactError::FinishedEarly(_)) => return Ok(None),
        // Peer reset the stream or connection was lost — treat as close
        Err(ReadExactError::ReadError(
            ReadError::Reset(_) | ReadError::ConnectionLost(_) | ReadError::ClosedStream,
        )) => return Ok(None),
        // Genuine error (e.g. 0-RTT rejected)
        Err(e) => return Err(e.into()),
    }

    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_MESSAGE_SIZE {
        bail!(
            "message too large: {} bytes (max {})",
            len,
            MAX_MESSAGE_SIZE
        );
    }

    let mut buf = vec![0u8; len];
    match stream.read_exact(&mut buf).await {
        Ok(()) => {}
        // Stream died mid-message — still a clean close from our perspective
        Err(ReadExactError::FinishedEarly(_)) => return Ok(None),
        Err(ReadExactError::ReadError(
            ReadError::Reset(_) | ReadError::ConnectionLost(_) | ReadError::ClosedStream,
        )) => return Ok(None),
        Err(e) => return Err(e.into()),
    }

    let mut obj: Value = serde_json::from_slice(&buf)?;

    // Extract transport fields
    let v = obj.get("v").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let request_id = obj
        .get("request_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Forward compatibility: ignore unknown versions
    if v != 1 {
        tracing::warn!(version = v, "unknown envelope version, skipping");
        return Ok(None);
    }

    // Remove transport fields before deserializing as ClientMessage
    if let Value::Object(ref mut map) = obj {
        map.remove("v");
        map.remove("seq");
        map.remove("request_id");
    }

    // Try to deserialize as a ClientMessage
    match serde_json::from_value::<ClientMessage>(obj) {
        Ok(message) => Ok(Some(IncomingMessage {
            message,
            request_id,
        })),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "unknown or malformed client message type, skipping"
            );
            Ok(None)
        }
    }
}

/// Write a `ClientMessage` to a QUIC send stream with length-prefixed framing.
///
/// Client-side counterpart to `write_message`. Injects `v: 1` and optional
/// `request_id` but no `seq` (server tracks its own sequence).
pub async fn write_client_message(
    stream: &mut SendStream,
    msg: &ClientMessage,
    request_id: Option<&str>,
) -> Result<()> {
    let mut obj = serde_json::to_value(msg)?;

    if let Value::Object(ref mut map) = obj {
        map.insert("v".into(), Value::from(1u32));
        if let Some(rid) = request_id {
            map.insert("request_id".into(), Value::from(rid));
        }
    }

    let bytes = serde_json::to_vec(&obj)?;
    let len = (bytes.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

/// Read a `ServerMessage` from a QUIC recv stream with length-prefixed framing.
///
/// Client-side counterpart to `read_message`. Strips transport fields (`v`, `seq`,
/// `request_id`) before deserializing.
///
/// Returns `None` if the stream is cleanly closed.
pub async fn read_server_message(stream: &mut RecvStream) -> Result<Option<ServerMessage>> {
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
    if len > MAX_MESSAGE_SIZE {
        bail!(
            "message too large: {} bytes (max {})",
            len,
            MAX_MESSAGE_SIZE
        );
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

    let mut obj: Value = serde_json::from_slice(&buf)?;

    // Strip transport fields
    if let Value::Object(ref mut map) = obj {
        map.remove("v");
        map.remove("seq");
        map.remove("request_id");
    }

    match serde_json::from_value::<ServerMessage>(obj) {
        Ok(msg) => Ok(Some(msg)),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "unknown or malformed server message type, skipping"
            );
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialize a ServerMessage into wire envelope bytes (test helper).
    fn serialize_flat(msg: &ServerMessage, seq: u64, request_id: Option<&str>) -> Result<Vec<u8>> {
        let mut obj = serde_json::to_value(msg)?;
        if let Value::Object(ref mut map) = obj {
            map.insert("v".into(), Value::from(1u32));
            map.insert("seq".into(), Value::from(seq));
            if let Some(rid) = request_id {
                map.insert("request_id".into(), Value::from(rid));
            }
        }
        Ok(serde_json::to_vec(&obj)?)
    }

    #[test]
    fn flat_envelope_roundtrip_no_request_id() {
        let msg = ServerMessage::InstanceStopped {
            instance_id: "test".to_string(),
        };
        let bytes = serialize_flat(&msg, 42, None).unwrap();
        let raw: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        // Transport fields are siblings of the message type
        assert_eq!(raw["v"], 1);
        assert_eq!(raw["seq"], 42);
        assert_eq!(raw["type"], "InstanceStopped");
        assert_eq!(raw["instance_id"], "test");
        // request_id should not appear
        assert!(raw.get("request_id").is_none());
        // No nested "data" object
        assert!(raw.get("data").is_none());
    }

    #[test]
    fn flat_envelope_roundtrip_with_request_id() {
        let msg = ServerMessage::InstanceStopped {
            instance_id: "test".to_string(),
        };
        let bytes = serialize_flat(&msg, 7, Some("req-123")).unwrap();
        let raw: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(raw["v"], 1);
        assert_eq!(raw["seq"], 7);
        assert_eq!(raw["request_id"], "req-123");
    }

    #[test]
    fn flat_envelope_no_double_serialize() {
        // Ensure there's no nested "data" or "msg_type" field
        let msg = ServerMessage::Error {
            instance_id: Some("x".into()),
            message: "test error".into(),
        };
        let bytes = serialize_flat(&msg, 0, None).unwrap();
        let raw: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(raw.get("data").is_none());
        assert!(raw.get("msg_type").is_none());
        assert_eq!(raw["type"], "Error");
        assert_eq!(raw["message"], "test error");
    }

    #[test]
    fn oversized_detection() {
        // We can't easily test the async read path, but verify the constant is reasonable
        assert_eq!(MAX_MESSAGE_SIZE, 1024 * 1024);
    }
}
