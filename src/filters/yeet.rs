//! Contains the `/yeet` endpoint filter.

use async_tempfile::TempFile;
use futures::Stream;
use hyper::{HeaderMap, StatusCode};
use std::convert::Infallible;
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;
use tracing::{debug, info};
use warp::body::stream;
use warp::header::headers_cloned;
use warp::reply::with_status;
use warp::{Filter, Rejection, Reply};

const ROUTE: &'static str = "yeet";

/// Provides metrics.
///
/// ```http
/// GET /metrics
/// ```
pub fn yeet_endpoint() -> impl Filter<Extract = (impl Reply,), Error = Rejection> + Clone {
    warp::post()
        .and(warp::path(ROUTE))
        .and(warp::path::end())
        .and(headers_cloned())
        .and(stream())
        .and_then(do_yeet)
}

async fn do_yeet<S, B>(headers: HeaderMap, stream: S) -> Result<impl Reply, Infallible>
where
    S: Stream<Item = Result<B, warp::Error>> + StreamExt,
    B: warp::Buf,
{
    info!("{:?}", headers);

    let content_length = headers.get("Content-Length");
    let content_type = headers.get("Content-Type");

    // Add server-side validation if header is present.
    let content_md5 = headers.get("Content-MD5");

    // TODO: Allow capacity?
    let mut file = match TempFile::new().await {
        Ok(file) => file,
        Err(e) => {
            return Ok(with_status(
                format!("Failed to create temporary file: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
            .into_response())
        }
    };

    debug!(
        "Buffering request payload to {file:?}",
        file = file.file_path()
    );

    if let Some(n) = content_length {
        debug!("Expecting {value:?} bytes", value = n);
    }

    let mut stream = Box::pin(stream);
    let mut md5_context = md5::Context::new();

    let mut bytes_written = 0;
    while let Some(result) = stream.next().await {
        let mut data = match result {
            Ok(data) => data,
            Err(e) => {
                return Ok(with_status(
                    format!("Failed to obtain data from the read stream: {e}"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
                .into_response())
            }
        };

        while data.has_remaining() {
            let chunk = data.chunk();
            md5_context.consume(chunk);

            match file.write(&chunk).await {
                Ok(0) => {}
                Ok(n) => {
                    bytes_written += n;
                    data.advance(n);
                }
                Err(e) => {
                    return Ok(with_status(
                        format!("Failed to write to temporary file: {e}"),
                        StatusCode::INTERNAL_SERVER_ERROR,
                    )
                    .into_response())
                }
            }
        }

        match file.sync_data().await {
            Ok(_) => {}
            Err(e) => {
                return Ok(with_status(
                    format!("Failed to flush data to temporary file: {e}"),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
                .into_response())
            }
        }

        // TODO: Wake up consumers
    }

    let md5 = md5_context.compute();

    debug!(
        "Stream ended, buffered {bytes} bytes to disk; MD5 {digest:x}",
        bytes = bytes_written,
        digest = md5
    );

    Ok("".into_response())
}
