# Crab City Accounts: Implementation Plan

## Overview

This plan breaks the account system into deliverable milestones. Each milestone is independently shippable and provides user-facing value. The system builds from the bottom up: instance-local auth first, registry second, enterprise features last.

## Milestone 0: Foundations

**Goal:** Shared crate with cryptographic types, invite token format (with delegation chains), GNAP-style access rights model, key fingerprints, hash-chained event types, and comprehensive property-based tests.

**Deliverables:**
- [ ] New crate: `packages/crab_city_auth/`
- [ ] Ed25519 types wrapping `ed25519-dalek`: `PublicKey`, `SigningKey`, `Signature`
- [ ] `PublicKey::fingerprint() -> String` — `crab_` + first 8 chars of Crockford base32
- [ ] `PublicKey::LOOPBACK` — the all-zeros sentinel constant
- [ ] `Capability` enum: `View`, `Collaborate`, `Admin`, `Owner` with `Ord` impl
- [ ] `AccessRight` struct: `{ type_: String, actions: Vec<String> }` with `serde` derive
- [ ] `Capability::access_rights() -> Vec<AccessRight>` — expand preset to GNAP-style access rights array
- [ ] **Capability algebra** — the only way to manipulate access rights:
  - `AccessRights::intersect(&self, other) -> AccessRights` (scoped sessions)
  - `AccessRights::contains(&self, type_, action) -> bool` (authorization checks)
  - `AccessRights::is_superset_of(&self, other) -> bool` (capability narrowing)
  - `AccessRights::diff(&self, other) -> (AccessRights, AccessRights)` (audit, access tweaking)
- [ ] `MembershipState` enum: `Invited`, `Active`, `Suspended`, `Removed` with transition validation
- [ ] `InviteLink` struct: issuer, capability, max_depth, max_uses, expires_at, nonce, signature
- [ ] `Invite` struct: instance NodeId + `Vec<InviteLink>` chain (flat invite = chain of length 1)
- [ ] `Invite::sign()`, `Invite::verify()`, `Invite::delegate()` methods
- [ ] Delegation chain verification: capability narrowing, depth checking, signature walking
- [ ] Base32 encoding/decoding (Crockford) for invite tokens
- [ ] **Cross-instance identity proofs**: `IdentityProof` struct with sign/verify methods
  - `IdentityProof::sign(signing_key, instance, related_keys, handle) -> Self`
  - `IdentityProof::verify() -> Result<IdentityProofClaims>`
- [ ] `IdentityNoun` enum: `Handle(String)`, `GitHub(String)`, `Google(String)`, `Email(String)` with `Display`, `FromStr`, parse/validate
- [ ] `NounResolution` struct: `{ account_id, handle, pubkeys, attestation }` — the result of resolving a noun through the registry
- [ ] `EventType` enum and `Event` struct with hash chain fields (`prev_hash`, `hash`)
- [ ] `Event::compute_hash()` and `Event::verify_chain()` helpers
- [ ] `EventCheckpoint` struct with instance signature
- [ ] **Structured error types**: `AuthError` enum with `recovery() -> Recovery` method, `Recovery` struct with typed actions
- [ ] Property-based tests (using `proptest`):
  - Round-trip: `Invite::from_bytes(invite.to_bytes()) == invite` for all valid invites
  - Signature: `invite.sign(k).verify(k.public()) == Ok` for all keypairs and invites
  - Forgery: `invite.sign(k1).verify(k2.public()) == Err` for all k1 != k2
  - Capability ordering: `c1 < c2` implies `c1.access_rights()` is a strict subset of `c2.access_rights()`
  - **Capability algebra**: `intersect` is commutative and idempotent; `intersect(a,b).is_superset_of(c)` implies both `a` and `b` are supersets of `c`
  - Delegation narrowing: verify chain with out-of-order capabilities is rejected
  - State machine: all reachable states under all transitions produce valid states
  - Access rights round-trip: `Capability::from_access(cap.access_rights()) == Some(cap)` for all presets
  - Hash chain: inserting/deleting/modifying any event in a chain is detectable
  - **Noun parsing round-trip**: `IdentityNoun::from_str(noun.to_string()) == Ok(noun)` for all valid nouns
  - **Noun validation**: malformed nouns (`github:`, `@`, `email:not-an-email`) are rejected
  - **Identity proof round-trip**: sign then verify succeeds; tamper then verify fails
- [ ] Fuzz targets:
  - `Invite::from_bytes()` (untrusted input from network — must not panic on any input)
  - `IdentityProof::from_bytes()` (untrusted input — must not panic)
- [ ] **Formal state machine model** (TLA+ or Alloy):
  - Membership state machine: prove no sequence of transitions violates invariants
  - `removed` is terminal, `suspend`/`reinstate` only from valid source states
  - `blocklist_lift` only restores blocklist-sourced suspensions
  - Generate test cases from the model for Rust implementation

**Crate dependencies:**
- `ed25519-dalek` (with `rand_core` feature)
- `data-encoding` (for Crockford base32)
- `serde_json` (for access rights serialization)
- `sha2` (for hash chain and token hashing)
- `serde` (for JSON API types)
- `uuid` (v7)

**Dev dependencies:**
- `proptest` (property-based testing)

**Build system:**
- Add `crab_city_auth` to `Cargo.toml` workspace members
- Add to `MODULE.bazel` as a local crate
- Wire into `//tools/format`
- Add `cargo-fuzz` target for invite parser

**Estimated size:** ~1200 lines of Rust + ~500 lines of property-based tests + fuzz targets + TLA+/Alloy model

---

## Milestone 1: Instance-Local Auth (iroh)

**Goal:** Instances authenticate all clients — native and browser — via iroh connections. Native clients connect via QUIC; browser clients connect via iroh WASM through the instance's embedded relay. One transport, one auth model. Invites (including delegated), membership management, and the full auth lifecycle. This milestone replaces the implicit "everyone is authenticated" model with real membership. Includes the join page with live preview, key backup modal, hash-chained event log, embedded iroh relay, multi-instance connection management, and integration tests.

**Depends on:** Milestone 0

### 1.1 Database Migrations

Add to `crab_city`'s SQLite migration system:

```
migrations/
  NNNN_create_member_identities.sql
  NNNN_create_member_grants.sql
  NNNN_create_invites.sql
  NNNN_create_blocklist.sql
  NNNN_create_event_log.sql
  NNNN_create_event_checkpoints.sql
  NNNN_seed_loopback_identity.sql
```

Tables: `member_identities`, `member_grants`, `invites`, `blocklist`, `blocklist_cache`, `event_log`, `event_checkpoints` (see design doc section 2.1). No session tokens or refresh tokens — the iroh connection IS the authenticated session.

The `seed_loopback_identity` migration inserts the loopback sentinel (all-zeros pubkey) with `owner` grant and `active` state.

### 1.2 Repository Layer

New file: `packages/crab_city/src/repository/membership.rs`

Functions:
- `create_identity(db, identity) -> Result<MemberIdentity>`
- `get_identity(db, public_key) -> Result<Option<MemberIdentity>>`
- `update_identity(db, public_key, updates) -> Result<()>`
- `create_grant(db, grant) -> Result<MemberGrant>`
- `get_grant(db, public_key) -> Result<Option<MemberGrant>>`
- `get_active_grant(db, public_key) -> Result<Option<MemberGrant>>` (state == active only)
- `list_members(db) -> Result<Vec<(MemberIdentity, MemberGrant)>>`
- `update_grant_capability(db, public_key, capability, access) -> Result<()>`
- `update_grant_access(db, public_key, access) -> Result<()>`
- `update_grant_state(db, public_key, new_state) -> Result<()>`
- `replace_grant(db, new_pubkey, old_pubkey) -> Result<()>`
- `list_grants_by_invite(db, invite_nonce) -> Result<Vec<MemberGrant>>`

New file: `packages/crab_city/src/repository/invites.rs`

Functions:
- `create_invite(db, invite) -> Result<()>`
- `get_invite(db, nonce) -> Result<Option<StoredInvite>>`
- `increment_invite_use_count(db, nonce) -> Result<()>`
- `revoke_invite(db, nonce) -> Result<()>`
- `list_active_invites(db) -> Result<Vec<StoredInvite>>`

New file: `packages/crab_city/src/repository/event_log.rs`

Functions:
- `log_event(db, event) -> Result<i64>` — computes hash chain (reads prev event hash, computes new hash)
- `query_events(db, filter) -> Result<Vec<Event>>`
- `verify_chain(db, from, to) -> Result<ChainVerification>` — sequential scan, verify hash linkage
- `get_chain_head(db) -> Result<(i64, [u8; 32])>` — latest event id and hash
- `create_checkpoint(db, event_id, signing_key) -> Result<EventCheckpoint>` — sign chain head
- `get_event_proof(db, event_id) -> Result<EventProof>` — event + surrounding hashes + nearest checkpoint

### 1.3 iroh RPC Handlers

All authenticated operations are iroh RPC messages (request/response over the
iroh stream). No HTTP auth endpoints.

New file: `packages/crab_city/src/handlers/rpc.rs`

The RPC dispatcher matches on the `type` field of incoming messages and routes
to the appropriate handler. Each handler receives `AuthUser` (populated from the
iroh connection) and the message payload.

New file: `packages/crab_city/src/handlers/invites.rs`

RPC messages:
- `CreateInvite` — create invite (requires `members:invite` access), supports `max_depth` for delegation. Accepts `idempotency_key` field.
- `RedeemInvite` — redeem invite token (flat or delegated chain), create identity + grant. Idempotent on `(invite_nonce, NodeId)` — returns existing grant on retry.
- `RevokeInvite` — revoke invite, optionally suspend derived members
- `InviteByNoun` — resolve noun via registry, create invite or register pending
- `ListPendingNouns` — list pending noun invites for admin visibility

New file: `packages/crab_city/src/handlers/members.rs`

RPC messages:
- `ListMembers` — list members (requires `content:read` access)
- `UpdateMember` — update capability (requires `members:update` access)
- `TweakAccess` — tweak individual access rights
- `RemoveMember` — remove member (requires `members:remove` access)
- `SuspendMember` — suspend member (closes their iroh connection immediately)
- `ReinstateMember` — reinstate member
- `ReplaceMember` — link new grant to old (key loss recovery)
- `QueryEvents` — query event log (requires `members:read` access)
- `VerifyEvents` — verify hash chain integrity
- `GetEventProof` — inclusion proof for a specific event (requires `content:read` access)

### 1.4 iroh Transport Layer

New file: `packages/crab_city/src/transport/iroh.rs`

The iroh transport handles ALL client connections (native and browser):

- **Embedded relay:** Start an in-process iroh relay server (`iroh-relay` crate
  with `server` feature). Exposes `ws://localhost:<port>/relay` for local
  browser clients and `wss://instance.example/relay` for remote browsers. The
  relay is a WebSocket-to-iroh bridge — same iroh protocol, same E2E
  encryption, same ed25519 handshake.
- **Connection acceptance:** Accept incoming iroh QUIC connections (native) and
  relay-mediated connections (browser). Extract `NodeId` from handshake.
- **Authentication:** Look up grant by pubkey. If grant exists and `state ==
  active`, accept and send initial state snapshot. If not, send structured error
  and close.
- **Stream management:** Open a bidirectional QUIC stream for the message
  protocol. Route incoming messages to RPC handlers.
- **Connection lifecycle:** Maintain a map of `NodeId -> active connection`. On
  grant suspension/removal, close the connection immediately.
- **Invite redemption:** Accept `RedeemInvite` messages from new connections
  (NodeId has no active grant yet, but carries a valid invite token).

New file: `packages/crab_city/src/transport/mod.rs`

```rust
/// A connected client. All clients connect via iroh.
struct ConnectedClient {
    node_id: NodeId,
    stream: QuicBiStream,
    grant: MemberGrant,
}

impl ConnectedClient {
    async fn send(&self, msg: &ServerMessage) -> Result<()>;
}
```

No transport enum, no dual-path logic. Every client is an iroh connection.

### 1.5 Auth Middleware

Modify: `packages/crab_city/src/auth.rs`

Auth chain (iroh-only):
1. Loopback bypass → synthetic owner identity (all-zeros pubkey, `owner` grant, full access)
2. iroh connection → extract NodeId from handshake → lookup grant → full access rights
3. No valid connection → reject

No session tokens, no revocation set, no signature verification on the hot path.
The grant is cached at connection establishment. Revocation is immediate — close
the iroh connection.

```rust
auth.require_access("tasks", "edit")?;  // calls AccessRights::contains() on grant
```

`AuthUser` is populated from the iroh connection state. Handlers are
transport-unaware — the same code works for native QUIC clients and browser
WASM clients.

### 1.6 Observability

Add from day one — not as an afterthought:

- `GET /metrics` endpoint (Prometheus format) with all counters/gauges from design doc section 14
- Structured logging for all auth decisions: `node_id_fingerprint`, `result`, `reason`, `transport` (`quic` or `relay`)
- Structured logging for all state transitions: `event_type`, `actor_fingerprint`, `target_fingerprint`
- iroh connection metrics: connections active (by transport label: `quic`/`relay`), reconnections, replay message count, snapshot count, auth rejections

**Crate dependencies:** `metrics`, `metrics-exporter-prometheus`, `tracing` (already a dep), `tracing-opentelemetry` (for future distributed tracing with registry)

### 1.7 Instance Bootstrap

On first startup, if the `member_identities` table has only the loopback sentinel:
- Generate an instance identity keypair (stored in config dir)
- Log the owner invite token to stdout

### 1.8 Frontend Changes

Modify: `packages/crab_city_ui/`

**Join page** (`/join` route):
- Extract invite token from `#fragment`
- Parse invite to show: instance name, inviter fingerprint, capability being granted
- If delegated invite: show delegation chain depth ("invited by Blake, who was invited by Alex")
- **Live preview panel**: connect to `/api/preview` WebSocket, show:
  - Number of users currently online (live-updating)
  - Abstracted activity visualization (blurred/stylized terminal with cursor movement, no content)
  - Instance uptime
- "Your name" input (pre-filled if registry account detected)
- "Join" button
- If existing keypair for this instance: show "Welcome back, [name]. [Rejoin]"

**Key backup modal** (blocking, post-keygen):
- "Save your identity key" explanation
- Copy-to-clipboard button (base64 private key)
- Download `.key` file button
- "I saved my key" checkbox — required to proceed

**Other frontend changes:**
- Add ed25519 keypair generation (Web Crypto API) and IndexedDB storage
- **iroh WASM integration**: bundle `iroh-wasm` in the SvelteKit app. On page
  load, initialize iroh with the user's keypair (= NodeId). Connect to the
  instance via the embedded relay (`ws://localhost:<port>/relay` for local,
  `wss://instance.example/relay` for remote).
- All authenticated communication goes over the iroh stream. No HTTP auth
  endpoints, no session tokens, no cookies.
- **Structured error handling**: parse `recovery` field from iroh error
  messages, route to appropriate UI flow (reconnect, contact_admin, etc.). No
  generic "something went wrong" screens.
- **iroh reconnection**: track `last_seq`, reconnect with `{ last_seq: N }` in
  the initial message, handle replay and snapshot messages transparently.
  Connection drops should be invisible to the user.
- Show current user identity + fingerprint in UI header
- Add member list panel (for admins, with state badges)
- Add invite creation UI (for admins) with **QR code rendering** (client-side SVG generation)
- Add member management actions (suspend, reinstate, remove, change capability)

### 1.9 Multi-Instance Connection Manager (CLI/TUI)

New file: `packages/crab_city/src/client/connection_manager.rs` (or in the TUI crate)

The connection manager holds N simultaneous iroh connections to different
instances:

- **Connection pool:** Map of `instance_id -> iroh::Connection`. All connections
  are established on startup (from a config file listing known instances).
- **Active instance:** One connection is "active" — receives input, renders in
  the TUI. Background connections receive presence, chat, and notifications.
- **Instance switcher:** Keybinding (Ctrl+1/2/3... or a picker UI) to switch
  the active instance. Switching is instant — the connection is already live.
- **Notification routing:** Background instances surface notifications
  (mentions, task assignments, new members) in a sidebar or status bar.
- **Connection health:** Each connection has independent reconnection logic.
  A failed connection to Instance B doesn't affect Instance A.

This is the only new UI feature required for multi-instance support. The
underlying iroh connections provide authentication and encryption by
construction.

### 1.10 Integration Tests

New file: `packages/crab_city/tests/auth_integration.rs`

End-to-end tests that spin up an in-memory instance and exercise the
security-critical paths. All tests use iroh connections (the only transport):

**Connection & auth:**
- Generate keypair → connect via iroh → verify NodeId extracted → verify grant checked → receive state snapshot
- Connect with unknown NodeId → verify connection rejected with structured error
- Connect via iroh, redeem invite over iroh stream → verify grant created → verify subsequent connections succeed
- Admin suspends user → verify their iroh connection closed immediately
- Disconnect → reconnect with `last_seq` → verify replay. Also: reconnect with very old `last_seq` → verify full snapshot.

**Invites & membership:**
- Generate keypair → create invite → redeem invite → verify capabilities are enforced
- **Delegated invite chain**: admin creates invite with `max_depth=2` → member delegates → sub-delegate redeems → verify capability narrowing is enforced, depth limit is enforced
- **Delegation forgery**: tamper with a link in a delegation chain → verify redemption is rejected
- Access enforcement: `collaborate` user cannot access `members` operations
- State machine: active → suspended → reinstate → active; suspended → removed
- Invite revocation: revoke invite → unredeemed uses fail, existing members unaffected
- Invite revocation with `suspend_derived_members`: all derived grants suspended
- **Idempotency**: redeem same invite with same NodeId twice → second call returns same grant, no duplicate.
- Key replacement: new key replaces old, old grant removed
- Loopback bypass: all-zeros pubkey → owner access on loopback, rejected remotely

**Event log & audit:**
- **Event log hash chain**: verify events have correct hash linkage → tamper with an event → verify `verify_chain` detects the break
- **Event checkpoints**: create checkpoint → verify signature → tamper with event before checkpoint → verify detection
- Event log: verify events recorded for all state transitions

**Preview & misc:**
- **Preview WebSocket**: connect to `/api/preview` without auth → verify only non-content signals are received
- **Structured errors**: verify all error messages include `recovery` field with valid action enum value
- **Identity proofs**: sign proof → verify → tamper → verify fails.

**Estimated size:** ~2000 lines Rust (iroh RPC handlers + repo + middleware + iroh transport layer + embedded relay + reconnection + multi-instance connection manager) + ~1400 lines Svelte/TS (join page + live preview + key backup + iroh WASM + member management + QR + reconnection + structured error handling) + ~800 lines integration tests

---

## Milestone 2: Registry Core (crabcity.dev)

**Goal:** A running registry at `crabcity.dev` where users can create accounts (with multi-device key management and key transparency from day one) and instances can register for discovery.

**Depends on:** Milestone 0 (shares `crab_city_auth` crate)

### 2.1 New Package

Create: `packages/crab_city_registry/`

This is a standalone axum binary. Same stack as `crab_city` (axum, SQLite, maud for any HTML pages), reuses `crab_city_auth` for crypto types.

```
packages/crab_city_registry/
  Cargo.toml
  src/
    main.rs
    config.rs
    db.rs
    repository/
      accounts.rs
      keys.rs
      instances.rs
      invites.rs
      blocklist.rs
      transparency.rs
    handlers/
      accounts.rs
      keys.rs
      instances.rs
      invites.rs
      transparency.rs
      health.rs
    middleware/
      auth.rs
      rate_limit.rs
```

### 2.2 Account Registration (with Multi-Device)

- `POST /api/v1/accounts` — register handle + public key + proof signature + key_label
- `GET /api/v1/accounts/by-handle/:handle` — public profile (includes all active keys)
- `GET /api/v1/accounts/by-key/:public_key` — reverse lookup (includes all active keys + account_id)
- `PATCH /api/v1/accounts/:id` — update display name, avatar (authenticated)

Handle validation: lowercase, alphanumeric + hyphens, 3-30 chars, no leading/trailing hyphens.

### 2.3 Key Management

- `POST /api/v1/accounts/:id/keys` — add a new key (authenticated with existing key, proof from new key). **Also appends to transparency log.**
- `DELETE /api/v1/accounts/:id/keys/:public_key` — revoke a key (cannot revoke last active key). **Also appends to transparency log.**
- `GET /api/v1/accounts/:id/keys` — list active keys

All key mutations go through a single codepath that appends to both the `account_keys` table and the `transparency_log` Merkle tree. This is enforced at the repository layer, not the handler layer — it is impossible to modify a key binding without creating a transparency entry.

### 2.3.1 Key Transparency

- `GET /api/v1/transparency/tree-head` — current signed Merkle tree head
- `GET /api/v1/transparency/proof?handle=:handle` — audit an account's key binding history with inclusion proofs
- `GET /api/v1/transparency/entries?start=N&end=M` — raw log entries for monitors (paginated)

The Merkle tree is a simple append-only binary tree (RFC 6962 style). Each leaf is `H(action ++ account_id ++ public_key ++ created_at)`. The tree head is signed by the registry's signing key on every mutation. Inclusion proofs are O(log n) in tree size.

Monitors (instances, public auditors) poll the entries endpoint to watch for unauthorized key bindings. An instance can optionally run a background task (alongside the heartbeat) that checks whether any of its members' registry accounts have unexpected key additions.

### 2.3.2 Identity Bindings and Noun Resolution

- `POST /api/v1/accounts/:id/identity-bindings` — link an external identity (GitHub OAuth, Google OAuth, email verification)
- `GET /api/v1/accounts/:id/identity-bindings` — list bindings
- `DELETE /api/v1/accounts/:id/identity-bindings/:id` — revoke binding
- `GET /api/v1/accounts/by-identity?provider=X&subject=Y` — resolve a noun to account + pubkeys + attestation

Identity bindings are the core of the noun model. The registry attests "pubkey A is bound to github:foo" by signing the binding. Instances verify the attestation at invite time.

Supported providers in M2: `github` (OAuth), `email` (verification link). Google and other OIDC providers are added in M5 alongside enterprise SSO.

Post-registration hook: when a new identity binding is created, check `pending_invites` for matching `(provider, subject)`. If found, resolve the pending invite and mark it for delivery via the next heartbeat to the originating instance.

### 2.4 Instance Registration and Directory

- `POST /api/v1/instances` — register instance (authenticated by NodeId proof)
- `POST /api/v1/instances/heartbeat` — periodic health check (scoped blocklist deltas in response)
- `GET /api/v1/instances` — public directory
- `GET /api/v1/instances/by-slug/:slug` — single instance lookup
- `PATCH /api/v1/instances/:id` — update metadata (authenticated)

### 2.5 Registry-Mediated Invites

- `POST /api/v1/invites` — register an invite, get short-code
- `GET /api/v1/invites/:short_code` — resolve invite metadata
- `GET /join/:short_code` — user-facing redirect to instance with invite token

### 2.5.1 Noun-Based Invite Endpoints

- `POST /api/v1/invites/by-noun` — resolve noun, create pending invite if unresolved
- `GET /api/v1/invites/pending?instance_id=X` — list pending noun invites for an instance
- `DELETE /api/v1/invites/pending/:id` — cancel pending invite

Heartbeat response gains a `resolved_invites` array: when a pending invite resolves (the person signed up and linked the matching identity), the resolved account and pubkeys are included in the next heartbeat to the originating instance.

### 2.6 Public Profile Pages (HTML)

- `GET /@:handle` — maud-rendered profile page (display name, avatar, public instances, active key fingerprints)
- `GET /d/:slug` — directory entry page for a public instance

Minimal HTML. Server-rendered. No JS required for viewing.

### 2.7 Deployment

- Single binary, OCI container image (reuse existing Bazel OCI rules)
- SQLite database file (persistent volume)
- Let's Encrypt TLS via reverse proxy (caddy or similar)
- Deploy to a single small VM

### 2.8 Rate Limiting

In-memory token bucket per IP (and per account for key management). No external dependencies. Reset on restart (acceptable at this traffic level).

**Estimated size:** ~3200 lines Rust + ~200 lines HTML templates (includes Merkle tree ~500 LOC, identity bindings + noun resolution + pending invites ~400 LOC)

---

## Milestone 3: Instance <-> Registry Integration

**Goal:** Instances can register with the registry, resolve handles (including multi-key accounts), and sync scoped blocklists.

**Depends on:** Milestone 1, Milestone 2

### 3.1 Registry Client

New file: `packages/crab_city/src/registry_client.rs`

A lightweight HTTP client (using `reqwest`) that talks to `crabcity.dev`:

- `register_instance(config) -> Result<RegistrationToken>`
- `heartbeat(token, status) -> Result<HeartbeatResponse>`
- `resolve_handle(handle) -> Result<Option<AccountInfo>>`
- `resolve_key(public_key) -> Result<Option<AccountInfo>>` — response includes `account_id` and all active keys
- `resolve_noun(noun) -> Result<NounResolution>` — resolve a noun to account + pubkeys + attestation
- `create_noun_invite(instance_id, noun, capability, fingerprint) -> Result<NounInviteResult>` — resolve or create pending
- `list_pending_noun_invites(instance_id) -> Result<Vec<PendingNounInvite>>`
- `cancel_pending_noun_invite(invite_id) -> Result<()>`
- `register_invite(token, invite) -> Result<ShortCode>`

### 3.2 Heartbeat Background Task

Spawn a tokio task on instance startup (if registry URL is configured):
- Send heartbeat every 5 minutes
- Process scoped blocklist deltas from response (global + per-org)
- Update `blocklist_cache` table
- On blocklist add: check if any active members match; if so, transition their grant to `suspended` and log `member.suspended` event
- Process `resolved_invites` from response: for each resolved noun invite, create a standard signed invite for the resolved pubkey, log `invite.noun_resolved` event, broadcast notification to admins
- Log warnings for MOTD

### 3.3 Handle Resolution

When displaying a member, if `handle` is NULL:
- Check local cache (a simple in-memory LRU, 1000 entries, 1-hour TTL)
- If miss: `resolve_key(public_key)` via registry client
- Cache result (including negative results, shorter TTL)
- Update `member_identities` row with resolved handle and `registry_account_id`
- If registry response shows multiple keys for the same account, update all matching identity rows

### 3.3.1 Noun-Based Invite Operations (Instance-Side)

New iroh RPC handlers on the instance (requires registry integration):

- `InviteByNoun` — resolve noun via registry, create invite or register pending. Requires `members:invite` access.
- `ListPendingNouns` — list pending noun invites for admin visibility. Requires `members:read` access.

TUI command: `/invite github:foo collaborate` or `/invite @blake admin` — parses noun, sends RPC message, displays result.

### 3.4 Configuration

Add to instance config:

```toml
[registry]
url = "https://crabcity.dev"    # omit to disable registry features
api_token = "..."                # from instance registration
heartbeat_interval_secs = 300
```

### 3.5 Blocklist Enforcement

Blocklist enforcement is a state transition, not a runtime check:
- When heartbeat delivers a blocklist add for a pubkey that has an active grant → transition to `suspended`
- When heartbeat delivers a blocklist remove for a pubkey that has a suspended grant (and `member.suspended` event shows `source: "blocklist"`) → transition to `active`
- Local blocklist adds also transition grants to `suspended`
- Auth middleware only checks `state == active` — no blocklist table joins

**Estimated size:** ~800 lines Rust (includes noun invite endpoints + resolved invite processing in heartbeat)

---

## Milestone 4: OIDC Provider (crabcity.dev)

**Goal:** `crabcity.dev` can issue OIDC tokens so instances can offer "Sign in with Crab City."

**Depends on:** Milestone 2

### 4.1 OIDC Discovery

Implement standard endpoints:
- `GET /.well-known/openid-configuration` — discovery document
- `GET /.well-known/jwks.json` — public signing keys

### 4.2 Signing Key Management

- Generate ed25519 signing keypair on first run
- Store encrypted in `signing_keys` table
- JWKS endpoint serves all active public keys
- Key rotation: scheduled task, new key every 90 days, old key valid for 180 days

### 4.3 Authorization Endpoint

`GET /oidc/authorize` — standard OIDC auth code flow:
- Validate `client_id` (instance NodeId), `redirect_uri`, `scope`, `state`, `nonce`
- If user not logged in → show login page (registry auth or redirect to enterprise IdP)
- If user logged in → issue auth code, redirect to instance

### 4.4 Token Endpoint

`POST /oidc/token` — exchange auth code for tokens:
- Validate `code`, `client_id`, `client_secret` (instance API token), `redirect_uri`
- Issue `id_token` (JWT signed with ed25519) + `access_token`
- Claims include: `sub`, `public_key`, `handle`, `org`, `org_role`, `capability`

### 4.5 Instance as OIDC Relying Party

Add to `crab_city`:
- `GET /api/auth/oidc/login` — initiate OIDC flow (redirect to `crabcity.dev/oidc/authorize`)
- `POST /api/auth/oidc/callback` — handle redirect, exchange code, extract claims, create identity + grant. Browser then connects via iroh WASM.

Configuration:
```toml
[registry.oidc]
client_secret = "..."            # issued during instance registration
```

### 4.6 Frontend: "Sign in with Crab City" Button

Add a button on the login page. Redirects to `/api/auth/oidc/login`. The rest is server-side redirects.

**Estimated size:** ~1500 lines Rust (OIDC is fiddly)

**Crate dependencies (registry):**
- `jsonwebtoken` (JWT creation)
- `openidconnect` (OIDC RP functionality for enterprise SSO, milestone 5)

---

## Milestone 5: Enterprise SSO

**Goal:** Enterprise orgs can configure their IdP (Okta/Entra/Google Workspace) so their employees can SSO into crab city instances.

**Depends on:** Milestone 4

### 5.1 Org Management API

- `POST /api/v1/orgs` — create org
- `PATCH /api/v1/orgs/:slug` — update org (name, OIDC config, quotas)
- `GET /api/v1/orgs/:slug` — org details
- `POST /api/v1/orgs/:slug/members` — add member
- `DELETE /api/v1/orgs/:slug/members/:account_id` — remove member
- `POST /api/v1/orgs/:slug/instances` — bind instance to org
- `DELETE /api/v1/orgs/:slug/instances/:instance_id` — unbind

### 5.2 Enterprise OIDC RP Flow

When a user logs in via `crabcity.dev` and their account (or email domain) is associated with an org that has OIDC configured:

1. `crabcity.dev` redirects to enterprise IdP
2. User authenticates with corporate credentials
3. Enterprise IdP redirects back to `crabcity.dev/oidc/enterprise/callback`
4. `crabcity.dev` maps `(issuer, subject)` → account (auto-provision if new)
5. Add to org membership if not already a member
6. Continue with the normal crabcity.dev OIDC provider flow (issue token to instance)

### 5.2.1 Enterprise Identity Bindings

Enterprise SSO creates identity bindings automatically: when a user authenticates via their corporate IdP, the registry creates a `google` (or `okta`, `entra`) identity binding for the account. This means enterprise users are immediately noun-resolvable by their corporate email: `google:alice@acme.com`.

Org admins can invite by corporate email before the employee has ever logged in: `/invite google:alice@acme.com`. The invite becomes pending and resolves when Alice first authenticates via SSO (which auto-provisions her account and creates the identity binding).

### 5.3 Auto-Provisioning

When an enterprise user SSOs for the first time:
- Create account (generate keypair server-side, or prompt user to provide one)
- Add to org
- Handle derivation: first part of email, or IdP `preferred_username` claim

### 5.4 Org Admin UI

Minimal admin pages on `crabcity.dev` (server-rendered HTML):
- Org settings (OIDC configuration, display name)
- Member list (with roles)
- Instance bindings (which instances, what default capability)

**Estimated size:** ~1200 lines Rust + ~400 lines HTML templates

---

## Milestone 6: Blocklists and Moderation

**Goal:** Global and org-level blocklists, distributed to instances via scoped heartbeat deltas.

**Depends on:** Milestone 3

### 6.1 Registry Blocklist Management

- `POST /api/v1/blocklist` — add entry (registry admin only)
- `DELETE /api/v1/blocklist/:id` — remove entry
- `GET /api/v1/blocklist` — full list
- `GET /api/v1/blocklist/delta?since_version=N` — delta

### 6.2 Org Blocklist Management

- `POST /api/v1/orgs/:slug/blocklist` — add entry (org admin)
- `DELETE /api/v1/orgs/:slug/blocklist/:id` — remove
- `GET /api/v1/orgs/:slug/blocklist` — full list
- `GET /api/v1/orgs/:slug/blocklist/delta?since_version=N` — delta

### 6.3 Distribution via Heartbeat

Already implemented in Milestone 3 heartbeat handler. This milestone adds:
- Org-scoped blocklist deltas in heartbeat response (for org-bound instances)
- Instance-side enforcement: blocklist add → suspend matching grants; blocklist remove → reinstate (if original suspension was blocklist-sourced)

### 6.4 Delisting

If a registry admin blocks an instance:
- Instance disappears from public directory
- Heartbeat response includes `{ "blocked": true, "reason": "..." }`
- Instance can still operate, but no longer appears in discovery

**Estimated size:** ~400 lines Rust

---

## Milestone 7: Iroh Invite Discovery

**Goal:** Invites can be **discovered and exchanged** peer-to-peer via iroh — no URL, no side-channel, no registry. Two devices on the same network (or reachable via iroh relay) find each other automatically.

Note: iroh as the primary *transport* for native client connections is already in M1. This milestone adds the **discovery** layer — advertising invites for nearby peers to find.

**Depends on:** Milestone 1

### 7.1 Invite Advertisement

Instance-side: publish a short-lived iroh document containing the invite token.

- Discovery via mDNS (local network) and iroh DHT (remote)
- Document contains: instance name, inviter fingerprint, capability, and the signed invite blob
- Document has a TTL (configurable, default 15 minutes) and self-destructs after redemption or expiry
- Multiple concurrent advertisements supported (one per active invite)

### 7.2 Invite Discovery (CLI/TUI)

Client-side: `crabcity join --discover`

- Scan for advertised invites via mDNS and iroh DHT
- Present discovered invites to the user: instance name, inviter, capability
- User selects one → client redeems the invite directly via iroh stream (M1 already supports invite redemption over iroh)

### 7.3 Invite Discovery (Web)

Browser-based discovery is harder (no mDNS, no raw iroh). Two options:

- **Option A (recommended):** Skip browser discovery. The web flow uses URLs. Iroh discovery is a CLI/TUI-only feature.
- **Option B (future):** WebRTC-based discovery via iroh relay. Requires iroh-web SDK maturity.

### 7.4 TUI `/invite` Command

New TUI command: `/invite --discover` or `/invite --nearby`

- Creates a temporary invite and advertises it
- Shows discovered peers as they appear
- Admin confirms each peer before the invite is delivered

**Estimated size:** ~400 lines Rust (iroh advertisement + discovery + TUI command — transport already in M1)

---

## Dependency Graph

```
M0: Foundations
 +-- M1: Instance-Local Auth (iroh)
 |    +-- M3: Instance <-> Registry Integration
 |    |    +-- M6: Blocklists
 |    +-- M7: Iroh Invite Discovery
 +-- M2: Registry Core (with multi-device keys + key transparency)
      +-- M3: Instance <-> Registry Integration
      +-- M4: OIDC Provider
           +-- M5: Enterprise SSO
```

## Build Order

Milestones can be parallelized where dependencies allow:

```
Phase 1:  M0 (foundations)              <- everything depends on this
Phase 2:  M1 + M2 (in parallel)        <- instance auth + registry core (with keys + transparency)
Phase 3:  M3 + M4 (in parallel)        <- integration + OIDC
Phase 4:  M5 + M6 + M7 (in parallel)   <- enterprise + moderation + iroh invites
```

## Total Estimated Scope

| Milestone | Rust LOC | Frontend LOC | Test LOC | Notes |
|-----------|----------|-------------|----------|-------|
| M0: Foundations | ~1200 | -- | ~500 | Shared crate, capability algebra, identity proofs, noun types, formal model, property tests, fuzz |
| M1: Instance Auth (iroh) | ~2000 | ~1400 | ~800 | iroh transport layer, embedded relay, iroh RPC handlers, iroh WASM browser integration, multi-instance connection manager, reconnection, idempotency, structured errors, QR codes, observability |
| M2: Registry Core | ~3200 | ~200 | ~400 | Multi-device keys, key transparency Merkle tree, identity bindings, noun resolution, pending invites |
| M3: Integration | ~800 | ~50 | ~150 | HTTP client + background task + noun invites + resolved invite delivery + TUI /invite noun command |
| M4: OIDC Provider | ~1500 | ~50 | ~200 | Fiddly but well-defined |
| M5: Enterprise SSO | ~1300 | ~400 | ~100 | Mostly registry-side, enterprise identity bindings |
| M6: Blocklists | ~400 | -- | ~100 | Scoped CRUD + delta sync |
| M7: Iroh Invite Discovery | ~400 | -- | ~100 | Iroh advertisement + discovery + TUI command (transport already in M1) |
| **Total** | **~10800** | **~2100** | **~2250** | |

## Risk Register

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| OIDC spec compliance edge cases | High | Medium | Use `openidconnect` crate, don't hand-roll. Test against real Okta tenant early in M4, not at the end. |
| Ed25519 in browser (Web Crypto) | Medium | Medium | Fallback: `@noble/ed25519` polyfill. Verify browser support matrix before M1 frontend work. |
| **iroh WASM maturity** | **Medium** | **High** | iroh WASM (0.33+) compiles and works in browsers but is relay-only. The API surface is newer than the native API. Mitigate: test early, pin iroh version, maintain a thin abstraction layer in the SvelteKit app so the iroh-wasm binding can be swapped if needed. |
| **Embedded relay complexity** | **Medium** | **Medium** | Running an in-process iroh relay server adds a new subsystem to each instance. Mitigate: the `iroh-relay` crate is maintained by n0 and designed for embedding. Start with the default config, add tuning knobs later. |
| Key backup UX ignored by users | High | High | Blocking modal with checkbox. Cannot be dismissed. Download + clipboard options. |
| Key loss for browser-only users | Medium | High | Blocking modal in M1. Multi-device keys in M2 reduce blast radius. Replace flow in M1 for recovery. |
| SQLite concurrent writes on registry | Low | Low | WAL mode + single-writer. Traffic is negligible. |
| Invite token size too large for some channels | Low | Low | 254 chars base32 fits in any medium. Registry short-codes are 8 chars. |
| OIDC key rotation disrupts active sessions | Low | Medium | Overlap window (2 active keys). Instance JWKS cache TTL = 1 hour. |
| Double-hop OIDC flow (enterprise) bugs | High | Medium | Both hops use `openidconnect` crate. Test against real IdPs early. |
| Blocklist propagation delay (5 min) | Low | Low | Documented as known property. Acceptable at scale. |
| Delegation chain token size | Low | Low | 3-hop chain is ~660 chars base32. Fits in URLs. Use registry short-codes for longer chains. |
| Delegation chain forgery | Low | High | Each link signed independently. Chain verification walks root-to-leaf. Property-tested and fuzzed. |
| Hash chain performance on large event logs | Low | Low | SHA-256 is fast. Chain verification is sequential but only needed for auditing, not hot path. |
| Merkle tree implementation correctness | Medium | High | Use RFC 6962 algorithm. Property-test inclusion proofs. Consider using an existing crate (`merkle-log`). |
| iroh mDNS discovery reliability | Medium | Low | Fallback to URL-based invites. iroh-native exchange is a convenience, not the primary path. |
| Key transparency log growth | Low | Low | One entry per key mutation. At projected scale, this grows by single-digit entries per day. |
| Preview WebSocket information leakage | Low | Medium | Strict allowlist: terminal count, cursor position (not content), user count, uptime. Code review the preview stream. |
| Reconnection ring buffer memory | Medium | Medium | 1000 messages in a global broadcast log. At 10K concurrent connections with average message size 500 bytes, that's ~500KB (shared buffer, per-client cursors). Monitor `crabcity_snapshots_total`. |
| iroh connection scalability | Medium | Medium | Each iroh QUIC connection consumes a file descriptor and memory for crypto state. Browser clients via embedded relay share a single relay listener but get individual iroh connections. At 10K concurrent clients, ~100MB. Monitor `crabcity_iroh_connections_active`. |
| Multi-instance connection management | Low | Medium | N simultaneous iroh connections from a single client. Memory and CPU scale linearly. Default cap of 10 concurrent instances. Background instances receive broadcasts but don't render — lower CPU than active instance. |
| Identity proof trust model confusion | Medium | Medium | Proofs are assertions, not guarantees. Document clearly. UI should display proof status as "claims" not "verified." Full verification requires contacting the remote instance. |
| Noun resolution depends on registry availability | Medium | Low | Noun-based invites require the registry. Raw keypair invites always work as fallback. Instance caches resolved nouns for display. |
| Stale identity bindings | Medium | Medium | A person may unlink their GitHub account but the registry still holds the binding. Mitigate: bindings have `revoked_at`, periodic re-verification (future), and the attestation includes a timestamp so instances can assess freshness. |
| Pending invite spam | Low | Low | Rate-limit noun invite creation per instance. Pending invites have a default 30-day TTL. Instances can cancel pending invites. |

## Resolved Questions

1. **Should the registry be a separate binary or a mode of `crab_city`?** **Decision: separate binary**, shared `crab_city_auth` crate. Different deployment, different config, different security profile. The shared crate keeps crypto types in sync.

2. **Browser keypair UX.** **Decision: blocking key backup modal in M1.** Copy-to-clipboard + download `.key` file + "I saved my key" checkbox. Cannot be dismissed. Multi-device keys (M2) reduce the blast radius of key loss. Replace flow (M1) handles recovery.

3. **Instance identity for OIDC.** `client_id` is the NodeId (public, deterministic). `client_secret` is issued at registration. If an instance re-installs and gets a new NodeId, it must re-register at the registry. The old registration becomes orphaned (instance won't heartbeat, marked offline). No migration path — new NodeId means new identity. This is the same model as SSH host keys.

4. **Subdomain routing.** Out of scope. No `<slug>.crabcity.dev` subdomains. Instances have their own hostnames or are accessed by IP.

5. **Loopback identity.** **Decision: synthetic all-zeros sentinel pubkey.** Not a real keypair. Cannot be used remotely. Always has `owner` grant. Preserves backward compatibility.

6. **Multi-device timing.** **Decision: day-one feature in M2**, not deferred. `account_keys` table is part of the initial registry schema. Users can add devices from the start.

7. **Key transparency timing.** **Decision: day-one feature in M2.** All key mutations go through a single codepath that appends to the transparency log. Designing this in later would require backfilling. The Merkle tree implementation is ~500 LOC and the API surface is 3 endpoints.

8. **Invite delegation.** **Decision: design the chain format in M0, implement in M1.** The `InviteLink` struct and chain verification are part of `crab_city_auth`. Flat invites are a chain of length 1 — no special-casing needed. `max_depth=0` disables delegation for simple use cases.

9. **Hash-chained event log.** **Decision: implement in M0 (types) + M1 (storage).** The `prev_hash` and `hash` fields are part of the `Event` struct in `crab_city_auth`. Computing the hash on every event insert is negligible overhead. Signed checkpoints are optional (configurable interval, default every 100 events).

10. **Capability algebra.** **Decision: four operations in `crab_city_auth`, property-tested.** `intersect`, `contains`, `is_superset_of`, `diff`. No code outside the crate performs ad-hoc access rights manipulation. Eliminates authorization logic inconsistencies.

11. **Reconnection.** **Decision: sequence numbers + bounded ring buffer.** Server assigns monotonic `seq` to each message. Client sends `last_seq` on reconnect. Server replays from ring buffer or sends full snapshot. Connection drops are invisible to users. Same mechanism for native and browser clients (both use iroh).

12. **Error recovery.** **Decision: structured `recovery` field on all error messages.** Closed enum of recovery actions: `reconnect`, `retry`, `contact_admin`, `redeem_invite`, `none`. Client parses into typed actions. No generic error screens.

13. **QR code invites.** **Decision: implement in M1.** Flat invites (256 chars) and delegated invites (660 chars) both fit in QR codes. TUI renders with Unicode half-blocks. Web UI renders as SVG. Invite response includes `qr_data` field.

14. **Cross-instance identity proofs.** **Decision: implement in M0 (types) + M1 (exchange).** Self-issued signed statements linking keys across instances. Assertions, not guarantees. The missing interconnect primitive.

15. **Formal state machine verification.** **Decision: TLA+ or Alloy model in M0.** The membership state machine is small enough to verify exhaustively. Generate test cases from the model.

16. **Observability.** **Decision: day-one in M1.** Prometheus metrics endpoint, structured logging for all auth decisions and state transitions, OpenTelemetry trace context propagation for registry communication.

17. **Noun-based invites: where does resolution happen?** **Decision: registry resolves, instance consumes.** The registry is the phonebook that maps nouns (GitHub usernames, emails, handles) to accounts and pubkeys. Instances call the registry at invite time to resolve nouns. Grants remain purely pubkey-based — nouns are an invite-time convenience, not a runtime concept. Pending invites (for people not yet on crabcity) live at the registry and are delivered via heartbeat when the person signs up.

18. **Identity bindings: trust model.** **Decision: registry-attested.** The registry signs identity bindings (e.g., "pubkey A is bound to github:foo"). Instances verify the attestation signature at invite time. The binding is established via OAuth/OIDC flows, not self-asserted. This is "pseudo-trustable" — as trustworthy as the OAuth provider and the registry's signing key.

19. **Transport: iroh everywhere.** **Decision: iroh is the ONLY transport for all clients.** Ed25519 keypairs are iroh NodeIds (same curve). The iroh handshake proves key ownership and establishes E2E encryption — no separate auth protocol needed. Native clients connect via QUIC; browser clients connect via iroh WASM through the instance's embedded relay. One transport, one auth model. No session tokens, no refresh tokens, no challenge-response, no revocation set. Massive simplification over dual-transport.

20. **Multi-instance connections.** **Decision: M1.** A native client holds N simultaneous iroh connections. One is "active" (receives input), others are background (presence, notifications). Instance switcher is a keybinding. Browser clients use the same model via iroh WASM.

## Open Questions

1. **Envelope versioning for existing messages.** The current protocol doesn't use envelope versioning. Wrapping existing messages (`StateChange`, `TaskUpdate`, etc.) in `{ "v": 1, "seq": N, "type": ..., "data": ... }` is a breaking change for connected clients. Recommend: cut over all at once in M1 — M1 is already a breaking change (auth required), so bundle the wire format change and sequence number addition.

2. **Merkle tree crate vs hand-roll.** The transparency log needs a basic append-only Merkle tree (RFC 6962). Options: (a) use an existing crate like `merkle-log`, (b) hand-roll ~500 LOC. The algorithm is simple and well-specified, but correctness is critical. Recommend: hand-roll with extensive property tests, since the dependency surface should be minimal for a security-critical component.

3. **Preview WebSocket scope creep.** The preview stream is intentionally minimal (cursor positions, user count, no content). Need to resist pressure to add "just one more signal" — every addition is a potential information leak. The allowlist should be defined in a single struct and adding a field should be a compile-time decision that forces review.

4. **Delegation chain depth limits.** The design allows `max_depth` up to 255 (u8). In practice, chains deeper than 3-4 are hard to reason about. Should there be a global cap (e.g., max 5 hops)? Or leave it to the invite creator? Recommend: configurable per-instance cap, default 3.

5. **Iroh-native invite discovery UX.** The discovery flow requires user confirmation on both sides (inviter accepts peer, invitee accepts invite). Should there be a "promiscuous" mode where the instance auto-accepts all discovered peers? Useful for workshops/demos, dangerous for production. Recommend: require explicit confirmation, but allow a `--auto-accept` flag for ephemeral instances.

6. **Formal verification tooling.** TLA+ has the largest community and tooling (TLC model checker). Alloy is more concise for relational models. Recommend TLA+ for the state machine (it's the standard for distributed systems verification), but this is a judgment call based on team familiarity.

7. **Noun provider extensibility.** The initial noun vocabulary is `@handle`, `github:`, `google:`, `email:`. Should there be a generic `oidc:<issuer>:<subject>` noun for arbitrary OIDC providers? Or limit to explicitly supported providers? Recommend: start with the four known providers, add `oidc:` as a generic fallback in M5 when enterprise SSO is implemented.

8. **Pending invite expiry and cleanup.** Pending noun invites could sit at the registry indefinitely if the person never signs up. Options: (a) mandatory expiry (configurable, default 30 days), (b) no expiry but periodic admin notification, (c) instance can cancel via API. Recommend (a) + (c) — default 30-day TTL, admin can cancel anytime, expired invites cleaned up by background sweep.

9. **iroh QUIC stream multiplexing strategy.** A single iroh connection can carry multiple QUIC streams. Options: (a) one bidirectional stream for the message protocol (simple, ordered), (b) separate streams per logical channel (messages, file transfer, terminal data — allows independent flow control), (c) start with (a), evolve to (b). Recommend (c) — one stream is sufficient for the current message protocol; multiple streams become valuable when adding large file transfers.

10. **iroh WASM bundle size and startup time.** The iroh WASM build includes QUIC crypto, relay client, and ed25519. Need to measure: (a) compressed bundle size (target: <500KB), (b) cold start time to first connection (target: <2s), (c) memory footprint in browser. If any target is missed, consider lazy-loading the WASM module after the join page renders.

11. **Embedded relay configuration.** The in-process iroh relay needs: (a) a WebSocket endpoint path (`/relay`), (b) TLS termination for remote connections (handled by reverse proxy), (c) rate limiting (separate from the main iroh connection limits). Should the relay be always-on or started on-demand when the first browser client connects? Recommend: always-on (simpler, negligible resource cost when idle).

12. **Instance switcher UX.** Multi-instance switching in the TUI. Options: (a) numbered keybindings (Ctrl+1/2/3), (b) fuzzy-search picker (like tmux window switcher), (c) both. Recommend (c) — keybindings for quick access, picker for discovery when you have many instances.
