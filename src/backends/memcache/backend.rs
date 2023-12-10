use crate::app_config::AppConfig;
use crate::backbone::{FileAccessor, FileReader, WriteSummary};
use crate::backends::memcache::config::DEFAULT_EXPIRATION;
use crate::backends::memcache::MemcacheBackendConfig;
use crate::backends::registry::BackendInfo;
use crate::backends::{
    Backend, BoxOkIter, DistributionError, DynBackend, MapOkIter, TryCreateFromConfig,
};
use axum::async_trait;
use r2d2::Pool;
use r2d2_memcache::memcache::{MemcacheError, ToMemcacheValue};
use r2d2_memcache::MemcacheConnectionManager;
use shortguid::ShortGuid;
use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::spawn_blocking;
use tokio_util::io::SyncIoBridge;
use tracing::trace;

pub struct MemcacheBackend {
    /// The tag identifying the backend.
    tag: String,
    /// The connection pool
    pool: Pool<MemcacheConnectionManager>,
    /// The expiration time for stored entries.
    expiration_secs: u32,
}

impl MemcacheBackend {
    pub fn try_new(
        config: &MemcacheBackendConfig,
    ) -> Result<Self, MemcacheBackendConstructionError> {
        let manager = MemcacheConnectionManager::new(&config.connection_string);
        let pool = Pool::builder()
            .min_idle(Some(1))
            .build(manager)
            .map_err(MemcacheBackendConstructionError::FailedToCreatePool)?;

        let expiration_secs = config
            .expiration_sec
            .map_or(DEFAULT_EXPIRATION, |secs| Duration::from_secs(secs as _))
            .as_secs()
            .min(u32::MAX as _) as u32;
        Ok(Self {
            tag: config.tag.clone(),
            pool,
            expiration_secs,
        })
    }
}

#[async_trait]
impl Backend for MemcacheBackend {
    fn tag(&self) -> &str {
        &self.tag
    }

    async fn distribute_file(
        &self,
        id: ShortGuid,
        summary: Arc<WriteSummary>,
        file_accessor: Arc<dyn FileAccessor>,
    ) -> Result<(), DistributionError> {
        let expiration = self.expiration_secs;
        let file = file_accessor.get_file(id).await?;
        let client = self.pool.get().unwrap();

        let result: Result<(), MemcacheError> = spawn_blocking(move || {
            let file = StreamWrapper::new(summary, file);

            let key = format!("data-{}", id);
            client.set(&key, file, expiration)?;
            trace!("Stored data under key {key} with expiration {expiration}");

            // TODO: Write item metadata.

            Ok(())
        })
        .await?;

        match result {
            Ok(()) => Ok(()),
            Err(e) => Err(DistributionError::BackendSpecific(Box::new(e))),
        }
    }
}

struct StreamWrapper {
    summary: Arc<WriteSummary>,
    bridge: Cell<Option<SyncIoBridge<FileReader>>>,
}

impl StreamWrapper {
    pub fn new(summary: Arc<WriteSummary>, reader: FileReader) -> StreamWrapper {
        Self {
            summary,
            bridge: Cell::new(Some(SyncIoBridge::new(reader))),
        }
    }
}

impl<W> ToMemcacheValue<W> for StreamWrapper
where
    W: std::io::Write,
{
    fn get_flags(&self) -> u32 {
        0_u32
    }

    fn get_length(&self) -> usize {
        self.summary.file_size_bytes
    }

    fn write_to(&self, stream: &mut W) -> std::io::Result<()> {
        if let Some(mut bridge) = self.bridge.take() {
            std::io::copy(&mut bridge, stream)?;
            Ok(())
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Source already read to end",
            ))
        }
    }
}

impl BackendInfo for MemcacheBackend {
    fn backend_name() -> &'static str {
        "Memcached"
    }

    fn backend_version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
}

impl TryCreateFromConfig for MemcacheBackend {
    type Error = MemcacheBackendConstructionError;

    fn try_from_config(config: &AppConfig) -> Result<Vec<DynBackend>, Self::Error> {
        let configs = &config.backends.memcache;
        if configs.is_empty() {
            return Ok(Vec::default());
        }

        configs
            .iter()
            .map(MemcacheBackend::try_new)
            .box_ok()
            .map_ok(DynBackend::from)
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MemcacheBackendConstructionError {
    #[error("Failed to create pool")]
    FailedToCreatePool(r2d2::Error),
}
