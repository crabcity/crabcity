# Milestone: Interconnect

**Goal:** Two people running Crab City can connect their instances over iroh.
Alice invites Bob. Bob sees Alice's terminals, chat, tasks — the full Crab City
experience — as a switchable context in his TUI. The transport is iroh. The
protocol is the same `ClientMessage`/`ServerMessage` we already speak. The
authorization is per-account. The invite flow is S-tier: secure, intentional,
and frictionless.

**Depends on:** M1 infrastructure (iroh transport, instance identity, membership,
invite system, message dispatch — all implemented on the `interconnect` branch).

**Key decisions:**
- **No iroh WASM.** Browsers stay on WebSocket. The home instance proxies to
  remote hosts over iroh. Browser clients never touch iroh directly.
- **Old auth stays.** Local/browser connections keep session-based auth. iroh is
  the native CLI/TUI and instance-to-instance transport. Both auth paths coexist.

## What Interconnect Means

Today a Crab City instance is a standalone island. You connect to it, see its
terminals, chat in its channels, work on its tasks. If you want to see what's
happening on a friend's instance, you have no path.

After this milestone, Alice's TUI has a Crab City switcher. She can view her
own Crab City or switch to any Crab City she's been invited to:

```
Viewing: Alice's Lab [▼]              Viewing: Bob's Workshop [▼]

┌ Instances ─────────────────┐        ┌ Instances ──────────────────┐
│ ► experiments.sh (running) │        │ ► deploy.sh     (running)   │
│   data-pipeline  (idle)    │        │   editor         (idle)     │
└────────────────────────────┘        │   data-pipeline  (running)  │
                                      └──────────────────────────────┘
Chat: Alice's #general                Chat: Bob's #general
Tasks: Alice's tasks                  Tasks: Bob's tasks
Presence: Alice, Dave                 Presence: Bob, Alice
```

When viewing Bob's Workshop, it's the complete Crab City experience — Bob's
instances, Bob's chat, Bob's tasks, Bob's presence. Not a blend. The whole UI
context switches. The experience is identical to being a local user on Bob's
instance, subject to whatever access Bob granted Alice.

## The S-Tier Invite/Join Experience

This is the pivotal moment: Bob has been running solo. He wants to invite Alice.
The experience must be secure, intentional, and feel as natural as sharing a
link.

### Bob's Side: Creating the Invite

```
$ crab invite --for alice --expires 1h

  Invite for Bob's Workshop
  Label:   alice
  Access:  collaborate
  Expires: 1 hour

  24G8R3YK7V3MQ6...
  (copied to clipboard)

  ██████████████
  █            █
  █  QR CODE   █
  █            █
  ██████████████

  Share with:  crab connect 24G8R3YK7V3MQ6...
```

Bob sends the token over whatever channel he and Alice already use — Slack,
Signal, email, in-person QR scan. The token is self-describing: it carries
Bob's instance name and fingerprint inside the signed envelope, so Alice can
verify what she's joining before any network activity.

### Alice's Side: Joining

```
$ crab connect 24G8R3YK7V3MQ6...

  Bob's Workshop
  Invited by: Bob (crab_2K7XM9QP)
  Access: collaborate
    terminals: read, input
    chat: send
    tasks: read, create

  Your identity: crab_7F3XM9QP

  Join this workspace? [Y/n] y

  Connected to Bob's Workshop
  3 terminals available:
    ► deploy.sh      (running)
      editor          (idle)
      data-pipeline   (running)

  Attaching to deploy.sh...
```

Alice sees exactly what she's joining and what access she'll have *before* she
commits her identity. The confirmation is explicit. No auto-redeem.

### Bob Gets Notified

In Bob's TUI, a transient notification:

```
  ┌────────────────────────────────────┐
  │ Alice (crab_7F3XM9QP) joined      │
  │ Access: collaborate                │
  └────────────────────────────────────┘
```

### Design Principles

1. **Make the state visible before the transition.** Alice sees the full picture
   (instance name, inviter, access rights) before she commits.
2. **Make impossible states impossible.** An expired invite says "expired 2 hours
   ago — ask Bob for a new one." An already-redeemed token says "you already have
   access — switch with `crab switch`." No generic errors.
3. **The token is the UX.** It's self-describing (carries metadata), self-
   verifying (signed by the instance), and self-contained (no network needed to
   read it). Copy/paste, QR scan, URL — all work because the token carries
   everything.
4. **Both sides get feedback.** Bob knows Alice joined. Alice knows she joined.
   The invite label creates a human-readable audit trail.

### Error States

Every error state has a specific message with a recovery action:

| Error | Message |
|-------|---------|
| Expired | "This invite expired 2 hours ago. Ask Bob for a new one." |
| Exhausted | "This invite has been fully used. Ask Bob for a new one." |
| Revoked | "This invite was revoked. Contact Bob." |
| Already a member | "You already have access to Bob's Workshop. Switch with: `crab switch 'Bob's Workshop'`" |
| Suspended | "Your access to Bob's Workshop has been suspended. Contact Bob." |
| Network unreachable | "Can't reach Bob's Workshop. Will retry automatically." |

## Connection Token v2

The current `ConnectionToken` (v1) carries: `[1B version][32B node_id][16B nonce][relay_url?]`.
This is enough to connect, but Alice sees nothing about what she's joining.

Token v2 adds signed metadata so Alice can inspect the invite offline:

```
ConnectionToken v2:
  [1B version=2]
  [32B node_id]              — host instance ed25519 pubkey
  [16B invite_nonce]         — nonce to redeem
  [1B name_len]              — length of instance name (0-255)
  [name_len B instance_name] — UTF-8, e.g. "Bob's Workshop"
  [8B inviter_fingerprint]   — first 8 bytes of inviter's pubkey hash (for display)
  [1B capability]            — 0=view, 1=collaborate, 2=admin
  [64B signature]            — ed25519 signature over all preceding bytes by instance key
  [remaining: relay_url]     — optional, same as v1
```

The signature is computed by the instance's signing key over all bytes preceding
it. This means Alice can verify that the metadata (instance name, inviter,
capability) is authentic — it wasn't tampered with in transit.

**Size:** For a typical instance name (20 chars), no relay URL:
`1 + 32 + 16 + 1 + 20 + 8 + 1 + 64 = 143 bytes → ~229 base32 chars`.

Longer than v1's ~79 chars, but the token goes through copy/paste or QR — not
typed by hand. The tradeoff is worth it: Alice can verify what she's joining
before any network activity. QR codes handle this size easily.

**Backward compatibility:** `from_base32` checks the version byte. v1 tokens
still parse (no metadata, no confirmation prompt — just the old behavior).
v2 tokens are only generated by instances that support them.

## The Model: Federated Accounts

Interconnect is a federation system. When Bob invites Alice to connect to his
Crab City, he's creating an **account on his instance for Alice's identity**.
Alice's key on Alice's server gets a corresponding account on Bob's server,
with whatever access Bob chooses to grant.

This is NOT an instance-to-instance relationship. Bob invited **Alice**, not
"everyone on Alice's server." Alice's teammate Dave does not get an account on
Bob's Crab City unless Bob separately invites Dave.

```
Bob's Crab City                         Alice's Crab City
  │                                         │
  │ Grant: Alice's pubkey → collaborate     │
  │                                         │
  │              iroh QUIC                  │
  │ <────────────────────────────────────── │
  │              (transport)                │
  │                                         │
  │ Alice authenticates with her key,       │
  │ sees Bob's terminals, chat, tasks       │
  │                                         │
  │ Dave (Alice's teammate) has no grant    │
  │ on Bob's instance — sees nothing.       │
```

### How It Works

Bob's instance creates a **federated account** for Alice. This account is keyed
to Alice's public key — the same ed25519 key that identifies her on her home
instance. It's a real account on Bob's server, with its own grant, just linked
to an identity that lives on another server.

```rust
FederatedAccount {
    account_key: PublicKey,          // Alice's ed25519 pubkey
    display_name: String,            // "Alice"
    home_node_id: Option<PublicKey>, // Alice's home instance NodeId
    home_name: Option<String>,       // "Alice's Lab"
    access: AccessRights,            // what Alice can do here
    state: GrantState,               // active, suspended
    created_by: PublicKey,           // Bob (admin who created this account)
    created_at: u64,
}
```

The capability algebra is identical to local member grants:

| Scenario | Access Rights |
|----------|---------------|
| View only | `content:read`, `terminals:read` |
| Collaborate | + `terminals:input`, `chat:send`, `tasks:read,create,edit` |
| Full | + `members:read` |

### Each Direction Is Independent

Bob inviting Alice does NOT mean Alice invited Bob:

```
Bob invites Alice:
  Bob's instance: federated account for Alice's pubkey ✓
  Alice's instance: bookmark for Bob's instance ✓
  Alice → can view Bob's stuff
  Bob → cannot view Alice's stuff

Alice also invites Bob:
  Alice's instance: federated account for Bob's pubkey ✓
  Bob's instance: bookmark for Alice's instance ✓
  Bob → can now view Alice's stuff too
```

The two grants are independent with potentially different access levels:

```
Bob → Alice: collaborate (terminals:read,input + chat:send + tasks:read,create)
Alice → Bob: view only (content:read + terminals:read)
```

## Transport

### The Insight: Same Protocol

An instance connecting to a remote host on behalf of its users is a transport
proxy. The remote host speaks the same `ClientMessage`/`ServerMessage` protocol
that WebSocket clients already speak. No new message types are needed for the
core experience.

```
Alice's TUI ──ws──► Alice's Instance ──iroh──► Bob's Instance
                    (transport proxy)           (host)

Alice's Browser ──ws──► Alice's Instance ──iroh──► Bob's Instance
                        (same proxy)                (same host)
```

Alice's instance maintains an iroh connection to Bob's. When Alice focuses one
of Bob's terminals, her instance forwards the `Focus` message to Bob's instance
over iroh. Bob's instance responds with `FocusAck`, `OutputHistory`, etc. — the
same flow as a local WebSocket client.

### One Tunnel, Many Users

The iroh connection is between instances (identified by NodeId). This is shared
infrastructure — if both Alice and Dave on the same home instance have grants on
Bob's Crab City, their messages travel over the same iroh connection. But each
user **authenticates individually** within that tunnel.

```
Alice's Instance ──iroh──► Bob's Instance
  │                             │
  │ Alice's messages ──────────►│ verify Alice's identity proof
  │                             │ lookup Alice's federated account → collaborate
  │                             │ allow
  │                             │
  │ Dave's messages ───────────►│ verify Dave's identity proof
  │                             │ lookup Dave's federated account → not found
  │                             │ reject
```

### Connection Establishment

```
Home Instance                           Host Instance
    |                                       |
    |  iroh QUIC connect (ALPN: crab/1)    |
    | ------------------------------------> |
    |                                       |  accept connection
    |                                       |  (instance-level, not
    |                                       |   yet user-level)
    |                                       |
    |  InstanceHello:                      |
    |  { instance_name, node_id }          |
    | ------------------------------------> |
    |                                       |
    |  InstanceWelcome:                    |
    |  { instance_name }                   |
    | <------------------------------------ |
    |                                       |
    |  transport tunnel established        |
    |  users authenticate individually     |
```

### Per-User Authentication

When Alice first focuses a remote terminal, her home instance sends an identity
proof on her behalf:

```
Home → Host:
{
    "type": "Authenticate",
    "identity_proof": <signed proof from Alice's key>,
    "display_name": "Alice"
}

Host → Home:
{
    "type": "AuthResult",
    "account_key": "<Alice's pubkey>",
    "access": [<granted access rights>]
}
```

After authentication, subsequent messages from Alice are tagged with her
account key. The host checks each message against her federated grant.

### Message Flow

After authentication, the connection carries the existing protocol:

**Home → Host** (subset of `ClientMessage`, tagged with user):
- `Focus { instance_id, account_key }` — view this terminal
- `Input { instance_id, data, account_key }` — terminal input
- `Resize { instance_id, rows, cols, account_key }` — viewport resize
- `TerminalVisible / TerminalHidden` — viewport registration
- `TerminalLockRequest / TerminalLockRelease` — lock management
- `ChatSend { scope, content, account_key }` — send a chat message
- `ChatHistory { scope, before_id, limit }` — request chat history

**Host → Home** (subset of `ServerMessage`):
- `InstanceList` — available terminals
- `Output / OutputHistory` — terminal data
- `StateChange` — instance state (idle, thinking, etc.)
- `FocusAck` — acknowledge focus switch
- `PresenceUpdate` — who's viewing what
- `TerminalLockUpdate` — lock state
- `ChatMessage / ChatHistoryResponse` — chat
- `TaskUpdate / TaskDeleted` — tasks
- `OutputLagged` — backpressure notification

Every inbound message is checked against the user's federated grant. If Alice
doesn't have `terminals:input`, her `Input` messages are rejected.

### Multiple QUIC Streams

One iroh connection per remote host. Multiple QUIC streams for parallel
delivery:
- Stream 0: Control (hello/welcome, authenticate, disconnect)
- Stream 1: Terminal output (high bandwidth, independent flow control)
- Stream 2: Everything else (chat, tasks, presence, state changes)

Framing: length-prefixed JSON (matching existing framing in `transport/framing.rs`).
Backpressure: same `OutputLagged` mechanism as WebSocket clients.

### Reconnection

If the iroh connection to a remote host drops:

1. Remote instances show as "(disconnected)" in the switcher
2. Home instance attempts reconnection with exponential backoff
3. On reconnect, re-authenticate and replay missed output (same ring buffer
   mechanism in `transport/replay_buffer.rs`)
4. The user sees a brief interruption, then normal service resumes

## Data Model

### Host-Side Schema

```sql
-- Federated accounts (accounts on this server for identities from other servers)
CREATE TABLE federated_accounts (
    account_key BLOB NOT NULL PRIMARY KEY,  -- 32 bytes, ed25519 pubkey
    display_name TEXT NOT NULL,
    home_node_id BLOB,                      -- 32 bytes, home instance NodeId
    home_name TEXT,                          -- "Alice's Lab" (display only)
    access TEXT NOT NULL DEFAULT '[]',       -- JSON: access rights
    state TEXT NOT NULL DEFAULT 'active',    -- 'active', 'suspended'
    created_by BLOB NOT NULL,               -- pubkey of admin who created this
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

### Home-Side Schema

```sql
-- Remote Crab Cities this user can connect to
CREATE TABLE remote_crab_cities (
    host_node_id BLOB NOT NULL,              -- 32 bytes, host instance NodeId
    account_key BLOB NOT NULL,               -- 32 bytes, local user this belongs to
    host_name TEXT NOT NULL,                  -- "Bob's Workshop"
    granted_access TEXT NOT NULL DEFAULT '[]', -- JSON: what the host granted us
    auto_connect INTEGER NOT NULL DEFAULT 1,  -- connect on startup?
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (host_node_id, account_key)
);
```

### Event Log

Connection lifecycle events extend the existing event log:

- `federation.granted` — new federated account created
- `federation.access_changed` — access rights updated
- `federation.suspended` — account suspended
- `federation.removed` — account removed
- `federation.authenticated` — remote user authenticated (informational)
- `federation.disconnected` — remote user disconnected (informational)

## What Already Exists (M1 Infrastructure)

Everything below is implemented and working on the `interconnect` branch:

| Component | File | LOC | Status |
|-----------|------|-----|--------|
| iroh transport + accept loop | `transport/iroh_transport.rs` | 973 | Complete |
| Message framing | `transport/framing.rs` | 280 | Complete |
| Embedded relay | `transport/relay.rs` | 79 | Complete |
| Replay buffer | `transport/replay_buffer.rs` | 169 | Complete |
| Connection token v1 | `transport/connection_token.rs` | 190 | Complete |
| Instance identity | `identity.rs` | 144 | Complete |
| Invite CRUD + chain verification | `handlers/interconnect.rs` | 1,660 | Complete |
| Membership repository | `repository/membership.rs` | 623 | Complete |
| Invite repository | `repository/invites.rs` | 270 | Complete |
| Event log (hash-chained) | `repository/event_log.rs` | 733 | Complete |
| CLI connect (two-phase) | `cli/connect.rs` | 409 | Complete |
| CLI invite + QR | `cli/invite.rs` | 170 | Complete |
| Unified message dispatch | `ws/dispatch.rs` | 1,095 | Complete |
| Protocol (all message types) | `ws/protocol.rs` | 2,267 | Complete |
| Preview WebSocket | `handlers/preview.rs` | 113 | Complete |
| Auth primitives (crab_city_auth) | separate crate | ~3,360 | Complete |

The M1 infrastructure handles single-client-per-QUIC-connection. Interconnect
lifts this to single-instance-per-QUIC-connection with multiplexed users.

## Implementation

### Phase 1: S-Tier Invite UX

Self-contained improvements to the existing invite/join flow. Ships value
immediately, no federation dependency.

#### 1.1 Connection Token v2

Modify: `transport/connection_token.rs`

Add v2 format with signed metadata. Keep v1 parsing for backward compat.

```rust
pub struct ConnectionToken {
    pub node_id: [u8; 32],
    pub invite_nonce: [u8; 16],
    pub relay_url: Option<String>,
    // v2 fields (None for v1 tokens)
    pub instance_name: Option<String>,
    pub inviter_fingerprint: Option<[u8; 8]>,
    pub capability: Option<Capability>,
    pub signature: Option<Signature>,
}
```

- [ ] Implement v2 serialization/deserialization
- [ ] Sign all preceding bytes with instance signing key
- [ ] Verify signature on parse (invalid signature → treat as v1 fallback)
- [ ] Update `to_base32` / `from_base32` to handle both versions
- [ ] Tests: v2 roundtrip, v1 backward compat, tampered signature rejected

#### 1.2 Invite Creation with Metadata

Modify: `handlers/interconnect.rs` (`handle_create_invite`)
Modify: `cli/invite.rs` (`invite_create_command`)

The server embeds instance name + inviter fingerprint + capability into the
connection token when generating it. The CLI auto-copies to clipboard.

- [ ] `handle_create_invite` returns v2 token with embedded metadata
- [ ] Instance name from config (add `instance_name` to config if not present)
- [ ] Inviter fingerprint from `AuthUser.public_key`
- [ ] `invite_create_command` copies token to clipboard (best-effort: `pbcopy`
      on macOS, `xclip`/`wl-copy` on Linux, skip silently if unavailable)
- [ ] Print "(copied to clipboard)" when successful

#### 1.3 Confirmation Prompt in Connect

Modify: `cli/connect.rs` (`connect_command`)

Before sending `RedeemInvite`, decode token metadata and show confirmation.

```rust
// After parsing token, before connecting:
if let Some(ref name) = token.instance_name {
    eprintln!();
    eprintln!("  {}", name);
    if let Some(ref fp) = token.inviter_fingerprint {
        eprintln!("  Invited by: {}", format_fingerprint(fp));
    }
    if let Some(cap) = token.capability {
        eprintln!("  Access: {}", cap.display_with_rights());
    }
    eprintln!();
    eprint!("  Join this workspace? [Y/n] ");
    // Read confirmation...
}
```

- [ ] Decode and display v2 metadata before connecting
- [ ] Confirmation prompt (default Y, skip with `--yes` flag)
- [ ] For v1 tokens: show node ID short form, skip metadata, still confirm
- [ ] `--yes` / `-y` flag to skip confirmation (for scripting)

#### 1.4 Specific Error Messages

Modify: `cli/connect.rs` (error handling after `RedeemInvite` response)

Match specific error patterns and show recovery actions:

- [ ] "expired" → "This invite expired. Ask the host for a new one."
- [ ] "exhausted" → "This invite has been fully used."
- [ ] "revoked" → "This invite was revoked."
- [ ] "already have a grant" → "You already have access. Switch with: `crab switch '<name>'`"
- [ ] "suspended" → "Your access has been suspended. Contact the host."
- [ ] Connection failure → "Can't reach host. Check your network."

#### 1.5 Instance Name Config

Modify: `config.rs`

Add `instance_name` field. Default to hostname or "Crab City".

- [ ] Add `instance_name: String` to config struct
- [ ] Default: `hostname::get().unwrap_or("Crab City")`
- [ ] Surface in `/api/invites` response and connection token
- [ ] Surface in preview WebSocket (`PreviewActivity.instance_name`)

#### Phase 1 Checklist

- [ ] v2 tokens carry instance name + inviter fingerprint + capability
- [ ] v2 tokens are signed by instance key, signature verified on parse
- [ ] v1 tokens still work (backward compat)
- [ ] `crab invite` copies token to clipboard
- [ ] `crab connect` shows metadata and asks for confirmation before joining
- [ ] All error states show specific messages with recovery actions
- [ ] Instance name configurable, defaults to hostname
- [ ] Tests pass

---

### Phase 2: Federation Data Model

#### 2.1 Migration

New file: `migrations/NNN_federation.sql`

```sql
CREATE TABLE federated_accounts (
    account_key BLOB NOT NULL PRIMARY KEY,
    display_name TEXT NOT NULL,
    home_node_id BLOB,
    home_name TEXT,
    access TEXT NOT NULL DEFAULT '[]',
    state TEXT NOT NULL DEFAULT 'active',
    created_by BLOB NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE remote_crab_cities (
    host_node_id BLOB NOT NULL,
    account_key BLOB NOT NULL,
    host_name TEXT NOT NULL,
    granted_access TEXT NOT NULL DEFAULT '[]',
    auto_connect INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (host_node_id, account_key)
);

CREATE INDEX idx_federated_state ON federated_accounts(state);
CREATE INDEX idx_remote_auto ON remote_crab_cities(auto_connect);
```

- [ ] Write migration
- [ ] Verify migration applies cleanly

#### 2.2 Repository: Federated Accounts

New file: `repository/federation.rs`

```rust
pub async fn create_federated_account(db, account_key, display_name, home_node_id, home_name, access, created_by) -> Result<()>
pub async fn get_federated_account(db, account_key) -> Result<Option<FederatedAccount>>
pub async fn list_federated_accounts(db) -> Result<Vec<FederatedAccount>>
pub async fn update_federated_access(db, account_key, access) -> Result<()>
pub async fn update_federated_state(db, account_key, state) -> Result<()>
pub async fn delete_federated_account(db, account_key) -> Result<()>

pub async fn add_remote_crab_city(db, host_node_id, account_key, host_name, granted_access) -> Result<()>
pub async fn list_remote_crab_cities(db, account_key) -> Result<Vec<RemoteCrabCity>>
pub async fn list_auto_connect(db) -> Result<Vec<RemoteCrabCity>>
pub async fn remove_remote_crab_city(db, host_node_id, account_key) -> Result<()>
pub async fn update_remote_access(db, host_node_id, account_key, granted_access) -> Result<()>
```

- [ ] Implement all functions
- [ ] Unit tests: CRUD roundtrips, state transitions
- [ ] Connection invite type (0x02) distinguished from member invite (0x01)

#### Phase 2 Checklist

- [ ] Migration applies cleanly
- [ ] Repository CRUD works, tests pass
- [ ] `cargo check -p crab_city` passes

---

### Phase 3: ConnectionManager (Home Side)

New file: `interconnect/manager.rs`

The outbound connection manager. Maintains iroh tunnels to remote hosts on
behalf of local users. This is the "home side" — your instance connecting out
to other Crab Cities you've been invited to.

```rust
pub struct ConnectionManager {
    /// Active iroh connections to remote hosts, keyed by host NodeId
    tunnels: HashMap<PublicKey, InstanceTunnel>,
    /// Database handle
    repo: ConversationRepository,
    /// Broadcast channel to forward remote events to local clients
    lifecycle_tx: broadcast::Sender<ServerMessage>,
    /// Local instance identity
    identity: Arc<InstanceIdentity>,
    /// iroh endpoint (shared with inbound transport)
    endpoint: Endpoint,
}

struct InstanceTunnel {
    host_node_id: PublicKey,
    host_name: String,
    state: TunnelState,  // Connected, Disconnected, Reconnecting
    /// Which local users are authenticated on this tunnel
    authenticated_users: HashMap<PublicKey, UserSession>,
    /// Cancel token for the tunnel task
    cancel: CancellationToken,
}

enum TunnelState {
    Connected { stream: QuicBiStream },
    Disconnected { since: Instant },
    Reconnecting { attempt: u32 },
}
```

#### 3.1 Tunnel Lifecycle

- [ ] On startup: query `remote_crab_cities` where `auto_connect = true`,
      group by `host_node_id`, connect to each
- [ ] `InstanceHello` / `InstanceWelcome` handshake per tunnel
- [ ] Spawn a receiver task per tunnel (reads `ServerMessage`, forwards to
      `lifecycle_tx` tagged with source host)
- [ ] On disconnect: mark tunnel disconnected, begin reconnection backoff
- [ ] Reconnection: exponential backoff (1s, 2s, 4s, 8s... max 60s)
- [ ] On reconnect: re-authenticate all users, replay from ring buffer

#### 3.2 Per-User Authentication

- [ ] When a local user focuses a remote terminal: check if already
      authenticated on that tunnel
- [ ] If not: send `Authenticate { identity_proof, display_name }` over the
      tunnel, await `AuthResult`
- [ ] Cache authenticated state per (tunnel, user) pair
- [ ] On tunnel reconnect: re-authenticate all previously authenticated users

#### 3.3 Message Forwarding

- [ ] Route `ClientMessage` from local WebSocket/iroh clients to the
      appropriate tunnel based on which Crab City the user is viewing
- [ ] Tag outbound messages with the user's `account_key`
- [ ] Route inbound `ServerMessage` from tunnels to the correct local clients

#### 3.4 Public API

```rust
impl ConnectionManager {
    pub async fn connect(&self, host_node_id: PublicKey, invite_token: &str) -> Result<String>
    pub async fn disconnect(&self, host_node_id: &PublicKey) -> Result<()>
    pub fn list_connections(&self) -> Vec<ConnectionInfo>
    pub async fn forward_message(&self, host_node_id: &PublicKey, user: &PublicKey, msg: ClientMessage) -> Result<()>
}
```

- [ ] Implement all methods
- [ ] Wire into server startup (start alongside `IrohTransport`)
- [ ] Wire `forward_message` into the dispatch path

#### Phase 3 Checklist

- [ ] Outbound tunnels connect and exchange hello/welcome
- [ ] Per-user authentication works within tunnels
- [ ] Messages forward from local clients to remote hosts
- [ ] Inbound messages from remote hosts reach local clients
- [ ] Tunnels reconnect after disconnection
- [ ] Tests pass

---

### Phase 4: HostHandler (Host Side)

New file: `interconnect/host.rs`

The inbound federation handler. Accepts connections from other Crab City
instances and dispatches per-user messages. This is the "host side" — another
instance connecting to yours on behalf of its users.

This is structurally similar to the existing `connection_handler` in
`iroh_transport.rs` but:
- Reads from an instance tunnel instead of a single-client connection
- Multiplexes multiple users over one connection
- Uses `federated_accounts` for authorization instead of `member_grants`

#### 4.1 Instance Tunnel Accept

Extend the iroh accept loop to distinguish between:
- Single-client connections (existing behavior: NodeId = user identity)
- Instance tunnel connections (new: `InstanceHello` as first message)

```rust
// In the accept loop, after QUIC handshake:
match first_message {
    ClientMessage::RedeemInvite { .. } => {
        // Existing: single client redeeming an invite
        handle_invite_redemption(...)
    }
    InstanceHello { instance_name, node_id } => {
        // New: another Crab City instance connecting
        spawn_tunnel_handler(conn, instance_name, node_id, ...)
    }
    _ => {
        // Existing: authenticated single client
        handle_authenticated_connection(...)
    }
}
```

- [ ] Detect `InstanceHello` as the start of a tunnel connection
- [ ] Respond with `InstanceWelcome { instance_name }`
- [ ] Spawn a `tunnel_handler` task per instance connection

#### 4.2 Per-User Auth Within Tunnel

- [ ] Handle `Authenticate` messages: verify identity proof, look up
      `federated_accounts` row
- [ ] On success: send `AuthResult` with granted access
- [ ] On failure: send `AuthResult` with error (no account, suspended, etc.)
- [ ] Track authenticated users per tunnel

#### 4.3 Message Dispatch

- [ ] Route user-tagged messages through the existing `dispatch_client_message`
- [ ] Construct `AuthUser` from federated account (same shape as local auth)
- [ ] Access gating: same `require_access` checks as local users
- [ ] Broadcast `ServerMessage` variants back through the tunnel

#### 4.4 Connection Invite (type 0x02)

A "connection invite" creates a `federated_accounts` row instead of a
`member_grants` row. Same invite wire format, different type byte.

- [ ] `CreateConnectionInvite` handler (type 0x02)
- [ ] `RedeemConnectionInvite` handler: creates federated account, returns
      `AuthResult`
- [ ] Reuse existing invite chain verification from `crab_city_auth`

#### Phase 4 Checklist

- [ ] Instance tunnels accepted and handshake works
- [ ] Per-user authentication within tunnels
- [ ] Message dispatch works for federated users
- [ ] Access rights enforced per federated grant
- [ ] Connection invites create federated accounts
- [ ] Tests pass

---

### Phase 5: TUI Integration

#### 5.1 Crab City Switcher

The top-level context switch. When viewing a remote Crab City, the entire TUI
context changes — instance list, chat, tasks, presence all come from the
remote host.

```
/connect switch "Bob's Workshop"
/connect switch home
```

Or a keybinding to cycle through connected Crab Cities.

- [ ] Add `viewing_context: CrabCityContext` to TUI state
      (`Local` or `Remote { host_node_id, host_name }`)
- [ ] Route all message dispatch through the context
- [ ] Instance list shows remote instances when viewing a remote context
- [ ] Status bar shows which Crab City is active

#### 5.2 /connect Commands

```
/connect invite [--for <label>] [--access <cap>] [--expires <dur>]
/connect join <token>
/connect list
/connect disconnect <name>
/connect remove <name>
/connect switch <name|home>
/connect access <user> <level>
/connect suspend <user>
```

- [ ] Wire each command to the appropriate handler
- [ ] `/connect list` shows connected + disconnected remote hosts
- [ ] `/connect invite` shows token + QR in a TUI panel

#### 5.3 Join Notifications

- [ ] Surface `MemberJoined` broadcasts as transient TUI notifications
- [ ] Show: display name, fingerprint, access level
- [ ] Auto-dismiss after 5 seconds

#### 5.4 Presence

Remote users in the presence display are annotated with their home instance:

```
deploy.sh: Bob (local), Alice (via Alice's Lab)
```

- [ ] Extend `PresenceUpdate` to carry `home_instance` annotation
- [ ] Display annotation in TUI presence view

#### 5.5 Terminal Dimensions

Remote viewports participate in dimension negotiation:

- [ ] Tag viewports with source (local vs remote + host_node_id)
- [ ] `MIN(all local viewports, all remote viewports)` for effective size
- [ ] Viewport removal on tunnel disconnect

#### Phase 5 Checklist

- [ ] Crab City switcher works (context switch is complete)
- [ ] `/connect` commands functional
- [ ] Join notifications appear and auto-dismiss
- [ ] Presence shows remote users with home instance annotation
- [ ] Terminal dimensions negotiate with remote viewports
- [ ] Tests pass

---

### Phase 6: Integration Tests

New file: `tests/federation_integration.rs`

End-to-end tests that spin up two Crab City instances and connect them.

#### 6.1 Connection + Auth

```rust
async fn instance_tunnel_established()
    // Instance A connects to Instance B → InstanceHello/Welcome exchanged

async fn federated_user_authenticates()
    // Alice (on A) authenticates on B → AuthResult with granted access

async fn unauthenticated_user_rejected()
    // Dave (on A, no federated account on B) → AuthResult error
```

#### 6.2 Connection Invite Flow

```rust
async fn create_connection_invite_and_redeem()
    // B creates invite → A connects → A redeems → federated account created

async fn connection_invite_expired()
    // Expired invite → specific error message

async fn connection_invite_revoked()
    // Revoked invite → specific error message
```

#### 6.3 Message Dispatch

```rust
async fn federated_user_receives_instance_list()
async fn federated_user_focuses_terminal_receives_output()
async fn federated_user_sends_input_with_collaborate_access()
async fn federated_user_input_rejected_with_view_access()
async fn federated_user_sends_chat()
async fn federated_user_sees_tasks()
```

#### 6.4 Multiplexing

```rust
async fn two_users_on_same_tunnel()
    // Alice and Dave (both on A) connect to B → each authenticates independently

async fn one_user_suspended_other_unaffected()
    // Suspend Alice → Dave still works
```

#### 6.5 Reconnection

```rust
async fn tunnel_reconnects_after_drop()
async fn users_reauthenticate_after_reconnect()
async fn replay_buffer_works_across_reconnect()
```

#### Phase 6 Checklist

- [ ] All integration tests pass
- [ ] Every RPC handler exercised
- [ ] Every error path tested
- [ ] Access gating verified for all capability levels

---

## New Code Layout

```
packages/crab_city/src/
  transport/
    connection_token.rs         MODIFY (v2 format with signed metadata)

  interconnect/
    mod.rs                      NEW (module re-exports)
    manager.rs                  NEW (ConnectionManager — home side)
    host.rs                     NEW (HostHandler — host side)
    proxy.rs                    NEW (per-user multiplexing)

  repository/
    federation.rs               NEW (federated_accounts + remote_crab_cities)

  config.rs                     MODIFY (add instance_name)
  cli/connect.rs                MODIFY (confirmation prompt, error messages)
  cli/invite.rs                 MODIFY (clipboard copy)

  migrations/
    NNN_federation.sql          NEW

tests/
  federation_integration.rs     NEW
```

## Dependency Order

```
Phase 1: S-Tier Invite UX          ← standalone, ships immediately
  │
  ├── Phase 2: Federation Data Model
  │     │
  │     ├── Phase 3: ConnectionManager (home side)
  │     │     │
  │     │     └── Phase 5: TUI Integration (needs manager API)
  │     │
  │     └── Phase 4: HostHandler (host side)
  │
  └── Phase 6: Integration Tests    ← incremental, alongside phases 3-5
```

Phases 3 and 4 can run in parallel (home side and host side are independent
until integration testing). Phase 5 depends on Phase 3 (TUI needs the
`ConnectionManager` API to switch contexts and forward messages).

## Estimated Scope

| Phase | New/Modified LOC | Test LOC | Notes |
|-------|-----------------|----------|-------|
| 1: Invite UX | ~150 | ~60 | Token v2, confirmation, clipboard, errors |
| 2: Data Model | ~240 | ~100 | Migration + repository CRUD |
| 3: ConnectionManager | ~400 | ~100 | Outbound tunnels, user auth, forwarding |
| 4: HostHandler | ~400 | ~100 | Inbound tunnels, dispatch, federation |
| 5: TUI Integration | ~350 | ~50 | Switcher, /connect commands, presence |
| 6: Integration Tests | — | ~400 | End-to-end two-instance tests |
| **Total** | **~1,540** | **~810** | |

Net: ~2,350 lines. Smaller than the original estimate because M1 infrastructure
already exists and handles the heavy lifting.

## Done Criteria

- [ ] v2 connection tokens carry signed metadata (instance name, inviter, capability)
- [ ] `crab invite` copies token to clipboard
- [ ] `crab connect` shows invite details and asks for confirmation before joining
- [ ] All error states show specific messages with recovery actions
- [ ] Host can create a connection invite (type 0x02)
- [ ] User can redeem connection invite and get a federated account
- [ ] Federated grant enforces access rights on every message
- [ ] Remote terminals appear in the TUI under the host's name
- [ ] Focusing a remote terminal streams output in real-time
- [ ] Terminal input works on remote terminals (with access gating)
- [ ] Terminal dimension negotiation includes remote viewports
- [ ] Terminal lock works for federated users
- [ ] Chat messages flow over the connection
- [ ] Tasks on the remote host are visible
- [ ] Presence shows federated users with home instance annotation
- [ ] Multiple local users share one tunnel, authenticate independently
- [ ] Each user's access is independent (no instance-wide access)
- [ ] Connection survives disconnection and reconnects automatically
- [ ] `/connect` TUI commands work
- [ ] Browser clients see remote Crab Cities (proxied through home instance)
- [ ] Event log records federation lifecycle events
- [ ] Bob inviting Alice does NOT give Alice's teammates access
- [ ] Integration tests cover auth, access gating, multiplexing, reconnection

## What Interconnect Is NOT

- **Symmetric.** Bob inviting Alice does not mean Alice invited Bob.
- **Instance-level.** Bob invites Alice, not "everyone on Alice's server."
- **Replication.** Terminal output streams live. Chat flows in real-time.
  Nothing is replicated.
- **Transitive.** If Alice has access to Bob's, and Carol has access to
  Alice's, Carol cannot see Bob's through Alice.
- **Registry-dependent.** Two instances can connect on a LAN with zero internet.

## Dependency Graph (Milestones)

```
M0: Auth Primitives (crab_city_auth) ✓
 +-- M1: Instance-Local Auth (iroh transport, identity, membership) ✓
 |    +-- Interconnect ← THIS MILESTONE
 |    |    +-- M3: Instance <-> Registry Integration
 |    |    |    +-- M6: Blocklists
 |    |    +-- M7: LAN Discovery (/connect discover)
 +-- M2: Registry Core
      +-- M3: Instance <-> Registry Integration
      +-- M4: OIDC Provider
           +-- M5: Enterprise SSO
```

Interconnect slots after M1 and before M2 because:
1. It requires M1's iroh transport and identity proofs (done)
2. It does NOT require the registry — two instances connect directly
3. M3 can later add registry-mediated discovery
4. The core value — connecting to another Crab City — ships before any
   registry infrastructure exists
