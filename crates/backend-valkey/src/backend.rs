use app_config::{
    AppConfig,
    valkey::{DEFAULT_EXPIRATION, DEFAULT_MAX_FILE_SIZE_BYTES, ValkeyBackendConfig},
};
use async_trait::async_trait;
use backend_traits::{
    Backend, BackendInfo, DistributeFile, DistributionError, FetchError, FetchFile, FetchedFile,
    TryCreateFromConfig,
};
use bytes::Bytes;
use file_distribution::protobuf::ItemMetadata;
use file_distribution::{BoxedFileReader, FileProvider, GetFile, WriteSummary};
use futures::{Stream, stream};
use map_ok::{BoxOk, MapOk};
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use redis::{AsyncCommands, Client, RedisError};
use shortguid::ShortGuid;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::io::AsyncReadExt;
use tokio_util::io::StreamReader;
use tracing::trace;

/// How long a single connection attempt may take.
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(1);

/// The size of the chunks in which files are written.
const WRITE_CHUNK_SIZE: usize = 1024 * 1024;

/// How long a partially written file is kept should its upload be abandoned.
const UPLOAD_EXPIRATION: Duration = Duration::from_secs(3600);

/// The size of the ranges in which stored files are read back.
const READ_CHUNK_SIZE: u64 = 1024 * 1024;

/// How often a failed connection attempt is retried within one request.
///
/// Requests fail fast while Valkey is unreachable so that fetches fall through
/// to the next backend quickly; later requests reconnect on their own.
const CONNECTION_RETRIES: usize = 1;

pub struct ValkeyBackend {
    /// The tag identifying the backend.
    tag: String,
    /// The connection, established lazily and re-established on failure.
    connection: ConnectionManager,
    /// The expiration time for stored entries, or `0` to keep them indefinitely.
    expiration_secs: u64,
    /// The largest file size accepted for storage, or `None` for no limit.
    max_file_size: Option<u64>,
    /// The priority of this backend relative to others.
    priority: i32,
}

impl ValkeyBackend {
    /// Creates the backend without connecting; the connection is established
    /// on first use.
    pub fn try_new(config: &ValkeyBackendConfig) -> Result<Self, ValkeyBackendConstructionError> {
        let client = Client::open(config.connection_string.as_str())
            .map_err(ValkeyBackendConstructionError::InvalidConnectionString)?;
        let connection = client
            .get_connection_manager_lazy(
                ConnectionManagerConfig::new()
                    .set_connection_timeout(Some(CONNECTION_TIMEOUT))
                    .set_number_of_retries(CONNECTION_RETRIES),
            )
            .map_err(ValkeyBackendConstructionError::FailedToCreateConnection)?;

        let expiration_secs = config
            .expiration_sec
            .map_or(DEFAULT_EXPIRATION, |secs| Duration::from_secs(secs.into()))
            .as_secs();

        // A limit of 0 disables the size check.
        let max_file_size = Some(
            config
                .max_file_size_bytes
                .unwrap_or(DEFAULT_MAX_FILE_SIZE_BYTES),
        )
        .filter(|&max| max != 0);

        Ok(Self {
            tag: config.tag.clone(),
            connection,
            expiration_secs,
            max_file_size,
            priority: config.priority,
        })
    }
}

fn data_key(id: ShortGuid) -> String {
    format!("data-{id}")
}

fn meta_key(id: ShortGuid) -> String {
    format!("meta-{id}")
}

#[async_trait]
impl DistributeFile for ValkeyBackend {
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
        let expiration = self.expiration_secs;
        let expires_at =
            (expiration != 0).then(|| SystemTime::now() + Duration::from_secs(expiration));
        let metadata = ItemMetadata::new(id, &summary, expires_at)
            .serialize_to_proto()
            .map_err(|e| DistributionError::BackendSpecific(Box::new(e)))?;

        // The content is written in chunks to a temporary key, so that readers
        // never see a partial value. The key expires on its own should the
        // upload be abandoned.
        let file = file_provider.get_file(id).await?;
        let temporary_key = format!("upload-{id}-{}", ShortGuid::new_random());
        let mut connection = self.connection.clone();
        if let Err(e) = upload_in_chunks(&mut connection, &temporary_key, file).await {
            let _: Result<(), _> = connection.del(&temporary_key).await;
            return Err(match e {
                UploadError::Read(e) => DistributionError::Io(e),
                UploadError::Valkey(e) => DistributionError::BackendSpecific(Box::new(e)),
            });
        }

        // Move the content into place and store the metadata in one step, so
        // that readers never see metadata without its data.
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.rename(&temporary_key, data_key(id)).ignore();
        if expiration == 0 {
            pipe.persist(data_key(id)).ignore();
            pipe.set(meta_key(id), metadata.as_ref()).ignore();
        } else {
            pipe.expire(data_key(id), expiration as i64).ignore();
            pipe.set_ex(meta_key(id), metadata.as_ref(), expiration)
                .ignore();
        }
        pipe.query_async::<()>(&mut connection)
            .await
            .map_err(|e| DistributionError::BackendSpecific(Box::new(e)))?;
        trace!(file_id = %id, "Stored file {id} in valkey with expiration {expiration}");
        Ok(())
    }
}

/// Writes a file to a key in chunks of [`WRITE_CHUNK_SIZE`] bytes.
async fn upload_in_chunks(
    connection: &mut ConnectionManager,
    key: &str,
    mut file: BoxedFileReader,
) -> Result<(), UploadError> {
    // The first chunk creates the key with its expiry; APPEND keeps it.
    let _: () = connection
        .set_ex(key, &b""[..], UPLOAD_EXPIRATION.as_secs())
        .await?;
    let mut chunk = vec![0; WRITE_CHUNK_SIZE];
    loop {
        let read = file.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        let _: u64 = connection.append(key, &chunk[..read]).await?;
    }
}

/// An error while writing a file in chunks.
#[derive(Debug, thiserror::Error)]
enum UploadError {
    #[error(transparent)]
    Read(#[from] std::io::Error),
    #[error(transparent)]
    Valkey(#[from] RedisError),
}

#[async_trait]
impl FetchFile for ValkeyBackend {
    async fn fetch_file(&self, id: ShortGuid) -> Result<Option<FetchedFile>, FetchError> {
        let mut connection = self.connection.clone();
        let (metadata, exists, size): (Option<Vec<u8>>, bool, u64) = redis::pipe()
            .get(meta_key(id))
            .exists(data_key(id))
            .strlen(data_key(id))
            .query_async(&mut connection)
            .await
            .map_err(|e: RedisError| FetchError::BackendSpecific(Box::new(e)))?;

        let (Some(metadata), true) = (metadata, exists) else {
            trace!(file_id = %id, "File {id} not found in valkey");
            return Ok(None);
        };

        // The content is read in ranges as it is consumed. Should the value
        // change or vanish in between, the size and hash checks reject the copy.
        let content = StreamReader::new(read_in_ranges(connection, data_key(id), size));
        let file = FetchedFile::from_stored(&metadata, Some(size), Box::new(content))?;
        trace!(file_id = %id, "Fetching file {id} of {size} bytes from valkey");
        Ok(Some(file))
    }
}

/// Reads a value in chunks of [`READ_CHUNK_SIZE`] bytes using `GETRANGE`.
fn read_in_ranges(
    connection: ConnectionManager,
    key: String,
    size: u64,
) -> impl Stream<Item = std::io::Result<Bytes>> + Send + Unpin {
    Box::pin(stream::try_unfold(
        (connection, 0u64),
        move |(mut connection, offset)| {
            let key = key.clone();
            async move {
                if offset >= size {
                    return Ok(None);
                }
                // GETRANGE takes an inclusive end offset.
                let end = (offset + READ_CHUNK_SIZE).min(size) - 1;
                let chunk: Vec<u8> = redis::cmd("GETRANGE")
                    .arg(&key)
                    .arg(offset)
                    .arg(end)
                    .query_async(&mut connection)
                    .await
                    .map_err(std::io::Error::other)?;
                if chunk.is_empty() {
                    // The value shrank or vanished; end the stream early.
                    return Ok(None);
                }
                let next = offset + chunk.len() as u64;
                Ok(Some((Bytes::from(chunk), (connection, next))))
            }
        },
    ))
}

impl BackendInfo for ValkeyBackend {
    fn backend_name() -> &'static str {
        "Valkey"
    }

    fn backend_version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
}

impl TryCreateFromConfig for ValkeyBackend {
    type Error = ValkeyBackendConstructionError;

    fn try_from_config(config: &AppConfig) -> Result<Vec<Backend>, Self::Error> {
        config
            .backends
            .valkey
            .iter()
            .map(ValkeyBackend::try_new)
            .box_ok()
            .map_ok(Backend::from)
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ValkeyBackendConstructionError {
    #[error("Invalid connection string: {0}")]
    InvalidConnectionString(RedisError),
    #[error("Failed to create connection: {0}")]
    FailedToCreateConnection(RedisError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(tag: &str) -> ValkeyBackendConfig {
        ValkeyBackendConfig {
            tag: tag.into(),
            connection_string: "valkey://127.0.0.1:6379"
                .parse()
                .expect("Invalid connection string"),
            ..ValkeyBackendConfig::default()
        }
    }

    #[test]
    fn reports_backend_info() {
        assert_eq!(ValkeyBackend::backend_name(), "Valkey");
        assert_eq!(ValkeyBackend::backend_version(), env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn creates_one_backend_per_config_without_connecting() {
        let mut app_config = AppConfig::default();
        app_config.backends.valkey = vec![config("first"), config("second")];

        let backends = ValkeyBackend::try_from_config(&app_config).expect("Failed to create");

        let tags: Vec<_> = backends.iter().map(|backend| backend.tag()).collect();
        assert_eq!(tags, ["first", "second"]);
    }

    #[tokio::test]
    async fn no_config_creates_no_backends() {
        let backends =
            ValkeyBackend::try_from_config(&AppConfig::default()).expect("Failed to create");
        assert!(backends.is_empty());
    }
}
