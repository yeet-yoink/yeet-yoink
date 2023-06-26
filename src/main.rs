#![forbid(unused_must_use)]

use crate::backbone::Backbone;
use crate::handlers::*;
use axum::Router;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use hyper::Server;
use libp2p::floodsub::Topic;
use libp2p::gossipsub::{self, IdentTopic, MessageAuthenticity};
use libp2p::mdns::Event;
use libp2p::swarm::{keep_alive, NetworkBehaviour, SwarmBuilder, SwarmEvent};
use libp2p::{floodsub, identity, mdns, ping, Multiaddr, PeerId, Transport};
use serde::{Deserialize, Serialize};
use shortguid::ShortGuid;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
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

/// Our network behaviour.
///
/// For illustrative purposes, this includes the [`KeepAlive`](behaviour::KeepAlive) behaviour so a continuous sequence of
/// pings can be observed.
#[derive(NetworkBehaviour)]
struct Behaviour {
    keep_alive: keep_alive::Behaviour,
    ping: ping::Behaviour,
    mdns: mdns::tokio::Behaviour,
    floodsub: floodsub::Floodsub,
    gossip: gossipsub::Behaviour,
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

    // The transport defines how to send data.
    // TODO: Replace with custom transport creation.
    let transport = libp2p::tokio_development_transport(local_key.clone())?;

    // mDNS
    let mdns = mdns::tokio::Behaviour::new(
        mdns::Config {
            enable_ipv6: false,
            ttl: Duration::from_secs(300),
            query_interval: Duration::from_secs(300),
        },
        local_peer_id,
    )?;

    let gossip = gossipsub::Behaviour::new(
        MessageAuthenticity::Signed(local_key),
        gossipsub::Config::default(),
    )?;

    let floodsub = floodsub::Floodsub::new(local_peer_id);

    // The behavior defines what data to send.
    let behaviour = Behaviour {
        keep_alive: Default::default(),
        ping: Default::default(),
        mdns,
        floodsub,
        gossip,
    };

    // The swarm combines the transport with the behavior, driving both.
    let mut swarm = SwarmBuilder::with_tokio_executor(transport, behaviour, local_peer_id).build();

    // Subscribe to the file topic.
    let gossip_file_topic = IdentTopic::new("file");
    let flood_file_topic = Topic::new("file");

    assert!(swarm.behaviour_mut().gossip.subscribe(&gossip_file_topic)?);
    assert!(swarm
        .behaviour_mut()
        .floodsub
        .subscribe(flood_file_topic.clone()));

    // Tell the swarm to listen on all interfaces and a random, OS-assigned port.
    let addr: Multiaddr = "/ip4/0.0.0.0/tcp/0".parse()?;
    swarm.listen_on(addr)?;

    // Dial the peer identified by the multi-address given as the second
    // command-line argument, if any.
    if let Some(addr) = matches.get_one::<String>("dial") {
        let remote: Multiaddr = addr.parse()?;
        swarm.dial(remote)?;
        println!("Dialed {addr}")
    }

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => info!("Listening on {address}"),
            SwarmEvent::Behaviour(event) => match event {
                BehaviourEvent::KeepAlive(keep_alive) => {
                    info!("Swarm event: Keep Alive {keep_alive:?}")
                }
                BehaviourEvent::Ping(ping) => info!("Swarm event: Keep Alive {ping:?}"),
                BehaviourEvent::Mdns(mdns) => {
                    match mdns {
                        Event::Discovered(peer) => {
                            info!("Swarm event: mDNS: discovered {peer:?}");
                        }
                        Event::Expired(peer) => info!("Swarm event: mDNS: expired {peer:?}"),
                    };
                }
                BehaviourEvent::Gossip(gossip) => {
                    info!("Swarm event: gossip {gossip:?}");

                    // TODO: This doesn't work?
                    swarm
                        .behaviour_mut()
                        .floodsub
                        .publish(flood_file_topic.clone(), vec![0, 1, 2, 3, 4]);

                    // TODO: This now creates an endless loop of sending and receiving between pairs.
                    swarm
                        .behaviour_mut()
                        .gossip
                        .publish(gossip_file_topic.clone(), vec![10, 11, 12, 13, 14])?;
                }
                BehaviourEvent::Floodsub(floodsub) => info!("Swarm event: Floodsub {floodsub:?}"),
            },
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                info!("Connection established: {peer_id}")
            }
            SwarmEvent::ConnectionClosed { peer_id, .. } => info!("Connection closed: {peer_id}"),
            SwarmEvent::IncomingConnection { .. } => info!("Incoming connection"),
            SwarmEvent::IncomingConnectionError { error, .. } => {
                warn!("Incoming connection error: {error}")
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                warn!("Outgoing connection error for {peer_id:?}: {error}")
            }
            SwarmEvent::ExpiredListenAddr { address, .. } => {
                warn!("Expired listen address: {address}")
            }
            SwarmEvent::ListenerClosed { .. } => {
                info!("Listener closed")
            }
            SwarmEvent::ListenerError { error, .. } => warn!("Listener error: {error}"),
            SwarmEvent::Dialing {
                peer_id,
                connection_id,
            } => info!("Dialing {peer_id:?} on {connection_id:?} ..."),
        }
    }

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
