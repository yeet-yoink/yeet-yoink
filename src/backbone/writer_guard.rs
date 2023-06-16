use crate::backbone::file_hashes::FileHashes;
use crate::backbone::writer::{err_broken_pipe, FinalizationError, Writer};
use crate::backbone::CompletionMode;
use std::io::{Error, IoSlice};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::AsyncWrite;
use tokio::sync::oneshot::Sender;

/// A writer guard to communicate back to the [`Backbone`](crate::backbone::Backbone);
///
/// This exists to ensure that we can drop the [`Writer`] (e.g. if the HTTP request
/// is cancelled) and still have the [`Backbone`](crate::backbone::Backbone) informed
/// about it.
pub struct WriterGuard {
    inner: Option<Writer>,
    sender: Option<Sender<WriteResult>>,
}

/// A write result.
pub enum WriteResult {
    /// The writer succeeded.
    Success(Arc<FileHashes>),
    /// The writer failed.
    Failure,
}

impl WriterGuard {
    pub fn new(writer: Writer, sender: Sender<WriteResult>) -> Self {
        Self {
            inner: Some(writer),
            sender: Some(sender),
        }
    }

    pub async fn finalize(
        mut self,
        mode: CompletionMode,
    ) -> Result<Arc<FileHashes>, FinalizationError> {
        if let Some(writer) = self.inner.take() {
            let hashes = writer.finalize(mode).await?;
            self.try_succeed(&hashes)?;
            Ok(hashes)
        } else {
            Err(FinalizationError::BackboneCommunicationFailed)
        }
    }

    /// Signal a success to the backbone.
    fn try_succeed(mut self, hashes: &Arc<FileHashes>) -> Result<(), FinalizationError> {
        // Send the hashes back to the backbone.
        match self.sender.take() {
            None => Err(FinalizationError::BackboneCommunicationFailed),
            Some(sender) => match sender.send(WriteResult::Success(hashes.clone())) {
                Ok(_) => Ok(()),
                Err(_) => Err(FinalizationError::BackboneCommunicationFailed),
            },
        }
    }

    /// Signal a failure to the backbone.
    ///
    /// ## Remarks
    ///
    /// Since [`finalize`](Self::finalize) consumes self, this method
    /// cannot be used if the operation was successful. Likewise, since
    /// this method consumes self, [`finalize`](Self::finalize) cannot be
    /// called afterwards.
    fn try_fail(&mut self) {
        self.sender
            .take()
            .and_then(move |s| s.send(WriteResult::Failure).ok());
    }
}

/// This ensures that accidentally dropping the guard does not leave
/// the backbone in an uninformed state.
impl Drop for WriterGuard {
    fn drop(&mut self) {
        // This call can only actually fail if finalize wasn't called before.
        self.try_fail()
    }
}

impl Deref for WriterGuard {
    type Target = Writer;

    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().expect("failed to deref writer")
    }
}

impl DerefMut for WriterGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.as_mut().expect("failed to deref writer")
    }
}

// Pass through to Writer.
impl AsyncWrite for WriterGuard {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
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
