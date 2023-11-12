use crate::app_config::AppConfig;
use crate::backbone::WriteSummary;
use crate::backends::map_ok::MapOkIter;
use crate::backends::memcache::{MemcacheBackendConfig, MemcacheConnectionString};
use crate::backends::{Backend, DistributionError, DynBackend, TryCreateFromConfig};
use axum::async_trait;
use memcache::Client;
use r2d2::Pool;
use r2d2_memcache::MemcacheConnectionManager;
use shortguid::ShortGuid;
use std::str::FromStr;
use std::sync::Arc;

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
            .map_err(|e| MemcacheBackendConstructionError::FailedToCreatePool(e))?;
        Ok(Self {
            tag: config.tag.clone(),
            pool,
        })
    }
}

#[async_trait]
impl Backend for MemcacheBackend {
    fn backend_info(&self) -> &str {
        "Memcached"
    }

    fn tag(&self) -> &str {
        &self.tag
    }

    async fn distribute_file(
        &self,
        id: ShortGuid,
        summary: Arc<WriteSummary>,
    ) -> Result<(), DistributionError> {
        todo!()
    }
}

impl TryFrom<&AppConfig> for Vec<MemcacheBackend> {
    type Error = MemcacheBackendConstructionError;

    fn try_from(value: &AppConfig) -> Result<Self, Self::Error> {
        if value.backends.memcache.is_empty() {
            return Ok(Vec::default());
        }

        value
            .backends
            .memcache
            .iter()
            .map(MemcacheBackend::try_new)
            .collect()
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
            .map_ok(Box::new)
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
