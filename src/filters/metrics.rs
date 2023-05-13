//! Contains the `/metrics` endpoint filter.

use crate::metrics::Metrics;
use axum::body::HttpBody;
use axum::routing::{get, MethodRouter};
use std::convert::Infallible;

const ROUTE: &'static str = "metrics";

/// Provides metrics.
///
/// ```http
/// GET /metrics
/// ```
pub fn metrics_endpoint<S, B>() -> MethodRouter<S, B, Infallible>
where
    S: Clone + Send + Sync + 'static,
    B: HttpBody + Send + 'static,
{
    get(render_metrics)
}

async fn render_metrics() -> String {
    Metrics::get().encode()
}
