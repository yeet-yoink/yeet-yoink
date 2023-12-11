use crate::{TryCreateFromConfig};
use app_config::AppConfig;
use std::error::Error;

pub trait BackendRegistration {
    fn add_backends<T>(self, config: &AppConfig) -> Result<(), RegisterBackendError>
    where
        T: TryCreateFromConfig;
}

#[derive(Debug, thiserror::Error)]
pub enum RegisterBackendError {
    #[error(transparent)]
    TryCreateFromConfig(Box<dyn Error>),
}
