use crate::cell::{Attrs, Cell, Color};
use crate::layer::Layer;

/// Render a rows x cols cell grid to ANSI bytes.
/// Tracks current attrs, emits SGR only on change, emits CUP for positioning.
pub fn render_grid(cells: &[Cell], rows: u16, cols: u16, cursor: (u16, u16)) -> Vec<u8> {
    let mut out = Vec::with_capacity(cells.len() * 2);
    let mut current_attrs = Attrs::default();

    // Reset and home
    out.extend_from_slice(b"\x1b[H\x1b[2J\x1b[0m");

    for row in 0..rows {
        if row > 0 {
            // Move to beginning of next row
            out.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
        }
        for col in 0..cols {
            let idx = row as usize * cols as usize + col as usize;
            let cell = &cells[idx];

            if cell.wide_continuation {
                continue;
            }

            emit_sgr_diff(&mut out, &current_attrs, &cell.attrs);
            current_attrs = cell.attrs;

            out.extend_from_slice(cell.contents.as_bytes());
        }
    }

    // Reset attributes and position cursor
    out.extend_from_slice(b"\x1b[0m");
    out.extend_from_slice(format!("\x1b[{};{}H", cursor.0 + 1, cursor.1 + 1).as_bytes());

    out
}

/// Render a positioned layer as ANSI paint sequence.
/// Uses save/restore cursor for non-disruptive overlay.
pub fn render_layer_paint(layer: &Layer, screen_rows: u16, screen_cols: u16) -> Vec<u8> {
    if !layer.visible {
        return Vec::new();
    }
    let Some((top_row, left_col)) = layer.resolve_position(screen_rows, screen_cols) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    // Save cursor
    out.extend_from_slice(b"\x1b7");

    for row in 0..layer.height {
        let screen_row = top_row + row;
        let mut col = 0u16;
        while col < layer.width {
            if let Some(cell) = layer.get_cell(row, col) {
                if !cell.wide_continuation {
                    let screen_col = left_col + col;
                    // Position cursor and reset attrs before each cell
                    out.extend_from_slice(
                        format!("\x1b[{};{}H", screen_row + 1, screen_col + 1).as_bytes(),
                    );
                    out.extend_from_slice(b"\x1b[0m");
                    let default_attrs = Attrs::default();
                    emit_sgr_diff(&mut out, &default_attrs, &cell.attrs);
                    out.extend_from_slice(cell.contents.as_bytes());
                }
            }
            col += 1;
        }
    }

    // Reset attributes and restore cursor
    out.extend_from_slice(b"\x1b[0m\x1b8");
    out
}

/// Render blanks over a layer's area to erase it.
pub fn render_layer_clear(layer: &Layer, screen_rows: u16, screen_cols: u16) -> Vec<u8> {
    let Some((top_row, left_col)) = layer.resolve_position(screen_rows, screen_cols) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    // Save cursor
    out.extend_from_slice(b"\x1b7");
    // Reset attributes
    out.extend_from_slice(b"\x1b[0m");

    for row in 0..layer.height {
        let screen_row = top_row + row;
        out.extend_from_slice(format!("\x1b[{};{}H", screen_row + 1, left_col + 1).as_bytes());
        for _ in 0..layer.width {
            out.push(b' ');
        }
    }

    // Restore cursor
    out.extend_from_slice(b"\x1b8");
    out
}

/// Emit SGR escape codes to transition from `from` attrs to `to` attrs.
fn emit_sgr_diff(out: &mut Vec<u8>, from: &Attrs, to: &Attrs) {
    if from == to {
        return;
    }

    // If any attr was turned off, we need a full reset then re-apply
    let needs_reset = (from.bold && !to.bold)
        || (from.italic && !to.italic)
        || (from.underline && !to.underline)
        || (from.inverse && !to.inverse)
        || (from.fg != Color::Default && to.fg == Color::Default)
        || (from.bg != Color::Default && to.bg == Color::Default);

    if needs_reset {
        out.extend_from_slice(b"\x1b[0m");
        // Re-apply all active attrs
        if to.bold {
            out.extend_from_slice(b"\x1b[1m");
        }
        if to.italic {
            out.extend_from_slice(b"\x1b[3m");
        }
        if to.underline {
            out.extend_from_slice(b"\x1b[4m");
        }
        if to.inverse {
            out.extend_from_slice(b"\x1b[7m");
        }
        emit_fg(out, &to.fg);
        emit_bg(out, &to.bg);
    } else {
        // Only emit changes (additions)
        if !from.bold && to.bold {
            out.extend_from_slice(b"\x1b[1m");
        }
        if !from.italic && to.italic {
            out.extend_from_slice(b"\x1b[3m");
        }
        if !from.underline && to.underline {
            out.extend_from_slice(b"\x1b[4m");
        }
        if !from.inverse && to.inverse {
            out.extend_from_slice(b"\x1b[7m");
        }
        if from.fg != to.fg {
            emit_fg(out, &to.fg);
        }
        if from.bg != to.bg {
            emit_bg(out, &to.bg);
        }
    }
}

fn emit_fg(out: &mut Vec<u8>, color: &Color) {
    match color {
        Color::Default => out.extend_from_slice(b"\x1b[39m"),
        Color::Idx(n) => {
            out.extend_from_slice(format!("\x1b[38;5;{}m", n).as_bytes());
        }
        Color::Rgb(r, g, b) => {
            out.extend_from_slice(format!("\x1b[38;2;{};{};{}m", r, g, b).as_bytes());
        }
    }
}

fn emit_bg(out: &mut Vec<u8>, color: &Color) {
    match color {
        Color::Default => out.extend_from_slice(b"\x1b[49m"),
        Color::Idx(n) => {
            out.extend_from_slice(format!("\x1b[48;5;{}m", n).as_bytes());
        }
        Color::Rgb(r, g, b) => {
            out.extend_from_slice(format!("\x1b[48;2;{};{};{}m", r, g, b).as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layer::{Anchor, Layer};

    fn make_grid(rows: u16, cols: u16) -> Vec<Cell> {
        vec![Cell::default(); rows as usize * cols as usize]
    }

    #[test]
    fn render_grid_blank() {
        let grid = make_grid(2, 3);
        let out = render_grid(&grid, 2, 3, (0, 0));
        let s = String::from_utf8_lossy(&out);
        // Should start with reset/home/clear
        assert!(s.starts_with("\x1b[H\x1b[2J\x1b[0m"));
        // Should end with cursor positioning
        assert!(s.ends_with("\x1b[1;1H"));
    }

    #[test]
    fn render_grid_with_text() {
        let mut grid = make_grid(1, 5);
        grid[0] = Cell {
            contents: "H".to_string(),
            attrs: Attrs::default(),
            wide_continuation: false,
        };
        grid[1] = Cell {
            contents: "i".to_string(),
            attrs: Attrs::default(),
            wide_continuation: false,
        };
        let out = render_grid(&grid, 1, 5, (0, 2));
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("Hi"));
        assert!(s.ends_with("\x1b[1;3H")); // cursor at (0,2) → 1-based (1,3)
    }

    #[test]
    fn render_grid_sgr_for_bold() {
        let mut grid = make_grid(1, 2);
        grid[0] = Cell {
            contents: "B".to_string(),
            attrs: Attrs {
                bold: true,
                ..Default::default()
            },
            wide_continuation: false,
        };
        let out = render_grid(&grid, 1, 2, (0, 0));
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("\x1b[1m")); // bold
        assert!(s.contains("B"));
    }

    #[test]
    fn render_grid_sgr_for_color() {
        let mut grid = make_grid(1, 1);
        grid[0] = Cell {
            contents: "C".to_string(),
            attrs: Attrs {
                fg: Color::Idx(1),
                bg: Color::Rgb(10, 20, 30),
                ..Default::default()
            },
            wide_continuation: false,
        };
        let out = render_grid(&grid, 1, 1, (0, 0));
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("\x1b[38;5;1m"));
        assert!(s.contains("\x1b[48;2;10;20;30m"));
    }

    #[test]
    fn render_layer_paint_basic() {
        let mut layer = Layer::new(1, Anchor::TopRight(0, 0), 5, 1, 0);
        let attrs = Attrs {
            inverse: true,
            ..Default::default()
        };
        layer.fill_text(0, 0, "hello", attrs);

        let out = render_layer_paint(&layer, 24, 80);
        let s = String::from_utf8_lossy(&out);
        // Should save/restore cursor
        assert!(s.starts_with("\x1b7"));
        assert!(s.ends_with("\x1b8"));
        // Should contain reverse video
        assert!(s.contains("\x1b[7m"));
        // Should contain text
        assert!(s.contains("h"));
        assert!(s.contains("o"));
        // Position: top-right, 5 wide on 80-col screen → col 76 (1-based)
        assert!(s.contains("\x1b[1;76H"));
    }

    #[test]
    fn render_layer_paint_invisible() {
        let mut layer = Layer::new(1, Anchor::TopLeft(0, 0), 5, 1, 0);
        layer.fill_text(0, 0, "hello", Attrs::default());
        layer.set_visible(false);
        let out = render_layer_paint(&layer, 24, 80);
        assert!(out.is_empty());
    }

    #[test]
    fn render_layer_paint_doesnt_fit() {
        let mut layer = Layer::new(1, Anchor::TopLeft(0, 0), 100, 1, 0);
        layer.fill_text(0, 0, "hi", Attrs::default());
        let out = render_layer_paint(&layer, 24, 80);
        assert!(out.is_empty());
    }

    #[test]
    fn render_layer_clear_basic() {
        let layer = Layer::new(1, Anchor::TopRight(0, 0), 5, 1, 0);
        let out = render_layer_clear(&layer, 24, 80);
        let s = String::from_utf8_lossy(&out);
        assert!(s.starts_with("\x1b7"));
        assert!(s.ends_with("\x1b8"));
        // Should write 5 spaces at the right position
        assert!(s.contains("\x1b[1;76H"));
        assert!(s.contains("     ")); // 5 spaces
    }

    #[test]
    fn render_layer_clear_multi_row() {
        let layer = Layer::new(1, Anchor::TopLeft(0, 0), 3, 2, 0);
        let out = render_layer_clear(&layer, 24, 80);
        let s = String::from_utf8_lossy(&out);
        // Two rows of 3 spaces
        assert!(s.contains("\x1b[1;1H   "));
        assert!(s.contains("\x1b[2;1H   "));
    }

    #[test]
    fn sgr_diff_same_attrs_no_output() {
        let mut out = Vec::new();
        let attrs = Attrs::default();
        emit_sgr_diff(&mut out, &attrs, &attrs);
        assert!(out.is_empty());
    }

    #[test]
    fn sgr_diff_add_bold() {
        let mut out = Vec::new();
        let from = Attrs::default();
        let to = Attrs {
            bold: true,
            ..Default::default()
        };
        emit_sgr_diff(&mut out, &from, &to);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("\x1b[1m"));
        // Should not contain a reset since we're only adding
        assert!(!s.contains("\x1b[0m"));
    }

    #[test]
    fn sgr_diff_remove_bold_needs_reset() {
        let mut out = Vec::new();
        let from = Attrs {
            bold: true,
            ..Default::default()
        };
        let to = Attrs::default();
        emit_sgr_diff(&mut out, &from, &to);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("\x1b[0m"));
    }

    #[test]
    fn sgr_diff_change_fg_color() {
        let mut out = Vec::new();
        let from = Attrs {
            fg: Color::Idx(1),
            ..Default::default()
        };
        let to = Attrs {
            fg: Color::Idx(2),
            ..Default::default()
        };
        emit_sgr_diff(&mut out, &from, &to);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("\x1b[38;5;2m"));
    }

    #[test]
    fn sgr_diff_inverse_on() {
        let mut out = Vec::new();
        let from = Attrs::default();
        let to = Attrs {
            inverse: true,
            ..Default::default()
        };
        emit_sgr_diff(&mut out, &from, &to);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("\x1b[7m"));
    }
}
