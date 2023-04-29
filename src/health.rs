use std::convert::Infallible;
use std::fmt::{Display, Formatter};
use warp::http::Response;
use warp::hyper::Body;
use warp::{path, Filter, Rejection, Reply};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum HealthCheck {
    /// A Kubernetes startup probe.
    Startup,
    /// A Kubernetes readiness probe.
    Readiness,
    /// A Kubernetes liveliness probe.
    Liveness,
    /// A full health check.
    Full(HealthCheckFormat),
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum HealthCheckFormat {
    Compact,
    Complex,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum HealthState {
    Healthy,
    Degraded,
    Failed,
}

/// Builds the health handlers.
pub fn health_filters() -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
    health_filter("health", HealthCheck::Full(HealthCheckFormat::Compact))
        .or(health_filter("startupz", HealthCheck::Startup))
        .or(health_filter("readyz", HealthCheck::Readiness))
        .or(health_filter("livez", HealthCheck::Liveness))
        .or(health_filter(
            "healthz",
            HealthCheck::Full(HealthCheckFormat::Complex),
        ))
}

/// Builds a health handler.
///
/// ## Arguments
/// * `path` - The path on which to host the handler, e.g. `health`, `readyz`, etc.
/// * `checks` - The type of health check to run on that path.
fn health_filter(
    path: &'static str,
    checks: HealthCheck,
) -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
    warp::get()
        .and(warp::path(path))
        .and(path::end())
        .and(with_check_type(checks))
        .and_then(handle_health)
}

/// Performs a health check.
///
/// ```http
/// GET /health
/// ```
async fn handle_health(checks: HealthCheck) -> Result<impl Reply, Rejection> {
    // TODO: Get the path, track the metric.
    match checks {
        HealthCheck::Startup => Ok(HealthState::Healthy),
        HealthCheck::Readiness => Ok(HealthState::Healthy),
        HealthCheck::Liveness => Ok(HealthState::Healthy),
        HealthCheck::Full(HealthCheckFormat::Compact) => Ok(HealthState::Healthy),
        HealthCheck::Full(HealthCheckFormat::Complex) => Ok(HealthState::Healthy),
    }
}

/// Injects the [`HealthCheck`] type into the request pipeline.
fn with_check_type(
    checks: HealthCheck,
) -> impl Filter<Extract = (HealthCheck,), Error = Infallible> + Copy + Clone {
    warp::any().map(move || checks)
}

impl Display for HealthState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthState::Healthy => write!(f, "Healthy"),
            HealthState::Degraded => write!(f, "Degraded"),
            HealthState::Failed => write!(f, "Failed"),
        }
    }
}

impl Reply for HealthState {
    fn into_response(self) -> warp::reply::Response {
        Response::new(Body::from(format!("{}", self)))
    }
}
