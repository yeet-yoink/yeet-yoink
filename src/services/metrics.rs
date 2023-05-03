use hyper::service::Service;
use hyper::{Request, StatusCode, Version};
use pin_project::pin_project;

use crate::metrics::http::HttpMetrics;
use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::time::Instant;
use tracing::debug;

/// A middleware for call metrics. Uses [`HttpMetrics`].
#[derive(Clone)]
pub struct HttpCallMetrics<S, O> {
    inner: S,
    _phantom: PhantomData<O>,
}

impl<S, O> HttpCallMetrics<S, O> {
    /// Creates a new [`HttpCallMetrics`]
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            _phantom: PhantomData::default(),
        }
    }
}

impl<S, B, O> Service<Request<B>> for HttpCallMetrics<S, O>
where
    S: Service<Request<B>>,
    S::Response: Into<hyper::Response<O>>,
{
    type Response = hyper::Response<O>;
    type Error = S::Error;
    type Future = HttpCallMetricsFuture<S::Future, O, Self::Error>;

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
pub struct HttpCallMetricsFuture<F, O, E> {
    #[pin]
    future: F,
    tracker: HttpCallMetricTracker,
    // Required to have the trait bounds accepted.
    _phantom: PhantomData<(O, E)>,
}

impl<F, O, E> HttpCallMetricsFuture<F, O, E> {
    fn new(future: F, tracker: HttpCallMetricTracker) -> Self {
        Self {
            future,
            tracker,
            _phantom: PhantomData::default(),
        }
    }
}

impl<F, R, O, E> Future for HttpCallMetricsFuture<F, O, E>
where
    F: Future<Output = Result<R, E>>,
    R: Into<hyper::Response<O>>,
{
    type Output = Result<hyper::Response<O>, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Note that this method will be called at least twice.
        let this = self.project();
        let response = match this.future.poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(reply) => reply,
        };

        let result = match response {
            Ok(reply) => {
                let response = reply.into();
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
    path: String,
    start: Instant,
    state: Cell<ResultState>,
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
        let path = request.uri().path().to_string();
        let version = request.version();

        debug!("Start processing {version:?} {method} {path}");
        HttpMetrics::inc_in_flight(path.as_str());
        let start = Instant::now();
        Self {
            version,
            method,
            path,
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
                    path = self.path,
                    duration = duration
                );
                HttpMetrics::track(&self.path, self.method.clone(), 0, duration);
            }
            ResultState::Result(status, version) => {
                let duration = self.duration();
                debug!(
                        "Done processing {version:?} {method} {path}: {response_version:?} {response_status} - {duration:?}",
                        version = self.version,
                        method = self.method,
                        path = self.path,
                        duration = duration,
                        response_version = version,
                        response_status = status
                    );
                HttpMetrics::track(&self.path, self.method.clone(), status.as_u16(), duration);
            }
        }

        HttpMetrics::dec_in_flight(self.path.as_str());
    }
}
