use crate::backbone::file_hashes::FileHashes;
use crate::backbone::hash::{HashMd5, HashSha256};
use sha2::Digest;
use shared_files::{CompleteWritingError, SharedTemporaryFileWriter};
use std::io::{Error, ErrorKind, IoSlice};
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;
use tracing::debug;
use uuid::Uuid;

pub struct Writer {
    inner: Option<SharedTemporaryFileWriter>,
    md5: HashMd5,
    sha256: HashSha256,
}

impl Writer {
    pub fn new(id: &Uuid, inner: SharedTemporaryFileWriter) -> Self {
        debug!(
            "Buffering payload for request {id} to {file:?}",
            file = inner.file_path()
        );

        Self {
            inner: Some(inner),
            md5: HashMd5::new(),
            sha256: HashSha256::new(),
        }
    }

    pub async fn sync_data(&self) -> Result<(), SynchronizationError> {
        let inner = self
            .inner
            .as_ref()
            .ok_or(SynchronizationError::FileClosed)?;
        Ok(inner.sync_data().await?)
    }

    pub async fn finalize(mut self, mode: CompletionMode) -> Result<FileHashes, FinalizationError> {
        let inner = self.inner.take().ok_or(FinalizationError::FileClosed)?;
        match mode {
            CompletionMode::Sync => inner.complete().await?,
            CompletionMode::NoSync => inner.complete_no_sync()?,
        }

        let md5 = self.md5.finalize();

        let sha256 = self.sha256.finalize();

        Ok(FileHashes { sha256, md5 })
    }

    fn err_broken_pipe() -> Error {
        Error::new(ErrorKind::BrokenPipe, "Writer closed")
    }

    fn update_hashes(&mut self, buf: &[u8]) {
        self.md5.update(buf);
        self.sha256.update(buf);
    }
}

pub enum CompletionMode {
    Sync,
    NoSync,
}

#[derive(Debug, thiserror::Error)]
pub enum FinalizationError {
    #[error("The file was already closed")]
    FileClosed,
    #[error("Syncing the file to disk failed")]
    FileSyncFailed(#[from] CompleteWritingError),
}

#[derive(Debug, thiserror::Error)]
pub enum SynchronizationError {
    #[error("The file was already closed")]
    FileClosed,
    #[error("Syncing the file to disk failed")]
    FileSyncFailed(#[from] CompleteWritingError),
}

impl AsyncWrite for Writer {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        self.update_hashes(&buf);
        if let Some(ref mut writer) = self.inner.as_mut() {
            Pin::new(writer).poll_write(cx, buf)
        } else {
            Poll::Ready(Err(Self::err_broken_pipe()))
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        if let Some(ref mut writer) = self.inner.as_mut() {
            Pin::new(writer).poll_flush(cx)
        } else {
            Poll::Ready(Err(Error::new(ErrorKind::BrokenPipe, "Writer closed")))
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        if let Some(ref mut writer) = self.inner.as_mut() {
            Pin::new(writer).poll_shutdown(cx)
        } else {
            Poll::Ready(Err(Error::new(ErrorKind::BrokenPipe, "Writer closed")))
        }
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize, Error>> {
        unimplemented!("Due to hashing, vectored writing is unsupported")
    }

    fn is_write_vectored(&self) -> bool {
        false
    }
}
