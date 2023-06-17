use sha2::digest::consts::U32;
use sha2::digest::generic_array::{ArrayLength, GenericArray};
use sha2::Digest;

/// An MD5 hash.
pub struct HashMd5(md5::Context);

/// A SHA-256 hash.
pub struct HashSha256(sha2::Sha256);

/// Alias for a SHA-256 hash digest.
pub type Md5Digest = md5::Digest;

/// Alias for a SHA-256 hash digest.
pub type Sha256Digest = GenericArray<u8, U32>;

impl HashMd5 {
    pub fn new() -> Self {
        Self(md5::Context::new())
    }

    pub fn update(&mut self, chunk: &[u8]) {
        self.0.consume(chunk)
    }

    pub fn finalize(self) -> Md5Digest {
        self.0.compute()
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
        let mut hash = [0u8; 32];
        let mut hash = GenericArray::from(hash);
        self.0.finalize_into(&mut hash);
        hash
    }
}
