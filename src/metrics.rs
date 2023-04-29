use lazy_static::lazy_static;
use prometheus_client::encoding::text::encode;
use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
use prometheus_client::metrics::counter::{Atomic, Counter};
use prometheus_client::metrics::family::Family;
use prometheus_client::registry::Registry;
use std::io::Write;

lazy_static! {
    // Create a metric registry.
    static ref METRICS: Metrics = Metrics::new();
}

/// The metrics registry.
pub struct Metrics {
    metrics: Registry,
}

impl Metrics {
    /// Gets a reference to the global metrics registry.
    pub fn get() -> &'static Self {
        &METRICS
    }

    /// Encode the metrics into the specified buffer..
    ///
    /// ## Arguments
    /// * `buffer` - The buffer to use to encode the metrics into.
    pub fn encode_into(&self, buffer: &mut String) {
        encode(buffer, &self.metrics).unwrap();
    }

    /// Encode the metrics into a string.
    ///
    /// ## Returns
    /// The Prometheus/OpenMetrics encoded metrics as as string.
    pub fn encode(&self) -> String {
        let mut buffer = String::new();
        self.encode_into(&mut buffer);
        buffer
    }

    /// Creates a new metrics registry.
    fn new() -> Self {
        let mut metrics = <Registry>::default();
        http::register_http_requests(&mut metrics);

        Self { metrics }
    }
}

/// HTTP based metrics.
pub mod http {
    use super::*;

    lazy_static! {
        // Create a sample counter metric family utilizing the above custom label
        // type, representing the number of HTTP requests received.
        static ref FAMILY: Family<Labels, Counter> = Family::default();
    }

    #[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
    struct Labels {
        // Use your own enum types to represent label values.
        method: Method,
        // Or just a plain string.
        path: String,
    }

    /// The HTTP method to track.
    #[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelValue)]
    pub enum Method {
        GET,
        PUT,
    }

    /// Register the `http_requests` metric family with the registry.
    pub fn register_http_requests(registry: &mut Registry) {
        registry.register(
            // With the metric name.
            "http_requests",
            // And the metric help text.
            "Number of HTTP requests received",
            FAMILY.clone(),
        );
    }

    /// HTTP call metrics. Can be cheaply cloned.
    #[derive(Default)]
    pub struct HttpMetrics;

    impl HttpMetrics {
        /// Tracks one call to the specified HTTP path and method.
        pub fn track<P: AsRef<str>>(path: P, method: Method) {
            FAMILY
                .get_or_create(&Labels {
                    method,
                    path: path.as_ref().to_string(),
                })
                .inc();
        }
    }
}

pub mod warp {
    use crate::metrics::http::{HttpMetrics, Method};
    use crate::metrics::Metrics;
    use warp::{Filter, Rejection, Reply};

    /// Performs a health check.
    ///
    /// ```http
    /// GET /health
    /// ```
    pub fn metrics_filter() -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
        warp::get()
            .and(warp::path("metrics"))
            .and(warp::path::end())
            .and_then(handle_metrics)
    }

    async fn handle_metrics() -> Result<impl Reply, Rejection> {
        HttpMetrics::track("metrics", Method::GET);
        let metrics = Metrics::get().encode();
        Ok(metrics)
    }
}
