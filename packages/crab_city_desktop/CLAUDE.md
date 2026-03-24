# crab_city_desktop — CLAUDE.md

Native desktop app for Crab City using Tauri 2. Embeds the `crab_city` server in-process (no separate daemon) and wraps the SvelteKit web UI in a native window with native OS integration.

## Architecture

The desktop app first checks for an existing healthy server (`check_existing_server()`). If found, it connects as an external client. Otherwise, it starts an `EmbeddedServer` in-process. The webview loads `http://127.0.0.1:{port}` either way.

```
src/
└── main.rs      Tauri setup, server discovery/lifecycle, menus, tray, window lifecycle
```

**Key types**:
- `ServerMode` — enum: `Embedded(EmbeddedServer)` (we own it) or `External { port }` (existing daemon)
- `AppState` — holds `Mutex<Option<ServerMode>>` and `AtomicU16` for the server port
- `EmbeddedServer` (from `crab_city::server`) — starts/stops the axum server, writes daemon files for CLI discovery

**Custom data directory**: `--data-dir /path/to/data` via clap (defaults to `~/.crabcity`).

## Native OS Integration

- **Menu bar**: App menu (About, Settings `Cmd+,`, Quit `Cmd+Q`), Edit (undo/redo/cut/copy/paste/select_all), View (Reload `Cmd+R`, Toggle DevTools `Cmd+Alt+I`, Fullscreen), Window (minimize/zoom/close)
- **System tray**: Left-click shows window, right-click context menu (Show Window, Quit). Tooltip shows "Crab City"
- **Window state persistence**: `tauri-plugin-window-state` saves/restores position, size, maximized state across launches. Window starts hidden (visible: false) to prevent flash before state restore
- **macOS close behavior**: Closing the window (Cmd+W / red X) hides it — the app stays running in the tray. Cmd+Q or tray Quit actually exits
- **Smooth loading transition**: Loading screen fades out before navigating to the server URL (no white flash)

## Build & Test

```sh
cargo check -p crab_city_desktop
cargo test -p crab_city_desktop
bazel build //packages/crab_city_desktop
bazel test //packages/crab_city_desktop:crab_city_desktop_test
```

### Bazel + Tauri Compatibility

Tauri's `generate_context!()` proc macro writes cache files to `OUT_DIR` during compilation (icon hashes, plist). In Bazel, `OUT_DIR` is read-only after the build script runs. The workaround is in `build.rs`: pre-create the exact cache files during the build script phase (where `OUT_DIR` IS writable), so `write_if_changed()` finds matching content and skips the write. The build-deps `png`, `blake3`, `plist`, and `serde_json` exist solely for this pre-creation.

Additionally, `ResolvedCommand` has `#[cfg(debug_assertions)]` fields. Bazel compiles proc macros in exec config (default: `opt`, no debug_assertions) but targets in `fastbuild` (with debug_assertions). This mismatch is fixed by `--host_compilation_mode=fastbuild` in `.bazelrc`.

## Dev Workflow

Single terminal: `cd packages/crab_city_desktop && cargo tauri dev`

This launches Vite's dev server (`beforeDevCommand`), then opens the Tauri window. The embedded server starts in-process and writes `daemon.port`, which Vite's `dynamicBackendProxy` reads to proxy `/api/*` and WebSocket requests.

In dev mode (`cargo tauri dev`), the webview loads Vite at `http://localhost:5173`. The embedded server still starts (so Vite can proxy to it), but the webview is not navigated away from the Vite URL.

## Key Patterns

### Loading Screen IPC

The loading page (`loading.html`) communicates with Rust via two mechanisms:
- **Rust → JS**: `window.eval()` calls global functions (`setStatus()`, `showError()`, `fadeOutAndNavigate()`)
- **JS → Rust**: Retry button invokes `retry_server_startup` Tauri command via `__TAURI_INTERNALS__.invoke()`

### Server Lifecycle

- **Discovery**: `start_or_discover_server()` first calls `check_existing_server()` — if a healthy daemon is found, sets `ServerMode::External` and spawns a health monitor
- **Embedded startup**: if no existing server, calls `EmbeddedServer::start()` (which acquires the daemon lock), sets `ServerMode::Embedded`
- **Health monitor**: for external servers, polls `health_check_port()` every 5s. On failure, shows loading page with "Server disconnected" error and retry button
- **Shutdown**: `RunEvent::Exit` checks `ServerMode` — `Embedded` triggers `server.shutdown()`, `External` leaves the daemon running

### macOS Window Lifecycle

- `CloseRequested` → `window.hide()` + `api.prevent_close()` (keeps app in tray)
- `RunEvent::Exit` → shut down embedded server (true quit via Cmd+Q or tray)

## Release Build (macOS .app bundle)

```sh
bazel build //packages/crab_city_desktop:macos_app
```

Produces `CrabCity.app` with the Tauri binary (which includes the embedded server) in `Contents/MacOS/`. No sidecar binary needed. The `macos_app` rule in `macos_app.bzl` uses a tree artifact (directory output) to assemble the bundle structure. Code signing and notarization are deferred to when public distribution is needed.
