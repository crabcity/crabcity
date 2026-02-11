use anyhow::Result;
use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyCode, KeyEventKind},
    layout::Alignment,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Padding},
};
use std::sync::mpsc;
use std::time::Duration;

use super::InstanceInfo;

pub enum PickerResult {
    Attach(String),
    NewInstance,
    Kill(String),
    KillServer,
    Settings,
    Quit,
}

/// Events received from the multiplexed WebSocket.
pub enum PickerEvent {
    Created(InstanceInfo),
    Stopped(String),
}

/// Show an interactive TUI picker for instances.
/// Falls back to most-recent or NewInstance when stdin is not a TTY.
/// The caller owns the terminal (shared with settings screen).
pub fn run_picker(
    terminal: &mut Option<DefaultTerminal>,
    base_url: &str,
    instances: Vec<InstanceInfo>,
    events: mpsc::Receiver<PickerEvent>,
) -> Result<PickerResult> {
    match terminal {
        Some(term) => picker_loop(term, base_url, instances, events),
        None => {
            // No TTY — fall back to most recent instance or new
            Ok(match instances.last() {
                Some(inst) => PickerResult::Attach(inst.id.clone()),
                None => PickerResult::NewInstance,
            })
        }
    }
}

fn picker_loop(
    terminal: &mut DefaultTerminal,
    base_url: &str,
    mut instances: Vec<InstanceInfo>,
    events: mpsc::Receiver<PickerEvent>,
) -> Result<PickerResult> {
    let mut state = ListState::default().with_selected(Some(0));
    let mut confirming_kill_server = false;

    loop {
        // Drain any pending live-update events
        while let Ok(ev) = events.try_recv() {
            match ev {
                PickerEvent::Created(inst) => {
                    if !instances.iter().any(|i| i.id == inst.id) {
                        instances.push(inst);
                    }
                }
                PickerEvent::Stopped(id) => {
                    instances.retain(|i| i.id != id);
                }
            }
            // Keep selection in bounds
            let total = instances.len() + 1;
            if let Some(sel) = state.selected() {
                if sel >= total {
                    state.select(Some(total.saturating_sub(1)));
                }
            }
        }

        let total = instances.len() + 1;

        terminal.draw(|frame| {
            let area = frame.area();
            let items = build_items(&instances, area.width);

            let bottom_bar = if confirming_kill_server {
                let running = instances.iter().filter(|i| i.running).count();
                Line::from(vec![
                    Span::styled(
                        format!(
                            " Kill server and {} session{}? ",
                            running,
                            if running == 1 { "" } else { "s" }
                        ),
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("y to confirm · any key to cancel "),
                ])
            } else {
                Line::raw(" ↑↓ navigate · enter select · x kill · s settings · Q kill server · q/esc quit ")
            };

            let list = List::new(items)
                .block(
                    Block::default()
                        .title(" crab: select session ")
                        .title(
                            Line::styled(
                                format!(" {} ", base_url),
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

        // Poll with a short timeout so we can process WS events between frames
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            if confirming_kill_server {
                confirming_kill_server = false;
                if key.code == KeyCode::Char('y') {
                    return Ok(PickerResult::KillServer);
                }
                continue;
            }

            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(PickerResult::Quit),
                KeyCode::Char('Q') => {
                    confirming_kill_server = true;
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
                    return if i < instances.len() {
                        Ok(PickerResult::Attach(instances[i].id.clone()))
                    } else {
                        Ok(PickerResult::NewInstance)
                    };
                }
                KeyCode::Char('x') => {
                    let i = state.selected().unwrap_or(0);
                    if i < instances.len() {
                        return Ok(PickerResult::Kill(instances[i].id.clone()));
                    }
                }
                KeyCode::Char('s') => {
                    return Ok(PickerResult::Settings);
                }
                _ => {}
            }
        }
    }
}

fn build_items(instances: &[InstanceInfo], _width: u16) -> Vec<ListItem<'static>> {
    let mut items: Vec<ListItem> = instances
        .iter()
        .map(|inst| {
            let status = if inst.running { "running" } else { "stopped" };
            let short_id = if inst.id.len() > 8 {
                &inst.id[..8]
            } else {
                &inst.id
            };
            let line = Line::from(vec![
                Span::styled(
                    format!("{:<20}", inst.name),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" {:<10}", short_id)),
                Span::styled(
                    format!(" {:<8}", status),
                    if inst.running {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().add_modifier(Modifier::DIM)
                    },
                ),
                Span::raw(format!(" {}", inst.working_dir)),
            ]);
            ListItem::new(line)
        })
        .collect();

    items.push(ListItem::new(Line::from(vec![Span::styled(
        "[ + ]  New instance",
        Style::default().add_modifier(Modifier::BOLD),
    )])));

    items
}
