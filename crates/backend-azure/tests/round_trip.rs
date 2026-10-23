//! Integration tests for the Azure Blob Storage backend.
//!
//! Storing and fetching is tested against an in-memory store in
//! `backend-object-store`. Set `YY_TEST_AZURITE` to the URL of an Azurite
//! emulator such as `http://127.0.0.1:10000` to run the emulator tests; they
//! are skipped otherwise.

use app_config::azure::AzureBackendConfig;
use axum_extra::headers::ContentType;
use backbone::{Backbone, CompletionMode, FileAccessorBridge};
use backend_azure::AzureBackend;
use backend_object_store::ObjectStoreBackend;
use backend_traits::{DistributeFile, DistributionError, FetchError, FetchFile, FetchedFile};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use file_distribution::{FileHashes, FileProvider, WriteSummary};
use hmac::{Hmac, KeyInit, Mac};
use rendezvous::Rendezvous;
use sha2::Sha256;
use shortguid::ShortGuid;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::mpsc;

const EMULATOR_URL_VAR: &str = "YY_TEST_AZURITE";
const EMULATOR_CONTAINER: &str = "yeet-yoink-test";

/// The account and well-known development key of the Azurite emulator.
const EMULATOR_ACCOUNT: &str = "devstoreaccount1";
const EMULATOR_ACCOUNT_KEY: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

fn config(expiration_sec: Option<u32>) -> AzureBackendConfig {
    AzureBackendConfig {
        tag: "test".into(),
        container: EMULATOR_CONTAINER.into(),
        prefix: Some("files".into()),
        expiration_sec,
        ..AzureBackendConfig::default()
    }
}

/// Creates a backend for the emulator, or returns `None` if none is configured.
async fn emulator(expiration_sec: Option<u32>) -> Option<ObjectStoreBackend> {
    let Ok(url) = std::env::var(EMULATOR_URL_VAR) else {
        eprintln!("{EMULATOR_URL_VAR} is not set; skipping Azurite test");
        return None;
    };
    create_container(url.trim_end_matches('/')).await;

    let config = AzureBackendConfig {
        emulator_url: Some(url.parse().expect("Invalid emulator URL")),
        ..config(expiration_sec)
    };
    Some(AzureBackend::try_new(&config).expect("Failed to create Azure backend"))
}

/// Creates the test container. object_store cannot create containers, so the
/// request is signed with the emulator's key (Shared Key authorization).
async fn create_container(url: &str) {
    const VERSION: &str = "2021-08-06";
    let date = httpdate::fmt_http_date(SystemTime::now());
    // Azurite addresses the account by path, so it appears twice.
    let string_to_sign = format!(
        "PUT\n\n\n\n\n\n\n\n\n\n\n\nx-ms-date:{date}\nx-ms-version:{VERSION}\n\
         /{EMULATOR_ACCOUNT}/{EMULATOR_ACCOUNT}/{EMULATOR_CONTAINER}\nrestype:container"
    );
    let key = BASE64.decode(EMULATOR_ACCOUNT_KEY).expect("Invalid key");
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).expect("Invalid key length");
    mac.update(string_to_sign.as_bytes());
    let signature = BASE64.encode(mac.finalize().into_bytes());

    let response = reqwest::Client::new()
        .put(format!(
            "{url}/{EMULATOR_ACCOUNT}/{EMULATOR_CONTAINER}?restype=container"
        ))
        .header("x-ms-date", date)
        .header("x-ms-version", VERSION)
        .header(
            "Authorization",
            format!("SharedKey {EMULATOR_ACCOUNT}:{signature}"),
        )
        .header("Content-Length", "0")
        .send()
        .await
        .expect("Failed to reach Azurite");
    // Creating an existing container fails with 409 Conflict, which is fine.
    assert!(
        response.status().is_success() || response.status() == reqwest::StatusCode::CONFLICT,
        "Failed to create container: {} {}",
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

/// Files above the multipart threshold go up as blocks of a block blob.
#[tokio::test]
async fn emulator_large_file_is_uploaded_in_blocks() {
    let Some(backend) = emulator(Some(60)).await else {
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

/// Requests must fail fast while Blob Storage is unreachable, so that the
/// server moves on to the next backend instead of waiting out retries.
#[tokio::test]
async fn unreachable_storage_fails_fast() {
    let config = AzureBackendConfig {
        emulator_url: Some("http://127.0.0.1:1".parse().expect("Invalid URL")),
        ..config(Some(60))
    };
    let backend = AzureBackend::try_new(&config).expect("Failed to create Azure backend");
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
