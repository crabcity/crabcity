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

#[derive(Deserialize)]
struct RemoteEntry {
    host_node_id: String,
    host_name: String,
    #[allow(dead_code)]
    granted_access: String,
    status: String,
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
    let remotes = fetch_remotes(&client, daemon)?;

    // Build the list: [0] = local, [1..] = remotes from saved entries (with live status)
    let mut contexts: Vec<CrabCityContext> = vec![CrabCityContext::Local];
    let mut statuses: Vec<String> = vec!["local".into()];
    for r in &remotes {
        if let Some(node_id) = hex_to_bytes_32(&r.host_node_id) {
            contexts.push(CrabCityContext::Remote {
                host_node_id: node_id,
                host_name: r.host_name.clone(),
            });
            statuses.push(r.status.clone());
        }
    }
    // Also include any live connections not in saved remotes
    for conn in &connections {
        let already = remotes.iter().any(|r| r.host_node_id == conn.host_node_id);
        if !already {
            if let Some(node_id) = hex_to_bytes_32(&conn.host_node_id) {
                contexts.push(CrabCityContext::Remote {
                    host_node_id: node_id,
                    host_name: conn.host_name.clone(),
                });
                statuses.push(conn.state.clone());
            }
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
            let items = build_items(&contexts, &statuses, &connections, current_context);

            let bottom_bar = Line::raw(" ↑↓ navigate · enter switch · esc back ");

            let list = List::new(items)
                .block(
                    Block::default()
                        .title(" crab: connections ")
                        .title(
                            Line::styled(
                                format!(" {} remote(s) ", contexts.len() - 1),
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
                        // If selecting a disconnected remote, trigger connect first
                        if let CrabCityContext::Remote { host_node_id, .. } = ctx {
                            let status = statuses.get(i).map(|s| s.as_str()).unwrap_or("disconnected");
                            if status != "connected" {
                                let hex: String = host_node_id.iter().map(|b| format!("{b:02x}")).collect();
                                let _ = trigger_connect(&client, daemon, &hex);
                            }
                        }
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
    statuses: &[String],
    connections: &[FederationConnection],
    current: &CrabCityContext,
) -> Vec<ListItem<'a>> {
    contexts
        .iter()
        .enumerate()
        .map(|(idx, ctx)| {
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
                    let status = statuses.get(idx).map(|s| s.as_str()).unwrap_or("unknown");
                    let users = connections
                        .iter()
                        .find(|c| {
                            hex_to_bytes_32(&c.host_node_id)
                                .is_some_and(|id| &id == host_node_id)
                        })
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

fn fetch_remotes(
    client: &reqwest::blocking::Client,
    daemon: &DaemonInfo,
) -> Result<Vec<RemoteEntry>> {
    let url = format!("{}/api/remotes", daemon.base_url());
    let resp = client.get(&url).send()?;
    if resp.status().is_success() {
        Ok(resp.json()?)
    } else {
        Ok(vec![])
    }
}

fn trigger_connect(
    client: &reqwest::blocking::Client,
    daemon: &DaemonInfo,
    host_node_id_hex: &str,
) -> Result<()> {
    let url = format!("{}/api/remotes/connect", daemon.base_url());
    let _ = client
        .post(&url)
        .json(&serde_json::json!({ "host_node_id": host_node_id_hex }))
        .send()?;
    Ok(())
}
