use sha2::Digest;

/// An MD5 hash.
pub struct HashMd5(Hash<md5::Context>);

/// A SHA-256 hash.
pub struct HashSha256(Hash<sha2::Sha256>);

impl HashMd5 {
    pub fn new() -> Self {
        Self(Hash::new(md5::Context::new()))
    }

    pub fn update(&mut self, chunk: &[u8]) {
        self.0.update(move |md5| md5.consume(chunk))
    }

    pub fn finalize(self) -> md5::Digest {
        self.0.finalize(move |h| h.compute())
    }
}

impl HashSha256 {
    pub fn new() -> Self {
        Self(Hash::new(sha2::Sha256::new()))
    }

    pub fn update(&mut self, chunk: &[u8]) {
        self.0.update(move |h| h.update(chunk))
    }

    pub fn finalize(self) -> [u8; 32] {
        self.0.finalize(move |sha256| {
            let mut hash = [0u8; 32];
            sha256.finalize_into((&mut hash).into());
            hash
        })
    }
}

/// The underlying hash implementation.
#[derive(Debug)]
struct Hash<T>(T);

impl<T> Hash<T> {
    pub fn new(hasher: T) -> Self {
        Self(hasher)
    }

    fn update<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut T) -> (),
    {
        f(&mut self.0)
    }

    fn finalize<F, D>(self, mut f: F) -> D
    where
        F: FnMut(T) -> D,
    {
        f(self.0)
    }
}
