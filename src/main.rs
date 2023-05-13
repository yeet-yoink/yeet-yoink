use futures::stream::FuturesUnordered;
use futures::StreamExt;
use hyper::Server;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::broadcast::Sender;
use tower::ServiceBuilder;
use tracing::{error, info, warn};
use warp::hyper::service::make_service_fn;
use warp::{Filter, Rejection, Reply};

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
    info!("Hi. 👋");

    // Provide a signal that can be used to shut down the server.
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    register_shutdown_handler(shutdown_tx.clone());

    // GET /hello/warp => 200 OK with body "Hello, warp!"
    let hello = warp::path!("hello" / String).and_then(hello);

    // GET /slow => a slow requests
    let slow = warp::path!("slow").and_then(slow);

    let make_svc = make_service_fn(|_conn| {
        let tx = shutdown_tx.clone();

        async move {
            let svc = warp::service(
                hello
                    .or(slow)
                    .or(filters::yeet_endpoint())
                    .or(filters::metrics_endpoint())
                    .or(filters::health_endpoints())
                    .or(filters::shutdown_endpoint(tx)),
            );

            let svc = services::HttpCallMetrics::new(svc);
            Ok::<_, Infallible>(svc)
        }
    });

    let service_builder = ServiceBuilder::new().service(make_svc);

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

        let builder = match Server::try_bind(&addr) {
            Ok(builder) => builder,
            Err(e) => {
                error!("Unable to bind to {addr}: {error}", addr = addr, error = e);

                // No servers are currently running since no await was called on any
                // of them yet. Therefore, exiting here is "graceful".
                return ExitCode::from(exitcode::NOPERM as u8);
            }
        };

        let server = builder
            .serve(service_builder)
            .with_graceful_shutdown(async move {
                shutdown_rx.recv().await.ok();
            });

        servers.push(server);
    }

    // Wait for all servers to stop.
    let mut exit_code = None;
    while let Some(result) = servers.next().await {
        match result {
            Ok(()) => {}
            Err(e) => {
                error!("Server error: {}", e);

                // Apply better error code if known.
                if exit_code.is_none() {
                    exit_code = Some(ExitCode::FAILURE);
                }
            }
        }

        // Ensure that all other servers also shut down in presence
        // of an error of any one of them.
        shutdown_tx.send(()).ok();
    }

    if let Some(error_code) = exit_code {
        return error_code;
    }

    info!("Bye. 👋");
    ExitCode::SUCCESS
}

fn register_shutdown_handler(shutdown_tx: Sender<()>) {
    ctrlc::set_handler(move || {
        warn!("Initiating shutdown from OS");
        shutdown_tx.send(()).ok();
    })
    .expect("Error setting process termination handler");
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
