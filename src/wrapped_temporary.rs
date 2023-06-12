use async_tempfile::TempFile;
use crossbeam::atomic::AtomicCell;
use pin_project::{pin_project, pinned_drop};
use std::collections::HashMap;
use std::io;
use std::io::{Error, ErrorKind, IoSlice};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use uuid::Uuid;

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

/// A reader for the shared temporary file.
#[pin_project(PinnedDrop)]
pub struct SharedTemporaryFileReader {
    /// The ID of the reader.
    id: Uuid,
    /// The file to read from.
    #[pin]
    file: TempFile,
    /// The sentinel value to keep the file alive.
    sentinel: Arc<Sentinel>,
}

struct Sentinel {
    /// The original file. This keeps the file open until all references are dropped.
    original: TempFile,
    /// The state of the write operation.
    state: AtomicCell<State>,
    /// Wakers to wake up all interested readers.
    wakers: Mutex<HashMap<Uuid, Waker>>,
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

/// Node ID for generating UUID v1 instances for readers.
/// These IDs never leave the current system, so the node ID is arbitrary.
static NODE_ID: &'static [u8; 6] = &[2, 3, 0, 6, 1, 2];

impl SharedTemporaryFile {
    /// Creates a new temporary file.
    pub async fn new() -> Result<SharedTemporaryFile, async_tempfile::Error> {
        let file = TempFile::new().await?;
        Ok(Self {
            sentinel: Arc::new(Sentinel {
                original: file,
                state: AtomicCell::new(State::Pending(0)),
                wakers: Mutex::new(HashMap::default()),
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

    /// Creates a reader for the file.
    pub async fn reader(&self) -> Result<SharedTemporaryFileReader, async_tempfile::Error> {
        let file = self.sentinel.original.open_ro().await?;
        Ok(SharedTemporaryFileReader {
            id: Uuid::now_v1(&NODE_ID),
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
    pub async fn complete(self) -> Result<(), CompleteWritingError> {
        self.file.sync_all().await?;
        self.complete_no_sync()
    }

    /// Completes the writing operation.
    ///
    /// If you need to sync the file to disk, consider calling
    /// [`complete`](Self::complete) instead.
    pub fn complete_no_sync(self) -> Result<(), CompleteWritingError> {
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
    /// ## Returns
    /// Returns the number of bytes written in total.
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
        sentinel: &Sentinel,
        poll: Poll<Result<usize, Error>>,
    ) -> Poll<Result<usize, io::Error>> {
        let result = match poll {
            Poll::Ready(result) => match result {
                Ok(written) => match Self::update_state(&sentinel.state, written) {
                    Ok(_) => Poll::Ready(Ok(written)),
                    Err(e) => Poll::Ready(Err(e)),
                },
                Err(e) => {
                    sentinel.state.store(State::Failed);
                    Poll::Ready(Err(e))
                }
            },
            Poll::Pending => Poll::Pending,
        };

        // Wake up waiting futures.
        if let Poll::Ready(e) = result {
            sentinel.wake_readers();
            Poll::Ready(e)
        } else {
            Poll::Pending
        }
    }
}

impl SharedTemporaryFileReader {
    /// Gets the (expected) size of the file to read.
    pub fn file_size(&self) -> FileSize {
        match self.sentinel.state.load() {
            State::Pending(size) => FileSize::AtLeast(size),
            State::Completed(size) => FileSize::Exactly(size),
            State::Failed => FileSize::Error,
        }
    }
}

/// The file size of the file to read.
#[derive(Debug, Copy, Clone)]
pub enum FileSize {
    /// The file is not entirely written yet. The specified amount is the minimum
    /// number known to exist.
    AtLeast(usize),
    /// The file is completely written and has exactly the specified amount of bytes.
    Exactly(usize),
    /// An error occurred while writing the file; reading may not complete.
    Error,
}

impl Sentinel {
    fn wake_readers(&self) {
        let mut lock = self
            .wakers
            .lock()
            .expect("failed to lock waker vector for writing");
        lock.drain().for_each(|(_id, w)| w.wake());
    }

    fn register_reader_waker(&self, id: Uuid, waker: &Waker) {
        let mut lock = self
            .wakers
            .lock()
            .expect("failed to lock waker vector for reading");

        lock.entry(id)
            .and_modify(|e| *e = waker.clone())
            .or_insert(waker.clone());
    }

    fn remove_reader_waker(&self, id: &Uuid) {
        let mut lock = self.wakers.lock().expect("failed to get lock for readers");
        lock.remove(id);
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

#[pinned_drop]
impl PinnedDrop for SharedTemporaryFileReader {
    fn drop(mut self: Pin<&mut Self>) {
        self.sentinel.remove_reader_waker(&self.id)
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
        Self::handle_poll_write_result(&this.sentinel, poll)
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
        let this = self.project();
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
        Self::handle_poll_write_result(&this.sentinel, poll)
    }

    fn is_write_vectored(&self) -> bool {
        self.file.is_write_vectored()
    }
}

impl AsyncRead for SharedTemporaryFileReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.project();
        match this.file.poll_read(cx, buf) {
            Poll::Ready(result) => {
                this.sentinel.remove_reader_waker(&this.id);
                Poll::Ready(result)
            }
            Poll::Pending => {
                this.sentinel
                    .register_reader_waker(this.id.clone(), cx.waker());
                Poll::Pending
            }
        }
    }
}
