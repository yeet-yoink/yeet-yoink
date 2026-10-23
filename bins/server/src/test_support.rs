//! Test doubles shared by the server's unit tests.

use crate::AppState;
use crate::backend_registry::BackendFetcher;
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use backbone::Backbone;
use backend_traits::{Backend, BackendCommand};
use backend_traits::{DistributeFile, DistributionError, FetchError, FetchFile, FetchedFile};
use bytes::Bytes;
use file_distribution::{FileHashes, FileProvider, WriteSummary};
use rendezvous::Rendezvous;
use shortguid::ShortGuid;
use std::collections::HashMap;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::SystemTime;
use tokio::sync::{broadcast, mpsc};
use tower::ServiceExt;

/// A backend that serves files from memory and counts fetches.
#[derive(Clone, Default)]
pub struct FakeBackend {
    files: Arc<Mutex<HashMap<ShortGuid, FakeFile>>>,
    fetches: Arc<AtomicUsize>,
    failing: Arc<AtomicBool>,
    max_file_size: Option<u64>,
    priority: i32,
}

impl FakeBackend {
    /// A backend whose fetches always fail.
    pub fn failing() -> Self {
        let backend = Self::default();
        backend.failing.store(true, Ordering::SeqCst);
        backend
    }

    /// A backend that accepts files up to the given size for distribution.
    pub fn with_max_file_size(max_file_size: u64) -> Self {
        Self {
            max_file_size: Some(max_file_size),
            ..Self::default()
        }
    }

    /// Sets the backend's priority.
    pub fn with_priority(self, priority: i32) -> Self {
        Self { priority, ..self }
    }

    pub fn insert(&self, id: ShortGuid, file: FakeFile) {
        self.files.lock().unwrap().insert(id, file);
    }

    pub fn clear(&self) {
        self.files.lock().unwrap().clear();
    }

    pub fn fetches(&self) -> usize {
        self.fetches.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl DistributeFile for FakeBackend {
    fn tag(&self) -> &str {
        "fake"
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    fn max_file_size(&self) -> Option<u64> {
        self.max_file_size
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

#[async_trait]
impl FetchFile for FakeBackend {
    async fn fetch_file(&self, id: ShortGuid) -> Result<Option<FetchedFile>, FetchError> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        if self.failing.load(Ordering::SeqCst) {
            return Err(FetchError::InvalidMetadata("simulated failure".into()));
        }
        Ok(self
            .files
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .map(FakeFile::into_fetched))
    }
}

/// A file held by a [`FakeBackend`]; each fetch streams a fresh copy of it.
#[derive(Clone)]
pub struct FakeFile {
    pub file_name: Option<String>,
    pub content_type: Option<String>,
    pub hashes: FileHashes,
    pub data: Bytes,
    pub expires_at: Option<SystemTime>,
}

impl FakeFile {
    fn into_fetched(self) -> FetchedFile {
        FetchedFile {
            file_name: self.file_name,
            content_type: self.content_type,
            hashes: self.hashes,
            size: Some(self.data.len() as u64),
            content: Box::new(Cursor::new(self.data)),
            expires_at: self.expires_at,
        }
    }
}

/// Creates a fake file with matching hashes.
pub fn fetched_file(data: &'static [u8]) -> FakeFile {
    FakeFile {
        file_name: Some("fetched.txt".into()),
        content_type: Some("text/plain".into()),
        hashes: FileHashes::of(data),
        data: Bytes::from_static(data),
        expires_at: None,
    }
}

/// The application state wired to a single [`FakeBackend`], as the server sets
/// it up.
pub struct TestApp {
    pub state: AppState,
    pub backend: FakeBackend,
    _backend_receiver: mpsc::Receiver<BackendCommand>,
}

/// Creates the application state with a fresh backbone and one fake backend.
pub fn test_app(near_cache: bool) -> TestApp {
    let (sender, backend_receiver) = mpsc::channel(16);
    let rendezvous = Rendezvous::new();
    let backbone = Backbone::new(
        sender.into(),
        rendezvous.fork_guard(),
        Duration::from_secs(3600),
    );

    // Dropping a Rendezvous blocks until all guards complete.
    std::mem::forget(rendezvous);

    let backend = FakeBackend::default();
    let state = AppState {
        shutdown_tx: broadcast::channel(1).0,
        backbone: Arc::new(backbone),
        backends: BackendFetcher::new(Arc::new(vec![Backend::wrap(backend.clone())])),
        near_cache,
    };
    TestApp {
        state,
        backend,
        _backend_receiver: backend_receiver,
    }
}

/// Sends a request through the router and collects the response.
pub async fn send(state: &AppState, request: Request<Body>) -> (StatusCode, HeaderMap, Bytes) {
    let response = crate::router(state.clone())
        .oneshot(request)
        .await
        .expect("Request failed");
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("Failed to read body");
    (status, headers, body)
}
