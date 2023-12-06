#![forbid(unused_must_use)]
// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

use crate::app_config::{load_config, AppConfig};
use crate::backbone::Backbone;
use crate::backends::memcache::MemcacheBackend;
use crate::backends::BackendRegistry;
use crate::handlers::*;
use crate::hosting::host_on_addresses;
use axum::Router;
use clap::ArgMatches;
use directories::ProjectDirs;
use futures_util::StreamExt;
use rendezvous::{Rendezvous, RendezvousGuard};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use tokio::sync::broadcast;
use tower::ServiceBuilder;
use tracing::{debug, error, info, warn};

mod app_config;
mod backbone;
mod backends;
mod commands;
mod handlers;
mod health;
mod hosting;
mod logging;
mod metrics;
mod services;

#[derive(Clone)]
pub struct AppState {
    shutdown_tx: broadcast::Sender<()>,
    backbone: Arc<Backbone>,
    pub rendezvous: RendezvousGuard,
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
        rendezvous: rendezvous.fork_guard(),
    };

    let exit_code = serve_requests(matches, app_state, rendezvous.fork_guard())
        .await
        .err();

    // If all servers are shut down, ensure the news is broadcast as well.
    stop_all_servers(shutdown_tx);

    // TODO: Ensure registry is dropped, backbone is halted, ...
    shut_down_backbone(backbone);

    debug!("Reached final rendezvous point");
    rendezvous.rendezvous_async().await.ok();

    info!("Bye. 👋");
    exit_code.unwrap_or(ExitCode::SUCCESS)
}

fn shut_down_backbone(backbone: Arc<Backbone>) {
    // assert_eq!(Arc::strong_count(&backbone), 1);
}

fn stop_all_servers(shutdown_tx: broadcast::Sender<()>) {
    // We take ownership of this channel so that it'll be closed after.
    shutdown_tx.send(()).ok();
}

async fn serve_requests(
    matches: ArgMatches,
    app_state: AppState,
    rendezvous: RendezvousGuard,
) -> Result<(), ExitCode> {
    let shutdown_tx = app_state.shutdown_tx.clone();

    let app = Router::new()
        .map_metrics_endpoint()
        .map_shutdown_endpoint()
        .map_yeet_endpoint()
        .map_yoink_endpoint()
        .map_health_endpoints()
        .with_state(app_state)
        .layer(services::HttpCallMetricsLayer::default())
        .layer(services::ShutdownLayer::new(shutdown_tx.subscribe()));

    let make_svc = app.into_make_service();

    let service_builder = ServiceBuilder::new().service(make_svc);

    // Get the HTTP socket addresses to bind on.
    let http_sockets: Vec<SocketAddr> = matches
        .get_many("bind_http")
        .into_iter()
        .flatten()
        .cloned()
        .collect();

    let mut exit_code = None;
    match host_on_addresses(
        shutdown_tx.clone(),
        service_builder,
        http_sockets,
        rendezvous,
    )
    .await
    {
        Ok(listeners) => {
            let mut shutdown_rx = shutdown_tx.subscribe();
            // Wait for all servers to stop.
            tokio::select! {
                biased;

                _ = listeners.for_each(|_| async {}) => {
                    info!("All listeners stopped");
                },
                _ = shutdown_rx.recv() => {
                    info!("Stopping condition met, exiting...");
                }
            }
        }
        Err(error) => match error {
            HostingError::IoError(_) => exit_code = Some(ExitCode::from(exitcode::NOPERM as u8)),
        },
    };

    // Ensure that all other servers also shut down in presence
    // of an error of any one of them.
    shutdown_tx.send(()).ok();

    // TODO: Ensure registry is dropped, backbone is halted, ....

    exit_code.map_or(Ok(()), |code| Err(code))
}

#[derive(Debug, thiserror::Error)]
pub enum HostingError {
    #[error(transparent)]
    IoError(std::io::Error),
}

fn register_shutdown_handler(shutdown_tx: broadcast::Sender<()>) {
    ctrlc::set_handler(move || {
        warn!("Initiating shutdown from OS");
        shutdown_tx.send(()).ok();
    })
    .expect("Error setting process termination handler");
}
