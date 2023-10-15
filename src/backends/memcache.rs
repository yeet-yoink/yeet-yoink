use memcache::{Client, Url};
use r2d2::Pool;
use r2d2_memcache::MemcacheConnectionManager;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::str::FromStr;

/// A memcache connection string.
#[derive(Debug, Default)]
pub struct ConnectionString(String);

impl ConnectionString {
    fn new<S: AsRef<str>>(url: S) -> Result<Self, ConnectionStringError> {
        let parsed_url = Url::parse(url.as_ref());
        match parsed_url {
            Ok(url) => Ok(ConnectionString(url.to_string())),
            Err(_) => Err(ConnectionStringError::InvalidFormat),
        }
    }

    fn into_inner(self) -> String {
        self.0
    }

    fn get_urls(self) -> Vec<String> {
        if self.0.is_empty() {
            Vec::default()
        } else {
            vec![self.0]
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectionStringError {
    #[error("Invalid connection string format")]
    InvalidFormat,
}

impl FromStr for ConnectionString {
    type Err = ConnectionStringError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ConnectionString::new(s)
    }
}

impl Serialize for ConnectionString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ConnectionString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;
        ConnectionString::new(s).map_err(de::Error::custom)
    }
}

impl memcache::Connectable for ConnectionString {
    fn get_urls(self) -> Vec<String> {
        self.get_urls()
    }
}

impl r2d2_memcache::memcache::Connectable for ConnectionString {
    fn get_urls(self) -> Vec<String> {
        self.get_urls()
    }
}

impl PartialEq<&str> for ConnectionString {
    fn eq(&self, other: &&str) -> bool {
        self.0.eq(other)
    }
}

pub struct MemcacheBackend {
    /// The connection pool
    pool: Pool<MemcacheConnectionManager>,
}

impl MemcacheBackend {
    pub fn try_new(connection_string: ConnectionString) -> Result<Self, BackendConstructionError> {
        let manager = MemcacheConnectionManager::new(connection_string);
        let pool =
            Pool::new(manager).map_err(|e| BackendConstructionError::FailedToCreatePool(e))?;
        Ok(Self { pool })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BackendConstructionError {
    #[error("Failed to create pool")]
    FailedToCreatePool(r2d2::Error),
}

fn _trivial() {
    let server = ConnectionString::from_str("memcache://127.0.0.1:11211")
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
    let server = ConnectionString::from_str("memcache://127.0.0.1:11211")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_string_parse_works() {
        let valid_conn_str = "memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true";
        let invalid_conn_str = "invalid_url";

        let valid_result: Result<ConnectionString, _> = valid_conn_str.parse();
        let invalid_result: Result<ConnectionString, _> = invalid_conn_str.parse();

        match valid_result {
            Ok(conn_str) => println!("Valid connection string: {}", conn_str.into_inner()),
            Err(err) => println!("Error parsing connection string: {}", err),
        }

        match invalid_result {
            Ok(conn_str) => println!("Valid connection string: {}", conn_str.into_inner()),
            Err(err) => println!("Error parsing connection string: {}", err),
        }
    }

    #[test]
    fn connection_string_deserialize_works() {
        let valid_result: ConnectionString =
            serde_yaml::from_str("memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true")
                .expect("Deserialization failed");
        assert_eq!(
            valid_result,
            "memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true"
        );
    }
}
