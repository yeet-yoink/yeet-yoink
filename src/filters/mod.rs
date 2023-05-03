//! Contains warp filters.

use crate::metrics::Metrics;
use warp::{Filter, Rejection, Reply};

/// Performs a health check.
///
/// ```http
/// GET /health
/// ```
pub fn metrics_endpoint() -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
    warp::get()
        .and(warp::path("metrics"))
        .and(warp::path::end())
        .and_then(render_metrics)
}

async fn render_metrics() -> Result<impl Reply, Rejection> {
    let metrics = Metrics::get().encode();
    Ok(metrics)
}
