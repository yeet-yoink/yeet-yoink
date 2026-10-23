use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use std::time::Duration;
use url::Url;

/// The default expiration time for Memcached entries.
pub const DEFAULT_EXPIRATION: Duration = Duration::from_secs(3600);

/// The default largest file size accepted by a Memcached backend, in bytes.
///
/// Memcached's default item size limit is 1 MiB (`-I 1m`), and that limit
/// includes the key and per-item overhead, so a value of exactly 1 MiB is
/// rejected. This leaves 1 KiB of headroom below it.
pub const DEFAULT_MAX_FILE_SIZE_BYTES: u64 = 1024 * 1024 - 1024;

/// The Memcached-specific configuration.
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
    /// ```text
    /// 300
    /// ```
    pub expiration_sec: Option<u32>,
    /// The largest file size, in bytes, to store in this backend. Larger files
    /// are not distributed to it. Use `0` to disable the limit. Defaults to
    /// [`DEFAULT_MAX_FILE_SIZE_BYTES`].
    ///
    /// Raise this together with Memcached's item size limit (`-I`), keeping it
    /// slightly below that limit to leave room for the key and item overhead.
    ///
    /// ### Example
    ///
    /// To accept files just under 8 MiB on a server started with `-I 8m`:
    ///
    /// ```text
    /// 8387584
    /// ```
    pub max_file_size_bytes: Option<u64>,
    /// The priority of this backend. Backends with a higher priority are fetched
    /// from first and have their distributions started first. Defaults to `0`.
    ///
    /// ### Example
    ///
    /// To prefer this backend over backends with the default priority:
    ///
    /// ```text
    /// 10
    /// ```
    #[serde(default)]
    pub priority: i32,
}

/// A Memcached connection string.
#[derive(Debug, Default, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub struct MemcacheConnectionString(String);

impl MemcacheConnectionString {
    /// Creates a new [`MemcacheConnectionString`] from a string.
    fn new<S>(url: S) -> Result<Self, MemcacheConnectionStringError>
    where
        S: AsRef<str>,
    {
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

    /// Obtains the URLs encoded by this connection string.
    pub fn get_urls(&self) -> Vec<String> {
        if self.0.is_empty() {
            Vec::default()
        } else {
            vec![self.0.clone()]
        }
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
        assert_eq!(config.max_file_size_bytes, None);
        assert_eq!(config.priority, 0);
    }

    #[test]
    fn deserialize_memcache_config_reads_optional_settings() {
        let yaml = r#"
            tag: memcache-1
            connection_string: "memcache://127.0.0.1:12345"
            max_file_size_bytes: 8387584
            priority: -5
        "#;

        let config: MemcacheBackendConfig =
            serde_yaml::from_str(yaml).expect("Failed to deserialize Memcache config");
        assert_eq!(config.max_file_size_bytes, Some(8 * 1024 * 1024 - 1024));
        assert_eq!(config.priority, -5);
    }

    #[test]
    fn connection_string_parse_works() {
        let valid_conn_str = "memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true";
        let invalid_conn_str = "invalid_url";

        let valid_result: Result<MemcacheConnectionString, _> = valid_conn_str.parse();
        let invalid_result: Result<MemcacheConnectionString, _> = invalid_conn_str.parse();

        assert_eq!(valid_result.expect("Failed to parse"), valid_conn_str);
        assert!(matches!(
            invalid_result,
            Err(MemcacheConnectionStringError::InvalidFormat)
        ));
    }

    #[test]
    fn connection_string_rejects_other_schemes() {
        let result: Result<MemcacheConnectionString, _> = "redis://127.0.0.1:6379".parse();
        assert!(matches!(
            result,
            Err(MemcacheConnectionStringError::InvalidFormat)
        ));
    }

    #[test]
    fn connection_string_get_urls() {
        let conn_str: MemcacheConnectionString = "memcache://127.0.0.1:12345"
            .parse()
            .expect("Failed to parse");
        assert_eq!(conn_str.get_urls(), vec!["memcache://127.0.0.1:12345"]);
        assert!(MemcacheConnectionString::default().get_urls().is_empty());
    }

    #[test]
    fn deserialize_rejects_invalid_connection_string() {
        let result: Result<MemcacheConnectionString, _> = serde_yaml::from_str("invalid_url");
        assert!(result.is_err());
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
