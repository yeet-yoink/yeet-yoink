//! Contains the `/stop` endpoint filter.

use crate::AppState;
use axum::body::HttpBody;
use axum::extract::State;
use axum::routing::{post, MethodRouter};
use tracing::warn;

/// Builds the route for the [`shutdown`] handler.
pub fn shutdown_endpoint() -> MethodRouter<AppState> {
    // POST /stop to shut down the server.
    post(shutdown)
}

/// Initiates a graceful shutdown.
///
/// ```http
/// POST /stop
/// ```
async fn shutdown(State(state): State<AppState>) {
    warn!("Initiating shutdown from API call");
    state.shutdown_tx.send(()).ok();
}
