# Crab City

**WARNING: EXPERIMENTAL -- if you have fewer than 40 unique chromosomes, this
isn't for you.  Not safe for humans (yet).  Star to learn more.**

Make Claude multiplayer.  Make Claude better.

<!-- TODO: add screenshot -->

## Quick Start

```sh
git clone https://github.com/anthropics/crab-city && cd crab-city
cargo build -p crab_city
cargo run -p crab_city
```

This starts the server and opens a TUI picker where you can create and attach to
Claude Code instances. Open `http://127.0.0.1:<port>` in a browser for the web
UI.

**Prerequisites:** [Rust 1.91+](https://rustup.rs/), [Claude Code
CLI](https://docs.anthropic.com/en/docs/claude-code). See
[CONTRIBUTING.md](CONTRIBUTING.md) for full setup including Bazel and the
frontend.

## What You Get

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

Each Claude Code instance runs in its own PTY. The server manages lifecycle,
multiplexes terminal output over WebSocket, detects Claude's state
(idle/thinking/tool use) from conversation logs and terminal heuristics, and
broadcasts changes to all connected clients.

## Three Ways to Use It

### TUI Picker (default)

```sh
crab
```

The default mode. A terminal UI where you can create, browse, and attach to
instances. No browser required.

### Web UI

```sh
crab server
```

Starts the server in the foreground. Open `http://127.0.0.1:<port>` for the full
dashboard: live terminal emulator, conversation viewer, task board, and instance
management.

### CLI

```sh
crab list                        # show running instances
crab attach swift-amber-falcon   # attach to an instance by name
crab kill <name-or-id>           # stop an instance
crab kill-server                 # stop the daemon and all instances
```

## Multi-User Collaboration

Crab City supports multiple users sharing the same set of Claude instances in
real time:

- **Shared terminals** — multiple users watch the same instance; a lock system
  coordinates who can type
- **Live presence** — see who's connected and what they're viewing
- **Broadcast chat** — real-time messaging overlaid on instances
- **Task board** — create and assign tasks tied to instances

### Setting It Up

Profiles set sensible defaults for common deployment scenarios:

| Profile  | Host        | Auth | Use Case                        |
|----------|-------------|------|---------------------------------|
| `local`  | `127.0.0.1` | off  | Solo development (default)      |
| `tunnel` | `127.0.0.1` | on   | Tunneling (ngrok, cloudflared)  |
| `server` | `0.0.0.0`   | on   | LAN or public deployment        |

To enable multi-user access:

```sh
# 1. Start with a multi-user profile
crab server --profile tunnel    # or --profile server

# 2. Enable auth and create an admin account
crab auth enable

# 3. Share the URL with your team
```

The first user to register becomes the admin. Auth uses JWT sessions with a
configurable TTL. Loopback requests (127.0.0.1) bypass auth so your local CLI
always works.

See [docs/configuration.md](docs/configuration.md) for the full config
reference.

## Configuration

Configuration uses layers — each overrides the one below it:

```
CLI flags  >  env vars  >  config.toml  >  profile defaults  >  struct defaults
```

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

Environment variables use the `CRAB_` prefix with `__` as the section separator
(e.g. `CRAB_AUTH__ENABLED=true`). See
[docs/configuration.md](docs/configuration.md) for the complete reference.

## Conversation History

Crab City imports Claude conversation logs from `~/.claude/projects/` into a
local SQLite database with full-text search. The web UI provides a searchable
notebook-style conversation viewer with syntax-highlighted diffs and code
blocks.

```sh
crab server --import-all              # import everything on startup
crab server --import-from ~/project   # import a specific project
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
cargo check -p crab_city          # quick compile check
cargo test -p crab_city           # run unit tests
bazel test //...                  # full CI build + tests
bazel run //tools/format          # format code (never use rustfmt directly)
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full development guide.

## Documentation

- [Configuration Reference](docs/configuration.md) — CLI options, profiles, config.toml, env vars
- [Architecture](docs/architecture.md) — system design, WebSocket protocol, state detection
- [Operations](docs/operations.md) — endpoints, metrics, troubleshooting, database management
- [Contributing](CONTRIBUTING.md) — dev setup, building, testing, code style

## License

Apache 2.0 — Copyright 2025 Empathic, Inc.
