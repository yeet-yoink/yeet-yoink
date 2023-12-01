#![forbid(unused_must_use)]
// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

use crate::app_config::{load_config, AppConfig};
use crate::backbone::Backbone;
use crate::backends::memcache::MemcacheBackend;
use crate::backends::BackendRegistry;
use crate::handlers::*;
use crate::tower_to_hyper_service::TowerToHyperService;
use axum::extract::Request;
use axum::routing::IntoMakeService;
use axum::Router;
use clap::ArgMatches;
use directories::ProjectDirs;
use futures_util::future::poll_fn;
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server;
use rendezvous::Rendezvous;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tower::{Service, ServiceBuilder};
use tracing::{error, info, warn};

mod app_config;
mod backbone;
mod backends;
mod commands;
mod handlers;
mod health;
mod logging;
mod metrics;
mod services;
mod tower_to_hyper_service;

#[derive(Clone)]
pub struct AppState {
    shutdown_tx: broadcast::Sender<()>,
    backbone: Arc<Backbone>,
}

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();
    let matches = commands::build_command().get_matches();
    logging::initialize_from_matches(&matches);

    info!("Hi. 👋");

    let dirs = match ProjectDirs::from("io.github", "sunsided", "yeet-yoink") {
        Some(dirs) => dirs,
        None => {
            error!("Could not determine the project directories");
            return ExitCode::FAILURE;
        }
    };

    let cfg: AppConfig = match load_config(dirs.config_local_dir(), &matches) {
        Ok(config) => config,
        Err(_) => {
            return ExitCode::FAILURE;
        }
    };

    // Provide a signal that can be used to shut down the server.
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    register_shutdown_handler(shutdown_tx.clone());

    // Create a rendezvous channel to ensure all relevant tasks have been shut down.
    let rendezvous = Rendezvous::new();

    // TODO: Create and register backends.
    let registry = BackendRegistry::builder(rendezvous.fork_guard());

    // TODO: This currently blocks if the Memcached instance is unavailable.
    //       We would prefer a solution where we can gracefully react to this in order to
    //       avoid having the service fail at runtime if Memcached becomes unresponsive.
    #[cfg(feature = "memcache")]
    let registry = match registry.add_backends::<MemcacheBackend>(&cfg) {
        Ok(registry) => registry,
        Err(_) => return ExitCode::FAILURE,
    };

    let backbone = Arc::new(Backbone::new(registry.build(), rendezvous.fork_guard()));

    // The application state is shared with the Axum servers.
    let app_state = AppState {
        shutdown_tx: shutdown_tx.clone(),
        backbone: backbone.clone(),
    };

    let exit_code = serve_requests(matches, app_state).await.err();

    // If all servers are shut down, ensure the news is broadcast as well.
    stop_all_servers(shutdown_tx);

    // TODO: Ensure registry is dropped, backbone is halted, ...
    shut_down_backbone(backbone);
    rendezvous.rendezvous_async().await.ok();

    info!("Bye. 👋");
    exit_code.unwrap_or(ExitCode::SUCCESS)
}

fn shut_down_backbone(backbone: Arc<Backbone>) {
    assert_eq!(Arc::strong_count(&backbone), 1);
}

fn stop_all_servers(shutdown_tx: broadcast::Sender<()>) {
    // We take ownership of this channel so that it'll be closed after.
    shutdown_tx.send(()).ok();
}

async fn serve_requests(matches: ArgMatches, app_state: AppState) -> Result<(), ExitCode> {
    let shutdown_tx = app_state.shutdown_tx.clone();

    let app = Router::new()
        .map_metrics_endpoint()
        .map_shutdown_endpoint()
        .map_yeet_endpoint()
        .map_yoink_endpoint()
        .map_health_endpoints()
        .with_state(app_state)
        .layer(services::HttpCallMetricsLayer::default());

    let make_svc = app.into_make_service();

    let service_builder = ServiceBuilder::new().service(make_svc);

    // Get the HTTP socket addresses to bind on.
    let http_sockets: Vec<SocketAddr> = matches
        .get_many("bind_http")
        .into_iter()
        .flatten()
        .cloned()
        .collect();

    let listeners = FuturesUnordered::new();
    for addr in http_sockets {
        let shutdown_tx = shutdown_tx.clone();
        let shutdown_rx = shutdown_tx.subscribe();

        match TcpListener::bind(&addr).await {
            Ok(listener) => {
                info!("Now listening on http://{addr}", addr = addr);
                listeners.push(listener_accept_loop(
                    listener,
                    addr.clone(),
                    shutdown_tx,
                    service_builder.clone(),
                ));
            }
            Err(e) => {
                error!("Unable to bind to {addr}: {error}", addr = addr, error = e);

                // No servers are currently running since no await was called on any
                // of them yet. Therefore, exiting here is "graceful".

                // TODO: Ensure registry is dropped, backbone is halted, ....
                return Err(ExitCode::from(exitcode::NOPERM as u8));
            }
        };
    }

    // Wait for all servers to stop.
    let mut exit_code = None;
    let mut shutdown_rx = shutdown_tx.subscribe();
    tokio::select! {
        _ = listeners.for_each(|_| async {}) => {
            error!("Listener task exited unexpectedly");
            exit_code = Some(ExitCode::FAILURE);
        },
        _ = shutdown_rx.recv() => {
            error!("Stopping condition met, exiting...");
        }
    }

    // Ensure that all other servers also shut down in presence
    // of an error of any one of them.
    shutdown_tx.send(()).ok();

    if let Some(exit_code) = exit_code {
        Err(exit_code)
    } else {
        Ok(())
    }
}

fn register_shutdown_handler(shutdown_tx: broadcast::Sender<()>) {
    ctrlc::set_handler(move || {
        warn!("Initiating shutdown from OS");
        shutdown_tx.send(()).ok();
    })
    .expect("Error setting process termination handler");
}

async fn listener_accept_loop(
    listener: TcpListener,
    addr: SocketAddr,
    mut stopping_tx: broadcast::Sender<()>,
    mut make_service: IntoMakeService<Router>,
) {
    loop {
        match listener.accept().await {
            Ok((stream, remote_addr)) => {
                info!(
                    "New connection on {}: {:?}",
                    remote_addr,
                    stream.peer_addr().unwrap()
                );

                /// This function checks readiness of a `Service` in our Axum/Tower/Hyper application.
                /// It is used to ensure the service can handle requests before they're sent. If the service isn't ready,
                /// the function pauses until it is, providing backpressure and preventing overload.
                poll_fn(|cx| {
                    <IntoMakeService<Router> as Service<Request>>::poll_ready(&mut make_service, cx)
                })
                .await
                .unwrap_or_else(|_infallible: Infallible| {});

                tokio::spawn(connection_handler(
                    stream,
                    remote_addr,
                    make_service.clone(),
                ));
            }
            Err(e) => {
                error!("Error on listener {}: {:?}", addr, e);
                let _ = stopping_tx.send(());
                break;
            }
        }
    }
}

/// Handles a TCP connection with the provided socket and remote address.
///
/// This function spawns a task to handle the connection asynchronously, allowing
/// multiple connections to be processed concurrently.
///
/// # Arguments
///
/// * `socket` - The TCP stream socket to handle.
/// * `remote_addr` - The remote address of the client.
/// * `make_service` - The factory function for creating a Tower Service that will handle the incoming request.
async fn connection_handler(
    socket: TcpStream,
    remote_addr: SocketAddr,
    mut make_service: IntoMakeService<Router>,
) {
    // Spawn a task to handle the connection. That way we can multiple connections
    // concurrently.
    tokio::spawn(async move {
        // Hyper has its own `AsyncRead` and `AsyncWrite` traits and doesn't use tokio.
        // `TokioIo` converts between them.
        let tcp_stream = TokioIo::new(socket);

        // Creates a new instance of networking service using a factory object `make_service`.
        // The factory is invoked with an `IncomingStream` representing a (fully established) incoming connection,
        // which internally encapsulates a TCP stream and the address of the remote client.
        // The service creation is asynchronous and any failure in this process is currently not handled.
        let tower_service = make_service
            .call(IncomingStream {
                tcp_stream: &tcp_stream,
                remote_addr,
            })
            .await
            .unwrap_or_else(|err| match err {});

        let hyper_service = TowerToHyperService {
            service: tower_service,
        };

        // `server::conn::auto::Builder` supports both http1 and http2.
        //
        // `TokioExecutor` tells hyper to use `tokio::spawn` to spawn tasks.
        if let Err(err) = server::conn::auto::Builder::new(TokioExecutor::new())
            // `serve_connection_with_upgrades` is required for websockets. If you don't need
            // that you can use `serve_connection` instead.
            .serve_connection_with_upgrades(tcp_stream, hyper_service)
            .await
        {
            // This error only appears when the client doesn't send a request and
            // terminate the connection.
            //
            // If client sends one request then terminate connection whenever, it doesn't
            // appear.
            error!("Failed to serve connection from {remote_addr}: {err:#}");
        }
    });
}

/// An incoming stream.
///
/// Used with [`serve`] and [`IntoMakeServiceWithConnectInfo`].
///
/// [`IntoMakeServiceWithConnectInfo`]: crate::extract::connect_info::IntoMakeServiceWithConnectInfo
#[derive(Debug)]
pub struct IncomingStream<'a> {
    tcp_stream: &'a TokioIo<TcpStream>,
    remote_addr: SocketAddr,
}

impl IncomingStream<'_> {
    /// Returns the local address that this stream is bound to.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.tcp_stream.inner().local_addr()
    }

    /// Returns the remote address that this stream is bound to.
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }
}
