use crate::bind::bind_tcp_sockets;
use crate::metrics::http_api::{with_end_call_metrics, with_start_call_metrics};
use std::process::ExitCode;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::info;
use warp::http::Response;
use warp::hyper::Body;
use warp::Filter;

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
    let shutdown_filter = warp::any().map(move || shutdown_tx.clone());

    info!("Hi. 👋");

    // GET /hello/warp => 200 OK with body "Hello, warp!"
    let hello = warp::path!("hello" / String).and_then(hello);

    // GET /slow => a slow requests
    let slow = warp::path!("slow").and_then(slow);

    // POST /stop to shut down the server.
    let shutdown = warp::post()
        .and(warp::path("stop"))
        .and(warp::path::end())
        .and(shutdown_filter)
        .and_then(shutdown);

    let streams = match bind_tcp_sockets(&matches).await {
        Ok(s) => s,
        Err(_e) => {
            // error is already logged
            return ExitCode::from(exitcode::NOPERM as u8);
        }
    };

    warp::serve(
        with_start_call_metrics()
            .and(
                hello
                    .or(slow)
                    .or(metrics::http_api::metrics_endpoint())
                    .or(health::http_api::health_endpoints())
                    .or(shutdown),
            )
            // TODO: If the call (e.g. of `slow`) is cancelled, this is never reached.
            .with(with_end_call_metrics()),
    )
    .serve_incoming_with_graceful_shutdown(streams, async move {
        shutdown_rx.recv().await.ok();
    })
    .await;

    info!("Bye. 👋");
    ExitCode::SUCCESS
}

/// Responds with the caller's name.
///
/// ```http
/// GET /hello/name
/// ```
async fn hello(name: String) -> Result<impl warp::Reply, warp::Rejection> {
    Ok(format!("Hello, {}!", name))
}

/// Responds with the caller's name.
///
/// ```http
/// GET /hello/name
/// ```
async fn slow() -> Result<impl warp::Reply, warp::Rejection> {
    tokio::time::sleep(Duration::from_secs(5)).await;
    Ok(format!("That was slow."))
}

/// Initiates a graceful shutdown.
///
/// ```http
/// POST /stop
/// ```
async fn shutdown(tx: broadcast::Sender<()>) -> Result<impl warp::Reply, warp::Rejection> {
    tx.send(()).ok();
    Ok(warp::reply::reply())
}
