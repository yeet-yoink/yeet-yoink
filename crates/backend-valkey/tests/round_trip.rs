//! Integration tests against a real Valkey instance.
//!
//! Set `YY_TEST_VALKEY` to a connection string such as
//! `valkey://127.0.0.1:6379` to run them; they are skipped otherwise.

use app_config::valkey::ValkeyBackendConfig;
use axum_extra::headers::ContentType;
use backbone::{Backbone, CompletionMode, FileAccessorBridge};
use backend_traits::{DistributeFile, FetchError, FetchFile};
use backend_valkey::ValkeyBackend;
use file_distribution::protobuf::ItemMetadata;
use file_distribution::{FileProvider, WriteSummary};
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

    assert_eq!(file.data, &b"hello, valkey"[..]);
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
