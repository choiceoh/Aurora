mod agent;
mod client;
mod config;
mod deneb;
mod preprocessing;
mod tools;
mod types;
mod ws;

use axum::{
    Router,
    extract::{State, WebSocketUpgrade},
    response::IntoResponse,
    routing::get,
};
use std::sync::Arc;

use config::Config;
use ws::AppState;

#[tokio::main]
async fn main() {
    let config = Config::load();

    let listen_addr = config
        .as_ref()
        .map(|c| c.listen_addr())
        .unwrap_or_else(|| "0.0.0.0:3710".to_string());

    let state = Arc::new(AppState::new(config));

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/health", get(health))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&listen_addr)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind {listen_addr}: {e}"));

    println!("Aurora server listening on {listen_addr}");
    println!("  WebSocket: ws://{listen_addr}/ws");
    println!("  Health:    http://{listen_addr}/health");

    axum::serve(listener, app).await.unwrap();
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws::handle_ws(socket, state))
}

async fn health() -> &'static str {
    "ok"
}
