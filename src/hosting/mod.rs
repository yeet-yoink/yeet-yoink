mod connection_handler;
mod incoming_stream;
mod tower_to_hyper_service;

use crate::hosting::connection_handler::listener_accept_loop;
use crate::HostingError;
use axum::routing::IntoMakeService;
use axum::Router;
use futures_util::stream::FuturesUnordered;
use rendezvous::RendezvousGuard;
use std::future::Future;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::sync::broadcast::Sender;
use tracing::{error, info};

/// Asynchronously host the service on the specified addresses.
///
/// This function binds a TCP listener to each provided address and starts a
/// listener_accept_loop task for each listener. It returns a `FuturesUnordered`
/// object that represents a collection of all the listener_accept_loop tasks.
///
/// # Arguments
///
/// * `shutdown_tx` - A reference to the sender for the shutdown signal. This is used
///   to gracefully shut down the server.
/// * `service_builder` - An object that can build a service to handle HTTP requests.
/// * `http_sockets` - A vector of SocketAddr objects representing the addresses
///   on which to host the service.
///
/// # Returns
///
/// A Result object that either contains the collection of listener_accept_loop tasks
/// or an error of type HostingError.
pub async fn host_on_addresses(
    shutdown_tx: Sender<()>,
    service_builder: IntoMakeService<Router>,
    http_sockets: Vec<SocketAddr>,
    rendezvous: RendezvousGuard,
) -> Result<FuturesUnordered<impl Future<Output = ()> + Sized>, HostingError> {
    let listeners = FuturesUnordered::new();
    for addr in http_sockets {
        match TcpListener::bind(&addr).await {
            Ok(listener) => {
                info!("Now listening on http://{addr}", addr = addr);
                listeners.push(listener_accept_loop(
                    listener,
                    addr.clone(),
                    service_builder.clone(),
                    shutdown_tx.clone(),
                    rendezvous.fork(),
                ));
            }
            Err(e) => {
                error!("Unable to bind to {addr}: {error}", addr = addr, error = e);

                // No servers are currently running since no await was called on any
                // of them yet. Therefore, exiting here is "graceful".
                rendezvous.completed();
                return Err(HostingError::IoError(e));
            }
        };
    }

    rendezvous.completed();
    Ok(listeners)
}
