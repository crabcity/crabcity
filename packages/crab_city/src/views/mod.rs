mod conversation;
mod history;
mod index;
mod settings;

pub use conversation::conversation_detail_page;
pub use history::history_page;
pub use index::index_page;
pub use settings::settings_page;

use crate::instance_manager::ClaudeInstance;
use maud::{Markup, html};

// Shared sidebar component
pub fn sidebar(instances: &[ClaudeInstance], current_page: &str) -> Markup {
    html! {
        div class="w-64 bg-gray-800 border-r border-gray-700 flex flex-col flex-shrink-0" {
            // Header
            div class="p-4 border-b border-gray-700" {
                h1 class="text-xl font-bold flex items-center gap-2" {
                    "🦀 Crab City"
                }
                p class="text-sm text-gray-400 mt-1" { "Claude Code Manager" }
            }

            // Navigation tabs
            div class="flex gap-2 p-4" {
                a href="/" class=(if current_page == "terminal" {
                    "flex-1 px-3 py-2 rounded bg-crab-accent text-white text-sm font-medium transition-colors text-center"
                } else {
                    "flex-1 px-3 py-2 rounded border border-gray-700 text-gray-400 hover:bg-gray-700 hover:text-gray-200 text-sm font-medium transition-colors text-center"
                }) {
                    "🖥️ Terminal"
                }
                a href="/settings" class=(if current_page == "settings" {
                    "flex-1 px-3 py-2 rounded bg-crab-accent text-white text-sm font-medium transition-colors text-center"
                } else {
                    "flex-1 px-3 py-2 rounded border border-gray-700 text-gray-400 hover:bg-gray-700 hover:text-gray-200 text-sm font-medium transition-colors text-center"
                }) {
                    "⚙️ Settings"
                }
                a href="/history" class=(if current_page == "history" {
                    "flex-1 px-3 py-2 rounded bg-crab-accent text-white text-sm font-medium transition-colors text-center"
                } else {
                    "flex-1 px-3 py-2 rounded border border-gray-700 text-gray-400 hover:bg-gray-700 hover:text-gray-200 text-sm font-medium transition-colors text-center"
                }) {
                    "📜 History"
                }
            }

            // New instance button
            button id="new-instance-btn" class="mx-4 px-4 py-2 bg-crab-accent text-white rounded font-medium hover:bg-blue-600 transition-colors" {
                "✨ New Instance"
            }

            // Instance list
            div class="flex-1 overflow-y-auto p-4 space-y-2" id="instance-list" {
                @for instance in instances {
                    div class="instance-card bg-gray-700 rounded p-3 cursor-pointer hover:bg-gray-600 transition-colors"
                        data-instance-id=(instance.id) data-port=(instance.wrapper_port) {
                        div class="flex items-center justify-between mb-1" {
                            span class="font-medium" {
                                @if !instance.name.is_empty() {
                                    (&instance.name)
                                } @else {
                                    (instance.id[0..8.min(instance.id.len())]) "..."
                                }
                            }
                            span class=(if instance.running { "text-green-500" } else { "text-gray-500" }) {
                                @if instance.running { "●" } @else { "○" }
                            }
                        }
                        div class="text-xs text-gray-400" {
                            (&instance.command)
                        }
                        button class="delete-btn mt-2 text-xs text-red-400 hover:text-red-300"
                            data-instance-id=(instance.id) {
                            "🗑️ Delete"
                        }
                    }
                }
            }
        }
    }
}

// Shared CSS constant
pub const CSS: &str = r#"
    .terminal {
        font-family: 'SF Mono', Monaco, 'Cascadia Code', 'Roboto Mono', monospace;
    }

    @keyframes blink {
        0%, 49% { opacity: 1; }
        50%, 100% { opacity: 0; }
    }

    .cursor::after {
        content: '▋';
        display: inline-block;
        animation: blink 1s infinite;
    }

    .instance-card {
        transition: all 0.3s ease;
    }

    .instance-card:hover {
        transform: translateY(-2px);
    }

    .fade-in {
        animation: fadeIn 0.3s ease-in;
    }

    @keyframes fadeIn {
        from { opacity: 0; transform: translateY(10px); }
        to { opacity: 1; transform: translateY(0); }
    }

    ::-webkit-scrollbar {
        width: 8px;
        height: 8px;
    }

    ::-webkit-scrollbar-track {
        background: #1f2937;
    }

    ::-webkit-scrollbar-thumb {
        background: #4b5563;
        border-radius: 4px;
    }

    ::-webkit-scrollbar-thumb:hover {
        background: #6b7280;
    }

    /* Conversation drawer styles */
    .notebook {
        display: flex;
        flex-direction: column;
        gap: 1rem;
        padding: 0.5rem;
    }

    .cell {
        border-radius: 0.5rem;
        padding: 0.75rem;
        background: rgba(255, 255, 255, 0.03);
        border: 1px solid rgba(255, 255, 255, 0.1);
        transition: background-color 0.2s;
    }

    .cell:hover {
        background: rgba(255, 255, 255, 0.05);
    }

    .user-cell {
        border-left: 3px solid #4299e1;
        background: rgba(66, 153, 225, 0.05);
    }

    .assistant-cell {
        border-left: 3px solid #48bb78;
        background: rgba(72, 187, 120, 0.05);
    }

    .cell-header {
        display: flex;
        justify-content: space-between;
        align-items: center;
        margin-bottom: 0.5rem;
        padding-bottom: 0.5rem;
        border-bottom: 1px solid rgba(255, 255, 255, 0.1);
    }

    .cell-type {
        font-weight: 600;
        font-size: 0.875rem;
        padding: 0.125rem 0.5rem;
        border-radius: 0.25rem;
    }

    .cell-type.user {
        color: #4299e1;
        background: rgba(66, 153, 225, 0.2);
    }

    .cell-type.assistant {
        color: #48bb78;
        background: rgba(72, 187, 120, 0.2);
    }

    .cell-meta {
        font-size: 0.75rem;
        color: #9ca3af;
    }

    .cell-content {
        font-size: 0.875rem;
        line-height: 1.5;
        color: #e5e7eb;
        word-wrap: break-word;
        overflow-wrap: break-word;
        white-space: pre-wrap;
    }

    .cell-tools {
        margin-top: 0.5rem;
        display: flex;
        flex-wrap: wrap;
        gap: 0.25rem;
    }

    .tool-badge {
        display: inline-block;
        font-size: 0.75rem;
        padding: 0.125rem 0.375rem;
        background: rgba(139, 92, 246, 0.2);
        color: #a78bfa;
        border-radius: 0.25rem;
        border: 1px solid rgba(139, 92, 246, 0.3);
    }
"#;
