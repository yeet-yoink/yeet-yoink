use app_config::{
    AppConfig,
    s3::{DEFAULT_EXPIRATION, S3BackendConfig},
};
use backend_object_store::{ObjectStoreBackend, ObjectStoreSettings, client_options, retry_config};
use backend_traits::{Backend, BackendInfo, TryCreateFromConfig};
use object_store::aws::AmazonS3Builder;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Creates storage backends keeping files in Amazon S3 or S3-compatible buckets.
pub struct S3Backend;

impl S3Backend {
    /// Creates a backend for the configured bucket. No request is made until
    /// the backend is used.
    pub fn try_new(
        config: &S3BackendConfig,
    ) -> Result<ObjectStoreBackend, S3BackendConstructionError> {
        // Credentials and the region may come from the environment, e.g. an
        // EKS service account role.
        let allow_http = config
            .endpoint
            .as_ref()
            .is_some_and(|endpoint| endpoint.scheme() == "http");
        let mut builder = AmazonS3Builder::from_env()
            .with_bucket_name(&config.bucket)
            .with_virtual_hosted_style_request(config.virtual_hosted_style())
            .with_client_options(client_options(allow_http))
            .with_retry(retry_config());

        if let Some(region) = &config.region {
            builder = builder.with_region(region);
        }
        if let Some(endpoint) = &config.endpoint {
            builder = builder.with_endpoint(endpoint.as_str().trim_end_matches('/'));
        }

        match (&config.access_key_id_file, &config.secret_access_key_file) {
            (Some(id_file), Some(secret_file)) => {
                builder = builder
                    .with_access_key_id(read_secret(id_file)?)
                    .with_secret_access_key(read_secret(secret_file)?);
            }
            (None, None) => {}
            _ => return Err(S3BackendConstructionError::IncompleteKey),
        }

        let store = builder
            .build()
            .map_err(S3BackendConstructionError::FailedToCreateClient)?;
        Ok(ObjectStoreBackend::new(
            Self::settings(config),
            Arc::new(store),
        ))
    }

    /// The settings shared by all object store backends.
    pub fn settings(config: &S3BackendConfig) -> ObjectStoreSettings {
        ObjectStoreSettings {
            tag: config.tag.clone(),
            prefix: config.prefix.clone(),
            expiration: Some(
                config
                    .expiration_sec
                    .map_or(DEFAULT_EXPIRATION, |secs| Duration::from_secs(secs.into())),
            ),
            // There is no limit by default.
            max_file_size: config.max_file_size_bytes,
            priority: config.priority,
        }
    }
}

/// Reads a credential from a file.
fn read_secret(path: &Path) -> Result<String, S3BackendConstructionError> {
    backend_object_store::read_secret(path).map_err(|source| {
        S3BackendConstructionError::ReadSecret {
            path: path.to_path_buf(),
            source,
        }
    })
}

impl BackendInfo for S3Backend {
    fn backend_name() -> &'static str {
        "S3"
    }

    fn backend_version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
}

impl TryCreateFromConfig for S3Backend {
    type Error = S3BackendConstructionError;

    fn try_from_config(config: &AppConfig) -> Result<Vec<Backend>, Self::Error> {
        config
            .backends
            .s3
            .iter()
            .map(|config| S3Backend::try_new(config).map(Backend::wrap))
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum S3BackendConstructionError {
    #[error("access_key_id_file and secret_access_key_file must be set together")]
    IncompleteKey,
    #[error("Failed to read the secret {path:?}: {source}")]
    ReadSecret {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Failed to create S3 client: {0}")]
    FailedToCreateClient(object_store::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> S3BackendConfig {
        S3BackendConfig {
            tag: "s3".into(),
            bucket: "files".into(),
            region: Some("eu-central-1".into()),
            ..S3BackendConfig::default()
        }
    }

    #[test]
    fn reports_backend_info() {
        assert_eq!(S3Backend::backend_name(), "S3");
        assert_eq!(S3Backend::backend_version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn defaults_apply_without_configuration() {
        let settings = S3Backend::settings(&config());
        assert_eq!(settings.tag, "s3");
        assert_eq!(settings.prefix, None);
        assert_eq!(settings.expiration, Some(DEFAULT_EXPIRATION));
        assert_eq!(settings.max_file_size, None);
        assert_eq!(settings.priority, 0);
    }

    #[test]
    fn configured_settings_are_passed_on() {
        let settings = S3Backend::settings(&S3BackendConfig {
            prefix: Some("files".into()),
            expiration_sec: Some(0),
            max_file_size_bytes: Some(10),
            priority: 3,
            ..config()
        });
        assert_eq!(settings.prefix.as_deref(), Some("files"));
        assert_eq!(settings.expiration, Some(Duration::ZERO));
        assert_eq!(settings.max_file_size, Some(10));
        assert_eq!(settings.priority, 3);
    }

    #[test]
    fn creates_one_backend_per_config_without_connecting() {
        let mut app_config = AppConfig::default();
        app_config.backends.s3 = vec![
            S3BackendConfig {
                tag: "aws".into(),
                ..config()
            },
            S3BackendConfig {
                tag: "compatible".into(),
                endpoint: Some("http://127.0.0.1:8333".parse().expect("Invalid URL")),
                ..config()
            },
        ];

        let backends = S3Backend::try_from_config(&app_config).expect("Failed to create");

        let tags: Vec<_> = backends.iter().map(|backend| backend.tag()).collect();
        assert_eq!(tags, ["aws", "compatible"]);
    }

    #[test]
    fn key_files_must_come_in_pairs() {
        for config in [
            S3BackendConfig {
                access_key_id_file: Some("/etc/id".into()),
                ..config()
            },
            S3BackendConfig {
                secret_access_key_file: Some("/etc/secret".into()),
                ..config()
            },
        ] {
            assert!(matches!(
                S3Backend::try_new(&config),
                Err(S3BackendConstructionError::IncompleteKey)
            ));
        }
    }

    #[test]
    fn missing_key_files_are_reported() {
        let config = S3BackendConfig {
            access_key_id_file: Some("/nonexistent/id".into()),
            secret_access_key_file: Some("/nonexistent/secret".into()),
            ..config()
        };
        assert!(matches!(
            S3Backend::try_new(&config),
            Err(S3BackendConstructionError::ReadSecret { .. })
        ));
    }
}
