use async_trait::async_trait;
use file_distribution::BoxedFileReader;
use shortguid::ShortGuid;
use std::error::Error;

use crate::BackendTag;

/// Main trait for file receiving from a backend.
#[async_trait]
pub trait ReceiveFile: Send + Sync + BackendTag {
    /// Downloads a file from the backend.
    async fn receive_file(&self, id: ShortGuid) -> Result<BoxedFileReader, ReceiveError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ReceiveError {
    #[error(transparent)]
    BackendSpecific(Box<dyn Error>),
    #[error("No file found for the specified ID {0}")]
    UnknownFile(ShortGuid),
    #[error("The file lease has expired for the specified ID {0}")]
    FileExpired(ShortGuid),
}
