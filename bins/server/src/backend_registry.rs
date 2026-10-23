use app_config::AppConfig;
use backend_traits::{
    Backend, BackendCommand, BackendCommandSender, BackendRegistration, FetchedFile,
    RegisterBackendError, TryCreateFromConfig,
};
use file_distribution::FileProvider;
use futures::future::join_all;
use rendezvous::RendezvousGuard;
use shortguid::ShortGuid;
use std::cell::Cell;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::task::{JoinError, JoinHandle};
use tracing::{debug, error, info, warn};

const EVENT_BUFFER_SIZE: usize = 64;

pub struct BackendRegistry {
    /// Handle to the event loop, retained for a future graceful `join` on shutdown.
    #[allow(dead_code)]
    handle: JoinHandle<()>,
    sender: Cell<Option<Sender<BackendCommand>>>,
    backends: Arc<Vec<Backend>>,
}

impl BackendRegistry {
    pub fn builder(
        cleanup_rendezvous: RendezvousGuard,
        file_accessor: FileProvider,
    ) -> BackendRegistryBuilder {
        BackendRegistryBuilder::new(cleanup_rendezvous, file_accessor)
    }

    fn new(
        cleanup_rendezvous: RendezvousGuard,
        mut backends: Vec<Backend>,
        file_accessor: FileProvider,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(EVENT_BUFFER_SIZE);
        sort_by_priority(&mut backends);
        let backends = Arc::new(backends);
        let handle = tokio::spawn(Self::handle_events(
            backends.clone(),
            receiver,
            cleanup_rendezvous,
            file_accessor,
        ));
        Self {
            handle,
            sender: Cell::new(Some(sender)),
            backends,
        }
    }

    pub(crate) fn get_sender(&self) -> Option<BackendCommandSender> {
        self.sender.take().map(BackendCommandSender::from)
    }

    /// Gets a handle for fetching files from the registered backends.
    pub(crate) fn fetcher(&self) -> BackendFetcher {
        BackendFetcher::new(self.backends.clone())
    }

    /// Awaits the backend event loop. Reserved for graceful shutdown wiring.
    #[allow(dead_code)]
    pub async fn join(self) -> Result<(), JoinError> {
        self.handle.await
    }

    async fn handle_events(
        backends: Arc<Vec<Backend>>,
        mut receiver: Receiver<BackendCommand>,
        cleanup_rendezvous: RendezvousGuard,
        file_accessor: FileProvider,
    ) {
        while let Some(event) = receiver.recv().await {
            match event {
                BackendCommand::DistributeFile(id, summary) => {
                    debug!(file_id = %id, "Handling distribution of file {id}", id = id);

                    // Distribute to all backends that accept the file's size
                    // concurrently in a detached task so that the event loop stays
                    // free to accept further commands. Distributions start in
                    // priority order. The forked rendezvous guard keeps graceful
                    // shutdown waiting until the distribution has finished.
                    let backends = backends.clone();
                    let file_accessor = file_accessor.clone();
                    let guard = cleanup_rendezvous.fork();
                    tokio::spawn(async move {
                        let file_size = summary.file_size_bytes as u64;
                        let distributions = Self::accepting_backends(&backends, id, file_size).map(|backend| {
                            let file_accessor = file_accessor.clone();
                            let summary = summary.clone();
                            async move {
                                if let Err(e) =
                                    backend.distribute_file(id, summary, file_accessor).await
                                {
                                    warn!(file_id = %id, "Failed to distribute file using backend {tag}: {error}", tag = backend.tag(), error = e);
                                }
                            }
                        });
                        join_all(distributions).await;
                        guard.completed();
                    });
                }
            }
        }

        debug!("Closing backend event loop");
        cleanup_rendezvous.completed();
    }

    /// Selects the backends whose size limit admits a file of the given size.
    fn accepting_backends(
        backends: &[Backend],
        id: ShortGuid,
        file_size: u64,
    ) -> impl Iterator<Item = &Backend> {
        backends.iter().filter(move |backend| {
            let accepted = backend.accepts_file_size(file_size);
            if !accepted {
                debug!(
                    file_id = %id,
                    "Skipping backend {tag}: file size {file_size} exceeds its limit of {max} bytes",
                    tag = backend.tag(),
                    max = backend.max_file_size().unwrap_or_default()
                );
            }
            accepted
        })
    }
}

/// Orders backends by descending priority, keeping registration order for ties.
fn sort_by_priority(backends: &mut [Backend]) {
    backends.sort_by_key(|backend| std::cmp::Reverse(backend.priority()));
}

/// Fetches files from the registered backends.
#[derive(Clone)]
pub struct BackendFetcher {
    backends: Arc<Vec<Backend>>,
}

impl BackendFetcher {
    pub(crate) fn new(backends: Arc<Vec<Backend>>) -> Self {
        Self { backends }
    }

    /// Fetches a file from the first backend that holds an intact, unexpired copy.
    ///
    /// Backends are queried in priority order. Errors, expired and corrupt
    /// copies are logged and skipped.
    pub async fn fetch(&self, id: ShortGuid) -> Option<FetchedFile> {
        for backend in self.backends.iter() {
            match backend.fetch_file(id).await {
                Ok(Some(file)) if file.is_expired() => {
                    debug!(file_id = %id, "Backend {tag} returned an expired copy of file {id}", tag = backend.tag());
                }
                Ok(Some(file)) if file.is_intact() => {
                    debug!(file_id = %id, "Fetched file {id} from backend {tag}", tag = backend.tag());
                    return Some(file);
                }
                Ok(Some(_)) => {
                    warn!(file_id = %id, "Backend {tag} returned a corrupt copy of file {id}", tag = backend.tag());
                }
                Ok(None) => {}
                Err(e) => {
                    warn!(file_id = %id, "Failed to fetch file {id} from backend {tag}: {error}", tag = backend.tag(), error = e);
                }
            }
        }
        None
    }
}

pub struct BackendRegistryBuilder {
    backends: Vec<Backend>,
    cleanup_rendezvous: RendezvousGuard,
    file_accessor: FileProvider,
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
    fn new(cleanup_rendezvous: RendezvousGuard, file_accessor: FileProvider) -> Self {
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
    fn add_backends_from_iter<I: IntoIterator<Item = Backend>>(
        mut self,
        backends: I,
    ) -> BackendRegistryBuilder {
        self.backends.extend(backends);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeBackend, fetched_file};
    use bytes::Bytes;

    fn fetcher(backends: &[FakeBackend]) -> BackendFetcher {
        BackendFetcher::new(Arc::new(
            backends.iter().cloned().map(Backend::wrap).collect(),
        ))
    }

    #[tokio::test]
    async fn fetch_skips_failing_backends() {
        let failing = FakeBackend::failing();
        let healthy = FakeBackend::default();
        let id = ShortGuid::new_random();
        healthy.insert(id, fetched_file(b"abc"));

        let file = fetcher(&[failing.clone(), healthy.clone()]).fetch(id).await;

        assert_eq!(file.map(|f| f.data), Some(Bytes::from_static(b"abc")));
        assert_eq!((failing.fetches(), healthy.fetches()), (1, 1));
    }

    #[tokio::test]
    async fn fetch_skips_corrupt_copies() {
        let corrupt = FakeBackend::default();
        let healthy = FakeBackend::default();
        let id = ShortGuid::new_random();
        corrupt.insert(
            id,
            FetchedFile {
                data: Bytes::from_static(b"abd"),
                ..fetched_file(b"abc")
            },
        );
        healthy.insert(id, fetched_file(b"abc"));

        let file = fetcher(&[corrupt, healthy]).fetch(id).await;

        assert_eq!(file.map(|f| f.data), Some(Bytes::from_static(b"abc")));
    }

    #[tokio::test]
    async fn fetch_skips_expired_copies() {
        use std::time::{Duration, SystemTime};

        let expired = FakeBackend::default();
        let id = ShortGuid::new_random();
        expired.insert(
            id,
            FetchedFile {
                expires_at: Some(SystemTime::now() - Duration::from_secs(1)),
                ..fetched_file(b"abc")
            },
        );

        assert!(fetcher(&[expired]).fetch(id).await.is_none());
    }

    #[tokio::test]
    async fn fetch_stops_at_the_first_hit() {
        let first = FakeBackend::default();
        let second = FakeBackend::default();
        let id = ShortGuid::new_random();
        first.insert(id, fetched_file(b"abc"));
        second.insert(id, fetched_file(b"abc"));

        assert!(fetcher(&[first, second.clone()]).fetch(id).await.is_some());
        assert_eq!(second.fetches(), 0);
    }

    fn accepting_count(backends: &[FakeBackend], file_size: u64) -> usize {
        let backends: Vec<Backend> = backends.iter().cloned().map(Backend::wrap).collect();
        BackendRegistry::accepting_backends(&backends, ShortGuid::new_random(), file_size).count()
    }

    #[test]
    fn distribution_skips_backends_below_the_file_size() {
        let small = FakeBackend::with_max_file_size(10);
        let unlimited = FakeBackend::default();

        assert_eq!(accepting_count(&[small.clone(), unlimited.clone()], 11), 1);
        assert_eq!(accepting_count(&[small, unlimited], 10), 2);
    }

    /// Wraps the backends and orders them as the registry does.
    fn prioritized(backends: &[FakeBackend]) -> BackendFetcher {
        let mut backends: Vec<Backend> = backends.iter().cloned().map(Backend::wrap).collect();
        sort_by_priority(&mut backends);
        BackendFetcher::new(Arc::new(backends))
    }

    #[tokio::test]
    async fn fetch_prefers_higher_priority_backends() {
        let low = FakeBackend::default().with_priority(-1);
        let default = FakeBackend::default();
        let high = FakeBackend::default().with_priority(10);
        let id = ShortGuid::new_random();
        for backend in [&low, &default, &high] {
            backend.insert(id, fetched_file(b"abc"));
        }

        assert!(
            prioritized(&[low.clone(), default.clone(), high.clone()])
                .fetch(id)
                .await
                .is_some()
        );
        assert_eq!(
            (high.fetches(), default.fetches(), low.fetches()),
            (1, 0, 0)
        );
    }

    #[tokio::test]
    async fn fetch_falls_back_to_lower_priority_backends() {
        let high = FakeBackend::default().with_priority(10);
        let low = FakeBackend::default();
        let id = ShortGuid::new_random();
        low.insert(id, fetched_file(b"abc"));

        assert!(
            prioritized(&[low.clone(), high.clone()])
                .fetch(id)
                .await
                .is_some()
        );
        assert_eq!((high.fetches(), low.fetches()), (1, 1));
    }

    #[tokio::test]
    async fn equal_priorities_keep_registration_order() {
        let first = FakeBackend::default();
        let second = FakeBackend::default();
        let id = ShortGuid::new_random();
        first.insert(id, fetched_file(b"abc"));
        second.insert(id, fetched_file(b"abc"));

        assert!(
            prioritized(&[first.clone(), second.clone()])
                .fetch(id)
                .await
                .is_some()
        );
        assert_eq!((first.fetches(), second.fetches()), (1, 0));
    }

    #[tokio::test]
    async fn fetch_without_backends_finds_nothing() {
        assert!(fetcher(&[]).fetch(ShortGuid::new_random()).await.is_none());
    }
}
