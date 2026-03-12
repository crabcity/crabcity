//! Virtual Terminal
//!
//! Sits between clients and a PTY. Maintains a screen buffer via `vt100`,
//! generates keyframe snapshots, stores deltas (raw PTY output since last
//! keyframe), and negotiates dimensions across multiple clients.

use std::collections::HashMap;

/// Type of client connection.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientType {
    Web,
    Terminal,
}

/// Per-client viewport tracking.
#[derive(Debug, Clone)]
pub struct ClientViewport {
    pub rows: u16,
    pub cols: u16,
    pub client_type: ClientType,
    pub is_active: bool,
}

/// A virtual terminal that sits between clients and a PTY.
/// Maintains a screen buffer, negotiates dimensions, and provides
/// keyframe + delta replay for efficient client attach.
pub struct VirtualTerminal {
    /// VT100 terminal emulator — processes PTY output, maintains screen state
    parser: vt100::Parser,

    /// Per-client viewport tracking
    client_viewports: HashMap<String, ClientViewport>,

    /// Current effective dimensions (min of all active viewports)
    effective_dims: (u16, u16),

    /// Last keyframe: screen state rendered as ANSI escape sequences
    keyframe: Option<Vec<u8>>,

    /// Raw PTY output accumulated since last keyframe
    deltas: Vec<u8>,

    /// Max delta buffer size before auto-compaction
    max_delta_bytes: usize,
}

impl VirtualTerminal {
    /// Create a new virtual terminal with the given initial dimensions.
    pub fn new(rows: u16, cols: u16, max_delta_bytes: usize, scrollback_lines: usize) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, scrollback_lines),
            client_viewports: HashMap::new(),
            effective_dims: (rows, cols),
            keyframe: None,
            deltas: Vec::new(),
            max_delta_bytes,
        }
    }

    /// Access the underlying vt100 screen for cell-level reads.
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Current cursor position (row, col) — 0-indexed.
    pub fn cursor_position(&self) -> (u16, u16) {
        self.parser.screen().cursor_position()
    }

    /// Process PTY output — feed to vt100 parser, append to deltas,
    /// auto-compact if deltas exceed threshold.
    pub fn process_output(&mut self, data: &[u8]) {
        self.parser.process(data);
        self.deltas.extend_from_slice(data);

        // Auto-compact when deltas get large
        if self.deltas.len() > self.max_delta_bytes {
            self.compact();
        }
    }

    /// Generate a keyframe: snapshot the screen as ANSI escape sequences.
    /// Clears the delta buffer.
    pub fn compact(&mut self) {
        let screen = self.parser.screen();
        let mut kf = Vec::new();
        // Reset terminal state, move home, clear screen, reset attributes
        kf.extend_from_slice(b"\x1b[H\x1b[2J\x1b[0m");
        kf.extend_from_slice(&screen.contents_formatted());
        // Restore cursor position
        let (row, col) = screen.cursor_position();
        kf.extend_from_slice(format!("\x1b[{};{}H", row + 1, col + 1).as_bytes());
        self.keyframe = Some(kf);
        self.deltas.clear();
    }

    /// Get the aggregated terminal state for a new client.
    ///
    /// Returns scrollback content (from the vt100 parser's buffer) followed
    /// by a keyframe (visible screen snapshot). Always compacts first so
    /// clients receive the fully-aggregated state — no raw deltas of cursor
    /// throbs or intermediate rewrites.
    pub fn replay(&mut self, client_rows: u16) -> Vec<u8> {
        self.compact();
        let mut result = Vec::new();

        let has_scrollback = self.append_scrollback_replay(&mut result);

        if has_scrollback {
            // The scrollback lines scrolled through the receiving terminal.
            // The last `client_rows` of them are still on the visible screen.
            // Scroll them into the client's scrollback buffer before the
            // keyframe's \x1b[2J erases the display.
            //
            // We use the CLIENT's row count here — the server's effective dims
            // may differ (e.g. the client's Resize hasn't been processed yet,
            // or min-of-viewports shrank the PTY).  Using the wrong count
            // either loses scrollback lines or injects blank rows.
            result.extend_from_slice(format!("\x1b[{};1H", client_rows).as_bytes());
            for _ in 0..client_rows {
                result.push(b'\n');
            }
        }

        // Clear screen and render the live visible content.
        let screen = self.parser.screen();
        result.extend_from_slice(b"\x1b[H\x1b[2J\x1b[0m");
        result.extend_from_slice(&screen.contents_formatted());
        let (row, col) = screen.cursor_position();
        result.extend_from_slice(format!("\x1b[{};{}H", row + 1, col + 1).as_bytes());

        result
    }

    /// Append formatted scrollback rows to `out`, from oldest to newest.
    /// Each row is output as SGR-styled text + `\r\n`, so it scrolls
    /// naturally through the receiving terminal. **No CUP sequences** —
    /// `rows_formatted()` emits absolute cursor-position sequences for rows
    /// with leading default cells, which stomp earlier pages when scrollback
    /// is emitted page-by-page.
    /// Returns `true` if any scrollback lines were emitted.
    fn append_scrollback_replay(&mut self, out: &mut Vec<u8>) -> bool {
        // Find total scrollback depth by scrolling all the way back
        self.parser.screen_mut().set_scrollback(usize::MAX);
        let total_scrollback = self.parser.screen().scrollback();

        if total_scrollback == 0 {
            self.parser.screen_mut().set_scrollback(0);
            return false;
        }

        let (rows, cols) = self.parser.screen().size();
        let rows_usize = rows as usize;

        // Iterate from oldest scrollback to newest, one page at a time.
        // At scrollback=N, viewport row 0 is N lines back from the live screen.
        // The top min(N, screen_height) rows are scrollback content.
        let mut remaining = total_scrollback;
        while remaining > 0 {
            self.parser.screen_mut().set_scrollback(remaining);
            let scrollback_rows_in_page = remaining.min(rows_usize);

            for i in 0..scrollback_rows_in_page {
                format_row_no_cup(self.parser.screen(), i as u16, cols, out);
                out.extend_from_slice(b"\x1b[0m\r\n");
            }

            remaining = remaining.saturating_sub(rows_usize);
        }

        self.parser.screen_mut().set_scrollback(0);
        true
    }

    /// Update a client's viewport. Returns new effective dims if changed.
    pub fn update_viewport(
        &mut self,
        connection_id: &str,
        rows: u16,
        cols: u16,
        client_type: ClientType,
    ) -> Option<(u16, u16)> {
        self.client_viewports.insert(
            connection_id.to_string(),
            ClientViewport {
                rows,
                cols,
                client_type,
                is_active: true,
            },
        );
        self.recalculate_effective_dims()
    }

    /// Set a client's terminal visibility. Returns new effective dims if changed.
    pub fn set_active(&mut self, connection_id: &str, active: bool) -> Option<(u16, u16)> {
        if let Some(viewport) = self.client_viewports.get_mut(connection_id) {
            viewport.is_active = active;
        }
        self.recalculate_effective_dims()
    }

    /// Remove a client (disconnect). Returns new effective dims if changed.
    pub fn remove_client(&mut self, connection_id: &str) -> Option<(u16, u16)> {
        self.client_viewports.remove(connection_id);
        self.recalculate_effective_dims()
    }

    /// Apply a resize to the vt100 parser (called when effective dims change).
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
    }

    /// Current effective dimensions.
    pub fn effective_dims(&self) -> (u16, u16) {
        self.effective_dims
    }

    /// Recalculate effective dims from active viewports.
    /// Returns `Some((rows, cols))` if dims changed, `None` otherwise.
    fn recalculate_effective_dims(&mut self) -> Option<(u16, u16)> {
        let new_dims = calculate_effective_dims(&self.client_viewports);
        if let Some((rows, cols)) = new_dims
            && (rows, cols) != self.effective_dims
        {
            self.effective_dims = (rows, cols);
            // Resize in place — preserves scrollback history.  The visible
            // screen may briefly show merged soft-wrapped lines, but the
            // PTY's SIGWINCH redraw fixes it within milliseconds.
            self.parser.screen_mut().set_size(rows, cols);
            self.keyframe = None;
            self.deltas.clear();
            return Some((rows, cols));
        }
        // If no active viewports, keep current dims (don't shrink to nothing)
        None
    }
}

/// Emit a single row as SGR-styled text — no CUP or cursor movement.
/// Iterates cells left-to-right, emitting SGR diffs for attribute changes
/// and cell contents (space for empty/default cells). Wide-continuation
/// cells are skipped.
fn format_row_no_cup(screen: &vt100::Screen, row: u16, cols: u16, out: &mut Vec<u8>) {
    let mut cur_fg = vt100::Color::Default;
    let mut cur_bg = vt100::Color::Default;
    let mut cur_bold = false;
    let mut cur_italic = false;
    let mut cur_underline = false;
    let mut cur_inverse = false;

    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else {
            out.push(b' ');
            continue;
        };
        if cell.is_wide_continuation() {
            continue;
        }

        let fg = cell.fgcolor();
        let bg = cell.bgcolor();
        let bold = cell.bold();
        let italic = cell.italic();
        let underline = cell.underline();
        let inverse = cell.inverse();

        let attrs_differ = fg != cur_fg
            || bg != cur_bg
            || bold != cur_bold
            || italic != cur_italic
            || underline != cur_underline
            || inverse != cur_inverse;

        if attrs_differ {
            // Check if any attribute was *removed* — needs a full reset
            let needs_reset = (cur_bold && !bold)
                || (cur_italic && !italic)
                || (cur_underline && !underline)
                || (cur_inverse && !inverse)
                || (cur_fg != vt100::Color::Default && fg == vt100::Color::Default)
                || (cur_bg != vt100::Color::Default && bg == vt100::Color::Default);

            if needs_reset {
                out.extend_from_slice(b"\x1b[0m");
                if bold {
                    out.extend_from_slice(b"\x1b[1m");
                }
                if italic {
                    out.extend_from_slice(b"\x1b[3m");
                }
                if underline {
                    out.extend_from_slice(b"\x1b[4m");
                }
                if inverse {
                    out.extend_from_slice(b"\x1b[7m");
                }
                emit_sgr_fg(out, fg);
                emit_sgr_bg(out, bg);
            } else {
                if !cur_bold && bold {
                    out.extend_from_slice(b"\x1b[1m");
                }
                if !cur_italic && italic {
                    out.extend_from_slice(b"\x1b[3m");
                }
                if !cur_underline && underline {
                    out.extend_from_slice(b"\x1b[4m");
                }
                if !cur_inverse && inverse {
                    out.extend_from_slice(b"\x1b[7m");
                }
                if cur_fg != fg {
                    emit_sgr_fg(out, fg);
                }
                if cur_bg != bg {
                    emit_sgr_bg(out, bg);
                }
            }

            cur_fg = fg;
            cur_bg = bg;
            cur_bold = bold;
            cur_italic = italic;
            cur_underline = underline;
            cur_inverse = inverse;
        }

        let contents = cell.contents();
        if contents.is_empty() {
            out.push(b' ');
        } else {
            out.extend_from_slice(contents.as_bytes());
        }
    }
}

fn emit_sgr_fg(out: &mut Vec<u8>, color: vt100::Color) {
    match color {
        vt100::Color::Default => out.extend_from_slice(b"\x1b[39m"),
        vt100::Color::Idx(n) => {
            out.extend_from_slice(format!("\x1b[38;5;{}m", n).as_bytes());
        }
        vt100::Color::Rgb(r, g, b) => {
            out.extend_from_slice(format!("\x1b[38;2;{};{};{}m", r, g, b).as_bytes());
        }
    }
}

fn emit_sgr_bg(out: &mut Vec<u8>, color: vt100::Color) {
    match color {
        vt100::Color::Default => out.extend_from_slice(b"\x1b[49m"),
        vt100::Color::Idx(n) => {
            out.extend_from_slice(format!("\x1b[48;5;{}m", n).as_bytes());
        }
        vt100::Color::Rgb(r, g, b) => {
            out.extend_from_slice(format!("\x1b[48;2;{};{};{}m", r, g, b).as_bytes());
        }
    }
}

/// Calculate effective dimensions from active viewports.
/// Returns min(rows) x min(cols) across all active clients.
fn calculate_effective_dims(viewports: &HashMap<String, ClientViewport>) -> Option<(u16, u16)> {
    let active: Vec<_> = viewports.values().filter(|v| v.is_active).collect();
    if active.is_empty() {
        return None;
    }
    let rows = active.iter().map(|v| v.rows).min().unwrap();
    let cols = active.iter().map(|v| v.cols).min().unwrap();
    Some((rows, cols))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_initial_dims() {
        let vt = VirtualTerminal::new(24, 80, 4096, 0);
        assert_eq!(vt.effective_dims(), (24, 80));
    }

    #[test]
    fn test_screen_access() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        vt.process_output(b"Hello");
        let screen = vt.screen();
        let (rows, cols) = screen.size();
        assert_eq!(rows, 24);
        assert_eq!(cols, 80);
        // Screen should contain "Hello"
        assert!(screen.contents().contains("Hello"));
    }

    #[test]
    fn test_single_client_viewport() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        let result = vt.update_viewport("client-1", 40, 120, ClientType::Web);
        // Effective dims changed from (24, 80) to (40, 120)
        assert_eq!(result, Some((40, 120)));
        assert_eq!(vt.effective_dims(), (40, 120));
    }

    #[test]
    fn test_two_clients_min_dims() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        vt.update_viewport("web", 40, 120, ClientType::Web);
        let result = vt.update_viewport("cli", 24, 80, ClientType::Terminal);
        // min(40,24)=24, min(120,80)=80
        assert_eq!(result, Some((24, 80)));
        assert_eq!(vt.effective_dims(), (24, 80));
    }

    #[test]
    fn test_inactive_client_excluded() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        vt.update_viewport("web", 40, 120, ClientType::Web);
        vt.update_viewport("cli", 24, 80, ClientType::Terminal);
        assert_eq!(vt.effective_dims(), (24, 80));

        // Deactivate the smaller client
        let result = vt.set_active("cli", false);
        assert_eq!(result, Some((40, 120)));
        assert_eq!(vt.effective_dims(), (40, 120));
    }

    #[test]
    fn test_reactivate_client() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        vt.update_viewport("web", 40, 120, ClientType::Web);
        vt.update_viewport("cli", 24, 80, ClientType::Terminal);
        vt.set_active("cli", false);
        assert_eq!(vt.effective_dims(), (40, 120));

        // Reactivate
        let result = vt.set_active("cli", true);
        assert_eq!(result, Some((24, 80)));
    }

    #[test]
    fn test_client_removal_recalculates() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        vt.update_viewport("web", 40, 120, ClientType::Web);
        vt.update_viewport("cli", 24, 80, ClientType::Terminal);
        assert_eq!(vt.effective_dims(), (24, 80));

        let result = vt.remove_client("cli");
        assert_eq!(result, Some((40, 120)));
    }

    #[test]
    fn test_remove_last_client_keeps_dims() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        vt.update_viewport("cli", 30, 100, ClientType::Terminal);
        assert_eq!(vt.effective_dims(), (30, 100));

        // Remove the only client — dims stay at (30, 100)
        let result = vt.remove_client("cli");
        assert!(result.is_none());
        assert_eq!(vt.effective_dims(), (30, 100));
    }

    #[test]
    fn test_process_output_and_compact_replay() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        vt.process_output(b"Hello, world!");
        vt.process_output(b"\r\nLine 2");

        let replay = vt.replay(24);
        // Replay should contain the keyframe + any deltas
        assert!(!replay.is_empty());

        // The replay should contain "Hello, world!" and "Line 2" when
        // rendered through a terminal
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(replay_str.contains("Hello, world!"));
        assert!(replay_str.contains("Line 2"));
    }

    #[test]
    fn test_auto_compaction() {
        let mut vt = VirtualTerminal::new(24, 80, 100, 0); // Small threshold
        // Write more than 100 bytes
        let data = vec![b'A'; 150];
        vt.process_output(&data);

        // After auto-compaction, deltas should be empty
        assert!(vt.deltas.is_empty());
        // And keyframe should exist
        assert!(vt.keyframe.is_some());
    }

    #[test]
    fn test_resize_invalidates_keyframe() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        vt.process_output(b"Some output");
        vt.compact();
        assert!(vt.keyframe.is_some());

        // Resize via a new client viewport
        vt.update_viewport("cli", 30, 100, ClientType::Terminal);
        // Keyframe should be invalidated
        assert!(vt.keyframe.is_none());
    }

    #[test]
    fn test_no_dims_change_returns_none() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        vt.update_viewport("cli", 30, 100, ClientType::Terminal);
        assert_eq!(vt.effective_dims(), (30, 100));

        // Same dims again — no change
        let result = vt.update_viewport("cli", 30, 100, ClientType::Terminal);
        assert!(result.is_none());
    }

    #[test]
    fn test_replay_compacts_if_no_keyframe() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        vt.process_output(b"Hello");
        assert!(vt.keyframe.is_none());

        let replay = vt.replay(24);
        // Should have compacted and produced a keyframe
        assert!(vt.keyframe.is_some());
        assert!(!replay.is_empty());
    }

    #[test]
    fn test_calculate_effective_dims_empty() {
        let viewports = HashMap::new();
        assert_eq!(calculate_effective_dims(&viewports), None);
    }

    #[test]
    fn test_calculate_effective_dims_all_inactive() {
        let mut viewports = HashMap::new();
        viewports.insert(
            "a".to_string(),
            ClientViewport {
                rows: 24,
                cols: 80,
                client_type: ClientType::Web,
                is_active: false,
            },
        );
        assert_eq!(calculate_effective_dims(&viewports), None);
    }

    #[test]
    fn test_update_viewport_implies_active() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        // update_viewport always sets is_active=true
        vt.update_viewport("cli", 30, 100, ClientType::Terminal);
        let vp = vt.client_viewports.get("cli").unwrap();
        assert!(vp.is_active);
    }

    #[test]
    fn test_keyframe_plus_deltas_replay() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        // Phase 1: write output, then compact to create a keyframe
        vt.process_output(b"Before keyframe");
        vt.compact();
        assert!(vt.keyframe.is_some());
        assert!(vt.deltas.is_empty());

        // Phase 2: write more output — goes into deltas
        vt.process_output(b"\r\nAfter keyframe");
        assert!(!vt.deltas.is_empty());

        // Replay should contain content from both phases
        let replay = vt.replay(24);
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(replay_str.contains("Before keyframe"));
        assert!(replay_str.contains("After keyframe"));
    }

    #[test]
    fn test_auto_compact_then_more_output() {
        let mut vt = VirtualTerminal::new(24, 80, 64, 0); // Small threshold
        // Write enough to trigger auto-compaction
        vt.process_output(
            b"AAAA repeating data that exceeds the sixty-four byte delta threshold!!",
        );
        assert!(vt.keyframe.is_some());
        assert!(vt.deltas.is_empty());

        // Write more output after auto-compaction
        vt.process_output(b"\r\nPost-compact line");
        assert!(!vt.deltas.is_empty());

        // Replay should contain everything
        let replay = vt.replay(24);
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(replay_str.contains("AAAA"));
        assert!(replay_str.contains("Post-compact line"));
    }

    #[test]
    fn test_empty_terminal_replay() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        // No output at all — replay on a fresh terminal
        let replay = vt.replay(24);
        // Should not be empty — the keyframe is the empty screen reset sequence
        assert!(!replay.is_empty());
        // Should contain the reset/clear sequence
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(replay_str.contains("\x1b[H\x1b[2J\x1b[0m"));
    }

    #[test]
    fn test_replay_idempotency() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        vt.process_output(b"Hello, world!\r\nLine two");

        let replay1 = vt.replay(24);
        let replay2 = vt.replay(24);
        assert_eq!(replay1, replay2);
    }

    #[test]
    fn test_cursor_position() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        assert_eq!(vt.cursor_position(), (0, 0));

        vt.process_output(b"Hello");
        assert_eq!(vt.cursor_position(), (0, 5));

        vt.process_output(b"\r\nLine 2");
        assert_eq!(vt.cursor_position(), (1, 6));
    }

    #[test]
    fn test_set_active_unknown_connection_id() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        // Should return None and not panic
        let result = vt.set_active("nonexistent", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_remove_unknown_connection_id() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        // Should return None and not panic
        let result = vt.remove_client("nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_resize_direct() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        vt.process_output(b"hello");
        vt.compact();
        assert!(vt.keyframe.is_some());

        // Direct resize (used by instance_actor after effective dims change)
        vt.resize(40, 120);

        // Replay should still work — content rendered at new size
        let replay = vt.replay(24);
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(replay_str.contains("hello"));
    }

    #[test]
    fn test_resize_then_replay() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 0);
        vt.process_output(b"Content at 24x80");
        vt.compact();
        assert!(vt.keyframe.is_some());

        // Resize via update_viewport — uses set_size, preserving content
        let dims_changed = vt.update_viewport("cli", 40, 120, ClientType::Web);
        assert_eq!(dims_changed, Some((40, 120)));
        assert!(vt.keyframe.is_none());
        assert!(vt.deltas.is_empty());

        // Replay preserves old content (set_size keeps screen state)
        let replay = vt.replay(24);
        assert!(!replay.is_empty());
        assert!(vt.keyframe.is_some());
        assert_eq!(vt.effective_dims(), (40, 120));
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(replay_str.contains("Content at 24x80"));

        // New output at the correct width IS captured
        vt.process_output(b"Content at 40x120");
        let replay = vt.replay(24);
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(replay_str.contains("Content at 40x120"));
    }

    // ── Scrollback tests ─────────────────────────────────────────────

    #[test]
    fn test_scrollback_in_replay() {
        // 4-row screen with 100 lines of scrollback
        let mut vt = VirtualTerminal::new(4, 40, 4096, 100);

        // Write 8 lines — 4 will scroll off into the scrollback buffer
        for i in 0..8 {
            vt.process_output(format!("Line {}\r\n", i).as_bytes());
        }

        let replay = vt.replay(24);
        let replay_str = String::from_utf8_lossy(&replay);

        // Scrollback lines (0-3) should be in the replay
        assert!(replay_str.contains("Line 0"), "scrollback line 0 missing");
        assert!(replay_str.contains("Line 3"), "scrollback line 3 missing");
        // Visible screen lines (4-7) should also be present
        assert!(replay_str.contains("Line 4"), "visible line 4 missing");
        assert!(replay_str.contains("Line 7"), "visible line 7 missing");
    }

    #[test]
    fn test_scrollback_empty_when_no_overflow() {
        let mut vt = VirtualTerminal::new(24, 80, 4096, 100);
        vt.process_output(b"Only one line");

        let replay = vt.replay(24);
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(replay_str.contains("Only one line"));
    }

    #[test]
    fn test_scrollback_zero_means_no_scrollback() {
        // scrollback_lines=0 — same as before
        let mut vt = VirtualTerminal::new(4, 40, 4096, 0);
        for i in 0..8 {
            vt.process_output(format!("Line {}\r\n", i).as_bytes());
        }

        let replay = vt.replay(24);
        let replay_str = String::from_utf8_lossy(&replay);
        // With zero scrollback, lines 0-4 are lost
        assert!(
            !replay_str.contains("Line 0"),
            "line 0 should be lost with zero scrollback"
        );
        assert!(
            !replay_str.contains("Line 4"),
            "line 4 should be lost with zero scrollback"
        );
        // Visible lines should be present (lines 5-7 remain on the 4-row screen
        // because each line ends with \r\n, scrolling the cursor down)
        assert!(replay_str.contains("Line 5"), "line 5 should be visible");
        assert!(replay_str.contains("Line 7"), "line 7 should be visible");
    }

    #[test]
    fn test_scrollback_preserves_formatting() {
        let mut vt = VirtualTerminal::new(4, 40, 4096, 100);

        // Write colored output that scrolls into scrollback
        for i in 0..8 {
            // \x1b[31m = red foreground
            vt.process_output(format!("\x1b[31mRed {}\x1b[0m\r\n", i).as_bytes());
        }

        let replay = vt.replay(24);
        let replay_str = String::from_utf8_lossy(&replay);

        // Scrollback should contain the color escape for old lines
        assert!(
            replay_str.contains("Red 0"),
            "scrollback should contain text"
        );
        // The replay should contain SGR sequences (red = 31)
        assert!(
            replay_str.contains("\x1b["),
            "scrollback should contain ANSI formatting"
        );
    }

    #[test]
    fn test_scrollback_survives_compaction() {
        let mut vt = VirtualTerminal::new(4, 40, 4096, 100);

        for i in 0..8 {
            vt.process_output(format!("Line {}\r\n", i).as_bytes());
        }
        vt.compact();

        // After compaction, replay should still include scrollback
        let replay = vt.replay(24);
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(
            replay_str.contains("Line 0"),
            "scrollback line should survive compaction"
        );
        assert!(
            replay_str.contains("Line 7"),
            "visible line should survive compaction"
        );
    }

    #[test]
    fn test_scrollback_capped_at_limit() {
        // Only 5 lines of scrollback
        let mut vt = VirtualTerminal::new(4, 40, 4096, 5);

        // Write 20 lines — 16 scroll off, but only 5 are retained
        for i in 0..20 {
            vt.process_output(format!("Line {:02}\r\n", i).as_bytes());
        }

        let replay = vt.replay(24);
        let replay_str = String::from_utf8_lossy(&replay);

        // Lines 0-10 should be gone (exceeded scrollback capacity)
        assert!(
            !replay_str.contains("Line 00"),
            "oldest lines should be evicted"
        );
        // Recent scrollback lines should be present
        assert!(
            replay_str.contains("Line 15"),
            "recent scrollback should be present"
        );
        // Visible screen lines should be present
        assert!(
            replay_str.contains("Line 19"),
            "visible lines should be present"
        );
    }

    #[test]
    fn test_resize_preserves_scrollback_buffer() {
        let mut vt = VirtualTerminal::new(4, 40, 4096, 500);
        // Fill some scrollback
        for i in 0..10 {
            vt.process_output(format!("Line {}\r\n", i).as_bytes());
        }
        // Resize via viewport change
        vt.update_viewport("cli", 30, 100, ClientType::Terminal);
        // Scrollback should survive the dimension change
        let replay = vt.replay(24);
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(
            replay_str.contains("Line 0"),
            "scrollback should survive resize"
        );
    }

    /// Viewport-driven dimension changes must preserve scrollback history.
    /// Regression test: previously, `recalculate_effective_dims` replaced the
    /// entire vt100 parser, destroying all scrollback.
    #[test]
    fn test_viewport_change_preserves_scrollback() {
        let mut vt = VirtualTerminal::new(4, 40, 4096, 100);

        // Build up scrollback
        for i in 0..20 {
            vt.process_output(format!("Line {:02}\r\n", i).as_bytes());
        }

        // Verify scrollback exists before dimension change
        let replay_before = vt.replay(24);
        let before_str = String::from_utf8_lossy(&replay_before);
        assert!(
            before_str.contains("Line 00"),
            "scrollback should exist before resize"
        );

        // Simulate a web client connecting with different dimensions —
        // this triggers recalculate_effective_dims
        vt.update_viewport("web-client", 30, 100, ClientType::Web);
        assert_eq!(vt.effective_dims(), (30, 100));

        // Scrollback must survive the dimension change
        let replay_after = vt.replay(24);
        let after_str = String::from_utf8_lossy(&replay_after);
        assert!(
            after_str.contains("Line 00"),
            "scrollback must survive viewport dimension change"
        );
        assert!(
            after_str.contains("Line 10"),
            "scrollback must survive viewport dimension change"
        );
    }

    /// Simulate the full client-side round-trip: server generates replay bytes,
    /// client processes them through a fresh vt100 parser, then check whether
    /// scrollback is accessible via set_scrollback/scrollback.
    #[test]
    fn test_replay_roundtrip_preserves_client_scrollback() {
        // Server side: 4-row screen, 100 lines of scrollback, lots of output
        let mut server_vt = VirtualTerminal::new(4, 40, 4096, 100);
        for i in 0..20 {
            server_vt.process_output(format!("Line {:02}\r\n", i).as_bytes());
        }

        // Server generates replay — client_rows=4 matches the client parser below
        let replay = server_vt.replay(4);
        assert!(!replay.is_empty());

        // Simulate JSON round-trip: server encodes as String, client decodes
        let replay_string = String::from_utf8_lossy(&replay).to_string();
        let replay_bytes = replay_string.as_bytes();

        // Client side: fresh parser with scrollback, same screen size
        let mut client_parser = vt100::Parser::new(4, 40, 100);
        client_parser.process(replay_bytes);

        // Client should have scrollback available
        client_parser.screen_mut().set_scrollback(usize::MAX);
        let client_scrollback = client_parser.screen().scrollback();
        assert!(
            client_scrollback > 0,
            "client should have scrollback after processing replay, got 0"
        );

        // Check oldest scrollback line is accessible
        let cell = client_parser.screen().cell(0, 0).unwrap();
        let ch = cell.contents();
        assert!(!ch.is_empty(), "oldest scrollback row should have content");

        // Reset to live screen
        client_parser.screen_mut().set_scrollback(0);
    }

    /// Regression: when the server has fewer scrollback lines than the client's
    /// screen height, the old replay format lost ALL scrollback (the lines
    /// stayed on the visible screen and were erased by \x1b[2J).
    #[test]
    fn test_replay_roundtrip_small_scrollback() {
        // Server: 24-row screen, only 5 lines of scrollback overflow
        let mut server_vt = VirtualTerminal::new(24, 80, 4096, 100);
        // Write 28 lines → 4 scroll off into scrollback (28 - 24 = 4)
        for i in 0..28 {
            server_vt.process_output(format!("SLine {:02}\r\n", i).as_bytes());
        }

        let replay = server_vt.replay(24);

        // Client: same screen size, processes the replay
        let mut client = vt100::Parser::new(24, 80, 100);
        client.process(&replay);

        // Even though only 4 lines were in scrollback (fewer than 24 rows),
        // the client should still have them.
        client.screen_mut().set_scrollback(usize::MAX);
        let sb = client.screen().scrollback();
        assert!(
            sb >= 4,
            "client should have at least 4 scrollback lines, got {}",
            sb,
        );
    }

    /// Verify that \x1b[2J (Erase in Display) does NOT clear the scrollback
    /// buffer in vt100. If this fails, the keyframe in our replay format
    /// would destroy scrollback on the client side.
    #[test]
    fn test_ed2_does_not_clear_scrollback() {
        let mut parser = vt100::Parser::new(4, 40, 100);

        // Fill screen and overflow into scrollback
        for i in 0..10 {
            parser.process(format!("Line {}\r\n", i).as_bytes());
        }

        // Verify scrollback exists
        parser.screen_mut().set_scrollback(usize::MAX);
        let before = parser.screen().scrollback();
        assert!(before > 0, "should have scrollback before ED2");
        parser.screen_mut().set_scrollback(0);

        // Clear screen with ED2 (the escape used in our keyframe)
        parser.process(b"\x1b[H\x1b[2J\x1b[0m");

        // Scrollback should still be there
        parser.screen_mut().set_scrollback(usize::MAX);
        let after = parser.screen().scrollback();
        assert!(
            after > 0,
            "scrollback should survive ED2 (\\x1b[2J), before={} after={}",
            before,
            after
        );
    }

    // ── Replay round-trip helpers ────────────────────────────────────

    /// Feed replay bytes into a fresh client parser, return it.
    fn replay_roundtrip(
        server_vt: &mut VirtualTerminal,
        client_rows: u16,
        client_cols: u16,
    ) -> vt100::Parser {
        let replay = server_vt.replay(client_rows);
        let mut client = vt100::Parser::new(client_rows, client_cols, 1000);
        client.process(&replay);
        client
    }

    /// Extract visible lines from a screen (strips trailing whitespace).
    fn visible_lines(screen: &vt100::Screen) -> Vec<String> {
        let (rows, cols) = screen.size();
        (0..rows)
            .map(|r| {
                (0..cols)
                    .map(|c| {
                        screen.cell(r, c).map_or(" ".to_string(), |cell| {
                            let s = cell.contents();
                            if s.is_empty() {
                                " ".to_string()
                            } else {
                                s.to_string()
                            }
                        })
                    })
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    /// Extract all scrollback lines from a parser (oldest first).
    fn scrollback_lines(parser: &mut vt100::Parser, cols: u16) -> Vec<String> {
        parser.screen_mut().set_scrollback(usize::MAX);
        let depth = parser.screen().scrollback();
        let mut lines = Vec::new();
        for offset in (1..=depth).rev() {
            parser.screen_mut().set_scrollback(offset);
            let row_text: String = (0..cols)
                .map(|c| {
                    parser.screen().cell(0, c).map_or(" ".to_string(), |cell| {
                        let s = cell.contents();
                        if s.is_empty() {
                            " ".to_string()
                        } else {
                            s.to_string()
                        }
                    })
                })
                .collect::<String>()
                .trim_end()
                .to_string();
            lines.push(row_text);
        }
        parser.screen_mut().set_scrollback(0);
        lines
    }

    // ── Replay rendering TDD tests ──────────────────────────────────

    /// Cell-level round-trip verification.
    #[test]
    fn test_replay_cell_content_exact() {
        let mut vt = VirtualTerminal::new(4, 40, 4096, 100);
        for i in 0..20 {
            vt.process_output(format!("Line {:02}\r\n", i).as_bytes());
        }

        let mut client = replay_roundtrip(&mut vt, 4, 40);
        let vis = visible_lines(client.screen());
        // Last \r\n scrolls cursor to row 3 → visible = [Line 17, Line 18, Line 19, ""]
        assert_eq!(vis[0], "Line 17", "visible row 0");
        assert_eq!(vis[1], "Line 18", "visible row 1");
        assert_eq!(vis[2], "Line 19", "visible row 2");
        assert_eq!(vis[3], "", "visible row 3 (empty after last \\r\\n)");

        let sb = scrollback_lines(&mut client, 40);
        // Filter out empty lines from the scroll-push mechanism
        let content_sb: Vec<_> = sb.iter().filter(|l| !l.is_empty()).collect();
        // Scrollback should contain Line 00..Line 16 in order
        assert!(
            content_sb.len() >= 16,
            "expected at least 16 content scrollback lines, got {}",
            content_sb.len()
        );
        assert!(
            content_sb.first().unwrap().contains("Line 00"),
            "first scrollback line should be Line 00, got {:?}",
            content_sb.first()
        );
        assert!(
            content_sb.last().unwrap().contains("Line 16"),
            "last content scrollback line should be Line 16, got {:?}",
            content_sb.last()
        );
    }

    /// Regression test: no CUP sequences in the scrollback portion of replay.
    #[test]
    fn test_replay_no_cup_in_scrollback() {
        let mut vt = VirtualTerminal::new(4, 40, 4096, 100);
        for i in 0..20 {
            vt.process_output(format!("Line {:02}\r\n", i).as_bytes());
        }

        let replay = vt.replay(4);

        // Find the keyframe boundary: \x1b[H\x1b[2J marks the start of the
        // visible screen section. Everything before it is scrollback + the
        // scroll-push section. The scroll-push section starts with a single
        // CUP \x1b[N;1H followed by newlines — exclude it by finding the
        // last \r\n before the keyframe marker.
        let replay_str = String::from_utf8_lossy(&replay);
        let keyframe_marker = "\x1b[H\x1b[2J";
        let keyframe_pos = replay_str
            .find(keyframe_marker)
            .expect("replay should contain keyframe marker");
        // The scrollback rows all end with \r\n. Find the last one before the keyframe.
        let scrollback_end = replay_str[..keyframe_pos].rfind("\r\n").unwrap_or(0);
        let scrollback_portion = &replay_str[..scrollback_end];

        // Scan for CUP sequences: \x1b[ <digits> ; <digits> H
        // Manual scan avoids a regex dependency.
        let sb_bytes = scrollback_portion.as_bytes();
        let mut cups_found = Vec::new();
        let mut i = 0;
        while i < sb_bytes.len().saturating_sub(4) {
            if sb_bytes[i] == b'\x1b' && sb_bytes[i + 1] == b'[' {
                let start = i;
                let mut j = i + 2;
                // digits
                while j < sb_bytes.len() && sb_bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j < sb_bytes.len() && sb_bytes[j] == b';' {
                    j += 1;
                    while j < sb_bytes.len() && sb_bytes[j].is_ascii_digit() {
                        j += 1;
                    }
                    if j < sb_bytes.len() && sb_bytes[j] == b'H' {
                        cups_found.push(String::from_utf8_lossy(&sb_bytes[start..=j]).to_string());
                    }
                }
            }
            i += 1;
        }
        assert!(
            cups_found.is_empty(),
            "scrollback portion should contain no CUP sequences, found: {:?}",
            cups_found
        );
    }

    /// The specific trigger for the CUP bug: lines with leading spaces.
    #[test]
    fn test_replay_scrollback_with_indented_lines() {
        let mut vt = VirtualTerminal::new(4, 40, 4096, 100);
        for i in 0..12 {
            vt.process_output(format!("    indented line {}\r\n", i).as_bytes());
        }

        let mut client = replay_roundtrip(&mut vt, 4, 40);
        let sb = scrollback_lines(&mut client, 40);
        // Filter out empty lines from the scroll-push mechanism
        let content_sb: Vec<_> = sb.into_iter().filter(|l| !l.is_empty()).collect();

        assert!(
            content_sb.len() >= 8,
            "should have at least 8 content scrollback lines, got {}",
            content_sb.len()
        );

        // All content scrollback lines should have the indentation and correct content
        for (idx, line) in content_sb.iter().enumerate() {
            assert!(
                line.starts_with("    indented line"),
                "scrollback line {} should start with '    indented line', got {:?}",
                idx,
                line
            );
        }

        // No line should contain content from another line (no overwrite artifacts)
        for line in &content_sb {
            let matches: Vec<_> = content_sb
                .iter()
                .filter(|other| line.contains(other.as_str()))
                .collect();
            assert!(
                matches.len() <= 1,
                "line {:?} should not contain content from other lines",
                line
            );
        }
    }

    /// Rows with trailing empty cells (short lines in a wide terminal).
    #[test]
    fn test_replay_scrollback_with_partial_lines() {
        let mut vt = VirtualTerminal::new(4, 80, 4096, 100);
        for i in 0..12 {
            vt.process_output(format!("Hi {}\r\n", i).as_bytes());
        }

        let mut client = replay_roundtrip(&mut vt, 4, 80);
        let sb = scrollback_lines(&mut client, 80);
        // Filter out empty lines from the scroll-push mechanism
        let content_lines: Vec<_> = sb.iter().filter(|l| !l.is_empty()).collect();

        for (idx, line) in content_lines.iter().enumerate() {
            assert!(
                line.starts_with("Hi "),
                "scrollback line {} should start with 'Hi ', got {:?}",
                idx,
                line
            );
        }
        assert!(
            content_lines.len() >= 8,
            "should have at least 8 content scrollback lines, got {}",
            content_lines.len()
        );
    }

    /// Client bigger than server — content should still be correct.
    #[test]
    fn test_replay_mismatched_dims() {
        let mut vt = VirtualTerminal::new(4, 40, 4096, 100);
        for i in 0..20 {
            vt.process_output(format!("Line {:02}\r\n", i).as_bytes());
        }

        let mut client = replay_roundtrip(&mut vt, 24, 80);
        let vis = visible_lines(client.screen());

        // The visible screen on the client should contain the server's visible content
        let vis_text = vis.join("\n");
        assert!(
            vis_text.contains("Line 17"),
            "client should show server's visible content"
        );
        assert!(
            vis_text.contains("Line 19"),
            "client should show server's visible content"
        );

        let sb = scrollback_lines(&mut client, 80);
        // Earlier lines should be in scrollback
        let sb_text = sb.join("\n");
        assert!(
            sb_text.contains("Line 00"),
            "earlier lines should be in client scrollback"
        );
    }

    /// SGR round-trip: colors survive replay.
    #[test]
    fn test_replay_scrollback_colors_preserved() {
        let mut vt = VirtualTerminal::new(4, 40, 4096, 100);
        for i in 0..8 {
            vt.process_output(format!("\x1b[31mRed {}\x1b[0m\r\n", i).as_bytes());
        }

        let mut client = replay_roundtrip(&mut vt, 4, 40);

        // Check scrollback cells for red foreground
        client.screen_mut().set_scrollback(usize::MAX);
        let depth = client.screen().scrollback();
        assert!(depth > 0, "client should have scrollback");

        // Check the first scrollback row (oldest)
        client.screen_mut().set_scrollback(depth);
        let cell = client.screen().cell(0, 0).unwrap();
        assert_eq!(
            cell.fgcolor(),
            vt100::Color::Idx(1),
            "scrollback cell should have red foreground, got {:?}",
            cell.fgcolor()
        );
        client.screen_mut().set_scrollback(0);
    }

    /// Ordering check across multiple pages — no repeats or gaps.
    #[test]
    fn test_replay_scrollback_order_preserved() {
        let mut vt = VirtualTerminal::new(4, 40, 4096, 500);
        for i in 0..100 {
            vt.process_output(format!("Line {:04}\r\n", i).as_bytes());
        }

        let mut client = replay_roundtrip(&mut vt, 4, 40);
        let sb = scrollback_lines(&mut client, 40);

        // Extract line numbers from scrollback and verify ascending order
        let mut prev_num: Option<u32> = None;
        for line in &sb {
            if let Some(pos) = line.find("Line ") {
                let num_str = &line[pos + 5..pos + 9];
                if let Ok(num) = num_str.trim().parse::<u32>() {
                    if let Some(p) = prev_num {
                        assert!(
                            num > p,
                            "scrollback lines should be in ascending order: {} followed by {}",
                            p,
                            num
                        );
                    }
                    prev_num = Some(num);
                }
            }
        }
        assert!(
            prev_num.is_some(),
            "should have found numbered lines in scrollback"
        );
    }

    /// Scrollback survives dimension change.
    #[test]
    fn test_replay_after_resize() {
        let mut vt = VirtualTerminal::new(4, 40, 4096, 100);
        for i in 0..20 {
            vt.process_output(format!("Line {:02}\r\n", i).as_bytes());
        }

        // Resize to 8×60
        vt.update_viewport("cli", 8, 60, ClientType::Terminal);

        let mut client = replay_roundtrip(&mut vt, 8, 60);
        let sb = scrollback_lines(&mut client, 60);

        let sb_text = sb.join("\n");
        assert!(
            sb_text.contains("Line 00"),
            "scrollback should survive resize"
        );
        assert!(
            sb_text.contains("Line 10"),
            "mid-range scrollback should survive resize"
        );

        // Verify ordering
        let mut prev_num: Option<u32> = None;
        for line in &sb {
            if let Some(pos) = line.find("Line ") {
                let num_str = &line[pos + 5..pos + 7];
                if let Ok(num) = num_str.trim().parse::<u32>() {
                    if let Some(p) = prev_num {
                        assert!(num > p, "order broken after resize: {} then {}", p, num);
                    }
                    prev_num = Some(num);
                }
            }
        }
    }

    /// CJK / wide characters in scrollback.
    #[test]
    fn test_replay_wide_chars_in_scrollback() {
        let mut vt = VirtualTerminal::new(4, 40, 4096, 100);
        for i in 0..8 {
            vt.process_output(format!("行{} test\r\n", i).as_bytes());
        }

        let mut client = replay_roundtrip(&mut vt, 4, 40);
        let sb = scrollback_lines(&mut client, 40);
        // Filter out empty lines from the scroll-push mechanism
        let content_sb: Vec<_> = sb.into_iter().filter(|l| !l.is_empty()).collect();

        assert!(
            content_sb.len() >= 4,
            "should have at least 4 CJK scrollback lines, got {}",
            content_sb.len()
        );

        for (idx, line) in content_sb.iter().enumerate() {
            assert!(
                line.contains("行"),
                "scrollback line {} should contain CJK char, got {:?}",
                idx,
                line
            );
            assert!(
                line.contains("test"),
                "scrollback line {} should contain 'test', got {:?}",
                idx,
                line
            );
        }
    }

    /// Edge case: no scrollback overflow → replay should not contain scroll-clear section.
    #[test]
    fn test_replay_empty_scrollback_no_scroll_clear() {
        let mut vt = VirtualTerminal::new(4, 40, 4096, 100);
        // Only 2 lines — no scrollback overflow
        vt.process_output(b"Line A\r\nLine B\r\n");

        let replay = vt.replay(4);
        let replay_str = String::from_utf8_lossy(&replay);

        // Should NOT contain the scroll-clear newline sequence that pushes
        // scrollback down — there's nothing to push.
        // The replay should start directly with the keyframe.
        assert!(
            replay_str.starts_with("\x1b[H\x1b[2J"),
            "replay with no scrollback should start with keyframe, got: {:?}",
            &replay_str[..replay_str.len().min(40)]
        );

        // Visible lines should be correct
        let mut client = vt100::Parser::new(4, 40, 100);
        client.process(&replay);
        let vis = visible_lines(client.screen());
        assert_eq!(vis[0], "Line A");
        assert_eq!(vis[1], "Line B");
    }
}
