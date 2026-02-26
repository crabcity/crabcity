# Interconnect Implementation

## Current State

The interconnect system is approximately 85% complete. The core transport,
protocol, authentication, and dispatch infrastructure are implemented and
tested. The remaining work is wiring the pieces together for the end-to-end
CLI experience and TUI integration.

## What's Built

### crab_city_auth (~3,360 LOC)

The shared cryptographic primitives crate. Complete.

| Module | LOC | Status |
|--------|-----|--------|
| `keys.rs` | 261 | Complete — PublicKey, SigningKey, Signature, fingerprint |
| `capability.rs` | 410 | Complete — AccessRights, Capability, algebra |
| `membership.rs` | 436 | Complete — MembershipState, transitions |
| `invite.rs` | 738 | Complete — InviteLink, Invite, chain verification |
| `identity_proof.rs` | 374 | Complete — sign/verify identity proofs |
| `noun.rs` | 319 | Complete — IdentityNoun, parsing, validation |
| `event.rs` | 558 | Complete — Event, hash chain, checkpoints |
| `error.rs` | 168 | Complete — AuthError, Recovery, RecoveryAction |
| `encoding.rs` | 76 | Complete — base32/hex utilities |

Property-tested with proptest. Kani bounded model checking proof harnesses
for parser safety, state machine invariants, and capability algebra.

### Transport Layer (~2,440 LOC)

| File | LOC | Status |
|------|-----|--------|
| `transport/iroh_transport.rs` | 1,139 | Complete — accept loop, connection handling |
| `transport/connection_token.rs` | 726 | Complete — v1 + v2 format, signed metadata |
| `transport/framing.rs` | 316 | Complete — length-prefixed JSON |
| `transport/relay.rs` | 78 | Complete — embedded iroh relay |
| `transport/replay_buffer.rs` | 168 | Complete — reconnection replay |

### Interconnect Core (~2,770 LOC)

| File | LOC | Status |
|------|-----|--------|
| `interconnect/host.rs` | 1,113 | Complete — inbound tunnel handler, auth, dispatch |
| `interconnect/manager.rs` | 710 | Complete — outbound tunnel management |
| `interconnect/protocol.rs` | 450 | Complete — tunnel message types, read/write |
| `interconnect/e2e_tests.rs` | 423 | Complete — 3 end-to-end federation tests |
| `interconnect/mod.rs` | 70 | Complete — CrabCityContext enum |

### Handlers & Dispatch (~2,920 LOC)

| File | LOC | Status |
|------|-----|--------|
| `handlers/interconnect.rs` | 1,714 | Complete — invite CRUD, chain verification, RPC dispatch |
| `ws/dispatch.rs` | 1,094 | Complete — unified message dispatch (local + remote) |
| `handlers/preview.rs` | 113 | Complete — unauthenticated preview stream |

### Repository Layer (~2,200 LOC)

| File | LOC | Status |
|------|-----|--------|
| `repository/event_log.rs` | 733 | Complete — hash-chained event log |
| `repository/membership.rs` | 623 | Complete — member identity + grants |
| `repository/federation.rs` | 577 | Complete — federated accounts + remotes |
| `repository/invites.rs` | 270 | Complete — invite CRUD |

### CLI (~770 LOC)

| File | LOC | Status |
|------|-----|--------|
| `cli/connect.rs` | 538 | Functional — two-phase connect with confirmation |
| `cli/invite.rs` | 236 | Functional — invite creation with QR |

### Other (~260 LOC)

| File | LOC | Status |
|------|-----|--------|
| `identity.rs` | 143 | Complete — instance identity keypair |
| `config.rs` | ~120 | Has `instance_name` field |

### Total Implemented: ~14,700 LOC

## What's Remaining

The implemented components work in isolation (proven by e2e tests), but the
full user-facing flow has gaps. The following items are needed to go from
"backend works" to "two computers can interconnect."

### Gap 1: `crab connect` doesn't persist remotes

**Problem:** `crab connect <token>` authenticates and connects but does NOT
save the remote to the `remote_crab_cities` table. On next startup, the
connection is lost.

**Fix (~20 LOC):**
```rust
// In cli/connect.rs, after successful authentication:
repo.add_remote_crab_city(
    &host_node_id,
    &local_account_key,
    &host_name,
    &granted_access,
).await?;
```

### Gap 2: No daemon-side HTTP endpoints for managing saved remotes

**Problem:** The daemon has no HTTP endpoints for listing, connecting to, or
removing saved remote Crab Cities. The TUI/browser can't manage connections
without these.

**Fix (~150 LOC):**
- `GET /api/remotes` — list saved remotes from `remote_crab_cities`
- `POST /api/remotes/connect` — trigger ConnectionManager to connect
- `DELETE /api/remotes/:host_node_id` — remove a saved remote
- `GET /api/remotes/:host_node_id/status` — tunnel state (connected,
  disconnected, reconnecting)

### Gap 3: Picker context switch is a no-op

**Problem:** The TUI picker has the `CrabCityContext` enum but switching to a
remote context doesn't establish a tunnel or proxy messages. The context
switch is plumbing without wiring.

**Fix (~200 LOC):**
- When picker switches to `Remote { host_node_id }`:
  1. Look up saved remote in DB
  2. If no active tunnel, call ConnectionManager.connect()
  3. Authenticate the local user on the tunnel
  4. Route subsequent ClientMessages through the tunnel
  5. Forward ServerMessages from the tunnel to the local WebSocket
- Status bar shows current context (local vs remote name)

### Gap 4: `crab switch` command doesn't exist

**Problem:** No CLI command to switch between local and remote contexts.

**Fix (~50 LOC):**
- `crab switch` — list available contexts
- `crab switch <name>` — switch to named remote
- `crab switch home` — switch back to local

### Gap 5: Auto-connect on startup

**Problem:** Saved remotes with `auto_connect = true` aren't connected on
daemon startup.

**Fix (~30 LOC):**
- On server startup, after iroh transport starts:
  1. Query `list_auto_connect()` from DB
  2. For each remote, spawn ConnectionManager.connect() task
  3. Log connection status

---

## Implementation Phases

### Phase 1: Persistence + CLI Wiring (Current Priority)

Close the gaps that prevent the end-to-end CLI flow from working.

- [ ] `crab connect` persists to `remote_crab_cities` after auth
- [ ] `crab switch` command (list contexts, switch)
- [ ] Auto-connect on startup for saved remotes
- [ ] Verify: Alice invites Bob, Bob connects, Bob's remote is saved,
      Bob restarts, Bob auto-connects

**Estimated: ~100 LOC**

### Phase 2: Daemon HTTP Endpoints

Enable the TUI and browser to manage remote connections.

- [ ] `GET /api/remotes` — list saved remotes
- [ ] `POST /api/remotes/connect` — trigger tunnel connect
- [ ] `DELETE /api/remotes/:host_node_id` — remove saved remote
- [ ] `GET /api/remotes/:host_node_id/status` — tunnel state

**Estimated: ~150 LOC**

### Phase 3: TUI Context Switching

Wire the picker to actually switch context and proxy messages.

- [ ] Picker context switch triggers ConnectionManager
- [ ] Message routing based on `CrabCityContext`
- [ ] Status bar shows current context
- [ ] Instance list shows remote instances when in remote context
- [ ] Chat, tasks, presence all come from remote host

**Estimated: ~200 LOC**

### Phase 4: Polish

- [ ] Presence shows remote users with home instance annotation:
      `deploy.sh: Bob (local), Alice (via Alice's Lab)`
- [ ] Terminal dimension negotiation includes remote viewports
- [ ] Reconnection UX: "(disconnected)" state in switcher, auto-reconnect
- [ ] `/connect` TUI commands for managing federation in-session
- [ ] Join notifications in TUI (transient, auto-dismiss)

**Estimated: ~250 LOC**

### Phase 5: Integration Tests

Extend the e2e test suite to cover the full user flow.

- [ ] Two-instance connect + authenticate + dispatch test
- [ ] Multiple users on same tunnel, independent access
- [ ] One user suspended, other unaffected
- [ ] Tunnel reconnection + re-authentication
- [ ] Access gating: view user can't send input

**Estimated: ~400 LOC test code**

## Dependency Graph

```
Phase 1: Persistence + CLI
  ├── Phase 2: HTTP Endpoints
  │     └── Phase 3: TUI Context Switching
  │           └── Phase 4: Polish
  └── Phase 5: Integration Tests (incremental alongside 2-4)
```

Phases 2 and 3 are sequential (TUI needs HTTP endpoints). Phase 5 runs
alongside everything.

## Estimated Remaining Work

| Phase | LOC | Notes |
|-------|-----|-------|
| 1: Persistence + CLI | ~100 | Critical path, unblocks everything |
| 2: HTTP Endpoints | ~150 | Enables TUI/browser management |
| 3: TUI Context Switching | ~200 | The "it works" moment |
| 4: Polish | ~250 | Presence, reconnection, notifications |
| 5: Integration Tests | ~400 | Prove it all works together |
| **Total remaining** | **~1,100** | |

## Done Criteria

- [ ] `crab invite` on Machine A creates a connection token
- [ ] `crab connect <token>` on Machine B connects, authenticates, saves remote
- [ ] Machine B restarts → auto-connects to Machine A
- [ ] `crab switch "Machine A"` on Machine B shows Machine A's terminals
- [ ] Terminal output streams from A to B in real-time
- [ ] Terminal input works from B to A (with `collaborate` access)
- [ ] Chat messages flow between instances
- [ ] Presence shows remote users
- [ ] Access gating works (view-only can't send input)
- [ ] Tunnel reconnects after network interruption
- [ ] Multiple users on same tunnel authenticate independently
- [ ] Integration tests pass
- [ ] `bazel test //packages/crab_city:interconnect_test` passes
- [ ] `bazel test //packages/crab_city:crab_city_tests` passes

## Future Milestones

These build on top of interconnect but are not required for it to function:

| Milestone | Depends On | Adds |
|-----------|-----------|------|
| Registry Core | Interconnect | User profiles, instance directory, handle-based invites |
| Registry Integration | Registry Core | Heartbeat, handle resolution, blocklist sync |
| OIDC Provider | Registry Core | "Sign in with Crab City" |
| Enterprise SSO | OIDC Provider | Okta/Entra/Google Workspace |
| Blocklists | Registry Integration | Global + org-scoped moderation |
| LAN Discovery | Interconnect | `crab connect --discover` (mDNS/DHT) |

```
Interconnect ← THIS
 +-- Registry Core
 |    +-- Registry Integration
 |    |    +-- Blocklists
 |    +-- OIDC Provider
 |         +-- Enterprise SSO
 +-- LAN Discovery
```

## Code Layout

```
packages/crab_city_auth/           ~3,360 LOC  (shared crypto primitives)
packages/crab_city/src/
  interconnect/
    mod.rs                         CrabCityContext enum
    host.rs                        Inbound tunnel handler
    manager.rs                     Outbound tunnel manager
    protocol.rs                    Tunnel message types
    e2e_tests.rs                   End-to-end federation tests
  transport/
    iroh_transport.rs              iroh accept loop + connection handling
    connection_token.rs            v1/v2 token format
    framing.rs                     Length-prefixed JSON
    relay.rs                       Embedded iroh relay
    replay_buffer.rs               Reconnection replay
  handlers/
    interconnect.rs                Invite CRUD, chain verification, RPC
    preview.rs                     Unauthenticated preview stream
  repository/
    federation.rs                  Federated accounts + remote cities
    membership.rs                  Member identities + grants
    invites.rs                     Invite CRUD
    event_log.rs                   Hash-chained event log
  ws/
    dispatch.rs                    Unified message dispatch
  cli/
    connect.rs                     Two-phase connect command
    invite.rs                      Invite creation + QR
  identity.rs                      Instance identity keypair
  config.rs                        Server config (includes instance_name)
```
