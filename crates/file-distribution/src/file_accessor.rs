use crate::BoxedFileReader;
use async_trait::async_trait;
use shortguid::ShortGuid;
use std::borrow::Borrow;
use std::sync::Arc;

#[async_trait]
pub trait FileAccessor: Sync + Send {
    async fn get_file(&self, id: ShortGuid) -> Result<BoxedFileReader, FileAccessorError>;
}

/// A wrapper around a dynamically dispatched [`FileAccessor`].
#[derive(Clone)]
pub struct DynFileAccessor(Arc<dyn FileAccessor>);

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

impl DynFileAccessor {
    /// Constructs a [`DynFileAccessor`] from an [`Arc`] containing a [`FileAccessor`] instance
    /// by cloning the [`Arc`].
    pub fn wrap<B, T>(accessor: B) -> Self
    where
        B: Borrow<Arc<T>>,
        T: FileAccessor + 'static,
    {
        Self(accessor.borrow().clone())
    }
}

#[async_trait]
impl FileAccessor for DynFileAccessor {
    async fn get_file(&self, id: ShortGuid) -> Result<BoxedFileReader, FileAccessorError> {
        self.0.get_file(id).await
    }
}
