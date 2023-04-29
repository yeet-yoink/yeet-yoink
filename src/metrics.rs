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
    use prometheus_client::encoding::LabelValueEncoder;
    use std::fmt::{Display, Error, Formatter, Write};
    use warp::http::Method;

    lazy_static! {
        // Create a sample counter metric family utilizing the above custom label
        // type, representing the number of HTTP requests received.
        static ref FAMILY: Family<Labels, Counter> = Family::default();
    }

    #[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
    struct Labels {
        // Use your own enum types to represent label values.
        method: HttpMethod,
        // Or just a plain string.
        path: String,
    }

    /// The HTTP method to track.
    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    pub enum HttpMethod {
        OPTIONS,
        GET,
        POST,
        PUT,
        DELETE,
        HEAD,
        PATCH,
        UNHANDLED(Method),
    }

    impl EncodeLabelValue for HttpMethod {
        fn encode(&self, encoder: &mut LabelValueEncoder) -> Result<(), Error> {
            encoder.write_str(self.to_string().as_str())
        }
    }

    impl Display for HttpMethod {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                HttpMethod::OPTIONS => write!(f, "OPTIONS"),
                HttpMethod::GET => write!(f, "GET"),
                HttpMethod::POST => write!(f, "POST"),
                HttpMethod::PUT => write!(f, "PUT"),
                HttpMethod::DELETE => write!(f, "DELETE"),
                HttpMethod::HEAD => write!(f, "HEAD"),
                HttpMethod::PATCH => write!(f, "PATCH"),
                HttpMethod::UNHANDLED(other) => write!(f, "{other}"),
            }
        }
    }

    impl From<Method> for HttpMethod {
        fn from(value: Method) -> Self {
            match value {
                Method::GET => Self::GET,
                Method::OPTIONS => Self::OPTIONS,
                Method::POST => Self::POST,
                Method::PUT => Self::PUT,
                Method::DELETE => Self::DELETE,
                Method::HEAD => Self::HEAD,
                Method::PATCH => Self::PATCH,
                other => Self::UNHANDLED(other),
            }
        }
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
        pub fn track<P: AsRef<str>>(path: P, method: HttpMethod) {
            FAMILY
                .get_or_create(&Labels {
                    method,
                    path: path.as_ref().to_string(),
                })
                .inc();
        }
    }
}

pub mod http_api {
    use crate::metrics::http::HttpMetrics;
    use crate::metrics::Metrics;
    use std::convert::Infallible;
    use warp::path::FullPath;
    use warp::{http, method, path, Filter, Rejection, Reply};

    /// Performs a health check.
    ///
    /// ```http
    /// GET /health
    /// ```
    pub fn metrics_endpoint() -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
        warp::get()
            .and(warp::path("metrics"))
            .and(path::end())
            .and_then(render_metrics)
    }

    pub fn with_call_metrics() -> impl Filter<Extract = (), Error = Infallible> + Clone {
        warp::any()
            .and(path::full())
            .and(method())
            .map(|path: FullPath, method: http::Method| {
                HttpMetrics::track(path.as_str(), method.into())
            })
            .untuple_one()
    }

    async fn render_metrics() -> Result<impl Reply, Rejection> {
        let metrics = Metrics::get().encode();
        Ok(metrics)
    }
}
