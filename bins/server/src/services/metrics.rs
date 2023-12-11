use hyper::service::Service;
use hyper::{Request, StatusCode, Version};
use pin_project::pin_project;

use crate::metrics::http::HttpMetrics;
use axum::body::BoxBody;
use axum::http::Response;
use axum::response::IntoResponse;
use hyper::body::HttpBody;
use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::time::Instant;
use tower::Layer;
use tracing::debug;

/// A middleware for call metrics. Uses [`HttpMetrics`].
#[derive(Clone)]
pub struct HttpCallMetrics<S> {
    inner: S,
}

/// A layer for call metrics. Uses [`HttpCallMetrics`].
#[derive(Clone, Default)]
pub struct HttpCallMetricsLayer;

impl<S> HttpCallMetrics<S> {
    /// Creates a new [`HttpCallMetrics`]
    pub fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S> Layer<S> for HttpCallMetricsLayer {
    type Service = HttpCallMetrics<S>;

    fn layer(&self, inner: S) -> Self::Service {
        HttpCallMetrics::new(inner)
    }
}

impl<S, B> Service<Request<B>> for HttpCallMetrics<S>
where
    S: Service<Request<B>>,
    S::Response: IntoResponse,
    B: HttpBody,
{
    type Response = Response<BoxBody>;
    type Error = S::Error;
    type Future = HttpCallMetricsFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        let tracker = HttpCallMetricTracker::start(&request);

        // We start tracking request time before the first call to the future.
        HttpCallMetricsFuture::new(self.inner.call(request), tracker)
    }
}

/// A future returned from the [`HttpCallMetrics`].
///
/// ## Type arguments
/// * `F` - A wrapped future returning `Result<impl Response<O>, E>`.
/// * `O` - The body type of the HTTP response enclosed in `Response<O>`.
/// * `E` - The error type returned by the wrapped future.
#[pin_project]
pub struct HttpCallMetricsFuture<F>
where
    F: Future,
{
    #[pin]
    future: F,
    tracker: HttpCallMetricTracker,
}

impl<F> HttpCallMetricsFuture<F>
where
    F: Future,
{
    fn new(future: F, tracker: HttpCallMetricTracker) -> Self {
        Self { future, tracker }
    }
}

impl<F, R, E> Future for HttpCallMetricsFuture<F>
where
    F: Future<Output = Result<R, E>>,
    R: IntoResponse,
{
    type Output = Result<Response<BoxBody>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Note that this method will be called at least twice.
        let this = self.project();
        let response = match this.future.poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(reply) => reply,
        };

        let result = match response {
            Ok(reply) => {
                let response = reply.into_response();
                this.tracker
                    .set_state(ResultState::Result(response.status(), response.version()));
                Ok(response)
            }
            Err(e) => {
                this.tracker.set_state(ResultState::Failed);
                Err(e)
            }
        };
        Poll::Ready(result)
    }
}

/// A metrics tracker. Will call [`HttpMetrics::inc_in_flight`]
/// on construction and [`HttpMetrics::dec_in_flight`] on drop.
///
/// We require this helper type because [`HttpCallMetricsFuture`] cannot imply [`Drop`]
/// due to the use of [`pin_project`](pin_project::pin_project).
struct HttpCallMetricTracker {
    version: Version,
    method: hyper::Method,
    path_base: String,
    start: Instant,
    state: Cell<ResultState>,
    path_full: String,
}

pub enum ResultState {
    /// No result was executed so far, or the result was already processed.
    None,
    /// Request was started.
    Started,
    /// The result failed with an error.
    Failed,
    /// The result is an actual HTTP response.
    Result(StatusCode, Version),
}

impl HttpCallMetricTracker {
    fn start<B>(request: &Request<B>) -> Self {
        let method = request.method().clone();
        let path = request.uri().path();
        let version = request.version();

        // Ensure we don't create a new metric for every file name, i.e.
        // /yoink/4d6DOAMKQ5uhlE6eXKM_dQ should be tracked as /yoink.
        let path_str = path.to_string();
        let path_base = match path[1..].find('/') {
            None => path_str.clone(),
            Some(pos) => String::from(&path[0..(pos + 1)]),
        };

        debug!(
            "Start processing {version:?} {method} {path} (tracking as {path_base})",
            path = path_str
        );
        HttpMetrics::inc_in_flight(path_base.as_str());
        let start = Instant::now();
        Self {
            version,
            method,
            path_full: path_str,
            path_base,
            start,
            state: Cell::new(ResultState::Started),
        }
    }

    fn set_state(&self, state: ResultState) {
        self.state.set(state)
    }

    fn duration(&self) -> Duration {
        Instant::now() - self.start
    }
}

/// Implements the metrics finalization logic.
impl Drop for HttpCallMetricTracker {
    fn drop(&mut self) {
        match self.state.replace(ResultState::None) {
            ResultState::None => {
                // This was already handled; don't decrement metrics again.
                return;
            }
            ResultState::Started => {
                // no request was actually performed.
            }
            ResultState::Failed => {
                let duration = self.duration();
                debug!(
                    "Fail processing {version:?} {method} {path} - {duration:?}",
                    version = self.version,
                    method = self.method,
                    path = self.path_full,
                    duration = duration
                );
                HttpMetrics::track(&self.path_base, self.method.clone(), 0, duration);
            }
            ResultState::Result(status, version) => {
                let duration = self.duration();
                debug!(
                        "Done processing {version:?} {method} {path}: {response_version:?} {response_status} - {duration:?}",
                        version = self.version,
                        method = self.method,
                        path = self.path_full,
                        duration = duration,
                        response_version = version,
                        response_status = status
                    );
                HttpMetrics::track(
                    &self.path_base,
                    self.method.clone(),
                    status.as_u16(),
                    duration,
                );
            }
        }

        HttpMetrics::dec_in_flight(self.path_base.as_str());
    }
}
