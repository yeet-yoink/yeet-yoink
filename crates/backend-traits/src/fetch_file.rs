use async_trait::async_trait;
use bytes::Bytes;
use file_distribution::FileHashes;
use file_distribution::hash::{HashMd5, HashSha256};
use file_distribution::protobuf::ItemMetadata;
use shortguid::ShortGuid;
use std::error::Error;
use std::time::SystemTime;

#[async_trait]
pub trait FetchFile: Send + Sync {
    /// Fetches a file that was previously distributed to this backend.
    ///
    /// Returns `Ok(None)` if the backend does not hold the file (anymore).
    async fn fetch_file(&self, id: ShortGuid) -> Result<Option<FetchedFile>, FetchError>;
}

/// A file fetched from a backend, together with its metadata.
///
/// The content is held in memory; backends are expected to store files of
/// limited size only.
#[derive(Clone)]
pub struct FetchedFile {
    /// The original file name, if one was specified on upload.
    pub file_name: Option<String>,
    /// The content type, if one was specified on upload.
    pub content_type: Option<String>,
    /// The hashes recorded when the file was uploaded.
    pub hashes: FileHashes,
    /// The file content.
    pub data: Bytes,
    /// When the backend's copy expires, or `None` if it is kept indefinitely.
    ///
    /// Copies must not be served or cached beyond this point.
    pub expires_at: Option<SystemTime>,
}

impl FetchedFile {
    /// Rebuilds a file from the serialized metadata and the content a backend
    /// stored on distribution.
    ///
    /// The metadata comes from the backend and is validated, not trusted.
    pub fn from_stored(metadata: &[u8], data: Bytes) -> Result<Self, FetchError> {
        let metadata = ItemMetadata::deserialize_from_proto(metadata)
            .map_err(|e| FetchError::InvalidMetadata(e.to_string()))?;
        let hashes = metadata
            .file_hashes()
            .ok_or_else(|| FetchError::InvalidMetadata("missing or malformed hashes".into()))?;
        let expires_at = metadata
            .expires_at()
            .map_err(|e| FetchError::InvalidMetadata(e.to_string()))?;
        Ok(Self {
            expires_at,
            file_name: metadata.file_name,
            content_type: metadata.content_type,
            hashes,
            data,
        })
    }

    /// Checks whether the backend's copy has expired.
    pub fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| expires_at <= SystemTime::now())
    }

    /// Checks the content against the recorded hashes.
    pub fn is_intact(&self) -> bool {
        let mut md5 = HashMd5::new();
        md5.update(&self.data);
        let mut sha256 = HashSha256::new();
        sha256.update(&self.data);
        md5.finalize() == self.hashes.md5 && sha256.finalize() == self.hashes.sha256
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error(transparent)]
    BackendSpecific(Box<dyn Error + Send + Sync>),
    #[error("Invalid metadata: {0}")]
    InvalidMetadata(String),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fetched(data: &'static [u8], hashed: &[u8]) -> FetchedFile {
        let mut md5 = HashMd5::new();
        md5.update(hashed);
        let mut sha256 = HashSha256::new();
        sha256.update(hashed);
        FetchedFile {
            file_name: None,
            content_type: None,
            hashes: FileHashes::new(md5.finalize(), sha256.finalize()),
            data: Bytes::from_static(data),
            expires_at: None,
        }
    }

    fn stored_metadata(metadata: ItemMetadata) -> Bytes {
        metadata.serialize_to_proto().expect("Failed to serialize")
    }

    #[test]
    fn stored_file_is_rebuilt_from_metadata() {
        use file_distribution::protobuf::Hashes;
        use std::time::{Duration, UNIX_EPOCH};

        let expected = fetched(b"abc", b"abc");
        let metadata = stored_metadata(ItemMetadata {
            file_name: Some("a.txt".into()),
            content_type: Some("text/plain".into()),
            hashes: Some(Hashes {
                md5: expected.hashes.md5.to_vec(),
                sha256: expected.hashes.sha256.to_vec(),
            }),
            expires_at_unix_ms: Some(1_000),
            ..ItemMetadata::default()
        });

        let file = FetchedFile::from_stored(&metadata, Bytes::from_static(b"abc"))
            .expect("Failed to rebuild file");

        assert_eq!(file.file_name.as_deref(), Some("a.txt"));
        assert_eq!(file.content_type.as_deref(), Some("text/plain"));
        assert_eq!(file.expires_at, Some(UNIX_EPOCH + Duration::from_secs(1)));
        assert!(file.is_intact());
    }

    #[test]
    fn stored_file_without_hashes_is_rejected() {
        let metadata = stored_metadata(ItemMetadata::default());
        let result = FetchedFile::from_stored(&metadata, Bytes::from_static(b"abc"));
        assert!(matches!(result, Err(FetchError::InvalidMetadata(_))));
    }

    #[test]
    fn undecodable_metadata_is_rejected() {
        let result = FetchedFile::from_stored(&[0xff, 0xff, 0xff], Bytes::from_static(b"abc"));
        assert!(matches!(result, Err(FetchError::InvalidMetadata(_))));
    }

    #[test]
    fn matching_content_is_intact() {
        assert!(fetched(b"abc", b"abc").is_intact());
    }

    #[test]
    fn expiry_is_checked_against_the_current_time() {
        use std::time::Duration;

        let mut file = fetched(b"abc", b"abc");
        assert!(!file.is_expired());

        file.expires_at = Some(SystemTime::now() + Duration::from_secs(60));
        assert!(!file.is_expired());

        file.expires_at = Some(SystemTime::now() - Duration::from_secs(1));
        assert!(file.is_expired());
    }

    #[test]
    fn altered_content_is_not_intact() {
        assert!(!fetched(b"abd", b"abc").is_intact());
    }
}
