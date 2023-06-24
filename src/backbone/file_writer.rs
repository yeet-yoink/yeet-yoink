use crate::backbone::file_hashes::FileHashes;
use crate::backbone::hash::{HashMd5, HashSha256};
use shared_files::{CompleteWritingError, SharedTemporaryFileWriter};
use shortguid::ShortGuid;
use std::io::{Error, ErrorKind};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::time::Instant;
use tracing::debug;

/// A write accessor for a temporary file.
///
/// ## Remarks
///
/// This writer will be protected by a [`WriterGuard`](crate::backbone::file_writer_guard::FileWriterGuard)
/// ensuring that regardless of whether this writer is finalized or dropped without finalization,
/// the [`Backbone`](crate::backbone::Backbone) is informed about it.
pub struct FileWriter {
    inner: SharedTemporaryFileWriter,
    md5: HashMd5,
    sha256: HashSha256,
}

impl FileWriter {
    pub fn new(id: &ShortGuid, inner: SharedTemporaryFileWriter) -> Self {
        debug!(
            "Buffering payload for request {id} to {file:?}",
            file = inner.file_path()
        );

        Self {
            inner,
            md5: HashMd5::new(),
            sha256: HashSha256::new(),
        }
    }

    pub async fn write(&mut self, chunk: &[u8]) -> std::io::Result<usize> {
        self.update_hashes(chunk);
        self.inner.write(chunk).await
    }

    pub async fn sync_data(&self) -> Result<(), SynchronizationError> {
        Ok(self.inner.sync_data().await?)
    }

    pub async fn finalize(
        self,
        mode: CompletionMode,
        expiration: Duration,
    ) -> Result<Arc<WriteSummary>, FinalizationError> {
        match mode {
            CompletionMode::Sync => self.inner.complete().await?,
            CompletionMode::NoSync => self.inner.complete_no_sync()?,
        }

        let md5 = self.md5.finalize();
        let sha256 = self.sha256.finalize();
        let summary = Arc::new(WriteSummary {
            expires: Instant::now() + expiration,
            hashes: FileHashes { sha256, md5 },
        });

        Ok(summary)
    }

    fn update_hashes(&mut self, buf: &[u8]) {
        self.md5.update(buf);
        self.sha256.update(buf);
    }
}

/// A write result.
#[derive(Debug)]
pub struct WriteSummary {
    /// The instant at which the file will expire.
    pub expires: Instant,
    /// The file hashes.
    pub hashes: FileHashes,
}

pub(crate) fn err_broken_pipe<T>() -> Result<T, Error> {
    Err(Error::new(ErrorKind::BrokenPipe, "Writer closed"))
}

#[allow(dead_code)]
pub enum CompletionMode {
    Sync,
    NoSync,
}

#[derive(Debug, thiserror::Error)]
pub enum FinalizationError {
    #[error("Syncing the file to disk failed")]
    FileSyncFailed(#[from] CompleteWritingError),
    #[error("Failed to communicate to the backbone")]
    BackboneCommunicationFailed,
}

#[derive(Debug, thiserror::Error)]
pub enum SynchronizationError {
    #[error("Syncing the file to disk failed")]
    FileSyncFailed(#[from] CompleteWritingError),
}
