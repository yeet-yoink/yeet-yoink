use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use std::time::Duration;
use url::Url;

/// The default expiration time for Memcached entries.
pub const DEFAULT_EXPIRATION: Duration = Duration::from_secs(3600);

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct MemcacheBackendConfig {
    /// A tag to identify the backend.
    pub tag: String,
    /// The connection string
    ///
    /// ## Example
    /// ```text
    /// memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true
    /// ```
    pub connection_string: MemcacheConnectionString,
    /// The number of seconds after which the item is considered expired. Use `0`
    /// to keep the entry indefinitely. Defaults to [`DEFAULT_EXPIRATION`].
    ///
    /// ### Example
    ///
    /// To keep the example for 5 minutes, use a value of 300 seconds:
    ///
    /// ```
    /// 300
    /// ```
    pub expiration_sec: Option<u32>,
}

/// A Memcached connection string.
#[derive(Debug, Default, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub struct MemcacheConnectionString(String);

impl MemcacheConnectionString {
    fn new<S: AsRef<str>>(url: S) -> Result<Self, MemcacheConnectionStringError> {
        let parsed_url = Url::parse(url.as_ref());
        match parsed_url {
            Ok(url) => {
                if url.scheme() != "memcache" {
                    Err(MemcacheConnectionStringError::InvalidFormat)
                } else {
                    Ok(MemcacheConnectionString(url.to_string()))
                }
            }
            Err(_) => Err(MemcacheConnectionStringError::InvalidFormat),
        }
    }

    pub fn get_urls(&self) -> Vec<String> {
        if self.0.is_empty() {
            Vec::default()
        } else {
            vec![self.0.clone()]
        }
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MemcacheConnectionStringError {
    #[error("Invalid connection string format")]
    InvalidFormat,
}

impl FromStr for MemcacheConnectionString {
    type Err = MemcacheConnectionStringError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        MemcacheConnectionString::new(s)
    }
}

impl Serialize for MemcacheConnectionString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MemcacheConnectionString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;
        MemcacheConnectionString::new(s).map_err(de::Error::custom)
    }
}

impl PartialEq<&str> for MemcacheConnectionString {
    fn eq(&self, other: &&str) -> bool {
        self.0.eq(other)
    }
}

impl Display for MemcacheConnectionString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_memcache_config_works() {
        let yaml = r#"
            tag: memcache-1
            connection_string: "memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true"
            expiration_sec: 500
        "#;

        let config: MemcacheBackendConfig =
            serde_yaml::from_str(yaml).expect("Failed to deserialize Memcache config");
        assert_eq!(config.tag, "memcache-1");
        assert_eq!(
            config.connection_string,
            "memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true"
        );
        assert_eq!(config.expiration_sec, Some(500));
    }

    #[test]
    fn connection_string_parse_works() {
        let valid_conn_str = "memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true";
        let invalid_conn_str = "invalid_url";

        let valid_result: Result<MemcacheConnectionString, _> = valid_conn_str.parse();
        let invalid_result: Result<MemcacheConnectionString, _> = invalid_conn_str.parse();

        match valid_result {
            Ok(conn_str) => println!("Valid connection string: {}", conn_str),
            Err(err) => println!("Error parsing connection string: {}", err),
        }

        match invalid_result {
            Ok(conn_str) => println!("Valid connection string: {}", conn_str),
            Err(err) => println!("Error parsing connection string: {}", err),
        }
    }

    #[test]
    fn connection_string_deserialize_works() {
        let valid_result: MemcacheConnectionString =
            serde_yaml::from_str("memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true")
                .expect("Deserialization failed");
        assert_eq!(
            valid_result,
            "memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true"
        );
    }
}
