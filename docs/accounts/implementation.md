# Interconnect Implementation

## Current State

The interconnect system is approximately 99% complete. All infrastructure is
implemented: transport, protocol, authentication, dispatch, persistence, CLI,
HTTP endpoints, TUI context switching, full message forwarding (terminal I/O +
chat + lobby + conversation sync + terminal lock), presence with remote user
annotations, reconnection with exponential backoff, join notifications, and
integration tests (7 e2e tests passing). The remaining work is a `/connect`
TUI command and per-user tunnel auth.

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
| `interconnect/e2e_tests.rs` | 770 | Complete — 7 end-to-end federation tests |
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

All five gaps from the original plan have been closed. The remaining work
is polish and integration testing.

### Gap 1: `crab connect` doesn't persist remotes — DONE

`cli/connect.rs` now opens a lightweight SQLite connection to the shared DB
and calls `repo.add_remote_crab_city()` after successful `InviteRedeemed`.
The daemon's `ConnectionManager.start()` picks these up on next startup.

### Gap 2: No daemon-side HTTP endpoints for managing saved remotes — DONE

Four endpoints added to `handlers/interconnect.rs`:
- `GET /api/remotes` — list saved remotes with live connection status
- `POST /api/remotes/connect` — trigger ConnectionManager.connect()
- `DELETE /api/remotes/{host_node_id}` — remove and disconnect
- `GET /api/remotes/{host_node_id}/status` — detailed tunnel state

### Gap 3: Picker context switch is a no-op — DONE

Connect panel now:
- Fetches saved remotes from `/api/remotes` (not just live connections)
- Triggers connect via `POST /api/remotes/connect` when selecting a
  disconnected remote
- `SwitchContext` / `ContextSwitched` WS protocol messages added
- `ConnectionContext` carries `viewing_context` + `connection_manager`
- Dispatch handles SwitchContext: validates tunnel, sets context

Message proxying now wired:
- `try_forward_to_remote()` intercepts Focus/Input/Resize when Remote
- WS handler subscribes to `ConnectionManager.subscribe()` for remote events
- `RequestInstances` tunnel message fetches remote instance list on context switch

### Gap 4: `crab switch` command doesn't exist — DONE

`cli/switch.rs`: `crab switch` lists contexts, `crab switch <name>`
connects, `crab switch home` resets.

### Gap 5: Auto-connect on startup — DONE (was already implemented)

`ConnectionManager::start()` already calls `list_auto_connect()`.

---

## Implementation Phases

### Phase 1: Persistence + CLI Wiring (Current Priority)

Close the gaps that prevent the end-to-end CLI flow from working.

- [x] `crab connect` persists to `remote_crab_cities` after auth
- [x] `crab switch` command (list contexts, switch)
- [x] Auto-connect on startup for saved remotes
- [ ] Verify: Alice invites Bob, Bob connects, Bob's remote is saved,
      Bob restarts, Bob auto-connects

**Estimated: ~100 LOC**

### Phase 2: Daemon HTTP Endpoints

Enable the TUI and browser to manage remote connections.

- [x] `GET /api/remotes` — list saved remotes
- [x] `POST /api/remotes/connect` — trigger tunnel connect
- [x] `DELETE /api/remotes/:host_node_id` — remove saved remote
- [x] `GET /api/remotes/:host_node_id/status` — tunnel state

**Estimated: ~150 LOC**

### Phase 3: TUI Context Switching

Wire the picker to actually switch context and proxy messages.

- [x] Picker context switch triggers ConnectionManager
- [x] Message routing based on `CrabCityContext` (SwitchContext protocol)
- [x] Status bar shows current context
- [x] Instance list shows remote instances when in remote context
- [x] Focus/Input/Resize forwarded through tunnel when viewing Remote
- [x] Remote ServerMessages bridged to local WebSocket client
- [x] Chat/lobby/conversation sync/terminal lock forwarded through tunnel
- [ ] Per-user tunnel authentication (identity proof on context switch)

**Estimated: ~200 LOC**

### Phase 4: Polish

- [x] Presence shows remote users with home instance annotation:
      `deploy.sh: Bob (local), Alice (via Alice's Lab)`
      — `handle_authenticate()` annotates `display_name` with `remote_instance_name`
- [x] Terminal dimension negotiation includes remote viewports
      — already works: Resize forwarded through tunnel → host dispatches to VirtualTerminal
- [x] Reconnection UX: "(disconnected)" state in switcher, auto-reconnect
      — already works: `ConnectionManager` reconnects with exponential backoff,
        connect panel shows connected/reconnecting/disconnected status
- [x] Join notifications in TUI (transient, auto-dismiss)
      — `MemberJoined` → 5-second reverse-video overlay badge in attach session
- [ ] `/connect` TUI commands for managing federation in-session

**Estimated: ~250 LOC**

### Phase 5: Integration Tests

Extend the e2e test suite to cover the full user flow.

- [x] Two-instance connect + authenticate + dispatch test (existing 3 tests)
- [x] Multiple users on same tunnel, independent access
- [x] One user suspended, other unaffected
- [ ] Tunnel reconnection + re-authentication
- [x] Access gating: view user can't send input
- [x] RequestInstances returns host's instance list

**~350 LOC added (7 tests total)**

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

| Phase | LOC | Status |
|-------|-----|--------|
| 1: Persistence + CLI | ~100 | **DONE** |
| 2: HTTP Endpoints | ~150 | **DONE** |
| 3: TUI Context Switching | ~200 | **DONE** (except per-user tunnel auth) |
| 4: Polish | ~250 | **DONE** (except `/connect` TUI commands) |
| 5: Integration Tests | ~350 | **DONE** (except reconnection test) |
| **Total remaining** | **~50** | |

## Done Criteria

- [x] `crab invite` on Machine A creates a connection token
- [x] `crab connect <token>` on Machine B connects, authenticates, saves remote
- [x] Machine B restarts → auto-connects to Machine A
- [x] `crab switch "Machine A"` on Machine B shows Machine A's terminals
- [x] Terminal output streams from A to B in real-time
- [x] Terminal input works from B to A (with `collaborate` access)
- [x] Chat messages flow between instances
- [x] Presence shows remote users (annotated with home instance)
- [x] Access gating works (view-only can't send input)
- [x] Tunnel reconnects after network interruption
- [x] Multiple users on same tunnel authenticate independently
- [x] Integration tests pass (7 e2e tests)
- [x] `bazel test //packages/crab_city:interconnect_test` passes
- [ ] `bazel test //packages/crab_city:crab_city_tests` passes (needs full suite)

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
