//! Invite tokens: creation, delegation chains, binary encoding, and verification.

use sha2::{Digest, Sha256};

use crate::capability::Capability;
use crate::encoding::{crockford_decode, crockford_encode};
use crate::error::AuthError;
use crate::keys::{PublicKey, Signature, SigningKey, verify};

/// A single link in an invite delegation chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteLink {
    pub issuer: PublicKey,
    pub capability: Capability,
    pub max_depth: u8,
    pub max_uses: u32,
    pub expires_at: Option<u64>,
    pub nonce: [u8; 16],
    pub signature: Signature,
}

/// Per-link binary size: 32 + 1 + 1 + 4 + 8 + 16 + 64 = 126 bytes.
const LINK_SIZE: usize = 126;

/// Maximum delegation chain depth. Rejects pathologically deep chains from
/// untrusted input (the byte is a u8, so a malicious sender could claim 255).
pub const MAX_CHAIN_DEPTH: usize = 16;

/// Parse errors for invite binary decoding — `Copy`, no allocation.
/// Kani harnesses test core parse functions directly, avoiding `format!()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InviteParseError {
    TooShort,
    EmptyChain,
    ChainTooDeep(u8),
    WrongSize { expected: usize, actual: usize },
    WrongLinkSize(usize),
    UnknownCapability(u8),
}

impl InviteLink {
    /// SHA-256 of the link's fields (excluding signature — the signature covers
    /// this hash via the signing message).
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.issuer.as_bytes());
        hasher.update([capability_to_byte(self.capability)]);
        hasher.update([self.max_depth]);
        hasher.update(self.max_uses.to_be_bytes());
        hasher.update(self.expires_at.unwrap_or(0).to_be_bytes());
        hasher.update(self.nonce);
        hasher.finalize().into()
    }

    /// Build the message that gets signed: H(prev_link) ++ instance ++ fields.
    fn signing_message(
        prev_link_hash: &[u8; 32],
        instance: &PublicKey,
        capability: Capability,
        max_depth: u8,
        max_uses: u32,
        expires_at: Option<u64>,
        nonce: &[u8; 16],
    ) -> Vec<u8> {
        let mut msg = Vec::with_capacity(32 + 32 + 1 + 1 + 4 + 8 + 16);
        msg.extend_from_slice(prev_link_hash);
        msg.extend_from_slice(instance.as_bytes());
        msg.push(capability_to_byte(capability));
        msg.push(max_depth);
        msg.extend_from_slice(&max_uses.to_be_bytes());
        msg.extend_from_slice(&expires_at.unwrap_or(0).to_be_bytes());
        msg.extend_from_slice(nonce);
        msg
    }

    /// Create and sign a new link.
    pub fn sign<R: rand::CryptoRng + rand::RngCore>(
        signing_key: &SigningKey,
        prev_link_hash: &[u8; 32],
        instance: &PublicKey,
        capability: Capability,
        max_depth: u8,
        max_uses: u32,
        expires_at: Option<u64>,
        rng: &mut R,
    ) -> Self {
        let mut nonce = [0u8; 16];
        rng.fill_bytes(&mut nonce);

        let msg = Self::signing_message(
            prev_link_hash,
            instance,
            capability,
            max_depth,
            max_uses,
            expires_at,
            &nonce,
        );
        let signature = signing_key.sign(&msg);

        Self {
            issuer: signing_key.public_key(),
            capability,
            max_depth,
            max_uses,
            expires_at,
            nonce,
            signature,
        }
    }

    /// Verify this link's signature.
    fn verify_signature(
        &self,
        prev_link_hash: &[u8; 32],
        instance: &PublicKey,
    ) -> Result<(), AuthError> {
        let msg = Self::signing_message(
            prev_link_hash,
            instance,
            self.capability,
            self.max_depth,
            self.max_uses,
            self.expires_at,
            &self.nonce,
        );
        verify(&self.issuer, &msg, &self.signature)
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(LINK_SIZE);
        buf.extend_from_slice(self.issuer.as_bytes());
        buf.push(capability_to_byte(self.capability));
        buf.push(self.max_depth);
        buf.extend_from_slice(&self.max_uses.to_be_bytes());
        buf.extend_from_slice(&self.expires_at.unwrap_or(0).to_be_bytes());
        buf.extend_from_slice(&self.nonce);
        buf.extend_from_slice(self.signature.as_bytes());
        buf
    }

    /// Core link parser — returns simple enum errors, no `format!()`.
    /// Kani harnesses verify this function directly.
    fn parse_link(bytes: &[u8]) -> Result<Self, InviteParseError> {
        if bytes.len() != LINK_SIZE {
            return Err(InviteParseError::WrongLinkSize(bytes.len()));
        }
        let issuer = PublicKey::from_bytes(bytes[0..32].try_into().unwrap());
        let capability = match bytes[32] {
            0 => Capability::View,
            1 => Capability::Collaborate,
            2 => Capability::Admin,
            3 => Capability::Owner,
            b => return Err(InviteParseError::UnknownCapability(b)),
        };
        let max_depth = bytes[33];
        let max_uses = u32::from_be_bytes(bytes[34..38].try_into().unwrap());
        let expires_raw = u64::from_be_bytes(bytes[38..46].try_into().unwrap());
        let expires_at = if expires_raw == 0 {
            None
        } else {
            Some(expires_raw)
        };
        let nonce: [u8; 16] = bytes[46..62].try_into().unwrap();
        let signature = Signature::from_bytes(bytes[62..126].try_into().unwrap());

        Ok(Self {
            issuer,
            capability,
            max_depth,
            max_uses,
            expires_at,
            nonce,
            signature,
        })
    }
}

/// A complete invite token: version + instance + chain of links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub version: u8,
    pub instance: PublicKey,
    pub links: Vec<InviteLink>,
}

/// What you get back from successful verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteClaims {
    pub instance: PublicKey,
    pub capability: Capability,
    pub root_issuer: PublicKey,
    pub leaf_issuer: PublicKey,
    pub chain_depth: usize,
    pub nonce: [u8; 16],
}

/// Root link signs H(0x00*32) as the "previous link hash".
const GENESIS_PREV_HASH: [u8; 32] = [0u8; 32];

impl Invite {
    /// Create a flat (non-delegated) invite: chain of length 1, max_depth=0.
    pub fn create_flat<R: rand::CryptoRng + rand::RngCore>(
        signing_key: &SigningKey,
        instance: &PublicKey,
        capability: Capability,
        max_uses: u32,
        expires_at: Option<u64>,
        rng: &mut R,
    ) -> Self {
        let link = InviteLink::sign(
            signing_key,
            &GENESIS_PREV_HASH,
            instance,
            capability,
            0, // flat invite cannot be delegated
            max_uses,
            expires_at,
            rng,
        );
        Self {
            version: 0x01,
            instance: *instance,
            links: vec![link],
        }
    }

    /// Delegate: append a new link to the chain.
    pub fn delegate<R: rand::CryptoRng + rand::RngCore>(
        parent: &Invite,
        signing_key: &SigningKey,
        capability: Capability,
        max_uses: u32,
        expires_at: Option<u64>,
        rng: &mut R,
    ) -> Result<Self, AuthError> {
        let leaf = parent
            .links
            .last()
            .ok_or_else(|| AuthError::InvalidInvite("empty chain".to_string()))?;

        if leaf.max_depth == 0 {
            return Err(AuthError::InvalidInvite(
                "cannot delegate: max_depth is 0".to_string(),
            ));
        }
        if capability > leaf.capability {
            return Err(AuthError::InvalidInvite(
                "cannot escalate capability beyond parent".into(),
            ));
        }

        let prev_hash = leaf.hash();
        let new_depth = leaf.max_depth - 1;

        let link = InviteLink::sign(
            signing_key,
            &prev_hash,
            &parent.instance,
            capability,
            new_depth,
            max_uses,
            expires_at,
            rng,
        );

        let mut links = parent.links.clone();
        links.push(link);

        Ok(Self {
            version: parent.version,
            instance: parent.instance,
            links,
        })
    }

    /// Verify the entire invite chain. Does NOT check use counts or issuer
    /// grant status — that's the instance's job at redemption time.
    pub fn verify(&self) -> Result<InviteClaims, AuthError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.verify_at(now)
    }

    /// Verify the invite chain against a caller-supplied timestamp (unix secs).
    /// Useful for testing expiry without mocking `SystemTime`.
    pub fn verify_at(&self, now_unix_secs: u64) -> Result<InviteClaims, AuthError> {
        if self.version != 0x01 {
            return Err(AuthError::InvalidInvite("unsupported version".into()));
        }
        if self.links.is_empty() {
            return Err(AuthError::InvalidInvite("empty chain".to_string()));
        }

        let mut prev_hash = GENESIS_PREV_HASH;
        let mut prev_capability = None;
        let mut prev_max_depth = None;

        for (i, link) in self.links.iter().enumerate() {
            // Verify signature
            link.verify_signature(&prev_hash, &self.instance)?;

            // Verify capability narrowing
            if let Some(prev_cap) = prev_capability {
                if link.capability > prev_cap {
                    return Err(AuthError::InvalidInvite(format!(
                        "capability escalation at link {i}: {} > {prev_cap}",
                        link.capability
                    )));
                }
            }

            // Verify depth constraints
            if let Some(prev_depth) = prev_max_depth {
                if prev_depth == 0 {
                    return Err(AuthError::InvalidInvite(format!(
                        "depth exhausted at link {i}"
                    )));
                }
                if link.max_depth >= prev_depth {
                    return Err(AuthError::InvalidInvite(format!(
                        "depth must decrease at link {i}: {} >= {prev_depth}",
                        link.max_depth
                    )));
                }
            }

            // Check expiry
            if let Some(expires) = link.expires_at {
                if now_unix_secs > expires {
                    return Err(AuthError::InvalidInvite(format!("link {i} expired")));
                }
            }

            prev_hash = link.hash();
            prev_capability = Some(link.capability);
            prev_max_depth = Some(link.max_depth);
        }

        let root = &self.links[0];
        let leaf = self.links.last().unwrap();

        Ok(InviteClaims {
            instance: self.instance,
            capability: leaf.capability,
            root_issuer: root.issuer,
            leaf_issuer: leaf.issuer,
            chain_depth: self.links.len(),
            nonce: leaf.nonce,
        })
    }

    /// Compact binary encoding.
    pub fn to_bytes(&self) -> Vec<u8> {
        // version(1) + instance(32) + chain_length(1) + links(N * LINK_SIZE)
        let mut buf = Vec::with_capacity(1 + 32 + 1 + self.links.len() * LINK_SIZE);
        buf.push(self.version);
        buf.extend_from_slice(self.instance.as_bytes());
        buf.push(self.links.len() as u8);
        for link in &self.links {
            buf.extend_from_slice(&link.to_bytes());
        }
        buf
    }

    /// Core invite parser — returns simple enum errors, no `format!()`.
    /// Kani harnesses verify this function directly.
    fn parse_invite(bytes: &[u8]) -> Result<Self, InviteParseError> {
        if bytes.len() < 34 {
            return Err(InviteParseError::TooShort);
        }
        let version = bytes[0];
        let instance = PublicKey::from_bytes(bytes[1..33].try_into().unwrap());
        let chain_length = bytes[33];

        if chain_length == 0 {
            return Err(InviteParseError::EmptyChain);
        }
        if chain_length as usize > MAX_CHAIN_DEPTH {
            return Err(InviteParseError::ChainTooDeep(chain_length));
        }

        let expected = 34 + chain_length as usize * LINK_SIZE;
        if bytes.len() != expected {
            return Err(InviteParseError::WrongSize {
                expected,
                actual: bytes.len(),
            });
        }

        let mut links = Vec::with_capacity(chain_length as usize);
        for i in 0..chain_length as usize {
            let offset = 34 + i * LINK_SIZE;
            let link = InviteLink::parse_link(&bytes[offset..offset + LINK_SIZE])?;
            links.push(link);
        }

        Ok(Self {
            version,
            instance,
            links,
        })
    }

    /// Parse from binary.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AuthError> {
        Self::parse_invite(bytes).map_err(|e| {
            let msg = match e {
                InviteParseError::TooShort => "too short".to_string(),
                InviteParseError::EmptyChain => "empty chain".to_string(),
                InviteParseError::ChainTooDeep(n) => {
                    format!("chain length {n} exceeds maximum {MAX_CHAIN_DEPTH}")
                }
                InviteParseError::WrongSize { expected, actual } => {
                    format!("wrong size: expected {expected}, got {actual}")
                }
                InviteParseError::WrongLinkSize(n) => {
                    format!("link size {n}, expected {LINK_SIZE}")
                }
                InviteParseError::UnknownCapability(b) => {
                    format!("unknown capability byte: {b}")
                }
            };
            AuthError::InvalidInvite(msg)
        })
    }

    /// Crockford base32 encoding.
    pub fn to_base32(&self) -> String {
        crockford_encode(&self.to_bytes())
    }

    /// Parse from Crockford base32.
    pub fn from_base32(s: &str) -> Result<Self, AuthError> {
        let bytes =
            crockford_decode(s).map_err(|_| AuthError::InvalidInvite("invalid base32".into()))?;
        Self::from_bytes(&bytes)
    }
}

// --- Capability byte encoding ---

fn capability_to_byte(c: Capability) -> u8 {
    match c {
        Capability::View => 0,
        Capability::Collaborate => 1,
        Capability::Admin => 2,
        Capability::Owner => 3,
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
    fn flat_invite_roundtrip_bytes() {
        let mut rng = rand::thread_rng();
        let (sk, _) = test_keypair();
        let instance = PublicKey::from_bytes([1u8; 32]);
        let invite =
            Invite::create_flat(&sk, &instance, Capability::Collaborate, 5, None, &mut rng);

        let bytes = invite.to_bytes();
        let parsed = Invite::from_bytes(&bytes).unwrap();
        assert_eq!(invite, parsed);
    }

    #[test]
    fn flat_invite_roundtrip_base32() {
        let mut rng = rand::thread_rng();
        let (sk, _) = test_keypair();
        let instance = PublicKey::from_bytes([2u8; 32]);
        let invite = Invite::create_flat(&sk, &instance, Capability::Admin, 0, None, &mut rng);

        let b32 = invite.to_base32();
        let parsed = Invite::from_base32(&b32).unwrap();
        assert_eq!(invite, parsed);
    }

    #[test]
    fn flat_invite_size() {
        let mut rng = rand::thread_rng();
        let (sk, _) = test_keypair();
        let instance = PublicKey::from_bytes([3u8; 32]);
        let invite = Invite::create_flat(&sk, &instance, Capability::View, 0, None, &mut rng);

        // version(1) + instance(32) + chain_length(1) + 1 link(126) = 160
        assert_eq!(invite.to_bytes().len(), 160);
    }

    #[test]
    fn flat_invite_verify() {
        let mut rng = rand::thread_rng();
        let (sk, _) = test_keypair();
        let instance = PublicKey::from_bytes([4u8; 32]);
        let invite =
            Invite::create_flat(&sk, &instance, Capability::Collaborate, 5, None, &mut rng);

        let claims = invite.verify().unwrap();
        assert_eq!(claims.instance, instance);
        assert_eq!(claims.capability, Capability::Collaborate);
        assert_eq!(claims.chain_depth, 1);
    }

    #[test]
    fn delegated_chain_3_hops() {
        let mut rng = rand::thread_rng();
        let (sk1, _) = test_keypair();
        let (sk2, _) = test_keypair();
        let (sk3, _) = test_keypair();
        let instance = PublicKey::from_bytes([5u8; 32]);

        // Root: admin, max_depth=2
        let root_link = InviteLink::sign(
            &sk1,
            &GENESIS_PREV_HASH,
            &instance,
            Capability::Admin,
            2,
            0,
            None,
            &mut rng,
        );
        let root = Invite {
            version: 0x01,
            instance,
            links: vec![root_link],
        };

        // Delegate to sk2: collaborate, depth=1
        let hop2 =
            Invite::delegate(&root, &sk2, Capability::Collaborate, 0, None, &mut rng).unwrap();
        assert_eq!(hop2.links.len(), 2);

        // Delegate to sk3: view, depth=0
        let hop3 = Invite::delegate(&hop2, &sk3, Capability::View, 0, None, &mut rng).unwrap();
        assert_eq!(hop3.links.len(), 3);

        // Verify the full chain
        let claims = hop3.verify().unwrap();
        assert_eq!(claims.capability, Capability::View);
        assert_eq!(claims.chain_depth, 3);
        assert_eq!(claims.root_issuer, sk1.public_key());
        assert_eq!(claims.leaf_issuer, sk3.public_key());

        // Size check: 1 + 32 + 1 + (126*3) = 412
        assert_eq!(hop3.to_bytes().len(), 412);
    }

    #[test]
    fn capability_escalation_rejected() {
        let mut rng = rand::thread_rng();
        let (sk1, _) = test_keypair();
        let (sk2, _) = test_keypair();
        let instance = PublicKey::from_bytes([6u8; 32]);

        let root_link = InviteLink::sign(
            &sk1,
            &GENESIS_PREV_HASH,
            &instance,
            Capability::Collaborate,
            1,
            0,
            None,
            &mut rng,
        );
        let root = Invite {
            version: 0x01,
            instance,
            links: vec![root_link],
        };

        // Try to escalate to admin
        let result = Invite::delegate(&root, &sk2, Capability::Admin, 0, None, &mut rng);
        assert!(result.is_err());
    }

    #[test]
    fn depth_exhausted_rejected() {
        let mut rng = rand::thread_rng();
        let (sk1, _) = test_keypair();
        let (sk2, _) = test_keypair();
        let instance = PublicKey::from_bytes([7u8; 32]);

        // Flat invite has max_depth=0
        let invite = Invite::create_flat(&sk1, &instance, Capability::Admin, 0, None, &mut rng);

        let result = Invite::delegate(&invite, &sk2, Capability::View, 0, None, &mut rng);
        assert!(result.is_err());
    }

    #[test]
    fn forgery_detected() {
        let mut rng = rand::thread_rng();
        let (sk, _) = test_keypair();
        let instance = PublicKey::from_bytes([8u8; 32]);
        let invite =
            Invite::create_flat(&sk, &instance, Capability::Collaborate, 5, None, &mut rng);

        let mut bytes = invite.to_bytes();
        // Tamper with the nonce (byte 80 is in the link's nonce area)
        bytes[80] ^= 0xff;
        let tampered = Invite::from_bytes(&bytes).unwrap();
        assert!(tampered.verify().is_err());
    }

    #[test]
    fn delegated_roundtrip_bytes() {
        let mut rng = rand::thread_rng();
        let (sk1, _) = test_keypair();
        let (sk2, _) = test_keypair();
        let instance = PublicKey::from_bytes([9u8; 32]);

        let root_link = InviteLink::sign(
            &sk1,
            &GENESIS_PREV_HASH,
            &instance,
            Capability::Admin,
            1,
            0,
            None,
            &mut rng,
        );
        let root = Invite {
            version: 0x01,
            instance,
            links: vec![root_link],
        };
        let delegated = Invite::delegate(&root, &sk2, Capability::View, 0, None, &mut rng).unwrap();

        let bytes = delegated.to_bytes();
        let parsed = Invite::from_bytes(&bytes).unwrap();
        assert_eq!(delegated, parsed);
    }

    #[test]
    fn verify_at_expired() {
        let mut rng = rand::thread_rng();
        let (sk, _) = test_keypair();
        let instance = PublicKey::from_bytes([10u8; 32]);
        // Expires at unix timestamp 1000
        let invite = Invite::create_flat(&sk, &instance, Capability::View, 1, Some(1000), &mut rng);

        // Before expiry — should pass
        assert!(invite.verify_at(999).is_ok());
        // At expiry — should pass (not strictly after)
        assert!(invite.verify_at(1000).is_ok());
        // After expiry — should fail
        assert!(invite.verify_at(1001).is_err());
    }

    #[test]
    fn verify_at_no_expiry() {
        let mut rng = rand::thread_rng();
        let (sk, _) = test_keypair();
        let instance = PublicKey::from_bytes([11u8; 32]);
        let invite = Invite::create_flat(&sk, &instance, Capability::View, 1, None, &mut rng);

        // Far future — should always pass with no expiry
        assert!(invite.verify_at(u64::MAX).is_ok());
    }

    #[test]
    fn chain_depth_bound_rejects_deep_chain() {
        // Forge a binary blob that claims chain_length=255
        let mut buf = vec![0x01]; // version
        buf.extend_from_slice(&[0u8; 32]); // instance
        buf.push(255); // chain_length = 255
        // Don't bother with link data — the bound check fires first
        let result = Invite::from_bytes(&buf);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("exceeds maximum"), "got: {msg}");
    }
}

#[cfg(kani)]
mod proofs {
    use super::*;

    /// Prove: `parse_link` never panics on any 126-byte input.
    /// Covers all `try_into().unwrap()` calls — Kani verifies the slice sizes
    /// are always correct after the length guard.
    #[kani::proof]
    fn link_from_bytes_no_panic() {
        let bytes: [u8; LINK_SIZE] = kani::any();
        let _ = InviteLink::parse_link(&bytes);
    }

    /// Prove: `parse_link` never panics on wrong-sized input.
    #[kani::proof]
    fn link_from_bytes_wrong_size_no_panic() {
        let len: usize = kani::any();
        kani::assume(len <= 200 && len != LINK_SIZE);
        let buf: [u8; 200] = kani::any();
        let _ = InviteLink::parse_link(&buf[..len]);
    }

    /// Prove: `parse_invite` never panics on any 160-byte input
    /// (the size of a flat invite: 34-byte header + one 126-byte link).
    #[kani::proof]
    #[kani::unwind(3)]
    fn invite_from_bytes_flat_no_panic() {
        let bytes: [u8; 160] = kani::any();
        let _ = Invite::parse_invite(&bytes);
    }

    /// Prove: `parse_invite` never panics on short inputs.
    #[kani::proof]
    fn invite_from_bytes_short_no_panic() {
        let len: usize = kani::any();
        kani::assume(len <= 34);
        let buf: [u8; 34] = kani::any();
        let _ = Invite::parse_invite(&buf[..len]);
    }

    /// Prove: any chain_length > MAX_CHAIN_DEPTH is rejected, regardless
    /// of the rest of the input.
    #[kani::proof]
    fn chain_depth_bound_enforced() {
        let mut header: [u8; 34] = kani::any();
        let chain_length: u8 = kani::any();
        kani::assume(chain_length as usize > MAX_CHAIN_DEPTH);
        header[33] = chain_length;
        let result = Invite::parse_invite(&header);
        assert!(result.is_err());
    }
}
