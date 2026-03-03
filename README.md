# Crab City

Run multiple Claude Code instances. Share them with your team. Search everything.

<!-- TODO: add screenshot -->

## Why Crab City?

**Before Crab City:**

- You want to run several Claude instances at once (one per task, one per repo)
  but managing them is a mess
- You want a teammate to see what Claude is doing on your machine, or pick up
  where you left off
- You want to search across all your past Claude conversations, not dig through
  JSONL files

Crab City fixes all of that. It's a local server that manages Claude Code
instances, multiplexes their terminals, and gives you a TUI and web dashboard to
drive everything.

## Install

```sh
curl -fsSL https://github.com/crabcity/crabcity/releases/latest/download/install.sh | bash
```

This installs the `crab` binary to `~/.local/bin`. You need [Claude
Code](https://docs.anthropic.com/en/docs/claude-code) installed separately.

To build from source instead, see [CONTRIBUTING.md](CONTRIBUTING.md).

## Quick Start

**Launch the TUI picker** (starts a background daemon automatically):

```sh
crab
```

This opens a terminal UI where you create and attach to Claude Code instances.
Each instance gets its own PTY — create as many as you want.

To switch between Claude and the picker/overview TUI, just type `CTRL+]`.

**Or start the server directly** for the web UI:

```sh
crab server
```

Then open `http://127.0.0.1:<port>` for the full dashboard: live terminal,
conversation viewer, task board, and instance management.

With a local server, just run `crab` to attach.

To switch between Claude and the picker/overview TUI, just type `CTRL+]`.

## Common Workflows

### Managing instances from the CLI

```sh
crab list                        # show running instances
crab attach swift-amber-falcon   # attach to an instance by name
crab kill <name-or-id>           # stop an instance
crab kill-server                 # stop the daemon and all instances
```

### Searching past conversations

Crab City imports your Claude conversation history into a searchable SQLite database:

```sh
crab server --import-all              # import everything from ~/.claude/projects/
crab server --import-from ~/project   # import a specific project
```

The web UI gives you full-text search across all conversations with syntax-highlighted code blocks and diffs.

### Sharing with your team

Set a profile and enable auth:

```sh
crab server --profile tunnel    # or --profile server
crab auth enable
```

Share the URL. The first person to register becomes admin.

| Profile  | Binds to    | Auth | Use case                        |
|----------|-------------|------|---------------------------------|
| `local`  | `127.0.0.1` | off  | Solo development (default)      |
| `tunnel` | `127.0.0.1` | on   | Tunneling (ngrok, cloudflared)  |
| `server` | `0.0.0.0`   | on   | LAN or public deployment        |

When sharing, multiple users can:
- **Watch the same instance** — with a lock system to coordinate typing
- **See who's online** — live presence shows who's connected and where
- **Chat** — real-time messaging overlaid on instances
- **Share tasks** — a task board tied to instances, synced across clients

Your local CLI (`127.0.0.1`) always bypasses auth, so `crab list` and `crab attach` keep working.

## Configuration

All config lives in `~/.crabcity/config.toml`:

```toml
profile = "local"

[auth]
enabled = false
session_ttl_secs = 604800      # 7 days
allow_registration = true

[server]
host = "127.0.0.1"
port = 0                       # 0 = auto-select
max_buffer_mb = 25             # output buffer per instance
hang_timeout_secs = 300        # hang detection (0 = disabled)
```

Layering: CLI flags > env vars > config.toml > profile defaults.

Environment variables use the `CRAB_` prefix with `__` as a section separator
(e.g. `CRAB_AUTH__ENABLED=true`).

Full reference: [docs/configuration.md](docs/configuration.md).

## FAQ

**What exactly runs when I type `crab`?**
A daemon process starts in the background (if not already running) that manages
all your Claude instances. The TUI connects to it. So does the web UI. Closing
the TUI doesn't kill your instances — they keep running until you `crab kill` them
or `crab kill-server`.

**How is this different from tmux + Claude Code?**
Crab City gives you things tmux can't: real-time state detection (is Claude
thinking? running a tool? idle?), a searchable conversation database, multi-user
auth, a web dashboard, and a task board. The terminal multiplexing is aware of
Claude's protocol, not just raw bytes.

**Does my data leave my machine?**
No. Everything is local — SQLite database, conversation logs, config. If you
enable the `server` profile, you're explicitly choosing to bind to a network
interface, but the data stays on that machine.

**Can I use this with other AI coding tools?**
Currently built for Claude Code only. The conversation importer, state detection,
and instance management are all Claude Code-specific.

**What platforms are supported?**
Linux (x86_64, aarch64) and macOS (Apple Silicon, Intel via Rosetta 2).

## Architecture

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

Each instance is an isolated Tokio task with its own PTY handle and virtual
terminal. The server detects Claude's state from conversation logs with a
terminal-heuristic fallback, and broadcasts changes to all connected clients
over WebSocket.

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
