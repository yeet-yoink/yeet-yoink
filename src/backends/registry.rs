use crate::app_config::AppConfig;
use crate::backends::DynBackend;

#[derive(Default)]
pub struct BackendRegistry {
    backends: Vec<DynBackend>,
}

impl BackendRegistry {
    /// Registers a backend.
    pub fn add_backend(&mut self, backend: DynBackend) {
        self.backends.push(backend)
    }

    /// Registers multiple backends.
    pub fn add_backends_from_iter<I: IntoIterator<Item = DynBackend>>(&mut self, backends: I) {
        self.backends.extend(backends.into_iter())
    }

    pub fn add_backends<T>(&mut self)
    where
        T: TryCreateFromConfig,
    {
    }
}

pub trait TryCreateFromConfig {
    type Error;

    fn try_from_config(config: &AppConfig) -> Result<Vec<DynBackend>, Self::Error>;
}
