# Architecture

This document covers the internal design of Crab City for contributors and curious users.

## Three-Layer Architecture

```
┌─────────────────────────────────────────────────┐
│  Web UI (SvelteKit)  or  TUI (ratatui)          │
├─────────────────────────────────────────────────┤
│  Server (axum)                                  │
│  ┌──────────┐ ┌──────────┐ ┌──────────────────┐ │
│  │ Instance │ │ WebSocket│ │ Conversation     │ │
│  │ Manager  │ │ Mux      │ │ Import + Search  │ │
│  └──────────┘ └──────────┘ └──────────────────┘ │
├─────────────────────────────────────────────────┤
│  PTY Layer (pty_manager + virtual_terminal)     │
│  ┌──────────┐ ┌──────────┐ ┌──────────────────┐ │
│  │ claude   │ │ claude   │ │ claude           │ │
│  │ instance │ │ instance │ │ instance         │ │
│  └──────────┘ └──────────┘ └──────────────────┘ │
└─────────────────────────────────────────────────┘
         SQLite (conversations, tasks, auth)
```

**Presentation layer** — The web UI (SvelteKit + xterm.js) and TUI (ratatui) are interchangeable frontends. Both connect to the server over WebSocket.

**Server layer** — An axum HTTP server that manages instance lifecycle, multiplexes terminal output, handles auth, and provides REST + WebSocket APIs.

**PTY layer** — Each Claude Code instance runs in its own pseudoterminal. The `pty_manager` crate handles lifecycle, `virtual_terminal` maintains a screen buffer and negotiates viewport dimensions across connected clients.

## Package Dependency Graph

```
crab_city (server + CLI + TUI)
├── claude_convo      (conversation log reader)
├── pty_manager        (PTY lifecycle)
├── virtual_terminal   (screen buffer + viewport negotiation)
└── compositor         (overlay layers for TUI)

tty_wrapper            (standalone HTTP-controlled PTY — not depended on by crab_city)
crab_city_ui           (SvelteKit frontend — embedded via rust-embed feature flag)
```

## Instance Lifecycle

Instances flow through: **Created → Running → Stopped**.

```
crab create / web UI "New Instance"
        │
        ▼
  instance_manager.rs
  ├── allocate PTY (pty_manager)
  ├── spawn claude CLI process
  ├── create virtual_terminal
  └── launch instance actor task
        │
        ▼
  instance_actor.rs (tokio::task)
  ├── owns: PTY handle, virtual terminal, compositor, client registry
  ├── reads PTY output → updates screen buffer → fans out to clients
  ├── receives client input → writes to PTY
  └── monitors process exit
        │
        ▼
  stop / process exit
  ├── cancel CancellationToken
  ├── clean up PTY
  └── update instance state
```

Each instance runs as a dedicated Tokio task following the **actor model**. The actor owns all instance-specific state and communicates with the rest of the system through channels.

## WebSocket Protocol

Two WebSocket endpoints:

| Endpoint | Purpose |
|----------|---------|
| `/api/instances/{id}/ws` | Single-instance terminal connection |
| `/api/ws` | Multiplexed connection (all instances, chat, presence, tasks) |

The multiplexed protocol uses `ServerMessage` (defined in `ws/protocol.rs`) — a tagged enum serialized as JSON. The design:

- **High-bandwidth path**: Terminal output from the ONE focused instance
- **Low-bandwidth path**: State changes from ALL instances
- **Focus switch**: Triggers a bounded history replay (max 64KB by default)

When adding new real-time features:
1. Add a variant to `ServerMessage` in `ws/protocol.rs`
2. Handle it in `ws/handler.rs` (server side)
3. Handle it in `stores/ws-handlers.ts` (client side)

## State Detection Pipeline

Claude's state (idle, thinking, tool use, streaming) is detected by two complementary systems:

```
Conversation JSONL log ──► conversation_watcher.rs ──┐
                                                      ├──► ClaudeState ──► broadcast
Terminal output patterns ──► inference/manager.rs ───┘
```

Priority of state signals (highest to lowest):

1. **Conversation JSONL `turn_duration` entry** — authoritative, emitted when Claude finishes a turn
2. **Conversation JSONL `end_turn` stop_reason** — authoritative
3. **Terminal output patterns** — heuristic analysis of screen content
4. **Timeout fallback** — safety net (10 seconds), prevents stuck states

The conversation watcher (`ws/conversation_watcher.rs`) tails the JSONL log file for structured state. The heuristic manager (`inference/manager.rs`) analyzes terminal output patterns as a fallback when log entries are delayed or missing.

State is exposed as `ClaudeState` in `inference/state.rs` and broadcast to all connected clients.

## Real-time Broadcast Pattern

All multi-user features (chat, presence, terminal lock, tasks, instance lifecycle) push updates to clients via `state_manager.broadcast_lifecycle(ServerMessage::...)`.

The pattern for mutation endpoints:

```
1. Mutate the database
2. Broadcast a ServerMessage variant with a FULL SNAPSHOT (not a diff)
3. Return the HTTP response
```

Full snapshots mean clients can join or reconnect at any time and immediately have consistent state. Client-side stores handle broadcasts idempotently (upsert by ID, not blind append) since the originating client receives both the HTTP response and its own broadcast echo.

See `handlers/tasks.rs` (`broadcast_task`, `broadcast_task_by_id`) and `repository::get_task_with_tags` for examples.

## Database

SQLite via sqlx with embedded migrations (`db.rs`).

### Schema overview

| Table(s) | Purpose |
|-----------|---------|
| `conversations`, `messages` | Imported Claude conversation history with full-text search |
| `tasks`, `task_tags` | Task board with tagging |
| `users`, `sessions` | Authentication and session management |
| `chat_messages` | Broadcast chat history |
| `instance_snapshots` | Periodic instance state persistence |

### Config

- **Location**: `~/.crabcity/crabcity.db` (configurable via `--data-dir`)
- **Migrations**: Embedded in the binary, run automatically on startup
- **Compile-time checked queries**: Uses `sqlx::query!` / `sqlx::query_as!`

## Auth

Auth middleware has a **loopback bypass** — CLI/TUI requests to `127.0.0.1` work without credentials. This means your local `crab` commands never need a token, even when auth is enabled for remote users.

For remote connections, auth uses JWT sessions with a configurable TTL. The first user to register becomes the admin.

## Terminal Multiplexing

Multiple clients share a single PTY per instance:

- `virtual_terminal` maintains the screen buffer and negotiates dimensions as `min(all active viewports)`
- `compositor` overlays UI elements (chat badges, status indicators) on the terminal output
- `websocket_proxy.rs` manages the fan-out from one PTY to N WebSocket clients

## Server Internals

- **Server loop** supports hot restart via `restart_tx` watch channel (config reload without process restart)
- **Config**: Figment-based layered configuration (`config.rs`)
- **Persistence**: Periodic instance state snapshots (`persistence.rs`) for recovery after restart
