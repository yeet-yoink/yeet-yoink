use crate::app_config::AppConfig;
use crate::backends::DynBackend;
use std::error::Error;
use tracing::{error, info};

#[derive(Default)]
pub struct BackendRegistry {
    backends: Vec<DynBackend>,
}

impl BackendRegistry {
    /// Adds backends to the application.
    ///
    /// This function takes a type `T` that implements the `TryCreateFromConfig` trait, and a reference to an `AppConfig`.
    /// It tries to create backends from the given configuration using the `try_from_config` method of `T`.
    /// If successful, it adds the created backends to the application using the `add_backends_from_iter` method.
    ///
    /// # Arguments
    ///
    /// * `config` - A reference to an `AppConfig` that provides the configuration for creating the backends.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the backends were added successfully, otherwise returns a `RegisterBackendError`.
    ///
    /// # Errors
    ///
    /// This function may return a `RegisterBackendError` if an error occurs during the registration of the backends.
    ///
    /// # Examples
    ///
    /// ```
    /// use crate::backend::Backend;
    ///
    /// let mut app = App::new();
    /// let config = AppConfig::new();
    ///
    /// match app.add_backends::<MyBackend>(&config) {
    ///     Ok(()) => println!("Backends added successfully"),
    ///     Err(error) => eprintln!("Failed to add backends: {}", error),
    /// };
    /// ```
    pub fn add_backends<T>(&mut self, config: &AppConfig) -> Result<(), RegisterBackendError>
    where
        T: TryCreateFromConfig,
    {
        let backends = T::try_from_config(config)
            .map_err(|e| RegisterBackendError::TryCreateFromConfig(Box::new(e)))?;
        if !backends.is_empty() {
            info!(
                "Registering {count} {backend} backend{plural} (backend version {backend_version})",
                count = backends.len(),
                backend = T::backend_name(),
                backend_version = T::backend_version(),
                plural = if backends.len() == 1 { "" } else { "s" }
            );
            self.add_backends_from_iter(backends);
        }
        Ok(())
    }

    /// Registers multiple backends.
    fn add_backends_from_iter<I: IntoIterator<Item = DynBackend>>(&mut self, backends: I) {
        self.backends.extend(backends.into_iter())
    }
}

pub trait BackendInfo {
    /// Gets a short name of the backend.
    fn backend_name() -> &'static str;

    /// Gets an informational string about the backend.
    fn backend_version() -> &'static str {
        ""
    }
}

pub trait TryCreateFromConfig: BackendInfo
where
    Self::Error: Error + 'static,
{
    type Error;

    fn try_from_config(config: &AppConfig) -> Result<Vec<DynBackend>, Self::Error>;
}

#[derive(Debug, thiserror::Error)]
pub enum RegisterBackendError {
    #[error(transparent)]
    TryCreateFromConfig(Box<dyn Error>),
}
