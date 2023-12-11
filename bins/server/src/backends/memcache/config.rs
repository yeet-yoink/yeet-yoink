use crate::backends::memcache::MemcacheConnectionString;
use serde::{Deserialize, Serialize};
use std::time::Duration;

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
}
