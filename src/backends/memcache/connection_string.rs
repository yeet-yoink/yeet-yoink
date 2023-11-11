use memcache::Url;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::str::FromStr;

/// A Memcached connection string.
#[derive(Debug, Default)]
pub struct MemcacheConnectionString(String);

impl MemcacheConnectionString {
    fn new<S: AsRef<str>>(url: S) -> Result<Self, MemcacheConnectionStringError> {
        let parsed_url = Url::parse(url.as_ref());
        match parsed_url {
            Ok(url) => {
                if url.scheme() != "memcache" {
                    Err(MemcacheConnectionStringError::InvalidFormat)
                } else {
                    Ok(MemcacheConnectionString(url.to_string()))
                }
            }
            Err(_) => Err(MemcacheConnectionStringError::InvalidFormat),
        }
    }

    fn into_inner(self) -> String {
        self.0
    }

    fn get_urls(self) -> Vec<String> {
        if self.0.is_empty() {
            Vec::default()
        } else {
            vec![self.0]
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MemcacheConnectionStringError {
    #[error("Invalid connection string format")]
    InvalidFormat,
}

impl FromStr for MemcacheConnectionString {
    type Err = MemcacheConnectionStringError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        MemcacheConnectionString::new(s)
    }
}

impl Serialize for MemcacheConnectionString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MemcacheConnectionString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;
        MemcacheConnectionString::new(s).map_err(de::Error::custom)
    }
}

impl memcache::Connectable for MemcacheConnectionString {
    fn get_urls(self) -> Vec<String> {
        self.get_urls()
    }
}

impl r2d2_memcache::memcache::Connectable for MemcacheConnectionString {
    fn get_urls(self) -> Vec<String> {
        self.get_urls()
    }
}

impl PartialEq<&str> for MemcacheConnectionString {
    fn eq(&self, other: &&str) -> bool {
        self.0.eq(other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_string_parse_works() {
        let valid_conn_str = "memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true";
        let invalid_conn_str = "invalid_url";

        let valid_result: Result<MemcacheConnectionString, _> = valid_conn_str.parse();
        let invalid_result: Result<MemcacheConnectionString, _> = invalid_conn_str.parse();

        match valid_result {
            Ok(conn_str) => println!("Valid connection string: {}", conn_str.into_inner()),
            Err(err) => println!("Error parsing connection string: {}", err),
        }

        match invalid_result {
            Ok(conn_str) => println!("Valid connection string: {}", conn_str.into_inner()),
            Err(err) => println!("Error parsing connection string: {}", err),
        }
    }

    #[test]
    fn connection_string_deserialize_works() {
        let valid_result: MemcacheConnectionString =
            serde_yaml::from_str("memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true")
                .expect("Deserialization failed");
        assert_eq!(
            valid_result,
            "memcache://127.0.0.1:12345?timeout=10&tcp_nodelay=true"
        );
    }
}
