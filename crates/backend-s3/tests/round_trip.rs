//! Integration tests for the S3 backend.
//!
//! Storing and fetching is tested against an in-memory store in
//! `backend-object-store`. Set `YY_TEST_S3` to the endpoint of an
//! S3-compatible store such as `http://127.0.0.1:8333` to run the store
//! tests; they are skipped otherwise. The store must accept the credentials
//! in `docker/seaweedfs-s3.json`.

use app_config::s3::S3BackendConfig;
use axum_extra::headers::ContentType;
use backbone::{Backbone, CompletionMode, FileAccessorBridge};
use backend_object_store::ObjectStoreBackend;
use backend_s3::S3Backend;
use backend_traits::{DistributeFile, DistributionError, FetchError, FetchFile, FetchedFile};
use file_distribution::{FileHashes, FileProvider, WriteSummary};
use object_store::aws::AmazonS3Builder;
use object_store::path::Path;
use object_store::signer::Signer;
use rendezvous::Rendezvous;
use shortguid::ShortGuid;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::mpsc;

const ENDPOINT_VAR: &str = "YY_TEST_S3";
const BUCKET: &str = "yeet-yoink-test";
const REGION: &str = "us-east-1";

/// The test credentials of `docker/seaweedfs-s3.json`.
const ACCESS_KEY_ID: &str = "yeetyoink";
const SECRET_ACCESS_KEY: &str = "yeetyoink-secret";

fn config(expiration_sec: Option<u32>) -> S3BackendConfig {
    S3BackendConfig {
        tag: "test".into(),
        bucket: BUCKET.into(),
        prefix: Some("files".into()),
        region: Some(REGION.into()),
        expiration_sec,
        ..S3BackendConfig::default()
    }
}

/// Writes the test credentials to files, as a mounted Secret would provide them.
fn key_files() -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("yy-s3-keys-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("Failed to create key directory");
    let (id, secret) = (dir.join("access-key-id"), dir.join("secret-access-key"));
    std::fs::write(&id, format!("{ACCESS_KEY_ID}\n")).expect("Failed to write key");
    std::fs::write(&secret, format!("{SECRET_ACCESS_KEY}\n")).expect("Failed to write key");
    (id, secret)
}

/// Creates a backend for the test store, or returns `None` if none is configured.
async fn store(expiration_sec: Option<u32>) -> Option<ObjectStoreBackend> {
    let Ok(endpoint) = std::env::var(ENDPOINT_VAR) else {
        eprintln!("{ENDPOINT_VAR} is not set; skipping S3 test");
        return None;
    };
    create_bucket(endpoint.trim_end_matches('/')).await;

    let (id, secret) = key_files();
    let config = S3BackendConfig {
        endpoint: Some(endpoint.parse().expect("Invalid endpoint")),
        access_key_id_file: Some(id),
        secret_access_key_file: Some(secret),
        ..config(expiration_sec)
    };
    Some(S3Backend::try_new(&config).expect("Failed to create S3 backend"))
}

/// Creates the test bucket. object_store cannot create buckets, so a
/// presigned PUT on the bucket itself does.
async fn create_bucket(endpoint: &str) {
    let signer = AmazonS3Builder::new()
        .with_endpoint(endpoint)
        .with_bucket_name(BUCKET)
        .with_region(REGION)
        .with_access_key_id(ACCESS_KEY_ID)
        .with_secret_access_key(SECRET_ACCESS_KEY)
        .with_allow_http(true)
        .build()
        .expect("Failed to create signer");
    let url = signer
        .signed_url(
            reqwest::Method::PUT,
            &Path::from(""),
            Duration::from_secs(60),
        )
        .await
        .expect("Failed to sign");

    let response = reqwest::Client::new()
        .put(url)
        .send()
        .await
        .expect("Failed to reach the S3 store");
    // Creating an existing bucket fails with 409 Conflict, which is fine.
    assert!(
        response.status().is_success() || response.status() == reqwest::StatusCode::CONFLICT,
        "Failed to create bucket: {} {}",
        response.status(),
        response.text().await.unwrap_or_default()
    );
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

#[tokio::test]
async fn store_round_trip() {
    let Some(backend) = store(Some(60)).await else {
        return;
    };
    assert_round_trip(&backend).await;
}

#[tokio::test]
async fn store_unknown_file_is_not_found() {
    let Some(backend) = store(Some(60)).await else {
        return;
    };
    let file = backend
        .fetch_file(ShortGuid::new_random())
        .await
        .expect("Failed to fetch");
    assert!(file.is_none());
}

/// Requests signed with other credentials are refused.
#[tokio::test]
async fn store_rejects_other_credentials() {
    let Ok(endpoint) = std::env::var(ENDPOINT_VAR) else {
        return;
    };
    create_bucket(endpoint.trim_end_matches('/')).await;
    let dir = std::env::temp_dir().join(format!("yy-s3-bad-keys-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (id, secret) = (dir.join("id"), dir.join("secret"));
    std::fs::write(&id, ACCESS_KEY_ID).unwrap();
    std::fs::write(&secret, "wrong-secret").unwrap();
    let backend = S3Backend::try_new(&S3BackendConfig {
        endpoint: Some(endpoint.parse().expect("Invalid endpoint")),
        access_key_id_file: Some(id),
        secret_access_key_file: Some(secret),
        ..config(Some(60))
    })
    .expect("Failed to create S3 backend");

    let result = backend.fetch_file(ShortGuid::new_random()).await;
    assert!(matches!(result, Err(FetchError::BackendSpecific(_))));
}

/// Files above the multipart threshold go up as a multipart upload.
#[tokio::test]
async fn store_large_file_is_uploaded_in_parts() {
    let Some(backend) = store(Some(60)).await else {
        return;
    };
    let id = ShortGuid::new_random();
    let data: Vec<u8> = (0..20 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    let (provider, summary) = buffered_file(id, &data).await;

    backend
        .distribute_file(id, summary.clone(), provider)
        .await
        .expect("Failed to distribute");
    let file = backend
        .fetch_file(id)
        .await
        .expect("Failed to fetch")
        .expect("File not found");
    assert_eq!(file.size, Some(data.len() as u64));
    let (file, fetched) = read_content(file).await;

    assert!(fetched == data, "Content differs");
    assert_eq!(file.hashes, summary.hashes);
    assert_eq!(file.file_name.as_deref(), Some("hello.txt"));
}

/// Requests must fail fast while S3 is unreachable, so that the
/// server moves on to the next backend instead of waiting out retries.
#[tokio::test]
async fn unreachable_storage_fails_fast() {
    let config = S3BackendConfig {
        endpoint: Some("http://127.0.0.1:1".parse().expect("Invalid URL")),
        ..config(Some(60))
    };
    let backend = S3Backend::try_new(&config).expect("Failed to create S3 backend");
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
