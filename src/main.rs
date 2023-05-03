use crate::bind::{bind_tcp_sockets, BindError};
use clap::ArgMatches;
use futures::stream::FuturesUnordered;
use futures::{StreamExt, TryFutureExt};
use hyper::Server;
use std::convert::Infallible;
use std::net::{SocketAddr, TcpListener};
use std::process::ExitCode;
use std::time::Duration;
use tokio::sync::broadcast;
use tower::ServiceBuilder;
use tracing::{debug, error, info, trace};
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

    // Get the HTTP socket addresses to bind on.
    let http_sockets: Vec<SocketAddr> = matches
        .get_many("bind_http")
        .into_iter()
        .flatten()
        .cloned()
        .collect();

    let mut servers = FuturesUnordered::new();
    for addr in http_sockets {
        info!("Binding to {addr}", addr = addr);
        let mut shutdown_rx = shutdown_tx.subscribe();
        // TODO: This panics now if the address is already in use.
        let server = Server::bind(&addr)
            .serve(builder)
            .with_graceful_shutdown(async move {
                shutdown_rx.recv().await.ok();
            });

        servers.push(server);
    }

    let mut server_error = false;
    while let Some(result) = servers.next().await {
        match result {
            Ok(()) => {}
            Err(e) => {
                server_error = true;
                error!("Server error: {}", e)
            }
        }

        // Ensure that all other servers also shut down in presence
        // of an error of any one of them.
        shutdown_tx.send(()).ok();
    }

    if server_error {
        ExitCode::FAILURE
    } else {
        info!("Bye. 👋");
        ExitCode::SUCCESS
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
