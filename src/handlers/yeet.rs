//! Contains the `/yeet` endpoint filter.

use crate::backbone::{CompletionMode, FileHashes};
use crate::headers::ContentMd5;
use crate::metrics::transfer::{TransferMethod, TransferMetrics};
use crate::AppState;
use axum::body::HttpBody;
use axum::extract::{BodyStream, State};
use axum::headers::{ContentLength, ContentType};
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Router, TypedHeader};
use base64::Engine;
use chrono::{DateTime, Utc};
use hyper::body::Buf;
use hyper::header::EXPIRES;
use hyper::StatusCode;
use serde::Serialize;
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
) -> Result<Response, StatusCode> {
    if let Some(TypedHeader(ContentLength(n))) = content_length {
        trace!("Expecting {value} bytes", value = n);
    }

    if let Some(TypedHeader(mime)) = content_type {
        trace!("Expecting MIME type {value}", value = mime);
    }

    let id = Uuid::new_v4();

    // TODO: Allow capacity?
    // TODO: Add server-side validation of MD5 value if header is present.
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
    }

    // The file was already synced to disk in the last iteration, so
    // we can skip the sync here.
    // TODO: Add server-side validation of MD5 value if header is present.
    let write_result = match writer.finalize(CompletionMode::NoSync).await {
        Ok(write_result) => write_result,
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
        bytes = bytes_written,
        hashes = write_result.hashes
    );

    let mut response = axum::Json(SuccessfulUploadResponse {
        id,
        hashes: (&write_result.hashes).into(),
    })
    .into_response();

    let expiration_date = expiration_as_rfc1123(&write_result.expires);

    *response.status_mut() = StatusCode::CREATED;
    let headers = response.headers_mut();
    headers
        .entry(EXPIRES)
        .or_insert(HeaderValue::from_str(&expiration_date).expect("invalid time input provided"));

    Ok(response)
}

fn expiration_as_rfc1123(expires: &tokio::time::Instant) -> String {
    let expire_in = expires.duration_since(tokio::time::Instant::now());
    let expiration_date = std::time::SystemTime::now() + expire_in;
    let expiration_date = DateTime::<Utc>::from(expiration_date);
    expiration_date
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}

#[derive(Serialize)]
struct SuccessfulUploadResponse {
    id: Uuid,
    hashes: Hashes,
}

#[derive(Serialize)]
struct Hashes {
    /// The MD5 hash in hex encoding.
    md5: String,
    /// The SHA-256 hash in hex encoding
    sha256: String,
}

impl From<&FileHashes> for Hashes {
    fn from(value: &FileHashes) -> Self {
        Self {
            md5: hex::encode(value.md5.as_slice()),
            sha256: hex::encode(value.sha256),
        }
    }
}
