//! Contains the `/metrics` endpoint filter.

use crate::metrics::Metrics;
use warp::{Filter, Rejection, Reply};

const ROUTE: &'static str = "metrics";

/// Provides metrics.
///
/// ```http
/// GET /metrics
/// ```
pub fn metrics_endpoint() -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
    warp::get()
        .and(warp::path(ROUTE))
        .and(warp::path::end())
        .and_then(render_metrics)
}

async fn render_metrics() -> Result<impl Reply, Rejection> {
    let metrics = Metrics::get().encode();
    Ok(metrics)
}
