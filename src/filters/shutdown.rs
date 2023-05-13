//! Contains the `/stop` endpoint filter.

use tokio::sync::broadcast;
use tracing::warn;

const ROUTE: &'static str = "stop";

/// Builds the route for the [`shutdown`] handler.
pub fn shutdown_endpoint(
    tx: broadcast::Sender<()>,
) -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
    let shutdown_filter = warp::any().map(move || tx.clone());

    // POST /stop to shut down the server.
    warp::post()
        .and(warp::path(ROUTE))
        .and(warp::path::end())
        .and(shutdown_filter)
        .and_then(shutdown)
}

/// Initiates a graceful shutdown.
///
/// ```http
/// POST /stop
/// ```
async fn shutdown(tx: broadcast::Sender<()>) -> Result<impl Reply, Rejection> {
    warn!("Initiating shutdown from API call");
    tx.send(()).ok();
    Ok(warp::reply::reply())
}
