use crate::cell::Cell;

pub type LayerId = u64;

/// Where a layer anchors relative to screen edges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    TopLeft(u16, u16),
    TopRight(u16, u16),
    BottomLeft(u16, u16),
    BottomRight(u16, u16),
}

pub struct Layer {
    pub id: LayerId,
    pub anchor: Anchor,
    pub z_order: i16,
    pub width: u16,
    pub height: u16,
    cells: Vec<Option<Cell>>,
    pub visible: bool,
}

impl Layer {
    pub(crate) fn new(id: LayerId, anchor: Anchor, width: u16, height: u16, z_order: i16) -> Self {
        let size = width as usize * height as usize;
        Self {
            id,
            anchor,
            z_order,
            width,
            height,
            cells: vec![None; size],
            visible: true,
        }
    }

    /// Set a single cell at (row, col) within the layer.
    pub fn set_cell(&mut self, row: u16, col: u16, cell: Cell) {
        if row < self.height && col < self.width {
            let idx = row as usize * self.width as usize + col as usize;
            self.cells[idx] = Some(cell);
        }
    }

    /// Get a cell at (row, col) within the layer.
    pub fn get_cell(&self, row: u16, col: u16) -> Option<&Cell> {
        if row < self.height && col < self.width {
            let idx = row as usize * self.width as usize + col as usize;
            self.cells[idx].as_ref()
        } else {
            None
        }
    }

    /// Fill a row starting at `col` with text using the given attrs.
    pub fn fill_text(&mut self, row: u16, col: u16, text: &str, attrs: crate::cell::Attrs) {
        let mut c = col;
        for ch in text.chars() {
            if c >= self.width {
                break;
            }
            self.set_cell(
                row,
                c,
                Cell {
                    contents: ch.to_string(),
                    attrs,
                    wide_continuation: false,
                },
            );
            c += 1;
        }
    }

    /// Clear all cells in the layer (make transparent).
    pub fn clear(&mut self) {
        for cell in &mut self.cells {
            *cell = None;
        }
    }

    /// Set layer visibility.
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    /// Resolve anchor to absolute (row, col) top-left position.
    /// Returns None if the layer doesn't fit at the resolved position.
    pub fn resolve_position(&self, screen_rows: u16, screen_cols: u16) -> Option<(u16, u16)> {
        resolve_position(
            &self.anchor,
            self.width,
            self.height,
            screen_rows,
            screen_cols,
        )
    }
}

/// Compute absolute top-left (row, col) from an anchor.
/// Returns None if the layer doesn't fit.
pub fn resolve_position(
    anchor: &Anchor,
    layer_w: u16,
    layer_h: u16,
    screen_rows: u16,
    screen_cols: u16,
) -> Option<(u16, u16)> {
    if layer_w > screen_cols || layer_h > screen_rows {
        return None;
    }
    match anchor {
        Anchor::TopLeft(row_off, col_off) => {
            let r = *row_off;
            let c = *col_off;
            if r + layer_h > screen_rows || c + layer_w > screen_cols {
                None
            } else {
                Some((r, c))
            }
        }
        Anchor::TopRight(row_off, col_off) => {
            let r = *row_off;
            let c = screen_cols.checked_sub(layer_w + *col_off)?;
            if r + layer_h > screen_rows {
                None
            } else {
                Some((r, c))
            }
        }
        Anchor::BottomLeft(row_off, col_off) => {
            let r = screen_rows.checked_sub(layer_h + *row_off)?;
            let c = *col_off;
            if c + layer_w > screen_cols {
                None
            } else {
                Some((r, c))
            }
        }
        Anchor::BottomRight(row_off, col_off) => {
            let r = screen_rows.checked_sub(layer_h + *row_off)?;
            let c = screen_cols.checked_sub(layer_w + *col_off)?;
            Some((r, c))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Attrs;

    #[test]
    fn new_layer_all_transparent() {
        let layer = Layer::new(1, Anchor::TopLeft(0, 0), 10, 5, 0);
        assert_eq!(layer.width, 10);
        assert_eq!(layer.height, 5);
        assert!(layer.visible);
        for r in 0..5 {
            for c in 0..10 {
                assert!(layer.get_cell(r, c).is_none());
            }
        }
    }

    #[test]
    fn set_and_get_cell() {
        let mut layer = Layer::new(1, Anchor::TopLeft(0, 0), 10, 5, 0);
        let cell = Cell {
            contents: "X".to_string(),
            attrs: Attrs::default(),
            wide_continuation: false,
        };
        layer.set_cell(2, 3, cell.clone());
        assert_eq!(layer.get_cell(2, 3), Some(&cell));
        assert!(layer.get_cell(0, 0).is_none());
    }

    #[test]
    fn set_cell_out_of_bounds_noop() {
        let mut layer = Layer::new(1, Anchor::TopLeft(0, 0), 5, 3, 0);
        layer.set_cell(10, 10, Cell::default());
        // Should not panic, just no-op
        assert!(layer.get_cell(10, 10).is_none());
    }

    #[test]
    fn fill_text_basic() {
        let mut layer = Layer::new(1, Anchor::TopLeft(0, 0), 10, 1, 0);
        let attrs = Attrs {
            inverse: true,
            ..Default::default()
        };
        layer.fill_text(0, 0, "hello", attrs);
        assert_eq!(layer.get_cell(0, 0).unwrap().contents, "h");
        assert_eq!(layer.get_cell(0, 4).unwrap().contents, "o");
        assert!(layer.get_cell(0, 5).is_none()); // past the text
    }

    #[test]
    fn fill_text_truncates_at_width() {
        let mut layer = Layer::new(1, Anchor::TopLeft(0, 0), 3, 1, 0);
        layer.fill_text(0, 0, "abcde", Attrs::default());
        assert_eq!(layer.get_cell(0, 0).unwrap().contents, "a");
        assert_eq!(layer.get_cell(0, 2).unwrap().contents, "c");
        // "d" and "e" should be truncated
    }

    #[test]
    fn fill_text_with_offset() {
        let mut layer = Layer::new(1, Anchor::TopLeft(0, 0), 10, 1, 0);
        layer.fill_text(0, 3, "hi", Attrs::default());
        assert!(layer.get_cell(0, 2).is_none());
        assert_eq!(layer.get_cell(0, 3).unwrap().contents, "h");
        assert_eq!(layer.get_cell(0, 4).unwrap().contents, "i");
    }

    #[test]
    fn clear_removes_all_cells() {
        let mut layer = Layer::new(1, Anchor::TopLeft(0, 0), 5, 3, 0);
        layer.fill_text(0, 0, "abc", Attrs::default());
        layer.fill_text(1, 0, "def", Attrs::default());
        layer.clear();
        for r in 0..3 {
            for c in 0..5 {
                assert!(layer.get_cell(r, c).is_none());
            }
        }
    }

    #[test]
    fn visibility_toggle() {
        let mut layer = Layer::new(1, Anchor::TopLeft(0, 0), 5, 3, 0);
        assert!(layer.visible);
        layer.set_visible(false);
        assert!(!layer.visible);
        layer.set_visible(true);
        assert!(layer.visible);
    }

    // Anchor resolution tests

    #[test]
    fn resolve_top_left_origin() {
        assert_eq!(
            resolve_position(&Anchor::TopLeft(0, 0), 10, 5, 24, 80),
            Some((0, 0))
        );
    }

    #[test]
    fn resolve_top_left_offset() {
        assert_eq!(
            resolve_position(&Anchor::TopLeft(2, 5), 10, 3, 24, 80),
            Some((2, 5))
        );
    }

    #[test]
    fn resolve_top_left_doesnt_fit() {
        // Layer 10 wide at col offset 75 in 80-col screen → 75+10=85 > 80
        assert_eq!(
            resolve_position(&Anchor::TopLeft(0, 75), 10, 1, 24, 80),
            None
        );
    }

    #[test]
    fn resolve_top_right_no_offset() {
        // 10-wide layer, screen 80 cols → starts at col 70
        assert_eq!(
            resolve_position(&Anchor::TopRight(0, 0), 10, 1, 24, 80),
            Some((0, 70))
        );
    }

    #[test]
    fn resolve_top_right_with_offset() {
        // 10-wide layer, 2 col offset → starts at col 68
        assert_eq!(
            resolve_position(&Anchor::TopRight(1, 2), 10, 1, 24, 80),
            Some((1, 68))
        );
    }

    #[test]
    fn resolve_bottom_left() {
        // 3-tall layer at bottom of 24-row screen → starts at row 21
        assert_eq!(
            resolve_position(&Anchor::BottomLeft(0, 0), 10, 3, 24, 80),
            Some((21, 0))
        );
    }

    #[test]
    fn resolve_bottom_right() {
        // 3-tall, 10-wide at bottom-right of 24x80
        assert_eq!(
            resolve_position(&Anchor::BottomRight(0, 0), 10, 3, 24, 80),
            Some((21, 70))
        );
    }

    #[test]
    fn resolve_bottom_right_with_offset() {
        assert_eq!(
            resolve_position(&Anchor::BottomRight(1, 2), 10, 3, 24, 80),
            Some((20, 68))
        );
    }

    #[test]
    fn resolve_layer_too_wide() {
        assert_eq!(
            resolve_position(&Anchor::TopLeft(0, 0), 100, 1, 24, 80),
            None
        );
    }

    #[test]
    fn resolve_layer_too_tall() {
        assert_eq!(
            resolve_position(&Anchor::TopLeft(0, 0), 10, 30, 24, 80),
            None
        );
    }

    #[test]
    fn resolve_exact_fit() {
        // Layer exactly fills the screen
        assert_eq!(
            resolve_position(&Anchor::TopLeft(0, 0), 80, 24, 24, 80),
            Some((0, 0))
        );
        assert_eq!(
            resolve_position(&Anchor::BottomRight(0, 0), 80, 24, 24, 80),
            Some((0, 0))
        );
    }

    #[test]
    fn layer_resolve_position_delegates() {
        let layer = Layer::new(1, Anchor::TopRight(0, 0), 10, 1, 0);
        assert_eq!(layer.resolve_position(24, 80), Some((0, 70)));
    }
}
