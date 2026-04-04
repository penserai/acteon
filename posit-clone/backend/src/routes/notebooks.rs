use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::models::notebook::{Language, Notebook};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateNotebookRequest {
    pub name: String,
    pub language: Language,
}

pub async fn list_notebooks(State(state): State<AppState>) -> impl IntoResponse {
    match state.list_notebooks() {
        Ok(notebooks) => (StatusCode::OK, Json(notebooks)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn create_notebook(
    State(state): State<AppState>,
    Json(body): Json<CreateNotebookRequest>,
) -> impl IntoResponse {
    let notebooks_dir = state.notebooks_dir();
    let notebook = Notebook::new(body.name, body.language, &notebooks_dir);
    match state.save_notebook(&notebook) {
        Ok(()) => (StatusCode::CREATED, Json(notebook)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn get_notebook(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match state.load_notebook(id) {
        Ok(Some(notebook)) => (StatusCode::OK, Json(notebook)).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn update_notebook(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(mut notebook): Json<Notebook>,
) -> impl IntoResponse {
    // Verify the notebook exists before accepting an update
    match state.load_notebook(id) {
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Ok(Some(_)) => {}
    }
    // Ensure the id in the URL matches the body
    notebook.id = id;
    notebook.updated_at = Utc::now();
    match state.save_notebook(&notebook) {
        Ok(()) => (StatusCode::OK, Json(notebook)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

pub async fn delete_notebook(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match state.delete_notebook(id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
