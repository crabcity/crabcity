use axum::{
    extract::{Path, State},
    response::{Html, IntoResponse},
};
use maud::{DOCTYPE, PreEscaped, html};
use std::path::PathBuf;
use toolpath_claude::{ClaudeConvo, ContentPart, MessageContent, MessageRole};
use tracing::error;

use super::CSS;
use crate::AppState;

pub async fn conversation_detail_page(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    // Load conversation directly from claude_convo
    let claude_convo = ClaudeConvo::new();
    let project_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Read the conversation
    let conversation =
        match claude_convo.read_conversation(&project_path.to_string_lossy(), &session_id) {
            Ok(conv) => conv,
            Err(e) => {
                error!("Failed to read conversation {}: {}", session_id, e);
                return Html("<h1>Conversation not found</h1>".to_string());
            }
        };

    // Read metadata for timestamps
    let metadata = claude_convo
        .read_conversation_metadata(&project_path.to_string_lossy(), &session_id)
        .ok();

    // Load notes for this conversation
    let notes = state.notes_storage.get_notes(&session_id).await;

    // Extract title from first user message
    let title = conversation
        .entries
        .iter()
        .find_map(|e| {
            e.message.as_ref().and_then(|msg| {
                if matches!(msg.role, MessageRole::User) {
                    msg.content.as_ref().map(|content| match content {
                        MessageContent::Text(text) => text.chars().take(100).collect::<String>(),
                        MessageContent::Parts(parts) => parts
                            .iter()
                            .find_map(|p| match p {
                                ContentPart::Text { text } => Some(text),
                                _ => None,
                            })
                            .map(|t| t.chars().take(100).collect::<String>())
                            .unwrap_or_else(|| {
                                format!("Session {}", &session_id[..8.min(session_id.len())])
                            }),
                    })
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| format!("Session {}", &session_id[..8.min(session_id.len())]));

    let markup = html! {
        (DOCTYPE)
        html {
            head {
                title {
                    (&title)
                    " - Crab City"
                }
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
                style { (PreEscaped(r#"
                    .notes-section {
                        position: sticky;
                        top: 1rem;
                        max-height: calc(100vh - 2rem);
                        overflow-y: auto;
                    }
                    .note-item {
                        transition: all 0.2s ease;
                    }
                    .note-item:hover {
                        transform: translateX(2px);
                    }
                    .note-editor {
                        display: none;
                        animation: slideDown 0.2s ease-out;
                    }
                    .note-editor.active {
                        display: block;
                    }
                    @keyframes slideDown {
                        from {
                            opacity: 0;
                            transform: translateY(-10px);
                        }
                        to {
                            opacity: 1;
                            transform: translateY(0);
                        }
                    }
                    .modal-overlay {
                        display: none;
                        position: fixed;
                        top: 0;
                        left: 0;
                        right: 0;
                        bottom: 0;
                        background: rgba(0, 0, 0, 0.5);
                        z-index: 1000;
                        align-items: center;
                        justify-content: center;
                    }
                    .modal-overlay.active {
                        display: flex;
                    }
                    .modal-content {
                        background: #1f2937;
                        border-radius: 0.5rem;
                        padding: 1.5rem;
                        max-width: 500px;
                        width: 90%;
                        animation: modalFadeIn 0.2s ease-out;
                    }
                    @keyframes modalFadeIn {
                        from {
                            opacity: 0;
                            transform: scale(0.95);
                        }
                        to {
                            opacity: 1;
                            transform: scale(1);
                        }
                    }
                "#)) }
            }
            body class="bg-gray-900 text-gray-200" {
                div class="max-w-7xl mx-auto p-8" {
                    // Header with back button
                    div class="mb-6" {
                        a href="/history" class="inline-flex items-center text-crab-accent hover:underline mb-4" {
                            "← Back to History"
                        }
                        h1 class="text-3xl font-bold" {
                            (&title)
                        }
                        div class="flex gap-4 mt-2 text-sm text-gray-400" {
                            span { "Session: " (&session_id[..8.min(session_id.len())]) "..." }
                            @if let Some(ref meta) = metadata {
                                @if let Some(started) = meta.started_at {
                                    span {
                                        "Started: "
                                        (started.format("%Y-%m-%d %H:%M").to_string())
                                    }
                                }
                            }
                        }
                    }

                    // Two column layout: conversation + notes
                    div class="flex gap-6" {
                        // Conversation entries column
                        div class="flex-1 space-y-4" {
                        @for entry in &conversation.entries {
                            @if let Some(msg) = &entry.message {
                                div class=(format!("rounded-lg p-4 relative group {}",
                                    if matches!(msg.role, MessageRole::User) { "bg-blue-900" }
                                    else if matches!(msg.role, MessageRole::Assistant) { "bg-gray-800" }
                                    else { "bg-gray-700" })) data-entry-id=(&entry.uuid) {

                                    // Count notes for this entry
                                    @let entry_notes = notes.iter().filter(|n| n.entry_id.as_ref() == Some(&entry.uuid)).collect::<Vec<_>>();

                                    div class="flex justify-between items-start mb-2" {
                                        div class="flex items-center gap-2" {
                                            span class="font-semibold" {
                                                @match msg.role {
                                                    MessageRole::User => {
                                                        "👤 User"
                                                    }
                                                    MessageRole::Assistant => {
                                                        "🤖 Assistant"
                                                        @if let Some(model) = &msg.model {
                                                            span class="ml-2 text-xs text-gray-400" { "(" (model) ")" }
                                                        }
                                                    }
                                                    MessageRole::System => {
                                                        "⚙️ System"
                                                    }
                                                }
                                            }
                                            @if !entry_notes.is_empty() {
                                                span class="px-2 py-1 bg-yellow-600 text-yellow-100 text-xs rounded" {
                                                    "📝 " (entry_notes.len()) " note" @if entry_notes.len() != 1 { "s" }
                                                }
                                            }
                                        }
                                        div class="flex items-center gap-2" {
                                            button class="add-note-to-entry opacity-0 group-hover:opacity-100 transition-opacity text-xs px-2 py-1 bg-gray-600 hover:bg-gray-500 rounded"
                                                data-entry-id=(&entry.uuid)
                                                data-session-id=(&session_id) {
                                                "📝 Add Note"
                                            }
                                            span class="text-xs text-gray-400" {
                                                (&entry.timestamp)
                                            }
                                        }
                                    }
                                    @if let Some(content) = &msg.content {
                                        div class="whitespace-pre-wrap" {
                                            @match content {
                                                MessageContent::Text(text) => {
                                                    (text)
                                                }
                                                MessageContent::Parts(parts) => {
                                                    @for part in parts {
                                                        @match part {
                                                            ContentPart::Text { text } => {
                                                                (text)
                                                            }
                                                            _ => {
                                                                div class="text-gray-400 italic" { "[Non-text content]" }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // Inline note editor (hidden by default)
                                    div class="note-editor mt-3 p-3 bg-gray-700 rounded" data-entry-id=(&entry.uuid) {
                                        textarea class="entry-note-textarea w-full px-3 py-2 bg-gray-600 text-gray-200 rounded border border-gray-500 focus:border-crab-accent focus:outline-none resize-none"
                                            rows="3"
                                            placeholder="Add your note..."
                                            data-entry-id=(&entry.uuid) {}
                                        div class="flex gap-2 mt-2" {
                                            button class="save-entry-note px-3 py-1 bg-crab-accent text-white rounded hover:bg-blue-500 transition-colors text-sm"
                                                data-entry-id=(&entry.uuid)
                                                data-session-id=(&session_id) {
                                                "Save Note"
                                            }
                                            button class="cancel-entry-note px-3 py-1 bg-gray-600 text-gray-200 rounded hover:bg-gray-500 transition-colors text-sm"
                                                data-entry-id=(&entry.uuid) {
                                                "Cancel"
                                            }
                                        }
                                    }

                                    // Show notes attached to this entry
                                    @if !entry_notes.is_empty() {
                                        div class="mt-3 pt-3 border-t border-gray-600" {
                                            div class="text-xs font-semibold text-yellow-400 mb-2" { "Notes:" }
                                            div class="space-y-2" {
                                                @for note in &entry_notes {
                                                    div class="bg-gray-700 rounded p-2 text-sm" {
                                                        div class="flex justify-between items-start mb-1" {
                                                            span class="text-xs text-gray-400" {
                                                                (chrono::DateTime::from_timestamp(note.created_at, 0)
                                                                    .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                                                                    .unwrap_or_else(|| "Unknown".to_string()))
                                                            }
                                                            button class="delete-note-btn text-red-400 hover:text-red-300 text-xs"
                                                                data-note-id=(&note.id)
                                                                data-session-id=(&session_id) {
                                                                "Delete"
                                                            }
                                                        }
                                                        div class="whitespace-pre-wrap" { (&note.content) }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        } // Close conversation entries column

                        // Notes sidebar
                        div class="w-96 notes-section" {
                            div class="bg-gray-800 rounded-lg p-4" {
                                h2 class="text-xl font-bold mb-4 flex items-center gap-2" {
                                    "📝 General Notes"
                                    @let general_notes = notes.iter().filter(|n| n.entry_id.is_none()).collect::<Vec<_>>();
                                    span class="text-sm text-gray-400 font-normal" { "(" (general_notes.len()) ")" }
                                }

                                // Add general note form
                                div class="mb-4" {
                                    textarea id="new-note-content"
                                        class="w-full px-3 py-2 bg-gray-700 text-gray-200 rounded border border-gray-600 focus:border-crab-accent focus:outline-none resize-none"
                                        rows="3"
                                        placeholder="Add a general note..." {}
                                    button id="add-note-btn"
                                        class="mt-2 w-full px-4 py-2 bg-crab-accent text-white rounded hover:bg-blue-500 transition-colors"
                                        data-session-id=(&session_id) {
                                        "Add General Note"
                                    }
                                }

                                // General notes list
                                div id="notes-list" class="space-y-3" {
                                    @for note in notes.iter().filter(|n| n.entry_id.is_none()) {
                                        div class="note-item bg-gray-700 rounded p-3" data-note-id=(&note.id) {
                                            div class="flex justify-between items-start mb-2" {
                                                span class="text-xs text-gray-400" {
                                                    (chrono::DateTime::from_timestamp(note.created_at, 0)
                                                        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                                                        .unwrap_or_else(|| "Unknown".to_string()))
                                                }
                                                button class="delete-note-btn text-red-400 hover:text-red-300 text-xs"
                                                    data-note-id=(&note.id)
                                                    data-session-id=(&session_id) {
                                                    "Delete"
                                                }
                                            }
                                            div class="text-sm whitespace-pre-wrap" { (&note.content) }
                                        }
                                    }
                                }
                            }
                        }
                    } // Close two column layout
                }

                // JavaScript for notes functionality
                script { (PreEscaped(r#"
                    // Add general note functionality
                    document.getElementById('add-note-btn').addEventListener('click', async (e) => {
                        const sessionId = e.target.dataset.sessionId;
                        const contentEl = document.getElementById('new-note-content');
                        const content = contentEl.value.trim();

                        if (!content) return;

                        try {
                            const response = await fetch(`/api/notes/${sessionId}`, {
                                method: 'POST',
                                headers: {
                                    'Content-Type': 'application/json',
                                },
                                body: JSON.stringify({ content })
                            });

                            if (response.ok) {
                                window.location.reload();
                            } else {
                                console.error('Failed to add note');
                            }
                        } catch (error) {
                            console.error('Error adding note:', error);
                        }
                    });

                    // Add note to specific entry - show inline editor
                    document.querySelectorAll('.add-note-to-entry').forEach(btn => {
                        btn.addEventListener('click', (e) => {
                            const entryId = e.target.dataset.entryId;
                            const editor = document.querySelector(`.note-editor[data-entry-id="${entryId}"]`);
                            const textarea = editor.querySelector('.entry-note-textarea');

                            // Hide all other editors
                            document.querySelectorAll('.note-editor').forEach(ed => {
                                if (ed !== editor) ed.classList.remove('active');
                            });

                            // Show this editor and focus
                            editor.classList.add('active');
                            textarea.focus();
                        });
                    });

                    // Save entry note
                    document.querySelectorAll('.save-entry-note').forEach(btn => {
                        btn.addEventListener('click', async (e) => {
                            const entryId = e.target.dataset.entryId;
                            const sessionId = e.target.dataset.sessionId;
                            const textarea = document.querySelector(`.entry-note-textarea[data-entry-id="${entryId}"]`);
                            const content = textarea.value.trim();

                            if (!content) {
                                textarea.focus();
                                return;
                            }

                            // Disable button during save
                            e.target.disabled = true;
                            e.target.textContent = 'Saving...';

                            try {
                                const response = await fetch(`/api/notes/${sessionId}`, {
                                    method: 'POST',
                                    headers: {
                                        'Content-Type': 'application/json',
                                    },
                                    body: JSON.stringify({
                                        content,
                                        entry_id: entryId
                                    })
                                });

                                if (response.ok) {
                                    window.location.reload();
                                } else {
                                    console.error('Failed to add note');
                                    e.target.disabled = false;
                                    e.target.textContent = 'Save Note';
                                }
                            } catch (error) {
                                console.error('Error adding note:', error);
                                e.target.disabled = false;
                                e.target.textContent = 'Save Note';
                            }
                        });
                    });

                    // Cancel entry note
                    document.querySelectorAll('.cancel-entry-note').forEach(btn => {
                        btn.addEventListener('click', (e) => {
                            const entryId = e.target.dataset.entryId;
                            const editor = document.querySelector(`.note-editor[data-entry-id="${entryId}"]`);
                            const textarea = editor.querySelector('.entry-note-textarea');

                            editor.classList.remove('active');
                            textarea.value = '';
                        });
                    });

                    // Delete note functionality
                    document.querySelectorAll('.delete-note-btn').forEach(btn => {
                        btn.addEventListener('click', async (e) => {
                            if (!confirm('Delete this note?')) return;

                            const sessionId = e.target.dataset.sessionId;
                            const noteId = e.target.dataset.noteId;

                            try {
                                const response = await fetch(`/api/notes/${sessionId}/${noteId}`, {
                                    method: 'DELETE'
                                });

                                if (response.ok) {
                                    // Remove note from UI or reload
                                    const noteEl = e.target.closest('.note-item');
                                    if (noteEl) {
                                        noteEl.remove();
                                    } else {
                                        // For inline notes, reload the page
                                        window.location.reload();
                                    }
                                } else {
                                    console.error('Failed to delete note');
                                }
                            } catch (error) {
                                console.error('Error deleting note:', error);
                            }
                        });
                    });

                    // Allow Enter key to add general note (Shift+Enter for newline)
                    document.getElementById('new-note-content').addEventListener('keydown', (e) => {
                        if (e.key === 'Enter' && !e.shiftKey) {
                            e.preventDefault();
                            document.getElementById('add-note-btn').click();
                        }
                    });

                    // Keyboard shortcuts for entry note editors
                    document.querySelectorAll('.entry-note-textarea').forEach(textarea => {
                        textarea.addEventListener('keydown', (e) => {
                            const entryId = textarea.dataset.entryId;

                            // Ctrl/Cmd + Enter to save
                            if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
                                e.preventDefault();
                                document.querySelector(`.save-entry-note[data-entry-id="${entryId}"]`).click();
                            }
                            // Escape to cancel
                            else if (e.key === 'Escape') {
                                e.preventDefault();
                                document.querySelector(`.cancel-entry-note[data-entry-id="${entryId}"]`).click();
                            }
                        });
                    });
                "#)) }
            }
        }
    };

    Html(markup.into_string())
}
