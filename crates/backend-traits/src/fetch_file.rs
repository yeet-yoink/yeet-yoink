use async_trait::async_trait;
use bytes::Bytes;
use file_distribution::FileHashes;
use file_distribution::protobuf::ItemMetadata;
use shortguid::ShortGuid;
use std::error::Error;
use std::fmt::{Debug, Formatter};
use std::io::Cursor;
use std::time::SystemTime;
use tokio::io::{AsyncRead, AsyncReadExt};

#[async_trait]
pub trait FetchFile: Send + Sync {
    /// Fetches a file that was previously distributed to this backend.
    ///
    /// Returns `Ok(None)` if the backend does not hold the file (anymore).
    async fn fetch_file(&self, id: ShortGuid) -> Result<Option<FetchedFile>, FetchError>;
}

/// The content of a fetched file, read as it arrives from the backend.
pub type FetchedContent = Box<dyn AsyncRead + Send + Unpin>;

/// A file fetched from a backend, together with its metadata.
///
/// The content is not verified: whoever reads it must check it against
/// [`hashes`](Self::hashes) before trusting it.
pub struct FetchedFile {
    /// The original file name, if one was specified on upload.
    pub file_name: Option<String>,
    /// The content type, if one was specified on upload.
    pub content_type: Option<String>,
    /// The hashes recorded when the file was uploaded.
    pub hashes: FileHashes,
    /// The content size in bytes, if the backend knows it up front.
    pub size: Option<u64>,
    /// The file content.
    pub content: FetchedContent,
    /// When the backend's copy expires, or `None` if it is kept indefinitely.
    ///
    /// Copies must not be served or cached beyond this point.
    pub expires_at: Option<SystemTime>,
}

impl FetchedFile {
    /// Rebuilds a file from the serialized metadata a backend stored on
    /// distribution and a stream of its content.
    ///
    /// The metadata comes from the backend and is validated, not trusted.
    pub fn from_stored(
        metadata: &[u8],
        size: Option<u64>,
        content: FetchedContent,
    ) -> Result<Self, FetchError> {
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
            size,
            content,
        })
    }

    /// Rebuilds a file from the serialized metadata and the content a backend
    /// stored on distribution, with the content already in memory.
    pub fn from_stored_bytes(metadata: &[u8], data: Bytes) -> Result<Self, FetchError> {
        let size = data.len() as u64;
        Self::from_stored(metadata, Some(size), Box::new(Cursor::new(data)))
    }

    /// Checks whether the backend's copy has expired.
    pub fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| expires_at <= SystemTime::now())
    }

    /// Reads the whole content into memory, without verifying it.
    pub async fn into_bytes(mut self) -> std::io::Result<Bytes> {
        let mut data = Vec::with_capacity(self.size.unwrap_or_default() as usize);
        self.content.read_to_end(&mut data).await?;
        Ok(Bytes::from(data))
    }
}

impl Debug for FetchedFile {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FetchedFile")
            .field("file_name", &self.file_name)
            .field("content_type", &self.content_type)
            .field("hashes", &self.hashes)
            .field("size", &self.size)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
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
    use file_distribution::protobuf::Hashes;
    use std::time::{Duration, UNIX_EPOCH};

    fn stored_metadata(metadata: ItemMetadata) -> Bytes {
        metadata.serialize_to_proto().expect("Failed to serialize")
    }

    fn metadata_for(data: &[u8]) -> Bytes {
        let hashes = FileHashes::of(data);
        stored_metadata(ItemMetadata {
            file_name: Some("a.txt".into()),
            content_type: Some("text/plain".into()),
            hashes: Some(Hashes {
                md5: hashes.md5.to_vec(),
                sha256: hashes.sha256.to_vec(),
            }),
            expires_at_unix_ms: Some(1_000),
            ..ItemMetadata::default()
        })
    }

    #[tokio::test]
    async fn stored_file_is_rebuilt_from_metadata() {
        let metadata = metadata_for(b"abc");

        let file = FetchedFile::from_stored_bytes(&metadata, Bytes::from_static(b"abc"))
            .expect("Failed to rebuild file");

        assert_eq!(file.file_name.as_deref(), Some("a.txt"));
        assert_eq!(file.content_type.as_deref(), Some("text/plain"));
        assert_eq!(file.expires_at, Some(UNIX_EPOCH + Duration::from_secs(1)));
        assert_eq!(file.size, Some(3));
        assert_eq!(file.hashes, FileHashes::of(b"abc"));
        assert_eq!(
            file.into_bytes().await.expect("Failed to read"),
            &b"abc"[..]
        );
    }

    #[tokio::test]
    async fn stored_file_can_be_streamed_without_a_known_size() {
        let metadata = metadata_for(b"abc");

        let file = FetchedFile::from_stored(&metadata, None, Box::new(&b"abc"[..]))
            .expect("Failed to rebuild file");

        assert_eq!(file.size, None);
        assert_eq!(
            file.into_bytes().await.expect("Failed to read"),
            &b"abc"[..]
        );
    }

    #[test]
    fn stored_file_without_hashes_is_rejected() {
        let metadata = stored_metadata(ItemMetadata::default());
        let result = FetchedFile::from_stored_bytes(&metadata, Bytes::from_static(b"abc"));
        assert!(matches!(result, Err(FetchError::InvalidMetadata(_))));
    }

    #[test]
    fn undecodable_metadata_is_rejected() {
        let result =
            FetchedFile::from_stored_bytes(&[0xff, 0xff, 0xff], Bytes::from_static(b"abc"));
        assert!(matches!(result, Err(FetchError::InvalidMetadata(_))));
    }

    #[test]
    fn expiry_is_checked_against_the_current_time() {
        let mut file = FetchedFile::from_stored_bytes(&metadata_for(b"abc"), Bytes::new())
            .expect("Failed to rebuild file");
        file.expires_at = None;
        assert!(!file.is_expired());

        file.expires_at = Some(SystemTime::now() + Duration::from_secs(60));
        assert!(!file.is_expired());

        file.expires_at = Some(SystemTime::now() - Duration::from_secs(1));
        assert!(file.is_expired());
    }

    #[test]
    fn debug_output_omits_the_content() {
        let file = FetchedFile::from_stored_bytes(&metadata_for(b"abc"), Bytes::new())
            .expect("Failed to rebuild file");
        let debug = format!("{file:?}");
        assert!(debug.contains("a.txt"));
        assert!(!debug.contains("content:"));
    }
}
