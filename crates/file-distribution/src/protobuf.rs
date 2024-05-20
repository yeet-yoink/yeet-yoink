use crate::WriteSummary;
use bytes::{Bytes, BytesMut};
use prost::Message;
use shortguid::ShortGuid;
use std::sync::Arc;

include!(concat!(env!("OUT_DIR"), "/types.rs"));

impl ItemMetadata {
    pub fn new(id: ShortGuid, summary: &Arc<WriteSummary>) -> Self {
        Self {
            id: Vec::from(id.as_bytes()),
            file_size_bytes: summary.file_size_bytes as _,
            file_name: summary.file_name.clone(),
            hashes: Some(Hashes {
                md5: Vec::from(summary.hashes.md5.as_slice()),
                sha256: Vec::from(summary.hashes.sha256.as_slice()),
            }),
        }
    }

    pub fn serialize_to_proto(&self) -> Result<Bytes, prost::EncodeError> {
        let mut metadata_buf = BytesMut::new();
        self.encode(&mut metadata_buf)?;
        Ok(metadata_buf.freeze())
    }

    pub fn deserialize_from_proto<B>(data: B) -> Result<Self, prost::DecodeError>
    where
        B: Into<Bytes>,
    {
        Self::decode(data.into())
    }
}
