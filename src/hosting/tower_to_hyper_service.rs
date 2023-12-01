//! Provides the [`TowerToHyperService`] type. This is taken from axum 0.7's code base.

use axum::body::Body;
use axum::extract::Request;
use hyper::body::Incoming;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tower::util::Oneshot;
use tower::ServiceExt;

/// A struct representing a tower service adapter for Hyper service.
///
/// This struct wraps a tower Service and exposes the Hyper service trait. It is generic over the
/// type of the tower service.
///
/// # Examples
///
/// ```
/// use tower::Service;
/// use tower_hyper::TowerToHyperService;
///
/// // Define a custom service
/// struct MyService;
///
/// impl Service for MyService {
///     // ...
/// }
///
/// // Create a new TowerToHyperService
/// let service = TowerToHyperService { service: MyService };
///
/// // Use the service as a Hyper service
/// let http_service = tower_hyper::service::make_service_fn(|_conn| {
///     let service = service.clone();
///     async move {
///         Ok::<_, hyper::Error>(service)
///     }
/// });
/// ```
#[derive(Debug, Copy, Clone)]
pub struct TowerToHyperService<S> {
    pub(crate) service: S,
}

impl<S> hyper::service::Service<Request<Incoming>> for TowerToHyperService<S>
where
    S: tower_service::Service<Request> + Clone,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = TowerToHyperServiceFuture<S, Request>;

    fn call(&self, req: Request<Incoming>) -> Self::Future {
        let req = req.map(Body::new);
        TowerToHyperServiceFuture {
            future: self.service.clone().oneshot(req),
        }
    }
}

/// A future that adapts a `tower_service::Service` into a `hyper::service::Service`.
///
/// This future is created by the `TowerToHyperService` middleware.
///
/// It wraps a `tower_service::Service` that takes a request `R` and returns a
/// future `F` representing the response. The `TowerToHyperServiceFuture` will
/// resolve to `Result<Response<B>, E>` where `B` is the response body type and
/// `E` is the error type produced by the service.
#[pin_project]
pub struct TowerToHyperServiceFuture<S, R>
where
    S: tower_service::Service<R>,
{
    #[pin]
    future: Oneshot<S, R>,
}

impl<S, R> Future for TowerToHyperServiceFuture<S, R>
where
    S: tower_service::Service<R>,
{
    type Output = Result<S::Response, S::Error>;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().future.poll(cx)
    }
}
