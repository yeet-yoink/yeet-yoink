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

    /// Gets the priority of the backend. Backends with a higher priority are
    /// fetched from first and have their distributions started first; backends
    /// with equal priority keep their registration order.
    fn priority(&self) -> i32 {
        0
    }

    /// Gets the largest file size, in bytes, this backend accepts for distribution.
    ///
    /// Files larger than this are not handed to the backend. `None` means no limit.
    fn max_file_size(&self) -> Option<u64> {
        None
    }

    /// Indicates whether this backend accepts a file of the given size for distribution.
    fn accepts_file_size(&self, size_bytes: u64) -> bool {
        self.max_file_size().is_none_or(|max| size_bytes <= max)
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A backend that relies on every default method.
    struct DefaultBackend;

    #[async_trait]
    impl DistributeFile for DefaultBackend {
        fn tag(&self) -> &str {
            "default"
        }

        async fn distribute_file(
            &self,
            _id: ShortGuid,
            _summary: Arc<WriteSummary>,
            _file_provider: FileProvider,
        ) -> Result<(), DistributionError> {
            Ok(())
        }
    }

    /// A backend with a size limit of 10 bytes.
    struct LimitedBackend;

    #[async_trait]
    impl DistributeFile for LimitedBackend {
        fn tag(&self) -> &str {
            "limited"
        }

        fn max_file_size(&self) -> Option<u64> {
            Some(10)
        }

        async fn distribute_file(
            &self,
            _id: ShortGuid,
            _summary: Arc<WriteSummary>,
            _file_provider: FileProvider,
        ) -> Result<(), DistributionError> {
            Ok(())
        }
    }

    #[test]
    fn defaults_to_neutral_priority_and_no_size_limit() {
        assert_eq!(DefaultBackend.priority(), 0);
        assert_eq!(DefaultBackend.max_file_size(), None);
        assert!(DefaultBackend.accepts_file_size(u64::MAX));
    }

    #[test]
    fn size_limit_is_inclusive() {
        assert!(LimitedBackend.accepts_file_size(10));
        assert!(!LimitedBackend.accepts_file_size(11));
    }
}
