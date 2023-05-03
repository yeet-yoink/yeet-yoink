//! Contains warp filters.

mod health;
mod metrics;
mod shutdown;

use warp::{Filter, Reply};

pub use health::health_endpoints;
pub use metrics::metrics_endpoint;
pub use shutdown::shutdown_endpoint;
