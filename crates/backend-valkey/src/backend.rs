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
use file_distribution::{FileProvider, GetFile, WriteSummary};
use map_ok::{BoxOk, MapOk};
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use redis::{Client, RedisError};
use shortguid::ShortGuid;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::io::AsyncReadExt;
use tracing::trace;

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
            .get_connection_manager_lazy(ConnectionManagerConfig::new())
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
        // The backend registry skips files above max_file_size(), so the
        // content can be buffered in memory.
        let mut file = file_provider.get_file(id).await?;
        let mut data = Vec::with_capacity(summary.file_size_bytes);
        file.read_to_end(&mut data).await?;

        let expiration = self.expiration_secs;
        let expires_at =
            (expiration != 0).then(|| SystemTime::now() + Duration::from_secs(expiration));
        let metadata = ItemMetadata::new(id, &summary, expires_at)
            .serialize_to_proto()
            .map_err(|e| DistributionError::BackendSpecific(Box::new(e)))?;

        // Store data and metadata together so that readers never see metadata
        // without its data.
        let mut pipe = redis::pipe();
        pipe.atomic();
        if expiration == 0 {
            pipe.set(data_key(id), data).ignore();
            pipe.set(meta_key(id), metadata.as_ref()).ignore();
        } else {
            pipe.set_ex(data_key(id), data, expiration).ignore();
            pipe.set_ex(meta_key(id), metadata.as_ref(), expiration)
                .ignore();
        }

        let mut connection = self.connection.clone();
        pipe.query_async::<()>(&mut connection)
            .await
            .map_err(|e| DistributionError::BackendSpecific(Box::new(e)))?;
        trace!(file_id = %id, "Stored file {id} in valkey with expiration {expiration}");
        Ok(())
    }
}

#[async_trait]
impl FetchFile for ValkeyBackend {
    async fn fetch_file(&self, id: ShortGuid) -> Result<Option<FetchedFile>, FetchError> {
        let mut connection = self.connection.clone();
        let (metadata, data): (Option<Vec<u8>>, Option<Vec<u8>>) = redis::cmd("MGET")
            .arg(meta_key(id))
            .arg(data_key(id))
            .query_async(&mut connection)
            .await
            .map_err(|e: RedisError| FetchError::BackendSpecific(Box::new(e)))?;

        let (Some(metadata), Some(data)) = (metadata, data) else {
            trace!(file_id = %id, "File {id} not found in valkey");
            return Ok(None);
        };

        let file = FetchedFile::from_stored(&metadata, Bytes::from(data))?;
        trace!(file_id = %id, "Fetched file {id} from valkey");
        Ok(Some(file))
    }
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
