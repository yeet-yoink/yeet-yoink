//! Contains warp filters.

mod health;
mod metrics;
mod shutdown;
mod yeet;
mod yoink;

use chrono::{DateTime, Utc};
pub use health::HealthRoutes;
pub use metrics::MetricsRoutes;
pub use shutdown::ShutdownRoutes;
pub use yeet::YeetRoutes;
pub use yoink::YoinkRoutes;

pub fn expiration_as_rfc1123(expires: &tokio::time::Instant) -> String {
    let expire_in = expires.duration_since(tokio::time::Instant::now());
    system_time_as_rfc1123(std::time::SystemTime::now() + expire_in)
}

/// Formats a point in time for HTTP headers such as `Expires`.
pub fn system_time_as_rfc1123(time: std::time::SystemTime) -> String {
    DateTime::<Utc>::from(time)
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}
