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
    let expiration_date = std::time::SystemTime::now() + expire_in;
    let expiration_date = DateTime::<Utc>::from(expiration_date);
    expiration_date
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}
