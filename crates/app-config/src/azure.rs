use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use url::Url;

/// The default expiration time for blobs stored in Azure Blob Storage.
pub const DEFAULT_EXPIRATION: Duration = Duration::from_secs(24 * 3600);

/// The Azure Blob Storage-specific configuration.
///
/// Without an access key or SAS token, credentials are taken from the
/// environment: a service principal (`AZURE_CLIENT_ID`, `AZURE_TENANT_ID`,
/// `AZURE_CLIENT_SECRET`), workload identity as set up by AKS
/// (`AZURE_FEDERATED_TOKEN_FILE`), or else the managed identity.
///
/// Blob Storage has no per-blob expiry. The expiry is recorded with each blob
/// and enforced when it is fetched; deleting old blobs is left to a lifecycle
/// management policy on the storage account.
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct AzureBackendConfig {
    /// A tag to identify the backend.
    pub tag: String,
    /// The storage account. Required unless an emulator is used, which
    /// defaults to Azurite's `devstoreaccount1`.
    #[serde(default)]
    pub account: Option<String>,
    /// The blob container to store files in. It must exist.
    pub container: String,
    /// A path prefix for all blobs, e.g. `yeet-yoink`. Defaults to the
    /// container root.
    #[serde(default)]
    pub prefix: Option<String>,
    /// A file holding the storage account's access key, e.g. a mounted
    /// Kubernetes Secret.
    #[serde(default)]
    pub access_key_file: Option<PathBuf>,
    /// A file holding a SAS token for the container.
    #[serde(default)]
    pub sas_token_file: Option<PathBuf>,
    /// The URL of an Azurite emulator, e.g. `http://127.0.0.1:10000`.
    /// Without an access key file, Azurite's well-known development key is
    /// used.
    #[serde(default)]
    pub emulator_url: Option<Url>,
    /// The number of seconds after which a stored file is considered expired.
    /// Use `0` to keep it indefinitely. Defaults to [`DEFAULT_EXPIRATION`].
    pub expiration_sec: Option<u32>,
    /// The largest file size, in bytes, to store in this backend. Larger files
    /// are not distributed to it. Files are streamed in both directions, so
    /// there is no limit by default; `0` also means no limit.
    pub max_file_size_bytes: Option<u64>,
    /// The priority of this backend. Backends with a higher priority are fetched
    /// from first and have their distributions started first. Defaults to `0`.
    #[serde(default)]
    pub priority: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_minimal_azure_config() {
        let yaml = r#"
            tag: azure-1
            account: mystorageaccount
            container: files
        "#;

        let config: AzureBackendConfig =
            serde_yaml::from_str(yaml).expect("Failed to deserialize Azure config");
        assert_eq!(config.tag, "azure-1");
        assert_eq!(config.account.as_deref(), Some("mystorageaccount"));
        assert_eq!(config.container, "files");
        assert_eq!(config.prefix, None);
        assert_eq!(config.access_key_file, None);
        assert_eq!(config.sas_token_file, None);
        assert_eq!(config.emulator_url, None);
        assert_eq!(config.expiration_sec, None);
        assert_eq!(config.max_file_size_bytes, None);
        assert_eq!(config.priority, 0);
    }

    #[test]
    fn deserialize_full_azure_config() {
        let yaml = r#"
            tag: azure-1
            account: mystorageaccount
            container: files
            prefix: yeet-yoink
            access_key_file: /etc/yy/azure/key
            sas_token_file: /etc/yy/azure/sas
            emulator_url: "http://127.0.0.1:10000"
            expiration_sec: 600
            max_file_size_bytes: 1048576
            priority: -1
        "#;

        let config: AzureBackendConfig =
            serde_yaml::from_str(yaml).expect("Failed to deserialize Azure config");
        assert_eq!(config.prefix.as_deref(), Some("yeet-yoink"));
        assert_eq!(
            config.access_key_file,
            Some(PathBuf::from("/etc/yy/azure/key"))
        );
        assert_eq!(
            config.sas_token_file,
            Some(PathBuf::from("/etc/yy/azure/sas"))
        );
        assert_eq!(
            config.emulator_url.as_ref().map(Url::as_str),
            Some("http://127.0.0.1:10000/")
        );
        assert_eq!(config.expiration_sec, Some(600));
        assert_eq!(config.max_file_size_bytes, Some(1024 * 1024));
        assert_eq!(config.priority, -1);
    }

    #[test]
    fn container_is_required() {
        let result: Result<AzureBackendConfig, _> =
            serde_yaml::from_str("tag: azure-1\naccount: a");
        assert!(result.is_err());
    }
}
