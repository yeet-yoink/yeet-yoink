// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod cluster;
#[cfg(feature = "gcs")]
pub mod gcs;
#[cfg(feature = "memcache")]
pub mod memcache;
#[cfg(feature = "valkey")]
pub mod valkey;

use clap::ArgMatches;
use config::builder::DefaultState;
use config::{ConfigBuilder, Environment, File, FileFormat};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{error, info};

/// The application configuration.
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    /// The version of the configuration.
    version: u8,
    /// The local cache configuration.
    #[serde(default)]
    pub cache: CacheConfig,
    /// The backend-specific configuration.
    pub backends: BackendsConfig,
    /// Finding and gossiping with other instances; disabled when absent.
    #[serde(default)]
    pub cluster: Option<cluster::ClusterConfig>,
}

/// The default local cache lease duration, in seconds.
const fn default_lease_secs() -> u64 {
    5 * 60
}

/// The near cache is enabled by default.
const fn default_near_cache() -> bool {
    true
}

/// Configuration of the local file cache.
#[derive(Debug, Serialize, Deserialize)]
pub struct CacheConfig {
    /// The number of seconds a buffered file remains available to readers after
    /// it was written. Defaults to [`default_lease_secs`].
    #[serde(default = "default_lease_secs")]
    pub lease_duration_secs: u64,
    /// Whether files fetched from a backend are buffered locally again, so that
    /// repeated reads are served without contacting the backend. Defaults to `true`.
    #[serde(default = "default_near_cache")]
    pub near_cache: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            lease_duration_secs: default_lease_secs(),
            near_cache: default_near_cache(),
        }
    }
}

impl CacheConfig {
    /// The lease duration as a [`Duration`].
    pub fn lease_duration(&self) -> Duration {
        Duration::from_secs(self.lease_duration_secs)
    }
}

/// Provides backend-specific configuration.
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct BackendsConfig {
    /// Provides Memcached specific configuration.
    #[cfg_attr(docsrs, doc(cfg(feature = "memcache")))]
    #[cfg(feature = "memcache")]
    #[serde(default)]
    pub memcache: Vec<memcache::MemcacheBackendConfig>,
    /// Provides Valkey specific configuration.
    #[cfg_attr(docsrs, doc(cfg(feature = "valkey")))]
    #[cfg(feature = "valkey")]
    #[serde(default)]
    pub valkey: Vec<valkey::ValkeyBackendConfig>,
    /// Provides Google Cloud Storage specific configuration.
    #[cfg_attr(docsrs, doc(cfg(feature = "gcs")))]
    #[cfg(feature = "gcs")]
    #[serde(default)]
    pub gcs: Vec<gcs::GcsBackendConfig>,
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

        // Environment variables override the files, e.g. to inject the pod IP.
        config_builder = config_builder.add_source(environment());

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

/// Reads settings from environment variables such as `YY_CLUSTER__ADVERTISE_IP`:
/// the `YY_` prefix, then the path of the setting with `__` between levels.
/// Lists, such as `YY_CLUSTER__SEEDS`, are separated by commas.
fn environment() -> Environment {
    Environment::with_prefix("YY")
        .prefix_separator("_")
        .separator("__")
        .try_parsing(true)
        .list_separator(",")
        .with_list_parse_key("cluster.seeds")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_config_defaults_to_five_minutes() {
        let config = CacheConfig::default();
        assert_eq!(config.lease_duration_secs, 300);
        assert_eq!(config.lease_duration(), Duration::from_secs(300));
    }

    #[test]
    fn cache_config_uses_default_when_field_missing() {
        let config: CacheConfig = serde_yaml::from_str("{}").expect("Failed to deserialize");
        assert_eq!(config.lease_duration(), Duration::from_secs(300));
    }

    #[test]
    fn near_cache_is_enabled_by_default() {
        assert!(CacheConfig::default().near_cache);
        let config: CacheConfig = serde_yaml::from_str("{}").expect("Failed to deserialize");
        assert!(config.near_cache);
    }

    #[test]
    fn near_cache_can_be_disabled() {
        let config: CacheConfig =
            serde_yaml::from_str("near_cache: false").expect("Failed to deserialize");
        assert!(!config.near_cache);
    }

    #[test]
    fn cache_config_lease_is_configurable() {
        let config: CacheConfig =
            serde_yaml::from_str("lease_duration_secs: 42").expect("Failed to deserialize");
        assert_eq!(config.lease_duration(), Duration::from_secs(42));
    }

    #[test]
    #[cfg(feature = "memcache")]
    fn example_config_deserializes() {
        let yaml = include_str!("../../../example-config.yaml");
        let config: AppConfig = serde_yaml::from_str(yaml).expect("Failed to deserialize");

        // The example does not specify a cache section, so the default applies.
        assert_eq!(config.cache.lease_duration(), Duration::from_secs(300));

        assert_eq!(config.backends.memcache.len(), 1);
        let memcache = &config.backends.memcache[0];
        assert_eq!(memcache.tag, "memcache-1");
        assert_eq!(memcache.expiration_sec, Some(500));
        assert_eq!(memcache.max_file_size_bytes, Some(1047552));
        assert_eq!(memcache.priority, 10);
    }

    #[test]
    #[cfg(feature = "valkey")]
    fn example_config_deserializes_valkey() {
        let yaml = include_str!("../../../example-config.yaml");
        let config: AppConfig = serde_yaml::from_str(yaml).expect("Failed to deserialize");

        assert_eq!(config.backends.valkey.len(), 1);
        let valkey = &config.backends.valkey[0];
        assert_eq!(valkey.tag, "valkey-1");
        assert_eq!(valkey.expiration_sec, Some(3600));
        assert_eq!(valkey.max_file_size_bytes, Some(64 * 1024 * 1024));
        assert_eq!(valkey.priority, 10);
    }

    #[test]
    #[cfg(feature = "gcs")]
    fn example_config_deserializes_gcs() {
        let yaml = include_str!("../../../example-config.yaml");
        let config: AppConfig = serde_yaml::from_str(yaml).expect("Failed to deserialize");

        assert_eq!(config.backends.gcs.len(), 1);
        let gcs = &config.backends.gcs[0];
        assert_eq!(gcs.tag, "gcs-1");
        assert_eq!(gcs.bucket, "my-yeet-yoink-bucket");
        assert_eq!(gcs.prefix.as_deref(), Some("files"));
        assert_eq!(gcs.service_account_path, None);
        assert_eq!(gcs.expiration_sec, Some(86400));
        assert_eq!(gcs.max_file_size_bytes, None);
    }

    #[test]
    #[cfg(all(feature = "memcache", feature = "valkey"))]
    fn backend_sections_are_optional() {
        let yaml = r#"
            version: 0
            backends:
              valkey:
                - tag: valkey-1
                  connection_string: "valkey://127.0.0.1:6379"
        "#;
        let config: AppConfig = serde_yaml::from_str(yaml).expect("Failed to deserialize");
        assert!(config.backends.memcache.is_empty());
        assert_eq!(config.backends.valkey.len(), 1);
    }

    #[test]
    fn environment_overrides_the_cluster_section() {
        let yaml = r#"
            version: 0
            backends: {}
            cluster:
              secret_file: /etc/yy/secret
              node_name: from-file
        "#;
        let variables = [
            ("YY_CLUSTER__ADVERTISE_IP", "10.1.2.3"),
            ("YY_CLUSTER__NODE_NAME", "worker-7"),
            (
                "YY_CLUSTER__SEEDS",
                "a.ns-a.svc.cluster.local:7946,b.ns-b.svc.cluster.local:7946",
            ),
            // Variables of other tools sharing the prefix are ignored.
            ("YY_TEST_MEMCACHE", "memcache://127.0.0.1:11211"),
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()));

        let config: AppConfig = ConfigBuilder::<DefaultState>::default()
            .add_source(config::File::from_str(yaml, FileFormat::Yaml))
            .add_source(environment().source(Some(variables.into_iter().collect())))
            .build()
            .and_then(|config| config.try_deserialize())
            .expect("Failed to load configuration");

        let cluster = config.cluster.expect("Missing cluster section");
        assert_eq!(cluster.advertise_ip, Some("10.1.2.3".parse().unwrap()));
        assert_eq!(cluster.node_name.as_deref(), Some("worker-7"));
        assert_eq!(
            cluster.seeds,
            [
                "a.ns-a.svc.cluster.local:7946",
                "b.ns-b.svc.cluster.local:7946"
            ]
        );
        assert_eq!(cluster.secret_file, PathBuf::from("/etc/yy/secret"));
    }

    #[test]
    fn cluster_section_is_optional() {
        let config: AppConfig =
            serde_yaml::from_str("version: 0\nbackends: {}").expect("Failed to deserialize");
        assert!(config.cluster.is_none());
    }

    #[test]
    #[cfg(feature = "memcache")]
    fn app_config_reads_cache_section() {
        let yaml = r#"
            version: 0
            cache:
              lease_duration_secs: 10
            backends:
              memcache: []
        "#;
        let config: AppConfig = serde_yaml::from_str(yaml).expect("Failed to deserialize");
        assert_eq!(config.cache.lease_duration(), Duration::from_secs(10));
        assert!(config.backends.memcache.is_empty());
    }
}
