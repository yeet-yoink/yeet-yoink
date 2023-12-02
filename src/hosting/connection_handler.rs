use crate::hosting::incoming_stream::IncomingStream;
use crate::hosting::tower_to_hyper_service::TowerToHyperService;
use axum::extract::Request;
use axum::routing::IntoMakeService;
use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server;
use rendezvous::RendezvousGuard;
use std::convert::Infallible;
use std::future::poll_fn;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream};
use tokio::select;
use tokio::sync::broadcast;
use tower_service::Service;
use tracing::{debug, error, info, trace};

/// Runs a loop that listens for incoming TCP connections, accepts them, and handles them
/// using the provided make_service function.
///
/// # Arguments
///
/// * `listener` - The TCP listener used to accept incoming connections.
/// * `addr` - The socket address of the listener.
/// * `stopping_rx` - A broadcaster used to receive a signal to stop the loop.
/// * `make_service` - The function used to create a service for handling each connection.
/// * `rendezvous` - The shutdown rendezvous guard.
///
/// # Examples
///
/// ```rust
/// use axum::handler::get;
/// use axum::Router;
/// use tokio::net::TcpListener;
///
/// let listener = TcpListener::bind("127.0.0.1:3000").await.unwrap();
/// let addr = listener.local_addr().unwrap();
/// let (stopping_tx, _) = tokio::sync::broadcast::channel(1);
/// let router = Router::new().route("/", get(|| async { "Hello, World!" }));
///
/// listener_accept_loop(listener, addr, stopping_tx, router.into_make_service());
/// ```
pub async fn listener_accept_loop(
    listener: TcpListener,
    addr: SocketAddr,
    mut make_service: IntoMakeService<Router>,
    stopping_tx: broadcast::Sender<()>,
    rendezvous: RendezvousGuard,
) {
    let stop_handle = stopping_tx.clone();
    let mut stopping_rx = stopping_tx.subscribe();
    'accept: loop {
        select! {
            biased;

            _ = stopping_rx.recv() => {
                debug!("Shutdown signal received in accept loop for {addr}");
                break
            },
            result = listener.accept() => match result {
                Ok((stream, remote_addr)) => {
                    info!(
                        "New connection on {}: {:?}",
                        addr,
                        remote_addr
                    );

                    // This function checks readiness of a `Service` in our Axum/Tower/Hyper application.
                    // It is used to ensure the service can handle requests before they're sent. If the service isn't ready,
                    // the function pauses until it is, providing backpressure and preventing overload.
                    poll_fn(|cx| {
                        <IntoMakeService<Router> as Service<Request>>::poll_ready(&mut make_service, cx)
                    })
                    .await
                    .unwrap_or_else(|_infallible: Infallible| {});

                    tokio::spawn(handle_connection(
                        stream,
                        remote_addr,
                        make_service.clone(),
                        stop_handle.subscribe(),
                        rendezvous.fork()
                    ));
                }
                Err(e) => {
                    error!("Error on listener {}: {:?}", addr, e);
                    break 'accept;
                }
            }
        }
    }

    info!("Stopped accepting connections on {addr}");
    rendezvous.completed();
}

/// Handles a TCP connection with the provided socket and remote address.
///
/// # Arguments
///
/// * `socket` - The TCP stream socket to handle.
/// * `remote_addr` - The remote address of the client.
/// * `make_service` - The factory function for creating a Tower Service that will handle the incoming request.
/// * `stopping_rx` - The shutdown receiver.
/// * `rendezvous` - The shutdown rendezvous guard.
async fn handle_connection(
    socket: TcpStream,
    remote_addr: SocketAddr,
    mut make_service: IntoMakeService<Router>,
    mut stopping_rx: broadcast::Receiver<()>,
    rendezvous: RendezvousGuard,
) {
    info!("Handling connection for {}", remote_addr);

    // Hyper has its own `AsyncRead` and `AsyncWrite` traits and doesn't use tokio.
    // `TokioIo` converts between them.
    let tcp_stream = TokioIo::new(socket);

    // Creates a new instance of networking service using a factory object `make_service`.
    // The factory is invoked with an `IncomingStream` representing a (fully established) incoming connection,
    // which internally encapsulates a TCP stream and the address of the remote client.
    // The service creation is asynchronous and any failure in this process is currently not handled.
    let tower_service = make_service
        .call(IncomingStream::new(&tcp_stream, remote_addr))
        .await
        .unwrap_or_else(|err| match err {});

    let hyper_service = TowerToHyperService {
        service: tower_service,
    };

    // `server::conn::auto::Builder` supports both http1 and http2.
    //
    // `TokioExecutor` tells hyper to use `tokio::spawn` to spawn tasks.
    let builder = server::conn::auto::Builder::new(TokioExecutor::new());
    select! {
        biased;

        // TODO: This is not graceful.
        _ = stopping_rx.recv() => {
            // this will stop handling the connection when the shutdown signal is received,
            // and jump to executing the next block of code after select.
            trace!("Shutdown signal received, stopping connection handling");
        },
        // `serve_connection_with_upgrades` is required for websockets. If you don't need
        // that you can use `serve_connection` instead.
        result = builder.serve_connection_with_upgrades(tcp_stream, hyper_service) => {
            if let Err(err) = result {
                // This error only appears when the client doesn't send a request and
                // terminate the connection.
                //
                // If client sends one request then terminate connection whenever, it doesn't
                // appear.
                trace!("Connection error observed from {remote_addr}: {err:#}");
            }
        }
    }

    info!("Connection with {} closed", remote_addr);
    rendezvous.completed();
}
