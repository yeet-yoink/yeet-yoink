use crate::BoxedFileReader;
use async_trait::async_trait;
use shortguid::ShortGuid;
use std::borrow::Borrow;
use std::error::Error;
use std::sync::Arc;

/// A wrapper around a dynamically dispatched [`GetFile`].
#[derive(Clone)]
pub struct FileProvider(Arc<dyn GetFile>);

/// Trait for registries that provide access to files by their identifier.
#[async_trait]
pub trait GetFile: Sync + Send {
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
    #[error("Internal error for file with ID {0}: {1}")]
    InternalError(ShortGuid, Box<dyn Error>),
}

impl FileProvider {
    /// Constructs a [`FileProvider`] from an [`Arc`] containing a [`GetFile`] instance
    /// by cloning the [`Arc`].
    pub fn wrap<B, T>(accessor: B) -> Self
    where
        B: Borrow<Arc<T>>,
        T: GetFile + 'static,
    {
        Self(accessor.borrow().clone())
    }
}

#[async_trait]
impl GetFile for FileProvider {
    async fn get_file(&self, id: ShortGuid) -> Result<BoxedFileReader, FileAccessorError> {
        self.0.get_file(id).await
    }
}
