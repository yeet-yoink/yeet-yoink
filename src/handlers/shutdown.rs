//! Contains the `/stop` endpoint filter.

use crate::AppState;
use axum::body::HttpBody;
use axum::extract::State;
use axum::routing::post;
use axum::Router;
use tracing::warn;

pub trait ShutdownRoutes {
    /// Provides an API for graceful shutdown.
    ///
    /// ```http
    /// POST /stop HTTP/1.1
    /// ```
    fn map_shutdown_endpoint(self) -> Self;
}

impl<B> ShutdownRoutes for Router<AppState, B>
where
    B: HttpBody + Send + 'static,
{
    fn map_shutdown_endpoint(self) -> Self {
        self.route("/stop", post(shutdown))
    }
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
