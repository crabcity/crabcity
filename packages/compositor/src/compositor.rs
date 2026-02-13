use crate::cell::Cell;
use crate::layer::{Anchor, Layer, LayerId};
use crate::render;

pub struct Compositor {
    layers: Vec<Layer>,
    next_id: LayerId,
}

impl Compositor {
    pub fn new() -> Self {
        Self {
            layers: Vec::new(),
            next_id: 1,
        }
    }

    /// Add a new layer and return its ID.
    pub fn add_layer(&mut self, anchor: Anchor, width: u16, height: u16, z_order: i16) -> LayerId {
        let id = self.next_id;
        self.next_id += 1;
        let layer = Layer::new(id, anchor, width, height, z_order);
        self.layers.push(layer);
        // Keep sorted by z_order for composition
        self.layers.sort_by_key(|l| l.z_order);
        id
    }

    /// Get a mutable reference to a layer by ID.
    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    /// Get an immutable reference to a layer by ID.
    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    /// Remove a layer by ID. Returns true if found.
    pub fn remove_layer(&mut self, id: LayerId) -> bool {
        let len_before = self.layers.len();
        self.layers.retain(|l| l.id != id);
        self.layers.len() < len_before
    }

    /// Full composition: base screen + all visible layers → ANSI bytes.
    pub fn compose(&self, screen: &vt100::Screen) -> Vec<u8> {
        if !self.has_visible_layers() {
            // Fast path: no overlays, use vt100's built-in renderer
            let mut out = Vec::new();
            out.extend_from_slice(b"\x1b[H\x1b[2J\x1b[0m");
            out.extend_from_slice(&screen.contents_formatted());
            let (row, col) = screen.cursor_position();
            out.extend_from_slice(format!("\x1b[{};{}H", row + 1, col + 1).as_bytes());
            return out;
        }

        let (rows, cols) = screen.size();

        // Build output grid from base screen
        let mut grid: Vec<Cell> = Vec::with_capacity(rows as usize * cols as usize);
        for row in 0..rows {
            for col in 0..cols {
                grid.push(Cell::from(screen.cell(row, col).unwrap()));
            }
        }

        // Overlay visible layers (sorted by z_order ascending)
        for layer in &self.layers {
            if !layer.visible {
                continue;
            }
            let Some((top_row, left_col)) = layer.resolve_position(rows, cols) else {
                continue;
            };
            for lr in 0..layer.height {
                for lc in 0..layer.width {
                    if let Some(cell) = layer.get_cell(lr, lc) {
                        let gr = (top_row + lr) as usize;
                        let gc = (left_col + lc) as usize;
                        let idx = gr * cols as usize + gc;
                        if idx < grid.len() {
                            grid[idx] = cell.clone();
                        }
                    }
                }
            }
        }

        let cursor = screen.cursor_position();
        render::render_grid(&grid, rows, cols, cursor)
    }

    /// Render a positioned paint sequence for a single layer (for live updates).
    pub fn paint_layer(&self, id: LayerId, screen_rows: u16, screen_cols: u16) -> Option<Vec<u8>> {
        let layer = self.layer(id)?;
        Some(render::render_layer_paint(layer, screen_rows, screen_cols))
    }

    /// Render a clear sequence for a single layer.
    pub fn clear_layer(&self, id: LayerId, screen_rows: u16, screen_cols: u16) -> Option<Vec<u8>> {
        let layer = self.layer(id)?;
        Some(render::render_layer_clear(layer, screen_rows, screen_cols))
    }

    /// Check if any layers are visible (optimization: skip composition when false).
    pub fn has_visible_layers(&self) -> bool {
        self.layers.iter().any(|l| l.visible)
    }

    /// Resize hint — no-op for now (anchors resolve lazily), but future-proofs the API.
    pub fn resize(&mut self, _rows: u16, _cols: u16) {}
}

impl Default for Compositor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Attrs;

    fn make_screen(rows: u16, cols: u16, text: &[u8]) -> vt100::Parser {
        let mut parser = vt100::Parser::new(rows, cols, 0);
        parser.process(text);
        parser
    }

    #[test]
    fn new_compositor_no_layers() {
        let c = Compositor::new();
        assert!(!c.has_visible_layers());
    }

    #[test]
    fn add_and_remove_layer() {
        let mut c = Compositor::new();
        let id = c.add_layer(Anchor::TopLeft(0, 0), 10, 1, 0);
        assert!(c.has_visible_layers());
        assert!(c.layer(id).is_some());
        assert!(c.remove_layer(id));
        assert!(!c.has_visible_layers());
    }

    #[test]
    fn remove_nonexistent_layer() {
        let mut c = Compositor::new();
        assert!(!c.remove_layer(999));
    }

    #[test]
    fn layer_mut_access() {
        let mut c = Compositor::new();
        let id = c.add_layer(Anchor::TopLeft(0, 0), 10, 1, 0);
        let layer = c.layer_mut(id).unwrap();
        layer.fill_text(0, 0, "test", Attrs::default());
        assert_eq!(c.layer(id).unwrap().get_cell(0, 0).unwrap().contents, "t");
    }

    #[test]
    fn ids_are_unique() {
        let mut c = Compositor::new();
        let id1 = c.add_layer(Anchor::TopLeft(0, 0), 5, 1, 0);
        let id2 = c.add_layer(Anchor::TopLeft(0, 0), 5, 1, 0);
        assert_ne!(id1, id2);
    }

    #[test]
    fn compose_fast_path_no_layers() {
        let parser = make_screen(24, 80, b"Hello");
        let c = Compositor::new();
        let out = c.compose(parser.screen());
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("Hello"));
        // Fast path uses contents_formatted
        assert!(s.starts_with("\x1b[H\x1b[2J\x1b[0m"));
    }

    #[test]
    fn compose_fast_path_hidden_layer() {
        let parser = make_screen(24, 80, b"Hello");
        let mut c = Compositor::new();
        let id = c.add_layer(Anchor::TopLeft(0, 0), 5, 1, 0);
        c.layer_mut(id).unwrap().set_visible(false);
        assert!(!c.has_visible_layers());
        let out = c.compose(parser.screen());
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("Hello"));
    }

    #[test]
    fn compose_with_one_layer() {
        let parser = make_screen(24, 80, b"Hello world");
        let mut c = Compositor::new();
        let id = c.add_layer(Anchor::TopRight(0, 0), 5, 1, 0);
        let attrs = Attrs {
            inverse: true,
            ..Default::default()
        };
        c.layer_mut(id).unwrap().fill_text(0, 0, "BADGE", attrs);

        let out = c.compose(parser.screen());
        let s = String::from_utf8_lossy(&out);
        // Should contain both base content and overlay
        assert!(s.contains("Hello"));
        assert!(s.contains("BADGE"));
    }

    #[test]
    fn compose_z_order_higher_wins() {
        let parser = make_screen(24, 80, b"");
        let mut c = Compositor::new();

        // Layer at z=0 fills position (0,0) with 'A'
        let id1 = c.add_layer(Anchor::TopLeft(0, 0), 1, 1, 0);
        c.layer_mut(id1)
            .unwrap()
            .fill_text(0, 0, "A", Attrs::default());

        // Layer at z=1 fills same position with 'B'
        let id2 = c.add_layer(Anchor::TopLeft(0, 0), 1, 1, 1);
        c.layer_mut(id2)
            .unwrap()
            .fill_text(0, 0, "B", Attrs::default());

        let out = c.compose(parser.screen());
        let s = String::from_utf8_lossy(&out);
        // 'B' should win (higher z-order)
        assert!(s.contains("B"));
    }

    #[test]
    fn compose_invisible_layer_skipped() {
        let parser = make_screen(24, 80, b"base");
        let mut c = Compositor::new();
        let id = c.add_layer(Anchor::TopLeft(0, 0), 5, 1, 0);
        c.layer_mut(id)
            .unwrap()
            .fill_text(0, 0, "OVER", Attrs::default());
        c.layer_mut(id).unwrap().set_visible(false);

        let out = c.compose(parser.screen());
        let s = String::from_utf8_lossy(&out);
        // Should see base content but not the overlay text overwriting it
        assert!(s.contains("base"));
    }

    #[test]
    fn paint_layer_returns_ansi() {
        let mut c = Compositor::new();
        let id = c.add_layer(Anchor::TopRight(0, 0), 5, 1, 0);
        c.layer_mut(id)
            .unwrap()
            .fill_text(0, 0, "hi", Attrs::default());

        let paint = c.paint_layer(id, 24, 80).unwrap();
        let s = String::from_utf8_lossy(&paint);
        assert!(s.starts_with("\x1b7")); // save cursor
        assert!(s.ends_with("\x1b8")); // restore cursor
    }

    #[test]
    fn paint_nonexistent_layer() {
        let c = Compositor::new();
        assert!(c.paint_layer(999, 24, 80).is_none());
    }

    #[test]
    fn clear_layer_returns_blanks() {
        let mut c = Compositor::new();
        let id = c.add_layer(Anchor::TopRight(0, 0), 5, 1, 0);
        c.layer_mut(id)
            .unwrap()
            .fill_text(0, 0, "badge", Attrs::default());

        let clear = c.clear_layer(id, 24, 80).unwrap();
        let s = String::from_utf8_lossy(&clear);
        assert!(s.contains("     ")); // 5 spaces
    }

    #[test]
    fn clear_nonexistent_layer() {
        let c = Compositor::new();
        assert!(c.clear_layer(999, 24, 80).is_none());
    }

    #[test]
    fn has_visible_layers_tracks_visibility() {
        let mut c = Compositor::new();
        assert!(!c.has_visible_layers());

        let id = c.add_layer(Anchor::TopLeft(0, 0), 5, 1, 0);
        assert!(c.has_visible_layers());

        c.layer_mut(id).unwrap().set_visible(false);
        assert!(!c.has_visible_layers());

        c.layer_mut(id).unwrap().set_visible(true);
        assert!(c.has_visible_layers());
    }

    #[test]
    fn compose_after_resize() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(b"Hello");

        let mut c = Compositor::new();
        let id = c.add_layer(Anchor::TopRight(0, 0), 5, 1, 0);
        c.layer_mut(id)
            .unwrap()
            .fill_text(0, 0, "badge", Attrs::default());

        // Resize compositor (no-op for now, anchors resolve lazily)
        c.resize(40, 120);

        // Compose still works with old screen
        let out = c.compose(parser.screen());
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("Hello"));

        // Resize the parser too, then compose again
        parser.set_size(40, 120);
        parser.process(b"\x1b[H\x1b[2JNew content");
        let out = c.compose(parser.screen());
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("New content"));
        assert!(s.contains("badge"));
    }

    #[test]
    fn default_impl() {
        let c = Compositor::default();
        assert!(!c.has_visible_layers());
    }

    #[test]
    fn multiple_layers_different_positions() {
        let parser = make_screen(24, 80, b"");
        let mut c = Compositor::new();

        let id1 = c.add_layer(Anchor::TopLeft(0, 0), 3, 1, 0);
        c.layer_mut(id1)
            .unwrap()
            .fill_text(0, 0, "TL", Attrs::default());

        let id2 = c.add_layer(Anchor::BottomRight(0, 0), 3, 1, 0);
        c.layer_mut(id2)
            .unwrap()
            .fill_text(0, 0, "BR", Attrs::default());

        let out = c.compose(parser.screen());
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("T"));
        assert!(s.contains("B"));
    }
}
