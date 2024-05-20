use crate::BackendTag;
use async_trait::async_trait;
use file_distribution::{FileAccessorError, FileProvider, WriteSummary};
use shortguid::ShortGuid;
use std::error::Error;
use std::sync::Arc;

/// Main trait for file distribution to a backend.
#[async_trait]
pub trait DistributeFile: Send + Sync + BackendTag {
    /// Handles a file that is ready for distribution.
    async fn distribute_file(
        &self,
        id: ShortGuid,
        summary: Arc<WriteSummary>,
        file_provider: FileProvider,
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
