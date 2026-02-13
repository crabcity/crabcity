/// Terminal color, mirrors vt100::Color.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    Idx(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Attrs {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub contents: String,
    pub attrs: Attrs,
    pub wide_continuation: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            contents: " ".to_string(),
            attrs: Attrs::default(),
            wide_continuation: false,
        }
    }
}

impl From<&vt100::Cell> for Cell {
    fn from(cell: &vt100::Cell) -> Self {
        let contents = cell.contents();
        Self {
            contents: if contents.is_empty() {
                " ".to_string()
            } else {
                contents
            },
            attrs: Attrs {
                fg: convert_color(cell.fgcolor()),
                bg: convert_color(cell.bgcolor()),
                bold: cell.bold(),
                italic: cell.italic(),
                underline: cell.underline(),
                inverse: cell.inverse(),
            },
            wide_continuation: cell.is_wide_continuation(),
        }
    }
}

fn convert_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Default,
        vt100::Color::Idx(n) => Color::Idx(n),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_cell_is_blank() {
        let cell = Cell::default();
        assert_eq!(cell.contents, " ");
        assert_eq!(cell.attrs, Attrs::default());
        assert!(!cell.wide_continuation);
    }

    #[test]
    fn default_attrs_all_off() {
        let attrs = Attrs::default();
        assert_eq!(attrs.fg, Color::Default);
        assert_eq!(attrs.bg, Color::Default);
        assert!(!attrs.bold);
        assert!(!attrs.italic);
        assert!(!attrs.underline);
        assert!(!attrs.inverse);
    }

    #[test]
    fn default_color_is_default() {
        assert_eq!(Color::default(), Color::Default);
    }

    #[test]
    fn cell_equality() {
        let a = Cell::default();
        let b = Cell::default();
        assert_eq!(a, b);
    }

    #[test]
    fn cell_inequality() {
        let a = Cell::default();
        let b = Cell {
            contents: "x".to_string(),
            ..Cell::default()
        };
        assert_ne!(a, b);
    }

    #[test]
    fn from_vt100_cell_plain_text() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(b"A");
        let screen = parser.screen();
        let vt_cell = screen.cell(0, 0).unwrap();
        let cell = Cell::from(vt_cell);
        assert_eq!(cell.contents, "A");
        assert_eq!(cell.attrs.fg, Color::Default);
        assert_eq!(cell.attrs.bg, Color::Default);
        assert!(!cell.attrs.bold);
        assert!(!cell.wide_continuation);
    }

    #[test]
    fn from_vt100_cell_bold() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(b"\x1b[1mB");
        let screen = parser.screen();
        let cell = Cell::from(screen.cell(0, 0).unwrap());
        assert_eq!(cell.contents, "B");
        assert!(cell.attrs.bold);
    }

    #[test]
    fn from_vt100_cell_colored() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        // Set fg to red (index 1) and bg to blue (index 4)
        parser.process(b"\x1b[31m\x1b[44mC");
        let screen = parser.screen();
        let cell = Cell::from(screen.cell(0, 0).unwrap());
        assert_eq!(cell.contents, "C");
        assert_eq!(cell.attrs.fg, Color::Idx(1));
        assert_eq!(cell.attrs.bg, Color::Idx(4));
    }

    #[test]
    fn from_vt100_cell_rgb_color() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(b"\x1b[38;2;100;200;50mD");
        let screen = parser.screen();
        let cell = Cell::from(screen.cell(0, 0).unwrap());
        assert_eq!(cell.attrs.fg, Color::Rgb(100, 200, 50));
    }

    #[test]
    fn from_vt100_cell_empty_becomes_space() {
        let parser = vt100::Parser::new(24, 80, 0);
        let screen = parser.screen();
        // Unwritten cell
        let cell = Cell::from(screen.cell(5, 5).unwrap());
        assert_eq!(cell.contents, " ");
    }

    #[test]
    fn from_vt100_cell_wide_char() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        // CJK character (wide)
        parser.process("漢".as_bytes());
        let screen = parser.screen();
        let cell0 = Cell::from(screen.cell(0, 0).unwrap());
        let cell1 = Cell::from(screen.cell(0, 1).unwrap());
        assert_eq!(cell0.contents, "漢");
        assert!(!cell0.wide_continuation);
        assert!(cell1.wide_continuation);
    }

    #[test]
    fn convert_color_variants() {
        assert_eq!(convert_color(vt100::Color::Default), Color::Default);
        assert_eq!(convert_color(vt100::Color::Idx(42)), Color::Idx(42));
        assert_eq!(
            convert_color(vt100::Color::Rgb(10, 20, 30)),
            Color::Rgb(10, 20, 30)
        );
    }
}
