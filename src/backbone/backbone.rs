use crate::backbone::writer::Writer;
use crate::backbone::writer_guard::{WriteResult, WriterGuard};
use async_tempfile::TempFile;
use axum::response::{IntoResponse, Response};
use hyper::StatusCode;
use shared_files::{SharedFileWriter, SharedTemporaryFile};
use std::borrow::Borrow;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use tokio::sync::oneshot::Receiver;
use tokio::sync::RwLock;
use uuid::Uuid;

/// A local file distribution manager.
///
/// This instance keeps track of currently processed files.
#[derive(Default)]
pub struct Backbone {
    // TODO: Add a temporal lease to the file.
    open: RwLock<HashMap<Uuid, FileRecord>>,
}

struct FileRecord {
    file: SharedTemporaryFile,
    // TODO: Do something to the record when the results come in or fail
    channel: Receiver<WriteResult>,
}

impl Backbone {
    /// Creates a new file buffer, registers it and returns a writer to it.
    pub async fn new_file(&self, id: Uuid) -> Result<WriterGuard, Error> {
        // We reuse the ID such that it is easier to find and debug the
        // created file if necessary.
        let file = Self::create_new_temporary_file(id).await?;
        let writer = Self::create_writer_for_file(&file).await?;

        let mut map = self.open.write().await;
        let (sender, receiver) = tokio::sync::oneshot::channel();

        match map.entry(id) {
            Entry::Occupied(_) => {
                // TODO: Actively mark the file as failed? This could invalidate all readers and writers.
                drop(writer);
                drop(file);
                return Err(Error::InternalErrorMayRetry);
            }
            Entry::Vacant(v) => v.insert(FileRecord {
                file,
                channel: receiver,
            }),
        };

        let writer = Writer::new(&id, writer);
        Ok(WriterGuard::new(writer, sender))
    }

    /// Removes an entry.
    ///
    /// Currently open writers or readers will continue to work.
    /// When the last reference is closed, the file will be removed.
    pub async fn remove<I: Borrow<Uuid>>(&self, id: I) {
        let mut map = self.open.write().await;
        map.remove(id.borrow());
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
