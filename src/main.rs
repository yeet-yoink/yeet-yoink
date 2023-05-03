use crate::bind::bind_tcp_sockets;
use hyper::Server;
use std::convert::Infallible;
use std::net::{SocketAddr, TcpListener};
use std::process::ExitCode;
use std::time::Duration;
use tokio::sync::broadcast;
use tower::ServiceBuilder;
use tracing::{error, info};
use warp::hyper::service::{make_service_fn, Service};
use warp::{Filter, Rejection, Reply};

mod bind;
mod commands;
mod filters;
mod health;
mod logging;
mod metrics;
mod services;

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

    let make_svc = make_service_fn(|_conn| {
        let tx = shutdown_tx.clone();

        async move {
            let svc = warp::service(
                hello
                    .or(slow)
                    .or(filters::metrics_endpoint())
                    .or(filters::health_endpoints())
                    .or(filters::shutdown_endpoint(tx)),
            );

            let svc = services::HttpCallMetrics::new(svc);
            Ok::<_, Infallible>(svc)
        }
    });

    let builder = ServiceBuilder::new().service(make_svc);

    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    let listener = TcpListener::bind(addr).unwrap();

    let server = Server::from_tcp(listener)
        .unwrap()
        .serve(builder)
        .with_graceful_shutdown(async move {
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
