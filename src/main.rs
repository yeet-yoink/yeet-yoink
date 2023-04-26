use tokio::sync::broadcast;
use tracing::info;
use warp::Filter;

mod commands;
mod logging;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let matches = commands::build_command().get_matches();
    logging::initialize_from_matches(&matches);

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

    let (_http_addr, http_warp) = warp::serve(hello.or(shutdown)).bind_with_graceful_shutdown(
        ([127, 0, 0, 1], 2080),
        async move {
            shutdown_rx.recv().await.ok();
        },
    );

    http_warp.await;

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
