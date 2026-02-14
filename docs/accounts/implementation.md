# Crab City Accounts: Implementation Plan

## Overview

This plan breaks the account system into deliverable milestones. Each milestone is independently shippable and provides user-facing value. The system builds from the bottom up: instance-local auth first, registry second, enterprise features last.

## Milestone 0: Foundations

**Goal:** Shared crate with cryptographic types, invite token format, permissions model, and key fingerprints.

**Deliverables:**
- [ ] New crate: `packages/crab_city_auth/`
- [ ] Ed25519 types wrapping `ed25519-dalek`: `PublicKey`, `SigningKey`, `Signature`
- [ ] `PublicKey::fingerprint() -> String` — `crab_` + first 8 chars of Crockford base32
- [ ] `PublicKey::LOOPBACK` — the all-zeros sentinel constant
- [ ] `Capability` enum: `View`, `Collaborate`, `Admin`, `Owner` with `Ord` impl
- [ ] `Permissions` bitflags: `VIEW_CONTENT`, `VIEW_TERMINALS`, `SEND_CHAT`, `EDIT_TASKS`, `TERMINAL_INPUT`, `CREATE_INSTANCE`, `MANAGE_MEMBERS`, `MANAGE_INSTANCE`
- [ ] `Capability::permissions() -> Permissions` — expand preset to bitfield
- [ ] `MembershipState` enum: `Invited`, `Active`, `Suspended`, `Removed`
- [ ] `Invite` struct with serialize/deserialize (binary format, 158 bytes)
- [ ] `Invite::sign()` and `Invite::verify()` methods
- [ ] Base32 encoding/decoding (Crockford) for invite tokens
- [ ] Challenge-response protocol types: `Challenge`, `ChallengeResponse`
- [ ] Structured challenge payload: `"crabcity:auth:v1:" ++ nonce ++ node_id ++ timestamp`
- [ ] `EventType` enum and `Event` struct for the audit log
- [ ] Unit tests for all crypto operations, round-trip serialization, permission expansion, state transitions

**Crate dependencies:**
- `ed25519-dalek` (with `rand_core` feature)
- `data-encoding` (for Crockford base32)
- `bitflags` (for permissions)
- `serde` (for JSON API types)
- `uuid` (v7)

**Build system:**
- Add `crab_city_auth` to `Cargo.toml` workspace members
- Add to `MODULE.bazel` as a local crate
- Wire into `//tools/format`

**Estimated size:** ~800 lines of Rust + tests

---

## Milestone 1: Instance-Local Auth

**Goal:** Instances can create invites, redeem them, and authenticate users via challenge-response. This milestone replaces the implicit "everyone is authenticated" model with real membership. Includes the join page, key backup modal, event log, and integration tests.

**Depends on:** Milestone 0

### 1.1 Database Migrations

Add to `crab_city`'s SQLite migration system:

```
migrations/
  NNNN_create_member_identities.sql
  NNNN_create_member_grants.sql
  NNNN_create_invites.sql
  NNNN_create_sessions.sql
  NNNN_create_blocklist.sql
  NNNN_create_event_log.sql
  NNNN_seed_loopback_identity.sql
```

Tables: `member_identities`, `member_grants`, `invites`, `sessions`, `blocklist`, `blocklist_cache`, `event_log` (see design doc section 2.1).

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
- `update_grant_capability(db, public_key, capability, permissions) -> Result<()>`
- `update_grant_permissions(db, public_key, permissions) -> Result<()>`
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

New file: `packages/crab_city/src/repository/sessions.rs`

Functions:
- `create_session(db, token_hash, public_key, expires_at) -> Result<()>`
- `get_session(db, token_hash) -> Result<Option<Session>>`
- `delete_session(db, token_hash) -> Result<()>`
- `extend_session(db, token_hash, new_expires_at) -> Result<()>`
- `cleanup_expired_sessions(db) -> Result<u64>`

New file: `packages/crab_city/src/repository/event_log.rs`

Functions:
- `log_event(db, event) -> Result<i64>`
- `query_events(db, filter) -> Result<Vec<Event>>`

### 1.3 Handler Layer

New file: `packages/crab_city/src/handlers/auth.rs`

Endpoints:
- `POST /api/auth/challenge` — generate nonce, store pending challenge (in-memory)
- `POST /api/auth/verify` — verify structured signature, check grant state, create session
- `DELETE /api/auth/session` — logout (revoke session)

New file: `packages/crab_city/src/handlers/invites.rs`

Endpoints:
- `POST /api/invites` — create invite (requires `MANAGE_MEMBERS`)
- `POST /api/invites/redeem` — redeem invite token, create identity + grant + session
- `GET /api/invites` — list active invites (requires `MANAGE_MEMBERS`)
- `POST /api/invites/revoke` — revoke invite, optionally suspend derived members

New file: `packages/crab_city/src/handlers/members.rs`

Endpoints:
- `GET /api/members` — list members (requires `VIEW_CONTENT`)
- `PATCH /api/members/:public_key` — update capability (requires `MANAGE_MEMBERS`)
- `PATCH /api/members/:public_key/permissions` — tweak permission bits
- `DELETE /api/members/:public_key` — remove member (requires `MANAGE_MEMBERS`)
- `POST /api/members/:public_key/suspend` — suspend member
- `POST /api/members/:public_key/reinstate` — reinstate member
- `POST /api/members/:public_key/replace` — link new grant to old (key loss recovery)
- `GET /api/events` — query event log (requires `MANAGE_MEMBERS`)

### 1.4 Auth Middleware Update

Modify: `packages/crab_city/src/auth.rs`

Updated auth chain:
1. Loopback bypass → synthetic owner identity (all-zeros pubkey, `owner` grant)
2. Check `Authorization: Bearer <token>` header → SHA-256 hash → lookup sessions table → lookup grant (state == active)
3. Check `__crab_session` cookie → same lookup
4. No credentials → 401

The middleware populates `AuthUser` with both identity and grant, including the `permissions` bitfield. Handlers call `auth.require_permission(Permissions::EDIT_TASKS)?`.

### 1.5 Instance Bootstrap

On first startup, if the `member_identities` table has only the loopback sentinel:
- Generate an instance identity keypair (stored in config dir)
- Log the owner invite token to stdout

### 1.6 Frontend Changes

Modify: `packages/crab_city_ui/`

**Join page** (`/join` route):
- Extract invite token from `#fragment`
- Parse invite to show: instance name, inviter fingerprint, capability being granted
- "Your name" input (pre-filled if registry account detected)
- "Join" button
- If existing keypair for this instance: show "Welcome back, [name]. [Rejoin]"

**Key backup modal** (blocking, post-keygen):
- "Save your identity key" explanation
- Copy-to-clipboard button (base64 private key)
- Download `.key` file button
- "I saved my key" checkbox — required to proceed

**Other frontend changes:**
- Add keypair generation and IndexedDB storage
- Add login flow (challenge-response)
- Store session token as cookie
- Show current user identity + fingerprint in UI header
- Add member list panel (for admins, with state badges)
- Add invite creation UI (for admins)
- Add member management actions (suspend, reinstate, remove, change capability)

### 1.7 Integration Tests

New file: `packages/crab_city/tests/auth_integration.rs`

End-to-end tests that spin up an in-memory instance and exercise the security-critical paths:

- Generate keypair → create invite → redeem invite → verify session works → verify capabilities are enforced
- Challenge-response flow: generate, sign, verify, get session
- Permission enforcement: `collaborate` user cannot access `MANAGE_MEMBERS` endpoints
- State machine: active → suspended → reinstate → active; suspended → removed
- Invite revocation: revoke invite → unredeemed uses fail, existing members unaffected
- Invite revocation with `suspend_derived_members`: all derived grants suspended
- Key replacement: new key replaces old, old grant removed
- Loopback bypass: all-zeros pubkey → owner access on loopback, rejected remotely
- Event log: verify events recorded for all state transitions

**Estimated size:** ~1500 lines Rust (handlers + repo + middleware + event log) + ~1000 lines Svelte/TS (join page + key backup + member management) + ~400 lines integration tests

---

## Milestone 2: Registry Core (crabcity.dev)

**Goal:** A running registry at `crabcity.dev` where users can create accounts (with multi-device key management from day one) and instances can register for discovery.

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
    handlers/
      accounts.rs
      keys.rs
      instances.rs
      invites.rs
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

- `POST /api/v1/accounts/:id/keys` — add a new key (authenticated with existing key, proof from new key)
- `DELETE /api/v1/accounts/:id/keys/:public_key` — revoke a key (cannot revoke last active key)
- `GET /api/v1/accounts/:id/keys` — list active keys

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

**Estimated size:** ~2300 lines Rust + ~200 lines HTML templates

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
- `register_invite(token, invite) -> Result<ShortCode>`

### 3.2 Heartbeat Background Task

Spawn a tokio task on instance startup (if registry URL is configured):
- Send heartbeat every 5 minutes
- Process scoped blocklist deltas from response (global + per-org)
- Update `blocklist_cache` table
- On blocklist add: check if any active members match; if so, transition their grant to `suspended` and log `member.suspended` event
- Log warnings for MOTD

### 3.3 Handle Resolution

When displaying a member, if `handle` is NULL:
- Check local cache (a simple in-memory LRU, 1000 entries, 1-hour TTL)
- If miss: `resolve_key(public_key)` via registry client
- Cache result (including negative results, shorter TTL)
- Update `member_identities` row with resolved handle and `registry_account_id`
- If registry response shows multiple keys for the same account, update all matching identity rows

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

**Estimated size:** ~600 lines Rust

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
- If user not logged in → show login page (challenge-response or redirect to enterprise IdP)
- If user logged in → issue auth code, redirect to instance

### 4.4 Token Endpoint

`POST /oidc/token` — exchange auth code for tokens:
- Validate `code`, `client_id`, `client_secret` (instance API token), `redirect_uri`
- Issue `id_token` (JWT signed with ed25519) + `access_token`
- Claims include: `sub`, `public_key`, `handle`, `org`, `org_role`, `capability`

### 4.5 Instance as OIDC Relying Party

Add to `crab_city`:
- `GET /api/auth/oidc/login` — initiate OIDC flow (redirect to `crabcity.dev/oidc/authorize`)
- `GET /api/auth/oidc/callback` — handle redirect, exchange code, extract claims, create identity + grant + session

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

## Dependency Graph

```
M0: Foundations
 +-- M1: Instance-Local Auth
 |    +-- M3: Instance <-> Registry Integration
 |         +-- M6: Blocklists
 +-- M2: Registry Core (with multi-device keys)
      +-- M3: Instance <-> Registry Integration
      +-- M4: OIDC Provider
           +-- M5: Enterprise SSO
```

## Build Order

Milestones can be parallelized where dependencies allow:

```
Phase 1:  M0 (foundations)              <- everything depends on this
Phase 2:  M1 + M2 (in parallel)        <- instance auth + registry core (with keys)
Phase 3:  M3 + M4 (in parallel)        <- integration + OIDC
Phase 4:  M5 + M6 (in parallel)        <- enterprise + moderation
```

## Total Estimated Scope

| Milestone | Rust LOC | Frontend LOC | Test LOC | Notes |
|-----------|----------|-------------|----------|-------|
| M0: Foundations | ~800 | -- | ~200 | Shared crate, permissions, fingerprints |
| M1: Instance Auth | ~1500 | ~1000 | ~400 | Core auth UX, join page, event log |
| M2: Registry Core | ~2300 | ~200 | ~200 | New binary, multi-device keys |
| M3: Integration | ~600 | -- | ~100 | HTTP client + background task |
| M4: OIDC Provider | ~1500 | ~50 | ~200 | Fiddly but well-defined |
| M5: Enterprise SSO | ~1200 | ~400 | ~100 | Mostly registry-side |
| M6: Blocklists | ~400 | -- | ~100 | Scoped CRUD + delta sync |
| **Total** | **~8300** | **~1650** | **~1300** | |

## Risk Register

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| OIDC spec compliance edge cases | High | Medium | Use `openidconnect` crate, don't hand-roll. Test against real Okta tenant early in M4, not at the end. |
| Ed25519 in browser (Web Crypto) | Medium | Medium | Fallback: `@noble/ed25519` polyfill. Verify browser support matrix before M1 frontend work. |
| Key backup UX ignored by users | High | High | Blocking modal with checkbox. Cannot be dismissed. Download + clipboard options. |
| Key loss for browser-only users | Medium | High | Blocking modal in M1. Multi-device keys in M2 reduce blast radius. Replace flow in M1 for recovery. |
| SQLite concurrent writes on registry | Low | Low | WAL mode + single-writer. Traffic is negligible. |
| Invite token size too large for some channels | Low | Low | 254 chars base32 fits in any medium. Registry short-codes are 8 chars. |
| OIDC key rotation disrupts active sessions | Low | Medium | Overlap window (2 active keys). Instance JWKS cache TTL = 1 hour. |
| Double-hop OIDC flow (enterprise) bugs | High | Medium | Both hops use `openidconnect` crate. Test against real IdPs early. |
| Pending challenges lost on restart | Low | Low | In-memory only, 60s TTL. Users retry. Documented as design choice. |
| Blocklist propagation delay (5 min) | Low | Low | Documented as known property. Acceptable at scale. |

## Resolved Questions

1. **Should the registry be a separate binary or a mode of `crab_city`?** **Decision: separate binary**, shared `crab_city_auth` crate. Different deployment, different config, different security profile. The shared crate keeps crypto types in sync.

2. **Browser keypair UX.** **Decision: blocking key backup modal in M1.** Copy-to-clipboard + download `.key` file + "I saved my key" checkbox. Cannot be dismissed. Multi-device keys (M2) reduce the blast radius of key loss. Replace flow (M1) handles recovery.

3. **Instance identity for OIDC.** `client_id` is the NodeId (public, deterministic). `client_secret` is issued at registration. If an instance re-installs and gets a new NodeId, it must re-register at the registry. The old registration becomes orphaned (instance won't heartbeat, marked offline). No migration path — new NodeId means new identity. This is the same model as SSH host keys.

4. **Subdomain routing.** Out of scope. No `<slug>.crabcity.dev` subdomains. Instances have their own hostnames or are accessed by IP.

5. **Loopback identity.** **Decision: synthetic all-zeros sentinel pubkey.** Not a real keypair. Cannot be used remotely. Always has `owner` grant. Preserves backward compatibility.

6. **Multi-device timing.** **Decision: day-one feature in M2**, not deferred. `account_keys` table is part of the initial registry schema. Users can add devices from the start.

## Open Questions

1. **Envelope versioning for existing WebSocket messages.** The current WebSocket protocol doesn't use envelope versioning. Wrapping existing messages (`StateChange`, `TaskUpdate`, etc.) in `{ "v": 1, "type": ..., "data": ... }` is a breaking change for connected clients. Options: (a) cut over all at once in M1, (b) support both formats during a transition period, (c) version the WebSocket handshake protocol. Recommend (a) — M1 is already a breaking change (auth required), so bundle the wire format change.
