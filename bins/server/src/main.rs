#![forbid(unused_must_use)]
// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

use crate::handlers::*;
use app_config::AppConfig;
use axum::Router;
use backbone::{Backbone, FileAccessorBridge};
use clap::ArgMatches;
use directories::ProjectDirs;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use rendezvous::Rendezvous;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use crate::backend_registry::{BackendFetcher, BackendRegistry};
#[cfg(feature = "azure")]
use backend_azure::AzureBackend;
#[cfg(feature = "gcs")]
use backend_gcs::GcsBackend;
#[cfg(feature = "memcache")]
use backend_memcache::MemcacheBackend;
#[cfg(feature = "valkey")]
use backend_valkey::ValkeyBackend;
use file_distribution::FileProvider;

mod backend_registry;
mod cluster;
mod commands;
mod handlers;
mod health;
mod logging;
mod services;
#[cfg(test)]
mod test_support;

#[derive(Clone)]
pub struct AppState {
    shutdown_tx: broadcast::Sender<()>,
    backbone: Arc<Backbone>,
    /// Fetches files that are not (or no longer) buffered locally.
    backends: BackendFetcher,
    /// Whether files fetched from a backend are buffered locally again.
    near_cache: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    dotenvy::dotenv().ok();
    let matches = commands::build_command().get_matches();
    logging::initialize_from_matches(&matches);

    info!("Hi. 👋");

    let dirs = match ProjectDirs::from("io.github", "yeet-yoink", "yeet-yoink") {
        Some(dirs) => dirs,
        None => {
            error!("Could not determine the project directories");
            return ExitCode::FAILURE;
        }
    };

    let cfg = match AppConfig::load(dirs.config_local_dir(), &matches) {
        Ok(config) => config,
        Err(_) => {
            return ExitCode::FAILURE;
        }
    };

    // Join the cluster of instances, announcing the first HTTP socket.
    let http_port = matches
        .get_many::<SocketAddr>("bind_http")
        .into_iter()
        .flatten()
        .next()
        .map(SocketAddr::port);
    let cluster = match &cfg.cluster {
        Some(config) => match cluster::join(config, http_port).await {
            Ok(cluster) => {
                cluster::log_membership_changes(&cluster).await;
                Some(cluster)
            }
            Err(e) => {
                error!("Failed to join the cluster: {e:#}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };

    // Provide a signal that can be used to shut down the server.
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    register_shutdown_handler(shutdown_tx.clone());

    // Create a rendezvous channel to ensure all relevant tasks have been shut down.
    let rendezvous = Rendezvous::new();

    let file_accessor = Arc::new(FileAccessorBridge::default());

    // TODO: Create and register backends.
    let registry =
        BackendRegistry::builder(rendezvous.fork_guard(), FileProvider::wrap(&file_accessor));

    // TODO: This currently blocks if the Memcached instance is unavailable.
    //       We would prefer a solution where we can gracefully react to this in order to
    //       avoid having the service fail at runtime if Memcached becomes unresponsive.
    #[cfg(feature = "memcache")]
    let registry = match registry.add_backends::<MemcacheBackend>(&cfg) {
        Ok(registry) => registry,
        Err(_) => return ExitCode::FAILURE,
    };

    // Valkey backends connect lazily on first use and reconnect on failure.
    #[cfg(feature = "valkey")]
    let registry = match registry.add_backends::<ValkeyBackend>(&cfg) {
        Ok(registry) => registry,
        Err(_) => return ExitCode::FAILURE,
    };

    // Cloud Storage backends make no request until they are used.
    #[cfg(feature = "gcs")]
    let registry = match registry.add_backends::<GcsBackend>(&cfg) {
        Ok(registry) => registry,
        Err(_) => return ExitCode::FAILURE,
    };

    // Azure Blob Storage backends make no request until they are used.
    #[cfg(feature = "azure")]
    let registry = match registry.add_backends::<AzureBackend>(&cfg) {
        Ok(registry) => registry,
        Err(_) => return ExitCode::FAILURE,
    };

    let registry = registry.build();
    let backend_sender = registry.get_sender().expect("failed to get backend sender");

    let backbone = Arc::new(Backbone::new(
        backend_sender,
        rendezvous.fork_guard(),
        cfg.cache.lease_duration(),
    ));
    file_accessor.set_backbone(&backbone);

    // The application state is shared with the Axum servers.
    let app_state = AppState {
        shutdown_tx: shutdown_tx.clone(),
        backbone: backbone.clone(),
        backends: registry.fetcher(),
        near_cache: cfg.cache.near_cache,
    };

    let exit_code = serve_requests(matches, app_state).await.err();

    // If all servers are shut down, ensure the news is broadcast as well.
    stop_all_servers(shutdown_tx);

    if let Some(cluster) = cluster
        && let Err(e) = cluster.leave().await
    {
        warn!("Failed to leave the cluster: {e:#}");
    }

    // TODO: Ensure registry is dropped, backbone is halted, ...
    shut_down_backbone(backbone);
    rendezvous.rendezvous_async().await;

    info!("Bye. 👋");
    exit_code.unwrap_or(ExitCode::SUCCESS)
}

/// Drops the last reference to the backbone, which ends all pending file leases.
fn shut_down_backbone(backbone: Arc<Backbone>) {
    assert_eq!(Arc::strong_count(&backbone), 1);
}

fn router(app_state: AppState) -> Router {
    Router::new()
        .map_metrics_endpoint()
        .map_shutdown_endpoint()
        .map_yeet_endpoint()
        .map_yoink_endpoint()
        .map_health_endpoints()
        .with_state(app_state)
        .layer(services::HttpCallMetricsLayer)
}

fn stop_all_servers(shutdown_tx: broadcast::Sender<()>) {
    // We take ownership of this channel so that it'll be closed after.
    shutdown_tx.send(()).ok();
}

async fn serve_requests(matches: ArgMatches, app_state: AppState) -> Result<(), ExitCode> {
    let shutdown_tx = app_state.shutdown_tx.clone();

    let app = router(app_state);

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

        let listener = match TcpListener::bind(addr).await {
            Ok(listener) => {
                info!("Now listening on http://{addr}", addr = addr);
                listener
            }
            Err(e) => {
                error!("Unable to bind to {addr}: {error}", addr = addr, error = e);

                // No servers are currently running since no await was called on any
                // of them yet. Therefore, exiting here is "graceful".

                // TODO: Ensure registry is dropped, backbone is halted, ....
                return Err(ExitCode::from(exitcode::NOPERM as u8));
            }
        };

        let app = app.clone();
        let server = async move {
            axum::serve(listener, app.into_make_service())
                .with_graceful_shutdown(async move {
                    shutdown_rx.recv().await.ok();
                })
                .await
        };

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
