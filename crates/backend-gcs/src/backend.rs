use app_config::{
    AppConfig,
    gcs::{DEFAULT_EXPIRATION, GcsBackendConfig},
};
use async_trait::async_trait;
use backend_traits::{
    Backend, BackendInfo, DistributeFile, DistributionError, FetchError, FetchFile, FetchedFile,
    TryCreateFromConfig,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use file_distribution::protobuf::ItemMetadata;
use file_distribution::{BoxedFileReader, FileProvider, GetFile, WriteSummary};
use futures::TryStreamExt;
use map_ok::{BoxOk, MapOk};
use object_store::gcp::GoogleCloudStorageBuilder;
use object_store::path::Path;
use object_store::{
    Attribute, Attributes, ClientOptions, ObjectStore, ObjectStoreExt, PutMultipartOptions,
    PutOptions, RetryConfig, WriteMultipart,
};
use shortguid::ShortGuid;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::io::AsyncReadExt;
use tokio_util::io::StreamReader;
use tracing::{trace, warn};

/// The custom object metadata key holding the serialized file metadata.
pub const METADATA_ATTRIBUTE: &str = "yy-metadata";

/// Files up to this size are uploaded in a single request; larger ones are
/// streamed as multipart uploads.
const MULTIPART_THRESHOLD: u64 = 8 * 1024 * 1024;

/// The size of the parts of a multipart upload. Cloud Storage requires all
/// parts but the last to be at least 5 MiB.
const PART_SIZE: usize = 8 * 1024 * 1024;

/// How many parts of a multipart upload may be in flight at once.
const MAX_CONCURRENT_PARTS: usize = 2;

/// The size of the chunks in which a file is read for upload.
const READ_CHUNK_SIZE: usize = 64 * 1024;

/// How long a single connection attempt may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a single request may take, including transferring the file.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// How often a failed request is retried.
///
/// Requests fail fast while Cloud Storage is unreachable so that fetches fall
/// through to the next backend quickly.
const MAX_RETRIES: usize = 2;

/// How long retries of a failed request may go on in total.
const RETRY_TIMEOUT: Duration = Duration::from_secs(10);

pub struct GcsBackend {
    /// The tag identifying the backend.
    tag: String,
    /// The object store holding the files.
    store: Arc<dyn ObjectStore>,
    /// The path prefix for all objects, or `None` for the bucket root.
    prefix: Option<Path>,
    /// The expiration time for stored files, or `0` to keep them indefinitely.
    expiration_secs: u64,
    /// The largest file size accepted for storage, or `None` for no limit.
    max_file_size: Option<u64>,
    /// The priority of this backend relative to others.
    priority: i32,
}

impl GcsBackend {
    /// Creates a backend for the configured bucket. No request is made until
    /// the backend is used.
    pub fn try_new(config: &GcsBackendConfig) -> Result<Self, GcsBackendConstructionError> {
        let client_options = ClientOptions::new()
            .with_connect_timeout(CONNECT_TIMEOUT)
            .with_timeout(REQUEST_TIMEOUT)
            // Emulators are usually served over plain HTTP.
            .with_allow_http(config.emulator_url.is_some());

        let mut builder = GoogleCloudStorageBuilder::new()
            .with_bucket_name(&config.bucket)
            .with_client_options(client_options)
            .with_retry(RetryConfig {
                max_retries: MAX_RETRIES,
                retry_timeout: RETRY_TIMEOUT,
                ..RetryConfig::default()
            });

        if let Some(emulator_url) = &config.emulator_url {
            // A service account key is how object_store is pointed at a
            // different endpoint without requesting OAuth tokens.
            let key = serde_json::json!({
                "gcs_base_url": emulator_url.as_str().trim_end_matches('/'),
                "disable_oauth": true,
                "client_email": "",
                "private_key": "",
                "private_key_id": "",
            });
            builder = builder.with_service_account_key(key.to_string());
        } else if let Some(path) = &config.service_account_path {
            builder = builder.with_service_account_path(path.to_string_lossy());
        }

        let store = builder
            .build()
            .map_err(GcsBackendConstructionError::FailedToCreateClient)?;
        Ok(Self::with_store(config, Arc::new(store)))
    }

    /// Creates a backend on top of an existing object store, ignoring the
    /// connection settings in `config`.
    pub fn with_store(config: &GcsBackendConfig, store: Arc<dyn ObjectStore>) -> Self {
        let expiration_secs = config
            .expiration_sec
            .map_or(DEFAULT_EXPIRATION, |secs| Duration::from_secs(secs.into()))
            .as_secs();

        // There is no limit by default; a limit of 0 means none as well.
        let max_file_size = config.max_file_size_bytes.filter(|&max| max != 0);

        let prefix = config
            .prefix
            .as_deref()
            .map(Path::from)
            .filter(|prefix| !prefix.as_ref().is_empty());

        Self {
            tag: config.tag.clone(),
            store,
            prefix,
            expiration_secs,
            max_file_size,
            priority: config.priority,
        }
    }

    /// Gets the path of the object holding the given file.
    fn object_path(&self, id: ShortGuid) -> Path {
        match &self.prefix {
            Some(prefix) => prefix.clone().join(id.to_string()),
            None => Path::from(id.to_string()),
        }
    }
}

#[async_trait]
impl DistributeFile for GcsBackend {
    fn tag(&self) -> &str {
        &self.tag
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    fn max_file_size(&self) -> Option<u64> {
        self.max_file_size
    }

    async fn distribute_file(
        &self,
        id: ShortGuid,
        summary: Arc<WriteSummary>,
        file_provider: FileProvider,
    ) -> Result<(), DistributionError> {
        let file = file_provider.get_file(id).await?;

        let expiration = self.expiration_secs;
        let expires_at =
            (expiration != 0).then(|| SystemTime::now() + Duration::from_secs(expiration));
        let metadata = ItemMetadata::new(id, &summary, expires_at)
            .serialize_to_proto()
            .map_err(|e| DistributionError::BackendSpecific(Box::new(e)))?;

        // The metadata travels with the object, so data and metadata are
        // written and read together.
        let mut attributes = Attributes::new();
        attributes.insert(
            Attribute::Metadata(METADATA_ATTRIBUTE.into()),
            BASE64.encode(&metadata).into(),
        );
        if let Some(content_type) = &summary.content_type {
            attributes.insert(Attribute::ContentType, content_type.clone().into());
        }

        let path = self.object_path(id);
        if summary.file_size_bytes as u64 <= MULTIPART_THRESHOLD {
            self.upload_whole(&path, file, attributes).await?;
        } else {
            self.upload_in_parts(&path, file, attributes).await?;
        }
        trace!(file_id = %id, "Stored file {id} as object {path}");
        Ok(())
    }
}

impl GcsBackend {
    /// Uploads a small file in a single request.
    async fn upload_whole(
        &self,
        path: &Path,
        mut file: BoxedFileReader,
        attributes: Attributes,
    ) -> Result<(), DistributionError> {
        let mut data = Vec::new();
        file.read_to_end(&mut data).await?;
        self.store
            .put_opts(
                path,
                data.into(),
                PutOptions {
                    attributes,
                    ..PutOptions::default()
                },
            )
            .await
            .map_err(|e| DistributionError::BackendSpecific(Box::new(e)))?;
        Ok(())
    }

    /// Streams a large file as a multipart upload, holding at most a few parts
    /// in memory. A failed upload is aborted.
    async fn upload_in_parts(
        &self,
        path: &Path,
        mut file: BoxedFileReader,
        attributes: Attributes,
    ) -> Result<(), DistributionError> {
        let upload = self
            .store
            .put_multipart_opts(
                path,
                PutMultipartOptions {
                    attributes,
                    ..PutMultipartOptions::default()
                },
            )
            .await
            .map_err(|e| DistributionError::BackendSpecific(Box::new(e)))?;
        let mut writer = WriteMultipart::new_with_chunk_size(upload, PART_SIZE);

        let mut chunk = vec![0; READ_CHUNK_SIZE];
        let result: Result<(), UploadError> = async {
            loop {
                writer.wait_for_capacity(MAX_CONCURRENT_PARTS).await?;
                let read = file.read(&mut chunk).await?;
                if read == 0 {
                    return Ok(());
                }
                writer.write(&chunk[..read]);
            }
        }
        .await;

        match result {
            Ok(()) => {
                writer
                    .finish()
                    .await
                    .map_err(|e| DistributionError::BackendSpecific(Box::new(e)))?;
                Ok(())
            }
            Err(e) => {
                if let Err(abort_error) = writer.abort().await {
                    warn!("Failed to abort the multipart upload of {path}: {abort_error}");
                }
                Err(match e {
                    UploadError::Read(e) => DistributionError::Io(e),
                    UploadError::Store(e) => DistributionError::BackendSpecific(Box::new(e)),
                })
            }
        }
    }
}

#[async_trait]
impl FetchFile for GcsBackend {
    async fn fetch_file(&self, id: ShortGuid) -> Result<Option<FetchedFile>, FetchError> {
        let path = self.object_path(id);
        let result = match self.store.get(&path).await {
            Ok(result) => result,
            Err(object_store::Error::NotFound { .. }) => {
                trace!(file_id = %id, "File {id} not found as object {path}");
                return Ok(None);
            }
            Err(e) => return Err(FetchError::BackendSpecific(Box::new(e))),
        };

        // Refuse objects this backend would not have stored.
        if let Some(max) = self.max_file_size
            && result.meta.size > max
        {
            return Err(FetchError::BackendSpecific(Box::new(
                GcsFetchError::TooLarge {
                    size: result.meta.size,
                    max,
                },
            )));
        }

        let metadata = result
            .attributes
            .get(&Attribute::Metadata(METADATA_ATTRIBUTE.into()))
            .ok_or_else(|| FetchError::InvalidMetadata("missing object metadata".into()))?;
        let metadata = BASE64
            .decode(metadata.as_ref())
            .map_err(|e| FetchError::InvalidMetadata(e.to_string()))?;

        let size = result.meta.size;
        let content = StreamReader::new(result.into_stream().map_err(std::io::Error::other));
        let file = FetchedFile::from_stored(&metadata, Some(size), Box::new(content))?;
        trace!(file_id = %id, "Fetching file {id} of {size} bytes from object {path}");
        Ok(Some(file))
    }
}

impl BackendInfo for GcsBackend {
    fn backend_name() -> &'static str {
        "Google Cloud Storage"
    }

    fn backend_version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
}

impl TryCreateFromConfig for GcsBackend {
    type Error = GcsBackendConstructionError;

    fn try_from_config(config: &AppConfig) -> Result<Vec<Backend>, Self::Error> {
        config
            .backends
            .gcs
            .iter()
            .map(GcsBackend::try_new)
            .box_ok()
            .map_ok(Backend::from)
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GcsBackendConstructionError {
    #[error("Failed to create Cloud Storage client: {0}")]
    FailedToCreateClient(object_store::Error),
}

/// An error while streaming a multipart upload.
#[derive(Debug, thiserror::Error)]
enum UploadError {
    #[error(transparent)]
    Read(#[from] std::io::Error),
    #[error(transparent)]
    Store(#[from] object_store::Error),
}

#[derive(Debug, thiserror::Error)]
enum GcsFetchError {
    #[error("Object of {size} bytes exceeds the limit of {max} bytes")]
    TooLarge { size: u64, max: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;

    fn config(prefix: Option<&str>) -> GcsBackendConfig {
        GcsBackendConfig {
            tag: "gcs".into(),
            bucket: "bucket".into(),
            prefix: prefix.map(Into::into),
            ..GcsBackendConfig::default()
        }
    }

    fn in_memory(prefix: Option<&str>) -> GcsBackend {
        GcsBackend::with_store(&config(prefix), Arc::new(InMemory::new()))
    }

    #[test]
    fn reports_backend_info() {
        assert_eq!(GcsBackend::backend_name(), "Google Cloud Storage");
        assert_eq!(GcsBackend::backend_version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn objects_are_placed_under_the_prefix() {
        let id = ShortGuid::new_random();
        assert_eq!(
            in_memory(Some("yy/files")).object_path(id).as_ref(),
            format!("yy/files/{id}")
        );
        assert_eq!(in_memory(None).object_path(id).as_ref(), id.to_string());
        assert_eq!(in_memory(Some("")).object_path(id).as_ref(), id.to_string());
    }

    #[test]
    fn defaults_apply_without_configuration() {
        let backend = in_memory(None);
        assert_eq!(backend.expiration_secs, DEFAULT_EXPIRATION.as_secs());
        assert_eq!(backend.max_file_size(), None);
        assert_eq!(backend.priority(), 0);
        assert_eq!(backend.tag(), "gcs");
    }

    #[test]
    fn zero_size_limit_disables_the_check() {
        let config = GcsBackendConfig {
            max_file_size_bytes: Some(0),
            priority: 3,
            ..config(None)
        };
        let backend = GcsBackend::with_store(&config, Arc::new(InMemory::new()));
        assert_eq!(backend.max_file_size(), None);
        assert_eq!(backend.priority(), 3);
    }

    #[test]
    fn creates_one_backend_per_config_without_connecting() {
        let mut app_config = AppConfig::default();
        app_config.backends.gcs = vec![
            GcsBackendConfig {
                tag: "first".into(),
                ..config(None)
            },
            GcsBackendConfig {
                tag: "second".into(),
                emulator_url: Some("http://127.0.0.1:4443".parse().expect("Invalid URL")),
                ..config(None)
            },
        ];

        let backends = GcsBackend::try_from_config(&app_config).expect("Failed to create");

        let tags: Vec<_> = backends.iter().map(|backend| backend.tag()).collect();
        assert_eq!(tags, ["first", "second"]);
    }

    #[test]
    fn missing_service_account_file_is_reported() {
        let config = GcsBackendConfig {
            service_account_path: Some("/nonexistent/sa.json".into()),
            ..config(None)
        };
        assert!(matches!(
            GcsBackend::try_new(&config),
            Err(GcsBackendConstructionError::FailedToCreateClient(_))
        ));
    }
}
