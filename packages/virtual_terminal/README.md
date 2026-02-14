# virtual_terminal

Virtual terminal that sits between clients and a PTY. Maintains a screen buffer via `vt100`, generates keyframe snapshots for efficient client attach, stores deltas (raw PTY output since last keyframe), and negotiates dimensions across multiple connected clients.

## Usage

```rust
use virtual_terminal::{VirtualTerminal, ClientType};

let mut vt = VirtualTerminal::new(24, 80, 64 * 1024); // rows, cols, max delta bytes

// Feed PTY output
vt.process_output(pty_bytes);

// Register a client viewport
if let Some((rows, cols)) = vt.update_viewport("client-1", 40, 120, ClientType::Web) {
    // Effective dimensions changed — resize the PTY
    pty.resize(rows, cols);
}

// Replay for a newly connected client (keyframe + deltas)
let replay = vt.replay();
client.send(replay);
```

## How It Works

### Keyframe + Delta Replay

Instead of replaying the entire PTY output history (which can be megabytes), the virtual terminal maintains:
1. **Keyframe** — a snapshot of the screen state as ANSI escape sequences
2. **Deltas** — raw PTY output accumulated since the last keyframe

When a new client connects, it receives `keyframe + deltas`, which is enough to reconstruct the current screen state. Auto-compaction triggers when deltas exceed the configured threshold.

### Viewport Negotiation

Multiple clients may have different terminal sizes. The effective dimensions are calculated as `min(rows) x min(cols)` across all active viewports. When a client disconnects or becomes inactive, dimensions are recalculated. The PTY is resized via SIGWINCH when effective dimensions change.

## API

- `new(rows, cols, max_delta_bytes)` — create with initial dimensions
- `process_output(data)` — feed PTY bytes, auto-compacts when threshold exceeded
- `compact()` — force a keyframe snapshot, clear deltas
- `replay()` — get keyframe + deltas for client attach
- `update_viewport(id, rows, cols, type)` — register/update client viewport
- `set_active(id, active)` — toggle client visibility
- `remove_client(id)` — unregister client
- `resize(rows, cols)` — direct resize of the underlying vt100 parser
- `effective_dims()` — current negotiated dimensions
- `screen()` — access the vt100 screen for cell-level reads

## Dependencies

Only `vt100` — no async runtime, no allocator, no network. This is a pure synchronous library.

## Testing

```sh
cargo test -p virtual_terminal
```
