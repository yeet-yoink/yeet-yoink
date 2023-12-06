use axum::body::HttpBody;
use axum::extract::Request;
use axum::response::IntoResponse;
use pin_project::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::broadcast;
use tower_service::Service;

#[derive(Clone)]
pub struct ShutdownLayer {
    shutdown: Arc<AtomicBool>,
}

impl ShutdownLayer {
    pub fn new(mut shutdown: broadcast::Receiver<()>) -> Self {
        let signal = Arc::new(AtomicBool::new(false));
        tokio::spawn({
            let signal = signal.clone();
            async move {
                let _ = shutdown.recv().await;
                signal.store(true, Ordering::SeqCst);
            }
        });

        Self { shutdown: signal }
    }
}

impl<S> tower::layer::Layer<S> for ShutdownLayer {
    type Service = ShutdownService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ShutdownService {
            inner,
            shutdown: self.shutdown.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ShutdownService<S> {
    inner: S,
    shutdown: Arc<AtomicBool>,
}

impl<S, B> Service<Request<B>> for ShutdownService<S>
where
    S: Service<axum::http::Request<B>>,
    S::Response: IntoResponse,
    B: HttpBody,
{
    type Response = axum::response::Response;
    type Error = S::Error;
    type Future = ShutdownServiceFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<B>) -> Self::Future {
        ShutdownServiceFuture::new(self.inner.call(request), self.shutdown.clone())
    }
}

#[pin_project]
pub struct ShutdownServiceFuture<F>
where
    F: Future,
{
    #[pin]
    future: F,
    shutdown: Arc<AtomicBool>,
}

impl<F> ShutdownServiceFuture<F>
where
    F: Future,
{
    fn new(future: F, shutdown: Arc<AtomicBool>) -> Self {
        Self { future, shutdown }
    }
}

impl<F, R, E> Future for ShutdownServiceFuture<F>
where
    F: Future<Output = Result<R, E>>,
    R: IntoResponse,
{
    type Output = Result<axum::response::Response, E>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        // If shutdown received then return with a error response
        if this.shutdown.load(Ordering::SeqCst) {
            // TODO: The connection remains open ...
            return Poll::Ready(Ok((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "Service is shutting down",
            )
                .into_response()));
        }

        // Default behavior
        let response = match this.future.poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(reply) => reply,
        };

        let result = match response {
            Ok(reply) => {
                let response = reply.into_response();
                Ok(response)
            }
            Err(e) => Err(e),
        };
        Poll::Ready(result)
    }
}
