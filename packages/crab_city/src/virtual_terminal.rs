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
    pub fn new(rows: u16, cols: u16, max_delta_bytes: usize) -> Self {
        Self {
            parser: vt100::Parser::new(rows, cols, 0),
            client_viewports: HashMap::new(),
            effective_dims: (rows, cols),
            keyframe: None,
            deltas: Vec::new(),
            max_delta_bytes,
        }
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

    /// Get replay data for a new client: keyframe + deltas.
    /// If no keyframe exists, compacts first.
    pub fn replay(&mut self) -> Vec<u8> {
        if self.keyframe.is_none() {
            self.compact();
        }
        let mut result = Vec::new();
        if let Some(ref kf) = self.keyframe {
            result.extend_from_slice(kf);
        }
        result.extend_from_slice(&self.deltas);
        result
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
        self.parser.set_size(rows, cols);
    }

    /// Current effective dimensions.
    pub fn effective_dims(&self) -> (u16, u16) {
        self.effective_dims
    }

    /// Recalculate effective dims from active viewports.
    /// Returns `Some((rows, cols))` if dims changed, `None` otherwise.
    fn recalculate_effective_dims(&mut self) -> Option<(u16, u16)> {
        let new_dims = calculate_effective_dims(&self.client_viewports);
        if let Some((rows, cols)) = new_dims {
            if (rows, cols) != self.effective_dims {
                self.effective_dims = (rows, cols);
                // Fresh parser at new dimensions — old screen content was
                // rendered at stale dimensions and vt100's set_size merges
                // soft-wrapped lines, corrupting the display.  The PTY resize
                // will SIGWINCH the application, which redraws cleanly.
                self.parser = vt100::Parser::new(rows, cols, 0);
                self.keyframe = None;
                self.deltas.clear();
                return Some((rows, cols));
            }
        }
        // If no active viewports, keep current dims (don't shrink to nothing)
        None
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
        let vt = VirtualTerminal::new(24, 80, 4096);
        assert_eq!(vt.effective_dims(), (24, 80));
    }

    #[test]
    fn test_single_client_viewport() {
        let mut vt = VirtualTerminal::new(24, 80, 4096);
        let result = vt.update_viewport("client-1", 40, 120, ClientType::Web);
        // Effective dims changed from (24, 80) to (40, 120)
        assert_eq!(result, Some((40, 120)));
        assert_eq!(vt.effective_dims(), (40, 120));
    }

    #[test]
    fn test_two_clients_min_dims() {
        let mut vt = VirtualTerminal::new(24, 80, 4096);
        vt.update_viewport("web", 40, 120, ClientType::Web);
        let result = vt.update_viewport("cli", 24, 80, ClientType::Terminal);
        // min(40,24)=24, min(120,80)=80
        assert_eq!(result, Some((24, 80)));
        assert_eq!(vt.effective_dims(), (24, 80));
    }

    #[test]
    fn test_inactive_client_excluded() {
        let mut vt = VirtualTerminal::new(24, 80, 4096);
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
        let mut vt = VirtualTerminal::new(24, 80, 4096);
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
        let mut vt = VirtualTerminal::new(24, 80, 4096);
        vt.update_viewport("web", 40, 120, ClientType::Web);
        vt.update_viewport("cli", 24, 80, ClientType::Terminal);
        assert_eq!(vt.effective_dims(), (24, 80));

        let result = vt.remove_client("cli");
        assert_eq!(result, Some((40, 120)));
    }

    #[test]
    fn test_remove_last_client_keeps_dims() {
        let mut vt = VirtualTerminal::new(24, 80, 4096);
        vt.update_viewport("cli", 30, 100, ClientType::Terminal);
        assert_eq!(vt.effective_dims(), (30, 100));

        // Remove the only client — dims stay at (30, 100)
        let result = vt.remove_client("cli");
        assert!(result.is_none());
        assert_eq!(vt.effective_dims(), (30, 100));
    }

    #[test]
    fn test_process_output_and_compact_replay() {
        let mut vt = VirtualTerminal::new(24, 80, 4096);
        vt.process_output(b"Hello, world!");
        vt.process_output(b"\r\nLine 2");

        let replay = vt.replay();
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
        let mut vt = VirtualTerminal::new(24, 80, 100); // Small threshold
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
        let mut vt = VirtualTerminal::new(24, 80, 4096);
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
        let mut vt = VirtualTerminal::new(24, 80, 4096);
        vt.update_viewport("cli", 30, 100, ClientType::Terminal);
        assert_eq!(vt.effective_dims(), (30, 100));

        // Same dims again — no change
        let result = vt.update_viewport("cli", 30, 100, ClientType::Terminal);
        assert!(result.is_none());
    }

    #[test]
    fn test_replay_compacts_if_no_keyframe() {
        let mut vt = VirtualTerminal::new(24, 80, 4096);
        vt.process_output(b"Hello");
        assert!(vt.keyframe.is_none());

        let replay = vt.replay();
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
        let mut vt = VirtualTerminal::new(24, 80, 4096);
        // update_viewport always sets is_active=true
        vt.update_viewport("cli", 30, 100, ClientType::Terminal);
        let vp = vt.client_viewports.get("cli").unwrap();
        assert!(vp.is_active);
    }

    #[test]
    fn test_keyframe_plus_deltas_replay() {
        let mut vt = VirtualTerminal::new(24, 80, 4096);
        // Phase 1: write output, then compact to create a keyframe
        vt.process_output(b"Before keyframe");
        vt.compact();
        assert!(vt.keyframe.is_some());
        assert!(vt.deltas.is_empty());

        // Phase 2: write more output — goes into deltas
        vt.process_output(b"\r\nAfter keyframe");
        assert!(!vt.deltas.is_empty());

        // Replay should contain content from both phases
        let replay = vt.replay();
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(replay_str.contains("Before keyframe"));
        assert!(replay_str.contains("After keyframe"));
    }

    #[test]
    fn test_auto_compact_then_more_output() {
        let mut vt = VirtualTerminal::new(24, 80, 64); // Small threshold
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
        let replay = vt.replay();
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(replay_str.contains("AAAA"));
        assert!(replay_str.contains("Post-compact line"));
    }

    #[test]
    fn test_empty_terminal_replay() {
        let mut vt = VirtualTerminal::new(24, 80, 4096);
        // No output at all — replay on a fresh terminal
        let replay = vt.replay();
        // Should not be empty — the keyframe is the empty screen reset sequence
        assert!(!replay.is_empty());
        // Should contain the reset/clear sequence
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(replay_str.contains("\x1b[H\x1b[2J\x1b[0m"));
    }

    #[test]
    fn test_replay_idempotency() {
        let mut vt = VirtualTerminal::new(24, 80, 4096);
        vt.process_output(b"Hello, world!\r\nLine two");

        let replay1 = vt.replay();
        let replay2 = vt.replay();
        assert_eq!(replay1, replay2);
    }

    #[test]
    fn test_set_active_unknown_connection_id() {
        let mut vt = VirtualTerminal::new(24, 80, 4096);
        // Should return None and not panic
        let result = vt.set_active("nonexistent", false);
        assert!(result.is_none());
    }

    #[test]
    fn test_remove_unknown_connection_id() {
        let mut vt = VirtualTerminal::new(24, 80, 4096);
        // Should return None and not panic
        let result = vt.remove_client("nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_resize_direct() {
        let mut vt = VirtualTerminal::new(24, 80, 4096);
        vt.process_output(b"hello");
        vt.compact();
        assert!(vt.keyframe.is_some());

        // Direct resize (used by instance_actor after effective dims change)
        vt.resize(40, 120);

        // Replay should still work — content rendered at new size
        let replay = vt.replay();
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(replay_str.contains("hello"));
    }

    #[test]
    fn test_resize_then_replay() {
        let mut vt = VirtualTerminal::new(24, 80, 4096);
        vt.process_output(b"Content at 24x80");
        vt.compact();
        assert!(vt.keyframe.is_some());

        // Resize via update_viewport — resets parser and clears deltas
        let dims_changed = vt.update_viewport("cli", 40, 120, ClientType::Web);
        assert_eq!(dims_changed, Some((40, 120)));
        assert!(vt.keyframe.is_none());
        assert!(vt.deltas.is_empty());

        // Replay on clean parser: just the reset sequence, no stale content
        let replay = vt.replay();
        assert!(!replay.is_empty());
        assert!(vt.keyframe.is_some());
        assert_eq!(vt.effective_dims(), (40, 120));
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(!replay_str.contains("Content at 24x80"));

        // New output at the correct width IS captured
        vt.process_output(b"Content at 40x120");
        let replay = vt.replay();
        let replay_str = String::from_utf8_lossy(&replay);
        assert!(replay_str.contains("Content at 40x120"));
    }
}
