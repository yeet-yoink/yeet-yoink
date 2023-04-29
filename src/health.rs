use std::fmt::{Display, Formatter};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum HealthState {
    Healthy,
    Degraded,
    Failed,
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

pub mod http_api {
    use super::*;
    use crate::metrics::http_api::with_call_metrics;
    use std::convert::Infallible;
    use warp::http::Response;
    use warp::hyper::Body;
    use warp::{path, Filter, Rejection, Reply};

    /// Defines a type of health check.
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

    /// Defines a specific type of format representation.
    #[derive(Debug, Copy, Clone, Eq, PartialEq)]
    pub enum HealthCheckFormat {
        Compact,
        Complex,
    }

    /// Builds the health handlers.
    pub fn health_endpoints() -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
        health_endpoint("health", HealthCheck::Full(HealthCheckFormat::Compact))
            .or(health_endpoint("startupz", HealthCheck::Startup))
            .or(health_endpoint("readyz", HealthCheck::Readiness))
            .or(health_endpoint("livez", HealthCheck::Liveness))
            .or(health_endpoint(
                "healthz",
                HealthCheck::Full(HealthCheckFormat::Complex),
            ))
    }

    /// Builds a health handler.
    ///
    /// ## Arguments
    /// * `path` - The path on which to host the handler, e.g. `health`, `readyz`, etc.
    /// * `checks` - The type of health check to run on that path.
    fn health_endpoint(
        path: &'static str,
        checks: HealthCheck,
    ) -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
        warp::get()
            .and(http_api::path(path))
            .and(path::end())
            .and(with_check_type(checks))
            .and(with_call_metrics())
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

    impl Reply for HealthState {
        fn into_response(self) -> warp::reply::Response {
            Response::new(Body::from(format!("{}", self)))
        }
    }
}
