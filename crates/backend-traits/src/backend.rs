use crate::{DistributeFile, ReceiveFile};
use std::ops::Deref;

pub trait BackendTag {
    /// Gets the tag of the backend.
    fn tag(&self) -> &str;
}

pub trait BackendTrait: BackendTag + ReceiveFile + DistributeFile {}

/// Auto-implementation for [`BackendTrait`] for supported types.
impl<T> BackendTrait for T where T: BackendTag + ReceiveFile + DistributeFile {}

/// [`Backend`] is a wrapper struct that holds a dynamically dispatched [`DistributeFile`] instance.
///
/// # Example
///
/// ```
/// use std::sync::Arc;
/// use async_trait::async_trait;
/// use shortguid::ShortGuid;
/// use backend_traits::{DistributeFile, DistributionError, Backend, BackendTag};
/// use file_distribution::{FileProvider, WriteSummary};
///
/// struct PostgresBackend;
///
/// impl BackendTag for PostgresBackend {
///     fn tag(&self) -> &str { "postgres" }
/// }
///
/// #[async_trait]
/// impl DistributeFile for PostgresBackend { //
///     async fn distribute_file(&self, id: ShortGuid, summary: Arc<WriteSummary>, file_accessor: FileProvider) -> Result<(), DistributionError> {
///         // ...
/// #       Ok(())
///     }
/// }
///
/// struct MySqlBackend;
///
/// impl BackendTag for MySqlBackend {
///     fn tag(&self) -> &str { "mysql" }
/// }
///
/// #[async_trait]
/// impl DistributeFile for MySqlBackend {
///     async fn distribute_file(&self, id: ShortGuid, summary: Arc<WriteSummary>, file_accessor: FileProvider) -> Result<(), DistributionError> {
///         // ...
/// #        Ok(())
///     }
/// }
///
/// let postgres_backend = Backend::wrap(PostgresBackend);
/// let my_sql_backend = Backend::wrap(MySqlBackend);
/// ```
pub struct Backend(Box<dyn BackendTrait>); // TODO: #54 Add ReceiveFile trait

impl Backend {
    pub fn new<T>(b: Box<T>) -> Self
    where
        T: BackendTrait + 'static,
    {
        Backend(b)
    }

    pub fn wrap<T>(b: T) -> Self
    where
        T: BackendTrait + 'static,
    {
        Self::new(Box::new(b))
    }
}

impl Deref for Backend {
    type Target = dyn BackendTrait;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl<T> From<Box<T>> for Backend
where
    T: BackendTrait + 'static,
{
    fn from(b: Box<T>) -> Self {
        Backend::new(b)
    }
}
