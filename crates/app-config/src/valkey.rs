use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::fmt::{Debug, Display, Formatter};
use std::str::FromStr;
use std::time::Duration;
use url::Url;

/// The default expiration time for Valkey entries.
pub const DEFAULT_EXPIRATION: Duration = Duration::from_secs(3600);

/// The default largest file size accepted by a Valkey backend, in bytes.
///
/// This matches Valkey's default limit for a single value of 512 MiB
/// (`proto-max-bulk-len`). Fetched files are held in memory, so a lower limit
/// may be preferable.
pub const DEFAULT_MAX_FILE_SIZE_BYTES: u64 = 512 * 1024 * 1024;

/// The Valkey-specific configuration.
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct ValkeyBackendConfig {
    /// A tag to identify the backend.
    pub tag: String,
    /// The connection string.
    ///
    /// ## Example
    /// ```text
    /// valkey://:password@127.0.0.1:6379/0
    /// ```
    pub connection_string: ValkeyConnectionString,
    /// The number of seconds after which the item is considered expired. Use `0`
    /// to keep the entry indefinitely. Defaults to [`DEFAULT_EXPIRATION`].
    ///
    /// ### Example
    ///
    /// To keep the example for 5 minutes, use a value of 300 seconds:
    ///
    /// ```text
    /// 300
    /// ```
    pub expiration_sec: Option<u32>,
    /// The largest file size, in bytes, to store in this backend. Larger files
    /// are not distributed to it. Use `0` to disable the limit. Defaults to
    /// [`DEFAULT_MAX_FILE_SIZE_BYTES`].
    ///
    /// ### Example
    ///
    /// To accept files up to 64 MiB:
    ///
    /// ```text
    /// 67108864
    /// ```
    pub max_file_size_bytes: Option<u64>,
    /// The priority of this backend. Backends with a higher priority are fetched
    /// from first and have their distributions started first. Defaults to `0`.
    ///
    /// ### Example
    ///
    /// To prefer this backend over backends with the default priority:
    ///
    /// ```text
    /// 10
    /// ```
    #[serde(default)]
    pub priority: i32,
}

/// A Valkey connection string.
///
/// The password, if any, is masked when the connection string is displayed or
/// debug-printed.
#[derive(Default, Clone, Ord, PartialOrd, Eq, PartialEq)]
pub struct ValkeyConnectionString(String);

impl ValkeyConnectionString {
    /// Creates a new [`ValkeyConnectionString`] from a string.
    fn new<S>(url: S) -> Result<Self, ValkeyConnectionStringError>
    where
        S: AsRef<str>,
    {
        match Url::parse(url.as_ref()) {
            Ok(url) if url.scheme() == "valkey" => Ok(ValkeyConnectionString(url.to_string())),
            _ => Err(ValkeyConnectionStringError::InvalidFormat),
        }
    }

    /// Gets the connection URL, including credentials.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ValkeyConnectionStringError {
    #[error("Invalid connection string format")]
    InvalidFormat,
}

impl FromStr for ValkeyConnectionString {
    type Err = ValkeyConnectionStringError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ValkeyConnectionString::new(s)
    }
}

impl Serialize for ValkeyConnectionString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ValkeyConnectionString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;
        ValkeyConnectionString::new(s).map_err(de::Error::custom)
    }
}

impl PartialEq<&str> for ValkeyConnectionString {
    fn eq(&self, other: &&str) -> bool {
        self.0.eq(other)
    }
}

impl Display for ValkeyConnectionString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match Url::parse(&self.0) {
            Ok(mut url) if url.password().is_some() => {
                // Setting a password only fails for URLs that cannot carry one.
                let _ = url.set_password(Some("***"));
                write!(f, "{url}")
            }
            _ => write!(f, "{}", self.0),
        }
    }
}

impl Debug for ValkeyConnectionString {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ValkeyConnectionString")
            .field(&self.to_string())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_valkey_config_works() {
        let yaml = r#"
            tag: valkey-1
            connection_string: "valkey://127.0.0.1:6379/0"
            expiration_sec: 500
        "#;

        let config: ValkeyBackendConfig =
            serde_yaml::from_str(yaml).expect("Failed to deserialize Valkey config");
        assert_eq!(config.tag, "valkey-1");
        assert_eq!(config.connection_string, "valkey://127.0.0.1:6379/0");
        assert_eq!(config.expiration_sec, Some(500));
        assert_eq!(config.max_file_size_bytes, None);
        assert_eq!(config.priority, 0);
    }

    #[test]
    fn deserialize_valkey_config_reads_optional_settings() {
        let yaml = r#"
            tag: valkey-1
            connection_string: "valkey://127.0.0.1:6379"
            max_file_size_bytes: 67108864
            priority: 5
        "#;

        let config: ValkeyBackendConfig =
            serde_yaml::from_str(yaml).expect("Failed to deserialize Valkey config");
        assert_eq!(config.max_file_size_bytes, Some(64 * 1024 * 1024));
        assert_eq!(config.priority, 5);
    }

    #[test]
    fn connection_string_rejects_other_schemes() {
        for invalid in [
            "redis://127.0.0.1:6379",
            "memcache://127.0.0.1",
            "invalid_url",
        ] {
            let result: Result<ValkeyConnectionString, _> = invalid.parse();
            assert!(
                matches!(result, Err(ValkeyConnectionStringError::InvalidFormat)),
                "{invalid} was accepted"
            );
        }
    }

    #[test]
    fn deserialize_rejects_invalid_connection_string() {
        let result: Result<ValkeyConnectionString, _> = serde_yaml::from_str("redis://localhost");
        assert!(result.is_err());
    }

    #[test]
    fn password_is_masked_when_displayed() {
        let conn_str: ValkeyConnectionString = "valkey://user:secret@127.0.0.1:6379/0"
            .parse()
            .expect("Failed to parse");

        assert_eq!(conn_str.as_str(), "valkey://user:secret@127.0.0.1:6379/0");
        assert_eq!(conn_str.to_string(), "valkey://user:***@127.0.0.1:6379/0");
        assert!(!format!("{conn_str:?}").contains("secret"));
    }

    #[test]
    fn connection_string_without_password_is_displayed_unchanged() {
        let conn_str: ValkeyConnectionString =
            "valkey://127.0.0.1:6379".parse().expect("Failed to parse");
        assert_eq!(conn_str.to_string(), "valkey://127.0.0.1:6379");
    }
}
