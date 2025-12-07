mod error;
mod handlers;
mod protocol;
mod state;
mod storage;
mod websocket;

pub use error::{Result, SignallingError};
pub use handlers::{get_peers, health, ws_grabber_handler, ws_player_handler};
pub use state::AppState;
pub use storage::Storage;

use axum::{
    routing::get,
    Router,
};
use std::sync::Arc;
use tower_http::{
    cors::{Any, CorsLayer},
    services::ServeDir,
};
use tracing::info;

pub fn create_router(state: Arc<AppState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/player", get(ws_player_handler))
        .route("/grabber/:name", get(ws_grabber_handler))
        .route("/api/peers", get(get_peers))
        .route("/api/health", get(health))
        .nest_service("/", ServeDir::new("web"))
        .layer(cors)
        .with_state(state)
}

pub async fn start_server(bind_addr: &str, state: Arc<AppState>) -> Result<()> {
    let app = create_router(state);

    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .map_err(|e| SignallingError::WebSocket(format!("Failed to bind: {}", e)))?;

    info!("Signalling server listening on {}", bind_addr);

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await
    .map_err(|e| SignallingError::WebSocket(format!("Server error: {}", e)))?;

    Ok(())
}
