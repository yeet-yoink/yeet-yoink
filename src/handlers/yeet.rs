//! Contains the `/yeet` endpoint filter.

use crate::headers::ContentMd5;
use crate::metrics::transfer::{TransferMethod, TransferMetrics};
use crate::wrapped_temporary::SharedTemporaryFile;
use axum::body::HttpBody;
use axum::extract::BodyStream;
use axum::headers::{ContentLength, ContentType};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Router, TypedHeader};
use hyper::body::Buf;
use hyper::StatusCode;
use sha2::Digest;
use std::convert::Infallible;
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;
use tracing::{debug, trace};

pub trait YeetRoutes {
    /// Provides an API for storing files.
    ///
    /// ```http
    /// POST /yeet HTTP/1.1
    /// Content-Length: 1024
    /// Content-Type: application/my-type
    ///
    /// your-data
    /// ```
    fn map_yeet_endpoint(self) -> Self;
}

impl<S, B> YeetRoutes for Router<S, B>
where
    S: Clone + Send + Sync + 'static,
    B: HttpBody + Send + Sync + 'static,
    axum::body::Bytes: From<<B as HttpBody>::Data>,
    <B as HttpBody>::Error: std::error::Error + Send + Sync,
{
    fn map_yeet_endpoint(self) -> Self {
        self.route("/yeet", post(do_yeet))
    }
}

#[axum::debug_handler]
async fn do_yeet(
    content_length: Option<TypedHeader<ContentLength>>,
    content_type: Option<TypedHeader<ContentType>>,
    content_md5: Option<TypedHeader<ContentMd5>>,
    stream: BodyStream,
) -> Result<Response, Infallible> {
    // TODO: Add server-side validation of MD5 value if header is present.

    if let Some(TypedHeader(ContentLength(n))) = content_length {
        trace!("Expecting {value} bytes", value = n);
    }

    if let Some(TypedHeader(mime)) = content_type {
        trace!("Expecting MIME type {value}", value = mime);
    }

    // TODO: Allow capacity?
    let file = match SharedTemporaryFile::new().await {
        Ok(file) => file,
        Err(e) => {
            return Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create temporary file: {e}"),
            )
                .into_response())
        }
    };

    let mut writer = match file.writer().await {
        Ok(file) => file,
        Err(e) => {
            return Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create a writer for the temporary file: {e}"),
            )
                .into_response())
        }
    };

    debug!(
        "Buffering request payload to {file:?}",
        file = file.file_path().await
    );

    let mut stream = Box::pin(stream);
    let mut md5 = md5::Context::new();
    let mut sha256 = sha2::Sha256::new();

    let mut bytes_written = 0;
    while let Some(result) = stream.next().await {
        let mut data = match result {
            Ok(data) => data,
            Err(e) => {
                return Ok((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to obtain data from the read stream: {e}"),
                )
                    .into_response())
            }
        };

        while data.has_remaining() {
            let chunk = data.chunk();
            md5.consume(chunk);
            sha256.update(chunk);

            match writer.write(&chunk).await {
                Ok(0) => {}
                Ok(n) => {
                    bytes_written += n;
                    data.advance(n);
                }
                Err(e) => {
                    return Ok((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to write to temporary file: {e}"),
                    )
                        .into_response())
                }
            }
        }

        match writer.sync_data().await {
            Ok(_) => {}
            Err(e) => {
                return Ok((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to flush data to temporary file: {e}"),
                )
                    .into_response())
            }
        }

        // TODO: Wake up consumers
    }

    // The file was already synced to disk in the last iteration, so
    // we can skip the sync here.
    match writer.complete_no_sync() {
        Ok(_) => {}
        Err(e) => {
            return Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to complete writing to temporary file: {e}"),
            )
                .into_response())
        }
    };

    let md5 = md5.compute();
    let sha256 = sha256.finalize();

    TransferMetrics::track(TransferMethod::Store, bytes_written);

    debug!(
        "Stream ended, buffered {bytes} bytes to disk; MD5 {md5:x}, SHA256 {sha256:x}",
        bytes = bytes_written,
        md5 = md5,
        sha256 = sha256
    );

    // TODO: Add test for slow writing and simultaneous reading.
    // let reader = file.reader().await.unwrap();
    // let size = reader.file_size();

    Ok("".into_response())
}
