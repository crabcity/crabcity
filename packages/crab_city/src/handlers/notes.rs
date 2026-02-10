use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;
use tracing::error;

use crate::AppState;

#[derive(Deserialize)]
pub struct CreateNoteRequest {
    content: String,
    entry_id: Option<String>,
}

pub async fn create_note(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(req): Json<CreateNoteRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    match state
        .notes_storage
        .add_note(&session_id, req.content, req.entry_id)
        .await
    {
        Ok(note) => Ok(Json(note)),
        Err(e) => {
            error!("Failed to create note: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

pub async fn get_notes(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let notes = state.notes_storage.get_notes(&session_id).await;
    Json(notes)
}

#[derive(Deserialize)]
pub struct UpdateNoteRequest {
    content: String,
}

pub async fn update_note(
    State(state): State<AppState>,
    Path((session_id, note_id)): Path<(String, String)>,
    Json(req): Json<UpdateNoteRequest>,
) -> StatusCode {
    match state
        .notes_storage
        .update_note(&session_id, &note_id, req.content)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(e) => {
            error!("Failed to update note: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

pub async fn delete_note(
    State(state): State<AppState>,
    Path((session_id, note_id)): Path<(String, String)>,
) -> StatusCode {
    match state.notes_storage.delete_note(&session_id, &note_id).await {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(e) => {
            error!("Failed to delete note: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
