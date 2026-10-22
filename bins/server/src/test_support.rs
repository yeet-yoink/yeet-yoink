//! Test doubles shared by the server's unit tests.

use async_trait::async_trait;
use backend_traits::{DistributeFile, DistributionError, FetchError, FetchFile, FetchedFile};
use bytes::Bytes;
use file_distribution::hash::{HashMd5, HashSha256};
use file_distribution::{FileHashes, FileProvider, WriteSummary};
use shortguid::ShortGuid;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// A backend that serves files from memory and counts fetches.
#[derive(Clone, Default)]
pub struct FakeBackend {
    files: Arc<Mutex<HashMap<ShortGuid, FetchedFile>>>,
    fetches: Arc<AtomicUsize>,
    failing: Arc<AtomicBool>,
}

impl FakeBackend {
    /// A backend whose fetches always fail.
    pub fn failing() -> Self {
        let backend = Self::default();
        backend.failing.store(true, Ordering::SeqCst);
        backend
    }

    pub fn insert(&self, id: ShortGuid, file: FetchedFile) {
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
        Ok(self.files.lock().unwrap().get(&id).cloned())
    }
}

/// Creates a fetched file with matching hashes.
pub fn fetched_file(data: &'static [u8]) -> FetchedFile {
    let mut md5 = HashMd5::new();
    md5.update(data);
    let mut sha256 = HashSha256::new();
    sha256.update(data);
    FetchedFile {
        file_name: Some("fetched.txt".into()),
        content_type: Some("text/plain".into()),
        hashes: FileHashes::new(md5.finalize(), sha256.finalize()),
        data: Bytes::from_static(data),
        expires_at: None,
    }
}
