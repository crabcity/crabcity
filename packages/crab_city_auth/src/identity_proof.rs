//! Self-issued identity proofs linking a user's keys across instances.

use crate::error::AuthError;
use crate::keys::{PublicKey, Signature, SigningKey, verify};

/// Self-issued identity proof linking a user's keys across instances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityProof {
    pub version: u8,
    pub subject: PublicKey,
    pub instance: PublicKey,
    pub related_keys: Vec<PublicKey>,
    pub registry_handle: Option<String>,
    pub timestamp: u64,
    pub signature: Signature,
}

#[derive(Debug, Clone)]
pub struct IdentityProofClaims {
    pub subject: PublicKey,
    pub instance: PublicKey,
    pub related_keys: Vec<PublicKey>,
    pub registry_handle: Option<String>,
    pub timestamp: u64,
}

/// Parse errors for the core binary parser — `Copy`, no allocation.
/// Kani harnesses test `parse_bytes` directly, avoiding `format!()` overhead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProofParseError {
    TooShort,
    KeyCountExceedsMax(u32),
    TruncatedKeys,
    TruncatedHandleLen,
    TruncatedHandle,
    InvalidUtf8Handle,
    TruncatedTrailer,
}

impl IdentityProof {
    /// Sign a new identity proof.
    pub fn sign(
        signing_key: &SigningKey,
        instance: &PublicKey,
        related_keys: Vec<PublicKey>,
        handle: Option<String>,
        timestamp: u64,
    ) -> Self {
        let subject = signing_key.public_key();
        let msg = Self::signing_message(&subject, instance, &related_keys, &handle, timestamp);
        let signature = signing_key.sign(&msg);

        Self {
            version: 0x01,
            subject,
            instance: *instance,
            related_keys,
            registry_handle: handle,
            timestamp,
            signature,
        }
    }

    /// Verify the proof's signature.
    pub fn verify(&self) -> Result<IdentityProofClaims, AuthError> {
        if self.version != 0x01 {
            return Err(AuthError::InvalidSignature);
        }
        let msg = Self::signing_message(
            &self.subject,
            &self.instance,
            &self.related_keys,
            &self.registry_handle,
            self.timestamp,
        );
        verify(&self.subject, &msg, &self.signature)?;
        Ok(IdentityProofClaims {
            subject: self.subject,
            instance: self.instance,
            related_keys: self.related_keys.clone(),
            registry_handle: self.registry_handle.clone(),
            timestamp: self.timestamp,
        })
    }

    fn signing_message(
        subject: &PublicKey,
        instance: &PublicKey,
        related_keys: &[PublicKey],
        handle: &Option<String>,
        timestamp: u64,
    ) -> Vec<u8> {
        let mut msg = Vec::new();
        msg.push(0x01); // version
        msg.extend_from_slice(subject.as_bytes());
        msg.extend_from_slice(instance.as_bytes());
        msg.extend_from_slice(&(related_keys.len() as u32).to_be_bytes());
        for key in related_keys {
            msg.extend_from_slice(key.as_bytes());
        }
        match handle {
            Some(h) => {
                msg.push(1);
                msg.extend_from_slice(&(h.len() as u16).to_be_bytes());
                msg.extend_from_slice(h.as_bytes());
            }
            None => msg.push(0),
        }
        msg.extend_from_slice(&timestamp.to_be_bytes());
        msg
    }

    /// Compact binary encoding.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(self.version);
        buf.extend_from_slice(self.subject.as_bytes());
        buf.extend_from_slice(self.instance.as_bytes());
        buf.extend_from_slice(&(self.related_keys.len() as u32).to_be_bytes());
        for key in &self.related_keys {
            buf.extend_from_slice(key.as_bytes());
        }
        match &self.registry_handle {
            Some(h) => {
                buf.push(1);
                buf.extend_from_slice(&(h.len() as u16).to_be_bytes());
                buf.extend_from_slice(h.as_bytes());
            }
            None => buf.push(0),
        }
        buf.extend_from_slice(&self.timestamp.to_be_bytes());
        buf.extend_from_slice(self.signature.as_bytes());
        buf
    }

    /// Maximum number of related keys allowed in an identity proof.
    const MAX_RELATED_KEYS: usize = 256;

    /// Core binary parser — returns simple enum errors, no `format!()`.
    /// Kani harnesses verify this function directly.
    fn parse_bytes(bytes: &[u8]) -> Result<Self, ProofParseError> {
        if bytes.len() < 1 + 32 + 32 + 4 {
            return Err(ProofParseError::TooShort);
        }

        let mut pos = 0;
        let version = bytes[pos];
        pos += 1;

        let subject = PublicKey::from_bytes(bytes[pos..pos + 32].try_into().unwrap());
        pos += 32;

        let instance = PublicKey::from_bytes(bytes[pos..pos + 32].try_into().unwrap());
        pos += 32;

        let key_count = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap());
        pos += 4;

        if key_count as usize > Self::MAX_RELATED_KEYS {
            return Err(ProofParseError::KeyCountExceedsMax(key_count));
        }
        let key_count = key_count as usize;

        if bytes.len() < pos + key_count * 32 + 1 {
            return Err(ProofParseError::TruncatedKeys);
        }

        let mut related_keys = Vec::with_capacity(key_count);
        for _ in 0..key_count {
            related_keys.push(PublicKey::from_bytes(
                bytes[pos..pos + 32].try_into().unwrap(),
            ));
            pos += 32;
        }

        let has_handle = bytes[pos];
        pos += 1;

        let registry_handle = if has_handle == 1 {
            if bytes.len() < pos + 2 {
                return Err(ProofParseError::TruncatedHandleLen);
            }
            let handle_len = u16::from_be_bytes(bytes[pos..pos + 2].try_into().unwrap()) as usize;
            pos += 2;
            if bytes.len() < pos + handle_len {
                return Err(ProofParseError::TruncatedHandle);
            }
            let handle = std::str::from_utf8(&bytes[pos..pos + handle_len])
                .map_err(|_| ProofParseError::InvalidUtf8Handle)?
                .to_string();
            pos += handle_len;
            Some(handle)
        } else {
            None
        };

        if bytes.len() < pos + 8 + 64 {
            return Err(ProofParseError::TruncatedTrailer);
        }

        let timestamp = u64::from_be_bytes(bytes[pos..pos + 8].try_into().unwrap());
        pos += 8;

        let signature = Signature::from_bytes(bytes[pos..pos + 64].try_into().unwrap());

        Ok(Self {
            version,
            subject,
            instance,
            related_keys,
            registry_handle,
            timestamp,
            signature,
        })
    }

    /// Parse from binary.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AuthError> {
        Self::parse_bytes(bytes).map_err(|e| {
            let msg = match e {
                ProofParseError::TooShort => "too short".to_string(),
                ProofParseError::KeyCountExceedsMax(n) => {
                    format!("key count {n} exceeds maximum {}", Self::MAX_RELATED_KEYS)
                }
                ProofParseError::TruncatedKeys => "truncated keys".to_string(),
                ProofParseError::TruncatedHandleLen => "truncated handle length".to_string(),
                ProofParseError::TruncatedHandle => "truncated handle".to_string(),
                ProofParseError::InvalidUtf8Handle => "invalid utf8 in handle".to_string(),
                ProofParseError::TruncatedTrailer => "truncated timestamp/signature".to_string(),
            };
            AuthError::InvalidIdentityProof(msg)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_keypair() -> (SigningKey, PublicKey) {
        let mut rng = rand::thread_rng();
        let sk = SigningKey::generate(&mut rng);
        let pk = sk.public_key();
        (sk, pk)
    }

    #[test]
    fn sign_verify_roundtrip() {
        let (sk, _) = test_keypair();
        let instance = PublicKey::from_bytes([1u8; 32]);
        let related = vec![PublicKey::from_bytes([2u8; 32])];
        let proof = IdentityProof::sign(
            &sk,
            &instance,
            related,
            Some("alex".to_string()),
            1700000000,
        );
        let claims = proof.verify().unwrap();
        assert_eq!(claims.subject, sk.public_key());
        assert_eq!(claims.instance, instance);
        assert_eq!(claims.registry_handle, Some("alex".to_string()));
    }

    #[test]
    fn tamper_detected() {
        let (sk, _) = test_keypair();
        let instance = PublicKey::from_bytes([3u8; 32]);
        let mut proof = IdentityProof::sign(&sk, &instance, vec![], None, 1700000000);
        proof.timestamp += 1; // tamper
        assert!(proof.verify().is_err());
    }

    #[test]
    fn wrong_key_detected() {
        let (sk1, _) = test_keypair();
        let (_, pk2) = test_keypair();
        let instance = PublicKey::from_bytes([4u8; 32]);
        let mut proof = IdentityProof::sign(&sk1, &instance, vec![], None, 1700000000);
        proof.subject = pk2; // wrong key
        assert!(proof.verify().is_err());
    }

    #[test]
    fn bytes_roundtrip() {
        let (sk, _) = test_keypair();
        let instance = PublicKey::from_bytes([5u8; 32]);
        let related = vec![
            PublicKey::from_bytes([6u8; 32]),
            PublicKey::from_bytes([7u8; 32]),
        ];
        let proof = IdentityProof::sign(
            &sk,
            &instance,
            related,
            Some("test".to_string()),
            1700000000,
        );
        let bytes = proof.to_bytes();
        let parsed = IdentityProof::from_bytes(&bytes).unwrap();
        assert_eq!(proof, parsed);
    }

    #[test]
    fn bytes_roundtrip_no_handle() {
        let (sk, _) = test_keypair();
        let instance = PublicKey::from_bytes([8u8; 32]);
        let proof = IdentityProof::sign(&sk, &instance, vec![], None, 1700000000);
        let bytes = proof.to_bytes();
        let parsed = IdentityProof::from_bytes(&bytes).unwrap();
        assert_eq!(proof, parsed);
    }

    #[test]
    fn excessive_key_count_rejected() {
        // Forge a binary blob with key_count = u32::MAX
        let mut buf = Vec::new();
        buf.push(0x01); // version
        buf.extend_from_slice(&[0u8; 32]); // subject
        buf.extend_from_slice(&[0u8; 32]); // instance
        buf.extend_from_slice(&u32::MAX.to_be_bytes()); // key_count
        let result = IdentityProof::from_bytes(&buf);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("exceeds maximum"), "got: {msg}");
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// Prove: `parse_bytes` never panics on any 142-byte input.
    /// 142 = minimum valid size (version + subject + instance + key_count=0
    ///       + has_handle=0 + timestamp + signature).
    /// Covers all `try_into().unwrap()` calls and bounds checks.
    #[kani::proof]
    fn from_bytes_min_size_no_panic() {
        let bytes: [u8; 142] = kani::any();
        let _ = IdentityProof::parse_bytes(&bytes);
    }

    /// Prove: `parse_bytes` never panics on short inputs (below minimum).
    #[kani::proof]
    fn from_bytes_short_no_panic() {
        let len: usize = kani::any();
        kani::assume(len <= 69);
        let buf: [u8; 69] = kani::any();
        let _ = IdentityProof::parse_bytes(&buf[..len]);
    }

    /// Prove: `parse_bytes` never panics on input with 1 related key
    /// (142 + 32 = 174 bytes).
    #[kani::proof]
    fn from_bytes_one_key_no_panic() {
        let bytes: [u8; 174] = kani::any();
        let _ = IdentityProof::parse_bytes(&bytes);
    }

    /// Prove: any key_count > MAX_RELATED_KEYS is rejected regardless
    /// of other input content.
    ///
    /// Buffer is concrete zeros — only key_count is symbolic, since the
    /// other 65 bytes are irrelevant to the bounds check.
    #[kani::proof]
    fn key_count_bound_enforced() {
        let mut buf = [0u8; 69];
        let key_count: u32 = kani::any();
        kani::assume(key_count as usize > IdentityProof::MAX_RELATED_KEYS);
        buf[65..69].copy_from_slice(&key_count.to_be_bytes());
        let result = IdentityProof::parse_bytes(&buf);
        assert!(result.is_err());
    }
}
