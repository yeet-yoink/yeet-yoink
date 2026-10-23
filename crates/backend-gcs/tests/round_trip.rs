//! Integration tests for the Google Cloud Storage backend.
//!
//! Most tests run against an in-memory object store. Set `YY_TEST_GCS` to the
//! URL of a Cloud Storage emulator such as `http://localhost:4443` to also run
//! the tests against it; they are skipped otherwise. fake-gcs-server routes by
//! host, so the URL must use the host given as its `-public-host`.

use app_config::gcs::GcsBackendConfig;
use axum_extra::headers::ContentType;
use backbone::{Backbone, CompletionMode, FileAccessorBridge};
use backend_gcs::{GcsBackend, METADATA_ATTRIBUTE};
use backend_traits::{DistributeFile, DistributionError, FetchError, FetchFile, FetchedFile};
use file_distribution::{FileHashes, FileProvider, WriteSummary};
use futures::StreamExt;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{Attribute, Attributes, ObjectStore, ObjectStoreExt, PutOptions, PutPayload};
use rendezvous::Rendezvous;
use shortguid::ShortGuid;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::mpsc;

const EMULATOR_URL_VAR: &str = "YY_TEST_GCS";
const EMULATOR_BUCKET: &str = "yeet-yoink-test";

fn config(expiration_sec: Option<u32>) -> GcsBackendConfig {
    GcsBackendConfig {
        tag: "test".into(),
        bucket: EMULATOR_BUCKET.into(),
        prefix: Some("files".into()),
        expiration_sec,
        ..GcsBackendConfig::default()
    }
}

/// Creates a backend on an empty in-memory store and returns both.
fn in_memory(config: &GcsBackendConfig) -> (GcsBackend, Arc<InMemory>) {
    let store = Arc::new(InMemory::new());
    (GcsBackend::with_store(config, store.clone()), store)
}

/// Creates a backend for the emulator, or returns `None` if none is configured.
async fn emulator(expiration_sec: Option<u32>) -> Option<GcsBackend> {
    let Ok(url) = std::env::var(EMULATOR_URL_VAR) else {
        eprintln!("{EMULATOR_URL_VAR} is not set; skipping Cloud Storage emulator test");
        return None;
    };

    // Creating an existing bucket fails with 409 Conflict, which is fine.
    let response = reqwest::Client::new()
        .post(format!("{}/storage/v1/b", url.trim_end_matches('/')))
        .body(format!(r#"{{"name":"{EMULATOR_BUCKET}"}}"#))
        .send()
        .await
        .expect("Failed to reach the Cloud Storage emulator");
    assert!(
        response.status().is_success() || response.status() == reqwest::StatusCode::CONFLICT,
        "Failed to create bucket: {}",
        response.status()
    );

    let config = GcsBackendConfig {
        emulator_url: Some(url.parse().expect("Invalid emulator URL")),
        ..config(expiration_sec)
    };
    Some(GcsBackend::try_new(&config).expect("Failed to create GCS backend"))
}

/// Reads the content of a fetched file, keeping its metadata.
async fn read_content(mut file: FetchedFile) -> (FetchedFile, Vec<u8>) {
    use tokio::io::AsyncReadExt;
    let mut data = Vec::new();
    file.content
        .read_to_end(&mut data)
        .await
        .expect("Failed to read content");
    (file, data)
}

/// Buffers a file in a backbone and returns a provider for it, as the server does.
async fn buffered_file(id: ShortGuid, data: &[u8]) -> (FileProvider, Arc<WriteSummary>) {
    let (sender, receiver) = mpsc::channel(16);
    // The test distributes manually; keep the channel open.
    std::mem::forget(receiver);

    let rendezvous = Rendezvous::new();
    let backbone = Arc::new(Backbone::new(
        sender.into(),
        rendezvous.fork_guard(),
        Duration::from_secs(60),
    ));
    // Dropping a Rendezvous blocks until all guards complete.
    std::mem::forget(rendezvous);

    let mut writer = backbone
        .new_file(
            id,
            Some(data.len() as u64),
            Some(ContentType::text()),
            None,
            Some("hello.txt".into()),
        )
        .await
        .expect("Failed to create file");
    // Writes may be partial; write until everything is buffered.
    let mut remaining = data;
    while !remaining.is_empty() {
        let written = writer.write(remaining).await.expect("Failed to write");
        remaining = &remaining[written..];
    }
    writer.sync_data().await.expect("Failed to sync");
    let summary = writer
        .finalize(CompletionMode::NoSync)
        .await
        .expect("Failed to finalize");

    let accessor = Arc::new(FileAccessorBridge::default());
    accessor.set_backbone(&backbone);
    // The accessor only holds a weak reference; keep the backbone alive.
    std::mem::forget(backbone);
    (FileProvider::wrap(&accessor), summary)
}

/// Distributes a file and fetches it back, checking content and metadata.
async fn assert_round_trip(backend: &GcsBackend) {
    let id = ShortGuid::new_random();
    let (provider, summary) = buffered_file(id, b"hello, cloud storage").await;

    backend
        .distribute_file(id, summary.clone(), provider)
        .await
        .expect("Failed to distribute");
    let file = backend
        .fetch_file(id)
        .await
        .expect("Failed to fetch")
        .expect("File not found");
    let (file, data) = read_content(file).await;

    assert_eq!(data, &b"hello, cloud storage"[..]);
    assert_eq!(file.file_name.as_deref(), Some("hello.txt"));
    assert_eq!(file.content_type.as_deref(), Some("text/plain"));
    assert_eq!(file.hashes.md5, summary.hashes.md5);
    assert_eq!(file.hashes.sha256, summary.hashes.sha256);
    assert_eq!(FileHashes::of(&data), file.hashes);

    let expires_at = file.expires_at.expect("Missing expiry");
    let remaining = expires_at
        .duration_since(SystemTime::now())
        .expect("Already expired");
    assert!(remaining <= Duration::from_secs(60));
    assert!(remaining > Duration::from_secs(50));
}

/// Stores an object directly, bypassing the backend.
async fn put_raw(store: &InMemory, id: ShortGuid, data: &'static [u8], attributes: Attributes) {
    store
        .put_opts(
            &Path::from(format!("files/{id}")),
            PutPayload::from_static(data),
            PutOptions {
                attributes,
                ..PutOptions::default()
            },
        )
        .await
        .expect("Failed to store object");
}

#[tokio::test]
async fn distributed_file_can_be_fetched() {
    let (backend, _) = in_memory(&config(Some(60)));
    assert_round_trip(&backend).await;
}

#[tokio::test]
async fn object_carries_content_type_and_metadata() {
    let (backend, store) = in_memory(&config(Some(60)));
    let id = ShortGuid::new_random();
    let (provider, summary) = buffered_file(id, b"abc").await;
    backend
        .distribute_file(id, summary, provider)
        .await
        .expect("Failed to distribute");

    let object = store
        .get(&Path::from(format!("files/{id}")))
        .await
        .expect("Object not stored under the prefix");
    assert_eq!(
        object
            .attributes
            .get(&Attribute::ContentType)
            .map(AsRef::as_ref),
        Some("text/plain")
    );
    assert!(
        object
            .attributes
            .get(&Attribute::Metadata(METADATA_ATTRIBUTE.into()))
            .is_some()
    );
}

#[tokio::test]
async fn unknown_file_is_not_found() {
    let (backend, _) = in_memory(&config(Some(60)));
    let file = backend
        .fetch_file(ShortGuid::new_random())
        .await
        .expect("Failed to fetch");
    assert!(file.is_none());
}

#[tokio::test]
async fn zero_expiration_keeps_the_file_indefinitely() {
    let (backend, _) = in_memory(&config(Some(0)));
    let id = ShortGuid::new_random();
    let (provider, summary) = buffered_file(id, b"forever").await;

    backend
        .distribute_file(id, summary, provider)
        .await
        .expect("Failed to distribute");
    let file = backend
        .fetch_file(id)
        .await
        .expect("Failed to fetch")
        .expect("File not found");
    let (file, _) = read_content(file).await;
    assert_eq!(file.expires_at, None);
}

#[tokio::test]
async fn object_without_metadata_is_rejected() {
    let (backend, store) = in_memory(&config(Some(60)));
    let id = ShortGuid::new_random();
    put_raw(&store, id, b"abc", Attributes::new()).await;

    let result = backend.fetch_file(id).await;
    assert!(matches!(result, Err(FetchError::InvalidMetadata(_))));
}

#[tokio::test]
async fn malformed_metadata_is_rejected() {
    let (backend, store) = in_memory(&config(Some(60)));
    let id = ShortGuid::new_random();
    let mut attributes = Attributes::new();
    attributes.insert(
        Attribute::Metadata(METADATA_ATTRIBUTE.into()),
        "not base64!".into(),
    );
    put_raw(&store, id, b"abc", attributes).await;

    let result = backend.fetch_file(id).await;
    assert!(matches!(result, Err(FetchError::InvalidMetadata(_))));
}

#[tokio::test]
async fn objects_above_the_size_limit_are_not_fetched() {
    let config = GcsBackendConfig {
        max_file_size_bytes: Some(2),
        ..config(Some(60))
    };
    let (backend, store) = in_memory(&config);
    let id = ShortGuid::new_random();
    put_raw(&store, id, b"abc", Attributes::new()).await;

    let result = backend.fetch_file(id).await;
    assert!(matches!(result, Err(FetchError::BackendSpecific(_))));
}

/// Requests must fail fast while Cloud Storage is unreachable, so that the
/// server moves on to the next backend instead of waiting out retries.
#[tokio::test]
async fn unreachable_storage_fails_fast() {
    let config = GcsBackendConfig {
        emulator_url: Some("http://127.0.0.1:1".parse().expect("Invalid URL")),
        ..config(Some(60))
    };
    let backend = GcsBackend::try_new(&config).expect("Failed to create GCS backend");
    let id = ShortGuid::new_random();

    let start = Instant::now();
    let fetched = backend.fetch_file(id).await;
    assert!(matches!(fetched, Err(FetchError::BackendSpecific(_))));
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "fetch took {:?}",
        start.elapsed()
    );

    let (provider, summary) = buffered_file(id, b"nowhere to go").await;
    let distributed = backend.distribute_file(id, summary, provider).await;
    assert!(matches!(
        distributed,
        Err(DistributionError::BackendSpecific(_))
    ));
}

#[tokio::test]
async fn emulator_round_trip() {
    let Some(backend) = emulator(Some(60)).await else {
        return;
    };
    assert_round_trip(&backend).await;
}

#[tokio::test]
async fn emulator_unknown_file_is_not_found() {
    let Some(backend) = emulator(Some(60)).await else {
        return;
    };
    let file = backend
        .fetch_file(ShortGuid::new_random())
        .await
        .expect("Failed to fetch");
    assert!(file.is_none());
}

/// Files above the multipart threshold are uploaded in parts, keeping their
/// metadata and content type.
#[tokio::test]
async fn large_file_is_uploaded_in_parts() {
    let (backend, store) = in_memory(&config(Some(60)));
    let id = ShortGuid::new_random();
    let data: Vec<u8> = (0..10 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    let (provider, summary) = buffered_file(id, &data).await;

    backend
        .distribute_file(id, summary.clone(), provider)
        .await
        .expect("Failed to distribute");

    let object = store
        .get(&Path::from(format!("files/{id}")))
        .await
        .expect("Object not stored");
    assert_eq!(object.meta.size, data.len() as u64);
    assert_eq!(
        object
            .attributes
            .get(&Attribute::ContentType)
            .map(AsRef::as_ref),
        Some("text/plain")
    );

    let file = backend
        .fetch_file(id)
        .await
        .expect("Failed to fetch")
        .expect("File not found");
    let (file, fetched) = read_content(file).await;
    assert!(fetched == data, "Content differs");
    assert_eq!(file.hashes, summary.hashes);
    assert_eq!(file.file_name.as_deref(), Some("hello.txt"));

    // Only the completed object exists; no stray parts are left behind.
    let objects: Vec<_> = store.list(None).collect::<Vec<_>>().await;
    assert_eq!(objects.len(), 1);
}

/// A local file that breaks off after a number of bytes.
struct BrokenFile {
    remaining: usize,
}

impl tokio::io::AsyncRead for BrokenFile {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if self.remaining == 0 {
            return std::task::Poll::Ready(Err(std::io::ErrorKind::UnexpectedEof.into()));
        }
        let count = buf.remaining().min(self.remaining);
        buf.put_slice(&vec![b'x'; count]);
        self.remaining -= count;
        std::task::Poll::Ready(Ok(()))
    }
}

impl file_distribution::FileReaderTrait for BrokenFile {
    fn summary(&self) -> &Option<Arc<WriteSummary>> {
        &None
    }
    fn expiration_date(&self) -> tokio::time::Instant {
        tokio::time::Instant::now()
    }
    fn file_size(&self) -> shared_files::FileSize {
        shared_files::FileSize::AtLeast(0)
    }
    fn file_age(&self) -> Duration {
        Duration::ZERO
    }
    fn content_type(&self) -> Option<std::borrow::Cow<'_, str>> {
        None
    }
}

/// Provides a file that breaks off after 9 MiB.
struct BrokenProvider;

#[async_trait::async_trait]
impl file_distribution::GetFile for BrokenProvider {
    async fn get_file(
        &self,
        _id: ShortGuid,
    ) -> Result<file_distribution::BoxedFileReader, file_distribution::FileAccessorError> {
        Ok(file_distribution::BoxedFileReader::new(BrokenFile {
            remaining: 9 * 1024 * 1024,
        }))
    }
}

/// A multipart upload that fails partway is aborted and leaves no object.
#[tokio::test]
async fn failed_multipart_upload_is_aborted() {
    let (backend, store) = in_memory(&config(Some(60)));
    let id = ShortGuid::new_random();
    let summary = Arc::new(WriteSummary {
        expires: tokio::time::Instant::now() + Duration::from_secs(60),
        hashes: FileHashes::of(b""),
        file_name: None,
        content_type: None,
        file_size_bytes: 10 * 1024 * 1024,
    });

    let result = backend
        .distribute_file(id, summary, FileProvider::wrap(Arc::new(BrokenProvider)))
        .await;

    assert!(matches!(result, Err(DistributionError::Io(_))));
    let objects: Vec<_> = store.list(None).collect::<Vec<_>>().await;
    assert!(objects.is_empty(), "Left behind: {objects:?}");
}
