#![forbid(unused_must_use)]
// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

use crate::app_config::{load_config, AppConfig};
use crate::backbone::Backbone;
use crate::backends::memcache::MemcacheBackend;
use crate::backends::{BackendRegistry, TryCreateFromConfig};
use crate::handlers::*;
use axum::Router;
use clap::ArgMatches;
use directories::ProjectDirs;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use hyper::Server;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::{broadcast, mpsc};
use tower::ServiceBuilder;
use tracing::{debug, error, info, warn};

mod app_config;
mod backbone;
mod backends;
mod commands;
mod handlers;
mod health;
mod logging;
mod metrics;
mod services;

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
    let (rendezvous_tx, mut rendezvous_rx) = mpsc::channel(16);

    // TODO: Create and register backends.
    let mut registry = BackendRegistry::new(rendezvous_tx.clone());

    // TODO: This currently blocks if the Memcached instance is unavailable.
    //       We would prefer a solution where we can gracefully react to this in order to
    //       avoid having the service fail at runtime if Memcached becomes unresponsive.
    #[cfg(feature = "memcache")]
    if let Err(e) = MemcacheBackend::register(&mut registry, &cfg) {
        error!("Failed to initialize Memcached backends: {}", e);
        return ExitCode::FAILURE;
    };

    let backbone = Arc::new(Backbone::new(registry, rendezvous_tx.clone()));

    // The application state is shared with the Axum servers.
    let app_state = AppState {
        shutdown_tx: shutdown_tx.clone(),
        backbone: backbone.clone(),
    };

    let exit_code = serve_requests(matches, app_state).await.err();

    // If all servers are shut down, ensure the news is broadcast as well.
    stop_all_servers(shutdown_tx, rendezvous_tx);

    // TODO: Ensure registry is dropped, backbone is halted, ...
    shut_down_backbone(backbone);
    rendezvous_workers(rendezvous_rx).await;

    info!("Bye. 👋");
    exit_code.unwrap_or(ExitCode::SUCCESS)
}

fn shut_down_backbone(backbone: Arc<Backbone>) {
    assert_eq!(Arc::strong_count(&backbone), 1);
}

async fn rendezvous_workers(mut rendezvous_rx: mpsc::Receiver<CleanupRendezvous>) {
    // Wait for all services to shut down.
    while let Some(event) = rendezvous_rx.recv().await {
        match event {
            CleanupRendezvous::BackendRegistry => debug!("Backend registry worker thread finished"),
            CleanupRendezvous::Backbone => debug!("Backbone worker thread finished"),
        }
    }
    info!("Shutdown rendezvous completed");
}

fn stop_all_servers(shutdown_tx: broadcast::Sender<()>, rendezvous_tx: Sender<CleanupRendezvous>) {
    // We take ownership of this channel so that it'll be closed after.
    shutdown_tx.send(()).ok();

    // We take ownership to ensure the rendezvous channel is closed.
    let _ = rendezvous_tx;
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

    let mut servers = FuturesUnordered::new();
    for addr in http_sockets {
        let mut shutdown_rx = shutdown_tx.subscribe();

        let builder = match Server::try_bind(&addr) {
            Ok(builder) => {
                info!("Now listening on http://{addr}", addr = addr);
                builder
            }
            Err(e) => {
                error!("Unable to bind to {addr}: {error}", addr = addr, error = e);

                // No servers are currently running since no await was called on any
                // of them yet. Therefore, exiting here is "graceful".

                // TODO: Ensure registry is dropped, backbone is halted, ....
                return Err(ExitCode::from(exitcode::NOPERM as u8));
            }
        };

        let server = builder
            .serve(service_builder.clone())
            .with_graceful_shutdown(async move {
                shutdown_rx.recv().await.ok();
            });

        servers.push(server);
    }

    // Wait for all servers to stop.
    let mut exit_code = None;
    while let Some(result) = servers.next().await {
        match result {
            Ok(()) => {
                debug!("A server stopped")
            }
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

pub enum CleanupRendezvous {
    BackendRegistry,
    Backbone,
}
