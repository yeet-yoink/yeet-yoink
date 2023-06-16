use crate::backbone::file_hashes::FileHashes;
use crate::backbone::hash::{HashMd5, HashSha256};
use crate::backbone::writer_guard::WriteResult;
use shared_files::{CompleteWritingError, SharedTemporaryFileWriter};
use std::io::{Error, ErrorKind, IoSlice};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;
use tokio::sync::oneshot::Sender;
use tracing::debug;
use uuid::Uuid;

/// A write accessor for a temporary file.
pub struct Writer {
    inner: Option<SharedTemporaryFileWriter>,
    sender: Sender<WriteResult>,
    md5: HashMd5,
    sha256: HashSha256,
}

impl Writer {
    pub fn new(id: &Uuid, inner: SharedTemporaryFileWriter, sender: Sender<WriteResult>) -> Self {
        debug!(
            "Buffering payload for request {id} to {file:?}",
            file = inner.file_path()
        );

        Self {
            inner: Some(inner),
            sender,
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

    pub async fn finalize(
        mut self,
        mode: CompletionMode,
    ) -> Result<Arc<FileHashes>, FinalizationError> {
        let inner = self.inner.take().ok_or(FinalizationError::FileClosed)?;
        match mode {
            CompletionMode::Sync => inner.complete().await?,
            CompletionMode::NoSync => inner.complete_no_sync()?,
        }

        let md5 = self.md5.finalize();
        let sha256 = self.sha256.finalize();
        let hashes = Arc::new(FileHashes { sha256, md5 });

        // Send the hashes back to the backbone.
        if self
            .sender
            .send(WriteResult::Success(hashes.clone()))
            .is_err()
        {
            return Err(FinalizationError::BackboneCommunicationFailed);
        }

        Ok(hashes)
    }

    /// Signal a failure to the backbone.
    ///
    /// ## Remarks
    ///
    /// Since [`finalize`](Self::finalize) consumes self, this method
    /// cannot be used if the operation was successful. Likewise, since
    /// this method consumes self, [`finalize`](Self::finalize) cannot be
    /// called afterwards.
    pub fn fail(self) {
        self.sender
            .send(WriteResult::Failure)
            .map_err(move |_| {}) // discard the error
            .expect("failed to send the failure event");
    }

    fn update_hashes(&mut self, buf: &[u8]) {
        self.md5.update(buf);
        self.sha256.update(buf);
    }
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
    #[error("The file was already closed")]
    FileClosed,
    #[error("Syncing the file to disk failed")]
    FileSyncFailed(#[from] CompleteWritingError),
    #[error("Failed to communicate to the backbone")]
    BackboneCommunicationFailed,
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
            Poll::Ready(err_broken_pipe())
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        if let Some(ref mut writer) = self.inner.as_mut() {
            Pin::new(writer).poll_flush(cx)
        } else {
            Poll::Ready(err_broken_pipe())
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        if let Some(ref mut writer) = self.inner.as_mut() {
            Pin::new(writer).poll_shutdown(cx)
        } else {
            Poll::Ready(err_broken_pipe())
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize, Error>> {
        unimplemented!("Due to hashing, vectored writing is unsupported")
    }

    fn is_write_vectored(&self) -> bool {
        // We don't, but we need the poll_write_vectored method to be called
        // so that we can report an error.
        true
    }
}
