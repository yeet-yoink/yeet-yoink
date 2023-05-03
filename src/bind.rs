//! Warp socket binding utilities.

use clap::ArgMatches;
use futures::stream::SelectAll;
use futures::{AsyncRead, AsyncWrite};
use hyper::body::HttpBody;
use hyper::server::accept::Accept;
use hyper::{Body, Server};
use std::future::Future;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tracing::{error, info};

/// Binds TCP sockets to the values specified on the command-line.
///
/// ```shell
/// cargo run -- --http 127.0.0.1:8080 --http 127.0.0.1:8081
/// ```
pub async fn bind_tcp_sockets(
    matches: &ArgMatches,
) -> Result<SelectAll<TcpListenerStream>, BindError> {
    // Get the HTTP socket addresses to bind on.
    let http_sockets: Vec<SocketAddr> = matches
        .get_many("bind_http")
        .into_iter()
        .flatten()
        .cloned()
        .collect();

    // Bind to all TCP sockets.
    let mut streams = Vec::default();
    for addr in http_sockets {
        let listener = TcpListener::bind(addr).await.map_err(|e| {
            error!(
                "Failed to bind to {socket_addr}: {error}",
                socket_addr = addr,
                error = e
            );
            BindError::CantBind(addr, e)
        })?;

        info!("Binding to {addr}", addr = addr);
        streams.push(TcpListenerStream::new(listener));
    }

    // Join all streams.
    Ok(SelectAll::from_iter(streams))
}

/// A socket binding error.
#[derive(Debug, thiserror::Error)]
pub enum BindError {
    /// The application was unable to bind to the specified socket.
    #[error("Unable to bind to {0}: {1}")]
    CantBind(SocketAddr, std::io::Error),
}
