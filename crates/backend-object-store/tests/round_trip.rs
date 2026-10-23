//! Integration tests of the object store backend against an in-memory store.

use axum_extra::headers::ContentType;
use backbone::{Backbone, CompletionMode, FileAccessorBridge};
use backend_object_store::{METADATA_ATTRIBUTE, ObjectStoreBackend, ObjectStoreSettings};
use backend_traits::{DistributeFile, DistributionError, FetchError, FetchFile, FetchedFile};
use file_distribution::{FileHashes, FileProvider, WriteSummary};
use futures::StreamExt;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{Attribute, Attributes, ObjectStore, ObjectStoreExt, PutOptions, PutPayload};
use rendezvous::Rendezvous;
use shortguid::ShortGuid;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;

fn settings(expiration_sec: Option<u64>) -> ObjectStoreSettings {
    ObjectStoreSettings {
        tag: "test".into(),
        prefix: Some("files".into()),
        expiration: expiration_sec.map(Duration::from_secs),
        ..ObjectStoreSettings::default()
    }
}

/// Creates a backend on an empty in-memory store and returns both.
fn in_memory(settings: ObjectStoreSettings) -> (ObjectStoreBackend, Arc<InMemory>) {
    let store = Arc::new(InMemory::new());
    (ObjectStoreBackend::new(settings, store.clone()), store)
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
async fn assert_round_trip(backend: &ObjectStoreBackend) {
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
    let (backend, _) = in_memory(settings(Some(60)));
    assert_round_trip(&backend).await;
}

#[tokio::test]
async fn object_carries_content_type_and_metadata() {
    let (backend, store) = in_memory(settings(Some(60)));
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
    let (backend, _) = in_memory(settings(Some(60)));
    let file = backend
        .fetch_file(ShortGuid::new_random())
        .await
        .expect("Failed to fetch");
    assert!(file.is_none());
}

#[tokio::test]
async fn zero_expiration_keeps_the_file_indefinitely() {
    let (backend, _) = in_memory(settings(Some(0)));
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
    let (backend, store) = in_memory(settings(Some(60)));
    let id = ShortGuid::new_random();
    put_raw(&store, id, b"abc", Attributes::new()).await;

    let result = backend.fetch_file(id).await;
    assert!(matches!(result, Err(FetchError::InvalidMetadata(_))));
}

#[tokio::test]
async fn malformed_metadata_is_rejected() {
    let (backend, store) = in_memory(settings(Some(60)));
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
    let (backend, store) = in_memory(ObjectStoreSettings {
        max_file_size: Some(2),
        ..settings(Some(60))
    });
    let id = ShortGuid::new_random();
    put_raw(&store, id, b"abc", Attributes::new()).await;

    let result = backend.fetch_file(id).await;
    assert!(matches!(result, Err(FetchError::BackendSpecific(_))));
}

/// Files above the multipart threshold are uploaded in parts, keeping their
/// metadata and content type.
#[tokio::test]
async fn large_file_is_uploaded_in_parts() {
    let (backend, store) = in_memory(settings(Some(60)));
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
    let (backend, store) = in_memory(settings(Some(60)));
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
