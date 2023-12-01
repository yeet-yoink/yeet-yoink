//! Contains the `/metrics` endpoint filter.

use crate::metrics::Metrics;
use axum::routing::get;
use axum::Router;

pub trait MetricsRoutes {
    /// Provides an API for Prometheus/OpenMetrics metrics.
    ///
    /// ```http
    /// GET /metrics HTTP/1.1
    /// ```
    fn map_metrics_endpoint(self) -> Self;
}

impl<S> MetricsRoutes for Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    // Ensure HttpCallMetricTracker is updated.
    fn map_metrics_endpoint(self) -> Self {
        self.route("/metrics", get(render_metrics))
    }
}

async fn render_metrics() -> String {
    Metrics::get().encode()
}
