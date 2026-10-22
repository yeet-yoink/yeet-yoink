use crate::FetchFile;
use async_trait::async_trait;
use file_distribution::{FileAccessorError, FileProvider, WriteSummary};
use shortguid::ShortGuid;
use std::error::Error;
use std::ops::Deref;
use std::sync::Arc;

/// Main trait for file distribution to a backend.
#[async_trait]
pub trait DistributeFile: Send + Sync {
    /// Gets the tag of the backend.
    fn tag(&self) -> &str;

    /// Handles a file that is ready for distribution.
    async fn distribute_file(
        &self,
        id: ShortGuid,
        summary: Arc<WriteSummary>,
        file_provider: FileProvider,
    ) -> Result<(), DistributionError>;
}

/// A storage backend: it receives files for distribution and can serve them back.
pub trait StorageBackend: DistributeFile + FetchFile {}

impl<T> StorageBackend for T where T: DistributeFile + FetchFile {}

/// [`Backend`] is a wrapper struct that holds a dynamically dispatched [`StorageBackend`] instance.
///
/// # Example
///
/// ```
/// use std::sync::Arc;
/// use async_trait::async_trait;
/// use shortguid::ShortGuid;
/// use backend_traits::{Backend, DistributeFile, DistributionError, FetchError, FetchFile, FetchedFile};
/// use file_distribution::{FileProvider, WriteSummary};
///
/// struct PostgresBackend;
///
/// #[async_trait]
/// impl DistributeFile for PostgresBackend {
///     fn tag(&self) -> &str { "postgres" }
///
///     async fn distribute_file(&self, id: ShortGuid, summary: Arc<WriteSummary>, file_accessor: FileProvider) -> Result<(), DistributionError> {
///         // ...
/// #       Ok(())
///     }
/// }
///
/// #[async_trait]
/// impl FetchFile for PostgresBackend {
///     async fn fetch_file(&self, id: ShortGuid) -> Result<Option<FetchedFile>, FetchError> {
///         // ...
/// #       Ok(None)
///     }
/// }
///
/// let postgres_backend = Backend::wrap(PostgresBackend);
/// ```
pub struct Backend(Box<dyn StorageBackend>);

impl Backend {
    pub fn new<T>(b: Box<T>) -> Self
    where
        T: StorageBackend + 'static,
    {
        Backend(b)
    }

    pub fn wrap<T>(b: T) -> Self
    where
        T: StorageBackend + 'static,
    {
        Self::new(Box::new(b))
    }
}

impl Deref for Backend {
    type Target = dyn StorageBackend;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl<T> From<Box<T>> for Backend
where
    T: StorageBackend + 'static,
{
    fn from(b: Box<T>) -> Self {
        Backend::new(b)
    }
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
