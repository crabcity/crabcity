# Crab City Accounts: Implementation Plan

## Overview

This plan breaks the account system into deliverable milestones. Each milestone is independently shippable and provides user-facing value. The system builds from the bottom up: instance-local auth first, registry second, enterprise features last.

## Milestone 0: Foundations

**Goal:** Shared crate with cryptographic types and invite token format.

**Deliverables:**
- [ ] New crate: `packages/crab_city_auth/`
- [ ] Ed25519 types wrapping `ed25519-dalek`: `PublicKey`, `SigningKey`, `Signature`
- [ ] `Capability` enum: `View`, `Collaborate`, `Admin`, `Owner` with `Ord` impl
- [ ] `Invite` struct with serialize/deserialize (binary format, 158 bytes)
- [ ] `Invite::sign()` and `Invite::verify()` methods
- [ ] Base32 encoding/decoding (Crockford) for invite tokens
- [ ] `Membership` struct (shared between instance and registry)
- [ ] Challenge-response protocol types: `Challenge`, `ChallengeResponse`
- [ ] Unit tests for all crypto operations and round-trip serialization

**Crate dependencies:**
- `ed25519-dalek` (with `rand_core` feature)
- `data-encoding` (for Crockford base32)
- `serde` (for JSON API types)
- `uuid` (v7)

**Build system:**
- Add `crab_city_auth` to `Cargo.toml` workspace members
- Add to `MODULE.bazel` as a local crate
- Wire into `//tools/format`

**Estimated size:** ~600 lines of Rust + tests

---

## Milestone 1: Instance-Local Auth

**Goal:** Instances can create invites, redeem them, and authenticate users via challenge-response. This milestone replaces the implicit "everyone is authenticated" model with real membership.

**Depends on:** Milestone 0

### 1.1 Database Migrations

Add to `crab_city`'s SQLite migration system:

```
migrations/
  NNNN_create_memberships.sql
  NNNN_create_invites.sql
  NNNN_create_sessions.sql
  NNNN_create_blocklist.sql
```

Tables: `memberships`, `invites`, `sessions`, `blocklist` (see design doc section 2.1).

### 1.2 Repository Layer

New file: `packages/crab_city/src/repository/auth.rs`

Functions:
- `create_membership(db, membership) -> Result<Membership>`
- `get_membership(db, public_key) -> Result<Option<Membership>>`
- `list_memberships(db) -> Result<Vec<Membership>>`
- `update_membership_capability(db, public_key, capability) -> Result<()>`
- `delete_membership(db, public_key) -> Result<()>`
- `create_invite(db, invite) -> Result<()>`
- `get_invite(db, nonce) -> Result<Option<StoredInvite>>`
- `increment_invite_use_count(db, nonce) -> Result<()>`
- `revoke_invite(db, nonce) -> Result<()>`
- `create_session(db, token_hash, public_key, expires_at) -> Result<()>`
- `get_session(db, token_hash) -> Result<Option<Session>>`
- `delete_session(db, token_hash) -> Result<()>`
- `cleanup_expired_sessions(db) -> Result<u64>`

### 1.3 Handler Layer

New file: `packages/crab_city/src/handlers/auth.rs`

Endpoints:
- `POST /api/auth/challenge` — generate nonce, store pending challenge
- `POST /api/auth/verify` — verify signature, create session
- `DELETE /api/auth/session` — logout (revoke session)

New file: `packages/crab_city/src/handlers/invites.rs`

Endpoints:
- `POST /api/invites` — create invite (requires `admin`+)
- `POST /api/invites/redeem` — redeem invite token
- `GET /api/invites` — list active invites (requires `admin`+)
- `DELETE /api/invites/:nonce` — revoke invite (requires `admin`+)

New file: `packages/crab_city/src/handlers/members.rs`

Endpoints:
- `GET /api/members` — list members (requires `view`+)
- `PATCH /api/members/:public_key` — update capability (requires `admin`+)
- `DELETE /api/members/:public_key` — remove member (requires `admin`+)

### 1.4 Auth Middleware Update

Modify: `packages/crab_city/src/middleware/` (existing auth middleware)

Add session token checking:
1. Loopback bypass (existing) → auto-create owner membership if none exists
2. Check `Authorization: Bearer <token>` header → hash, lookup in sessions table
3. Check `__crab_session` cookie → same lookup
4. No credentials → 401

The first loopback request auto-provisions an `owner` membership for the loopback user's keypair. This preserves backward compatibility: local access still "just works."

### 1.5 Instance Bootstrap

On first startup, if the memberships table is empty:
- Generate an instance identity keypair (stored in config dir)
- Log the owner invite token to stdout
- Optionally: auto-provision the first loopback-connected user as owner

### 1.6 Frontend Changes

Modify: `packages/crab_city_ui/`

- Add keypair generation and storage (IndexedDB)
- Add invite redemption flow (extract token from `#fragment`, call `/api/invites/redeem`)
- Add login flow (challenge-response against `/api/auth/challenge` + `/api/auth/verify`)
- Store session token in cookie
- Show current user identity in UI header
- Add member list panel (for admins)
- Add invite creation UI (for admins)

**Estimated size:** ~1200 lines Rust (handlers + repo + middleware) + ~800 lines Svelte/TS

---

## Milestone 2: Registry Core (crabcity.dev)

**Goal:** A running registry at `crabcity.dev` where users can create accounts and instances can register for discovery.

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
      instances.rs
      invites.rs
      blocklist.rs
    handlers/
      accounts.rs
      instances.rs
      invites.rs
      health.rs
    middleware/
      auth.rs
      rate_limit.rs
```

### 2.2 Account Registration

- `POST /api/v1/accounts` — register handle + public key + proof signature
- `GET /api/v1/accounts/by-handle/:handle` — public profile
- `GET /api/v1/accounts/by-key/:public_key` — reverse lookup
- `PATCH /api/v1/accounts/:id` — update display name, avatar (authenticated)

Handle validation: lowercase, alphanumeric + hyphens, 3-30 chars, no leading/trailing hyphens.

### 2.3 Instance Registration and Directory

- `POST /api/v1/instances` — register instance (authenticated by NodeId proof)
- `POST /api/v1/instances/heartbeat` — periodic health check
- `GET /api/v1/instances` — public directory
- `GET /api/v1/instances/by-slug/:slug` — single instance lookup
- `PATCH /api/v1/instances/:id` — update metadata (authenticated)

### 2.4 Registry-Mediated Invites

- `POST /api/v1/invites` — register an invite, get short-code
- `GET /api/v1/invites/:short_code` — resolve invite metadata
- `GET /join/:short_code` — user-facing redirect to instance with invite token

### 2.5 Public Profile Pages (HTML)

- `GET /@:handle` — maud-rendered profile page (display name, avatar, public instances)
- `GET /d/:slug` — directory entry page for a public instance

Minimal HTML. Server-rendered. No JS required for viewing.

### 2.6 Deployment

- Single binary, OCI container image (reuse existing Bazel OCI rules)
- SQLite database file (persistent volume)
- Let's Encrypt TLS via reverse proxy (caddy or similar)
- Deploy to a single small VM

### 2.7 Rate Limiting

In-memory token bucket per IP. No external dependencies. Reset on restart (acceptable at this traffic level).

**Estimated size:** ~2000 lines Rust + ~200 lines HTML templates

---

## Milestone 3: Instance ↔ Registry Integration

**Goal:** Instances can register with the registry, resolve handles, and sync blocklists.

**Depends on:** Milestone 1, Milestone 2

### 3.1 Registry Client

New file: `packages/crab_city/src/registry_client.rs`

A lightweight HTTP client (using `reqwest`) that talks to `crabcity.dev`:

- `register_instance(config) -> Result<RegistrationToken>`
- `heartbeat(token, status) -> Result<HeartbeatResponse>`
- `resolve_handle(handle) -> Result<Option<AccountInfo>>`
- `resolve_key(public_key) -> Result<Option<AccountInfo>>`
- `register_invite(token, invite) -> Result<ShortCode>`

### 3.2 Heartbeat Background Task

Spawn a tokio task on instance startup (if registry URL is configured):
- Send heartbeat every 5 minutes
- Process blocklist deltas from response
- Update `blocklist_cache` table
- Log warnings for MOTD

### 3.3 Handle Resolution

When displaying a member, if `handle` is NULL:
- Check local cache (a simple in-memory LRU, 1000 entries, 1-hour TTL)
- If miss: `resolve_key(public_key)` via registry client
- Cache result (including negative results, shorter TTL)
- Update membership row with resolved handle

### 3.4 Configuration

Add to instance config:

```toml
[registry]
url = "https://crabcity.dev"    # omit to disable registry features
api_token = "..."                # from instance registration
heartbeat_interval_secs = 300
```

### 3.5 Blocklist Enforcement

On the auth middleware path, after checking session validity:
- Check `blocklist` (local) for the user's public key
- Check `blocklist_cache` (global/org) for the user's public key
- If blocked → 403

**Estimated size:** ~500 lines Rust

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
- `GET /api/auth/oidc/callback` — handle redirect, exchange code, extract claims, create membership + session

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

**Goal:** Global and org-level blocklists, distributed to instances.

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

### 6.3 Distribution via Heartbeat

Already implemented in Milestone 3 heartbeat handler. This milestone adds:
- Org-scoped blocklist deltas in heartbeat response (for org-bound instances)
- Instance-side enforcement of org blocklist

### 6.4 Delisting

If a registry admin blocks an instance:
- Instance disappears from public directory
- Heartbeat response includes `{ "blocked": true, "reason": "..." }`
- Instance can still operate, but no longer appears in discovery

**Estimated size:** ~400 lines Rust

---

## Milestone 7: Multi-Device and Key Management

**Goal:** Users can link multiple devices to one account, add/revoke keys.

**Depends on:** Milestone 2

### 7.1 Account Keys Table

Add `account_keys` table to registry (see design doc section 7.1).

### 7.2 Key Management API

- `POST /api/v1/accounts/:id/keys` — add a new key (authenticated with existing key)
- `DELETE /api/v1/accounts/:id/keys/:public_key` — revoke a key
- `GET /api/v1/accounts/:id/keys` — list active keys

### 7.3 Instance-Side Key Resolution

When resolving a public key via the registry, the response includes the canonical account ID. The instance can recognize that two different keys belong to the same logical user:

```json
{
  "account_id": "...",
  "handle": "alex",
  "public_keys": ["<key1>", "<key2>"]
}
```

The instance stores the account_id alongside the public_key in memberships. Different keys for the same account share a single membership.

**Estimated size:** ~300 lines Rust

---

## Dependency Graph

```
M0: Foundations
 ├── M1: Instance-Local Auth
 │    └── M3: Instance ↔ Registry Integration
 │         └── M6: Blocklists
 └── M2: Registry Core
      ├── M3: Instance ↔ Registry Integration
      ├── M4: OIDC Provider
      │    └── M5: Enterprise SSO
      └── M7: Multi-Device
```

## Build Order

Milestones can be parallelized where dependencies allow:

```
Phase 1:  M0 (foundations)              ← everything depends on this
Phase 2:  M1 + M2 (in parallel)        ← instance auth + registry core
Phase 3:  M3 + M4 (in parallel)        ← integration + OIDC
Phase 4:  M5 + M6 + M7 (in parallel)   ← enterprise + moderation + multi-device
```

## Total Estimated Scope

| Milestone | Rust LOC | Frontend LOC | Notes |
|-----------|----------|-------------|-------|
| M0: Foundations | ~600 | — | Shared crate |
| M1: Instance Auth | ~1200 | ~800 | Core auth UX |
| M2: Registry Core | ~2000 | ~200 | New binary |
| M3: Integration | ~500 | — | HTTP client + background task |
| M4: OIDC Provider | ~1500 | ~50 | Fiddly but well-defined |
| M5: Enterprise SSO | ~1200 | ~400 | Mostly registry-side |
| M6: Blocklists | ~400 | — | Thin CRUD + delta sync |
| M7: Multi-Device | ~300 | — | Key management |
| **Total** | **~7700** | **~1450** | |

## Risk Register

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| OIDC spec compliance edge cases | High | Medium | Use `openidconnect` crate, don't hand-roll. Test against real IdPs early. |
| Ed25519 in browser (Web Crypto) | Medium | Medium | Fallback: `@noble/ed25519` polyfill. Verify browser support matrix. |
| Key custody for browser-only users | Medium | High | Defer to M7. Initially require users to save a key backup file. |
| SQLite concurrent writes on registry | Low | Low | WAL mode + single-writer. Traffic is negligible. |
| Invite token size too large for some channels | Low | Low | 254 chars base32 fits in any medium. Registry short-codes are 8 chars. |
| OIDC key rotation disrupts active sessions | Low | Medium | Overlap window (2 active keys). Instance JWKS cache TTL = 1 hour. |

## Open Questions

1. **Should the registry be a separate binary or a mode of `crab_city`?** Separate binary is cleaner (different deployment, different config), but shares a lot of code. Could be a cargo feature flag on `crab_city` instead. Recommend: separate binary, shared `crab_city_auth` crate.

2. **Browser keypair UX.** Generating a keypair in the browser is invisible to the user. But what happens when they clear their browser data? Need to decide: (a) "download your key" prompt, (b) registry-custodied keys, (c) passphrase-derived keys. Recommend: (a) for M1, add (b) in M4 when registry auth exists.

3. **Instance identity for OIDC.** Instances need a `client_id` and `client_secret` for the OIDC flow. The `client_id` is the NodeId (public, deterministic). The `client_secret` is issued at registration. But what if an instance re-installs and gets a new NodeId? Need a re-registration flow or NodeId migration path.

4. **Subdomain routing.** Should `crabcity.dev` offer `<slug>.crabcity.dev` subdomains that proxy to instances? Out of scope for now, but worth noting as a future possibility that would affect DNS/TLS setup.
