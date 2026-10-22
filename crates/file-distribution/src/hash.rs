use sha2::Digest;

/// An MD5 hash.
pub struct HashMd5(md5::Context);

/// A SHA-256 hash.
pub struct HashSha256(sha2::Sha256);

/// Alias for an MD5 hash digest.
pub type Md5Digest = md5::Digest;

/// Alias for a SHA-256 hash digest.
pub type Sha256Digest = [u8; 32];

impl HashMd5 {
    pub fn new() -> Self {
        Self(md5::Context::new())
    }

    pub fn update(&mut self, chunk: &[u8]) {
        self.0.consume(chunk)
    }

    pub fn finalize(self) -> Md5Digest {
        self.0.finalize()
    }
}

impl HashSha256 {
    pub fn new() -> Self {
        Self(sha2::Sha256::new())
    }

    pub fn update(&mut self, chunk: &[u8]) {
        self.0.update(chunk)
    }

    pub fn finalize(self) -> Sha256Digest {
        let digest = self.0.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&digest);
        hash
    }
}

impl Default for HashMd5 {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for HashSha256 {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md5_hex(chunks: &[&[u8]]) -> String {
        let mut hash = HashMd5::new();
        for chunk in chunks {
            hash.update(chunk);
        }
        format!("{:x}", hash.finalize())
    }

    fn sha256_hex(chunks: &[&[u8]]) -> String {
        let mut hash = HashSha256::new();
        for chunk in chunks {
            hash.update(chunk);
        }
        hex::encode(hash.finalize())
    }

    #[test]
    fn md5_of_empty_input() {
        assert_eq!(md5_hex(&[]), "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn md5_known_vector() {
        assert_eq!(md5_hex(&[b"abc"]), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn sha256_of_empty_input() {
        assert_eq!(
            sha256_hex(&[]),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_known_vector() {
        assert_eq!(
            sha256_hex(&[b"abc"]),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn chunked_updates_match_single_update() {
        assert_eq!(md5_hex(&[b"a", b"", b"bc"]), md5_hex(&[b"abc"]));
        assert_eq!(sha256_hex(&[b"a", b"", b"bc"]), sha256_hex(&[b"abc"]));
    }
}
