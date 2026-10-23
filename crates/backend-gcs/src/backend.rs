use app_config::{
    AppConfig,
    gcs::{DEFAULT_EXPIRATION, GcsBackendConfig},
};
use backend_object_store::{ObjectStoreBackend, ObjectStoreSettings, client_options, retry_config};
use backend_traits::{Backend, BackendInfo, TryCreateFromConfig};
use object_store::gcp::GoogleCloudStorageBuilder;
use std::sync::Arc;
use std::time::Duration;

/// Creates storage backends keeping files in Google Cloud Storage buckets.
pub struct GcsBackend;

impl GcsBackend {
    /// Creates a backend for the configured bucket. No request is made until
    /// the backend is used.
    pub fn try_new(
        config: &GcsBackendConfig,
    ) -> Result<ObjectStoreBackend, GcsBackendConstructionError> {
        let mut builder = GoogleCloudStorageBuilder::new()
            .with_bucket_name(&config.bucket)
            // Emulators are usually served over plain HTTP.
            .with_client_options(client_options(config.emulator_url.is_some()))
            .with_retry(retry_config());

        if let Some(emulator_url) = &config.emulator_url {
            // A service account key is how object_store is pointed at a
            // different endpoint without requesting OAuth tokens.
            let key = serde_json::json!({
                "gcs_base_url": emulator_url.as_str().trim_end_matches('/'),
                "disable_oauth": true,
                "client_email": "",
                "private_key": "",
                "private_key_id": "",
            });
            builder = builder.with_service_account_key(key.to_string());
        } else if let Some(path) = &config.service_account_path {
            builder = builder.with_service_account_path(path.to_string_lossy());
        }

        let store = builder
            .build()
            .map_err(GcsBackendConstructionError::FailedToCreateClient)?;
        Ok(ObjectStoreBackend::new(
            Self::settings(config),
            Arc::new(store),
        ))
    }

    /// The settings shared by all object store backends.
    pub fn settings(config: &GcsBackendConfig) -> ObjectStoreSettings {
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

impl BackendInfo for GcsBackend {
    fn backend_name() -> &'static str {
        "Google Cloud Storage"
    }

    fn backend_version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
}

impl TryCreateFromConfig for GcsBackend {
    type Error = GcsBackendConstructionError;

    fn try_from_config(config: &AppConfig) -> Result<Vec<Backend>, Self::Error> {
        config
            .backends
            .gcs
            .iter()
            .map(|config| GcsBackend::try_new(config).map(Backend::wrap))
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GcsBackendConstructionError {
    #[error("Failed to create Cloud Storage client: {0}")]
    FailedToCreateClient(object_store::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> GcsBackendConfig {
        GcsBackendConfig {
            tag: "gcs".into(),
            bucket: "bucket".into(),
            ..GcsBackendConfig::default()
        }
    }

    #[test]
    fn reports_backend_info() {
        assert_eq!(GcsBackend::backend_name(), "Google Cloud Storage");
        assert_eq!(GcsBackend::backend_version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn defaults_apply_without_configuration() {
        let settings = GcsBackend::settings(&config());
        assert_eq!(settings.tag, "gcs");
        assert_eq!(settings.prefix, None);
        assert_eq!(settings.expiration, Some(DEFAULT_EXPIRATION));
        assert_eq!(settings.max_file_size, None);
        assert_eq!(settings.priority, 0);
    }

    #[test]
    fn configured_settings_are_passed_on() {
        let settings = GcsBackend::settings(&GcsBackendConfig {
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
        app_config.backends.gcs = vec![
            GcsBackendConfig {
                tag: "first".into(),
                ..config()
            },
            GcsBackendConfig {
                tag: "second".into(),
                emulator_url: Some("http://127.0.0.1:4443".parse().expect("Invalid URL")),
                ..config()
            },
        ];

        let backends = GcsBackend::try_from_config(&app_config).expect("Failed to create");

        let tags: Vec<_> = backends.iter().map(|backend| backend.tag()).collect();
        assert_eq!(tags, ["first", "second"]);
    }

    #[test]
    fn missing_service_account_file_is_reported() {
        let config = GcsBackendConfig {
            service_account_path: Some("/nonexistent/sa.json".into()),
            ..config()
        };
        assert!(matches!(
            GcsBackend::try_new(&config),
            Err(GcsBackendConstructionError::FailedToCreateClient(_))
        ));
    }
}
