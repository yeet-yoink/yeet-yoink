use async_tempfile::TempFile;
use crossbeam::atomic::AtomicCell;
use pin_project::{pin_project, pinned_drop};
use std::io;
use std::io::{Error, ErrorKind, IoSlice};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use thiserror::Error;
use tokio::io::AsyncWrite;

/// A temporary file with shared read/write access.
pub struct SharedTemporaryFile {
    /// The sentinel value to keep the file alive.
    sentinel: Arc<Sentinel>,
}

/// A writer for the shared temporary file.
#[pin_project(PinnedDrop)]
pub struct SharedTemporaryFileWriter {
    /// The file to write to.
    #[pin]
    file: TempFile,
    /// The sentinel value to keep the file alive.
    sentinel: Arc<Sentinel>,
}

// WrappedTemporaryFile is Unpin because the future is
// contained within the Inner struct which, in turn, is stored
// on the heap due to the Arc<> wrapper. As a result,
// WrappedTemporaryFile can move freely without moving the future.
impl Unpin for SharedTemporaryFile {}

struct Sentinel {
    /// The original file. This keeps the file open until all references are dropped.
    original: TempFile,
    /// The state of the write operation.
    state: AtomicCell<State>,
}

#[derive(Debug, Clone, Copy)]
enum State {
    /// The write operation is pending. Contains the number of bytes written.
    Pending(usize),
    /// The write operation completed. Contains the file size.
    Completed(usize),
    /// The write operation failed.
    Failed,
}

impl SharedTemporaryFile {
    /// Creates a new temporary file.
    pub async fn new() -> Result<SharedTemporaryFile, async_tempfile::Error> {
        let file = TempFile::new().await?;
        Ok(Self {
            sentinel: Arc::new(Sentinel {
                original: file,
                state: AtomicCell::new(State::Pending(0)),
            }),
        })
    }

    /// Obtains the path of the temporary file.
    pub async fn file_path(&self) -> &PathBuf {
        self.sentinel.original.file_path()
    }

    /// Creates a writer for the file.
    ///
    /// Note that this operation can result in odd behavior if the
    /// file is accessed multiple times for write access. User code
    /// must make sure that only one meaningful write is performed at
    /// the same time.
    pub async fn writer(&self) -> Result<SharedTemporaryFileWriter, async_tempfile::Error> {
        let file = self.sentinel.original.open_rw().await?;
        Ok(SharedTemporaryFileWriter {
            sentinel: self.sentinel.clone(),
            file,
        })
    }
}

impl SharedTemporaryFileWriter {
    /// Gets the file path.
    pub async fn file_path(&self) -> &PathBuf {
        self.file.file_path()
    }

    /// Synchronizes data and metadata with the disk buffer.
    pub async fn sync_all(&self) -> Result<(), Error> {
        self.file.sync_all().await
    }

    /// Synchronizes data with the disk buffer.
    pub async fn sync_data(&self) -> Result<(), Error> {
        self.file.sync_data().await
    }

    /// Completes the writing operation.
    ///
    /// Use [`complete_no_sync`](Self::complete_no_sync) if you do not wish
    /// to sync the file to disk.
    pub async fn complete(mut self) -> Result<(), CompleteWritingError> {
        self.file.sync_all().await?;
        self.complete_no_sync()
    }

    /// Completes the writing operation.
    ///
    /// If you need to sync the file to disk, consider calling
    /// [`complete`](Self::complete) instead.
    pub fn complete_no_sync(mut self) -> Result<(), CompleteWritingError> {
        self.finalize_state()
    }

    /// Sets the state to finalized.
    ///
    /// See also [`update_state`](Self::update_state) for increasing the byte count.
    fn finalize_state(&self) -> Result<(), CompleteWritingError> {
        match self.sentinel.state.load() {
            State::Pending(size) => {
                self.sentinel.state.store(State::Completed(size));
                Ok(())
            }
            State::Completed(_) => Ok(()),
            State::Failed => Err(CompleteWritingError::FileWritingFailed),
        }
    }

    /// Updates the internal byte count with the specified number of bytes written.
    /// Will produce an error if the update failed.
    ///
    /// See also [`finalize_state`](Self::finalize_state) for finalizing the write.
    fn update_state(state: &AtomicCell<State>, written: usize) -> Result<usize, Error> {
        match state.load() {
            State::Pending(count) => {
                state.store(State::Pending(count + written));
                Ok(count)
            }
            State::Completed(count) => {
                // Ensure we do not try to write more data after completing
                // the file.
                if written != 0 {
                    return Err(Error::new(ErrorKind::BrokenPipe, WriteError::FileClosed));
                }
                Ok(count)
            }
            State::Failed => Err(Error::from(ErrorKind::Other)),
        }
    }

    /// Processes a [`Poll`] result from a write operation.
    ///
    /// This will update the internal byte count and produce an error
    /// if the update failed.
    fn handle_poll_write_result(
        state: &AtomicCell<State>,
        poll: Poll<Result<usize, Error>>,
    ) -> Poll<Result<usize, io::Error>> {
        match poll {
            Poll::Ready(result) => match result {
                Ok(written) => match Self::update_state(&state, written) {
                    Ok(count) => Poll::Ready(Ok(count)),
                    Err(e) => Poll::Ready(Err(e)),
                },
                Err(e) => {
                    state.store(State::Failed);
                    Poll::Ready(Err(e))
                }
            },
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Debug, Error)]
pub enum CompleteWritingError {
    #[error(transparent)]
    Io(#[from] Error),
    #[error("Writing to the file failed")]
    FileWritingFailed,
}

#[derive(Debug, Error)]
pub enum WriteError {
    #[error(transparent)]
    Io(#[from] Error),
    #[error("The file was already closed")]
    FileClosed,
}

#[pinned_drop]
impl PinnedDrop for SharedTemporaryFileWriter {
    fn drop(mut self: Pin<&mut Self>) {
        self.finalize_state().ok();
    }
}

impl AsyncWrite for SharedTemporaryFileWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.project();
        let poll = this.file.poll_write(cx, buf);
        Self::handle_poll_write_result(&this.sentinel.state, poll)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.project();
        match this.file.poll_flush(cx) {
            Poll::Ready(result) => match result {
                Ok(()) => {
                    // Flushing doesn't change the number of bytes written,
                    // so we don't update the counter here.
                    Poll::Ready(Ok(()))
                }
                Err(e) => {
                    this.sentinel.state.store(State::Failed);
                    Poll::Ready(Err(e))
                }
            },
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        let mut this = self.project();
        match this.file.poll_shutdown(cx) {
            Poll::Ready(result) => match result {
                Ok(()) => {
                    if let State::Pending(count) = this.sentinel.state.load() {
                        this.sentinel.state.store(State::Completed(count));
                    }

                    Poll::Ready(Ok(()))
                }
                Err(e) => {
                    this.sentinel.state.store(State::Failed);
                    Poll::Ready(Err(e))
                }
            },
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize, Error>> {
        let this = self.project();
        let poll = this.file.poll_write_vectored(cx, bufs);
        Self::handle_poll_write_result(&this.sentinel.state, poll)
    }

    fn is_write_vectored(&self) -> bool {
        self.file.is_write_vectored()
    }
}
