use crate::hash::{Md5Digest, Sha256Digest};
use std::fmt::{Debug, Display, Formatter};

/// The calculated hashes of a file.
#[derive(Clone)]
pub struct FileHashes {
    /// The MD5 digest.
    pub md5: Md5Digest,
    /// The SHA-256 hash.
    pub sha256: Sha256Digest,
}

impl FileHashes {
    pub fn new(md5: Md5Digest, sha256: Sha256Digest) -> Self {
        Self { md5, sha256 }
    }
}

impl Debug for FileHashes {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

impl Display for FileHashes {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MD5 {md5:x}, SHA256 {sha256:x}",
            md5 = self.md5,
            sha256 = self.sha256
        )
    }
}
