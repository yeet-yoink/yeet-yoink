//! Contains the `/yoink` endpoint filter.

use crate::backbone::GetReaderError;
use crate::expiration_as_rfc1123;
use crate::metrics::transfer::{TransferMethod, TransferMetrics};
use crate::AppState;
use axum::body::{HttpBody, StreamBody};
use axum::extract::{Path, State};
use axum::http::{header, HeaderName};
use axum::response::{AppendHeaders, IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use base64::Engine;
use hyper::StatusCode;
use mime_db::extension;
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use shared_files::FileSize;
use shortguid::ShortGuid;
use tokio_util::io::ReaderStream;

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

impl<B> YoinkRoutes for Router<AppState, B>
where
    B: HttpBody + Send + Sync + 'static,
    axum::body::Bytes: From<<B as HttpBody>::Data>,
    <B as HttpBody>::Error: std::error::Error + Send + Sync,
{
    // Ensure HttpCallMetricTracker is updated.
    fn map_yoink_endpoint(self) -> Self {
        self.route("/yoink/:id", get(do_yoink))
    }
}

#[axum::debug_handler]
async fn do_yoink(
    Path(id): Path<ShortGuid>,
    State(state): State<AppState>,
) -> Result<Response, StatusCode> {
    let file = match state.backbone.get_file(id).await {
        Ok(file) => file,
        Err(e) => return Ok(e.into()),
    };

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
        headers.push((
            header::ETAG,
            base64::engine::general_purpose::STANDARD.encode(&summary.hashes.sha256[..]),
        ));

        headers.push((
            HeaderName::from_static("content-md5"),
            base64::engine::general_purpose::STANDARD.encode(&summary.hashes.md5[..]),
        ));

        headers.push((
            HeaderName::from_static("x-file-md5"),
            hex::encode(&summary.hashes.md5[..]),
        ));

        headers.push((
            HeaderName::from_static("x-file-sha256"),
            hex::encode(&summary.hashes.sha256[..]),
        ));

        let file_name = &summary.file_name;

        let header = content_disposition_from_optional_name(&id, &content_type, file_name);
        headers.push(header);
    } else {
        // Use a default file name when none is known.
        let header = default_content_disposition_header(&id, &content_type);
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
    let body = StreamBody::new(stream);

    let headers = AppendHeaders(headers);
    Ok((headers, body).into_response())
}

/// Attempts to generate a `Content-Disposition` header from the optionally specified
/// file name. If no name was set, falls back to a generated file name based on the ID.
fn content_disposition_from_optional_name(
    id: &ShortGuid,
    content_type: &String,
    file_name: &Option<String>,
) -> (HeaderName, String) {
    if let Some(file_name) = file_name {
        let file_name = utf8_percent_encode(&file_name, &ASCII_CONTROLS).to_string();
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{file_name}\""),
        )
    } else {
        default_content_disposition_header(&id, &content_type)
    }
}

/// Generates a `Content-Disposition` header based on the ID. If the `Content-Type` was specified,
/// a default extension will be appended to the file.
fn default_content_disposition_header(
    id: &ShortGuid,
    content_type: &String,
) -> (HeaderName, String) {
    if content_type.is_empty() {
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{id}\""),
        )
    } else {
        // See also https://github.com/viz-rs/mime-db/pull/9
        let ext = extension(&content_type).unwrap_or("");
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

impl From<GetReaderError> for Response {
    fn from(value: GetReaderError) -> Self {
        match value {
            GetReaderError::UnknownFile(id) => problemdetails::new(StatusCode::NOT_FOUND)
                .with_title("File not found")
                .with_detail(format!("The file with ID {id} could not be found"))
                .with_instance(format!("/yoink/{id}"))
                .with_value("id", id.to_string())
                .into_response(),
            GetReaderError::FileExpired(id) => problemdetails::new(StatusCode::GONE)
                .with_title("File not found")
                .with_detail(format!("The file with ID {id} has expired"))
                .with_instance(format!("/yoink/{id}"))
                .with_value("id", id.to_string())
                .into_response(),
            GetReaderError::FileError(id, e) => {
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
}
