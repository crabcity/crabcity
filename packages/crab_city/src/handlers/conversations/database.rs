use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Deserialize;

use crate::AppState;
use crate::repository;

#[derive(Deserialize)]
pub struct PaginationParams {
    page: Option<i64>,
    per_page: Option<i64>,
}

pub async fn list_conversations(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
) -> Result<impl IntoResponse, StatusCode> {
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(20).clamp(1, 100);

    match state
        .repository
        .list_conversations_paginated(page, per_page)
        .await
    {
        Ok(response) => Ok(Json(response)),
        Err(e) => {
            tracing::error!("Failed to list conversations: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Deserialize)]
pub struct SearchParams {
    q: String,
    page: Option<i64>,
    per_page: Option<i64>,
    /// Filter by message role (user, assistant)
    role: Option<String>,
    /// Filter entries after this Unix timestamp
    date_from: Option<i64>,
    /// Filter entries before this Unix timestamp
    date_to: Option<i64>,
    /// Only return conversations containing tool use
    has_tools: Option<bool>,
}

pub async fn search_conversations_handler(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<impl IntoResponse, StatusCode> {
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(20).clamp(1, 100);

    let filters = repository::SearchFilters {
        role: params.role,
        date_from: params.date_from,
        date_to: params.date_to,
        has_tools: params.has_tools,
    };

    match state
        .repository
        .search_conversations(&params.q, page, per_page, 3, &filters)
        .await
    {
        Ok(response) => Ok(Json(response)),
        Err(e) => {
            tracing::error!("Failed to search conversations: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

pub async fn get_conversation_by_id(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    match state.repository.get_conversation_with_entries(&id).await {
        Ok(Some(conversation)) => Ok(Json(conversation)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!("Failed to get conversation {}: {}", id, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Deserialize)]
pub struct CreateCommentRequest {
    author: Option<String>,
    content: String,
    entry_uuid: Option<String>,
}

pub async fn add_comment(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Json(req): Json<CreateCommentRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let comment =
        crate::models::Comment::new(conversation_id, req.content, req.author, req.entry_uuid);

    match state.repository.add_comment(&comment).await {
        Ok(id) => Ok(Json(serde_json::json!({ "id": id }))),
        Err(e) => {
            tracing::error!("Failed to add comment: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

pub async fn get_comments(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    match state
        .repository
        .get_conversation_comments(&conversation_id)
        .await
    {
        Ok(comments) => Ok(Json(comments)),
        Err(e) => {
            tracing::error!("Failed to get comments: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[derive(Deserialize)]
pub struct CreateShareRequest {
    expires_in_days: Option<i32>,
    title: Option<String>,
    description: Option<String>,
}

pub async fn create_share(
    State(state): State<AppState>,
    Path(conversation_id): Path<String>,
    Json(req): Json<CreateShareRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let mut share = crate::models::ConversationShare::new(conversation_id, req.expires_in_days);

    if let Some(title) = req.title {
        share.title = Some(title);
    }
    if let Some(desc) = req.description {
        share.description = Some(desc);
    }

    match state.repository.create_share(&share).await {
        Ok(token) => Ok(Json(serde_json::json!({
            "share_token": token,
            "url": format!("/api/share/{}", token)
        }))),
        Err(e) => {
            tracing::error!("Failed to create share: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

pub async fn get_shared_conversation(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let share = match state.repository.get_share(&token).await {
        Ok(Some(s)) => s,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!("Failed to get share: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    if share.is_expired() {
        return Err(StatusCode::GONE);
    }

    if share.is_access_limit_reached() {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    if let Err(e) = state.repository.increment_share_access(&token).await {
        tracing::warn!("Failed to increment share access: {}", e);
    }

    match state
        .repository
        .get_conversation_with_entries(&share.conversation_id)
        .await
    {
        Ok(Some(conversation)) => Ok(Json(conversation)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!("Failed to get shared conversation: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}
