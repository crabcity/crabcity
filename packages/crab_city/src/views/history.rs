use axum::{
    extract::{Query, State},
    response::{Html, IntoResponse},
};
use claude_convo::ClaudeConvo;
use maud::{DOCTYPE, PreEscaped, html};
use serde::Deserialize;
use std::path::PathBuf;
use tracing::{debug, error, info};

use super::CSS;
use crate::{AppState, models};

#[derive(Deserialize)]
pub struct Pagination {
    #[serde(default = "default_page")]
    page: usize,
    #[serde(default = "default_per_page")]
    per_page: usize,
}

fn default_page() -> usize {
    1
}
fn default_per_page() -> usize {
    20
}

pub async fn history_page(
    State(state): State<AppState>,
    Query(pagination): Query<Pagination>,
) -> impl IntoResponse {
    // Quick synchronous page load - only get session IDs, not full metadata
    let claude_convo = ClaudeConvo::new();
    let project_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let session_ids = claude_convo
        .list_conversations(&project_path.to_string_lossy())
        .unwrap_or_else(|e| {
            error!("Failed to list conversations: {}", e);
            vec![]
        });

    let total_count = session_ids.len();
    let page = pagination.page.max(1);
    let per_page = pagination.per_page.clamp(5, 50); // Limit between 5 and 50 items per page

    // Calculate pagination
    let start_idx = (page - 1) * per_page;
    let end_idx = (start_idx + per_page).min(total_count);
    let total_pages = (total_count + per_page - 1) / per_page;

    info!(
        "Loading conversations page {} of {} (showing {}-{} of {})",
        page,
        total_pages,
        start_idx + 1,
        end_idx,
        total_count
    );

    // Load metadata only for the current page
    let mut conversations = Vec::new();
    for session_id in session_ids.iter().skip(start_idx).take(per_page) {
        match claude_convo.read_conversation_metadata(&project_path.to_string_lossy(), session_id) {
            Ok(metadata) => {
                // Try to get a meaningful title from the first user message
                let title = match claude_convo
                    .read_conversation(&project_path.to_string_lossy(), session_id)
                {
                    Ok(conv) => {
                        // Find first user message for title
                        conv.entries
                            .iter()
                            .find_map(|e| {
                                e.message.as_ref().and_then(|msg| {
                                    if matches!(msg.role, claude_convo::MessageRole::User) {
                                        msg.content.as_ref().map(|content| {
                                            let text = match content {
                                                claude_convo::MessageContent::Text(t) => t.clone(),
                                                claude_convo::MessageContent::Parts(parts) => parts
                                                    .iter()
                                                    .filter_map(|p| match p {
                                                        claude_convo::ContentPart::Text {
                                                            text,
                                                        } => Some(text.clone()),
                                                        _ => None,
                                                    })
                                                    .collect::<Vec<_>>()
                                                    .join(" "),
                                            };
                                            // Clean up and truncate for title
                                            let clean_text = text
                                                .lines()
                                                .next()
                                                .unwrap_or(&text)
                                                .trim()
                                                .chars()
                                                .take(80)
                                                .collect::<String>();
                                            if clean_text.len() >= 80 {
                                                format!("{}...", clean_text)
                                            } else {
                                                clean_text
                                            }
                                        })
                                    } else {
                                        None
                                    }
                                })
                            })
                            .or_else(|| {
                                Some(format!(
                                    "Session {}",
                                    &session_id[..8.min(session_id.len())]
                                ))
                            })
                    }
                    Err(_) => Some(format!(
                        "Session {}",
                        &session_id[..8.min(session_id.len())]
                    )),
                };

                // Convert to ConversationSummary for display
                let summary = models::ConversationSummary {
                    id: session_id.clone(),
                    title,
                    instance_id: project_path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                    created_at: metadata.started_at.map(|dt| dt.timestamp()).unwrap_or(0),
                    updated_at: metadata.last_activity.map(|dt| dt.timestamp()).unwrap_or(0),
                    entry_count: metadata.message_count as i32,
                    is_public: false,
                };
                conversations.push(summary);
            }
            Err(e) => {
                debug!("Failed to read metadata for {}: {}", session_id, e);
            }
        }
    }

    // Sort by updated_at descending
    conversations.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    // Get instances for sidebar
    let instances = state.instance_manager.list().await;

    let markup = html! {
        (DOCTYPE)
        html {
            head {
                title { "Conversation History - Crab City" }
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                script src="https://cdn.tailwindcss.com" {}
                script { (PreEscaped(r#"
                    tailwind.config = {
                        theme: {
                            extend: {
                                colors: {
                                    'crab-dark': '#0a0e1a',
                                    'crab-navy': '#16213e',
                                    'crab-blue': '#0f3460',
                                    'crab-accent': '#4299e1',
                                }
                            }
                        }
                    }
                "#)) }
                style { (PreEscaped(CSS)) }
            }
            body class="bg-gray-900 text-gray-200 h-screen overflow-hidden" {
                div class="flex h-screen" {
                    // Use shared sidebar
                    (super::sidebar(&instances, "history"))

                    // Main content area
                    div class="flex-1 overflow-auto" {
                        div class="max-w-6xl mx-auto p-8" {
                            h2 class="text-2xl font-bold mb-6" { "Conversation History" }

                            @if conversations.is_empty() && page == 1 {
                                div class="bg-gray-800 rounded-lg p-6 text-center text-gray-400" {
                                    "No conversations found. Start a Claude conversation to see it here!"
                                }
                            } @else {
                                // Pagination info
                                div class="flex justify-between items-center mb-4" {
                                    p class="text-sm text-gray-400" {
                                        "Showing " (start_idx + 1) "-" (end_idx) " of " (total_count) " conversations"
                                    }
                                    div class="flex gap-2" {
                                        @if page > 1 {
                                            a href=(format!("/history?page={}&per_page={}", page - 1, per_page))
                                              class="px-3 py-1 bg-gray-700 text-gray-300 rounded hover:bg-gray-600 text-sm" {
                                                "← Previous"
                                            }
                                        }
                                        span class="px-3 py-1 text-sm text-gray-400" {
                                            "Page " (page) " of " (total_pages)
                                        }
                                        @if page < total_pages {
                                            a href=(format!("/history?page={}&per_page={}", page + 1, per_page))
                                              class="px-3 py-1 bg-gray-700 text-gray-300 rounded hover:bg-gray-600 text-sm" {
                                                "Next →"
                                            }
                                        }
                                    }
                                }

                                div class="space-y-4" {
                                    @for conv in &conversations {
                                        a href=(format!("/conversation/{}", conv.id))
                                          class="block bg-gray-800 rounded-lg p-4 hover:bg-gray-700 transition-colors border border-gray-700 hover:border-crab-accent" {
                                            div class="flex justify-between items-start mb-3" {
                                                h3 class="text-base font-medium text-gray-100 leading-relaxed flex-1 mr-4" {
                                                    (conv.title.as_deref().unwrap_or("Untitled Conversation"))
                                                }
                                                @if conv.is_public {
                                                    span class="px-2 py-1 bg-green-900 text-green-300 text-xs rounded flex-shrink-0" { "Public" }
                                                }
                                            }
                                            div class="flex items-center gap-4 text-xs text-gray-500" {
                                                span {
                                                    "📅 " (chrono::DateTime::from_timestamp(conv.created_at, 0)
                                                        .map(|dt| dt.format("%b %d, %H:%M").to_string())
                                                        .unwrap_or_else(|| "Unknown".to_string()))
                                                }
                                                span {
                                                    "💬 " (conv.entry_count) " messages"
                                                }
                                                span class="text-gray-600" {
                                                    "ID: " (&conv.id[..8.min(conv.id.len())]) "..."
                                                }
                                            }
                                        }
                                    }
                                }

                                // Bottom pagination controls
                                @if total_pages > 1 {
                                    div class="mt-6 flex justify-center items-center gap-2" {
                                        @if page > 1 {
                                            a href="/history?page=1"
                                              class="px-3 py-1 bg-gray-700 text-gray-300 rounded hover:bg-gray-600 text-sm" {
                                                "First"
                                            }
                                            a href=(format!("/history?page={}&per_page={}", page - 1, per_page))
                                              class="px-3 py-1 bg-gray-700 text-gray-300 rounded hover:bg-gray-600 text-sm" {
                                                "←"
                                            }
                                        }

                                        // Page numbers
                                        @for p in ((page as i32 - 2).max(1) as usize)..=((page + 2).min(total_pages)) {
                                            @if p == page {
                                                span class="px-3 py-1 bg-crab-accent text-white rounded text-sm" {
                                                    (p)
                                                }
                                            } @else {
                                                a href=(format!("/history?page={}&per_page={}", p, per_page))
                                                  class="px-3 py-1 bg-gray-700 text-gray-300 rounded hover:bg-gray-600 text-sm" {
                                                    (p)
                                                }
                                            }
                                        }

                                        @if page < total_pages {
                                            a href=(format!("/history?page={}&per_page={}", page + 1, per_page))
                                              class="px-3 py-1 bg-gray-700 text-gray-300 rounded hover:bg-gray-600 text-sm" {
                                                "→"
                                            }
                                            a href=(format!("/history?page={}&per_page={}", total_pages, per_page))
                                              class="px-3 py-1 bg-gray-700 text-gray-300 rounded hover:bg-gray-600 text-sm" {
                                                "Last"
                                            }
                                        }
                                    }
                                }

                                // Per-page selector
                                div class="mt-4 text-center" {
                                    label class="text-sm text-gray-400 mr-2" { "Items per page:" }
                                    @for size in &[10, 20, 30, 50] {
                                        @if *size == per_page {
                                            span class="px-2 py-1 bg-crab-accent text-white rounded text-sm mx-1" {
                                                (size)
                                            }
                                        } @else {
                                            a href=(format!("/history?page=1&per_page={}", size))
                                              class="px-2 py-1 bg-gray-700 text-gray-300 rounded hover:bg-gray-600 text-sm mx-1" {
                                                (size)
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    };

    Html(markup.into_string())
}
