// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "memcache")]
pub mod memcache;

use clap::ArgMatches;
use config::builder::DefaultState;
use config::{ConfigBuilder, File, FileFormat};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{error, info};

/// The application configuration.
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    /// The version of the configuration.
    version: u8,
    /// The backend-specific configuration.
    pub backends: BackendsConfig,
}

/// Provides backend-specific configuration.
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct BackendsConfig {
    /// Provides Memcached specific configuration.
    #[cfg_attr(docsrs, doc(cfg(feature = "memcache")))]
    #[cfg(feature = "memcache")]
    pub memcache: Vec<memcache::MemcacheBackendConfig>,
}

impl AppConfig {
    pub fn load(config_dir: &Path, matches: &ArgMatches) -> Result<Self, anyhow::Error> {
        // TODO: Document configuration file locations
        let mut config_builder = ConfigBuilder::<DefaultState>::default();

        // Add default configuration.
        config_builder = config_builder
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

        if let Some(path) = matches.get_one::<PathBuf>("config_file").cloned() {
            info!(
                "Loading configuration file from {config_path:?}",
                config_path = path
            );
            config_builder =
                config_builder.add_source(File::from(path).format(FileFormat::Yaml).required(true))
        }

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
}
