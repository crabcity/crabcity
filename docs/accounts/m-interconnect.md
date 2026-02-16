# Milestone: Interconnect

**Goal:** Do what we already do over WebSockets, but over iroh between Crab
City instances. A user's TUI or browser shows all the Crab Cities they have
access to, and they switch between them. The transport is iroh. The protocol
is the same. The authorization is per-account.

**Depends on:** M1 (iroh transport, instance identity, membership)

## What Interconnect Means

Today a crab city instance is a standalone island. You connect to it, see its
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

## The Model: Federated Accounts

Interconnect is a federation system. When Bob invites Alice to connect to his
Crab City, he's creating an **account on his instance for Alice's account on
her instance**. Alice's key on Alice's server gets a corresponding account on
Bob's server, with whatever access Bob chooses to grant.

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

Bob's instance creates a **federated account** for Alice. This account is
keyed to Alice's public key — the same ed25519 key that identifies her on her
home instance. It's a real account on Bob's server, with its own grant, just
linked to an identity that lives on another server.

```rust
FederatedAccount {
    account_key: PublicKey,          // Alice's ed25519 pubkey (same as on her home instance)
    display_name: String,            // "Alice"
    home_instance: Option<PublicKey>, // Alice's home instance NodeId
    home_name: Option<String>,       // "Alice's Lab"
    access: AccessRights,            // what Alice can do here
    state: GrantState,               // active, suspended
    created_by: PublicKey,           // Bob (admin who created this account)
    created_at: u64,
}
```

The access rights are the same GNAP-style rights as local member grants:

| Scenario | Access Rights |
|----------|---------------|
| View only | `content:read`, `terminals:read` |
| Collaborate | + `terminals:input`, `chat:send`, `tasks:read,create,edit` |
| Full | + `members:read` |

From Bob's instance's perspective, Alice has an account — just one whose
identity originates from another server. She appears in presence, can hold
terminal locks, can chat, can create tasks. The only difference is how she
connects (through her home instance over iroh, rather than directly).

### Each Direction Is Independent

If Bob invites Alice, Alice can see Bob's Crab City. That does **not** mean
Bob can see Alice's. For that, Alice would need to separately invite Bob.

```
Bob invites Alice:
  Bob's instance: grant for Alice's pubkey ✓
  Alice's instance: bookmark for Bob's instance ✓
  Alice → can view Bob's stuff
  Bob → cannot view Alice's stuff

Alice also invites Bob:
  Alice's instance: grant for Bob's pubkey ✓
  Bob's instance: bookmark for Alice's instance ✓
  Bob → can now view Alice's stuff too
```

The two grants are independent. They may have completely different access
levels:

```
Bob → Alice: collaborate (terminals:read,input + chat:send + tasks:read,create)
Alice → Bob: view only (content:read + terminals:read)
```

Alice can type in Bob's terminals. Bob can only watch Alice's.

### Local Accounts vs. Federated Accounts

Both are accounts on the same instance, both use the same access rights and
authorization model. The difference is where the identity originates:

| | Local Account | Federated Account |
|---|---|---|
| **Identity** | Created locally | Originates from another server |
| **Keyed to** | Account pubkey | Account pubkey (same key as on home server) |
| **Access rights** | GNAP AccessRights | GNAP AccessRights (same) |
| **Connects via** | Direct (WebSocket or iroh) | Home instance → iroh → host |
| **Authenticates** | iroh handshake (M1) | Identity proof over iroh tunnel |
| **Appears in presence** | Yes | Yes (annotated with home instance) |
| **Can hold terminal lock** | Yes | Yes |
| **Created by** | Member invite (type 0x01) | Connection invite (type 0x02) |

The capability algebra works identically. Alice's access on Bob's instance is
determined solely by her federated account's grant on Bob's instance. Her
permissions on her home instance are irrelevant — Bob's server is the
authority for what Alice can do on Bob's server.

## Protocol

### The Insight: Same Protocol

An instance connecting to a remote host on behalf of its users is essentially
a transport proxy. The remote host speaks the same `ClientMessage`/`ServerMessage`
protocol that WebSocket clients already speak. No new message types are needed.

```
Alice's TUI ──ws──► Alice's Instance ──iroh──► Bob's Instance
                    (transport proxy)           (host)
```

Alice's instance maintains an iroh connection to Bob's. When Alice focuses
one of Bob's terminals, her instance forwards the `Focus` message to Bob's
instance over iroh. Bob's instance responds with `FocusAck`, `OutputHistory`,
etc. — the same flow as a local WebSocket client.

### Transport: Instance-to-Instance

The iroh connection is between instances (identified by instance NodeId). This
is shared infrastructure — if both Alice and Dave on the same home instance
have grants on Bob's Crab City, their messages travel over the same iroh
connection. But each user **authenticates individually** within that tunnel.

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
    |  instance_hello:                     |
    |  { instance_name, node_id }          |
    | ------------------------------------> |
    |                                       |
    |  instance_welcome:                   |
    |  { instance_name }                   |
    | <------------------------------------ |
    |                                       |
    |  transport tunnel established        |
    |  users authenticate individually     |
```

The instance-level connection is just a transport tunnel. Authorization
happens per-user, per-message.

### User Authentication

When Alice first focuses a remote terminal on Bob's instance, her home
instance sends an identity proof on her behalf:

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
account key. The host instance checks each message against her federated
grant.

### Message Flow

After authentication, the connection carries the existing protocol:

**Home → Host** (subset of `ClientMessage`, tagged with user):
- `Focus { instance_id, account_key }` — Alice wants to view this terminal
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
doesn't have `terminals:input`, her `Input` messages are rejected. The host
treats federated grants exactly like local member grants for authorization.

### Terminal Dimensions

Remote viewers participate in the same dimension negotiation as local viewers.
When Alice focuses Bob's `deploy.sh` terminal:

1. Alice's home instance sends `TerminalVisible` with Alice's viewport size
2. Bob's instance adds that viewport to the VirtualTerminal for `deploy.sh`
3. Effective size: `MIN(all local viewports, all remote viewports)`
4. If Alice has a smaller terminal, everyone's terminal shrinks

This is the existing VirtualTerminal dimension negotiation — remote viewports
are just another entry in the viewport set.

### Terminal Lock

The existing terminal lock extends to federated users:

- A federated user can acquire the lock if their grant includes
  `terminals:input`
- `TerminalLockUpdate` broadcasts include the holder's display name and home
  instance
- Lock timeout (120s) applies equally to local and federated holders

### Chat

Chat messages from federated users appear like any other user, annotated with
their home instance:

```
[Bob's Workshop]
  Bob: the deploy is running
  Alice (Alice's Lab): looks good, tests passing on my end
```

The host stores chat messages with the federated user's identity. Chat history
queries return them normally.

### Tasks

Tasks are visible per the federated grant. If Alice's grant includes
`tasks:read`, she sees all tasks on Bob's instance. If it includes
`tasks:create`, she can create tasks there. Tasks live in the host's database
— the host is the source of truth.

### Presence

The combined presence view on the host shows local and federated users:

```
deploy.sh: Bob (local), Alice (via Alice's Lab)
editor: Carol (local)
```

Alice's home TUI shows the same presence info for Bob's instance, received via
`PresenceUpdate` messages from the host.

## Data Model

### Host-Side Schema

```sql
-- Federated accounts (accounts on this server for identities from other servers)
CREATE TABLE federated_accounts (
    account_key BLOB NOT NULL PRIMARY KEY,  -- 32 bytes, ed25519 pubkey (same as on home server)
    display_name TEXT NOT NULL,
    home_node_id BLOB,                      -- 32 bytes, their home instance's NodeId
    home_name TEXT,                          -- "Alice's Lab" (display only)
    access TEXT NOT NULL DEFAULT '[]',       -- JSON: access rights
    state TEXT NOT NULL DEFAULT 'active',    -- 'active', 'suspended'
    created_by BLOB NOT NULL,               -- pubkey of admin who created this account
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
```

### Home-Side Schema

```sql
-- Remote Crab Cities this user can connect to
CREATE TABLE remote_crab_cities (
    host_node_id BLOB NOT NULL,              -- 32 bytes, host instance's NodeId
    account_key BLOB NOT NULL,               -- 32 bytes, local user this bookmark belongs to
    host_name TEXT NOT NULL,                  -- "Bob's Workshop"
    granted_access TEXT NOT NULL DEFAULT '[]', -- JSON: what the host granted us (cached)
    auto_connect INTEGER NOT NULL DEFAULT 1,  -- connect on startup?
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (host_node_id, account_key)
);
```

The home-side table is per-user. If both Alice and Dave have grants on Bob's
instance, there are two rows — one for each account.

### Event Log

Connection lifecycle events extend the existing event log:

**Host-side events:**
- `federation.granted` — new federated grant created
- `federation.access_changed` — grant access rights updated
- `federation.suspended` — grant suspended
- `federation.removed` — grant removed
- `federation.authenticated` — remote user authenticated (informational)
- `federation.disconnected` — remote user disconnected (informational)

## Establishing a Connection

### Flow: Invite Token

```
Bob's TUI:
  > /connect invite
  Connection invite for Bob's Workshop:
  Token: CRAB7X9M2KP4...  (copy or share)
  Access: collaborate
  Expires: 1 hour

Alice's TUI:
  > /connect join CRAB7X9M2KP4...
  Connecting to Bob's Workshop...
  Authenticated as Alice.
  Access: collaborate
  3 terminals available.
```

Under the hood:

1. Bob creates a `ConnectionInvite` — a signed token containing his instance's
   NodeId, relay info, access rights, expiry. Same wire format as member
   invites but with a different type byte.
2. Alice's instance parses the token and connects to Bob's instance over iroh
   (if not already connected).
3. Alice authenticates with her identity proof over the iroh tunnel.
4. Bob's instance verifies the invite + Alice's identity, creates a
   `federated_accounts` row for Alice's pubkey.
5. Alice's instance stores a `remote_crab_cities` row for Alice.
6. Alice sees Bob's instance in her sidebar.

Note: this creates a grant for **Alice specifically**. Alice's teammate Dave
gains nothing from this invite. Dave would need Bob to create a separate
invite, or Bob to invite Dave directly.

### Flow: LAN Discovery (future, M7)

```
Bob's TUI:
  > /connect discover
  Scanning...
  Found: "Alice's Lab" (nearby)
  Invite Alice to connect? [y/n] > y
  Access level? [view/collaborate] > collaborate
  Invite sent.
```

### Flow: Registry-Mediated (future, M3+)

The registry can broker introductions, but the actual connection is always
direct iroh. The registry is a matchmaker, not a relay.

## Client Experience

### Switching Crab Cities

The core UI change: a **top-level context switch** between Crab Cities. When
you switch to Bob's Crab City, you get the full Bob's Crab City experience —
his instance list, his chat, his tasks, his presence. Not a blended view.

```
Alice viewing her own Crab City:

  ┌ Alice's Lab ──────────── [▼] ┐   ← Crab City switcher
  │ ► experiments.sh  (running)  │
  │   data-pipeline   (idle)     │
  └──────────────────────────────┘
  Chat: Alice's #general
  Tasks: Alice's tasks
  Presence: Alice, Dave

Alice switches to Bob's:

  ┌ Bob's Workshop ──────── [▼] ┐    ← same switcher, different context
  │ ► deploy.sh       (running) │
  │   editor           (idle)   │
  │   data-pipeline    (running)│
  └─────────────────────────────┘
  Chat: Bob's #general
  Tasks: Bob's tasks
  Presence: Bob, Alice (via Alice's Lab)
```

It's the same holistic Crab City experience, just for a different Crab City.
The switcher (dropdown, keybinding, however the TUI presents it) is the only
new UI element. Everything else — instance list, terminal view, chat panel,
task list, presence — comes from whichever Crab City you're currently viewing.

**TUI**: The switcher could be a header bar, a keybinding cycle, or a command:
```
/connect switch Bob's Workshop
/connect switch home
```

**Web UI**: Same model. A top-level selector in the sidebar or header. The
browser connects to its home instance; the home instance proxies everything
from the remote host.

```
Browser ──ws/iroh──► Home Instance ──iroh──► Remote Host
```

No additional connections per remote host. The client protocol is unchanged.
When viewing a remote Crab City, every `ClientMessage` goes to the remote host
and every `ServerMessage` comes from it — the home instance is a transparent
proxy for the currently-viewed context.

### Reconnection

If the iroh connection to a remote host drops:

1. Remote instances show as "(disconnected)" in the sidebar
2. Home instance attempts reconnection with exponential backoff
3. On reconnect, re-authenticate and replay missed output (same ring buffer
   mechanism as WebSocket reconnection)
4. The user sees a brief interruption, then normal service resumes

## Implementation

### New Code

```
packages/crab_city/src/
  interconnect/
    mod.rs              # FederatedGrant, ConnectionInvite types
    manager.rs          # outbound connections to remote hosts (home side)
    host.rs             # inbound connections, per-user auth and dispatch
    proxy.rs            # user multiplexing over shared iroh tunnel
```

### Connection Manager (Home Side)

```rust
pub struct ConnectionManager {
    /// Active iroh connections to remote hosts, keyed by host NodeId
    tunnels: HashMap<PublicKey, InstanceTunnel>,
    /// Database handle
    db: DbPool,
    /// Broadcast channel to forward remote events to local clients
    lifecycle_tx: broadcast::Sender<ServerMessage>,
    /// Local instance identity (for iroh transport)
    identity: InstanceIdentity,
}

struct InstanceTunnel {
    host_node_id: PublicKey,
    host_name: String,
    stream: Option<QuicBiStream>,     // None if disconnected
    state: TunnelState,               // Connected, Disconnected, Reconnecting
    /// Which local users are authenticated on this tunnel
    authenticated_users: HashSet<PublicKey>,
}
```

**Lifecycle:**
1. On startup, query `remote_crab_cities` with `auto_connect = true`
2. Group by `host_node_id` — one iroh connection per remote host
3. Attempt connection to each host
4. On success, exchange `instance_hello`/`instance_welcome`
5. As users focus remote terminals, authenticate them on-demand
6. Forward received `ServerMessage` variants to local `lifecycle_tx`
7. On disconnect, attempt reconnection with exponential backoff

### Host Handler (Host Side)

Extends the existing iroh accept loop. When an instance tunnel is established:

1. Accept `instance_hello`, respond with `instance_welcome`
2. Spawn a handler task per tunnel
3. On receiving `Authenticate`, verify the identity proof, look up the
   `federated_accounts` row for that pubkey
4. For subsequent messages tagged with that pubkey, check access rights
   and dispatch (same logic as the WebSocket handler)

This is structurally identical to `handle_multiplexed_ws` but:
- Reads from an iroh stream instead of a WebSocket
- Multiplexes multiple users over one connection
- Uses federated grants instead of local member grants for authorization

### Integration with Existing Systems

**Instance Manager:** No changes. Remote instances are not local PTYs.

**State Manager:** No changes. The ConnectionManager (home side) uses the
existing `lifecycle_tx` to forward remote messages.

**WebSocket Handler:** No changes. Remote instances show up as additional
`ServerMessage` variants flowing through `lifecycle_tx`.

**VirtualTerminal:** Minor change — accept remote viewports (tagged with
source) in the viewport set. Dimension negotiation unchanged.

**Terminal Lock:** Minor change — lock holder can be a federated user.

### Transport Details

**One iroh connection per remote host.** Multiple QUIC streams for parallel
delivery:
- Stream 0: Control (hello/welcome, authenticate, disconnect)
- Stream 1: Terminal output (high bandwidth, independent flow control)
- Stream 2: Everything else (chat, tasks, presence, state changes)

**Message framing:** Length-prefixed JSON (matching existing WebSocket framing).
Each message: `[4-byte length][JSON payload]`.

**Backpressure:** Same `OutputLagged` mechanism as WebSocket clients.

## Connection Invite Token

Reuse the existing `Invite` wire format with a new type byte:

```
ConnectionInvite = {
    version: u8,                    // 0x02 (vs 0x01 for member invites)
    instance: [u8; 32],            // host instance's NodeId
    links: [InviteLink],          // chain of 1 (no delegation)
}
```

The `capability` field on the link maps to federated access:
- `View` → `content:read`, `terminals:read`
- `Collaborate` → + `terminals:input`, `chat:send`, `tasks:read,create`

Connection invites are always flat (no delegation). Only instance owners/admins
can create them.

## TUI Commands

```
/connect invite [access]      Create a connection invite (default: collaborate)
/connect join <token>         Accept an invite, connect to remote host
/connect list                 List remote Crab Cities and federated grants
/connect disconnect <name>    Disconnect from a remote host
/connect remove <name>        Remove a federated grant or remote bookmark
/connect access <user> <lvl>  Change a federated grant's access level
/connect suspend <user>       Suspend a federated grant
```

## What Interconnect Is

Interconnect is a **federation system**. Instances remain sovereign — each has
its own membership, event log, and data. But accounts can have relationships
across instances. Bob can grant Alice access to his Crab City, and Alice
experiences it as a seamless extension of her own TUI.

## What Interconnect Is NOT

- **Symmetric.** Bob inviting Alice does not mean Alice invited Bob. Each
  direction is an independent grant for a specific account.
- **Instance-level.** Bob invites Alice, not "everyone on Alice's server."
  Alice's teammate Dave has no access unless Bob also invites Dave.
- **Replication.** Terminal output streams live. Chat flows in real-time.
  Nothing is replicated to a local store.
- **Transitive.** If Alice has access to Bob's instance and Carol has access
  to Alice's, Carol cannot see Bob's instance through Alice.
- **Registry-dependent.** Two instances can connect on a LAN with zero
  internet. The registry can help with discovery (M3+) but is not required.

## Estimated Scope

| Component | LOC | Notes |
|-----------|-----|-------|
| Federated grant / invite types | ~150 | Grant, invite, identity proof |
| ConnectionManager (home side) | ~350 | Outbound tunnels, reconnection |
| Host handler | ~350 | Per-user auth, access gating, dispatch |
| User multiplexing proxy | ~200 | Multi-user over shared tunnel |
| Database migration | ~40 | Two tables (host + home side) |
| Repository CRUD | ~200 | federated_accounts + remote_crab_cities |
| Event log integration | ~50 | 6 event types |
| TUI: Crab City switching | ~250 | Two-level instance list, focus routing |
| TUI: /connect commands | ~200 | Invite, join, list, etc. |
| VirtualTerminal remote viewports | ~50 | Tag viewports with source |
| Terminal lock remote support | ~30 | Federated holder attribution |
| Integration tests | ~400 | Auth, access gating, multiplexing |
| **Total** | **~2,270** | |

## Done Criteria

- [ ] Host can create a connection invite token
- [ ] User can redeem invite and authenticate on remote host
- [ ] Federated grant enforces access rights on every message
- [ ] Remote terminals appear in the TUI under the host Crab City's name
- [ ] Focusing a remote terminal streams output in real-time
- [ ] Terminal input works on remote terminals (with access gating)
- [ ] Terminal dimension negotiation includes remote viewports
- [ ] Terminal lock works for federated users
- [ ] Chat messages flow over the connection
- [ ] Tasks on the remote host are visible
- [ ] Presence shows local + federated users with home instance annotation
- [ ] Multiple local users can connect to the same remote host independently
- [ ] Each user authenticates individually (no instance-wide access)
- [ ] Connection survives disconnection and reconnects automatically
- [ ] `/connect` TUI commands work
- [ ] Browser clients see remote Crab Cities (proxied through home instance)
- [ ] Event log records federation lifecycle events
- [ ] Bob inviting Alice does NOT give Alice's teammates access (account-level)
- [ ] Integration tests cover per-user auth, access gating, and reconnection

## Dependency Graph (Updated)

```
M0: Foundations
 +-- M1: Instance-Local Auth (iroh)
 |    +-- Interconnect <── THIS MILESTONE
 |    |    +-- M3: Instance <-> Registry Integration
 |    |    |    +-- M6: Blocklists
 |    |    +-- (M3 can also enable registry-mediated discovery)
 |    +-- M7: Iroh Invite Discovery (also enables /connect discover)
 +-- M2: Registry Core
      +-- M3: Instance <-> Registry Integration
      +-- M4: OIDC Provider
           +-- M5: Enterprise SSO
```

Interconnect slots after M1 and before M2 because:
1. It requires M1's iroh transport and identity proofs
2. It does NOT require the registry — two instances connect directly
3. M3 (registry integration) can later add registry-mediated discovery
4. This ordering means the core value proposition — connecting to another Crab
   City — ships before any registry infrastructure exists
