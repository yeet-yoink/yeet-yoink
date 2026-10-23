use async_trait::async_trait;
use backend_traits::{DistributeFile, DistributionError, FetchError, FetchFile, FetchedFile};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use file_distribution::protobuf::ItemMetadata;
use file_distribution::{BoxedFileReader, FileProvider, GetFile, WriteSummary};
use futures::TryStreamExt;
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

/// The size of the parts of a multipart upload. Google Cloud Storage requires
/// all parts but the last to be at least 5 MiB.
const PART_SIZE: usize = 8 * 1024 * 1024;

/// How many parts of a multipart upload may be in flight at once.
const MAX_CONCURRENT_PARTS: usize = 2;

/// The size of the chunks in which a file is read for upload.
const READ_CHUNK_SIZE: usize = 64 * 1024;

/// How long a single connection attempt may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a single request may take, including transferring the file.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// How often a failed request is retried.
///
/// Requests fail fast while the store is unreachable so that fetches fall
/// through to the next backend quickly.
const MAX_RETRIES: usize = 2;

/// How long retries of a failed request may go on in total.
const RETRY_TIMEOUT: Duration = Duration::from_secs(10);

/// Client options that make requests fail fast while the store is unreachable.
/// `allow_http` is meant for emulators.
pub fn client_options(allow_http: bool) -> ClientOptions {
    ClientOptions::new()
        .with_connect_timeout(CONNECT_TIMEOUT)
        .with_timeout(REQUEST_TIMEOUT)
        .with_allow_http(allow_http)
}

/// A retry policy that gives up quickly while the store is unreachable.
pub fn retry_config() -> RetryConfig {
    RetryConfig {
        max_retries: MAX_RETRIES,
        retry_timeout: RETRY_TIMEOUT,
        ..RetryConfig::default()
    }
}

/// The settings every object store backend shares.
#[derive(Debug, Clone, Default)]
pub struct ObjectStoreSettings {
    /// The tag identifying the backend.
    pub tag: String,
    /// A path prefix for all objects, or `None` for the root.
    pub prefix: Option<String>,
    /// How long stored files are kept, or `None` to keep them indefinitely.
    pub expiration: Option<Duration>,
    /// The largest file size accepted for storage, or `None` for no limit.
    pub max_file_size: Option<u64>,
    /// The priority of this backend relative to others.
    pub priority: i32,
}

/// Stores each file as one object, with its metadata as a custom attribute.
pub struct ObjectStoreBackend {
    /// The tag identifying the backend.
    tag: String,
    /// The object store holding the files.
    store: Arc<dyn ObjectStore>,
    /// The path prefix for all objects, or `None` for the root.
    prefix: Option<Path>,
    /// How long stored files are kept, or `None` to keep them indefinitely.
    expiration: Option<Duration>,
    /// The largest file size accepted for storage, or `None` for no limit.
    max_file_size: Option<u64>,
    /// The priority of this backend relative to others.
    priority: i32,
}

impl ObjectStoreBackend {
    /// Creates a backend storing files in `store`. No request is made until
    /// the backend is used.
    pub fn new(settings: ObjectStoreSettings, store: Arc<dyn ObjectStore>) -> Self {
        let prefix = settings
            .prefix
            .as_deref()
            .map(Path::from)
            .filter(|prefix| !prefix.as_ref().is_empty());

        Self {
            tag: settings.tag,
            store,
            prefix,
            expiration: settings.expiration.filter(|e| !e.is_zero()),
            max_file_size: settings.max_file_size.filter(|&max| max != 0),
            priority: settings.priority,
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
impl DistributeFile for ObjectStoreBackend {
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

        let expires_at = self
            .expiration
            .map(|expiration| SystemTime::now() + expiration);
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

impl ObjectStoreBackend {
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
impl FetchFile for ObjectStoreBackend {
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
                FetchSizeError::TooLarge {
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

/// An error while streaming a multipart upload.
#[derive(Debug, thiserror::Error)]
enum UploadError {
    #[error(transparent)]
    Read(#[from] std::io::Error),
    #[error(transparent)]
    Store(#[from] object_store::Error),
}

#[derive(Debug, thiserror::Error)]
enum FetchSizeError {
    #[error("Object of {size} bytes exceeds the limit of {max} bytes")]
    TooLarge { size: u64, max: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;

    fn backend(settings: ObjectStoreSettings) -> ObjectStoreBackend {
        ObjectStoreBackend::new(settings, Arc::new(InMemory::new()))
    }

    fn with_prefix(prefix: Option<&str>) -> ObjectStoreBackend {
        backend(ObjectStoreSettings {
            prefix: prefix.map(Into::into),
            ..ObjectStoreSettings::default()
        })
    }

    #[test]
    fn objects_are_placed_under_the_prefix() {
        let id = ShortGuid::new_random();
        assert_eq!(
            with_prefix(Some("yy/files")).object_path(id).as_ref(),
            format!("yy/files/{id}")
        );
        assert_eq!(with_prefix(None).object_path(id).as_ref(), id.to_string());
        assert_eq!(
            with_prefix(Some("")).object_path(id).as_ref(),
            id.to_string()
        );
    }

    #[test]
    fn settings_are_applied() {
        let backend = backend(ObjectStoreSettings {
            tag: "store".into(),
            prefix: None,
            expiration: Some(Duration::from_secs(60)),
            max_file_size: Some(10),
            priority: 3,
        });
        assert_eq!(backend.tag(), "store");
        assert_eq!(backend.expiration, Some(Duration::from_secs(60)));
        assert_eq!(backend.max_file_size(), Some(10));
        assert_eq!(backend.priority(), 3);
    }

    #[test]
    fn zero_means_no_expiry_and_no_size_limit() {
        let backend = backend(ObjectStoreSettings {
            expiration: Some(Duration::ZERO),
            max_file_size: Some(0),
            ..ObjectStoreSettings::default()
        });
        assert_eq!(backend.expiration, None);
        assert_eq!(backend.max_file_size(), None);
    }

    #[test]
    fn client_options_follow_the_fail_fast_policy() {
        let retry = retry_config();
        assert_eq!(retry.max_retries, MAX_RETRIES);
        assert_eq!(retry.retry_timeout, RETRY_TIMEOUT);
        // ClientOptions offers no getters; building them must not panic.
        let _ = client_options(true);
    }
}
