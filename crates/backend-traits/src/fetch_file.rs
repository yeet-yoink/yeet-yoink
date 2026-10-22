use async_trait::async_trait;
use bytes::Bytes;
use file_distribution::FileHashes;
use file_distribution::hash::{HashMd5, HashSha256};
use shortguid::ShortGuid;
use std::error::Error;

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
}

impl FetchedFile {
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
        }
    }

    #[test]
    fn matching_content_is_intact() {
        assert!(fetched(b"abc", b"abc").is_intact());
    }

    #[test]
    fn altered_content_is_not_intact() {
        assert!(!fetched(b"abd", b"abc").is_intact());
    }
}
