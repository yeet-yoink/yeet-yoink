//! Integration tests against a real Valkey instance.
//!
//! Set `YY_TEST_VALKEY` to a connection string such as
//! `valkey://127.0.0.1:6379` to run them; they are skipped otherwise.

use app_config::valkey::ValkeyBackendConfig;
use axum_extra::headers::ContentType;
use backbone::{Backbone, CompletionMode, FileAccessorBridge};
use backend_traits::{DistributeFile, FetchError, FetchFile, FetchedFile};
use backend_valkey::ValkeyBackend;
use file_distribution::protobuf::ItemMetadata;
use file_distribution::{FileHashes, FileProvider, WriteSummary};
use rendezvous::Rendezvous;
use shortguid::ShortGuid;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;

const CONNECTION_STRING_VAR: &str = "YY_TEST_VALKEY";

/// Configures the test instance, or returns `None` if none is configured.
fn config(expiration_sec: Option<u32>) -> Option<ValkeyBackendConfig> {
    let Ok(connection_string) = std::env::var(CONNECTION_STRING_VAR) else {
        eprintln!("{CONNECTION_STRING_VAR} is not set; skipping valkey test");
        return None;
    };
    Some(ValkeyBackendConfig {
        tag: "test".into(),
        connection_string: connection_string
            .parse()
            .expect("Invalid valkey connection string"),
        expiration_sec,
        max_file_size_bytes: None,
        priority: 0,
    })
}

/// Creates a backend for the test instance, or returns `None` if none is configured.
fn backend(expiration_sec: Option<u32>) -> Option<ValkeyBackend> {
    let config = config(expiration_sec)?;
    Some(ValkeyBackend::try_new(&config).expect("Failed to create valkey backend"))
}

/// Opens a plain connection to the test instance, bypassing the backend.
async fn raw_connection() -> redis::aio::MultiplexedConnection {
    let url = std::env::var(CONNECTION_STRING_VAR).expect("Checked by the caller");
    redis::Client::open(url)
        .expect("Invalid valkey connection string")
        .get_multiplexed_async_connection()
        .await
        .expect("Failed to connect to valkey")
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

#[tokio::test]
async fn distributed_file_can_be_fetched() {
    let Some(backend) = backend(Some(60)) else {
        return;
    };
    let id = ShortGuid::new_random();
    let (provider, summary) = buffered_file(id, b"hello, valkey").await;

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

    assert_eq!(data, &b"hello, valkey"[..]);
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

#[tokio::test]
async fn entries_expire_in_valkey() {
    let Some(backend) = backend(Some(60)) else {
        return;
    };
    let id = ShortGuid::new_random();
    let (provider, summary) = buffered_file(id, b"short-lived").await;
    backend
        .distribute_file(id, summary, provider)
        .await
        .expect("Failed to distribute");

    let mut connection = raw_connection().await;
    for key in [format!("data-{id}"), format!("meta-{id}")] {
        let ttl: i64 = redis::cmd("TTL")
            .arg(&key)
            .query_async(&mut connection)
            .await
            .expect("Failed to query TTL");
        assert!((50..=60).contains(&ttl), "{key} has TTL {ttl}");
    }
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
    let (file, _) = read_content(file).await;
    assert_eq!(file.expires_at, None);

    // -1: the key exists without an expiry.
    let ttl: i64 = redis::cmd("TTL")
        .arg(format!("data-{id}"))
        .query_async(&mut raw_connection().await)
        .await
        .expect("Failed to query TTL");
    assert_eq!(ttl, -1);
}

#[tokio::test]
async fn poisoned_expiry_is_rejected() {
    let Some(backend) = backend(Some(60)) else {
        return;
    };
    let id = ShortGuid::new_random();
    let (_, summary) = buffered_file(id, b"abc").await;

    // Write entries directly, bypassing the backend, with an absurd expiry.
    let mut metadata = ItemMetadata::new(id, &summary, None);
    metadata.expires_at_unix_ms = Some(u64::MAX);
    let metadata = metadata.serialize_to_proto().expect("Failed to serialize");
    let mut connection = raw_connection().await;
    redis::pipe()
        .set_ex(format!("meta-{id}"), metadata.as_ref(), 60)
        .set_ex(format!("data-{id}"), &b"abc"[..], 60)
        .query_async::<()>(&mut connection)
        .await
        .expect("Failed to store entries");

    let result = backend.fetch_file(id).await;
    assert!(matches!(result, Err(FetchError::InvalidMetadata(_))));
}

#[tokio::test]
async fn configured_priority_and_size_limit_are_reported() {
    use app_config::valkey::DEFAULT_MAX_FILE_SIZE_BYTES;

    let Some(mut config) = config(Some(60)) else {
        return;
    };
    let backend = ValkeyBackend::try_new(&config).expect("Failed to create valkey backend");
    assert_eq!(backend.priority(), 0);
    assert_eq!(backend.max_file_size(), Some(DEFAULT_MAX_FILE_SIZE_BYTES));

    config.priority = 7;
    config.max_file_size_bytes = Some(0);
    let backend = ValkeyBackend::try_new(&config).expect("Failed to create valkey backend");
    assert_eq!(backend.priority(), 7);
    // A limit of 0 disables the size check.
    assert_eq!(backend.max_file_size(), None);
}

/// Creates a backend for a port nothing listens on.
fn unreachable_backend() -> ValkeyBackend {
    let config = ValkeyBackendConfig {
        tag: "unreachable".into(),
        connection_string: "valkey://127.0.0.1:1"
            .parse()
            .expect("Invalid valkey connection string"),
        ..ValkeyBackendConfig::default()
    };
    ValkeyBackend::try_new(&config).expect("Failed to create valkey backend")
}

/// Fetches must fail fast while Valkey is unreachable, so that the server
/// moves on to the next backend instead of waiting out a reconnect backoff.
#[tokio::test]
async fn fetch_from_unreachable_valkey_fails_fast() {
    let backend = unreachable_backend();

    let start = std::time::Instant::now();
    let result = backend.fetch_file(ShortGuid::new_random()).await;

    assert!(matches!(result, Err(FetchError::BackendSpecific(_))));
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "fetch took {:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn distribution_to_unreachable_valkey_fails() {
    use backend_traits::DistributionError;

    let backend = unreachable_backend();
    let id = ShortGuid::new_random();
    let (provider, summary) = buffered_file(id, b"nowhere to go").await;

    let result = backend.distribute_file(id, summary, provider).await;
    assert!(matches!(result, Err(DistributionError::BackendSpecific(_))));
}

/// Files larger than one read range are reassembled from several ranges.
#[tokio::test]
async fn file_spanning_several_ranges_is_read_completely() {
    let Some(backend) = backend(Some(60)) else {
        return;
    };
    let id = ShortGuid::new_random();
    let data: Vec<u8> = (0..2_621_440u32).map(|i| (i % 251) as u8).collect();
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
    assert_eq!(file.size, Some(data.len() as u64));
    let (file, fetched) = read_content(file).await;

    assert_eq!(fetched.len(), data.len());
    assert!(fetched == data, "Content differs");
    assert_eq!(FileHashes::of(&fetched), file.hashes);
}

/// A value that shrinks between reading the size and the content ends the
/// stream early, so the copy is rejected by its length.
#[tokio::test]
async fn value_changing_while_read_ends_short() {
    let Some(backend) = backend(Some(60)) else {
        return;
    };
    let id = ShortGuid::new_random();
    let data = vec![b'x'; 1_500_000];
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
    redis::cmd("SET")
        .arg(format!("data-{id}"))
        .arg(&b"short"[..])
        .query_async::<()>(&mut raw_connection().await)
        .await
        .expect("Failed to overwrite value");
    let (file, fetched) = read_content(file).await;

    assert!((fetched.len() as u64) < file.size.expect("Missing size"));
}
