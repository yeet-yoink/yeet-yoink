//! Contains the `/yoink` endpoint filter.

use crate::AppState;
use crate::{expiration_as_rfc1123, system_time_as_rfc1123};
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderName, StatusCode, header};
use axum::response::{AppendHeaders, IntoResponse, Response};
use axum::routing::get;
use backend_traits::FetchedFile;
use base64::Engine;
use file_distribution::{BoxedFileReader, FileHashes, FileReaderTrait, GetFileReaderError};
use metrics::transfer::{TransferMethod, TransferMetrics};
use mime_db::extension;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use shared_files::FileSize;
use shortguid::ShortGuid;
use std::borrow::Borrow;
use tokio_util::io::ReaderStream;
use tracing::warn;

/// Escape control set for URL/hex-encoding file names in the Content-Disposition header.
static ASCII_CONTROLS: AsciiSet = CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

pub trait YoinkRoutes {
    /// Provides an API for storing files.
    ///
    /// ```http
    /// GET /yoink/KmC6e8laTnK3dioUSMpM0Q HTTP/1.1
    ///
    /// Content-Length: 1024
    /// Content-Type: application/my-type
    ///
    /// your-data
    /// ```
    fn map_yoink_endpoint(self) -> Self;
}

impl YoinkRoutes for Router<AppState> {
    // Ensure HttpCallMetricTracker is updated.
    fn map_yoink_endpoint(self) -> Self {
        self.route("/yoink/{id}", get(do_yoink))
    }
}

#[axum::debug_handler]
async fn do_yoink(
    Path(id): Path<ShortGuid>,
    State(state): State<AppState>,
) -> Result<Response, StatusCode> {
    let not_found = match state.backbone.get_file(id).await {
        Ok(file) => return Ok(local_file_response(id, file)),
        Err(e @ (GetFileReaderError::UnknownFile(_) | GetFileReaderError::FileExpired(_))) => e,
        Err(e) => return Ok(map_file_reader_error_to_response(e)),
    };

    // The file is not buffered locally (anymore); try the backends.
    let Some(fetched) = state.backends.fetch(id).await else {
        return Ok(map_file_reader_error_to_response(not_found));
    };

    if state.near_cache {
        match state.backbone.cache_file(id, &fetched).await {
            Ok(()) => match state.backbone.get_file(id).await {
                Ok(file) => return Ok(local_file_response(id, file)),
                Err(e) => warn!(file_id = %id, "Failed to read file {id} from the near cache: {e}"),
            },
            Err(e) => {
                warn!(file_id = %id, "Failed to cache file {id} fetched from a backend: {e}")
            }
        }
    }

    Ok(fetched_file_response(id, fetched))
}

/// Streams a file buffered by the backbone.
fn local_file_response(id: ShortGuid, file: BoxedFileReader) -> Response {
    TransferMetrics::track_transfer(TransferMethod::Fetch);

    let summary = file.summary();

    let mut headers = Vec::new();
    if let FileSize::Exactly(size) = file.file_size() {
        headers.push((header::CONTENT_LENGTH, size.to_string()));
    }

    // The content type specified on file creation, or an empty string.
    let content_type = file
        .content_type()
        .map_or(String::default(), |c| c.to_string());

    // Add ETag from SHA-256 hash, etc.
    if let Some(summary) = summary {
        headers.extend(hash_headers(&summary.hashes));

        let file_name = &summary.file_name;

        let header = content_disposition_from_optional_name(id, &content_type, file_name);
        headers.push(header);
    } else {
        // Use a default file name when none is known.
        let header = default_content_disposition_header(id, &content_type);
        headers.push(header);
    }

    if !content_type.is_empty() {
        headers.push((header::CONTENT_TYPE, content_type));
    }

    headers.push((header::AGE, file.file_age().as_secs().to_string()));

    // Provide expiration header.
    let expiration_date = expiration_as_rfc1123(&file.expiration_date());
    headers.push((header::EXPIRES, expiration_date));

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let headers = AppendHeaders(headers);
    (headers, body).into_response()
}

/// Serves a file fetched from a backend without buffering it locally.
///
/// `Expires` reflects the backend copy's expiry, if it has one. No `Age` is sent.
fn fetched_file_response(id: ShortGuid, file: FetchedFile) -> Response {
    TransferMetrics::track_transfer(TransferMethod::Fetch);
    TransferMetrics::track_bytes_transferred(TransferMethod::Fetch, file.data.len());

    let content_type = file.content_type.unwrap_or_default();

    let mut headers = vec![(header::CONTENT_LENGTH, file.data.len().to_string())];
    headers.extend(hash_headers(&file.hashes));
    headers.push(content_disposition_from_optional_name(
        id,
        &content_type,
        &file.file_name,
    ));
    if !content_type.is_empty() {
        headers.push((header::CONTENT_TYPE, content_type));
    }
    if let Some(expires_at) = file.expires_at {
        headers.push((header::EXPIRES, system_time_as_rfc1123(expires_at)));
    }

    (AppendHeaders(headers), Body::from(file.data)).into_response()
}

/// Generates the `ETag`, `Content-MD5` and hex-encoded hash headers.
fn hash_headers(hashes: &FileHashes) -> [(HeaderName, String); 4] {
    let base64 = &base64::engine::general_purpose::STANDARD;
    [
        (header::ETAG, base64.encode(&hashes.sha256[..])),
        (
            HeaderName::from_static("content-md5"),
            base64.encode(&hashes.md5[..]),
        ),
        (
            HeaderName::from_static("yy-file-md5"),
            hex::encode(&hashes.md5[..]),
        ),
        (
            HeaderName::from_static("yy-file-sha256"),
            hex::encode(&hashes.sha256[..]),
        ),
    ]
}

/// Attempts to generate a `Content-Disposition` header from the optionally specified
/// file name. If no name was set, falls back to a generated file name based on the ID.
fn content_disposition_from_optional_name<I>(
    id: I,
    content_type: &String,
    file_name: &Option<String>,
) -> (HeaderName, String)
where
    I: Borrow<ShortGuid>,
{
    let id = id.borrow();
    if let Some(file_name) = file_name {
        let file_name = utf8_percent_encode(file_name, &ASCII_CONTROLS).to_string();
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{file_name}\""),
        )
    } else {
        default_content_disposition_header(id, content_type)
    }
}

/// Generates a `Content-Disposition` header based on the ID. If the `Content-Type` was specified,
/// a default extension will be appended to the file.
fn default_content_disposition_header<I>(id: I, content_type: &String) -> (HeaderName, String)
where
    I: Borrow<ShortGuid>,
{
    let id = id.borrow();
    if content_type.is_empty() {
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{id}\""),
        )
    } else {
        // See also https://github.com/viz-rs/mime-db/pull/9
        let ext = extension(content_type).unwrap_or("");
        if ext.is_empty() {
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{id}\""),
            )
        } else {
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{id}.{ext}\""),
            )
        }
    }
}

fn map_file_reader_error_to_response(value: GetFileReaderError) -> Response {
    match value {
        GetFileReaderError::UnknownFile(id) => problemdetails::new(StatusCode::NOT_FOUND)
            .with_title("File not found")
            .with_detail(format!("The file with ID {id} could not be found"))
            .with_instance(format!("/yoink/{id}"))
            .with_value("id", id.to_string())
            .into_response(),
        GetFileReaderError::FileExpired(id) => problemdetails::new(StatusCode::GONE)
            .with_title("File not found")
            .with_detail(format!("The file with ID {id} has expired"))
            .with_instance(format!("/yoink/{id}"))
            .with_value("id", id.to_string())
            .into_response(),
        GetFileReaderError::FileError(id, e) => {
            problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                .with_title("File not found")
                .with_detail(format!("Unable to process file: {e}"))
                .with_instance(format!("/yoink/{id}"))
                .with_value("id", id.to_string())
                .with_value("error", e.to_string())
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_id() -> ShortGuid {
        ShortGuid::from_bytes([1u8; 16])
    }

    #[test]
    fn uses_id_when_no_name_and_no_content_type() {
        let id = test_id();
        let (name, value) = content_disposition_from_optional_name(id, &String::new(), &None);
        assert_eq!(name, header::CONTENT_DISPOSITION);
        assert_eq!(value, format!("attachment; filename=\"{id}\""));
    }

    #[test]
    fn appends_extension_for_known_content_type() {
        let id = test_id();
        let (_, value) =
            content_disposition_from_optional_name(id, &"application/json".to_string(), &None);
        assert_eq!(value, format!("attachment; filename=\"{id}.json\""));
    }

    #[test]
    fn omits_extension_for_unknown_content_type() {
        let id = test_id();
        let (_, value) = content_disposition_from_optional_name(
            id,
            &"application/x-yeet-yoink-unknown".to_string(),
            &None,
        );
        assert_eq!(value, format!("attachment; filename=\"{id}\""));
    }

    #[test]
    fn prefers_explicit_file_name() {
        let (_, value) = content_disposition_from_optional_name(
            test_id(),
            &"application/json".to_string(),
            &Some("report.txt".to_string()),
        );
        assert_eq!(value, "attachment; filename=\"report.txt\"");
    }

    #[test]
    fn escapes_unsafe_characters_in_file_name() {
        let (_, value) = content_disposition_from_optional_name(
            test_id(),
            &String::new(),
            &Some("my \"file\".txt".to_string()),
        );
        assert_eq!(value, "attachment; filename=\"my%20%22file%22.txt\"");
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;
    use crate::backend_registry::BackendFetcher;
    use crate::test_support::{FakeBackend, fetched_file};
    use axum::http::Request;
    use backbone::{Backbone, CompletionMode};
    use backend_traits::{Backend, FetchedFile};
    use bytes::Bytes;
    use rendezvous::Rendezvous;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{broadcast, mpsc};
    use tower::ServiceExt;

    struct Fixture {
        state: AppState,
        backend: FakeBackend,
        _backend_receiver: mpsc::Receiver<backend_traits::BackendCommand>,
    }

    fn fixture(near_cache: bool) -> Fixture {
        let (sender, backend_receiver) = mpsc::channel(16);
        let rendezvous = Rendezvous::new();
        let backbone = Backbone::new(
            sender.into(),
            rendezvous.fork_guard(),
            Duration::from_secs(3600),
        );

        // Dropping a Rendezvous blocks until all guards complete.
        std::mem::forget(rendezvous);

        let backend = FakeBackend::default();
        let state = AppState {
            shutdown_tx: broadcast::channel(1).0,
            backbone: Arc::new(backbone),
            backends: BackendFetcher::new(Arc::new(vec![Backend::wrap(backend.clone())])),
            near_cache,
        };
        Fixture {
            state,
            backend,
            _backend_receiver: backend_receiver,
        }
    }

    async fn yoink(state: &AppState, id: ShortGuid) -> (StatusCode, HeaderMapSnapshot, Bytes) {
        let request = Request::get(format!("/yoink/{id}"))
            .body(Body::empty())
            .expect("Invalid request");
        let response = crate::router(state.clone())
            .oneshot(request)
            .await
            .expect("Request failed");
        let status = response.status();
        let headers = HeaderMapSnapshot(response.headers().clone());
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("Failed to read body");
        (status, headers, body)
    }

    struct HeaderMapSnapshot(axum::http::HeaderMap);

    impl HeaderMapSnapshot {
        fn get(&self, name: &str) -> Option<&str> {
            self.0.get(name).and_then(|v| v.to_str().ok())
        }
    }

    #[tokio::test]
    async fn local_file_is_served_without_backends() {
        let fixture = fixture(true);
        let id = ShortGuid::new_random();

        let mut writer = fixture
            .state
            .backbone
            .new_file(id, Some(3), None, None, None)
            .await
            .expect("Failed to create file");
        writer.write(b"abc").await.expect("Failed to write");
        writer.sync_data().await.expect("Failed to sync");
        writer
            .finalize(CompletionMode::NoSync)
            .await
            .expect("Failed to finalize");

        let (status, _, body) = yoink(&fixture.state, id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "abc");
        assert_eq!(fixture.backend.fetches(), 0);
    }

    #[tokio::test]
    async fn missing_file_is_not_found() {
        let fixture = fixture(true);

        let (status, _, _) = yoink(&fixture.state, ShortGuid::new_random()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(fixture.backend.fetches(), 1);
    }

    #[tokio::test]
    async fn file_is_served_from_backend() {
        let fixture = fixture(false);
        let id = ShortGuid::new_random();
        fixture.backend.insert(id, fetched_file(b"abc"));

        let (status, headers, body) = yoink(&fixture.state, id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "abc");
        assert_eq!(headers.get("content-type"), Some("text/plain"));
        assert_eq!(headers.get("content-length"), Some("3"));
        assert_eq!(
            headers.get("yy-file-md5"),
            Some("900150983cd24fb0d6963f7d28e17f72")
        );
        assert_eq!(
            headers.get("content-disposition"),
            Some("attachment; filename=\"fetched.txt\"")
        );
        assert_eq!(headers.get("expires"), None);

        // Without the near cache, every read goes to the backend.
        let (status, _, _) = yoink(&fixture.state, id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(fixture.backend.fetches(), 2);
    }

    #[tokio::test]
    async fn served_backend_copy_expires_with_the_backend() {
        use std::time::{Duration, SystemTime, UNIX_EPOCH};

        let fixture = fixture(false);
        let id = ShortGuid::new_random();
        // Whole seconds, since HTTP dates have no sub-second precision.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let expires_at = UNIX_EPOCH + Duration::from_secs(now + 60);
        fixture.backend.insert(
            id,
            FetchedFile {
                expires_at: Some(expires_at),
                ..fetched_file(b"abc")
            },
        );

        let (status, headers, _) = yoink(&fixture.state, id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers.get("expires"),
            Some(system_time_as_rfc1123(expires_at).as_str())
        );
    }

    #[tokio::test]
    async fn fetched_file_is_near_cached() {
        let fixture = fixture(true);
        let id = ShortGuid::new_random();
        fixture.backend.insert(id, fetched_file(b"abc"));

        let (status, headers, body) = yoink(&fixture.state, id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "abc");
        assert_eq!(headers.get("content-type"), Some("text/plain"));
        assert!(headers.get("expires").is_some());

        // The second read is served locally, even once the backend lost the file.
        fixture.backend.clear();
        let (status, headers, body) = yoink(&fixture.state, id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "abc");
        assert_eq!(
            headers.get("content-disposition"),
            Some("attachment; filename=\"fetched.txt\"")
        );
        assert_eq!(fixture.backend.fetches(), 1);
    }

    #[tokio::test]
    async fn corrupt_backend_copy_is_not_served() {
        let fixture = fixture(true);
        let id = ShortGuid::new_random();
        fixture.backend.insert(
            id,
            FetchedFile {
                data: Bytes::from_static(b"abd"),
                ..fetched_file(b"abc")
            },
        );

        let (status, _, _) = yoink(&fixture.state, id).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
