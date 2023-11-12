use crate::backbone::WriteSummary;
pub use crate::backends::dyn_backend::DynBackend;
pub use crate::backends::registry::BackendRegistry;
pub use crate::backends::registry::TryCreateFromConfig;
use axum::async_trait;
use shortguid::ShortGuid;
use std::error::Error;
use std::future::Future;
use std::sync::Arc;

mod dyn_backend;
mod map_ok;
#[cfg_attr(docsrs, doc(cfg(feature = "memcache")))]
#[cfg(feature = "memcache")]
pub mod memcache;
mod registry;

#[async_trait]
pub trait Backend: Send + Sync {
    /// Gets an informational string about the backend.
    fn backend_info(&self) -> &str;

    /// Gets the tag of the backend.
    fn tag(&self) -> &str;

    /// Handles a file that is ready for distribution.
    async fn distribute_file(
        &self,
        id: ShortGuid,
        summary: Arc<WriteSummary>,
    ) -> Result<(), DistributionError>;
}

#[derive(Debug, thiserror::Error)]
pub enum DistributionError {
    #[error(transparent)]
    Generic(#[from] Box<dyn Error>),
}
