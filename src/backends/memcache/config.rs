use crate::backends::memcache::MemcacheConnectionString;
use serde::{Deserialize, Serialize};

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_memcache_config_works() {
        let yaml = r#"
            tag: memcache-1
            connection_string: "memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true"
        "#;

        let config: MemcacheBackendConfig =
            serde_yaml::from_str(yaml).expect("Failed to deserialize Memcache config");
        assert_eq!(config.tag, "memcache-1");
        assert_eq!(
            config.connection_string,
            "memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true"
        );
    }
}
