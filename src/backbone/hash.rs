use sha2::Digest;

/// An MD5 hash.
pub struct HashMd5(Hash<md5::Context, md5::Digest>);

/// A SHA-256 hash.
pub struct HashSha256(Hash<sha2::Sha256, [u8; 32]>);

impl HashMd5 {
    pub fn new() -> Self {
        Self(Hash::new(md5::Context::new()))
    }

    pub fn update(&mut self, chunk: &[u8]) -> Result<(), HashFinalizationError> {
        self.0.update(move |md5| md5.consume(chunk))
    }

    pub fn finalize(self) -> Result<md5::Digest, HashFinalizationError> {
        self.0.finalize(move |h| h.compute())
    }
}

impl HashSha256 {
    pub fn new() -> Self {
        Self(Hash::new(sha2::Sha256::new()))
    }

    pub fn update(&mut self, chunk: &[u8]) -> Result<(), HashFinalizationError> {
        self.0.update(move |h| h.update(chunk))
    }

    pub fn finalize(self) -> Result<[u8; 32], HashFinalizationError> {
        self.0.finalize(move |sha256| {
            let mut hash = [0u8; 32];
            sha256.finalize_into((&mut hash).into());
            hash
        })
    }
}

/// The underlying hash implementation.
#[derive(Debug)]
enum Hash<T, D> {
    /// The hash is about to be computed.
    Computing(T),
    /// The hash was computed. Will never be instantiated.
    Finalized(D),
}

impl<T, D> Hash<T, D> {
    pub fn new(hasher: T) -> Self {
        Self::Computing(hasher)
    }

    fn update<F>(&mut self, mut f: F) -> Result<(), HashFinalizationError>
    where
        F: FnMut(&mut T) -> (),
    {
        match self {
            Hash::Computing(h) => {
                f(h);
                Ok(())
            }
            Hash::Finalized(_) => Err(HashFinalizationError::HashAlreadyFinalized),
        }
    }

    fn finalize<F>(self, mut f: F) -> Result<D, HashFinalizationError>
    where
        F: FnMut(T) -> D,
    {
        match self {
            Hash::Computing(h) => Ok(f(h)),
            Hash::Finalized(_) => Err(HashFinalizationError::HashAlreadyFinalized),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HashFinalizationError {
    #[error("The hash was already finalized")]
    HashAlreadyFinalized,
}
