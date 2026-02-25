//! TUI panel for federation connections — list, switch context, disconnect.

use anyhow::Result;
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyCode, KeyEventKind},
    layout::Alignment,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Padding},
};
use serde::Deserialize;
use std::time::Duration;

use super::daemon::DaemonInfo;
use crate::interconnect::CrabCityContext;

/// Result of the connect panel interaction.
pub enum ConnectPanelResult {
    /// User chose a context to switch to.
    SwitchContext(CrabCityContext),
    /// User pressed Esc — no change.
    Back,
}

#[derive(Deserialize)]
struct FederationConnection {
    host_node_id: String,
    host_name: String,
    state: String,
    #[serde(default)]
    authenticated_users: Vec<String>,
}

fn hex_to_bytes_32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Run the federation connect panel. Blocks until the user picks or escapes.
pub fn run_connect_panel(
    terminal: &mut DefaultTerminal,
    daemon: &DaemonInfo,
    current_context: &CrabCityContext,
) -> Result<ConnectPanelResult> {
    let client = reqwest::blocking::Client::new();
    let connections = fetch_connections(&client, daemon)?;

    // Build the list: [0] = local, [1..] = remotes
    let mut contexts: Vec<CrabCityContext> = vec![CrabCityContext::Local];
    for conn in &connections {
        if let Some(node_id) = hex_to_bytes_32(&conn.host_node_id) {
            contexts.push(CrabCityContext::Remote {
                host_node_id: node_id,
                host_name: conn.host_name.clone(),
            });
        }
    }

    // Select the row matching current_context
    let initial = contexts
        .iter()
        .position(|c| c == current_context)
        .unwrap_or(0);
    let mut state = ListState::default().with_selected(Some(initial));

    loop {
        let total = contexts.len();

        terminal.draw(|frame| {
            let area = frame.area();
            let items = build_items(&contexts, &connections, current_context);

            let bottom_bar = Line::raw(" ↑↓ navigate · enter switch · esc back ");

            let list = List::new(items)
                .block(
                    Block::default()
                        .title(" crab: connections ")
                        .title(
                            Line::styled(
                                format!(" {} remote(s) ", connections.len()),
                                Style::default().add_modifier(Modifier::DIM),
                            )
                            .alignment(Alignment::Right),
                        )
                        .title_bottom(bottom_bar)
                        .borders(Borders::ALL)
                        .padding(Padding::horizontal(1)),
                )
                .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED))
                .highlight_symbol("▸ ");
            frame.render_stateful_widget(list, area, &mut state);
        })?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    return Ok(ConnectPanelResult::Back);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let i = state.selected().unwrap_or(0);
                    state.select(Some((i + 1) % total));
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    let i = state.selected().unwrap_or(0);
                    state.select(Some(if i == 0 { total - 1 } else { i - 1 }));
                }
                KeyCode::Enter => {
                    let i = state.selected().unwrap_or(0);
                    if let Some(ctx) = contexts.get(i) {
                        return Ok(ConnectPanelResult::SwitchContext(ctx.clone()));
                    }
                }
                _ => {}
            }
        }
    }
}

fn build_items<'a>(
    contexts: &[CrabCityContext],
    connections: &[FederationConnection],
    current: &CrabCityContext,
) -> Vec<ListItem<'a>> {
    contexts
        .iter()
        .map(|ctx| {
            let is_current = ctx == current;
            let marker = if is_current { "● " } else { "  " };

            match ctx {
                CrabCityContext::Local => {
                    let mut spans = vec![
                        Span::raw(marker.to_string()),
                        Span::styled(
                            format!("{:<24}", "Local"),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            "your instance",
                            Style::default().add_modifier(Modifier::DIM),
                        ),
                    ];
                    if is_current {
                        spans.push(Span::styled(
                            "  (active)",
                            Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC),
                        ));
                    }
                    ListItem::new(Line::from(spans))
                }
                CrabCityContext::Remote {
                    host_node_id,
                    host_name,
                } => {
                    // Find the connection info for status
                    let conn = connections.iter().find(|c| {
                        hex_to_bytes_32(&c.host_node_id).is_some_and(|id| &id == host_node_id)
                    });
                    let status = conn.map(|c| c.state.as_str()).unwrap_or("unknown");
                    let users = conn
                        .map(|c| c.authenticated_users.join(", "))
                        .unwrap_or_default();

                    let status_style = match status {
                        "connected" => Style::default().add_modifier(Modifier::BOLD),
                        "reconnecting" => {
                            Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC)
                        }
                        _ => Style::default().add_modifier(Modifier::DIM),
                    };

                    let mut spans = vec![
                        Span::raw(marker.to_string()),
                        Span::styled(
                            format!("{:<24}", host_name),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(format!("{:<14}", status), status_style),
                    ];
                    if !users.is_empty() {
                        spans.push(Span::styled(
                            users,
                            Style::default().add_modifier(Modifier::DIM),
                        ));
                    }
                    if is_current {
                        spans.push(Span::styled(
                            "  (active)",
                            Style::default().add_modifier(Modifier::DIM | Modifier::ITALIC),
                        ));
                    }
                    ListItem::new(Line::from(spans))
                }
            }
        })
        .collect()
}

fn fetch_connections(
    client: &reqwest::blocking::Client,
    daemon: &DaemonInfo,
) -> Result<Vec<FederationConnection>> {
    let url = format!("{}/api/federation/connections", daemon.base_url());
    let resp = client.get(&url).send()?;
    if resp.status().is_success() {
        Ok(resp.json()?)
    } else {
        Ok(vec![])
    }
}
