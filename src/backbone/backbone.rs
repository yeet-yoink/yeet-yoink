use crate::backbone::file_record::FileRecord;
use crate::backbone::file_writer::FileWriter;
use crate::backbone::file_writer_guard::FileWriterGuard;
use async_tempfile::TempFile;
use axum::response::{IntoResponse, Response};
use hyper::StatusCode;
use shared_files::{SharedFileWriter, SharedTemporaryFile};
use std::borrow::Borrow;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::info;
use uuid::Uuid;

/// The duration for which to keep each file alive.
pub const TEMPORAL_LEASE: Duration = Duration::from_secs(5 * 60);

/// A local file distribution manager.
///
/// This instance keeps track of currently processed files.
pub struct Backbone {
    inner: Arc<RwLock<Inner>>,
    sender: mpsc::Sender<BackboneCommand>,
}

struct Inner {
    open: HashMap<Uuid, FileRecord>,
}

impl Backbone {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel(1024);
        let inner = Arc::new(RwLock::new(Inner {
            open: HashMap::default(),
        }));
        let _ = tokio::spawn(Self::command_loop(inner.clone(), receiver));
        Self { inner, sender }
    }

    /// Creates a new file buffer, registers it and returns a writer to it.
    pub async fn new_file(&self, id: Uuid) -> Result<FileWriterGuard, Error> {
        // We reuse the ID such that it is easier to find and debug the
        // created file if necessary.
        let file = Self::create_new_temporary_file(id).await?;
        let writer = Self::create_writer_for_file(&file).await?;

        let mut inner = self.inner.write().await;
        let (sender, receiver) = oneshot::channel();

        let temporal_lease = TEMPORAL_LEASE;

        // This needs to happen synchronously so that the moment we return the writer,
        // we know the entry exists.
        match inner.open.entry(id) {
            Entry::Occupied(_) => {
                // TODO: Actively mark the file as failed? This could invalidate all readers and writers.
                drop(writer);
                drop(file);
                return Err(Error::InternalErrorMayRetry);
            }
            Entry::Vacant(v) => v.insert(FileRecord::new(
                id,
                file,
                self.sender.clone(),
                receiver,
                temporal_lease,
            )),
        };

        let writer = FileWriter::new(&id, writer);
        Ok(FileWriterGuard::new(writer, sender, temporal_lease))
    }

    /// Requests to remove an entry.
    ///
    /// Currently open writers or readers will continue to work.
    /// When the last reference is closed, the file will be removed.
    ///
    /// However, no new readers can be created after this point.
    pub async fn remove<I: Borrow<Uuid>>(&self, id: I) {
        self.sender
            .send(BackboneCommand::RemoveWriter(id.borrow().clone()))
            .await
            .ok();
    }

    async fn create_new_temporary_file(id: Uuid) -> Result<SharedTemporaryFile, Error> {
        SharedTemporaryFile::new_with_uuid(id)
            .await
            .map_err(|e| Error::FailedCreatingFile(e))
    }

    async fn create_writer_for_file(
        file: &SharedTemporaryFile,
    ) -> Result<SharedFileWriter<TempFile>, Error> {
        file.writer()
            .await
            .map_err(|e| Error::FailedCreatingWriter(e))
    }

    async fn command_loop(inner: Arc<RwLock<Inner>>, mut channel: mpsc::Receiver<BackboneCommand>) {
        while let Some(command) = channel.recv().await {
            match command {
                BackboneCommand::RemoveWriter(id) => {
                    info!("Removing file {id} from bookkeeping");
                    let mut inner = inner.write().await;
                    inner.open.remove(id.borrow());
                }
                BackboneCommand::ReadyForDistribution(id) => {
                    info!("The file {id} was buffered completely and can now be distributed")
                }
            }
        }

        info!("The backbone command loop stopped");
    }
}

impl Default for Backbone {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub enum BackboneCommand {
    /// Removes an entry. This should only be called when there are no
    /// more open references to the file.
    ///
    /// Currently open writers or readers will continue to work.
    /// When the last reference is closed, the file will be removed.
    RemoveWriter(Uuid),
    /// Marks the file ready for distribution to other backends.
    ReadyForDistribution(Uuid),
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Failed to create the file: {0}")]
    FailedCreatingFile(async_tempfile::Error),
    #[error("Failed to create a writer to the file: {0}")]
    FailedCreatingWriter(async_tempfile::Error),
    #[error("An internal error occurred; the operation may be retried")]
    InternalErrorMayRetry,
}

impl From<Error> for Response {
    fn from(value: Error) -> Self {
        match value {
            Error::FailedCreatingFile(e) => {
                internal_server_error(format!("Failed to create temporary file: {e}"))
            }
            Error::FailedCreatingWriter(e) => internal_server_error(format!(
                "Failed to create a writer for the temporary file: {e}"
            )),
            Error::InternalErrorMayRetry => internal_server_error(format!(
                "Failed to create temporary file - ID already in use"
            )),
        }
    }
}

fn internal_server_error(message: String) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, message).into_response()
}
