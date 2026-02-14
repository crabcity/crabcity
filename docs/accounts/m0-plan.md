# Milestone 0: Foundations — Implementation Plan

Pure library crate. No IO, no database, no network. Types, crypto, validation,
and tests.

## Crate: `packages/crab_city_auth/`

### Setup

- [ ] Create `packages/crab_city_auth/Cargo.toml`
- [ ] Create `packages/crab_city_auth/src/lib.rs`
- [ ] Add to workspace `Cargo.toml` members + default-members
- [ ] Add new workspace deps: `ed25519-dalek`, `data-encoding`, `sha2`,
      `proptest`, `rand`
- [ ] Add `crate_index.spec()` entries to `MODULE.bazel` for all new deps
- [ ] Create `packages/crab_city_auth/BUILD.bazel` (rust_library + rust_test)
- [ ] Verify: `cargo check -p crab_city_auth`
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

- [ ] Newtype `PublicKey([u8; 32])` wrapping raw bytes
  - `from_bytes`, `as_bytes`, `Display` (base64), `Debug` (fingerprint)
  - `serde` as base64 (URL-safe, unpadded)
  - `Eq`, `Hash`, `Clone`, `Copy`
- [ ] `PublicKey::fingerprint() -> String`
  - `crab_` + first 8 chars of Crockford base32 of the 32-byte key
  - Case-insensitive output (Crockford base32 is uppercase)
- [ ] `PublicKey::LOOPBACK` — `PublicKey([0u8; 32])`, the all-zeros sentinel
- [ ] `PublicKey::is_loopback(&self) -> bool`
- [ ] Newtype `SigningKey` wrapping `ed25519_dalek::SigningKey`
  - `generate(rng) -> Self`
  - `public_key() -> PublicKey`
  - `sign(message: &[u8]) -> Signature`
  - `Clone` but NOT `Copy` (signing keys are sensitive)
- [ ] Newtype `Signature([u8; 64])` wrapping raw bytes
  - `from_bytes`, `as_bytes`
  - `serde` as base64
- [ ] `verify(public_key, message, signature) -> Result<()>` standalone fn
- [ ] Unit tests:
  - Generate keypair → sign → verify → Ok
  - Generate keypair → sign → verify with wrong key → Err
  - Generate keypair → sign → tamper message → verify → Err
  - Fingerprint format: starts with `crab_`, 13 chars total, deterministic
  - LOOPBACK fingerprint is stable
  - Serde round-trip (JSON)

**Estimated: ~150 LOC + ~50 LOC tests**

### Step 2: Capability and Access Rights (`capability.rs`)

The authorization model. Must exist before invites (invites carry capabilities).

- [ ] `Capability` enum: `View`, `Collaborate`, `Admin`, `Owner`
  - `Ord` impl: `View < Collaborate < Admin < Owner`
  - `serde` as lowercase string: `"view"`, `"collaborate"`, `"admin"`, `"owner"`
  - `Display`, `FromStr`
- [ ] `AccessRight` struct: `{ type_: String, actions: Vec<String> }`
  - `serde` with `#[serde(rename = "type")]` for `type_`
  - `Eq`, `Clone`
- [ ] `AccessRights` newtype wrapping `Vec<AccessRight>`
  - `serde` transparent
- [ ] `Capability::access_rights(&self) -> AccessRights` — expand preset:
  - `View` → `[content:read, terminals:read]`
  - `Collaborate` → View + `[terminals:input, chat:send,
    tasks:read,create,edit, instances:create]`
  - `Admin` → Collaborate + `[members:read,invite,suspend,reinstate,remove,update]`
  - `Owner` → Admin + `[instance:manage,transfer]`
- [ ] **Capability algebra** on `AccessRights`:
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
- [ ] `Capability::from_access(access: &AccessRights) -> Option<Capability>`
  - Reverse mapping: if access exactly matches a preset, return it
- [ ] Property tests (in `tests/property_tests.rs`):
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

- [ ] `MembershipState` enum: `Invited`, `Active`, `Suspended`, `Removed`
  - `serde` as lowercase string
  - `Display`, `FromStr`
- [ ] `MembershipTransition` enum:
  - `Activate` (invited → active, first auth)
  - `Suspend { reason: String, source: SuspensionSource }` (active → suspended)
  - `Reinstate` (suspended → active)
  - `Remove` (active|suspended → removed)
  - `Expire` (invited → removed, invite expired before first auth)
  - `BlocklistHit { scope: String }` (active → suspended)
  - `BlocklistLift` (suspended → active, only if blocklist-sourced)
  - `Replace { new_pubkey: PublicKey }` (any non-removed → removed, creates
    new grant)
- [ ] `MembershipState::apply(transition) -> Result<MembershipState, TransitionError>`
  - Validates: removed is terminal, suspend only from active, reinstate only
    from suspended, blocklist_lift only if suspension was blocklist-sourced
- [ ] `SuspensionSource` enum: `Admin`, `Blocklist { scope: String }`
- [ ] `TransitionError` enum with descriptive variants
- [ ] Exhaustive unit tests — every valid transition, every invalid transition:
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

- [ ] `InviteLink` struct:
  - `issuer: PublicKey`
  - `capability: Capability`
  - `max_depth: u8`
  - `max_uses: u32`
  - `expires_at: Option<u64>` (unix timestamp, None = never)
  - `nonce: [u8; 16]`
  - `signature: Signature`
- [ ] `Invite` struct:
  - `version: u8` (0x01)
  - `instance: PublicKey` (NodeId)
  - `links: Vec<InviteLink>` (ordered, root-to-leaf)
- [ ] Binary serialization (not serde — this is a wire format):
  - `Invite::to_bytes(&self) -> Vec<u8>` — compact binary encoding
  - `Invite::from_bytes(bytes: &[u8]) -> Result<Invite>` — parse with
    validation
- [ ] Base32 encoding/decoding:
  - `Invite::to_base32(&self) -> String` (Crockford, no padding)
  - `Invite::from_base32(s: &str) -> Result<Invite>`
- [ ] Signing:
  - `InviteLink::sign(signing_key, prev_link_hash, instance) -> InviteLink`
    - Root link signs `H(0x00*32) ++ instance ++ fields`
    - Subsequent links sign `H(prev_link) ++ instance ++ fields`
  - `InviteLink::hash(&self) -> [u8; 32]` (SHA-256 of the link's fields)
- [ ] `Invite::create_flat(signing_key, instance, capability, max_uses,
      expires_at) -> Invite`
  - Convenience for single-link invites with `max_depth = 0`
- [ ] `Invite::delegate(parent_invite, signing_key, capability, max_uses,
      expires_at) -> Result<Invite>`
  - Append a new link. Validates:
    - `capability <= parent.leaf().capability`
    - `parent.leaf().max_depth > 0`
    - New link's `max_depth = parent.leaf().max_depth - 1`
- [ ] Verification:
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
- [ ] Property tests:
  - Round-trip: `from_bytes(invite.to_bytes()) == invite` for all valid
    invites
  - Round-trip: `from_base32(invite.to_base32()) == invite`
  - Signature: `create_flat(k, ...).verify() == Ok` for all keypairs
  - Forgery: `create_flat(k1, ...).verify()` then tamper any byte → `Err`
  - Delegation: 3-hop chain verifies correctly
  - Capability narrowing: chain with `admin → owner` → verify rejects
  - Depth exhausted: chain at `max_depth=0` → delegate → Err
  - Size check: flat invite is 160 bytes, 3-hop is 412 bytes
- [ ] Fuzz target: `Invite::from_bytes()` — must not panic on any input

**Estimated: ~350 LOC + ~150 LOC tests**

### Step 5: Identity Proofs (`identity_proof.rs`)

Depends on: keys.

- [ ] `IdentityProof` struct:
  - `version: u8` (0x01)
  - `subject: PublicKey`
  - `instance: PublicKey` (NodeId)
  - `related_keys: Vec<PublicKey>`
  - `registry_handle: Option<String>`
  - `timestamp: u64`
  - `signature: Signature`
- [ ] `IdentityProof::sign(signing_key, instance, related_keys, handle) ->
      Self`
  - Subject signs all fields
- [ ] `IdentityProof::verify(&self) -> Result<IdentityProofClaims>`
  - Verify signature using `subject` as public key
  - Returns parsed claims
- [ ] `to_bytes`, `from_bytes` (binary format)
- [ ] Unit tests:
  - Sign → verify → Ok
  - Tamper any field → verify → Err
  - Wrong key → verify → Err
- [ ] Fuzz target: `IdentityProof::from_bytes()` — must not panic
- [ ] Property test: sign then verify succeeds for arbitrary inputs; tamper
      then verify fails

**Estimated: ~120 LOC + ~50 LOC tests**

### Step 6: Nouns (`noun.rs`)

Depends on: keys (for NounResolution).

- [ ] `IdentityNoun` enum:
  - `Handle(String)` — `@alex`
  - `GitHub(String)` — `github:foo`
  - `Google(String)` — `google:alice@acme.com`
  - `Email(String)` — `email:bob@bar.com`
- [ ] `Display` impl: renders back to noun string
- [ ] `FromStr` impl: parses noun string
  - `@foo` → `Handle("foo")`
  - `github:foo` → `GitHub("foo")`
  - `google:foo@bar.com` → `Google("foo@bar.com")`
  - `email:foo@bar.com` → `Email("foo@bar.com")`
  - Anything else → `Err`
- [ ] Validation:
  - Handle: lowercase alphanumeric + hyphens, 3-30 chars, no leading/trailing
    hyphens
  - GitHub: 1-39 chars, alphanumeric + hyphens, no leading hyphen (GitHub
    rules)
  - Google/Email: basic email format validation
- [ ] `IdentityNoun::provider(&self) -> &str` — `"handle"`, `"github"`,
      `"google"`, `"email"`
- [ ] `IdentityNoun::subject(&self) -> &str` — the inner value
- [ ] `NounResolution` struct:
  - `account_id: uuid::Uuid`
  - `handle: Option<String>`
  - `pubkeys: Vec<PublicKey>`
  - `attestation: Vec<u8>` (opaque signed blob from registry)
- [ ] Property tests:
  - Round-trip: `from_str(noun.to_string()) == Ok(noun)` for all valid nouns
  - Validation: malformed nouns are rejected (`github:`, `@`, `email:nope`,
    `unknown:foo`)
- [ ] Unit tests:
  - Parse each format
  - Display each format
  - Reject invalid handles, usernames, emails

**Estimated: ~150 LOC + ~60 LOC tests**

### Step 7: Event Log Types (`event.rs`)

Depends on: keys, capability.

- [ ] `EventType` enum (serde as dotted string):
  - `MemberJoined`, `MemberSuspended`, `MemberReinstated`, `MemberRemoved`,
    `MemberReplaced`
  - `GrantCapabilityChanged`, `GrantAccessChanged`
  - `InviteCreated`, `InviteRedeemed`, `InviteRevoked`, `InviteNounCreated`,
    `InviteNounResolved`
  - `IdentityUpdated`
- [ ] `Event` struct:
  - `id: u64`
  - `prev_hash: [u8; 32]`
  - `event_type: EventType`
  - `actor: Option<PublicKey>`
  - `target: Option<PublicKey>`
  - `payload: serde_json::Value`
  - `created_at: String` (ISO 8601)
  - `hash: [u8; 32]`
- [ ] `Event::compute_hash(&self) -> [u8; 32]`
  - `SHA-256(id ++ prev_hash ++ event_type ++ actor ++ target ++ payload ++
    created_at)`
- [ ] `Event::genesis_prev_hash(instance_node_id: &PublicKey) -> [u8; 32]`
  - `SHA-256(instance_node_id)` — the prev_hash for the first event
- [ ] `verify_chain(events: &[Event]) -> Result<(), ChainError>`
  - Sequential scan: each event's `prev_hash` must equal the previous event's
    `hash`
  - Each event's `hash` must match `compute_hash()`
- [ ] `EventCheckpoint` struct:
  - `event_id: u64`
  - `chain_head_hash: [u8; 32]`
  - `signature: Signature` (instance signs the chain head)
  - `created_at: String`
- [ ] `EventCheckpoint::sign(signing_key, event_id, chain_head_hash) -> Self`
- [ ] `EventCheckpoint::verify(&self, verifying_key) -> Result<()>`
- [ ] Property tests:
  - Build a chain of N events → verify succeeds
  - Tamper with any event's payload → verify detects break
  - Delete an event from the middle → verify detects break
  - Modify an event's hash → verify detects break
  - Checkpoint signature round-trip

**Estimated: ~200 LOC + ~80 LOC tests**

### Step 8: Error Types (`error.rs`)

Depends on: nothing (can be done anytime, but logically last since it
references other types).

- [ ] `RecoveryAction` enum:
  - `Reconnect`
  - `Retry { retry_after_secs: u64 }`
  - `ContactAdmin { admin_fingerprints: Vec<String>, reason: String }`
  - `RedeemInvite`
  - `None`
- [ ] `Recovery` struct: `{ action: RecoveryAction }`
  - `serde` for JSON responses
- [ ] `AuthError` enum (used in iroh stream error messages and registry HTTP):
  - `InvalidInvite(String)`
  - `NotAMember`
  - `GrantNotActive { reason: String }`
  - `InsufficientAccess { required_type: String, required_action: String }`
  - `Blocklisted { reason: String }`
  - `HandleTaken`
  - `AlreadyAMember`
  - `RateLimited { retry_after_secs: u64 }`
- [ ] `AuthError::recovery(&self) -> Recovery` — maps each variant to the
      correct recovery action
- [ ] `AuthError::error_code(&self) -> &str` (e.g., `"not_a_member"`)
- [ ] `serde` impl for JSON error response format:
  ```json
  { "error": "...", "message": "...", "recovery": { ... } }
  ```
- [ ] Unit tests: every variant produces correct status code, error code, and
      recovery action

**Estimated: ~150 LOC + ~40 LOC tests**

### Step 9: Property Test Suite (`tests/property_tests.rs`)

Consolidates cross-cutting property tests that exercise multiple modules
together.

- [ ] Move per-module property tests here (or keep inline and add
      cross-cutting ones here)
- [ ] Invite + capability interaction:
  - Delegated invite's effective capability is always <= root issuer's
    capability
  - `invite.verify().capability.access_rights().is_superset_of(leaf.capability.access_rights())`
    is false (leaf is narrower or equal)
- [ ] State machine exhaustive exploration:
  - Generate all reachable (state, transition) pairs
  - Verify every valid transition produces a valid state
  - Verify every invalid transition produces an error

**Estimated: ~80 LOC (additional cross-cutting tests)**

### Step 10: Formal Model

Not Rust. Separate artifact that generates test vectors.

- [ ] TLA+ (or Alloy) specification of the membership state machine
  - States: `{Invited, Active, Suspended, Removed}`
  - Transitions: all variants of `MembershipTransition`
  - Invariants:
    - `Removed` is terminal
    - `Suspend`/`Reinstate` only from valid source states
    - `BlocklistLift` only restores blocklist-sourced suspensions
    - Capability changes only in `Active` state
    - `Replace` creates new grant, transitions old to `Removed`
  - Model checker proves no sequence of transitions violates invariants
- [ ] Generate test cases from the model → feed into Rust tests
- [ ] Document the model in a `docs/accounts/state-machine.tla` (or `.als`)

**Estimated: ~150 lines TLA+**

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

- [ ] `cargo check -p crab_city_auth` passes
- [ ] `cargo test -p crab_city_auth` passes (all property tests green)
- [ ] `bazel build //packages/crab_city_auth` passes
- [ ] `bazel test //packages/crab_city_auth:crab_city_auth_test` passes
- [ ] `bazel run //tools/format` produces no changes
- [ ] Fuzz targets run for 5 minutes with no crashes
- [ ] TLA+ model checker finds no invariant violations
- [ ] No `pub` exports of internal implementation details — clean API surface
- [ ] Every public type has a `#[cfg(test)]` module with at least basic
      round-trip coverage
