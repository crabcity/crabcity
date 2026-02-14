# pty_manager

Pure PTY lifecycle management library. Spawns child processes in pseudo-terminals, streams their output via tokio channels, and handles process termination with signal support.

## Usage

```rust
use pty_manager::{PtyManager, PtyConfig, PtyEvent};

let manager = PtyManager::new();

let config = PtyConfig {
    command: "claude".into(),
    args: vec![],
    working_dir: Some("/path/to/project".into()),
    env: vec![("TERM".into(), "xterm-256color".into())],
    rows: 24,
    cols: 80,
};

let (id, mut rx) = manager.spawn(config).await?;

// Stream output
while let Some(event) = rx.recv().await {
    match event {
        PtyEvent::Output { data, .. } => { /* terminal bytes */ }
        PtyEvent::Exited { exit_code, signal, .. } => { break; }
    }
}
```

## Design

- **No HTTP dependencies** — this is a pure async PTY library
- Uses `portable-pty` for cross-platform PTY support
- Output is delivered as `PtyEvent` variants over a tokio broadcast channel
- Process signals (kill, resize) are sent via the `PtyManager` API
- Unix-only signal handling via `nix`

## Modules

- `manager.rs` — `PtyManager` struct: spawn, kill, resize, subscribe
- `pty.rs` — `PtyHandle`, `PtyConfig`, `PtyState`, `PtyEvent`
- `error.rs` — `PtyError` type

## Testing

```sh
cargo test -p pty_manager
```
