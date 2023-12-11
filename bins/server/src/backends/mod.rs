use crate::backbone::{FileAccessor, FileAccessorError};
pub use crate::backends::dyn_backend::DynBackend;
pub use crate::backends::registry::{BackendCommand, BackendRegistry, TryCreateFromConfig};
use axum::async_trait;
use file_distribution::WriteSummary;
use shortguid::ShortGuid;
use std::error::Error;
use std::sync::Arc;

mod dyn_backend;
#[cfg_attr(docsrs, doc(cfg(feature = "memcache")))]
#[cfg(feature = "memcache")]
pub mod memcache;
mod registry;

#[async_trait]
pub trait Backend: Send + Sync {
    /// Gets the tag of the backend.
    fn tag(&self) -> &str;

    /// Handles a file that is ready for distribution.
    async fn distribute_file(
        &self,
        id: ShortGuid,
        summary: Arc<WriteSummary>,
        file_accessor: Arc<dyn FileAccessor>,
    ) -> Result<(), DistributionError>;
}

#[derive(Debug, thiserror::Error)]
pub enum DistributionError {
    #[error(transparent)]
    BackendSpecific(Box<dyn Error>),
    #[error(transparent)]
    FileAccessor(#[from] FileAccessorError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
}
