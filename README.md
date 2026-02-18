# Crab City

**WARNING: EXPERIMENTAL -- if you have fewer than 40 unique chromosomes, this
isn't for you.  Not safe for humans (yet).  Star to learn more.**

A terminal multiplexer and web-based manager for Claude Code instances with real-time collaboration.

Run multiple Claude Code sessions simultaneously, manage them from a web interface or TUI, share terminals with collaborators, and persist your entire conversation history in a searchable database.

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) (1.91.0+, edition 2024)
- [Claude Code](https://docs.anthropic.com/en/docs/claude-code) CLI installed
- [Bazel](https://bazel.build/) 8+ (for full builds) or Cargo (for development)

### Build and run

```sh
# Build with Cargo (fastest for local development)
cargo build -p crab_city

# Start the server + TUI picker
cargo run -p crab_city
```

This starts the daemon on `127.0.0.1` with a random port, then opens a TUI picker where you can create and attach to Claude Code instances.

To start just the server (for the web UI):

```sh
cargo run -p crab_city -- server
```

Then open `http://127.0.0.1:<port>` in your browser.

### Build with Bazel (CI-equivalent)

```sh
# Build everything
bazel build //packages/crab_city:crab

# Run all tests
bazel test //...
```

## How It Works

Crab City has three layers:

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

Each Claude Code instance runs in its own PTY. The server manages instance lifecycle, multiplexes terminal output over a single WebSocket per client, detects Claude's state (idle/thinking/tool use) from conversation logs and terminal heuristics, and broadcasts state changes to all connected clients.

## CLI Usage

The `crab` binary is both a server and a client:

```sh
# Default: start daemon + open TUI picker
crab

# Start daemon in the foreground
crab server

# Attach to an instance by name or ID prefix
crab attach swift-amber-falcon

# List running instances
crab list
crab list --json

# Kill a specific instance
crab kill <name-or-id>

# Stop the daemon and all instances
crab kill-server

# Manage authentication
crab auth enable
crab auth disable
crab auth status
```

### Server Options

```sh
crab server \
  --profile local \          # local | tunnel | server
  --port 8080 \              # 0 = auto-select
  --host 127.0.0.1 \
  --import-all \             # import existing Claude conversations
  --import-from ~/project \  # import from a specific project
  --debug                    # enable debug logging
```

## Configuration

Crab City uses layered configuration. Each layer overrides the one below it:

```
CLI flags  >  env vars  >  config.toml  >  profile defaults  >  struct defaults
```

### Profiles

Profiles set sensible defaults for common deployment scenarios:

| Profile  | Host        | Auth | Use Case                        |
|----------|-------------|------|---------------------------------|
| `local`  | `127.0.0.1` | off  | Solo development                |
| `tunnel` | `127.0.0.1` | on   | Tunneling (ngrok, cloudflared)  |
| `server` | `0.0.0.0`   | on   | LAN or public deployment        |

### Config File

`~/.crabcity/config.toml`:

```toml
profile = "local"

[auth]
enabled = false
session_ttl_secs = 604800
allow_registration = true

[server]
host = "127.0.0.1"
port = 8080
max_buffer_mb = 25
max_history_kb = 64
hang_timeout_secs = 300
```

### Environment Variables

Every config field can be set via `CRAB_` prefixed environment variables with `__` as the section separator:

```sh
CRAB_AUTH__ENABLED=true
CRAB_SERVER__PORT=8080
CRAB_SERVER__MAX_BUFFER_MB=50
```

## Collaboration

Crab City supports multiple users sharing the same set of Claude instances:

- **Real-time presence** — see who's connected and what they're viewing
- **Shared terminals** — multiple users can watch the same instance; a lock system coordinates who can type
- **Broadcast chat** — real-time messaging overlaid on instances
- **Task board** — create and assign tasks tied to instances

All multi-user state is pushed to clients via WebSocket as full snapshots (not diffs), so clients can join or reconnect at any time and immediately have consistent state.

## Conversation History

Crab City imports Claude conversation logs from `~/.claude/projects/` into a local SQLite database with full-text search:

```sh
# Import everything on startup
crab server --import-all

# Import a specific project
crab server --import-from /path/to/project
```

The web UI provides a searchable notebook-style conversation viewer with syntax-highlighted diffs and code blocks.

## Web UI

The embedded SvelteKit web interface features:

- Instance sidebar with live status indicators
- xterm.js terminal emulator
- Notebook-style conversation viewer
- Task board with tags and filtering
- Conversation history browser with search

### Building the Web UI

For development, the server runs without the embedded UI (it's behind a feature flag). To build with the UI embedded:

```sh
# Build the SvelteKit app
cd packages/crab_city_ui
pnpm install
pnpm build
cd ../..

# Build the Rust binary with embedded UI
CRAB_CITY_UI_PATH=packages/crab_city_ui/build cargo build -p crab_city --features embedded-ui
```

Or use Bazel, which handles everything automatically:

```sh
bazel build //packages/crab_city:crab
```

## Project Structure

```
packages/
├── crab_city/           Main server + CLI + TUI (axum, clap, ratatui)
├── crab_city_ui/        SvelteKit web frontend (Svelte 5, xterm.js)
│   └── crate/           Rust crate for embedding built UI assets
├── claude_convo/        Library for reading Claude conversation logs
├── pty_manager/         Pure async PTY lifecycle management
├── virtual_terminal/    VT100 screen buffer, keyframe/delta replay, viewport negotiation
├── compositor/          Cell-level terminal compositor with overlay layers
└── tty_wrapper/         Standalone HTTP-controlled TTY wrapper
```

Each package has its own README with usage examples and API documentation.

## Development

```sh
# Quick compile check
cargo check -p crab_city

# Run unit tests
cargo test -p crab_city

# Full CI build and test (includes format checks, edition 2024)
bazel test //...

# Format code (do not use rustfmt directly)
bazel run //tools/format
```

### Logging

```sh
# Default
RUST_LOG=crab_city=info cargo run -p crab_city -- server

# Debug
RUST_LOG=crab_city=debug cargo run -p crab_city -- server

# Specific modules
RUST_LOG=crab_city::ws=debug,crab_city::inference=trace cargo run -p crab_city -- server
```

### Data Directory

All runtime data lives in `~/.crabcity/`:

```
~/.crabcity/
├── config.toml          Configuration file
├── crabcity.db          SQLite database
├── exports/             Exported conversations
└── logs/                Server logs
```

## License

Apache 2.0 — Copyright 2025 Empathic, Inc.
