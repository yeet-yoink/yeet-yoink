#![forbid(unused_must_use)]

use crate::backbone::Backbone;
use crate::handlers::*;
use axum::Router;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use hyper::Server;
use libp2p::core::upgrade;
use libp2p::{identity, noise, tcp, PeerId, Transport};
use serde::{Deserialize, Serialize};
use shortguid::ShortGuid;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::broadcast::Sender;
use tower::ServiceBuilder;
use tracing::{error, info, warn};

mod backbone;
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

#[derive(Debug, Serialize, Deserialize)]
struct FileMetadata {
    id: ShortGuid,
}

#[derive(Debug, Serialize, Deserialize)]
struct MetadataAnnounce {
    metadata: FileMetadata,
}

#[tokio::main]
async fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let matches = commands::build_command().get_matches();

    logging::initialize_from_matches(&matches);
    info!("Hi. 👋");

    info!("Generating Ed25519 keypair ...");
    let local_key = identity::Keypair::generate_ed25519();
    let local_peer_id = PeerId::from(local_key.public());
    info!("Local peer ID: {local_peer_id}");

    // TODO: Replace with custom transport creation.
    let transport = libp2p::tokio_development_transport(local_key)?;

    // Provide a signal that can be used to shut down the server.
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    register_shutdown_handler(shutdown_tx.clone());

    let app_state = AppState {
        shutdown_tx: shutdown_tx.clone(),
        backbone: Arc::new(Backbone::default()),
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
        info!("Binding to {addr}", addr = addr);
        let mut shutdown_rx = shutdown_tx.subscribe();

        let builder = match Server::try_bind(&addr) {
            Ok(builder) => builder,
            Err(e) => {
                error!("Unable to bind to {addr}: {error}", addr = addr, error = e);

                // No servers are currently running since no await was called on any
                // of them yet. Therefore, exiting here is "graceful".
                return Ok(ExitCode::from(exitcode::NOPERM as u8));
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
        return Ok(error_code);
    }

    info!("Bye. 👋");
    Ok(ExitCode::SUCCESS)
}

fn register_shutdown_handler(shutdown_tx: Sender<()>) {
    ctrlc::set_handler(move || {
        warn!("Initiating shutdown from OS");
        shutdown_tx.send(()).ok();
    })
    .expect("Error setting process termination handler");
}
