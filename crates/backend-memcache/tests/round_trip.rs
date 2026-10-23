//! Integration tests against a real memcached instance.
//!
//! Set `YY_TEST_MEMCACHE` to a connection string such as
//! `memcache://127.0.0.1:11211` to run them; they are skipped otherwise.

use app_config::memcache::MemcacheBackendConfig;
use axum_extra::headers::ContentType;
use backbone::{Backbone, CompletionMode, FileAccessorBridge};
use backend_memcache::MemcacheBackend;
use backend_traits::{DistributeFile, FetchFile};
use file_distribution::{FileProvider, WriteSummary};
use rendezvous::Rendezvous;
use shortguid::ShortGuid;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;

const CONNECTION_STRING_VAR: &str = "YY_TEST_MEMCACHE";

/// Connects to the test instance, or returns `None` if none is configured.
fn backend(expiration_sec: Option<u32>) -> Option<MemcacheBackend> {
    let Ok(connection_string) = std::env::var(CONNECTION_STRING_VAR) else {
        eprintln!("{CONNECTION_STRING_VAR} is not set; skipping memcached test");
        return None;
    };
    let config = MemcacheBackendConfig {
        tag: "test".into(),
        connection_string: connection_string
            .parse()
            .expect("Invalid memcache connection string"),
        expiration_sec,
        max_file_size_bytes: None,
        priority: 0,
    };
    Some(MemcacheBackend::try_new(&config).expect("Failed to connect to memcached"))
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
    writer.write(data).await.expect("Failed to write");
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

#[tokio::test]
async fn distributed_file_can_be_fetched() {
    let Some(backend) = backend(Some(60)) else {
        return;
    };
    let id = ShortGuid::new_random();
    let (provider, summary) = buffered_file(id, b"hello, memcached").await;

    backend
        .distribute_file(id, summary.clone(), provider)
        .await
        .expect("Failed to distribute");
    let file = backend
        .fetch_file(id)
        .await
        .expect("Failed to fetch")
        .expect("File not found");

    assert_eq!(file.data, &b"hello, memcached"[..]);
    assert_eq!(file.file_name.as_deref(), Some("hello.txt"));
    assert_eq!(file.content_type.as_deref(), Some("text/plain"));
    assert_eq!(file.hashes.md5, summary.hashes.md5);
    assert_eq!(file.hashes.sha256, summary.hashes.sha256);
    assert!(file.is_intact());

    let expires_at = file.expires_at.expect("Missing expiry");
    let remaining = expires_at
        .duration_since(SystemTime::now())
        .expect("Already expired");
    assert!(remaining <= Duration::from_secs(60));
    assert!(remaining > Duration::from_secs(50));
}

#[tokio::test]
async fn unknown_file_is_not_found() {
    let Some(backend) = backend(Some(60)) else {
        return;
    };

    let file = backend
        .fetch_file(ShortGuid::new_random())
        .await
        .expect("Failed to fetch");
    assert!(file.is_none());
}

#[tokio::test]
async fn zero_expiration_keeps_the_file_indefinitely() {
    let Some(backend) = backend(Some(0)) else {
        return;
    };
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

    assert_eq!(file.expires_at, None);
}

#[tokio::test]
async fn poisoned_expiry_is_rejected() {
    use backend_traits::FetchError;
    use file_distribution::protobuf::ItemMetadata;
    use r2d2_memcache::memcache;

    let Some(backend) = backend(Some(60)) else {
        return;
    };
    let id = ShortGuid::new_random();
    let (_, summary) = buffered_file(id, b"abc").await;

    // Write entries directly, bypassing the backend, with an absurd expiry.
    let mut metadata = ItemMetadata::new(id, &summary, None);
    metadata.expires_at_unix_ms = Some(u64::MAX);
    let metadata = metadata.serialize_to_proto().expect("Failed to serialize");
    let url = std::env::var(CONNECTION_STRING_VAR).expect("Checked above");
    let client = memcache::Client::connect(url).expect("Failed to connect");
    client
        .set(&format!("meta-{id}"), metadata.as_ref(), 60)
        .expect("Failed to store metadata");
    client
        .set(&format!("data-{id}"), &b"abc"[..], 60)
        .expect("Failed to store data");

    let result = backend.fetch_file(id).await;
    assert!(matches!(result, Err(FetchError::InvalidMetadata(_))));
}

/// Assumes the test instance runs with Memcached's default item size limit.
#[tokio::test]
async fn file_at_the_default_size_limit_fits() {
    use app_config::memcache::DEFAULT_MAX_FILE_SIZE_BYTES;

    let Some(backend) = backend(Some(60)) else {
        return;
    };
    assert_eq!(backend.max_file_size(), Some(DEFAULT_MAX_FILE_SIZE_BYTES));

    let id = ShortGuid::new_random();
    let data = vec![b'x'; DEFAULT_MAX_FILE_SIZE_BYTES as usize];
    let (provider, summary) = buffered_file(id, &data).await;

    backend
        .distribute_file(id, summary, provider)
        .await
        .expect("Failed to distribute");
    let file = backend
        .fetch_file(id)
        .await
        .expect("Failed to fetch")
        .expect("File not found");

    assert_eq!(file.data.len(), data.len());
    assert!(file.is_intact());
}
