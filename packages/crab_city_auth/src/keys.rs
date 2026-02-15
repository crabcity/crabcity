//! Ed25519 key types, signatures, and standalone verification.

use std::fmt;
use std::hash::{Hash, Hasher};

use ed25519_dalek::Verifier;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::encoding::{base64_decode, base64_encode, crockford_encode};
use crate::error::AuthError;

// --- PublicKey ---

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    /// The loopback sentinel: 32 zero bytes. Used for local CLI/TUI connections.
    pub const LOOPBACK: PublicKey = PublicKey([0u8; 32]);

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn is_loopback(&self) -> bool {
        self.0 == [0u8; 32]
    }

    /// `crab_` + first 8 chars of Crockford base32 of the 32-byte key.
    pub fn fingerprint(&self) -> String {
        // data_encoding's BASE32 uses RFC 4648 (A-Z2-7). Crockford uses 0-9A-V.
        // We use a custom Crockford encoding.
        let encoded = crockford_encode(&self.0);
        format!("crab_{}", &encoded[..8])
    }
}

impl Hash for PublicKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // URL-safe base64, unpadded
        let encoded = base64_encode(&self.0);
        write!(f, "{encoded}")
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.fingerprint())
    }
}

impl Serialize for PublicKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let encoded = base64_encode(&self.0);
        serializer.serialize_str(&encoded)
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        let bytes = base64_decode(&s).map_err(serde::de::Error::custom)?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("public key must be 32 bytes"))?;
        Ok(PublicKey(arr))
    }
}

// --- SigningKey ---

#[derive(Clone)]
pub struct SigningKey(ed25519_dalek::SigningKey);

impl SigningKey {
    pub fn generate<R: rand::CryptoRng + rand::RngCore>(rng: &mut R) -> Self {
        Self(ed25519_dalek::SigningKey::generate(rng))
    }

    /// Reconstruct from raw 32-byte seed.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(ed25519_dalek::SigningKey::from_bytes(&bytes))
    }

    /// Raw 32-byte seed (suitable for persistent storage).
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.0.verifying_key().to_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> Signature {
        use ed25519_dalek::Signer;
        Signature(self.0.sign(message).to_bytes())
    }
}

// --- Signature ---

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Signature([u8; 64]);

impl Signature {
    pub fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Signature({}...)", &base64_encode(&self.0[..8]))
    }
}

impl Serialize for Signature {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let encoded = base64_encode(&self.0);
        serializer.serialize_str(&encoded)
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        let bytes = base64_decode(&s).map_err(serde::de::Error::custom)?;
        let arr: [u8; 64] = bytes
            .try_into()
            .map_err(|_| serde::de::Error::custom("signature must be 64 bytes"))?;
        Ok(Signature(arr))
    }
}

// --- Standalone verify ---

pub fn verify(
    public_key: &PublicKey,
    message: &[u8],
    signature: &Signature,
) -> Result<(), AuthError> {
    let vk = ed25519_dalek::VerifyingKey::from_bytes(public_key.as_bytes())
        .map_err(|_| AuthError::InvalidSignature)?;
    let sig = ed25519_dalek::Signature::from_bytes(signature.as_bytes());
    vk.verify(message, &sig)
        .map_err(|_| AuthError::InvalidSignature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_roundtrip() {
        let mut rng = rand::rng();
        let sk = SigningKey::generate(&mut rng);
        let pk = sk.public_key();
        let msg = b"hello crab city";
        let sig = sk.sign(msg);
        assert!(verify(&pk, msg, &sig).is_ok());
    }

    #[test]
    fn verify_wrong_key_fails() {
        let mut rng = rand::rng();
        let sk1 = SigningKey::generate(&mut rng);
        let sk2 = SigningKey::generate(&mut rng);
        let msg = b"hello";
        let sig = sk1.sign(msg);
        assert!(verify(&sk2.public_key(), msg, &sig).is_err());
    }

    #[test]
    fn verify_tampered_message_fails() {
        let mut rng = rand::rng();
        let sk = SigningKey::generate(&mut rng);
        let pk = sk.public_key();
        let sig = sk.sign(b"original");
        assert!(verify(&pk, b"tampered", &sig).is_err());
    }

    #[test]
    fn fingerprint_format() {
        let mut rng = rand::rng();
        let sk = SigningKey::generate(&mut rng);
        let fp = sk.public_key().fingerprint();
        assert!(
            fp.starts_with("crab_"),
            "fingerprint should start with crab_: {fp}"
        );
        assert_eq!(fp.len(), 13, "fingerprint should be 13 chars: {fp}");
    }

    #[test]
    fn fingerprint_deterministic() {
        let pk = PublicKey::from_bytes([42u8; 32]);
        let fp1 = pk.fingerprint();
        let fp2 = pk.fingerprint();
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn loopback_fingerprint_stable() {
        let fp = PublicKey::LOOPBACK.fingerprint();
        assert!(fp.starts_with("crab_"));
        // Should always be the same for all-zeros key
        let fp2 = PublicKey::LOOPBACK.fingerprint();
        assert_eq!(fp, fp2);
    }

    #[test]
    fn loopback_detection() {
        assert!(PublicKey::LOOPBACK.is_loopback());
        assert!(!PublicKey::from_bytes([1u8; 32]).is_loopback());
    }

    #[test]
    fn serde_roundtrip() {
        let pk = PublicKey::from_bytes([7u8; 32]);
        let json = serde_json::to_string(&pk).unwrap();
        let pk2: PublicKey = serde_json::from_str(&json).unwrap();
        assert_eq!(pk, pk2);
    }

    #[test]
    fn signing_key_bytes_roundtrip() {
        let mut rng = rand::rng();
        let sk = SigningKey::generate(&mut rng);
        let bytes = sk.to_bytes();
        let sk2 = SigningKey::from_bytes(bytes);
        assert_eq!(sk.public_key(), sk2.public_key());
        // Signs the same
        let msg = b"roundtrip test";
        let sig = sk.sign(msg);
        assert!(verify(&sk2.public_key(), msg, &sig).is_ok());
    }

    #[test]
    fn signature_serde_roundtrip() {
        let mut rng = rand::rng();
        let sk = SigningKey::generate(&mut rng);
        let sig = sk.sign(b"test");
        let json = serde_json::to_string(&sig).unwrap();
        let sig2: Signature = serde_json::from_str(&json).unwrap();
        assert_eq!(sig, sig2);
    }
}
