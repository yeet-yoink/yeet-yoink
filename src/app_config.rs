use config::builder::DefaultState;
use config::{ConfigBuilder, File, FileFormat};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::error;

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    version: u8,
    pub backends: BackendsConfig,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct BackendsConfig {
    #[cfg_attr(docsrs, doc(cfg(feature = "memcache")))]
    #[cfg(feature = "memcache")]
    pub memcache: Vec<MemcacheBackendConfig>,
}

#[cfg_attr(docsrs, doc(cfg(feature = "memcache")))]
#[cfg(feature = "memcache")]
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
    pub connection_string: crate::backends::memcache::ConnectionString,
}

pub fn load_config(config_dir: &Path) -> Result<AppConfig, anyhow::Error> {
    // TODO: Document configuration file locations

    let mut config_builder = ConfigBuilder::<DefaultState>::default()
        .add_source(
            File::from(config_dir.join("default.yml"))
                .format(FileFormat::Yaml)
                .required(false),
        )
        .add_source(
            // The YAML FAQ requests `.yaml` to be used as the default.
            File::from(config_dir.join("default.yaml"))
                .format(FileFormat::Yaml)
                .required(false),
        );
    let config = match config_builder.build() {
        Ok(config) => config,
        Err(e) => {
            error!("Unable to load configuration: {error}", error = e);
            return Err(e.into());
        }
    };

    match config.try_deserialize() {
        Ok(config) => Ok(config),
        Err(e) => {
            error!("Unable to deserialize configuration: {error}", error = e);
            Err(e.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "memcache")]
    fn deserialize_memcache_works() {
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
