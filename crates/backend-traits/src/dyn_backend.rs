use crate::Backend;
use std::ops::Deref;

/// `DynBackend` is a wrapper struct that holds a boxed trait object,
/// enabling dynamic dispatch for different implementations of the `Backend` trait.
///
/// # Example
///
/// ```
/// use backend_traits::DynBackend;
///
/// trait Backend {
///     fn execute(&self, query: &str) -> String;
/// }
///
/// struct PostgresBackend;
/// impl Backend for PostgresBackend {
///     fn execute(&self, query: &str) -> String {
///         // Perform query execution logic specific to Postgres
///         // ...
///     }
/// }
///
/// struct MySqlBackend;
/// impl Backend for MySqlBackend {
///     fn execute(&self, query: &str) -> String {
///         // Perform query execution logic specific to MySql
///         // ...
///     }
/// }
///
/// let postgres_backend: DynBackend = DynBackend::new(Box::new(PostgresBackend));
/// let my_sql_backend: DynBackend = DynBackend::new(Box::new(MySqlBackend));
/// ```
pub struct DynBackend(Box<dyn Backend>);

impl DynBackend {
    pub fn new<T>(b: Box<T>) -> Self
    where
        T: Backend + 'static,
    {
        DynBackend(b)
    }
}

impl Deref for DynBackend {
    type Target = dyn Backend;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl<T> From<Box<T>> for DynBackend
where
    T: Backend + 'static,
{
    fn from(b: Box<T>) -> Self {
        DynBackend::new(b)
    }
}
