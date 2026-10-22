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

pub fn expiration_as_rfc1123(expires: &tokio::time::Instant) -> Option<String> {
    let expire_in = expires.duration_since(tokio::time::Instant::now());
    system_time_as_rfc1123(std::time::SystemTime::now().checked_add(expire_in)?)
}

/// Formats a point in time for HTTP headers such as `Expires`.
///
/// Returns `None` for times that cannot be represented, rather than panicking.
pub fn system_time_as_rfc1123(time: std::time::SystemTime) -> Option<String> {
    let since_epoch = time.duration_since(std::time::UNIX_EPOCH).ok()?;
    let secs = i64::try_from(since_epoch.as_secs()).ok()?;
    let time = DateTime::<Utc>::from_timestamp(secs, since_epoch.subsec_nanos())?;
    Some(time.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn formats_http_dates() {
        let time = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        assert_eq!(
            system_time_as_rfc1123(time).as_deref(),
            Some("Fri, 15 Jan 2027 08:00:00 GMT")
        );
    }

    #[test]
    fn unrepresentable_dates_are_rejected() {
        let far_future = UNIX_EPOCH + Duration::from_millis(u64::MAX);
        assert_eq!(system_time_as_rfc1123(far_future), None);
        let before_epoch = UNIX_EPOCH - Duration::from_secs(1);
        assert_eq!(system_time_as_rfc1123(before_epoch), None);
    }
}
