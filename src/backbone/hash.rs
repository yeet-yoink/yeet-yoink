use sha2::Digest;

/// An MD5 hash.
pub struct HashMd5(md5::Context);

/// A SHA-256 hash.
pub struct HashSha256(sha2::Sha256);

impl HashMd5 {
    pub fn new() -> Self {
        Self(md5::Context::new())
    }

    pub fn update(&mut self, chunk: &[u8]) {
        self.0.consume(chunk)
    }

    pub fn finalize(self) -> md5::Digest {
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

    pub fn finalize(self) -> [u8; 32] {
        let mut hash = [0u8; 32];
        self.0.finalize_into((&mut hash).into());
        hash
    }
}
