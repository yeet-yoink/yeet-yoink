use crate::{Backend, BackendInfo, BackendRegistration, RegisterBackendError};
use app_config::AppConfig;
use std::error::Error;

pub trait TryCreateFromConfig: BackendInfo
where
    Self::Error: Error + 'static,
{
    type Error;

    fn try_from_config(config: &AppConfig) -> Result<Vec<Backend>, Self::Error>;

    fn register<T>(registry: T, config: &AppConfig) -> Result<(), RegisterBackendError>
    where
        Self: Sized,
        T: BackendRegistration,
    {
        registry.add_backends::<Self>(config)
    }
}
