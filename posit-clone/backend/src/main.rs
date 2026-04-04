mod models;
mod routes;
mod state;

use axum::{routing::get, Router};
use std::path::PathBuf;
use tower_http::cors::CorsLayer;

use routes::notebooks::{
    create_notebook, delete_notebook, get_notebook, list_notebooks, update_notebook,
};
use state::AppState;

#[tokio::main]
async fn main() {
    let workspace_dir = std::env::var("WORKSPACE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("workspace"));

    let state = AppState::new(workspace_dir.clone());

    // Ensure the notebooks directory exists on startup
    let notebooks_dir = state.notebooks_dir();
    std::fs::create_dir_all(&notebooks_dir)
        .unwrap_or_else(|e| eprintln!("Failed to create notebooks dir: {e}"));

    let app = build_router(state);

    let addr = "0.0.0.0:3001";
    println!("Listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/notebooks", get(list_notebooks).post(create_notebook))
        .route(
            "/api/notebooks/:id",
            get(get_notebook)
                .put(update_notebook)
                .delete(delete_notebook),
        )
        .with_state(state)
        .layer(CorsLayer::permissive())
}
