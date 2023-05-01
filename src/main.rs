use crate::bind::bind_tcp_sockets;
use crate::metrics::http_api::{with_end_call_metrics, with_start_call_metrics};
use hyper::Server;
use std::convert::Infallible;
use std::future::{ready, Ready};
use std::net::{SocketAddr, TcpListener};
use std::process::ExitCode;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{error, info, warn};
use warp::http::{Request, Response};
use warp::hyper::service::{make_service_fn, Service};
use warp::hyper::Body;
use warp::{Filter, Rejection, Reply};

mod bind;
mod commands;
mod health;
mod logging;
mod metrics;

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();
    let matches = commands::build_command().get_matches();
    logging::initialize_from_matches(&matches);

    // Provide a signal that can be used to shut down the server.
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

    info!("Hi. 👋");

    // GET /hello/warp => 200 OK with body "Hello, warp!"
    let hello = warp::path!("hello" / String).and_then(hello);

    // GET /hello/ => 200 OK with body "Hello World!"
    // let hello_world = warp::path!("hello").and_then(Hello);

    // GET /slow => a slow requests
    let slow = warp::path!("slow").and_then(slow);

    /*
    let streams = match bind_tcp_sockets(&matches).await {
        Ok(s) => s,
        Err(_e) => {
            // error is already logged
            return ExitCode::from(exitcode::NOPERM as u8);
        }
    };
     */

    /*
    let filter = with_start_call_metrics()
        .and(
            hello
                // .or(hello_world)
                .or(slow)
                .or(metrics::http_api::metrics_endpoint())
                .or(health::http_api::health_endpoints())
                .or(shutdown),
        )
        // TODO: If the call (e.g. of `slow`) is cancelled, this is never reached.
        .with(with_end_call_metrics());
     */

    // Typical hyper setup...
    let make_svc = make_service_fn(move |_| {
        let tx = shutdown_tx.clone();
        async move {
            Ok::<_, Infallible>(warp::service(
                hello
                    // .or(hello_world)
                    .or(slow)
                    .or(metrics::http_api::metrics_endpoint())
                    .or(health::http_api::health_endpoints())
                    .or(shutdown_route(tx)),
            ))
        }
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    let listener = TcpListener::bind(addr).unwrap();

    let server = Server::from_tcp(listener).unwrap().serve(make_svc);

    let server = server.with_graceful_shutdown(async move {
        shutdown_rx.recv().await.ok();
    });

    match server.await {
        Ok(()) => {
            info!("Bye. 👋");
            ExitCode::SUCCESS
        }
        Err(e) => {
            error!("Server error: {}", e);
            ExitCode::FAILURE
        }
    }
}

/// Responds with the caller's name.
///
/// ```http
/// GET /hello/name
/// ```
async fn hello(name: String) -> Result<impl Reply, Rejection> {
    Ok(format!("Hello, {}!", name))
}

/// Responds with the caller's name.
///
/// ```http
/// GET /hello/name
/// ```
async fn slow() -> Result<impl Reply, Rejection> {
    tokio::time::sleep(Duration::from_secs(5)).await;
    Ok(format!("That was slow."))
}

/// Initiates a graceful shutdown.
///
/// ```http
/// POST /stop
/// ```
async fn shutdown(tx: broadcast::Sender<()>) -> Result<impl Reply, Rejection> {
    warn!("Initiating shutdown from API call");
    tx.send(()).ok();
    Ok(warp::reply::reply())
}

/// Builds the route for the [`shutdown`] handler.
fn shutdown_route(
    tx: broadcast::Sender<()>,
) -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
    let shutdown_filter = warp::any().map(move || tx.clone());

    // POST /stop to shut down the server.
    warp::post()
        .and(warp::path("stop"))
        .and(warp::path::end())
        .and(shutdown_filter)
        .and_then(shutdown)
}

struct Hello;

impl Service<Request<Body>> for Hello {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // We produce our result right away.
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: Request<Body>) -> Self::Future {
        ready(Ok(Response::new(Body::from("Hello world"))))
    }
}
