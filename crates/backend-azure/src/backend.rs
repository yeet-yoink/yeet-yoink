use app_config::{
    AppConfig,
    azure::{AzureBackendConfig, DEFAULT_EXPIRATION},
};
use backend_object_store::{ObjectStoreBackend, ObjectStoreSettings, client_options, retry_config};
use backend_traits::{Backend, BackendInfo, TryCreateFromConfig};
use object_store::azure::{AzureConfigKey, MicrosoftAzureBuilder};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The account of the Azurite emulator.
const EMULATOR_ACCOUNT: &str = "devstoreaccount1";

/// The well-known, public development key of the Azurite emulator.
const EMULATOR_ACCOUNT_KEY: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

/// Creates storage backends keeping files in Azure Blob Storage containers.
pub struct AzureBackend;

impl AzureBackend {
    /// Creates a backend for the configured container. No request is made
    /// until the backend is used.
    pub fn try_new(
        config: &AzureBackendConfig,
    ) -> Result<ObjectStoreBackend, AzureBackendConstructionError> {
        let emulator = config.emulator_url.is_some();
        // Outside the emulator, credentials may come from the environment,
        // e.g. workload identity on AKS.
        let builder = if emulator {
            MicrosoftAzureBuilder::new()
        } else {
            MicrosoftAzureBuilder::from_env()
        };
        let account = match (&config.account, emulator) {
            (Some(account), _) => account.clone(),
            (None, true) => EMULATOR_ACCOUNT.to_string(),
            (None, false) => return Err(AzureBackendConstructionError::MissingAccount),
        };

        let mut builder = builder
            .with_account(&account)
            .with_container_name(&config.container)
            // Emulators are usually served over plain HTTP.
            .with_client_options(client_options(emulator))
            .with_retry(retry_config());

        let mut access_key = config
            .access_key_file
            .as_deref()
            .map(read_secret)
            .transpose()?;

        if let Some(emulator_url) = &config.emulator_url {
            // Azurite addresses accounts by path rather than by host name.
            let endpoint = format!("{}/{account}", emulator_url.as_str().trim_end_matches('/'));
            builder = builder.with_endpoint(endpoint);
            access_key.get_or_insert_with(|| EMULATOR_ACCOUNT_KEY.to_string());
        }
        if let Some(access_key) = access_key {
            builder = builder.with_access_key(access_key);
        }
        if let Some(path) = &config.sas_token_file {
            builder = builder.with_config(AzureConfigKey::SasKey, read_secret(path)?);
        }

        let store = builder
            .build()
            .map_err(AzureBackendConstructionError::FailedToCreateClient)?;
        Ok(ObjectStoreBackend::new(
            Self::settings(config),
            Arc::new(store),
        ))
    }

    /// The settings shared by all object store backends.
    pub fn settings(config: &AzureBackendConfig) -> ObjectStoreSettings {
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

/// Reads a secret from a file, ignoring surrounding whitespace such as the
/// trailing line break editors and `echo` add.
fn read_secret(path: &Path) -> Result<String, AzureBackendConstructionError> {
    std::fs::read_to_string(path)
        .map(|secret| secret.trim().to_string())
        .map_err(|source| AzureBackendConstructionError::ReadSecret {
            path: path.to_path_buf(),
            source,
        })
}

impl BackendInfo for AzureBackend {
    fn backend_name() -> &'static str {
        "Azure Blob Storage"
    }

    fn backend_version() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }
}

impl TryCreateFromConfig for AzureBackend {
    type Error = AzureBackendConstructionError;

    fn try_from_config(config: &AppConfig) -> Result<Vec<Backend>, Self::Error> {
        config
            .backends
            .azure
            .iter()
            .map(|config| AzureBackend::try_new(config).map(Backend::wrap))
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AzureBackendConstructionError {
    #[error("An Azure storage account is required unless an emulator is used")]
    MissingAccount,
    #[error("Failed to read the secret {path:?}: {source}")]
    ReadSecret {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Failed to create Azure Blob Storage client: {0}")]
    FailedToCreateClient(object_store::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AzureBackendConfig {
        AzureBackendConfig {
            tag: "azure".into(),
            account: Some("account".into()),
            container: "files".into(),
            ..AzureBackendConfig::default()
        }
    }

    #[test]
    fn reports_backend_info() {
        assert_eq!(AzureBackend::backend_name(), "Azure Blob Storage");
        assert_eq!(AzureBackend::backend_version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn defaults_apply_without_configuration() {
        let settings = AzureBackend::settings(&config());
        assert_eq!(settings.tag, "azure");
        assert_eq!(settings.prefix, None);
        assert_eq!(settings.expiration, Some(DEFAULT_EXPIRATION));
        assert_eq!(settings.max_file_size, None);
        assert_eq!(settings.priority, 0);
    }

    #[test]
    fn configured_settings_are_passed_on() {
        let settings = AzureBackend::settings(&AzureBackendConfig {
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
        app_config.backends.azure = vec![
            AzureBackendConfig {
                tag: "first".into(),
                ..config()
            },
            AzureBackendConfig {
                tag: "second".into(),
                account: None,
                emulator_url: Some("http://127.0.0.1:10000".parse().expect("Invalid URL")),
                ..config()
            },
        ];

        let backends = AzureBackend::try_from_config(&app_config).expect("Failed to create");

        let tags: Vec<_> = backends.iter().map(|backend| backend.tag()).collect();
        assert_eq!(tags, ["first", "second"]);
    }

    #[test]
    fn account_is_required_outside_the_emulator() {
        let config = AzureBackendConfig {
            account: None,
            ..config()
        };
        assert!(matches!(
            AzureBackend::try_new(&config),
            Err(AzureBackendConstructionError::MissingAccount)
        ));
    }

    #[test]
    fn missing_secret_files_are_reported() {
        for config in [
            AzureBackendConfig {
                access_key_file: Some("/nonexistent/key".into()),
                ..config()
            },
            AzureBackendConfig {
                sas_token_file: Some("/nonexistent/sas".into()),
                ..config()
            },
        ] {
            assert!(matches!(
                AzureBackend::try_new(&config),
                Err(AzureBackendConstructionError::ReadSecret { .. })
            ));
        }
    }

    #[test]
    fn secrets_are_trimmed() {
        let dir = std::env::temp_dir().join(format!("yy-azure-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("key");
        std::fs::write(&file, "c2VjcmV0\n").unwrap();

        assert_eq!(read_secret(&file).unwrap(), "c2VjcmV0");
        std::fs::remove_dir_all(dir).ok();
    }
}
