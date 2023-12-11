//! Contains the `/yeet` endpoint filter.

use crate::expiration_as_rfc1123;
use crate::AppState;
use axum::body::HttpBody;
use axum::extract::{BodyStream, Query, State, TypedHeader};
use axum::headers::{ContentLength, ContentType};
use axum::http::{HeaderName, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use backbone::{CompletionMode, NewFileError};
use file_distribution::{FileHashes, GetFileReaderError};
use headers_content_md5::ContentMd5;
use hyper::body::Buf;
use hyper::header::EXPIRES;
use hyper::StatusCode;
use metrics::transfer::TransferMethod;
use metrics::transfer::TransferMetrics;
use serde::Serialize;
use shortguid::ShortGuid;
use tokio_stream::StreamExt;
use tracing::{debug, trace};

static ID_HEADER: HeaderName = HeaderName::from_static("yy-id");

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
    // Ensure HttpCallMetricTracker is updated.
    fn map_yeet_endpoint(self) -> Self {
        self.route("/yeet", post(do_yeet))
    }
}

#[derive(Debug, serde::Deserialize)]
struct QueryParams {
    file_name: Option<String>,
}

#[axum::debug_handler]
async fn do_yeet(
    content_length: Option<TypedHeader<ContentLength>>,
    content_type: Option<TypedHeader<ContentType>>,
    content_md5: Option<TypedHeader<ContentMd5>>,
    State(state): State<AppState>,
    query: Query<QueryParams>,
    stream: BodyStream,
) -> Result<Response, StatusCode> {
    TransferMetrics::track_transfer(TransferMethod::Store);

    let content_length = if let Some(TypedHeader(ContentLength(n))) = content_length {
        trace!("Expecting {value} bytes", value = n);
        Some(n)
    } else {
        None
    };

    let content_type = if let Some(TypedHeader(content_type)) = content_type {
        trace!("Expecting MIME type {value}", value = content_type);
        Some(content_type)
    } else {
        None
    };

    let content_md5 = if let Some(TypedHeader(ContentMd5(md5))) = content_md5 {
        trace!("Expecting content MD5 {value}", value = hex::encode(md5));
        Some(md5)
    } else {
        None
    };

    let id = ShortGuid::new_random();

    // TODO: Allow capacity? Test whether we have enough resources?

    let mut writer = match state
        .backbone
        .new_file(
            id,
            content_length,
            content_type,
            content_md5,
            query.file_name.clone(),
        )
        .await
    {
        Ok(writer) => writer,
        Err(e) => return Ok(map_new_file_error_to_response(e)),
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
            match writer.write(chunk).await {
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

    debug!(
        file_id = %id,
        "Stream ended, buffered {bytes} bytes to disk; {hashes}",
        bytes = bytes_written,
        hashes = write_result.hashes
    );

    let mut response = axum::Json(SuccessfulUploadResponse {
        id,
        file_size_bytes: write_result.file_size_bytes,
        hashes: (&write_result.hashes).into(),
    })
    .into_response();

    let expiration_date = expiration_as_rfc1123(&write_result.expires);

    *response.status_mut() = StatusCode::CREATED;
    let headers = response.headers_mut();

    // Set the file expiration.
    headers
        .entry(EXPIRES)
        .or_insert(HeaderValue::from_str(&expiration_date).expect("invalid time input provided"));

    // Add the ID as a separate header to simplify testing.
    let id = format!("{id}");
    headers
        .entry(&ID_HEADER)
        .or_insert(HeaderValue::from_str(&id).expect("invalid ID input provided"));

    Ok(response)
}

#[derive(Serialize)]
struct SuccessfulUploadResponse {
    /// The ID of the file.
    id: ShortGuid,
    /// The file size in bytes.
    file_size_bytes: usize,
    /// The hashes of the file.
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

fn map_new_file_error_to_response(value: NewFileError) -> Response {
    match value {
        NewFileError::FailedCreatingFile(id, e) => {
            problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("File not found")
                .with_detail(format!("Failed to create temporary file: {e}"))
                .with_value("id", id.to_string())
                .with_value("error", e.to_string())
                .into_response()
        }
        NewFileError::FailedCreatingWriter(id, e) => {
            problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("File not found")
                .with_detail(format!(
                    "Failed to create a writer for the temporary file: {e}"
                ))
                .with_value("id", id.to_string())
                .with_value("error", e.to_string())
                .into_response()
        }
        NewFileError::InternalErrorMayRetry(id) => {
            problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("File not found")
                .with_detail("Failed to create temporary file - ID already in use".to_string())
                .with_value("id", id.to_string())
                .into_response()
        }
    }
}
