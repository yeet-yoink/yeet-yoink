pub mod http;

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
