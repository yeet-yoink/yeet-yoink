use sha2::Digest;
use std::fmt::{Debug, Display, Formatter};

/// An MD5 hash.
pub struct HashMd5(Hash<md5::Context, md5::Digest>);

/// A SHA-256 hash.
pub struct HashSha256(Hash<sha2::Sha256, [u8; 32]>);

impl HashMd5 {
    pub fn new() -> Self {
        Self(Hash::new(md5::Context::new()))
    }

    pub fn finalize(&mut self) -> Result<&md5::Digest, HashFinalizationError> {
        self.0.finalize(|md5| md5.compute())
    }

    pub fn hash(&self) -> Option<&md5::Digest> {
        self.0.hash()
    }
}

impl HashSha256 {
    pub fn new() -> Self {
        Self(Hash::new(sha2::Sha256::new()))
    }

    pub fn finalize(&mut self) -> Result<&[u8; 32], HashFinalizationError> {
        self.0.finalize(|sha256| {
            let mut hash = [0u8; 32];
            sha256.finalize_into((&mut hash).into());
            hash
        })
    }

    pub fn hash(&self) -> Option<&[u8; 32]> {
        self.0.hash()
    }
}

impl Display for HashMd5 {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Hash::Computing(_) => write!(f, "MD5 hash computing"),
            Hash::Finalizing => write!(f, "MD5 hash finalizing"),
            Hash::Finalized(hash) => write!(f, "MD5 {hash:x}"),
        }
    }
}

impl Debug for HashMd5 {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

impl Display for HashSha256 {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Hash::Computing(_) => write!(f, "SHA-256 hash computing"),
            Hash::Finalizing => write!(f, "SHA-256 hash finalizing"),
            Hash::Finalized(hash) => write!(f, "SHA-256 {hash}", hash = hex::encode(hash)),
        }
    }
}

impl Debug for HashSha256 {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

#[derive(Debug)]
enum Hash<T, D> {
    Computing(T),
    Finalizing,
    Finalized(D),
}

impl<T, D> Hash<T, D> {
    pub fn new(hasher: T) -> Self {
        Self::Computing(hasher)
    }

    fn finalize<F>(&mut self, mut f: F) -> Result<&D, HashFinalizationError>
    where
        F: FnMut(T) -> D,
    {
        *self = match std::mem::replace(self, Hash::Finalizing) {
            Hash::Computing(h) => Self::Finalized(f(h)),
            Hash::Finalized(_) => return Err(HashFinalizationError::HashAlreadyFinalized),
            Hash::Finalizing => unreachable!(),
        };

        if let Hash::Finalized(hash) = self {
            Ok(hash)
        } else {
            unreachable!()
        }
    }

    pub fn hash(&self) -> Option<&D> {
        match self {
            Hash::Computing(_) => None,
            Hash::Finalizing => None,
            Hash::Finalized(digest) => Some(digest),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HashFinalizationError {
    #[error("Failed to obtain a lock on the inner hash")]
    FailedToObtainLock,
    #[error("The hash was already finalized")]
    HashAlreadyFinalized,
}
