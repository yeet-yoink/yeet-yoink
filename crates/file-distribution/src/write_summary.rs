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
    // TODO: Ensure this data is actually correct - could be subject to a race condition when not flushing the data properly
    pub file_size_bytes: usize,
}
