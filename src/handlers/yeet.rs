//! Contains the `/yeet` endpoint filter.

use crate::backbone::CompletionMode;
use crate::headers::ContentMd5;
use crate::metrics::transfer::{TransferMethod, TransferMetrics};
use crate::AppState;
use axum::body::HttpBody;
use axum::extract::{BodyStream, State};
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
use uuid::Uuid;

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

impl<B> YeetRoutes for Router<AppState, B>
where
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
    State(state): State<AppState>,
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
    let id = Uuid::new_v4();
    let mut writer = match state.backbone.new_file(id).await {
        Ok(writer) => writer,
        Err(e) => return Ok(e.into()),
    };

    let mut stream = Box::pin(stream);

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
    let hashes = match writer.finalize(CompletionMode::NoSync).await {
        Ok(hashes) => hashes,
        Err(e) => {
            return Ok((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to complete writing to temporary file: {e}"),
            )
                .into_response())
        }
    };

    TransferMetrics::track(TransferMethod::Store, bytes_written);

    debug!(
        "Stream ended, buffered {bytes} bytes to disk; {hashes}",
        bytes = bytes_written
    );

    // TODO: Add test for slow writing and simultaneous reading.
    // let reader = file.reader().await.unwrap();
    // let size = reader.file_size();

    // TODO: Temporary solution to remove the file.
    state.backbone.remove(id).await;

    Ok("".into_response())
}
