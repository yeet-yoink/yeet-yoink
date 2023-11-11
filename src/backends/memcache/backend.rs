use crate::app_config::AppConfig;
use crate::backends::memcache::{MemcacheBackendConfig, MemcacheConnectionString};
use crate::backends::Backend;
use memcache::Client;
use r2d2::Pool;
use r2d2_memcache::MemcacheConnectionManager;
use std::str::FromStr;

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

impl Backend for MemcacheBackend {
    fn backend_info(&self) -> &str {
        "Memcached"
    }

    fn tag(&self) -> &str {
        &self.tag
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
