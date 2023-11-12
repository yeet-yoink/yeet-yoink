use crate::app_config::AppConfig;
use crate::backbone::WriteSummary;
use crate::backends::DynBackend;
use crate::shutdown_rendezvous::ShutdownRendezvousEvent;
use shortguid::ShortGuid;
use std::error::Error;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::{JoinError, JoinHandle};
use tracing::{debug, error, info};

const EVENT_BUFFER_SIZE: usize = 64;

pub struct BackendRegistry {
    backends: Vec<DynBackend>,
    handle: JoinHandle<()>,
    sender: Option<Sender<BackendCommand>>,
}

impl BackendRegistry {
    pub fn new(cleanup_rendezvous: Sender<ShutdownRendezvousEvent>) -> Self {
        let (sender, receiver) = mpsc::channel(EVENT_BUFFER_SIZE);
        let handle = tokio::spawn(Self::handle_events(receiver, cleanup_rendezvous));
        Self {
            backends: Vec::default(),
            handle,
            sender: Some(sender),
        }
    }

    pub(crate) fn get_sender(&mut self) -> Option<Sender<BackendCommand>> {
        self.sender.take()
    }

    pub async fn join(self) -> Result<(), JoinError> {
        self.handle.await
    }

    async fn handle_events(
        mut receiver: Receiver<BackendCommand>,
        cleanup_rendezvous: Sender<ShutdownRendezvousEvent>,
    ) {
        while let Some(event) = receiver.recv().await {
            match event {
                BackendCommand::DistributeFile(id, _summary) => {
                    // TODO: Handle file distribution
                    debug!(file_id = %id, "Handling distribution of file {id}", id = id);
                }
            }
        }

        // TODO: Wait until all currently running tasks have finished.
        debug!("Closing backend event loop");
        cleanup_rendezvous
            .send(ShutdownRendezvousEvent::BackendRegistry)
            .await
            .ok();
    }

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

    fn register(
        registry: &mut BackendRegistry,
        config: &AppConfig,
    ) -> Result<(), RegisterBackendError>
    where
        Self: Sized,
    {
        registry.add_backends::<Self>(&config)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegisterBackendError {
    #[error(transparent)]
    TryCreateFromConfig(Box<dyn Error>),
}

pub enum BackendCommand {
    DistributeFile(ShortGuid, Arc<WriteSummary>),
}