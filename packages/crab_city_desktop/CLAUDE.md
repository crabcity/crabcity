# crab_city_desktop — CLAUDE.md

Native desktop app for Crab City using Tauri 2. Wraps the SvelteKit web UI in a native window, with daemon lifecycle management and native OS integration.

## Architecture

The Tauri webview loads `http://localhost:{port}` from the daemon's HTTP server. The SvelteKit frontend uses relative paths for API calls and derives WebSocket URLs from `window.location`, so zero frontend changes are needed.

```
src/
├── main.rs      Tauri setup, menus, tray, window lifecycle, daemon startup, health monitor
├── daemon.rs    Daemon discovery/start/health/stop (mirrors cli/daemon.rs)
└── config.rs    Minimal path config for daemon file locations
```

**Daemon ownership**: The app tracks whether it started the daemon (`we_started` flag). On app exit, it only stops the daemon if it started it — pre-existing daemons (from CLI/TUI) survive.

## Native OS Integration

- **Menu bar**: App menu (About, Settings `Cmd+,`, Quit `Cmd+Q`), Edit (undo/redo/cut/copy/paste/select_all), View (Reload `Cmd+R`, Toggle DevTools `Cmd+Alt+I`, Fullscreen), Window (minimize/zoom/close)
- **System tray**: Left-click shows window, right-click context menu (Show Window, Quit). Tooltip shows "Crab City"
- **Window state persistence**: `tauri-plugin-window-state` saves/restores position, size, maximized state across launches. Window starts hidden (visible: false) to prevent flash before state restore
- **macOS close behavior**: Closing the window (Cmd+W / red X) hides it — the app stays running in the tray. Cmd+Q or tray Quit actually exits
- **Smooth loading transition**: Loading screen fades out before navigating to the daemon URL (no white flash)

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

Two terminals:
1. `cargo run -p crab_city -- server --port 0` — start daemon
2. `cd packages/crab_city_desktop && cargo tauri dev` — launches Vite dev server automatically (`beforeDevCommand`), then opens Tauri window

In dev mode, Tauri loads `http://localhost:5173` (Vite), which proxies `/api/*` and WebSocket to the daemon via `dynamicBackendProxy` in `vite.config.ts`.

## Key Patterns

### Loading Screen IPC

The loading page (`loading.html`) communicates with Rust via two mechanisms:
- **Rust → JS**: `window.eval()` calls global functions (`setStatus()`, `showError()`, `fadeOutAndNavigate()`)
- **JS → Rust**: Retry button invokes `retry_daemon_startup` Tauri command via `__TAURI_INTERNALS__.invoke()`

### macOS Window Lifecycle

- `CloseRequested` → `window.hide()` + `api.prevent_close()` (keeps app in tray)
- `RunEvent::Exit` → stop daemon if we started it (true quit via Cmd+Q or tray)
- Non-macOS: `CloseRequested` stops daemon and closes normally

## Key Differences from CLI daemon.rs

- Uses `which crab` instead of `current_exe()` to find the server binary
- `ensure_daemon()` returns `(DaemonInfo, we_started: bool)` for ownership tracking
- No `DaemonError` enum or tungstenite support — simpler error handling via `anyhow`
- Tests use the same patterns (temp_config, spawn_health_server) as the CLI tests
