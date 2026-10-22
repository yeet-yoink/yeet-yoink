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
            "MD5 {md5:x}, SHA256 {sha256}",
            md5 = self.md5,
            sha256 = hex::encode(self.sha256)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::{HashMd5, HashSha256};

    #[test]
    fn display_formats_both_hashes_as_hex() {
        let mut md5 = HashMd5::new();
        md5.update(b"abc");
        let mut sha256 = HashSha256::new();
        sha256.update(b"abc");

        let hashes = FileHashes::new(md5.finalize(), sha256.finalize());
        let expected = "MD5 900150983cd24fb0d6963f7d28e17f72, \
            SHA256 ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(hashes.to_string(), expected);
        assert_eq!(format!("{hashes:?}"), expected);
    }
}
