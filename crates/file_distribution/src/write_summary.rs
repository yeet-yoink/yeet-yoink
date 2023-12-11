use crate::FileHashes;
use tokio::time::Instant;

/// A write result.
#[derive(Debug)]
pub struct WriteSummary {
    /// The instant at which the file will expire.
    pub expires: Instant,
    /// The file hashes.
    pub hashes: FileHashes,
    /// The optional file name.
    pub file_name: Option<String>,
    /// The file size in bytes.
    pub file_size_bytes: usize,
}
