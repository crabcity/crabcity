# Configuration Reference

Crab City uses layered configuration. Each layer overrides the one below it:

```
CLI flags  >  env vars  >  config.toml  >  profile defaults  >  struct defaults
```

## CLI Options

### `crab server`

| Flag | Description | Default |
|------|-------------|---------|
| `--profile <PROFILE>` | Configuration profile: `local`, `tunnel`, or `server` | `local` |
| `-p, --port <PORT>` | Port for the web server (0 = auto-select) | `0` |
| `-b, --host <HOST>` | Host to bind to | `127.0.0.1` |
| `--instance-base-port <PORT>` | Base port for instances | `9000` |
| `--default-command <CMD>` | Default command for new instances | — |
| `-d, --debug` | Enable debug logging | `false` |
| `--data-dir <PATH>` | Custom data directory | `~/.crabcity` |
| `--reset-db` | Reset database (with confirmation prompt) | — |
| `--import-all` | Import all existing Claude conversations on startup | — |
| `--import-from <PATH>` | Import conversations from a specific project directory | — |

### Other subcommands

| Command | Description |
|---------|-------------|
| `crab` | Start daemon + open TUI picker (default) |
| `crab attach <name-or-id>` | Attach to an instance by name or ID prefix |
| `crab list [--json]` | List running instances |
| `crab kill <name-or-id>` | Stop a specific instance |
| `crab kill-server` | Stop the daemon and all instances |
| `crab auth enable` | Enable authentication |
| `crab auth disable` | Disable authentication |
| `crab auth status` | Show current auth status |

## Profiles

Profiles set sensible defaults for common deployment scenarios. Specify with `--profile` or `profile = "..."` in config.toml.

| Profile  | Host        | Auth | Use Case                        |
|----------|-------------|------|---------------------------------|
| `local`  | `127.0.0.1` | off  | Solo development (default)      |
| `tunnel` | `127.0.0.1` | on   | Tunneling (ngrok, cloudflared)  |
| `server` | `0.0.0.0`   | on   | LAN or public deployment        |

### Profile Details

**`local`** — For single-user development on your own machine. Binds to localhost only, no authentication required. CLI commands work without credentials.

**`tunnel`** — For exposing your instance through a tunnel service. Binds to localhost (the tunnel handles external traffic) but enables auth so anyone with the tunnel URL must log in.

**`server`** — For LAN or public deployment. Binds to all interfaces and requires authentication. Use this when running on a shared server.

## Config File

Location: `~/.crabcity/config.toml`

Full annotated reference:

```toml
# Configuration profile (local | tunnel | server)
profile = "local"

[auth]
# Enable/disable authentication
enabled = false
# Session lifetime in seconds (default: 7 days)
session_ttl_secs = 604800
# Allow new user registration
allow_registration = true

[server]
# Host to bind to
host = "127.0.0.1"
# Port (0 = auto-select)
port = 0
# Maximum output buffer per instance in MB
max_buffer_mb = 25
# Maximum history bytes sent on focus switch in KB
max_history_kb = 64
# Hang detection timeout in seconds (0 = disabled)
hang_timeout_secs = 300
```

## Environment Variables

Every config field can be set via environment variable using the `CRAB_` prefix with `__` (double underscore) as the section separator.

| Variable | Config equivalent | Example |
|----------|-------------------|---------|
| `CRAB_PROFILE` | `profile` | `tunnel` |
| `CRAB_AUTH__ENABLED` | `auth.enabled` | `true` |
| `CRAB_AUTH__SESSION_TTL_SECS` | `auth.session_ttl_secs` | `604800` |
| `CRAB_AUTH__ALLOW_REGISTRATION` | `auth.allow_registration` | `true` |
| `CRAB_SERVER__HOST` | `server.host` | `0.0.0.0` |
| `CRAB_SERVER__PORT` | `server.port` | `8080` |
| `CRAB_SERVER__MAX_BUFFER_MB` | `server.max_buffer_mb` | `50` |
| `CRAB_SERVER__MAX_HISTORY_KB` | `server.max_history_kb` | `128` |
| `CRAB_SERVER__HANG_TIMEOUT_SECS` | `server.hang_timeout_secs` | `600` |

Legacy environment variables (still supported):

| Variable | Description | Default |
|----------|-------------|---------|
| `CRAB_CITY_MAX_BUFFER_MB` | Maximum output buffer per instance (MB) | `1` |
| `CRAB_CITY_MAX_HISTORY_KB` | Maximum history bytes sent on focus switch (KB) | `64` |
| `CRAB_CITY_HANG_TIMEOUT_SECS` | Hang detection timeout (0 = disabled) | `300` |

## Conversation Import

Crab City can import Claude conversation logs from `~/.claude/projects/` into its local SQLite database for full-text search and browsing.

### On startup

```sh
# Import all conversations from all projects
crab server --import-all

# Import from a specific project directory
crab server --import-from /path/to/project
```

### At runtime (via API)

```sh
# Import all conversations
curl -X POST http://localhost:PORT/api/admin/import \
  -H "Content-Type: application/json" \
  -d '{"import_all": true}'
```

The web UI provides a searchable notebook-style conversation viewer with syntax-highlighted diffs and code blocks.

## Data Directory

All runtime data lives in `~/.crabcity/` (override with `--data-dir`):

```
~/.crabcity/
├── config.toml          Configuration file
├── crabcity.db          SQLite database
├── exports/             Exported conversations
└── logs/                Server logs
```

## Web UI Build

The web UI is a SvelteKit application that can be embedded into the Rust binary behind a feature flag. For development, the server runs without the embedded UI.

### Building with embedded UI

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
