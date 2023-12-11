mod file_reader;

use async_trait::async_trait;
pub use file_reader::{BoxedFileReader, FileReaderTrait};
use shortguid::ShortGuid;

#[async_trait]
pub trait FileAccessor: Sync + Send {
    async fn get_file(&self, id: ShortGuid) -> Result<BoxedFileReader, FileAccessorError>;
}

#[derive(Debug, thiserror::Error)]
pub enum FileAccessorError {
    #[error("The backbone is unavailable")]
    BackboneUnavailable,
    #[error("Unable to obtain a lock on a mutex")]
    FailedToLock,
    #[error(transparent)]
    GetReaderError(#[from] GetFileReaderError),
}

#[derive(Debug, thiserror::Error)]
pub enum GetFileReaderError {
    #[error("No file found for the specified ID {0}")]
    UnknownFile(ShortGuid),
    #[error("The file lease has expired for the specified ID {0}")]
    FileExpired(ShortGuid),
    #[error("Failed to open the file for ID {0}: {1}")]
    FileError(ShortGuid, async_tempfile::Error),
}
