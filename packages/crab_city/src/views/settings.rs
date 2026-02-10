use axum::{
    extract::State,
    response::{Html, IntoResponse},
};
use maud::{DOCTYPE, PreEscaped, html};

use super::CSS;
use crate::AppState;

pub async fn settings_page(State(state): State<AppState>) -> impl IntoResponse {
    let instances = state.instance_manager.list().await;
    let total_instances = instances.len();
    let active_instances = instances.iter().filter(|i| i.running).count();

    let markup = html! {
        (DOCTYPE)
        html {
            head {
                title { "Settings - Crab City" }
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1.0";
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
                    (super::sidebar(&instances, "settings"))

                    // Main content area
                    div class="flex-1 overflow-auto" {
                        div class="max-w-2xl mx-auto p-8" {
                            h2 class="text-2xl font-bold mb-6" { "Settings" }

                            // Statistics Card
                            div class="bg-gray-800 rounded-lg p-6 mb-6" {
                                h3 class="text-lg font-semibold mb-4" { "Instance Statistics" }
                                div class="grid grid-cols-2 gap-4" {
                                    div {
                                        p class="text-gray-400 text-sm" { "Total Instances" }
                                        p class="text-2xl font-bold text-crab-accent" { (total_instances) }
                                    }
                                    div {
                                        p class="text-gray-400 text-sm" { "Active Instances" }
                                        p class="text-2xl font-bold text-green-500" { (active_instances) }
                                    }
                                }
                            }

                            // Settings Form
                            div class="bg-gray-800 rounded-lg p-6 mb-6" {
                                h3 class="text-lg font-semibold mb-4" { "Default Settings" }

                                div class="mb-4" {
                                    label class="block text-sm font-medium mb-2" { "Default Command" }
                                    input type="text" id="default-command-input"
                                        class="w-full px-3 py-2 bg-gray-700 border border-gray-600 rounded focus:border-crab-accent focus:outline-none"
                                        placeholder="bash" {}
                                    p class="text-xs text-gray-400 mt-1" { "Command to run when creating new instances" }
                                }

                                button id="save-settings-btn"
                                    class="px-4 py-2 bg-crab-accent text-white rounded hover:bg-blue-600 transition-colors" {
                                    "Save Settings"
                                }
                            }

                            // Danger Zone
                            div class="bg-red-900/20 border border-red-600 rounded-lg p-6" {
                                h3 class="text-lg font-semibold mb-4 text-red-400" { "Danger Zone" }

                                div class="mb-4" {
                                    p class="text-sm text-gray-300 mb-3" {
                                        "Clear all saved terminal sessions from browser storage"
                                    }
                                    button id="clear-storage-btn"
                                        class="px-4 py-2 bg-red-600 text-white rounded hover:bg-red-700 transition-colors" {
                                        "Clear Storage"
                                    }
                                }

                                div {
                                    p class="text-sm text-gray-300 mb-1" { "Storage used: " }
                                    p id="storage-size" class="text-xs text-gray-400" { "Calculating..." }
                                }
                            }
                        }
                    }
                }

                script { (PreEscaped(r#"
                    // Load saved settings
                    const defaultCommand = localStorage.getItem('crab_city_default_command') || 'bash';
                    document.getElementById('default-command-input').value = defaultCommand;

                    // Save settings
                    document.getElementById('save-settings-btn').addEventListener('click', () => {
                        const defaultCommand = document.getElementById('default-command-input').value || 'bash';
                        localStorage.setItem('crab_city_default_command', defaultCommand);
                        alert('Settings saved!');
                    });

                    // Calculate storage
                    let storageUsed = 0;
                    try {
                        for (let i = 0; i < localStorage.length; i++) {
                            const key = localStorage.key(i);
                            if (key && key.startsWith('terminal_')) {
                                const value = localStorage.getItem(key);
                                storageUsed += value ? value.length : 0;
                            }
                        }
                    } catch (e) {}

                    const formatBytes = (bytes) => {
                        if (bytes === 0) return '0 B';
                        const k = 1024;
                        const sizes = ['B', 'KB', 'MB'];
                        const i = Math.floor(Math.log(bytes) / Math.log(k));
                        return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
                    };

                    document.getElementById('storage-size').textContent = formatBytes(storageUsed);

                    // Clear storage
                    document.getElementById('clear-storage-btn').addEventListener('click', () => {
                        if (confirm('This will clear all saved terminal sessions. Are you sure?')) {
                            const keys = [];
                            for (let i = 0; i < localStorage.length; i++) {
                                const key = localStorage.key(i);
                                if (key && key.startsWith('terminal_')) {
                                    keys.push(key);
                                }
                            }
                            keys.forEach(key => localStorage.removeItem(key));
                            alert('Storage cleared!');
                            document.getElementById('storage-size').textContent = '0 B';
                        }
                    });
                "#)) }
            }
        }
    };

    Html(markup.into_string())
}
