use axum::{
    extract::State,
    response::{Html, IntoResponse},
};
use maud::{DOCTYPE, PreEscaped, html};

use super::CSS;
use crate::{AppState, terminal};

pub async fn index_page(State(state): State<AppState>) -> impl IntoResponse {
    let instances = state.instance_manager.list().await;

    let markup = html! {
        (DOCTYPE)
        html {
            head {
                title { "Crab City - Claude Code Manager" }
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/xterm@5.3.0/css/xterm.css";
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
                    (super::sidebar(&instances, "terminal"))

                    // Main content area
                    div class="flex-1 flex flex-col bg-gray-900 min-w-0" {
                        // Terminal View
                        div id="terminal-view" class="flex flex-col h-full" {
                            // Terminal header
                            div class="bg-gray-800 border-b border-gray-700 px-4 py-3 flex items-center justify-between flex-shrink-0" {
                                div class="font-medium truncate" id="terminal-title" {
                                    "Select an instance to connect"
                                }
                                div class="flex items-center gap-2 flex-shrink-0" {
                                    input type="text" id="instance-command-edit"
                                        class="hidden px-2 py-1 bg-gray-700 text-sm rounded border border-gray-600 focus:border-crab-accent focus:outline-none"
                                        placeholder="Command to run";
                                    button id="restart-btn" class="hidden px-3 py-1 bg-gray-700 hover:bg-gray-600 rounded text-sm transition-colors" title="Restart with new command" {
                                        "🔄 Restart"
                                    }
                                    button id="refresh-btn" class="hidden px-3 py-1 bg-gray-700 hover:bg-gray-600 rounded text-sm transition-colors" title="Refresh terminal output" {
                                        "♻️ Refresh"
                                    }
                                    button id="toggle-drawer-btn" class="hidden px-3 py-1 bg-gray-700 hover:bg-gray-600 rounded text-sm transition-colors" title="Toggle conversation view" {
                                        "💬 Context"
                                    }
                                }
                                div class="text-sm text-gray-400" id="terminal-info" {}
                            }
                            div class="flex flex-1 min-h-0 relative" {
                                div class="flex-1 flex flex-col bg-black min-w-0" id="terminal-container" {
                                    div class="flex-1 flex items-center justify-center text-gray-400" id="empty-state" {
                                        div class="text-center" {
                                            h2 class="text-2xl font-bold mb-2" { "No Instance Connected" }
                                            p { "Create a new instance or select one from the sidebar" }
                                        }
                                    }
                                    div id="terminal" style="display: none; height: 100%;" {}
                                }
                                div class="bg-gray-900 border-l border-gray-700 flex flex-col flex-shrink-0 relative" id="conversation-drawer" style="display: none; width: 400px;" {
                                    div class="absolute -left-1 top-0 bottom-0 w-2 cursor-ew-resize hover:bg-blue-400 transition-colors z-10" id="drawer-resize-handle" style="background-color: rgba(59, 130, 246, 0.2);" {}
                                    div class="flex items-center justify-between p-4 border-b border-gray-700 flex-shrink-0" {
                                        h3 class="text-lg font-semibold" { "📝 Conversation Context" }
                                        button class="text-2xl text-gray-400 hover:text-white transition-colors" id="drawer-close-btn" {
                                            "×"
                                        }
                                    }
                                    div class="flex-1 p-4 overflow-y-auto" id="drawer-content" {
                                        div class="text-gray-500 text-center" {
                                            p { "Conversation structure will appear here when using Claude" }
                                        }
                                    }
                                }
                            }
                        }

                        // Welcome message (hidden when terminal is active)
                        div id="welcome-message" class="hidden h-full flex items-center justify-center" {
                            div class="text-center" {
                                h2 class="text-2xl font-bold mb-4" { "Welcome to Crab City! 🦀" }
                                p class="text-gray-400 mb-6" { "Select an instance from the sidebar or create a new one" }
                                button id="create-first-btn" class="px-6 py-3 bg-crab-accent text-white rounded-lg hover:bg-blue-600 transition-colors" {
                                    "Create Your First Instance"
                                }
                            }
                        }
                    }
                }

                // Scripts
                script src="https://cdn.jsdelivr.net/npm/xterm@5.3.0/lib/xterm.js" {}
                script src="https://cdn.jsdelivr.net/npm/xterm-addon-fit@0.8.0/lib/xterm-addon-fit.js" {}
                script src="https://cdn.jsdelivr.net/npm/xterm-addon-web-links@0.9.0/lib/xterm-addon-web-links.js" {}
                script src="https://cdn.jsdelivr.net/npm/xterm-addon-serialize@0.11.0/lib/xterm-addon-serialize.js" {}
                script { (PreEscaped(terminal::JAVASCRIPT)) }
            }
        }
    };

    Html(markup.into_string())
}
