//! Hash-chained event log and signed checkpoints.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::AuthError;
use crate::keys::{PublicKey, Signature, SigningKey, verify};

// --- EventType ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    MemberJoined,
    MemberSuspended,
    MemberReinstated,
    MemberRemoved,
    MemberReplaced,
    GrantCapabilityChanged,
    GrantAccessChanged,
    InviteCreated,
    InviteRedeemed,
    InviteRevoked,
    InviteNounCreated,
    InviteNounResolved,
    IdentityUpdated,
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::MemberJoined => "member.joined",
            Self::MemberSuspended => "member.suspended",
            Self::MemberReinstated => "member.reinstated",
            Self::MemberRemoved => "member.removed",
            Self::MemberReplaced => "member.replaced",
            Self::GrantCapabilityChanged => "grant.capability_changed",
            Self::GrantAccessChanged => "grant.access_changed",
            Self::InviteCreated => "invite.created",
            Self::InviteRedeemed => "invite.redeemed",
            Self::InviteRevoked => "invite.revoked",
            Self::InviteNounCreated => "invite.noun_created",
            Self::InviteNounResolved => "invite.noun_resolved",
            Self::IdentityUpdated => "identity.updated",
        };
        write!(f, "{s}")
    }
}

impl FromStr for EventType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "member.joined" => Ok(Self::MemberJoined),
            "member.suspended" => Ok(Self::MemberSuspended),
            "member.reinstated" => Ok(Self::MemberReinstated),
            "member.removed" => Ok(Self::MemberRemoved),
            "member.replaced" => Ok(Self::MemberReplaced),
            "grant.capability_changed" => Ok(Self::GrantCapabilityChanged),
            "grant.access_changed" => Ok(Self::GrantAccessChanged),
            "invite.created" => Ok(Self::InviteCreated),
            "invite.redeemed" => Ok(Self::InviteRedeemed),
            "invite.revoked" => Ok(Self::InviteRevoked),
            "invite.noun_created" => Ok(Self::InviteNounCreated),
            "invite.noun_resolved" => Ok(Self::InviteNounResolved),
            "identity.updated" => Ok(Self::IdentityUpdated),
            _ => Err(format!("unknown event type: {s}")),
        }
    }
}

// --- Event ---

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: u64,
    pub prev_hash: [u8; 32],
    pub event_type: EventType,
    pub actor: Option<PublicKey>,
    pub target: Option<PublicKey>,
    pub payload: serde_json::Value,
    pub created_at: String,
    pub hash: [u8; 32],
}

impl Event {
    /// Build a new event, computing the hash automatically.
    pub fn new(
        id: u64,
        prev_hash: [u8; 32],
        event_type: EventType,
        actor: Option<PublicKey>,
        target: Option<PublicKey>,
        payload: serde_json::Value,
        created_at: String,
    ) -> Self {
        let hash = compute_hash(
            id,
            &prev_hash,
            &event_type,
            &actor,
            &target,
            &payload,
            &created_at,
        );
        Self {
            id,
            prev_hash,
            event_type,
            actor,
            target,
            payload,
            created_at,
            hash,
        }
    }

    /// The prev_hash for the very first event in an instance's log.
    pub fn genesis_prev_hash(instance_node_id: &PublicKey) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(instance_node_id.as_bytes());
        hasher.finalize().into()
    }

    /// Recompute this event's hash and check it matches.
    pub fn verify_hash(&self) -> bool {
        let expected = compute_hash(
            self.id,
            &self.prev_hash,
            &self.event_type,
            &self.actor,
            &self.target,
            &self.payload,
            &self.created_at,
        );
        self.hash == expected
    }
}

fn compute_hash(
    id: u64,
    prev_hash: &[u8; 32],
    event_type: &EventType,
    actor: &Option<PublicKey>,
    target: &Option<PublicKey>,
    payload: &serde_json::Value,
    created_at: &str,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(id.to_be_bytes());
    hasher.update(prev_hash);
    hasher.update(event_type.to_string().as_bytes());
    match actor {
        Some(pk) => {
            hasher.update([1u8]);
            hasher.update(pk.as_bytes());
        }
        None => hasher.update([0u8]),
    }
    match target {
        Some(pk) => {
            hasher.update([1u8]);
            hasher.update(pk.as_bytes());
        }
        None => hasher.update([0u8]),
    }
    // Canonicalize payload before hashing: sort all object keys recursively.
    // serde_json::Value may use IndexMap (insertion-ordered) when the
    // `preserve_order` feature is enabled (e.g. via Bazel feature unification),
    // so we cannot rely on to_vec producing sorted output.
    let canonical = canonicalize_json(payload);
    let payload_bytes = serde_json::to_vec(&canonical).expect("event payload must be serializable");
    hasher.update(&payload_bytes);
    hasher.update(created_at.as_bytes());
    hasher.finalize().into()
}

/// Recursively sort all object keys in a JSON value so that serialization
/// is deterministic regardless of `serde_json`'s `preserve_order` feature.
fn canonicalize_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut sorted: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            sorted.sort_by_key(|(k, _)| *k);
            let canonical_map: serde_json::Map<String, serde_json::Value> = sorted
                .into_iter()
                .map(|(k, v)| (k.clone(), canonicalize_json(v)))
                .collect();
            serde_json::Value::Object(canonical_map)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(canonicalize_json).collect())
        }
        other => other.clone(),
    }
}

// --- Chain verification ---

#[derive(Debug, Clone, thiserror::Error)]
pub enum ChainError {
    #[error("event {id}: hash mismatch (computed != stored)")]
    HashMismatch { id: u64 },

    #[error("event {id}: prev_hash does not match previous event's hash")]
    BrokenLink { id: u64 },
}

/// Verify a sequence of events forms a valid hash chain.
///
/// The caller must provide the expected `genesis_prev_hash` (typically
/// `Event::genesis_prev_hash(instance_id)`) to anchor the chain.
pub fn verify_chain(events: &[Event], genesis_prev_hash: &[u8; 32]) -> Result<(), ChainError> {
    let mut expected_prev = *genesis_prev_hash;

    for event in events {
        if event.prev_hash != expected_prev {
            return Err(ChainError::BrokenLink { id: event.id });
        }
        if !event.verify_hash() {
            return Err(ChainError::HashMismatch { id: event.id });
        }
        expected_prev = event.hash;
    }

    Ok(())
}

// --- EventCheckpoint ---

/// A signed attestation that the chain head at a given event_id has a specific hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventCheckpoint {
    pub event_id: u64,
    pub chain_head_hash: [u8; 32],
    pub signature: Signature,
    pub created_at: String,
}

impl EventCheckpoint {
    pub fn sign(
        signing_key: &SigningKey,
        event_id: u64,
        chain_head_hash: [u8; 32],
        created_at: String,
    ) -> Self {
        let msg = Self::signing_message(event_id, &chain_head_hash, &created_at);
        let signature = signing_key.sign(&msg);
        Self {
            event_id,
            chain_head_hash,
            signature,
            created_at,
        }
    }

    pub fn verify(&self, verifying_key: &PublicKey) -> Result<(), AuthError> {
        let msg = Self::signing_message(self.event_id, &self.chain_head_hash, &self.created_at);
        verify(verifying_key, &msg, &self.signature)
    }

    fn signing_message(event_id: u64, chain_head_hash: &[u8; 32], created_at: &str) -> Vec<u8> {
        let mut msg = Vec::new();
        msg.extend_from_slice(b"crab_city_checkpoint_v1:");
        msg.extend_from_slice(&event_id.to_be_bytes());
        msg.extend_from_slice(chain_head_hash);
        msg.extend_from_slice(created_at.as_bytes());
        msg
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

    fn make_chain(instance: &PublicKey, n: usize) -> Vec<Event> {
        let (sk, _) = test_keypair();
        let actor = sk.public_key();
        let mut events = Vec::new();
        let mut prev = Event::genesis_prev_hash(instance);

        for i in 0..n {
            let event = Event::new(
                i as u64,
                prev,
                EventType::MemberJoined,
                Some(actor),
                None,
                serde_json::json!({"seq": i}),
                format!("2025-01-01T00:00:0{i}Z"),
            );
            prev = event.hash;
            events.push(event);
        }
        events
    }

    // --- Basic construction ---

    #[test]
    fn event_hash_deterministic() {
        let instance = PublicKey::from_bytes([1u8; 32]);
        let prev = Event::genesis_prev_hash(&instance);
        let e1 = Event::new(
            0,
            prev,
            EventType::MemberJoined,
            None,
            None,
            serde_json::json!({}),
            "t".to_string(),
        );
        let e2 = Event::new(
            0,
            prev,
            EventType::MemberJoined,
            None,
            None,
            serde_json::json!({}),
            "t".to_string(),
        );
        assert_eq!(e1.hash, e2.hash);
    }

    #[test]
    fn event_hash_changes_with_payload() {
        let prev = [0u8; 32];
        let e1 = Event::new(
            0,
            prev,
            EventType::MemberJoined,
            None,
            None,
            serde_json::json!({"a": 1}),
            "t".to_string(),
        );
        let e2 = Event::new(
            0,
            prev,
            EventType::MemberJoined,
            None,
            None,
            serde_json::json!({"a": 2}),
            "t".to_string(),
        );
        assert_ne!(e1.hash, e2.hash);
    }

    #[test]
    fn verify_hash_ok() {
        let event = Event::new(
            0,
            [0u8; 32],
            EventType::InviteCreated,
            None,
            None,
            serde_json::json!(null),
            "now".to_string(),
        );
        assert!(event.verify_hash());
    }

    #[test]
    fn verify_hash_detects_tamper() {
        let mut event = Event::new(
            0,
            [0u8; 32],
            EventType::InviteCreated,
            None,
            None,
            serde_json::json!(null),
            "now".to_string(),
        );
        event.payload = serde_json::json!("tampered");
        assert!(!event.verify_hash());
    }

    // --- Chain verification ---

    #[test]
    fn valid_chain() {
        let instance = PublicKey::from_bytes([10u8; 32]);
        let events = make_chain(&instance, 5);
        let genesis = Event::genesis_prev_hash(&instance);
        assert!(verify_chain(&events, &genesis).is_ok());
    }

    #[test]
    fn empty_chain_valid() {
        let genesis = [0u8; 32];
        assert!(verify_chain(&[], &genesis).is_ok());
    }

    #[test]
    fn tampered_payload_detected() {
        let instance = PublicKey::from_bytes([11u8; 32]);
        let mut events = make_chain(&instance, 3);
        let genesis = Event::genesis_prev_hash(&instance);
        events[1].payload = serde_json::json!("tampered");
        let err = verify_chain(&events, &genesis).unwrap_err();
        assert!(matches!(err, ChainError::HashMismatch { id: 1 }));
    }

    #[test]
    fn deleted_middle_event_detected() {
        let instance = PublicKey::from_bytes([12u8; 32]);
        let mut events = make_chain(&instance, 4);
        let genesis = Event::genesis_prev_hash(&instance);
        events.remove(1); // delete event at index 1
        let err = verify_chain(&events, &genesis).unwrap_err();
        assert!(matches!(err, ChainError::BrokenLink { .. }));
    }

    #[test]
    fn modified_hash_detected() {
        let instance = PublicKey::from_bytes([13u8; 32]);
        let mut events = make_chain(&instance, 3);
        let genesis = Event::genesis_prev_hash(&instance);
        events[0].hash[0] ^= 0xff; // flip a bit in the stored hash
        // This should break the link check on event[1]
        let err = verify_chain(&events, &genesis).unwrap_err();
        // Could be HashMismatch on event 0 or BrokenLink on event 1
        assert!(matches!(
            err,
            ChainError::HashMismatch { .. } | ChainError::BrokenLink { .. }
        ));
    }

    #[test]
    fn wrong_genesis_detected() {
        let instance = PublicKey::from_bytes([14u8; 32]);
        let events = make_chain(&instance, 2);
        let wrong_genesis = [0xffu8; 32];
        let err = verify_chain(&events, &wrong_genesis).unwrap_err();
        assert!(matches!(err, ChainError::BrokenLink { id: 0 }));
    }

    // --- EventType Display / FromStr ---

    #[test]
    fn event_type_display_fromstr() {
        let types = [
            EventType::MemberJoined,
            EventType::MemberSuspended,
            EventType::MemberReinstated,
            EventType::MemberRemoved,
            EventType::MemberReplaced,
            EventType::GrantCapabilityChanged,
            EventType::GrantAccessChanged,
            EventType::InviteCreated,
            EventType::InviteRedeemed,
            EventType::InviteRevoked,
            EventType::InviteNounCreated,
            EventType::InviteNounResolved,
            EventType::IdentityUpdated,
        ];
        for t in types {
            let s = t.to_string();
            let back: EventType = s.parse().unwrap();
            assert_eq!(t, back, "roundtrip failed for {t}");
        }
    }

    #[test]
    fn event_type_unknown() {
        assert!("nope".parse::<EventType>().is_err());
    }

    // --- Checkpoint ---

    #[test]
    fn checkpoint_sign_verify() {
        let (sk, pk) = test_keypair();
        let cp = EventCheckpoint::sign(&sk, 42, [0xab; 32], "2025-01-01T00:00:00Z".to_string());
        assert!(cp.verify(&pk).is_ok());
    }

    #[test]
    fn checkpoint_wrong_key() {
        let (sk, _) = test_keypair();
        let (_, pk2) = test_keypair();
        let cp = EventCheckpoint::sign(&sk, 42, [0xab; 32], "2025-01-01T00:00:00Z".to_string());
        assert!(cp.verify(&pk2).is_err());
    }

    #[test]
    fn checkpoint_tamper_detected() {
        let (sk, pk) = test_keypair();
        let mut cp = EventCheckpoint::sign(&sk, 42, [0xab; 32], "2025-01-01T00:00:00Z".to_string());
        cp.event_id = 43; // tamper
        assert!(cp.verify(&pk).is_err());
    }

    // --- Genesis ---

    #[test]
    fn genesis_prev_hash_deterministic() {
        let pk = PublicKey::from_bytes([99u8; 32]);
        let h1 = Event::genesis_prev_hash(&pk);
        let h2 = Event::genesis_prev_hash(&pk);
        assert_eq!(h1, h2);
    }

    #[test]
    fn payload_key_order_is_canonical() {
        // Insert keys in different order — hash must be identical because
        // compute_hash canonicalizes JSON (sorts keys) before hashing.
        let mut map_a = serde_json::Map::new();
        map_a.insert("zzz".to_string(), serde_json::json!(1));
        map_a.insert("aaa".to_string(), serde_json::json!(2));

        let mut map_b = serde_json::Map::new();
        map_b.insert("aaa".to_string(), serde_json::json!(2));
        map_b.insert("zzz".to_string(), serde_json::json!(1));

        let prev = [0u8; 32];
        let e1 = Event::new(
            0,
            prev,
            EventType::MemberJoined,
            None,
            None,
            serde_json::Value::Object(map_a),
            "t".to_string(),
        );
        let e2 = Event::new(
            0,
            prev,
            EventType::MemberJoined,
            None,
            None,
            serde_json::Value::Object(map_b),
            "t".to_string(),
        );
        assert_eq!(e1.hash, e2.hash, "key insertion order must not affect hash");
    }

    #[test]
    fn genesis_prev_hash_varies_by_instance() {
        let pk1 = PublicKey::from_bytes([1u8; 32]);
        let pk2 = PublicKey::from_bytes([2u8; 32]);
        assert_ne!(
            Event::genesis_prev_hash(&pk1),
            Event::genesis_prev_hash(&pk2)
        );
    }
}
