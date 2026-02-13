use anyhow::Result;
use compositor::{Anchor, Attrs, Compositor, LayerId, render_layer_clear, render_layer_paint};
use nix::libc;
use std::io::Write;

/// RAII guard that saves terminal settings and restores them on drop.
/// Uses a local `Compositor` for client-side overlays (e.g. the "attached" badge).
#[cfg(unix)]
pub struct TerminalGuard {
    original: Option<nix::sys::termios::Termios>,
    compositor: Compositor,
    /// The active overlay layer ID, if any.
    overlay_id: Option<LayerId>,
    /// Current terminal width (for anchor resolution).
    cols: u16,
    /// Current terminal height.
    rows: u16,
    /// Cached paint sequence for the overlay layer.
    paint_cache: Vec<u8>,
}

#[cfg(unix)]
impl TerminalGuard {
    pub fn new() -> Self {
        use nix::sys::termios;
        let stdin = std::io::stdin();
        let original = termios::tcgetattr(&stdin).ok();
        Self {
            original,
            compositor: Compositor::new(),
            overlay_id: None,
            cols: 80,
            rows: 24,
            paint_cache: Vec::new(),
        }
    }

    pub fn enter_raw_mode(&self) {
        if let Some(ref original) = self.original {
            use nix::sys::termios;
            let stdin = std::io::stdin();
            let mut raw = original.clone();
            termios::cfmakeraw(&mut raw);
            let _ = termios::tcsetattr(&stdin, termios::SetArg::TCSANOW, &raw);
        }
    }

    /// Paint a reverse-video overlay badge in the upper-right corner.
    /// No-op if the terminal is too narrow. `text` must be ASCII.
    pub fn show_overlay(&mut self, text: &str, cols: u16) {
        self.cols = cols;
        // badge = " {text} " (1-char padding each side)
        let badge_width = (text.len() + 2) as u16;
        if cols < badge_width {
            return;
        }

        let id = self.compositor.add_layer(
            Anchor::TopRight(0, 0),
            badge_width,
            1,
            100, // high z-order for overlay
        );
        let attrs = Attrs {
            inverse: true,
            ..Default::default()
        };
        if let Some(layer) = self.compositor.layer_mut(id) {
            // Fill " {text} " into the layer
            let padded = format!(" {} ", text);
            layer.fill_text(0, 0, &padded, attrs);
        }

        // Paint and cache
        let paint = self
            .compositor
            .paint_layer(id, self.rows, self.cols)
            .unwrap_or_default();
        if !paint.is_empty() {
            write_escape_bytes(&paint);
        }
        self.paint_cache = paint;
        self.overlay_id = Some(id);
    }

    /// Update the overlay position for a new terminal width.
    /// Clears at the old position first to avoid ghost artifacts.
    /// No-op if no overlay is active.
    pub fn repaint_overlay(&mut self, cols: u16) {
        let Some(id) = self.overlay_id else {
            return;
        };

        // Clear at old dimensions
        if let Some(layer) = self.compositor.layer(id) {
            let clear = render_layer_clear(layer, self.rows, self.cols);
            if !clear.is_empty() {
                write_escape_bytes(&clear);
            }
        }

        // Update dimensions and repaint
        self.cols = cols;
        if let Some(layer) = self.compositor.layer(id) {
            let paint = render_layer_paint(layer, self.rows, cols);
            if !paint.is_empty() {
                write_escape_bytes(&paint);
            }
            self.paint_cache = paint;
        }
    }

    /// Returns the cached escape sequence bytes for repainting the overlay.
    /// Append these after PTY output to keep the badge visible.
    /// Returns an empty slice if no overlay is active.
    pub fn overlay_paint_bytes(&self) -> &[u8] {
        &self.paint_cache
    }

    /// Clear any active overlay badge.
    pub fn clear_overlay(&mut self) {
        if let Some(id) = self.overlay_id.take() {
            if let Some(layer) = self.compositor.layer(id) {
                let clear = render_layer_clear(layer, self.rows, self.cols);
                if !clear.is_empty() {
                    write_escape_bytes(&clear);
                }
            }
            self.compositor.remove_layer(id);
            self.paint_cache.clear();
        }
    }
}

#[cfg(unix)]
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.clear_overlay();
        if let Some(ref original) = self.original {
            use nix::sys::termios;
            let stdin = std::io::stdin();
            let _ = termios::tcsetattr(&stdin, termios::SetArg::TCSANOW, original);
        }
    }
}

fn write_escape_bytes(bytes: &[u8]) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(bytes);
    let _ = stdout.flush();
}

/// Get the current terminal size (rows, cols).
#[cfg(unix)]
pub fn get_terminal_size() -> Result<(u16, u16)> {
    let mut ws = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let ret = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
    if ret == -1 {
        anyhow::bail!("ioctl TIOCGWINSZ failed");
    }
    Ok((ws.ws_row, ws.ws_col))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a layer and render its paint sequence to check ANSI output.
    fn make_badge_paint(text: &str, cols: u16) -> Vec<u8> {
        let mut comp = Compositor::new();
        let badge_width = (text.len() + 2) as u16;
        let id = comp.add_layer(Anchor::TopRight(0, 0), badge_width, 1, 100);
        let attrs = Attrs {
            inverse: true,
            ..Default::default()
        };
        if let Some(layer) = comp.layer_mut(id) {
            layer.fill_text(0, 0, &format!(" {} ", text), attrs);
        }
        comp.paint_layer(id, 24, cols).unwrap_or_default()
    }

    fn make_badge_clear(text: &str, cols: u16) -> Vec<u8> {
        let mut comp = Compositor::new();
        let badge_width = (text.len() + 2) as u16;
        let id = comp.add_layer(Anchor::TopRight(0, 0), badge_width, 1, 100);
        let attrs = Attrs {
            inverse: true,
            ..Default::default()
        };
        if let Some(layer) = comp.layer_mut(id) {
            layer.fill_text(0, 0, &format!(" {} ", text), attrs);
        }
        comp.clear_layer(id, 24, cols).unwrap_or_default()
    }

    #[test]
    fn badge_paint_basic() {
        let paint = make_badge_paint("hello", 80);
        let s = String::from_utf8_lossy(&paint);
        // Should save/restore cursor
        assert!(s.starts_with("\x1b7"));
        assert!(s.ends_with("\x1b8"));
        // Should contain reverse video
        assert!(s.contains("\x1b[7m"));
        // Should contain text
        assert!(s.contains("h"));
        assert!(s.contains("o"));
    }

    #[test]
    fn badge_paint_too_narrow() {
        // "hello" badge width = 7, cols = 5 → doesn't fit
        let paint = make_badge_paint("hello", 5);
        assert!(paint.is_empty());
    }

    #[test]
    fn badge_clear_basic() {
        let clear = make_badge_clear("hello", 80);
        let s = String::from_utf8_lossy(&clear);
        assert!(s.starts_with("\x1b7"));
        assert!(s.ends_with("\x1b8"));
        // Should contain spaces for clearing
        assert!(s.contains("       ")); // 7 spaces
        // Should NOT contain reverse-video escape
        assert!(!s.contains("\x1b[7m"));
    }

    #[test]
    fn badge_clear_too_narrow() {
        let clear = make_badge_clear("hello", 5);
        assert!(clear.is_empty());
    }

    #[test]
    fn badge_paint_long_text() {
        let text = "attached -- Ctrl-] to detach"; // 28 chars, badge = 30
        let paint = make_badge_paint(text, 80);
        let s = String::from_utf8_lossy(&paint);
        assert!(!paint.is_empty());
        assert!(s.contains("\x1b[7m")); // reverse video
    }

    #[test]
    fn badge_paint_exact_fit() {
        // badge width = 7, cols = 7 → just fits
        let paint = make_badge_paint("hello", 7);
        assert!(!paint.is_empty());
    }

    #[test]
    fn badge_repaint_different_widths() {
        // Clear at old width, paint at new width
        let clear_80 = make_badge_clear("hello", 80);
        let paint_40 = make_badge_paint("hello", 40);
        // Both should be non-empty
        assert!(!clear_80.is_empty());
        assert!(!paint_40.is_empty());
        // They should target different positions
        let clear_s = String::from_utf8_lossy(&clear_80);
        let paint_s = String::from_utf8_lossy(&paint_40);
        // 80-col: TopRight(0,0) with 7-wide layer → col 73 (0-based) → \x1b[1;74H
        assert!(clear_s.contains("\x1b[1;74H"));
        // 40-col: TopRight(0,0) with 7-wide layer → col 33 (0-based) → \x1b[1;34H
        assert!(paint_s.contains("\x1b[1;34H"));
    }
}
