use crate::{FileHashes, WriteSummary};
use bytes::{Bytes, BytesMut};
use prost::Message;
use shortguid::ShortGuid;
use std::sync::Arc;

include!(concat!(env!("OUT_DIR"), "/types.rs"));

impl ItemMetadata {
    pub fn new(id: ShortGuid, summary: &Arc<WriteSummary>) -> Self {
        Self {
            id: Vec::from(id.as_bytes()),
            file_name: summary.file_name.clone(),
            hashes: Some(Hashes {
                md5: Vec::from(summary.hashes.md5.as_slice()),
                sha256: Vec::from(summary.hashes.sha256.as_slice()),
            }),
            content_type: summary.content_type.clone(),
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
        let buf = ItemMetadata::new(id, &summary)
            .serialize_to_proto()
            .expect("Failed to serialize");

        let metadata = ItemMetadata::deserialize_from_proto(&buf).expect("Failed to deserialize");
        assert_eq!(metadata.id, id.as_bytes());
        assert_eq!(metadata.file_name.as_deref(), Some("abc.txt"));
        assert_eq!(metadata.content_type.as_deref(), Some("text/plain"));

        let hashes = metadata.file_hashes().expect("Missing hashes");
        assert_eq!(hashes.md5, summary.hashes.md5);
        assert_eq!(hashes.sha256, summary.hashes.sha256);
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
        };
        assert!(metadata.file_hashes().is_none());

        let metadata = ItemMetadata {
            hashes: None,
            ..metadata
        };
        assert!(metadata.file_hashes().is_none());
    }
}
