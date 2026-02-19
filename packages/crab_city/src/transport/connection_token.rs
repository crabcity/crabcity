//! Connection token: compact wire format for sharing connection info.
//!
//! Binary format: `[1B version=1][32B node_id][16B invite_nonce][remaining: relay_url or empty]`
//!
//! The token encodes everything a client needs to connect:
//! - The server's node ID (ed25519 public key for QUIC identity)
//! - An invite nonce to redeem on first connect
//! - An optional relay URL hint (for private/airgapped deployments)
//!
//! Default mode (public relays): no relay URL needed. Token = 49 bytes → ~79 base32 chars.

use crab_city_auth::encoding::{crockford_decode, crockford_encode};

const VERSION: u8 = 1;

/// A connection token that encodes server identity + invite nonce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionToken {
    pub node_id: [u8; 32],
    pub invite_nonce: [u8; 16],
    /// Optional relay URL hint (for private/airgapped deployments).
    /// Absent in the default case (public relays handle routing).
    pub relay_url: Option<String>,
}

impl ConnectionToken {
    /// Serialize to binary wire format.
    pub fn to_bytes(&self) -> Vec<u8> {
        let relay_bytes = self.relay_url.as_deref().unwrap_or("").as_bytes();
        let mut buf = Vec::with_capacity(1 + 32 + 16 + relay_bytes.len());
        buf.push(VERSION);
        buf.extend_from_slice(&self.node_id);
        buf.extend_from_slice(&self.invite_nonce);
        buf.extend_from_slice(relay_bytes);
        buf
    }

    /// Deserialize from binary wire format.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() {
            return Err("empty token".into());
        }
        if bytes[0] != VERSION {
            return Err(format!("unsupported token version: {}", bytes[0]));
        }
        if bytes.len() < 1 + 32 + 16 {
            return Err(format!("token too short: {} bytes (min 49)", bytes.len()));
        }

        let node_id: [u8; 32] = bytes[1..33].try_into().unwrap();
        let invite_nonce: [u8; 16] = bytes[33..49].try_into().unwrap();

        let relay_url = if bytes.len() > 49 {
            let s =
                std::str::from_utf8(&bytes[49..]).map_err(|e| format!("invalid relay URL: {e}"))?;
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        } else {
            None
        };

        Ok(Self {
            node_id,
            invite_nonce,
            relay_url,
        })
    }

    /// Encode as Crockford base32 string.
    pub fn to_base32(&self) -> String {
        crockford_encode(&self.to_bytes())
    }

    /// Decode from Crockford base32 string.
    pub fn from_base32(s: &str) -> Result<Self, String> {
        let bytes = crockford_decode(s)?;
        Self::from_bytes(&bytes)
    }
}

impl std::fmt::Display for ConnectionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_base32())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_no_relay() {
        let token = ConnectionToken {
            node_id: [0xaa; 32],
            invite_nonce: [0xbb; 16],
            relay_url: None,
        };
        let bytes = token.to_bytes();
        assert_eq!(bytes.len(), 49);
        assert_eq!(bytes[0], 1); // version

        let decoded = ConnectionToken::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, token);
    }

    #[test]
    fn roundtrip_with_relay() {
        let token = ConnectionToken {
            node_id: [0xcc; 32],
            invite_nonce: [0xdd; 16],
            relay_url: Some("http://192.168.1.100:4434".to_string()),
        };
        let bytes = token.to_bytes();
        assert!(bytes.len() > 49);

        let decoded = ConnectionToken::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, token);
    }

    #[test]
    fn base32_roundtrip() {
        let token = ConnectionToken {
            node_id: [0x01; 32],
            invite_nonce: [0x02; 16],
            relay_url: None,
        };
        let encoded = token.to_base32();
        let decoded = ConnectionToken::from_base32(&encoded).unwrap();
        assert_eq!(decoded, token);
    }

    #[test]
    fn base32_roundtrip_with_relay() {
        let token = ConnectionToken {
            node_id: [0xff; 32],
            invite_nonce: [0x00; 16],
            relay_url: Some("https://relay.example.com".to_string()),
        };
        let encoded = token.to_base32();
        let decoded = ConnectionToken::from_base32(&encoded).unwrap();
        assert_eq!(decoded, token);
    }

    #[test]
    fn display_matches_base32() {
        let token = ConnectionToken {
            node_id: [0x42; 32],
            invite_nonce: [0x13; 16],
            relay_url: None,
        };
        assert_eq!(format!("{token}"), token.to_base32());
    }

    #[test]
    fn too_short() {
        let err = ConnectionToken::from_bytes(&[1; 20]).unwrap_err();
        assert!(err.contains("too short"));
    }

    #[test]
    fn wrong_version() {
        let mut bytes = vec![99u8]; // version 99
        bytes.extend_from_slice(&[0; 48]);
        let err = ConnectionToken::from_bytes(&bytes).unwrap_err();
        assert!(err.contains("unsupported token version"));
    }

    #[test]
    fn empty_input() {
        let err = ConnectionToken::from_bytes(&[]).unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn base32_length_no_relay() {
        // 49 bytes → ceil(49*8/5) = 79 base32 chars
        let token = ConnectionToken {
            node_id: [0; 32],
            invite_nonce: [0; 16],
            relay_url: None,
        };
        let encoded = token.to_base32();
        // Crockford base32: 49 bytes = 79 chars (ceil(49*8/5) = 78.4 → 79 with padding bits)
        assert!(encoded.len() <= 80, "encoded length: {}", encoded.len());
    }
}
