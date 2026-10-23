use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use url::Url;

/// The default expiration time for objects stored in S3.
pub const DEFAULT_EXPIRATION: Duration = Duration::from_secs(24 * 3600);

/// The configuration of Amazon S3 or an S3-compatible store such as MinIO,
/// Ceph, SeaweedFS or Cloudflare R2.
///
/// Without key files, credentials are taken from the environment: the
/// `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` variables, EKS service
/// account roles (`AWS_WEB_IDENTITY_TOKEN_FILE`), or else the instance
/// metadata service.
///
/// S3 has no per-object expiry. The expiry is recorded with each object and
/// enforced when it is fetched; deleting old objects is left to a lifecycle
/// rule on the bucket.
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct S3BackendConfig {
    /// A tag to identify the backend.
    pub tag: String,
    /// The bucket to store files in. It must exist.
    pub bucket: String,
    /// A path prefix for all objects, e.g. `yeet-yoink`. Defaults to the
    /// bucket root.
    #[serde(default)]
    pub prefix: Option<String>,
    /// The region of the bucket, e.g. `eu-central-1`. Defaults to the
    /// `AWS_REGION` environment variable, or `us-east-1`.
    #[serde(default)]
    pub region: Option<String>,
    /// The endpoint of an S3-compatible store, e.g. `https://s3.example.com`.
    /// Defaults to Amazon S3.
    #[serde(default)]
    pub endpoint: Option<Url>,
    /// Whether the bucket is addressed as part of the host name
    /// (`bucket.endpoint`) rather than the path (`endpoint/bucket`). Defaults
    /// to `true` for Amazon S3 and to `false` with a custom endpoint, which
    /// is what most S3-compatible stores expect. With a custom endpoint and
    /// this enabled, the endpoint must include the bucket.
    #[serde(default)]
    pub virtual_hosted_style: Option<bool>,
    /// A file holding the access key ID, e.g. a mounted Kubernetes Secret.
    /// Requires `secret_access_key_file`.
    #[serde(default)]
    pub access_key_id_file: Option<PathBuf>,
    /// A file holding the secret access key. Requires `access_key_id_file`.
    #[serde(default)]
    pub secret_access_key_file: Option<PathBuf>,
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

impl S3BackendConfig {
    /// Whether the bucket is addressed as part of the host name.
    pub fn virtual_hosted_style(&self) -> bool {
        self.virtual_hosted_style.unwrap_or(self.endpoint.is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_minimal_s3_config() {
        let config: S3BackendConfig = serde_yaml::from_str("tag: s3-1\nbucket: files")
            .expect("Failed to deserialize S3 config");
        assert_eq!(config.tag, "s3-1");
        assert_eq!(config.bucket, "files");
        assert_eq!(config.prefix, None);
        assert_eq!(config.region, None);
        assert_eq!(config.endpoint, None);
        assert_eq!(config.access_key_id_file, None);
        assert_eq!(config.secret_access_key_file, None);
        assert_eq!(config.expiration_sec, None);
        assert_eq!(config.max_file_size_bytes, None);
        assert_eq!(config.priority, 0);
        // Amazon S3 addresses buckets by host name.
        assert!(config.virtual_hosted_style());
    }

    #[test]
    fn deserialize_full_s3_config() {
        let yaml = r#"
            tag: s3-1
            bucket: files
            prefix: yeet-yoink
            region: eu-central-1
            endpoint: "https://s3.example.com"
            access_key_id_file: /etc/yy/s3/access-key-id
            secret_access_key_file: /etc/yy/s3/secret-access-key
            expiration_sec: 600
            max_file_size_bytes: 1048576
            priority: -1
        "#;
        let config: S3BackendConfig =
            serde_yaml::from_str(yaml).expect("Failed to deserialize S3 config");

        assert_eq!(config.prefix.as_deref(), Some("yeet-yoink"));
        assert_eq!(config.region.as_deref(), Some("eu-central-1"));
        assert_eq!(
            config.endpoint.as_ref().map(Url::as_str),
            Some("https://s3.example.com/")
        );
        assert_eq!(
            config.access_key_id_file,
            Some(PathBuf::from("/etc/yy/s3/access-key-id"))
        );
        assert_eq!(
            config.secret_access_key_file,
            Some(PathBuf::from("/etc/yy/s3/secret-access-key"))
        );
        assert_eq!(config.expiration_sec, Some(600));
        assert_eq!(config.max_file_size_bytes, Some(1024 * 1024));
        assert_eq!(config.priority, -1);
        // Most S3-compatible stores address buckets by path.
        assert!(!config.virtual_hosted_style());
    }

    #[test]
    fn addressing_style_can_be_chosen() {
        let config: S3BackendConfig = serde_yaml::from_str(
            "tag: s3\nbucket: b\nendpoint: \"https://b.s3.example.com\"\nvirtual_hosted_style: true",
        )
        .expect("Failed to deserialize S3 config");
        assert!(config.virtual_hosted_style());
    }

    #[test]
    fn bucket_is_required() {
        let result: Result<S3BackendConfig, _> = serde_yaml::from_str("tag: s3-1");
        assert!(result.is_err());
    }
}
