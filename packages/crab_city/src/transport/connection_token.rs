//! Connection token: compact wire format for sharing connection info.
//!
//! ## V1 format (49+ bytes)
//! ```text
//! [1B version=1][32B node_id][16B invite_nonce][remaining: relay_url or empty]
//! ```
//!
//! ## V2 format (122+ bytes)
//! ```text
//! [1B version=2]
//! [32B node_id]              — host instance ed25519 pubkey
//! [16B invite_nonce]         — nonce to redeem
//! [1B name_len]              — length of instance name (0-255)
//! [name_len B instance_name] — UTF-8
//! [8B inviter_fingerprint]   — first 8 bytes of SHA-256 of inviter's pubkey
//! [1B capability]            — 0=view, 1=collaborate, 2=admin, 3=owner
//! [64B signature]            — ed25519 over all preceding bytes by instance key
//! [remaining: relay_url]     — optional
//! ```
//!
//! The token encodes everything a client needs to connect:
//! - The server's node ID (ed25519 public key for QUIC identity)
//! - An invite nonce to redeem on first connect
//! - An optional relay URL hint (for private/airgapped deployments)
//! - (V2) Instance name, inviter fingerprint, capability level, and signature
//!
//! Default mode (public relays): no relay URL needed. V1 token = 49 bytes → ~79 base32 chars.

use crab_city_auth::encoding::{crockford_decode, crockford_encode};
use crab_city_auth::{PublicKey, Signature, SigningKey};
use sha2::{Digest, Sha256};

const VERSION_V1: u8 = 1;
const VERSION_V2: u8 = 2;

/// A connection token that encodes server identity + invite nonce.
///
/// V1 tokens carry only `node_id`, `invite_nonce`, and an optional `relay_url`.
/// V2 tokens additionally carry instance metadata, an inviter fingerprint,
/// a capability level, and a cryptographic signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionToken {
    pub node_id: [u8; 32],
    pub invite_nonce: [u8; 16],
    /// Optional relay URL hint (for private/airgapped deployments).
    /// Absent in the default case (public relays handle routing).
    pub relay_url: Option<String>,

    // --- V2 fields (all None for v1 tokens) ---
    /// Human-readable instance name (0-255 bytes UTF-8).
    pub instance_name: Option<String>,
    /// First 8 bytes of SHA-256 of the inviter's public key.
    pub inviter_fingerprint: Option<[u8; 8]>,
    /// Capability level: 0=view, 1=collaborate, 2=admin, 3=owner.
    pub capability: Option<u8>,
    /// Ed25519 signature over all preceding bytes, by the instance key.
    pub signature: Option<[u8; 64]>,
}

impl ConnectionToken {
    /// Returns true when any v2 metadata fields are set.
    fn has_v2_metadata(&self) -> bool {
        self.instance_name.is_some()
            || self.inviter_fingerprint.is_some()
            || self.capability.is_some()
            || self.signature.is_some()
    }

    /// Serialize to binary wire format.
    ///
    /// Produces v2 format when any metadata fields are present, v1 otherwise.
    pub fn to_bytes(&self) -> Vec<u8> {
        if self.has_v2_metadata() {
            self.to_bytes_v2()
        } else {
            self.to_bytes_v1()
        }
    }

    fn to_bytes_v1(&self) -> Vec<u8> {
        let relay_bytes = self.relay_url.as_deref().unwrap_or("").as_bytes();
        let mut buf = Vec::with_capacity(1 + 32 + 16 + relay_bytes.len());
        buf.push(VERSION_V1);
        buf.extend_from_slice(&self.node_id);
        buf.extend_from_slice(&self.invite_nonce);
        buf.extend_from_slice(relay_bytes);
        buf
    }

    fn to_bytes_v2(&self) -> Vec<u8> {
        let name_bytes = self.instance_name.as_deref().unwrap_or("").as_bytes();
        let name_len = name_bytes.len().min(255) as u8;
        let fingerprint = self.inviter_fingerprint.unwrap_or([0u8; 8]);
        let capability = self.capability.unwrap_or(0);
        let signature = self.signature.unwrap_or([0u8; 64]);
        let relay_bytes = self.relay_url.as_deref().unwrap_or("").as_bytes();

        // 1 + 32 + 16 + 1 + name_len + 8 + 1 + 64 + relay
        let capacity = 1 + 32 + 16 + 1 + (name_len as usize) + 8 + 1 + 64 + relay_bytes.len();
        let mut buf = Vec::with_capacity(capacity);
        buf.push(VERSION_V2);
        buf.extend_from_slice(&self.node_id);
        buf.extend_from_slice(&self.invite_nonce);
        buf.push(name_len);
        buf.extend_from_slice(&name_bytes[..name_len as usize]);
        buf.extend_from_slice(&fingerprint);
        buf.push(capability);
        buf.extend_from_slice(&signature);
        buf.extend_from_slice(relay_bytes);
        buf
    }

    /// Build the v2 byte prefix (everything before the signature) for signing/verification.
    fn signable_bytes_v2(&self) -> Vec<u8> {
        let name_bytes = self.instance_name.as_deref().unwrap_or("").as_bytes();
        let name_len = name_bytes.len().min(255) as u8;
        let fingerprint = self.inviter_fingerprint.unwrap_or([0u8; 8]);
        let capability = self.capability.unwrap_or(0);

        // Everything before the 64-byte signature
        let capacity = 1 + 32 + 16 + 1 + (name_len as usize) + 8 + 1;
        let mut buf = Vec::with_capacity(capacity);
        buf.push(VERSION_V2);
        buf.extend_from_slice(&self.node_id);
        buf.extend_from_slice(&self.invite_nonce);
        buf.push(name_len);
        buf.extend_from_slice(&name_bytes[..name_len as usize]);
        buf.extend_from_slice(&fingerprint);
        buf.push(capability);
        buf
    }

    /// Deserialize from binary wire format. Supports both v1 and v2.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() {
            return Err("empty token".into());
        }
        match bytes[0] {
            VERSION_V1 => Self::from_bytes_v1(bytes),
            VERSION_V2 => Self::from_bytes_v2(bytes),
            v => Err(format!("unsupported token version: {v}")),
        }
    }

    fn from_bytes_v1(bytes: &[u8]) -> Result<Self, String> {
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
            instance_name: None,
            inviter_fingerprint: None,
            capability: None,
            signature: None,
        })
    }

    fn from_bytes_v2(bytes: &[u8]) -> Result<Self, String> {
        // Minimum: 1 + 32 + 16 + 1 (name_len=0) + 8 + 1 + 64 = 123
        const MIN_V2: usize = 1 + 32 + 16 + 1 + 8 + 1 + 64;
        if bytes.len() < MIN_V2 {
            return Err(format!(
                "v2 token too short: {} bytes (min {MIN_V2})",
                bytes.len()
            ));
        }

        let node_id: [u8; 32] = bytes[1..33].try_into().unwrap();
        let invite_nonce: [u8; 16] = bytes[33..49].try_into().unwrap();

        let name_len = bytes[49] as usize;
        let name_end = 50 + name_len;

        // Check we have enough bytes for name + fingerprint + capability + signature
        let sig_end = name_end + 8 + 1 + 64;
        if bytes.len() < sig_end {
            return Err(format!(
                "v2 token too short for name_len={name_len}: {} bytes (need {sig_end})",
                bytes.len()
            ));
        }

        let instance_name = if name_len > 0 {
            let s = std::str::from_utf8(&bytes[50..name_end])
                .map_err(|e| format!("invalid instance name: {e}"))?;
            Some(s.to_string())
        } else {
            None
        };

        let fingerprint: [u8; 8] = bytes[name_end..name_end + 8].try_into().unwrap();
        let capability = bytes[name_end + 8];
        let signature: [u8; 64] = bytes[name_end + 9..sig_end].try_into().unwrap();

        let relay_url = if bytes.len() > sig_end {
            let s = std::str::from_utf8(&bytes[sig_end..])
                .map_err(|e| format!("invalid relay URL: {e}"))?;
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
            instance_name,
            inviter_fingerprint: Some(fingerprint),
            capability: Some(capability),
            signature: Some(signature),
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

    /// Compute the inviter fingerprint: first 8 bytes of SHA-256 of a public key.
    pub fn compute_fingerprint(pubkey: &[u8; 32]) -> [u8; 8] {
        let hash = Sha256::digest(pubkey);
        let mut fp = [0u8; 8];
        fp.copy_from_slice(&hash[..8]);
        fp
    }

    /// Sign this token with the instance's signing key.
    ///
    /// Produces an ed25519 signature over all bytes preceding the signature field
    /// in the v2 wire format. Sets the `signature` field on self.
    pub fn sign(&mut self, signing_key: &SigningKey) {
        let message = self.signable_bytes_v2();
        let sig = signing_key.sign(&message);
        self.signature = Some(*sig.as_bytes());
    }

    /// Verify the v2 signature against the given instance public key.
    ///
    /// Returns `false` if no signature is present (v1 token) or verification fails.
    pub fn verify_signature(&self, instance_pubkey: &[u8; 32]) -> bool {
        let Some(sig_bytes) = self.signature else {
            return false;
        };
        let message = self.signable_bytes_v2();
        let pubkey = PublicKey::from_bytes(*instance_pubkey);
        let signature = Signature::from_bytes(sig_bytes);
        crab_city_auth::keys::verify(&pubkey, &message, &signature).is_ok()
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

    /// Helper to create a simple v1 token.
    fn v1_token() -> ConnectionToken {
        ConnectionToken {
            node_id: [0xaa; 32],
            invite_nonce: [0xbb; 16],
            relay_url: None,
            instance_name: None,
            inviter_fingerprint: None,
            capability: None,
            signature: None,
        }
    }

    // ---- Existing v1 tests (preserved) ----

    #[test]
    fn roundtrip_no_relay() {
        let token = ConnectionToken {
            node_id: [0xaa; 32],
            invite_nonce: [0xbb; 16],
            relay_url: None,
            instance_name: None,
            inviter_fingerprint: None,
            capability: None,
            signature: None,
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
            instance_name: None,
            inviter_fingerprint: None,
            capability: None,
            signature: None,
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
            instance_name: None,
            inviter_fingerprint: None,
            capability: None,
            signature: None,
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
            instance_name: None,
            inviter_fingerprint: None,
            capability: None,
            signature: None,
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
            instance_name: None,
            inviter_fingerprint: None,
            capability: None,
            signature: None,
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
            instance_name: None,
            inviter_fingerprint: None,
            capability: None,
            signature: None,
        };
        let encoded = token.to_base32();
        // Crockford base32: 49 bytes = 79 chars (ceil(49*8/5) = 78.4 → 79 with padding bits)
        assert!(encoded.len() <= 80, "encoded length: {}", encoded.len());
    }

    // ---- V2 tests ----

    #[test]
    fn v2_roundtrip_with_metadata() {
        let mut rng = rand::rng();
        let sk = SigningKey::generate(&mut rng);
        let pk = sk.public_key();
        let fp = ConnectionToken::compute_fingerprint(pk.as_bytes());

        let mut token = ConnectionToken {
            node_id: *pk.as_bytes(),
            invite_nonce: [0x42; 16],
            relay_url: None,
            instance_name: Some("my-instance".to_string()),
            inviter_fingerprint: Some(fp),
            capability: Some(1), // collaborate
            signature: None,
        };
        token.sign(&sk);

        let bytes = token.to_bytes();
        assert_eq!(bytes[0], 2); // v2

        let decoded = ConnectionToken::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, token);
        assert_eq!(decoded.instance_name.as_deref(), Some("my-instance"));
        assert_eq!(decoded.inviter_fingerprint, Some(fp));
        assert_eq!(decoded.capability, Some(1));
        assert!(decoded.signature.is_some());
    }

    #[test]
    fn v2_roundtrip_with_relay() {
        let mut rng = rand::rng();
        let sk = SigningKey::generate(&mut rng);
        let pk = sk.public_key();
        let fp = ConnectionToken::compute_fingerprint(pk.as_bytes());

        let mut token = ConnectionToken {
            node_id: *pk.as_bytes(),
            invite_nonce: [0x99; 16],
            relay_url: Some("https://relay.private.net:4433".to_string()),
            instance_name: Some("prod-server".to_string()),
            inviter_fingerprint: Some(fp),
            capability: Some(2), // admin
            signature: None,
        };
        token.sign(&sk);

        let bytes = token.to_bytes();
        let decoded = ConnectionToken::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, token);
        assert_eq!(
            decoded.relay_url.as_deref(),
            Some("https://relay.private.net:4433")
        );
    }

    #[test]
    fn v2_roundtrip_empty_name() {
        let mut rng = rand::rng();
        let sk = SigningKey::generate(&mut rng);
        let fp = ConnectionToken::compute_fingerprint(sk.public_key().as_bytes());

        let mut token = ConnectionToken {
            node_id: *sk.public_key().as_bytes(),
            invite_nonce: [0x11; 16],
            relay_url: None,
            instance_name: None,
            inviter_fingerprint: Some(fp),
            capability: Some(0), // view
            signature: None,
        };
        token.sign(&sk);

        let bytes = token.to_bytes();
        assert_eq!(bytes[0], 2);
        assert_eq!(bytes[49], 0); // name_len = 0

        let decoded = ConnectionToken::from_bytes(&bytes).unwrap();
        assert_eq!(decoded, token);
        assert!(decoded.instance_name.is_none());
    }

    #[test]
    fn v1_backward_compat() {
        // A v1 token should parse correctly and have None for all v2 fields.
        let token = v1_token();
        let bytes = token.to_bytes();
        assert_eq!(bytes[0], 1);

        let decoded = ConnectionToken::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.node_id, [0xaa; 32]);
        assert_eq!(decoded.invite_nonce, [0xbb; 16]);
        assert!(decoded.relay_url.is_none());
        assert!(decoded.instance_name.is_none());
        assert!(decoded.inviter_fingerprint.is_none());
        assert!(decoded.capability.is_none());
        assert!(decoded.signature.is_none());
    }

    #[test]
    fn v1_does_not_produce_v2() {
        // A token with no v2 fields should serialize as v1.
        let token = v1_token();
        let bytes = token.to_bytes();
        assert_eq!(bytes[0], 1);
        assert_eq!(bytes.len(), 49);
    }

    #[test]
    fn signature_verification_valid() {
        let mut rng = rand::rng();
        let sk = SigningKey::generate(&mut rng);
        let pk = sk.public_key();
        let fp = ConnectionToken::compute_fingerprint(pk.as_bytes());

        let mut token = ConnectionToken {
            node_id: *pk.as_bytes(),
            invite_nonce: [0x77; 16],
            relay_url: None,
            instance_name: Some("test".to_string()),
            inviter_fingerprint: Some(fp),
            capability: Some(1),
            signature: None,
        };
        token.sign(&sk);

        assert!(token.verify_signature(pk.as_bytes()));
    }

    #[test]
    fn signature_verification_after_roundtrip() {
        let mut rng = rand::rng();
        let sk = SigningKey::generate(&mut rng);
        let pk = sk.public_key();
        let fp = ConnectionToken::compute_fingerprint(pk.as_bytes());

        let mut token = ConnectionToken {
            node_id: *pk.as_bytes(),
            invite_nonce: [0x55; 16],
            relay_url: Some("https://example.com".to_string()),
            instance_name: Some("roundtrip-sig".to_string()),
            inviter_fingerprint: Some(fp),
            capability: Some(3), // owner
            signature: None,
        };
        token.sign(&sk);

        // Serialize, deserialize, then verify
        let bytes = token.to_bytes();
        let decoded = ConnectionToken::from_bytes(&bytes).unwrap();
        assert!(decoded.verify_signature(pk.as_bytes()));
    }

    #[test]
    fn tampered_signature_fails() {
        let mut rng = rand::rng();
        let sk = SigningKey::generate(&mut rng);
        let pk = sk.public_key();
        let fp = ConnectionToken::compute_fingerprint(pk.as_bytes());

        let mut token = ConnectionToken {
            node_id: *pk.as_bytes(),
            invite_nonce: [0x33; 16],
            relay_url: None,
            instance_name: Some("tamper-test".to_string()),
            inviter_fingerprint: Some(fp),
            capability: Some(1),
            signature: None,
        };
        token.sign(&sk);

        // Tamper with the signature
        let mut sig = token.signature.unwrap();
        sig[0] ^= 0xff;
        token.signature = Some(sig);

        assert!(!token.verify_signature(pk.as_bytes()));
    }

    #[test]
    fn tampered_payload_fails_verification() {
        let mut rng = rand::rng();
        let sk = SigningKey::generate(&mut rng);
        let pk = sk.public_key();
        let fp = ConnectionToken::compute_fingerprint(pk.as_bytes());

        let mut token = ConnectionToken {
            node_id: *pk.as_bytes(),
            invite_nonce: [0x33; 16],
            relay_url: None,
            instance_name: Some("tamper-payload".to_string()),
            inviter_fingerprint: Some(fp),
            capability: Some(1),
            signature: None,
        };
        token.sign(&sk);

        // Tamper with the capability (payload, not signature)
        token.capability = Some(3);
        assert!(!token.verify_signature(pk.as_bytes()));
    }

    #[test]
    fn wrong_key_fails_verification() {
        let mut rng = rand::rng();
        let sk1 = SigningKey::generate(&mut rng);
        let sk2 = SigningKey::generate(&mut rng);
        let pk1 = sk1.public_key();
        let pk2 = sk2.public_key();
        let fp = ConnectionToken::compute_fingerprint(pk1.as_bytes());

        let mut token = ConnectionToken {
            node_id: *pk1.as_bytes(),
            invite_nonce: [0x44; 16],
            relay_url: None,
            instance_name: Some("wrong-key".to_string()),
            inviter_fingerprint: Some(fp),
            capability: Some(0),
            signature: None,
        };
        token.sign(&sk1);

        // Verify with the wrong public key
        assert!(!token.verify_signature(pk2.as_bytes()));
    }

    #[test]
    fn v1_verify_returns_false() {
        // A v1 token has no signature, so verify should return false.
        let token = v1_token();
        assert!(!token.verify_signature(&[0xaa; 32]));
    }

    #[test]
    fn v2_base32_roundtrip() {
        let mut rng = rand::rng();
        let sk = SigningKey::generate(&mut rng);
        let pk = sk.public_key();
        let fp = ConnectionToken::compute_fingerprint(pk.as_bytes());

        let mut token = ConnectionToken {
            node_id: *pk.as_bytes(),
            invite_nonce: [0xee; 16],
            relay_url: None,
            instance_name: Some("base32-test".to_string()),
            inviter_fingerprint: Some(fp),
            capability: Some(2),
            signature: None,
        };
        token.sign(&sk);

        let encoded = token.to_base32();
        let decoded = ConnectionToken::from_base32(&encoded).unwrap();
        assert_eq!(decoded, token);
        assert!(decoded.verify_signature(pk.as_bytes()));
    }

    #[test]
    fn v2_too_short() {
        // Version 2 but too few bytes
        let mut bytes = vec![2u8];
        bytes.extend_from_slice(&[0; 48]); // 49 total, need at least 123
        let err = ConnectionToken::from_bytes(&bytes).unwrap_err();
        assert!(err.contains("too short"));
    }

    #[test]
    fn compute_fingerprint_deterministic() {
        let key = [42u8; 32];
        let fp1 = ConnectionToken::compute_fingerprint(&key);
        let fp2 = ConnectionToken::compute_fingerprint(&key);
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn compute_fingerprint_different_keys() {
        let fp1 = ConnectionToken::compute_fingerprint(&[1u8; 32]);
        let fp2 = ConnectionToken::compute_fingerprint(&[2u8; 32]);
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn display_matches_base32_v2() {
        let mut rng = rand::rng();
        let sk = SigningKey::generate(&mut rng);
        let fp = ConnectionToken::compute_fingerprint(sk.public_key().as_bytes());

        let mut token = ConnectionToken {
            node_id: *sk.public_key().as_bytes(),
            invite_nonce: [0x42; 16],
            relay_url: None,
            instance_name: Some("display".to_string()),
            inviter_fingerprint: Some(fp),
            capability: Some(1),
            signature: None,
        };
        token.sign(&sk);

        assert_eq!(format!("{token}"), token.to_base32());
    }
}
