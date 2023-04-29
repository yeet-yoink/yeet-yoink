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
pub fn health_handlers() -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
    health_handler("health", HealthCheck::Full(HealthCheckFormat::Compact))
        .or(health_handler("startupz", HealthCheck::Startup))
        .or(health_handler("readyz", HealthCheck::Readiness))
        .or(health_handler("livez", HealthCheck::Liveness))
        .or(health_handler(
            "healthz",
            HealthCheck::Full(HealthCheckFormat::Complex),
        ))
}
/// Builds a health handler.
///
/// ## Arguments
/// * `path` - The path on which to host the handler, e.g. `health`, `readyz`, etc.
/// * `checks` - The type of health check to run on that path.
pub fn health_handler(
    path: &'static str,
    checks: HealthCheck,
) -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
    warp::get()
        .and(warp::path(path))
        .and(path::end())
        .and(with_check_type(checks))
        .and_then(health)
}

/// Performs a health check.
///
/// ```http
/// GET /health
/// ```
async fn health(checks: HealthCheck) -> Result<impl Reply, Rejection> {
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
