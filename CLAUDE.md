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

All rust code should use Rust edition 2024

### Build Commands

- `cargo check -p crab_city` — quick compile check
- `cargo test -p crab_city` — run unit tests
- `bazel test //...` — full CI-equivalent (includes format check, edition 2024)
- `CRAB_CITY_UI_PATH=packages/crab_city_ui/build cargo build -p crab_city_ui` — build embedded UI crate

## TUI Styling

The terminal theme is solarized. Hardcoded ANSI colors are invisible or clash:
- **Avoid**: `Color::DarkGray`, `Color::White`, `Color::Cyan`, `Color::Black`, `Color::Green`, `Color::Yellow`
- **Use modifiers**: `BOLD`, `DIM`, `REVERSED`, `ITALIC`, `UNDERLINED`
- **Exception**: `Color::Red` is safe (universally visible, use for errors)

## Architecture Notes

- Auth middleware has a loopback bypass — CLI/TUI requests to `127.0.0.1` work without credentials
- Server loop supports hot restart via `restart_tx` watch channel (config reload without process restart)
- Config layering: struct defaults < profile defaults < config.toml < env vars < CLI flags < runtime overrides

### Real-time Broadcast Pattern

All multi-user features (chat, presence, terminal lock, tasks, instance lifecycle) push updates to WebSocket clients via `state_manager.broadcast_lifecycle(ServerMessage::...)`. When adding a new mutation endpoint:

1. Mutate the DB
2. Broadcast a `ServerMessage` variant with a full snapshot (not a diff)
3. Return the HTTP response

Helpers in `handlers/tasks.rs` (`broadcast_task`, `broadcast_task_by_id`) and `repository::get_task_with_tags` show the pattern. Client-side stores must handle broadcasts idempotently (upsert by ID, not blind append) since the originating client receives both the HTTP response and its own broadcast echo.
