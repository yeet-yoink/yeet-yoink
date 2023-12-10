use crate::app_config::AppConfig;
use crate::backbone::{FileAccessor, WriteSummary};
use crate::backends::memcache::{MemcacheBackendConfig, MemcacheConnectionString};
use crate::backends::registry::BackendInfo;
use crate::backends::{
    Backend, BoxOkIter, DistributionError, DynBackend, MapOkIter, TryCreateFromConfig,
};
use axum::async_trait;
use memcache::Client;
use r2d2::Pool;
use r2d2_memcache::MemcacheConnectionManager;
use shortguid::ShortGuid;
use std::str::FromStr;
use std::sync::Arc;
use tokio::io::AsyncReadExt;

pub struct MemcacheBackend {
    /// The tag identifying the backend.
    tag: String,
    /// The connection pool
    pool: Pool<MemcacheConnectionManager>,
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
        Ok(Self {
            tag: config.tag.clone(),
            pool,
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
        // use a Vec to collect the stream chunks
        let mut buffer: Vec<u8> = Vec::with_capacity(summary.file_size_bytes);

        let mut file = file_accessor.get_file(id).await?;
        file.read_to_end(&mut buffer).await?;

        // Get a memoized connection
        let client = self.pool.get().unwrap();

        // Collect the data from the stream and write it to a Memcached server
        let key = id.to_string();
        client
            .set(&key, buffer.as_slice(), 0)
            .map_err(|e| DistributionError::BackendSpecific(Box::new(e)))?;

        // TODO: Write item metadata.

        Ok(())
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

fn _trivial() {
    let server = MemcacheConnectionString::from_str("memcache://127.0.0.1:11211")
        .expect("Failed to parse connection string");
    let client = Client::connect(server).expect("Failed to connect to Memcache server");

    client
        .set("my_key", "my_value", 0)
        .expect("Failed to set value");
    let value: Option<String> = client.get("my_key").expect("Failed to get value");
    println!("Retrieved value: {:?}", value);
    assert_eq!(value.unwrap(), "my_value");
}

fn _pooled() {
    // Create a connection manager
    let server = MemcacheConnectionString::from_str("memcache://127.0.0.1:11211")
        .expect("Failed to parse connection string");
    let manager = MemcacheConnectionManager::new(server);

    // Create the connection pool
    let pool = Pool::new(manager).expect("Failed to create pool");

    // Get a connection from the pool
    let conn = pool.get().expect("Failed to get connection from pool");

    // Use the connection (e.g., set and get a value)
    conn.set("my_key", "my_value", 0)
        .expect("Failed to set value");
    let value: Option<String> = conn.get("my_key").expect("Failed to get value");
    println!("Retrieved value: {:?}", value);
    assert_eq!(value.unwrap(), "my_value");
}
