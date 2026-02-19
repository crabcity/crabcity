# crab_city — CLAUDE.md

The main server, CLI client, and TUI picker. This is the largest package in the monorepo.

## Module Map

```
src/
├── main.rs              CLI entry point (clap), server loop, route registration
├── config.rs            Figment-based layered config (CrabCityConfig, ServerConfig, AuthConfig)
├── db.rs                SQLite init + embedded migrations
├── models.rs            Shared data types (Instance, Message, Task, User, etc.)
├── auth.rs              JWT/session auth, registration, middleware (loopback bypass)
│
├── cli/                 CLI subcommands (attach, list, kill, auth, daemon, picker)
├── handlers/            HTTP route handlers (instances, tasks, conversations, admin, notes)
├── repository/          Database query layer (conversations, tasks, auth, chat, search)
├── ws/                  WebSocket subsystem
│   ├── protocol.rs      ServerMessage enum — the wire format
│   ├── state_manager.rs Broadcast lifecycle, presence, terminal lock
│   ├── handler.rs       Message dispatch (client → server)
│   ├── conversation_watcher.rs  Tail JSONL logs for live conversation updates
│   ├── focus.rs         User focus tracking
│   └── session_discovery.rs
│
├── inference/           Claude state detection
│   ├── engine.rs        Structured state from conversation logs
│   ├── manager.rs       Heuristic fallback from terminal output
│   └── state.rs         ClaudeState enum
│
├── files/               File browsing (tree, content, search)
├── git/                 Git operations (diff, status, log, branches)
├── views/               Maud HTML templates (fallback when no embedded UI)
├── instance_actor.rs    Per-instance tokio task (PTY + virtual terminal + clients)
├── instance_manager.rs  Create/list/stop instances
├── import.rs            Claude conversation JSONL → SQLite importer
├── persistence.rs       Instance state snapshots (periodic + on-demand)
├── metrics.rs           Prometheus-style counters/gauges
├── notes.rs             Markdown note storage
├── onboarding.rs        First-run setup (admin account creation)
├── terminal.rs          Terminal emulation helpers
└── embedded_ui.rs       Conditional SPA serving (behind `embedded-ui` feature)
```

## Key Patterns

### Adding a New HTTP Endpoint

1. Write the handler in `handlers/<module>.rs`
2. Add the route in `main.rs` (in the router builder around line 567)
3. If it mutates shared state, follow the broadcast pattern (mutate DB → broadcast `ServerMessage` → return HTTP response)
4. Re-export the handler in `handlers/mod.rs`

### Adding a New WebSocket Message

1. Add a variant to `ServerMessage` in `ws/protocol.rs`
2. Handle incoming client messages in `ws/handler.rs`
3. Add the corresponding handler in the frontend's `stores/ws-handlers.ts`
4. Update the relevant frontend store to process the message

### Database Queries

All database access goes through `repository/`. Each file is a focused domain:
- `repository/tasks.rs` — task CRUD + tag operations
- `repository/conversations.rs` — full-text search, retrieval
- `repository/auth.rs` — user/session management
- `repository/chat.rs` — broadcast chat messages

Use `sqlx::query!` / `sqlx::query_as!` for compile-time checked queries.

### Instance Lifecycle

Instances flow through: Created → Running → Stopped. The actor model (`instance_actor.rs`) owns the PTY handle, virtual terminal, and compositor for each instance. The `instance_manager.rs` coordinates creation and teardown.

## Testing

```sh
cargo test -p crab_city
```

Test helpers are in `test_helpers.rs`. Tests that need a database use `tempfile` for isolated SQLite instances.
