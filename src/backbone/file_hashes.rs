use std::fmt::{Display, Formatter};

/// The calculated hashes of a file.
pub struct FileHashes {
    /// The MD5 digest.
    pub md5: md5::Digest,
    /// The SHA-256 hash.
    pub sha256: [u8; 32], // TODO: Replace with GenericArray
}

impl Display for FileHashes {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MD5 {md5:x}, SHA256 {sha256}",
            md5 = self.md5,
            sha256 = hex::encode(self.sha256)
        )
    }
}
