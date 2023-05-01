use lazy_static::lazy_static;
use prometheus_client::encoding::text::encode;
use prometheus_client::registry::Registry;

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
    use prometheus_client::encoding::{EncodeLabelSet, EncodeLabelValue};
    use prometheus_client::metrics::counter::Counter;
    use prometheus_client::metrics::family::Family;
    use prometheus_client::metrics::gauge::Gauge;
    use prometheus_client::registry::{Registry, Unit};
    use std::fmt::{Display, Error, Formatter, Write};
    use std::time::Duration;
    use warp::http::Method;

    lazy_static! {
        // Create a sample counter metric family utilizing the above custom label
        // type, representing the number of HTTP requests received.
        static ref TRACK_ENDPOINT: Family<Labels, Counter> = Family::default();
        static ref TRACK_DURATION: Family<Labels, Counter<f64>> = Family::default();
        static ref TRACK_IN_FLIGHT: Family<InFlightLabels, Gauge> = Family::default();
    }

    #[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
    struct Labels {
        // Use your own enum types to represent label values.
        method: HttpMethod,
        // Or just a plain string.
        path: String,
        /// The HTTP status code.
        status: u16,
    }

    #[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
    struct InFlightLabels {
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

    impl From<&Method> for HttpMethod {
        fn from(value: &Method) -> Self {
            match value {
                &Method::GET => Self::GET,
                &Method::OPTIONS => Self::OPTIONS,
                &Method::POST => Self::POST,
                &Method::PUT => Self::PUT,
                &Method::DELETE => Self::DELETE,
                &Method::HEAD => Self::HEAD,
                &Method::PATCH => Self::PATCH,
                other => Self::UNHANDLED(other.clone()),
            }
        }
    }

    impl From<Method> for HttpMethod {
        fn from(value: Method) -> Self {
            HttpMethod::from(&value)
        }
    }

    /// Register the `http_requests` metric family with the registry.
    pub fn register_http_requests(registry: &mut Registry) {
        registry.register(
            // With the metric name.
            "http_requests",
            // And the metric help text.
            "Number of HTTP requests received",
            TRACK_ENDPOINT.clone(),
        );

        registry.register_with_unit(
            "http_duration",
            "Duration of HTTP requests executed",
            Unit::Seconds,
            TRACK_DURATION.clone(),
        );

        registry.register(
            "http_requests_in_flight",
            "Number of requests that are currently in flight",
            TRACK_IN_FLIGHT.clone(),
        );
    }

    /// HTTP call metrics. Can be cheaply cloned.
    #[derive(Default)]
    pub struct HttpMetrics;

    impl HttpMetrics {
        /// Tracks one call to the specified HTTP path and method.
        pub fn track<P: AsRef<str>>(path: P, method: HttpMethod, status: u16, elapsed: Duration) {
            TRACK_ENDPOINT
                .get_or_create(&Labels {
                    method: method.clone(),
                    path: path.as_ref().to_string(),
                    status,
                })
                .inc();

            TRACK_DURATION
                .get_or_create(&Labels {
                    method,
                    path: path.as_ref().to_string(),
                    status,
                })
                .inc_by(elapsed.as_secs_f64());
        }

        pub fn inc_in_flight<P: AsRef<str>>(path: P) {
            TRACK_IN_FLIGHT
                .get_or_create(&InFlightLabels {
                    path: path.as_ref().to_string(),
                })
                .inc();
        }

        pub fn dec_in_flight<P: AsRef<str>>(path: P) {
            TRACK_IN_FLIGHT
                .get_or_create(&InFlightLabels {
                    path: path.as_ref().to_string(),
                })
                .dec();
        }
    }
}

pub mod http_api {
    use crate::metrics::http::HttpMetrics;
    use crate::metrics::Metrics;
    use hyper::service::Service;
    use hyper::Request;
    use std::convert::Infallible;
    use std::task::{Context, Poll};
    use tower::Layer;
    use warp::log::{Info, Log};
    use warp::path::FullPath;
    use warp::{path, Filter, Rejection, Reply};

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

    pub fn with_start_call_metrics() -> impl Filter<Extract = (), Error = Infallible> + Clone {
        warp::any()
            .and(path::full())
            .map(|path: FullPath| {
                HttpMetrics::inc_in_flight(path.as_str());
            })
            .untuple_one()
    }

    pub fn with_end_call_metrics() -> Log<fn(Info<'_>)> {
        warp::log::custom(|info| {
            HttpMetrics::track(
                info.path(),
                info.method().into(),
                info.status().as_u16(),
                info.elapsed(),
            );
            HttpMetrics::dec_in_flight(info.path());
        })
    }

    async fn render_metrics() -> Result<impl Reply, Rejection> {
        let metrics = Metrics::get().encode();
        Ok(metrics)
    }

    /// A middleware for call metrics.
    pub struct HttpCallMetrics<T> {
        inner: T,
    }

    impl<T> HttpCallMetrics<T> {
        /// Creates a new [`HttpCallMetrics`]
        pub fn new(inner: T) -> Self {
            Self { inner }
        }
    }

    impl<S, B> Service<Request<B>> for HttpCallMetrics<S>
    where
        S: Service<Request<B>>,
    {
        type Response = S::Response;
        type Error = S::Error;
        type Future = S::Future;

        fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            self.inner.poll_ready(cx)
        }

        fn call(&mut self, request: Request<B>) -> Self::Future {
            // TODO: This doesn't work because `call` isn't `await'ed, therefore we drop before actually doing the work.
            let _guard = HttpCallMetricTracker::track(request.uri().path().to_string());
            self.inner.call(request)
        }
    }

    impl<S> Layer<S> for HttpCallMetrics<S> {
        type Service = HttpCallMetrics<S>;

        fn layer(&self, inner: S) -> Self::Service {
            HttpCallMetrics::new(inner)
        }
    }

    /// A metrics tracker. Will call [`HttpMetrics::inc_in_flight`]
    /// on construction and [`HttpMetrics::dec_in_flight`] on drop.
    struct HttpCallMetricTracker {
        path: String,
    }

    impl HttpCallMetricTracker {
        pub fn track(path: String) -> Self {
            HttpMetrics::inc_in_flight(path.as_str());
            Self { path }
        }
    }

    impl Drop for HttpCallMetricTracker {
        fn drop(&mut self) {
            HttpMetrics::dec_in_flight(self.path.as_str());
        }
    }
}
