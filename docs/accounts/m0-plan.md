# Milestone 0: Foundations — Implementation Plan

Pure library crate. No IO, no database, no network. Types, crypto, validation,
and tests.

## Crate: `packages/crab_city_auth/`

### Setup

- [x] Create `packages/crab_city_auth/Cargo.toml`
- [x] Create `packages/crab_city_auth/src/lib.rs`
- [x] Add to workspace `Cargo.toml` members + default-members
- [x] Add new workspace deps: `ed25519-dalek`, `data-encoding`, `sha2`,
      `proptest`, `rand`
- [x] Add `crate_index.spec()` entries to `MODULE.bazel` for all new deps
- [x] Create `packages/crab_city_auth/BUILD.bazel` (rust_library + rust_test)
- [x] Verify: `cargo check -p crab_city_auth`
- [ ] Verify: `bazel build //packages/crab_city_auth`

```toml
# packages/crab_city_auth/Cargo.toml
[package]
name = "crab_city_auth"
version = "0.1.0"
edition = "2024"
description = "Cryptographic identity, authorization, and invite primitives for Crab City"

[lib]
name = "crab_city_auth"
path = "src/lib.rs"

[dependencies]
data-encoding = "2"
ed25519-dalek = { version = "2", features = ["rand_core"] }
rand = "0.8"
serde = { workspace = true }
serde_json = { workspace = true }
sha2 = "0.10"
thiserror = { workspace = true }
uuid = { workspace = true }

[dev-dependencies]
proptest = "1"
```

### File Layout

```
packages/crab_city_auth/
  Cargo.toml
  BUILD.bazel
  src/
    lib.rs              # re-exports, module declarations
    keys.rs             # PublicKey, SigningKey, Signature, fingerprint, LOOPBACK
    capability.rs       # Capability enum, AccessRight, AccessRights, algebra
    membership.rs       # MembershipState enum, transition validation
    invite.rs           # InviteLink, Invite, sign/verify/delegate, base32
    identity_proof.rs   # IdentityProof, sign/verify
    noun.rs             # IdentityNoun enum, NounResolution, parse/validate
    event.rs            # EventType, Event, hash chain, EventCheckpoint
    error.rs            # AuthError, Recovery, RecoveryAction
  tests/
    property_tests.rs   # proptest suite
  fuzz/                 # cargo-fuzz targets (later)
```

---

## Implementation Order

Build bottom-up. Each step depends only on prior steps. Mark each `[x]` as you
complete it.

### Step 1: Keys (`keys.rs`)

The foundation everything else is built on.

- [x] Newtype `PublicKey([u8; 32])` wrapping raw bytes
  - `from_bytes`, `as_bytes`, `Display` (base64), `Debug` (fingerprint)
  - `serde` as base64 (URL-safe, unpadded)
  - `Eq`, `Hash`, `Clone`, `Copy`
- [x] `PublicKey::fingerprint() -> String`
  - `crab_` + first 8 chars of Crockford base32 of the 32-byte key
  - Case-insensitive output (Crockford base32 is uppercase)
- [x] `PublicKey::LOOPBACK` — `PublicKey([0u8; 32])`, the all-zeros sentinel
- [x] `PublicKey::is_loopback(&self) -> bool`
- [x] Newtype `SigningKey` wrapping `ed25519_dalek::SigningKey`
  - `generate(rng) -> Self`
  - `public_key() -> PublicKey`
  - `sign(message: &[u8]) -> Signature`
  - `Clone` but NOT `Copy` (signing keys are sensitive)
- [x] Newtype `Signature([u8; 64])` wrapping raw bytes
  - `from_bytes`, `as_bytes`
  - `serde` as base64
- [x] `verify(public_key, message, signature) -> Result<()>` standalone fn
- [x] Unit tests:
  - Generate keypair → sign → verify → Ok
  - Generate keypair → sign → verify with wrong key → Err
  - Generate keypair → sign → tamper message → verify → Err
  - Fingerprint format: starts with `crab_`, 13 chars total, deterministic
  - LOOPBACK fingerprint is stable
  - Serde round-trip (JSON)

**Estimated: ~150 LOC + ~50 LOC tests**

### Step 2: Capability and Access Rights (`capability.rs`)

The authorization model. Must exist before invites (invites carry capabilities).

- [x] `Capability` enum: `View`, `Collaborate`, `Admin`, `Owner`
  - `Ord` impl: `View < Collaborate < Admin < Owner`
  - `serde` as lowercase string: `"view"`, `"collaborate"`, `"admin"`, `"owner"`
  - `Display`, `FromStr`
- [x] `AccessRight` struct: `{ type_: String, actions: Vec<String> }`
  - `serde` with `#[serde(rename = "type")]` for `type_`
  - `Eq`, `Clone`
- [x] `AccessRights` newtype wrapping `Vec<AccessRight>`
  - `serde` transparent
- [x] `Capability::access_rights(&self) -> AccessRights` — expand preset:
  - `View` → `[content:read, terminals:read]`
  - `Collaborate` → View + `[terminals:input, chat:send,
    tasks:read,create,edit, instances:create]`
  - `Admin` → Collaborate + `[members:read,invite,suspend,reinstate,remove,update]`
  - `Owner` → Admin + `[instance:manage,transfer]`
- [x] **Capability algebra** on `AccessRights`:
  - `intersect(&self, other: &AccessRights) -> AccessRights`
    - For each type present in both, intersect actions
    - Types present in only one side are dropped
  - `contains(&self, type_: &str, action: &str) -> bool`
    - Find matching type, check if action is present
  - `is_superset_of(&self, other: &AccessRights) -> bool`
    - For every type+action in `other`, `self` must also contain it
  - `diff(&self, other: &AccessRights) -> (AccessRights, AccessRights)`
    - Returns `(added, removed)` — rights in `other` not in `self`, and vice
      versa
- [x] `Capability::from_access(access: &AccessRights) -> Option<Capability>`
  - Reverse mapping: if access exactly matches a preset, return it
- [x] Property tests (in `tests/property_tests.rs`):
  - `intersect` is commutative: `a ∩ b == b ∩ a`
  - `intersect` is idempotent: `a ∩ a == a`
  - `intersect` narrows: `(a ∩ b).is_superset_of(c)` → `a ⊇ c && b ⊇ c`
  - Preset ordering: `Owner ⊇ Admin ⊇ Collaborate ⊇ View`
  - Round-trip: `from_access(cap.access_rights()) == Some(cap)` for all
    presets
  - Empty intersect: `intersect(view, {tasks:create})` → empty
  - `contains` consistent with `is_superset_of`: if `a.contains(t, a_)` then
    `a.is_superset_of(AccessRights::single(t, a_))`
  - `diff` consistent with `intersect`: `old.diff(new).added` ∪
    `old.intersect(new)` == `new`

**Estimated: ~250 LOC + ~150 LOC tests**

### Step 3: Membership State Machine (`membership.rs`)

Small but critical. The correctness kernel for grant lifecycle.

- [x] `MembershipState` enum: `Invited`, `Active`, `Suspended`, `Removed`
  - `serde` as lowercase string
  - `Display`, `FromStr`
- [x] `MembershipTransition` enum:
  - `Activate` (invited → active, first auth)
  - `Suspend { reason: String, source: SuspensionSource }` (active → suspended)
  - `Reinstate` (suspended → active)
  - `Remove` (active|suspended → removed)
  - `Expire` (invited → removed, invite expired before first auth)
  - `BlocklistHit { scope: String }` (active → suspended)
  - `BlocklistLift` (suspended → active, only if blocklist-sourced)
  - `Replace { new_pubkey: PublicKey }` (any non-removed → removed, creates
    new grant)
- [x] `MembershipState::apply(transition) -> Result<MembershipState, TransitionError>`
  - Validates: removed is terminal, suspend only from active, reinstate only
    from suspended, blocklist_lift only if suspension was blocklist-sourced
- [x] `SuspensionSource` enum: `Admin`, `Blocklist { scope: String }`
- [x] `TransitionError` enum with descriptive variants
- [x] Exhaustive unit tests — every valid transition, every invalid transition:
  - `invited → Activate → active` ✓
  - `invited → Expire → removed` ✓
  - `invited → Suspend → Err` ✗
  - `active → Suspend → suspended` ✓
  - `active → Remove → removed` ✓
  - `active → Activate → Err` ✗
  - `active → Reinstate → Err` ✗
  - `suspended → Reinstate → active` ✓
  - `suspended → Remove → removed` ✓
  - `suspended(blocklist) → BlocklistLift → active` ✓
  - `suspended(admin) → BlocklistLift → Err` ✗
  - `removed → anything → Err` ✗

**Estimated: ~120 LOC + ~80 LOC tests**

### Step 4: Invite Tokens (`invite.rs`)

Depends on: keys, capability.

- [x] `InviteLink` struct:
  - `issuer: PublicKey`
  - `capability: Capability`
  - `max_depth: u8`
  - `max_uses: u32`
  - `expires_at: Option<u64>` (unix timestamp, None = never)
  - `nonce: [u8; 16]`
  - `signature: Signature`
- [x] `Invite` struct:
  - `version: u8` (0x01)
  - `instance: PublicKey` (NodeId)
  - `links: Vec<InviteLink>` (ordered, root-to-leaf)
- [x] Binary serialization (not serde — this is a wire format):
  - `Invite::to_bytes(&self) -> Vec<u8>` — compact binary encoding
  - `Invite::from_bytes(bytes: &[u8]) -> Result<Invite>` — parse with
    validation
- [x] Base32 encoding/decoding:
  - `Invite::to_base32(&self) -> String` (Crockford, no padding)
  - `Invite::from_base32(s: &str) -> Result<Invite>`
- [x] Signing:
  - `InviteLink::sign(signing_key, prev_link_hash, instance) -> InviteLink`
    - Root link signs `H(0x00*32) ++ instance ++ fields`
    - Subsequent links sign `H(prev_link) ++ instance ++ fields`
  - `InviteLink::hash(&self) -> [u8; 32]` (SHA-256 of the link's fields)
- [x] `Invite::create_flat(signing_key, instance, capability, max_uses,
      expires_at) -> Invite`
  - Convenience for single-link invites with `max_depth = 0`
- [x] `Invite::delegate(parent_invite, signing_key, capability, max_uses,
      expires_at) -> Result<Invite>`
  - Append a new link. Validates:
    - `capability <= parent.leaf().capability`
    - `parent.leaf().max_depth > 0`
    - New link's `max_depth = parent.leaf().max_depth - 1`
- [x] Verification:
  - `Invite::verify(&self) -> Result<InviteClaims>`
    - Walk chain root-to-leaf
    - Verify each signature
    - Verify capability narrowing (each link <= previous)
    - Verify depth constraints
    - Check expiry (if set)
    - Returns `InviteClaims { instance, capability, root_issuer, leaf_issuer,
      chain_depth, nonce }`
  - Note: does NOT check use count or issuer grant status — that's the
    instance's job at redemption time
- [x] Property tests:
  - Round-trip: `from_bytes(invite.to_bytes()) == invite` for all valid
    invites
  - Round-trip: `from_base32(invite.to_base32()) == invite`
  - Signature: `create_flat(k, ...).verify() == Ok` for all keypairs
  - Forgery: `create_flat(k1, ...).verify()` then tamper any byte → `Err`
  - Delegation: 3-hop chain verifies correctly
  - Capability narrowing: chain with `admin → owner` → verify rejects
  - Depth exhausted: chain at `max_depth=0` → delegate → Err
  - Size check: flat invite is 160 bytes, 3-hop is 412 bytes
- [x] ~~Fuzz target~~ Kani proof harnesses: `link_from_bytes_no_panic`,
      `invite_from_bytes_flat_no_panic`, `invite_from_bytes_short_no_panic`,
      `chain_depth_bound_enforced` — bounded model checking proves absence of
      panics for ALL inputs, strictly stronger than fuzzing

**Estimated: ~350 LOC + ~150 LOC tests**

### Step 5: Identity Proofs (`identity_proof.rs`)

Depends on: keys.

- [x] `IdentityProof` struct:
  - `version: u8` (0x01)
  - `subject: PublicKey`
  - `instance: PublicKey` (NodeId)
  - `related_keys: Vec<PublicKey>`
  - `registry_handle: Option<String>`
  - `timestamp: u64`
  - `signature: Signature`
- [x] `IdentityProof::sign(signing_key, instance, related_keys, handle) ->
      Self`
  - Subject signs all fields
- [x] `IdentityProof::verify(&self) -> Result<IdentityProofClaims>`
  - Verify signature using `subject` as public key
  - Returns parsed claims
- [x] `to_bytes`, `from_bytes` (binary format)
- [x] Unit tests:
  - Sign → verify → Ok
  - Tamper any field → verify → Err
  - Wrong key → verify → Err
- [x] ~~Fuzz target~~ Kani proof harnesses: `from_bytes_min_size_no_panic`,
      `from_bytes_short_no_panic`, `from_bytes_one_key_no_panic`,
      `key_count_bound_enforced` — bounded model checking, see Step 4 note
- [x] Property test: sign then verify succeeds for arbitrary inputs; tamper
      then verify fails

**Estimated: ~120 LOC + ~50 LOC tests**

### Step 6: Nouns (`noun.rs`)

Depends on: keys (for NounResolution).

- [x] `IdentityNoun` enum:
  - `Handle(String)` — `@alex`
  - `GitHub(String)` — `github:foo`
  - `Google(String)` — `google:alice@acme.com`
  - `Email(String)` — `email:bob@bar.com`
- [x] `Display` impl: renders back to noun string
- [x] `FromStr` impl: parses noun string
  - `@foo` → `Handle("foo")`
  - `github:foo` → `GitHub("foo")`
  - `google:foo@bar.com` → `Google("foo@bar.com")`
  - `email:foo@bar.com` → `Email("foo@bar.com")`
  - Anything else → `Err`
- [x] Validation:
  - Handle: lowercase alphanumeric + hyphens, 3-30 chars, no leading/trailing
    hyphens
  - GitHub: 1-39 chars, alphanumeric + hyphens, no leading hyphen (GitHub
    rules)
  - Google/Email: basic email format validation
- [x] `IdentityNoun::provider(&self) -> &str` — `"handle"`, `"github"`,
      `"google"`, `"email"`
- [x] `IdentityNoun::subject(&self) -> &str` — the inner value
- [x] `NounResolution` struct:
  - `account_id: uuid::Uuid`
  - `handle: Option<String>`
  - `pubkeys: Vec<PublicKey>`
  - `attestation: Vec<u8>` (opaque signed blob from registry)
- [x] Property tests:
  - Round-trip: `from_str(noun.to_string()) == Ok(noun)` for all valid nouns
  - Validation: malformed nouns are rejected (`github:`, `@`, `email:nope`,
    `unknown:foo`)
- [x] Unit tests:
  - Parse each format
  - Display each format
  - Reject invalid handles, usernames, emails

**Estimated: ~150 LOC + ~60 LOC tests**

### Step 7: Event Log Types (`event.rs`)

Depends on: keys, capability.

- [x] `EventType` enum (serde as dotted string):
  - `MemberJoined`, `MemberSuspended`, `MemberReinstated`, `MemberRemoved`,
    `MemberReplaced`
  - `GrantCapabilityChanged`, `GrantAccessChanged`
  - `InviteCreated`, `InviteRedeemed`, `InviteRevoked`, `InviteNounCreated`,
    `InviteNounResolved`
  - `IdentityUpdated`
- [x] `Event` struct:
  - `id: u64`
  - `prev_hash: [u8; 32]`
  - `event_type: EventType`
  - `actor: Option<PublicKey>`
  - `target: Option<PublicKey>`
  - `payload: serde_json::Value`
  - `created_at: String` (ISO 8601)
  - `hash: [u8; 32]`
- [x] `Event::compute_hash(&self) -> [u8; 32]`
  - `SHA-256(id ++ prev_hash ++ event_type ++ actor ++ target ++ payload ++
    created_at)`
- [x] `Event::genesis_prev_hash(instance_node_id: &PublicKey) -> [u8; 32]`
  - `SHA-256(instance_node_id)` — the prev_hash for the first event
- [x] `verify_chain(events: &[Event]) -> Result<(), ChainError>`
  - Sequential scan: each event's `prev_hash` must equal the previous event's
    `hash`
  - Each event's `hash` must match `compute_hash()`
- [x] `EventCheckpoint` struct:
  - `event_id: u64`
  - `chain_head_hash: [u8; 32]`
  - `signature: Signature` (instance signs the chain head)
  - `created_at: String`
- [x] `EventCheckpoint::sign(signing_key, event_id, chain_head_hash) -> Self`
- [x] `EventCheckpoint::verify(&self, verifying_key) -> Result<()>`
- [x] Property tests:
  - Build a chain of N events → verify succeeds
  - Tamper with any event's payload → verify detects break
  - Delete an event from the middle → verify detects break
  - Modify an event's hash → verify detects break
  - Checkpoint signature round-trip

**Estimated: ~200 LOC + ~80 LOC tests**

### Step 8: Error Types (`error.rs`)

Depends on: nothing (can be done anytime, but logically last since it
references other types).

- [x] `RecoveryAction` enum:
  - `Reconnect`
  - `Retry { retry_after_secs: u64 }`
  - `ContactAdmin { admin_fingerprints: Vec<String>, reason: String }`
  - `RedeemInvite`
  - `None`
- [x] `Recovery` struct: `{ action: RecoveryAction }`
  - `serde` for JSON responses
- [x] `AuthError` enum (used in iroh stream error messages and registry HTTP):
  - `InvalidInvite(String)`
  - `NotAMember`
  - `GrantNotActive { reason: String }`
  - `InsufficientAccess { required_type: String, required_action: String }`
  - `Blocklisted { reason: String }`
  - `HandleTaken`
  - `AlreadyAMember`
  - `RateLimited { retry_after_secs: u64 }`
- [x] `AuthError::recovery(&self) -> Recovery` — maps each variant to the
      correct recovery action
- [x] `AuthError::error_code(&self) -> &str` (e.g., `"not_a_member"`)
- [x] `serde` impl for JSON error response format:
  ```json
  { "error": "...", "message": "...", "recovery": { ... } }
  ```
- [x] Unit tests: every variant produces correct status code, error code, and
      recovery action

**Estimated: ~150 LOC + ~40 LOC tests**

### Step 9: Property Test Suite (`tests/property_tests.rs`)

Consolidates cross-cutting property tests that exercise multiple modules
together.

- [x] Move per-module property tests here (or keep inline and add
      cross-cutting ones here)
- [x] Invite + capability interaction:
  - Delegated invite's effective capability is always <= root issuer's
    capability
  - `invite.verify().capability.access_rights().is_superset_of(leaf.capability.access_rights())`
    is false (leaf is narrower or equal)
- [x] State machine exhaustive exploration:
  - Generate all reachable (state, transition) pairs
  - Verify every valid transition produces a valid state
  - Verify every invalid transition produces an error

**Estimated: ~80 LOC (additional cross-cutting tests)**

### Step 10: Formal Model (Kani)

Replaced TLA+/Alloy with Kani bounded model checking. Kani verifies the
actual Rust implementation directly — no model-to-code gap.

- [x] Membership state machine invariants (`membership.rs` `#[cfg(kani)]`):
  - `removed_is_terminal` — no transition from `Removed` ever succeeds
  - `valid_transitions_produce_valid_states` — every `Ok` result is a valid
    state; suspended states always carry a suspension source
  - `multi_step_invariants` — 5-step arbitrary transition sequences maintain
    all invariants; once `Removed` is reached, all subsequent transitions fail
- [x] Capability algebra invariants (`capability.rs` `#[cfg(kani)]`):
  - `intersect_is_commutative` — `a ∩ b == b ∩ a` for all capability pairs
  - `superset_follows_ord` — `a >= b` implies `a ⊇ b` in access rights
  - `from_access_roundtrips` — `from_access(cap.access_rights()) == Some(cap)`
  - `intersect_narrows` — result is always a subset of both inputs
- [x] Parser safety (`invite.rs`, `identity_proof.rs` `#[cfg(kani)]`):
  - Proves `from_bytes` never panics on ANY input (all `try_into().unwrap()`
    calls are reachable only after sufficient length guards)
  - Proves bounds checks (MAX_CHAIN_DEPTH, MAX_RELATED_KEYS) are enforced

Run with: `cargo kani -p crab_city_auth` (requires `cargo install --locked
kani-verifier && cargo kani setup`)

---

## Dependency Order

```
Step 1: keys
  ├── Step 2: capability (needs keys for tests only)
  │     └── Step 3: membership (needs capability for state context)
  ├── Step 4: invite (needs keys + capability)
  ├── Step 5: identity_proof (needs keys)
  └── Step 6: noun (needs keys for NounResolution)
Step 7: event (needs keys + capability)
Step 8: error (standalone, references other types)
Step 9: cross-cutting property tests (needs everything)
Step 10: formal model (parallel, not Rust)
```

Steps 4-6 are independent of each other and can be done in any order (or
parallel). Steps 1-3 are sequential. Steps 7-10 are the tail.

## Estimated Totals

| Component | Rust LOC | Test LOC |
|-----------|----------|----------|
| keys | ~150 | ~50 |
| capability | ~250 | ~150 |
| membership | ~120 | ~80 |
| invite | ~350 | ~150 |
| identity_proof | ~120 | ~50 |
| noun | ~150 | ~60 |
| event | ~200 | ~80 |
| error | ~120 | ~40 |
| cross-cutting tests | -- | ~80 |
| **Total** | **~1460** | **~740** |

Plus: ~150 lines TLA+, 2 fuzz targets.

## Done Criteria

- [x] `cargo check -p crab_city_auth` passes
- [x] `cargo test -p crab_city_auth` passes (all property tests green)
- [ ] `bazel build //packages/crab_city_auth` passes
- [ ] `bazel test //packages/crab_city_auth:crab_city_auth_test` passes
- [x] `bazel run //tools/format` produces no changes
- [ ] `cargo kani -p crab_city_auth` — all 15 proof harnesses verify
- [x] No `pub` exports of internal implementation details — clean API surface
- [x] Every public type has a `#[cfg(test)]` module with at least basic
      round-trip coverage
