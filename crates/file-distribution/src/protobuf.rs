use crate::{FileHashes, WriteSummary};
use bytes::{Bytes, BytesMut};
use prost::Message;
use shortguid::ShortGuid;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

include!(concat!(env!("OUT_DIR"), "/types.rs"));

/// 9999-12-31T23:59:59.999Z, the latest expiry accepted from a backend.
const MAX_EXPIRES_AT_UNIX_MS: u64 = 253_402_300_799_999;

#[derive(Debug, thiserror::Error)]
#[error("Expiry timestamp {0} ms is out of range")]
pub struct InvalidExpiry(pub u64);

impl ItemMetadata {
    /// Creates the metadata for a file stored by a backend.
    ///
    /// `expires_at` is when the backend's copy expires, or `None` if it is kept
    /// indefinitely.
    pub fn new(id: ShortGuid, summary: &Arc<WriteSummary>, expires_at: Option<SystemTime>) -> Self {
        Self {
            id: Vec::from(id.as_bytes()),
            file_name: summary.file_name.clone(),
            hashes: Some(Hashes {
                md5: Vec::from(summary.hashes.md5.as_slice()),
                sha256: Vec::from(summary.hashes.sha256.as_slice()),
            }),
            content_type: summary.content_type.clone(),
            expires_at_unix_ms: expires_at.map(|t| {
                let since_epoch = t.duration_since(UNIX_EPOCH).unwrap_or_default();
                since_epoch.as_millis().min(u64::MAX as u128) as u64
            }),
        }
    }

    /// Gets when the backend's copy expires, or `None` if it is kept indefinitely.
    ///
    /// Metadata comes from the backend and is not trusted: timestamps beyond the
    /// year 9999 are rejected.
    pub fn expires_at(&self) -> Result<Option<SystemTime>, InvalidExpiry> {
        match self.expires_at_unix_ms {
            None => Ok(None),
            Some(ms) if ms > MAX_EXPIRES_AT_UNIX_MS => Err(InvalidExpiry(ms)),
            Some(ms) => Ok(Some(UNIX_EPOCH + Duration::from_millis(ms))),
        }
    }

    pub fn deserialize_from_proto(buf: &[u8]) -> Result<Self, prost::DecodeError> {
        Self::decode(buf)
    }

    /// Gets the file hashes, or `None` if they are missing or malformed.
    pub fn file_hashes(&self) -> Option<FileHashes> {
        let hashes = self.hashes.as_ref()?;
        let md5 = hashes.md5.as_slice().try_into().ok()?;
        let sha256 = hashes.sha256.as_slice().try_into().ok()?;
        Some(FileHashes::new(md5::Digest(md5), sha256))
    }

    pub fn serialize_to_proto(&self) -> Result<Bytes, prost::EncodeError> {
        let mut metadata_buf = BytesMut::new();
        self.encode(&mut metadata_buf)?;
        Ok(metadata_buf.freeze())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::{HashMd5, HashSha256};
    use tokio::time::Instant;

    fn summary() -> Arc<WriteSummary> {
        let mut md5 = HashMd5::new();
        md5.update(b"abc");
        let mut sha256 = HashSha256::new();
        sha256.update(b"abc");
        Arc::new(WriteSummary {
            expires: Instant::now(),
            hashes: FileHashes::new(md5.finalize(), sha256.finalize()),
            file_name: Some("abc.txt".into()),
            content_type: Some("text/plain".into()),
            file_size_bytes: 3,
        })
    }

    #[test]
    fn metadata_round_trips() {
        let summary = summary();
        let id = ShortGuid::from_bytes([7u8; 16]);
        let expires_at = UNIX_EPOCH + Duration::from_millis(1_800_000_000_000);
        let buf = ItemMetadata::new(id, &summary, Some(expires_at))
            .serialize_to_proto()
            .expect("Failed to serialize");

        let metadata = ItemMetadata::deserialize_from_proto(&buf).expect("Failed to deserialize");
        assert_eq!(metadata.id, id.as_bytes());
        assert_eq!(metadata.file_name.as_deref(), Some("abc.txt"));
        assert_eq!(metadata.content_type.as_deref(), Some("text/plain"));
        assert_eq!(metadata.expires_at().ok(), Some(Some(expires_at)));

        let hashes = metadata.file_hashes().expect("Missing hashes");
        assert_eq!(hashes.md5, summary.hashes.md5);
        assert_eq!(hashes.sha256, summary.hashes.sha256);
    }

    #[test]
    fn metadata_without_expiry_is_kept_indefinitely() {
        let buf = ItemMetadata::new(ShortGuid::from_bytes([7u8; 16]), &summary(), None)
            .serialize_to_proto()
            .expect("Failed to serialize");

        let metadata = ItemMetadata::deserialize_from_proto(&buf).expect("Failed to deserialize");
        assert_eq!(metadata.expires_at().ok(), Some(None));
    }

    #[test]
    fn out_of_range_expiry_is_rejected() {
        let mut metadata = ItemMetadata::new(ShortGuid::from_bytes([7u8; 16]), &summary(), None);

        metadata.expires_at_unix_ms = Some(MAX_EXPIRES_AT_UNIX_MS);
        assert!(metadata.expires_at().is_ok());

        metadata.expires_at_unix_ms = Some(u64::MAX);
        assert!(metadata.expires_at().is_err());
    }

    #[test]
    fn malformed_hashes_are_rejected() {
        let metadata = ItemMetadata {
            id: vec![],
            file_name: None,
            hashes: Some(Hashes {
                md5: vec![0; 15],
                sha256: vec![0; 32],
            }),
            content_type: None,
            expires_at_unix_ms: None,
        };
        assert!(metadata.file_hashes().is_none());

        let metadata = ItemMetadata {
            hashes: None,
            ..metadata
        };
        assert!(metadata.file_hashes().is_none());
    }
}
