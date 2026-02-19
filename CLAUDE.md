# CLAUDE.md

## Build System

This is a monorepo with Bazel, Cargo, and TS/JS build systems. Everything must stay in sync.

### Crate Features

When adding a crate feature (e.g. `reqwest`'s `blocking`), update **both**:
1. `Cargo.toml` — the package's `[dependencies]` features list
2. `MODULE.bazel` — the corresponding `crate_index.spec()` features list

### Formatting

Always use `bazel run //tools/format` to format code. Do not run `rustfmt` directly.

### Rust Edition 2024

All Rust code uses edition 2024. Cargo defaults to edition 2021 for `cargo check`/`cargo test`, so some edition 2024 errors only surface in Bazel. Known gotcha: `ref mut` in match/if-let patterns is disallowed when the default binding mode is already `ref mut` (e.g. matching on `&mut Option<T>` — use `Some(x)` not `Some(ref mut x)`).

### Build Commands

**All builds and tests MUST go through Bazel.** Do not use `pnpm`, `node`, `npx`, or `cargo test` directly. Cargo is only acceptable for quick `cargo check` compile checks.

- `cargo check -p crab_city` — quick compile check (Cargo OK here)
- `bazel test //packages/crab_city:crab_city_test` — run Rust unit tests for the server
- `bazel test //packages/<pkg>:*_test` — run unit tests for any workspace crate
- `bazel test //packages/crab_city_ui:typecheck_test` — frontend type checking
- `bazel test //packages/crab_city_ui:unit_tests` — frontend Jest tests
- `bazel test //...` — full CI-equivalent (includes format check, edition 2024)
- `bazel build //packages/crab_city:crab` — build the server binary
- `bazel build //packages/crab_city_ui:build` — build the web UI

## TUI Styling

The terminal theme is solarized. Hardcoded ANSI colors are invisible or clash:
- **Avoid**: `Color::DarkGray`, `Color::White`, `Color::Cyan`, `Color::Black`, `Color::Green`, `Color::Yellow`
- **Use modifiers**: `BOLD`, `DIM`, `REVERSED`, `ITALIC`, `UNDERLINED`
- **Exception**: `Color::Red` is safe (universally visible, use for errors)

## Architecture Notes

### Package Dependency Graph

```
crab_city (server + CLI + TUI)
├── claude_convo      (conversation log reader)
├── pty_manager        (PTY lifecycle)
├── virtual_terminal   (screen buffer + viewport negotiation)
└── compositor         (overlay layers for TUI)

tty_wrapper            (standalone HTTP-controlled PTY — not depended on by crab_city)
crab_city_ui           (SvelteKit frontend — embedded via rust-embed feature flag)
```

### Server Internals

- **Auth middleware** has a loopback bypass — CLI/TUI requests to `127.0.0.1` work without credentials
- **Server loop** supports hot restart via `restart_tx` watch channel (config reload without process restart)
- **Config layering**: struct defaults < profile defaults < config.toml < env vars < CLI flags < runtime overrides
- **Instance actor model**: each Claude instance runs in a dedicated `tokio::task` with its own PTY handle, virtual terminal, and client registry (`instance_actor.rs`)
- **Database**: SQLite via sqlx with embedded migrations (`db.rs`). Schema covers conversations, messages, tasks, users, sessions, chat, and instance snapshots

### Real-time Broadcast Pattern

All multi-user features (chat, presence, terminal lock, tasks, instance lifecycle) push updates to WebSocket clients via `state_manager.broadcast_lifecycle(ServerMessage::...)`. When adding a new mutation endpoint:

1. Mutate the DB
2. Broadcast a `ServerMessage` variant with a full snapshot (not a diff)
3. Return the HTTP response

Helpers in `handlers/tasks.rs` (`broadcast_task`, `broadcast_task_by_id`) and `repository::get_task_with_tags` show the pattern. Client-side stores must handle broadcasts idempotently (upsert by ID, not blind append) since the originating client receives both the HTTP response and its own broadcast echo.

### WebSocket Protocol

Single multiplexed endpoint: `/api/ws` — handles all instances, chat, presence, tasks, and terminal I/O. CLI `attach` also uses this endpoint (with `LoopbackAuth` + `Focus`).

The protocol uses `ClientMessage`/`ServerMessage` (defined in `ws/protocol.rs`) — tagged enums serialized as JSON. Auth is always challenge-response inside the WS connection (not HTTP middleware). Loopback clients without a keypair send `LoopbackAuth` to get Owner access; remote clients must use `ChallengeResponse` or `PasswordAuth`.

When adding new real-time features, add a variant to `ServerMessage` and handle it in `ws/handler.rs` (server) and `stores/ws-handlers.ts` (client).

### Inference Engine

Claude's state (idle/thinking/tool-use/streaming) is detected by two systems:
- **Conversation watcher** (`ws/conversation_watcher.rs`) — tails the JSONL log file for structured state
- **Heuristic manager** (`inference/manager.rs`) — analyzes terminal output patterns as a fallback

State is exposed as `ClaudeState` in `inference/state.rs` and broadcast to clients.

### Terminal Multiplexing

Multiple clients share a single PTY per instance:
- `virtual_terminal` maintains the screen buffer and negotiates dimensions as min(all active viewports)
- `compositor` overlays UI elements (chat badges, status indicators) on the terminal output
- `instance_actor.rs` manages the fan-out from one PTY to N WebSocket clients via the mux endpoint
