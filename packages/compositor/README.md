# compositor

Cell-level terminal compositor with overlay layers and ANSI rendering. Composites arbitrary rectangular layers on top of a vt100 screen buffer, producing ANSI escape sequences for display.

## Usage

```rust
use compositor::{Compositor, Anchor, Attrs};

let mut comp = Compositor::new();

// Add a 10x1 layer anchored to the top-right corner
let badge_id = comp.add_layer(Anchor::TopRight(0, 0), 10, 1, /*z_order=*/ 0);

// Write text into the layer
let attrs = Attrs { inverse: true, ..Default::default() };
comp.layer_mut(badge_id).unwrap().fill_text(0, 0, " LOCKED ", attrs);

// Full composition: base screen + all visible layers → ANSI bytes
let output = comp.compose(virtual_terminal.screen());

// Incremental update: paint just one layer (non-disruptive, saves/restores cursor)
let paint = comp.paint_layer(badge_id, screen_rows, screen_cols);

// Erase a layer's area
let clear = comp.clear_layer(badge_id, screen_rows, screen_cols);
```

## Design

### Layers

Each layer is a rectangular grid of `Option<Cell>` values. `None` cells are transparent (the base screen shows through). Layers are positioned via anchors:

- `Anchor::TopLeft(row_offset, col_offset)`
- `Anchor::TopRight(row_offset, col_offset)`
- `Anchor::BottomLeft(row_offset, col_offset)`
- `Anchor::BottomRight(row_offset, col_offset)`

Anchors resolve lazily against the current screen dimensions, so layers adapt to terminal resizes without explicit repositioning.

### Composition Order

Layers are sorted by `z_order` (ascending). Higher z-order layers overwrite lower ones. The base vt100 screen is always at the bottom.

### Rendering Modes

- **Full composition** (`compose`) — rebuilds the entire screen grid with all visible layers, emits full ANSI output. Used for keyframe generation.
- **Layer paint** (`paint_layer`) — emits positioned ANSI for just one layer using save/restore cursor. Used for live overlay updates without redrawing the entire screen.
- **Layer clear** (`render_layer_clear`) — overwrites a layer's area with spaces. Used when hiding/removing a layer.

### SGR Optimization

The ANSI renderer (`render.rs`) tracks current text attributes and only emits SGR escape codes when attributes change, minimizing output bandwidth.

## Modules

- `compositor.rs` — `Compositor` struct: add/remove/compose layers
- `layer.rs` — `Layer`, `Anchor`, position resolution
- `cell.rs` — `Cell`, `Attrs`, `Color`
- `render.rs` — ANSI output: `render_grid`, `render_layer_paint`, `render_layer_clear`

## Dependencies

Only `vt100` — for reading the base screen's cell data. No async runtime.

## Testing

```sh
cargo test -p compositor
```
