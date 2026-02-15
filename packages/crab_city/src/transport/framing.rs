//! Length-prefixed JSON envelope over QUIC streams.
//!
//! Wire format: `[4-byte big-endian length][JSON payload]`
//!
//! Envelope: `{ "v": 1, "seq": N, "type": "...", "data": {...} }`

use anyhow::{Result, bail};
use iroh::endpoint::{RecvStream, SendStream};
use serde::{Deserialize, Serialize};

use crate::ws::{ClientMessage, ServerMessage};

/// Maximum message size (1 MiB). Rejects messages larger than this.
const MAX_MESSAGE_SIZE: usize = 1024 * 1024;

/// Wire envelope wrapping every message.
#[derive(Debug, Serialize, Deserialize)]
struct Envelope {
    /// Protocol version (currently 1).
    v: u32,
    /// Monotonically increasing sequence number.
    seq: u64,
    /// Message type tag (matches serde `type` field).
    #[serde(rename = "type")]
    msg_type: String,
    /// The message payload.
    data: serde_json::Value,
}

/// Write a `ServerMessage` to a QUIC send stream with length-prefixed framing.
pub async fn write_message(
    stream: &mut SendStream,
    msg: &ServerMessage,
    seq: &mut u64,
) -> Result<()> {
    let data = serde_json::to_value(msg)?;
    let msg_type = data
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let envelope = Envelope {
        v: 1,
        seq: *seq,
        msg_type,
        data,
    };

    let bytes = serde_json::to_vec(&envelope)?;
    let len = (bytes.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(&bytes).await?;
    *seq += 1;
    Ok(())
}

/// Read a `ClientMessage` from a QUIC recv stream with length-prefixed framing.
///
/// Returns `None` if the stream is cleanly closed (peer finished).
pub async fn read_message(stream: &mut RecvStream) -> Result<Option<ClientMessage>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(()) => {}
        Err(e) => {
            // quinn ReadExactError wraps ReadError; check if it's a clean finish
            let msg = e.to_string();
            if msg.contains("closed") || msg.contains("finished") || msg.contains("reset") {
                return Ok(None);
            }
            return Err(e.into());
        }
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
    stream.read_exact(&mut buf).await?;

    let envelope: Envelope = serde_json::from_slice(&buf)?;

    // Forward compatibility: ignore unknown versions
    if envelope.v != 1 {
        tracing::warn!(version = envelope.v, "unknown envelope version, skipping");
        return Ok(None);
    }

    // Try to deserialize the data as a ClientMessage
    match serde_json::from_value::<ClientMessage>(envelope.data.clone()) {
        Ok(msg) => Ok(Some(msg)),
        Err(e) => {
            tracing::warn!(
                msg_type = %envelope.msg_type,
                error = %e,
                "unknown or malformed client message type, skipping"
            );
            Ok(None)
        }
    }
}

/// Serialize a ServerMessage into the wire envelope bytes (for replay buffer storage).
pub fn serialize_envelope(msg: &ServerMessage, seq: u64) -> Result<Vec<u8>> {
    let data = serde_json::to_value(msg)?;
    let msg_type = data
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    let envelope = Envelope {
        v: 1,
        seq,
        msg_type,
        data,
    };

    Ok(serde_json::to_vec(&envelope)?)
}

/// Write raw pre-serialized envelope bytes to a stream with length prefix.
pub async fn write_raw(stream: &mut SendStream, bytes: &[u8]) -> Result<()> {
    let len = (bytes.len() as u32).to_be_bytes();
    stream.write_all(&len).await?;
    stream.write_all(bytes).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_roundtrip() {
        let msg = ServerMessage::InstanceStopped {
            instance_id: "test".to_string(),
        };
        let bytes = serialize_envelope(&msg, 42).unwrap();
        let envelope: Envelope = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(envelope.v, 1);
        assert_eq!(envelope.seq, 42);
        assert_eq!(envelope.msg_type, "InstanceStopped");
    }

    #[test]
    fn oversized_detection() {
        // We can't easily test the async read path, but verify the constant is reasonable
        assert_eq!(MAX_MESSAGE_SIZE, 1024 * 1024);
    }
}
