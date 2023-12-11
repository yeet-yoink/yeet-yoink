use app_config::AppConfig;
use backend_traits::{
    BackendCommand, BackendCommandSender, BackendRegistration, DynBackend, RegisterBackendError,
    TryCreateFromConfig,
};
use file_distribution::FileAccessor;
use rendezvous::RendezvousGuard;
use std::cell::Cell;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::{JoinError, JoinHandle};
use tracing::{debug, error, info, warn};

const EVENT_BUFFER_SIZE: usize = 64;

pub struct BackendRegistry {
    handle: JoinHandle<()>,
    sender: Cell<Option<Sender<BackendCommand>>>,
}

impl BackendRegistry {
    pub fn builder(
        cleanup_rendezvous: RendezvousGuard,
        file_accessor: Arc<dyn FileAccessor>, // TODO: Refactor Arc<dyn FileAccessor> into DynFileAccessor
    ) -> BackendRegistryBuilder {
        BackendRegistryBuilder::new(cleanup_rendezvous, file_accessor)
    }

    fn new(
        cleanup_rendezvous: RendezvousGuard,
        backends: Vec<DynBackend>,
        file_accessor: Arc<dyn FileAccessor>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(EVENT_BUFFER_SIZE);
        let handle = tokio::spawn(Self::handle_events(
            backends,
            receiver,
            cleanup_rendezvous,
            file_accessor,
        ));
        Self {
            handle,
            sender: Cell::new(Some(sender)),
        }
    }

    pub(crate) fn get_sender(&self) -> Option<BackendCommandSender> {
        self.sender.take().map(BackendCommandSender::from)
    }

    pub async fn join(self) -> Result<(), JoinError> {
        self.handle.await
    }

    async fn handle_events(
        backends: Vec<DynBackend>,
        mut receiver: Receiver<BackendCommand>,
        cleanup_rendezvous: RendezvousGuard,
        file_accessor: Arc<dyn FileAccessor>,
    ) {
        while let Some(event) = receiver.recv().await {
            match event {
                BackendCommand::DistributeFile(id, summary) => {
                    // TODO: Handle file distribution
                    debug!(file_id = %id, "Handling distribution of file {id}", id = id);

                    // TODO: Spawn distribution tasks in background

                    // TODO: Initiate tasks in priority order?
                    for backend in &backends {
                        match backend
                            .distribute_file(id, summary.clone(), file_accessor.clone())
                            .await
                        {
                            Ok(_) => {}
                            Err(e) => {
                                warn!(file_id = %id, "Failed to distribute file using backend {tag}: {error}", tag = backend.tag(), error = e);
                            }
                        }
                    }
                }
            }
        }

        // TODO: Wait until all currently running tasks have finished.
        debug!("Closing backend event loop");
        cleanup_rendezvous.completed();
    }
}

pub struct BackendRegistryBuilder {
    backends: Vec<DynBackend>,
    cleanup_rendezvous: RendezvousGuard,
    file_accessor: Arc<dyn FileAccessor>,
}

impl BackendRegistration for BackendRegistryBuilder {
    fn add_backends<T>(self, config: &AppConfig) -> Result<(), RegisterBackendError>
    where
        T: TryCreateFromConfig,
    {
        self.add_backends::<T>(config)?;
        Ok(())
    }
}

impl BackendRegistryBuilder {
    fn new(cleanup_rendezvous: RendezvousGuard, file_accessor: Arc<dyn FileAccessor>) -> Self {
        Self {
            backends: Vec::default(),
            cleanup_rendezvous,
            file_accessor,
        }
    }

    pub fn build(self) -> BackendRegistry {
        BackendRegistry::new(self.cleanup_rendezvous, self.backends, self.file_accessor)
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
    pub fn add_backends<T>(
        self,
        config: &AppConfig,
    ) -> Result<BackendRegistryBuilder, RegisterBackendError>
    where
        T: TryCreateFromConfig,
    {
        match T::try_from_config(config)
            .map_err(|e| RegisterBackendError::TryCreateFromConfig(Box::new(e)))
        {
            Ok(backends) => {
                if !backends.is_empty() {
                    info!(
                "Registering {count} {backend} backend{plural} (backend version {backend_version})",
                count = backends.len(),
                backend = T::backend_name(),
                backend_version = T::backend_version(),
                plural = if backends.len() == 1 { "" } else { "s" }
            );
                    Ok(self.add_backends_from_iter(backends))
                } else {
                    Ok(self)
                }
            }
            Err(e) => {
                error!("Failed to initialize Memcached backends: {}", e);
                Err(e)
            }
        }
    }

    /// Registers multiple backends.
    fn add_backends_from_iter<I: IntoIterator<Item = DynBackend>>(
        mut self,
        backends: I,
    ) -> BackendRegistryBuilder {
        self.backends.extend(backends.into_iter());
        self
    }
}
