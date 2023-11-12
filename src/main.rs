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
use directories::ProjectDirs;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use hyper::Server;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::broadcast::Sender;
use tower::ServiceBuilder;
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

#[derive(Clone)]
pub struct AppState {
    shutdown_tx: Sender<()>,
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

    // TODO: Create and register backends.
    let mut registry = BackendRegistry::default();

    // TODO: This currently blocks if the Memcached instance is unavailable.
    //       We would prefer a solution where we can gracefully react to this in order to
    //       avoid having the service fail at runtime if Memcached becomes unresponsive.
    #[cfg(feature = "memcache")]
    if let Err(e) = MemcacheBackend::register(&mut registry, &cfg) {
        error!("Failed to initialize Memcached backends: {}", e);
        return ExitCode::FAILURE;
    };

    let backbone = Backbone::default();

    // The application state is shared with the Axum servers.
    let app_state = AppState {
        shutdown_tx: shutdown_tx.clone(),
        backbone: Arc::new(backbone),
    };

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
                return ExitCode::from(exitcode::NOPERM as u8);
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
