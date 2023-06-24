//! Contains the `/yoink` endpoint filter.

use crate::AppState;
use axum::body::HttpBody;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use hyper::StatusCode;
use shortguid::ShortGuid;
use tracing::info;

pub trait YoinkRoutes {
    /// Provides an API for storing files.
    ///
    /// ```http
    /// GET /yoink/KmC6e8laTnK3dioUSMpM0Q HTTP/1.1
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
    fn map_yoink_endpoint(self) -> Self {
        self.route("/yoink/:id", get(do_yoink))
    }
}

#[axum::debug_handler]
async fn do_yoink(
    Path(id): Path<ShortGuid>,
    State(state): State<AppState>,
) -> Result<Response, StatusCode> {
    info!("A yoink was attempted for ID {id}");

    todo!()
}
