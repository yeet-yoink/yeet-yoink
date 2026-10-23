use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use url::Url;

/// The default expiration time for objects stored in Google Cloud Storage.
pub const DEFAULT_EXPIRATION: Duration = Duration::from_secs(24 * 3600);

/// The default largest file size accepted by a Google Cloud Storage backend, in bytes.
///
/// Uploads are buffered in memory, so this stays well below what Cloud Storage
/// itself accepts. Fetches are streamed and need no such limit.
pub const DEFAULT_MAX_FILE_SIZE_BYTES: u64 = 64 * 1024 * 1024;

/// The Google Cloud Storage-specific configuration.
///
/// Cloud Storage has no per-object expiry. The expiry is recorded with each
/// object and enforced when it is fetched; deleting old objects is left to a
/// lifecycle rule on the bucket.
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct GcsBackendConfig {
    /// A tag to identify the backend.
    pub tag: String,
    /// The name of the bucket to store files in.
    pub bucket: String,
    /// A path prefix for all objects, e.g. `yeet-yoink`. Defaults to the bucket root.
    #[serde(default)]
    pub prefix: Option<String>,
    /// The path to a service account key file. Without it, Application Default
    /// Credentials are used.
    #[serde(default)]
    pub service_account_path: Option<PathBuf>,
    /// The URL of a Cloud Storage emulator. Requests to it are not authenticated.
    ///
    /// ### Example
    ///
    /// ```text
    /// http://127.0.0.1:4443
    /// ```
    #[serde(default)]
    pub emulator_url: Option<Url>,
    /// The number of seconds after which a stored file is considered expired.
    /// Use `0` to keep it indefinitely. Defaults to [`DEFAULT_EXPIRATION`].
    pub expiration_sec: Option<u32>,
    /// The largest file size, in bytes, to store in this backend. Larger files
    /// are not distributed to it. Use `0` to disable the limit. Defaults to
    /// [`DEFAULT_MAX_FILE_SIZE_BYTES`].
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
    fn deserialize_minimal_gcs_config() {
        let yaml = r#"
            tag: gcs-1
            bucket: my-bucket
        "#;

        let config: GcsBackendConfig =
            serde_yaml::from_str(yaml).expect("Failed to deserialize GCS config");
        assert_eq!(config.tag, "gcs-1");
        assert_eq!(config.bucket, "my-bucket");
        assert_eq!(config.prefix, None);
        assert_eq!(config.service_account_path, None);
        assert_eq!(config.emulator_url, None);
        assert_eq!(config.expiration_sec, None);
        assert_eq!(config.max_file_size_bytes, None);
        assert_eq!(config.priority, 0);
    }

    #[test]
    fn deserialize_full_gcs_config() {
        let yaml = r#"
            tag: gcs-1
            bucket: my-bucket
            prefix: yeet-yoink
            service_account_path: /etc/yy/sa.json
            emulator_url: "http://127.0.0.1:4443"
            expiration_sec: 600
            max_file_size_bytes: 1048576
            priority: -1
        "#;

        let config: GcsBackendConfig =
            serde_yaml::from_str(yaml).expect("Failed to deserialize GCS config");
        assert_eq!(config.prefix.as_deref(), Some("yeet-yoink"));
        assert_eq!(
            config.service_account_path,
            Some(PathBuf::from("/etc/yy/sa.json"))
        );
        assert_eq!(
            config.emulator_url.as_ref().map(Url::as_str),
            Some("http://127.0.0.1:4443/")
        );
        assert_eq!(config.expiration_sec, Some(600));
        assert_eq!(config.max_file_size_bytes, Some(1024 * 1024));
        assert_eq!(config.priority, -1);
    }

    #[test]
    fn deserialize_rejects_invalid_emulator_url() {
        let yaml = r#"
            tag: gcs-1
            bucket: my-bucket
            emulator_url: "not a url"
        "#;
        let result: Result<GcsBackendConfig, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err());
    }
}
