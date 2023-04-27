use std::net::SocketAddr;
use tokio::net::{TcpListener, UnixListener};
use tokio::sync::broadcast;
use tokio_stream::{wrappers::TcpListenerStream, wrappers::UnixListenerStream, Stream, StreamExt};
use tracing::info;
use warp::Filter;

mod commands;
mod logging;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let matches = commands::build_command().get_matches();
    logging::initialize_from_matches(&matches);

    // Get the HTTP socket addresses to bind on.
    let http_sockets: Vec<SocketAddr> = matches
        .get_many("bind_http")
        .into_iter()
        .flatten()
        .cloned()
        .collect();

    // Provide a signal that can be used to shut down the server.
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);
    let shutdown_filter = warp::any().map(move || shutdown_tx.clone());

    info!("Hi. 👋");

    // GET /hello/warp => 200 OK with body "Hello, warp!"
    let hello = warp::path!("hello" / String).and_then(hello);

    // POST /stop to shut down the server.
    let shutdown = warp::post()
        .and(warp::path("stop"))
        .and(warp::path::end())
        .and(shutdown_filter)
        .and_then(shutdown);

    // Bind to all TCP sockets.
    let streams = futures::future::join_all(http_sockets.into_iter().map(|addr| {
        tokio::spawn(async move {
            let listener = TcpListener::bind(addr).await.unwrap();
            TcpListenerStream::new(listener)
        })
    }))
    .await
    .into_iter()
    .flatten();
    let streams = futures::stream::SelectAll::from_iter(streams);

    // let listener = UnixListener::bind("/tmp/warp.sock").unwrap();
    // let stream = UnixListenerStream::new(listener);

    warp::serve(hello.or(shutdown))
        .serve_incoming_with_graceful_shutdown(streams, async move {
            shutdown_rx.recv().await.ok();
        })
        .await;

    info!("Bye. 👋");
}

/// Responds with the caller's name.
///
/// ```http
/// GET /hello/name
/// ```
async fn hello(name: String) -> Result<impl warp::Reply, warp::Rejection> {
    Ok(format!("Hello, {}!", name))
}

/// Initiates a graceful shutdown.
///
/// ```http
/// POST /stop
/// ```
async fn shutdown(tx: broadcast::Sender<()>) -> Result<impl warp::Reply, warp::Rejection> {
    tx.send(()).ok();
    Ok(warp::reply::reply())
}
