//! Contains the `/yeet` endpoint filter.

use crate::AppState;
use crate::expiration_as_rfc1123;
use axum::Router;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::header::EXPIRES;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum_extra::TypedHeader;
use axum_extra::headers::{ContentLength, ContentType};
use backbone::{CompletionMode, FinalizationError, NewFileError};
use bytes::Buf;
use file_distribution::FileHashes;
use headers_content_md5::ContentMd5;
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

impl YeetRoutes for Router<AppState> {
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
    body: Body,
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

    let mut stream = Box::pin(body.into_data_stream());

    let mut bytes_written = 0;
    while let Some(result) = stream.next().await {
        let mut data = match result {
            Ok(data) => data,
            Err(e) => {
                return Ok((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to obtain data from the read stream: {e}"),
                )
                    .into_response());
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
                Err(e) => return Ok(map_write_error_to_response(e)),
            }
        }

        match writer.sync_data().await {
            Ok(_) => {}
            Err(e) => {
                return Ok((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to flush data to temporary file: {e}"),
                )
                    .into_response());
            }
        }
    }

    // The file was already synced to disk in the last iteration, so
    // we can skip the sync here. Finalizing checks the content against the
    // announced length and MD5, if any.
    let write_result = match writer.finalize(CompletionMode::NoSync).await {
        Ok(write_result) => write_result,
        Err(e) => return Ok(map_finalization_error_to_response(e)),
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

    *response.status_mut() = StatusCode::CREATED;
    let headers = response.headers_mut();

    // Set the file expiration.
    if let Some(expiration_date) = expiration_as_rfc1123(&write_result.expires) {
        headers.entry(EXPIRES).or_insert(
            HeaderValue::from_str(&expiration_date).expect("invalid time input provided"),
        );
    }

    // Add the ID as a separate header to simplify testing.
    let id = format!("{id}");
    headers
        .entry(&ID_HEADER)
        .or_insert(HeaderValue::from_str(&id).expect("invalid ID input provided"));

    Ok(response)
}

/// Maps a failed write to a response. Sending more than the announced
/// `Content-Length` is the client's mistake; anything else is ours.
fn map_write_error_to_response(error: std::io::Error) -> Response {
    if error.kind() == std::io::ErrorKind::InvalidInput {
        return (
            StatusCode::BAD_REQUEST,
            "The request body is longer than its Content-Length",
        )
            .into_response();
    }
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("Failed to write to temporary file: {error}"),
    )
        .into_response()
}

/// Maps a failed finalization to a response. Content that does not match its
/// announced length or MD5 is the client's mistake; anything else is ours.
fn map_finalization_error_to_response(error: FinalizationError) -> Response {
    match error {
        FinalizationError::InvalidFileLength(actual, expected) => (
            StatusCode::BAD_REQUEST,
            format!(
                "The request body is {actual} bytes long, but its Content-Length is {expected}"
            ),
        )
            .into_response(),
        FinalizationError::IntegrityCheckFailed(expected, actual) => (
            StatusCode::BAD_REQUEST,
            format!("The request body has MD5 {actual}, but its Content-MD5 is {expected}"),
        )
            .into_response(),
        e => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to complete writing to temporary file: {e}"),
        )
            .into_response(),
    }
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

#[cfg(test)]
mod endpoint_tests {
    use crate::test_support::{send, test_app};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use base64::Engine;

    /// The base64-encoded MD5 of `abc`.
    const ABC_MD5: &str = "kAFQmDzST7DWlj99KOF/cg==";

    fn yeet() -> axum::http::request::Builder {
        Request::post("/yeet").header("content-type", "text/plain")
    }

    async fn status_of(request: Request<Body>) -> (StatusCode, String) {
        let (status, _, body) = send(&test_app(true).state, request).await;
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn upload_is_accepted() {
        let app = test_app(true);
        let request = yeet()
            .header("content-length", "3")
            .header("content-md5", ABC_MD5)
            .body(Body::from("abc"))
            .expect("Invalid request");

        let (status, headers, body) = send(&app.state, request).await;

        assert_eq!(status, StatusCode::CREATED);
        let response: serde_json::Value = serde_json::from_slice(&body).expect("Invalid JSON");
        assert_eq!(response["file_size_bytes"], 3);
        let id = headers
            .get("yy-id")
            .and_then(|v| v.to_str().ok())
            .expect("Missing ID header");
        assert_eq!(response["id"], id);
    }

    #[tokio::test]
    async fn md5_mismatch_is_a_client_error() {
        let wrong_md5 = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        let request = yeet()
            .header("content-md5", wrong_md5)
            .body(Body::from("abc"))
            .expect("Invalid request");

        let (status, body) = status_of(request).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("Content-MD5"), "{body}");
    }

    #[tokio::test]
    async fn body_shorter_than_announced_is_a_client_error() {
        let request = yeet()
            .header("content-length", "5")
            .body(Body::from("abc"))
            .expect("Invalid request");

        let (status, body) = status_of(request).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("Content-Length"), "{body}");
    }

    #[tokio::test]
    async fn body_longer_than_announced_is_a_client_error() {
        let request = yeet()
            .header("content-length", "2")
            .body(Body::from("abc"))
            .expect("Invalid request");

        let (status, body) = status_of(request).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("Content-Length"), "{body}");
    }

    #[tokio::test]
    async fn rejected_upload_is_given_no_id() {
        let app = test_app(true);
        let wrong_md5 = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        let request = yeet()
            .header("content-md5", wrong_md5)
            .body(Body::from("abc"))
            .expect("Invalid request");
        let (status, headers, _) = send(&app.state, request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(headers.get("yy-id").is_none());
    }
}
