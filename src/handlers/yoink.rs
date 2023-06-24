//! Contains the `/yoink` endpoint filter.

use crate::AppState;
use axum::body::HttpBody;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use hyper::StatusCode;

pub trait YoinkRoutes {
    /// Provides an API for storing files.
    ///
    /// ```http
    /// GET /yoink/e121b81c-c1f5-465f-949a-aca490b87d2a HTTP/1.1
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
        self.route("/yeet", get(do_yoink))
    }
}

#[axum::debug_handler]
async fn do_yoink(State(state): State<AppState>) -> Result<Response, StatusCode> {
    todo!()
}
