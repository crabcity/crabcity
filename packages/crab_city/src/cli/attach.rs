use anyhow::Result;
use futures::{SinkExt, StreamExt};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseEventKind,
};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::Widget;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite;

use crate::cli::daemon::{DaemonError, DaemonInfo};
use crate::cli::terminal::get_terminal_size;
use crate::config::{MAX_SCROLLBACK_LINES, MIN_SCROLLBACK_LINES};
use crate::inference::ClaudeState;
use crate::websocket_proxy::WsMessage;

/// Default scrollback lines if config fetch fails.
const DEFAULT_SCROLLBACK_LINES: usize = 10_000;

/// Lines scrolled per mouse wheel tick.
const MOUSE_SCROLL_LINES: usize = 3;

/// Detect detach key (Ctrl-]).
///
/// Crossterm 0.28 maps bytes 0x1C-0x1F to Ctrl+'4'-'7' (not the real
/// characters \, ], ^, _). Byte 0x1D (Ctrl-]) therefore arrives as
/// `Char('5') + CONTROL`.  Match both representations for safety.
fn is_detach_key(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5'))
}

/// Build status bar text based on current Claude state and elapsed time.
fn status_bar_text(state: &ClaudeState, started_at: Instant) -> String {
    match state {
        ClaudeState::Initializing => {
            let elapsed = started_at.elapsed().as_secs();
            let msg = if elapsed < 10 {
                "Waiting for first byte..."
            } else if elapsed < 30 {
                "Process is starting \u{2014} no output yet"
            } else {
                "No output received. Ctrl-] to switch instances"
            };
            format!(" INIT \u{2502} {} \u{2502} Ctrl-] switch ", msg)
        }
        ClaudeState::Starting => {
            let elapsed = started_at.elapsed().as_secs();
            let msg = if elapsed < 10 {
                "Loading Claude Code..."
            } else if elapsed < 30 {
                "Claude is taking a moment to initialize..."
            } else {
                "Startup is slower than usual \u{2014} network latency or API load"
            };
            format!(" STARTING \u{2502} {} \u{2502} Ctrl-] switch ", msg)
        }
        ClaudeState::Idle | ClaudeState::WaitingForInput { .. } => {
            " READY \u{2502} Claude is idle \u{2502} Ctrl-] detach ".to_string()
        }
        ClaudeState::Thinking => " ACTIVE \u{2502} Thinking... \u{2502} Ctrl-] detach ".to_string(),
        ClaudeState::Responding => {
            " ACTIVE \u{2502} Responding... \u{2502} Ctrl-] detach ".to_string()
        }
        ClaudeState::ToolExecuting { tool } => {
            format!(
                " ACTIVE \u{2502} Running {}... \u{2502} Ctrl-] detach ",
                tool
            )
        }
    }
}

// ── Color conversion ────────────────────────────────────────────────

fn vt100_color_to_ratatui(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(n) => Color::Indexed(n),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

// ── PtyWidget ───────────────────────────────────────────────────────

/// Renders a `vt100::Screen` buffer into a ratatui `Buffer`.
struct PtyWidget<'a> {
    screen: &'a vt100::Screen,
}

impl Widget for PtyWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let screen = self.screen;
        for row in 0..area.height {
            for col in 0..area.width {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let contents = cell.contents();
                let symbol = if contents.is_empty() { " " } else { contents };

                let mut style = Style::default()
                    .fg(vt100_color_to_ratatui(cell.fgcolor()))
                    .bg(vt100_color_to_ratatui(cell.bgcolor()));
                let mut modifiers = Modifier::empty();
                if cell.bold() {
                    modifiers |= Modifier::BOLD;
                }
                if cell.italic() {
                    modifiers |= Modifier::ITALIC;
                }
                if cell.underline() {
                    modifiers |= Modifier::UNDERLINED;
                }
                if cell.inverse() {
                    modifiers |= Modifier::REVERSED;
                }
                style = style.add_modifier(modifiers);

                let x = area.x + col;
                let y = area.y + row;
                if x < area.right() && y < area.bottom() {
                    buf[(x, y)].set_symbol(symbol).set_style(style);
                }
            }
        }
    }
}

// ── StatusBarWidget ─────────────────────────────────────────────────

/// Full-width reversed+bold status bar.
struct StatusBarWidget<'a> {
    text: &'a str,
}

impl Widget for StatusBarWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let style = Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD);
        // Fill entire area with spaces first
        for x in area.left()..area.right() {
            buf[(x, area.y)].set_symbol(" ").set_style(style);
        }
        // Write text characters
        let mut col = area.x;
        for ch in self.text.chars() {
            if col >= area.right() {
                break;
            }
            buf[(col, area.y)]
                .set_symbol(&ch.to_string())
                .set_style(style);
            col += 1;
        }
    }
}

// ── OverlayBadge ────────────────────────────────────────────────────

/// Right-aligned reversed badge rendered in the top-right of the given area.
struct OverlayBadge<'a> {
    text: &'a str,
}

impl Widget for OverlayBadge<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let padded = format!(" {} ", self.text);
        let badge_width = padded.len() as u16;
        if badge_width > area.width {
            return;
        }
        let style = Style::default().add_modifier(Modifier::REVERSED);
        let start_col = area.right().saturating_sub(badge_width);
        for (i, ch) in padded.chars().enumerate() {
            let x = start_col + i as u16;
            if x < area.right() {
                buf[(x, area.y)]
                    .set_symbol(&ch.to_string())
                    .set_style(style);
            }
        }
    }
}

// ── key_to_bytes ────────────────────────────────────────────────────

/// Convert a crossterm `KeyEvent` to the byte sequence a PTY expects.
fn key_to_bytes(key: &KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char(c) if ctrl => {
            // Crossterm 0.28 maps control bytes to KeyCode::Char in two ranges:
            //   0x01-0x1A  → Ctrl + 'a'-'z'   (c - 0x01 + b'a')
            //   0x1C-0x1F  → Ctrl + '4'-'7'   (c - 0x1C + b'4')
            // Reverse both mappings to recover the original byte.
            match c {
                'a'..='z' => Some(vec![(c as u8) - b'a' + 1]),
                '4'..='7' => Some(vec![(c as u8) - b'4' + 0x1C]),
                _ => None,
            }
        }
        KeyCode::Char(c) => {
            let mut bytes = [0u8; 4];
            let s = c.encode_utf8(&mut bytes);
            Some(s.as_bytes().to_vec())
        }
        KeyCode::Enter => Some(b"\r".to_vec()),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Esc => Some(b"\x1b".to_vec()),
        KeyCode::Tab => Some(b"\t".to_vec()),
        KeyCode::BackTab => Some(b"\x1b[Z".to_vec()),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::F(1) => Some(b"\x1bOP".to_vec()),
        KeyCode::F(2) => Some(b"\x1bOQ".to_vec()),
        KeyCode::F(3) => Some(b"\x1bOR".to_vec()),
        KeyCode::F(4) => Some(b"\x1bOS".to_vec()),
        KeyCode::F(5) => Some(b"\x1b[15~".to_vec()),
        KeyCode::F(6) => Some(b"\x1b[17~".to_vec()),
        KeyCode::F(7) => Some(b"\x1b[18~".to_vec()),
        KeyCode::F(8) => Some(b"\x1b[19~".to_vec()),
        KeyCode::F(9) => Some(b"\x1b[20~".to_vec()),
        KeyCode::F(10) => Some(b"\x1b[21~".to_vec()),
        KeyCode::F(11) => Some(b"\x1b[23~".to_vec()),
        KeyCode::F(12) => Some(b"\x1b[24~".to_vec()),
        _ => None,
    }
}

// ── Events from the WS reader task ─────────────────────────────────

enum AttachEvent {
    Output(String),
    StateChange(ClaudeState),
    Closed,
}

/// What happened when an attach session ended.
pub enum AttachOutcome {
    /// User pressed Ctrl-] to detach; instance is still running.
    Detached,
    /// The remote process exited (WebSocket closed).
    Exited,
}

/// Attach to an instance, forwarding terminal I/O over WebSocket.
pub async fn attach(daemon: &DaemonInfo, instance_id: &str) -> Result<AttachOutcome, DaemonError> {
    // 1. Fetch scrollback_lines from server config (best-effort, fall back to default)
    let scrollback_lines = fetch_scrollback_lines(daemon).await;

    // 2. Connect WebSocket — the server sends a screen replay as the first
    //    Output message (see websocket_proxy::handle_proxy), so there is no
    //    need for a separate HTTP fetch of /output.
    let ws_url = daemon.ws_url(instance_id);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .map_err(DaemonError::from_tungstenite)?;

    // 3. Session phase — internal anyhow, mapped to Other at boundary
    attach_session(ws_stream, scrollback_lines)
        .await
        .map_err(Into::into)
}

/// Fetch scrollback_lines from the server config endpoint.
/// Clamped to 100–100,000 to prevent degenerate allocations.
async fn fetch_scrollback_lines(daemon: &DaemonInfo) -> usize {
    let url = format!("{}/api/admin/config", daemon.base_url());
    let result: Option<usize> = async {
        let resp = reqwest::get(&url).await.ok()?;
        let json: serde_json::Value = resp.json().await.ok()?;
        json.get("scrollback_lines")?.as_u64().map(|v| v as usize)
    }
    .await;
    result
        .unwrap_or(DEFAULT_SCROLLBACK_LINES)
        .clamp(MIN_SCROLLBACK_LINES, MAX_SCROLLBACK_LINES)
}

/// Run the attach session after WebSocket is connected.
async fn attach_session(
    ws_stream: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    scrollback_lines: usize,
) -> Result<AttachOutcome> {
    let (mut ws_write, mut ws_read) = ws_stream.split();

    // Size the PTY: terminal height minus 1 row for status bar
    let (term_rows, term_cols) = get_terminal_size().unwrap_or((24, 80));
    let pty_rows = term_rows.saturating_sub(1).max(1);

    let mut vt_parser = vt100::Parser::new(pty_rows, term_cols, scrollback_lines);

    // Channels: WS reader → main loop, main loop → WS writer
    let (ws_read_tx, ws_read_rx) = std::sync::mpsc::channel::<AttachEvent>();
    let (ws_write_tx, ws_write_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Send initial resize
    let resize_msg = WsMessage::Resize {
        rows: pty_rows,
        cols: term_cols,
    };
    let json = serde_json::to_string(&resize_msg)?;
    ws_write.send(tungstenite::Message::Text(json)).await?;

    // Spawn async WS reader task
    let read_tx = ws_read_tx.clone();
    tokio::spawn(async move {
        while let Some(msg) = ws_read.next().await {
            match msg {
                Ok(tungstenite::Message::Text(text)) => {
                    match serde_json::from_str::<WsMessage>(&text) {
                        Ok(WsMessage::Output { data, .. }) => {
                            if read_tx.send(AttachEvent::Output(data)).is_err() {
                                break;
                            }
                        }
                        Ok(WsMessage::StateChange { state, .. }) => {
                            if read_tx.send(AttachEvent::StateChange(state)).is_err() {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                Ok(tungstenite::Message::Close(_)) | Err(_) => {
                    let _ = read_tx.send(AttachEvent::Closed);
                    break;
                }
                _ => {}
            }
        }
    });

    // Spawn async WS writer task
    let mut ws_write_rx = ws_write_rx;
    tokio::spawn(async move {
        while let Some(json) = ws_write_rx.recv().await {
            if ws_write
                .send(tungstenite::Message::Text(json))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Enter the blocking ratatui render loop (with mouse capture for scroll wheel)
    let mut terminal = ratatui::init();
    let _ = ratatui::crossterm::execute!(std::io::stdout(), EnableMouseCapture);
    let outcome = tokio::task::block_in_place(|| {
        run_event_loop(&mut terminal, &mut vt_parser, &ws_read_rx, &ws_write_tx)
    });
    let _ = ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();

    outcome
}

/// Blocking event loop: drain WS events, render, handle input.
fn run_event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    vt_parser: &mut vt100::Parser,
    ws_read_rx: &std::sync::mpsc::Receiver<AttachEvent>,
    ws_write_tx: &tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<AttachOutcome> {
    let mut claude_state = ClaudeState::Initializing;
    let attach_time = Instant::now();
    let badge_until = Instant::now() + Duration::from_secs(5);
    const OVERLAY_TEXT: &str = "attached -- Ctrl-] to detach";

    // Scrollback: 0 = live screen (bottom), >0 = scrolled into history.
    let mut scroll_offset: usize = 0;

    loop {
        // Drain WS messages
        let mut ws_closed = false;
        while let Ok(ev) = ws_read_rx.try_recv() {
            match ev {
                AttachEvent::Output(data) => vt_parser.process(data.as_bytes()),
                AttachEvent::StateChange(state) => claude_state = state,
                AttachEvent::Closed => {
                    ws_closed = true;
                }
            }
        }
        if ws_closed {
            eprintln!("\r\n[crab: exited]");
            return Ok(AttachOutcome::Exited);
        }

        // Apply scrollback viewport before reading the screen.
        // set_scrollback clamps internally; read back the actual offset so our
        // counter doesn't run past the real history depth.
        vt_parser.screen_mut().set_scrollback(scroll_offset);
        scroll_offset = vt_parser.screen().scrollback();

        // Render
        let screen = vt_parser.screen();
        let bar_text = if scroll_offset > 0 {
            format!(
                " SCROLL \u{2502} {} lines up \u{2502} Shift-PgDn or scroll to return ",
                scroll_offset
            )
        } else {
            status_bar_text(&claude_state, attach_time)
        };
        let show_badge = Instant::now() < badge_until;
        let cursor_pos = screen.cursor_position();
        let hide_cursor = screen.hide_cursor();
        let at_bottom = scroll_offset == 0;

        terminal.draw(|frame| {
            let [content, status] =
                Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());

            frame.render_widget(PtyWidget { screen }, content);
            frame.render_widget(StatusBarWidget { text: &bar_text }, status);

            if show_badge && at_bottom {
                frame.render_widget(OverlayBadge { text: OVERLAY_TEXT }, content);
            }

            // Only show cursor when at live screen position
            if at_bottom && !hide_cursor {
                let (row, col) = cursor_pos;
                frame.set_cursor_position((content.x + col, content.y + row));
            }
        })?;

        // Input events
        if event::poll(Duration::from_millis(16))? {
            let ev = event::read()?;

            // Compute content area height for page scrolling
            let page_size = terminal
                .size()
                .map_or(23, |s| s.height.saturating_sub(1).max(1) as usize);

            match ev {
                // Scroll keys: Shift+PageUp/Down, Shift+Up/Down
                Event::Key(key)
                    if key.kind == KeyEventKind::Press
                        && key.modifiers.contains(KeyModifiers::SHIFT) =>
                {
                    match key.code {
                        KeyCode::PageUp => {
                            scroll_offset = scroll_offset.saturating_add(page_size);
                        }
                        KeyCode::PageDown => {
                            scroll_offset = scroll_offset.saturating_sub(page_size);
                        }
                        KeyCode::Up => {
                            scroll_offset = scroll_offset.saturating_add(1);
                        }
                        KeyCode::Down => {
                            scroll_offset = scroll_offset.saturating_sub(1);
                        }
                        _ => {
                            // Any other Shift+key: snap to bottom and forward
                            scroll_offset = 0;
                            if let Some(bytes) = key_to_bytes(&key) {
                                send_input(ws_write_tx, &bytes);
                            }
                        }
                    }
                }

                // Mouse wheel
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        scroll_offset = scroll_offset.saturating_add(MOUSE_SCROLL_LINES);
                    }
                    MouseEventKind::ScrollDown => {
                        scroll_offset = scroll_offset.saturating_sub(MOUSE_SCROLL_LINES);
                    }
                    _ => {}
                },

                // Regular key presses
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if is_detach_key(&key) {
                        eprintln!("\r\n[crab: detached]");
                        return Ok(AttachOutcome::Detached);
                    }
                    // Any non-scroll input snaps back to live screen
                    scroll_offset = 0;
                    if let Some(bytes) = key_to_bytes(&key) {
                        send_input(ws_write_tx, &bytes);
                    }
                }

                Event::Resize(cols, rows) => {
                    let pty_rows = rows.saturating_sub(1).max(1);
                    vt_parser.screen_mut().set_size(pty_rows, cols);
                    let msg = WsMessage::Resize {
                        rows: pty_rows,
                        cols,
                    };
                    if let Ok(json) = serde_json::to_string(&msg) {
                        let _ = ws_write_tx.send(json);
                    }
                }

                _ => {}
            }
        }
    }
}

/// Send input bytes to the PTY via the WS writer channel.
fn send_input(ws_write_tx: &tokio::sync::mpsc::UnboundedSender<String>, bytes: &[u8]) {
    let msg = WsMessage::Input {
        data: String::from_utf8_lossy(bytes).to_string(),
    };
    if let Ok(json) = serde_json::to_string(&msg) {
        let _ = ws_write_tx.send(json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Color conversion ────────────────────────────────────────────

    #[test]
    fn color_conversion_default() {
        assert_eq!(vt100_color_to_ratatui(vt100::Color::Default), Color::Reset);
    }

    #[test]
    fn color_conversion_indexed() {
        assert_eq!(
            vt100_color_to_ratatui(vt100::Color::Idx(42)),
            Color::Indexed(42)
        );
    }

    #[test]
    fn color_conversion_rgb() {
        assert_eq!(
            vt100_color_to_ratatui(vt100::Color::Rgb(10, 20, 30)),
            Color::Rgb(10, 20, 30)
        );
    }

    // ── PtyWidget ───────────────────────────────────────────────────

    #[test]
    fn pty_widget_renders_text() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(b"Hello");

        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        PtyWidget {
            screen: parser.screen(),
        }
        .render(area, &mut buf);

        assert_eq!(buf[(0, 0)].symbol(), "H");
        assert_eq!(buf[(1, 0)].symbol(), "e");
        assert_eq!(buf[(2, 0)].symbol(), "l");
        assert_eq!(buf[(3, 0)].symbol(), "l");
        assert_eq!(buf[(4, 0)].symbol(), "o");
    }

    #[test]
    fn pty_widget_renders_colors() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process(b"\x1b[31mRed");

        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        PtyWidget {
            screen: parser.screen(),
        }
        .render(area, &mut buf);

        assert_eq!(buf[(0, 0)].symbol(), "R");
        assert_eq!(buf[(0, 0)].fg, Color::Indexed(1));
    }

    #[test]
    fn pty_widget_wide_chars() {
        let mut parser = vt100::Parser::new(24, 80, 0);
        parser.process("漢字".as_bytes());

        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        PtyWidget {
            screen: parser.screen(),
        }
        .render(area, &mut buf);

        // First char occupies cols 0-1, second occupies cols 2-3
        assert_eq!(buf[(0, 0)].symbol(), "漢");
        // Col 1 is wide continuation — should still be space (skipped)
        assert_eq!(buf[(2, 0)].symbol(), "字");
    }

    // ── key_to_bytes ────────────────────────────────────────────────

    #[test]
    fn key_to_bytes_printable() {
        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert_eq!(key_to_bytes(&key), Some(vec![0x61]));
    }

    #[test]
    fn key_to_bytes_ctrl_c() {
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(key_to_bytes(&key), Some(vec![0x03]));
    }

    #[test]
    fn key_to_bytes_arrows() {
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(key_to_bytes(&up), Some(b"\x1b[A".to_vec()));
        assert_eq!(key_to_bytes(&down), Some(b"\x1b[B".to_vec()));
        assert_eq!(key_to_bytes(&left), Some(b"\x1b[D".to_vec()));
        assert_eq!(key_to_bytes(&right), Some(b"\x1b[C".to_vec()));
    }

    #[test]
    fn key_to_bytes_enter_backspace() {
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let backspace = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(key_to_bytes(&enter), Some(b"\r".to_vec()));
        assert_eq!(key_to_bytes(&backspace), Some(vec![0x7f]));
    }

    #[test]
    fn key_to_bytes_crossterm_ctrl_bracket_range() {
        // Crossterm 0.28 maps bytes 0x1C-0x1F to Ctrl+'4'-'7'
        let ctrl4 = KeyEvent::new(KeyCode::Char('4'), KeyModifiers::CONTROL);
        let ctrl5 = KeyEvent::new(KeyCode::Char('5'), KeyModifiers::CONTROL);
        let ctrl6 = KeyEvent::new(KeyCode::Char('6'), KeyModifiers::CONTROL);
        let ctrl7 = KeyEvent::new(KeyCode::Char('7'), KeyModifiers::CONTROL);
        assert_eq!(key_to_bytes(&ctrl4), Some(vec![0x1C])); // Ctrl-\
        assert_eq!(key_to_bytes(&ctrl5), Some(vec![0x1D])); // Ctrl-]
        assert_eq!(key_to_bytes(&ctrl6), Some(vec![0x1E])); // Ctrl-^
        assert_eq!(key_to_bytes(&ctrl7), Some(vec![0x1F])); // Ctrl-_
    }

    #[test]
    fn detach_key_matches_crossterm_representation() {
        // Crossterm reports Ctrl-] as Char('5') + CONTROL
        let crossterm_ctrl_bracket = KeyEvent::new(KeyCode::Char('5'), KeyModifiers::CONTROL);
        assert!(is_detach_key(&crossterm_ctrl_bracket));
        // Also match the "ideal" representation in case a future crossterm fixes this
        let ideal_ctrl_bracket = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL);
        assert!(is_detach_key(&ideal_ctrl_bracket));
        // Regular keys should not match
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!is_detach_key(&ctrl_c));
        let plain_5 = KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE);
        assert!(!is_detach_key(&plain_5));
    }

    // ── Scrollback ───────────────────────────────────────────────────

    #[test]
    fn scrollback_preserves_history() {
        let mut parser = vt100::Parser::new(3, 10, 100);
        // Write 6 lines into a 3-row terminal → 3 lines should be in scrollback
        for i in 0..6 {
            parser.process(format!("line {}\r\n", i).as_bytes());
        }
        // At offset 0 (bottom), visible screen shows the most recent rows
        assert_eq!(parser.screen().scrollback(), 0);

        // Scroll up to see history
        parser.screen_mut().set_scrollback(3);
        let offset = parser.screen().scrollback();
        assert_eq!(offset, 3);

        // Row 0 of the viewport is now a scrollback row
        let cell = parser.screen().cell(0, 0).unwrap();
        let ch = cell.contents();
        // Should be one of the earlier lines (the exact content depends on
        // newline wrapping, but it must not be empty/blank)
        assert!(!ch.is_empty(), "scrollback row should have content");
    }

    #[test]
    fn scrollback_offset_clamps_to_max() {
        let mut parser = vt100::Parser::new(3, 10, 100);
        // Only 2 lines of output → at most ~0-1 scrollback rows
        parser.process(b"hello\r\nworld");
        parser.screen_mut().set_scrollback(9999);
        // Should clamp to the actual scrollback length
        let offset = parser.screen().scrollback();
        assert!(offset <= 2, "offset should be clamped, got {}", offset);
    }

    // ── StatusBarWidget ─────────────────────────────────────────────

    #[test]
    fn status_bar_renders() {
        let states = vec![
            ClaudeState::Initializing,
            ClaudeState::Starting,
            ClaudeState::Idle,
            ClaudeState::Thinking,
            ClaudeState::Responding,
            ClaudeState::ToolExecuting {
                tool: "Read".to_string(),
            },
            ClaudeState::WaitingForInput { prompt: None },
        ];
        let started = Instant::now();
        for state in states {
            let text = status_bar_text(&state, started);
            assert!(!text.is_empty(), "status bar text for {:?} is empty", state);

            let area = Rect::new(0, 0, 80, 1);
            let mut buf = Buffer::empty(area);
            StatusBarWidget { text: &text }.render(area, &mut buf);

            // Every cell should be reversed+bold
            let cell = &buf[(0, 0)];
            assert!(
                cell.modifier.contains(Modifier::REVERSED),
                "status bar cell should be reversed for {:?}",
                state
            );
            assert!(
                cell.modifier.contains(Modifier::BOLD),
                "status bar cell should be bold for {:?}",
                state
            );
        }
    }
}
