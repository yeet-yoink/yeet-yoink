//! Contains the `/health` endpoint filter.

use crate::health::HealthState;
use axum::body::HttpBody;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, MethodRouter};
use axum::Router;
use std::convert::Infallible;

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

pub trait HealthRoutes {
    /// Provides an API for initiating health checks.
    ///
    /// For readiness probes (compact output):
    ///
    /// ```http
    /// GET /readyz HTTP/1.1
    /// ```
    ///
    /// For liveness probes (compact output):
    ///
    /// ```http
    /// GET /livez HTTP/1.1
    /// ```
    ///
    /// For combined health probes (compact output):
    ///
    /// ```http
    /// GET /health HTTP/1.1
    /// ```
    ///
    /// For combined health probes in human-readable output:
    ///
    /// ```http
    /// GET /healthz HTTP/1.1
    /// ```
    fn map_health_endpoints(self) -> Self;
}

impl<S, B> HealthRoutes for Router<S, B>
where
    S: Clone + Send + Sync + 'static,
    B: HttpBody + Send + 'static,
{
    fn map_health_endpoints(self) -> Self {
        // Ensure HttpCallMetricTracker is updated.
        self.route(
            "/health",
            health_endpoint(HealthCheck::Full(HealthCheckFormat::Compact)),
        )
        .route("/startupz", health_endpoint(HealthCheck::Startup))
        .route("/readyz", health_endpoint(HealthCheck::Readiness))
        .route("/livez", health_endpoint(HealthCheck::Liveness))
        .route(
            "/healthz",
            health_endpoint(HealthCheck::Full(HealthCheckFormat::Complex)),
        )
    }
}

/// Builds a health handler.
///
/// ## Arguments
/// * `path` - The path on which to host the handler, e.g. `health`, `readyz`, etc.
/// * `checks` - The type of health check to run on that path.
fn health_endpoint<S, B>(checks: HealthCheck) -> MethodRouter<S, B, Infallible>
where
    S: Clone + Send + Sync + 'static,
    B: HttpBody + Send + 'static,
{
    get(move || handle_health(checks))
}

/// Performs a health check.
///
/// ```http
/// GET /health
/// ```
async fn handle_health(checks: HealthCheck) -> Result<HealthState, Infallible> {
    // TODO: Actually implement health checks!
    match checks {
        HealthCheck::Startup => Ok(HealthState::Healthy),
        HealthCheck::Readiness => Ok(HealthState::Healthy),
        HealthCheck::Liveness => Ok(HealthState::Healthy),
        HealthCheck::Full(HealthCheckFormat::Compact) => Ok(HealthState::Healthy),
        HealthCheck::Full(HealthCheckFormat::Complex) => Ok(HealthState::Healthy),
    }
}

impl IntoResponse for HealthState {
    fn into_response(self) -> Response {
        format!("{}", self).into_response()
    }
}
