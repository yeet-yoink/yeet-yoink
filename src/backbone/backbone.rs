use crate::backbone::file_reader::FileReader;
use crate::backbone::file_record::{FileRecord, GetReaderError};
use crate::backbone::file_writer::FileWriter;
use crate::backbone::file_writer_guard::FileWriterGuard;
use crate::backbone::WriteSummary;
use async_tempfile::TempFile;
use axum::headers::ContentType;
use axum::response::{IntoResponse, Response};
use hyper::StatusCode;
use shared_files::{SharedFileWriter, SharedTemporaryFile};
use shortguid::ShortGuid;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::time::Instant;
use tracing::info;

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
    open: HashMap<ShortGuid, FileRecord>,
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
    pub async fn new_file(
        &self,
        id: ShortGuid,
        expected_size: Option<u64>,
        content_type: Option<ContentType>,
        content_md5: Option<[u8; 16]>,
        file_name: Option<String>,
    ) -> Result<FileWriterGuard, NewFileError> {
        // We reuse the ID such that it is easier to find and debug the
        // created file if necessary.
        let file = Self::create_new_temporary_file(id).await?;
        let writer = Self::create_writer_for_file(id, &file).await?;

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
                return Err(NewFileError::InternalErrorMayRetry(id));
            }
            Entry::Vacant(v) => v.insert(FileRecord::new(
                id,
                file,
                self.sender.clone(),
                receiver,
                temporal_lease,
                content_type,
                Instant::now(),
            )),
        };

        let writer = FileWriter::new(&id, writer, file_name);
        Ok(FileWriterGuard::new(
            writer,
            sender,
            temporal_lease,
            expected_size,
            content_md5,
        ))
    }

    /// Creates a new file buffer, registers it and returns a writer to it.
    pub async fn get_file(&self, id: ShortGuid) -> Result<FileReader, GetReaderError> {
        let inner = self.inner.read().await;
        match inner.open.get(&id) {
            None => Err(GetReaderError::UnknownFile(id)),
            Some(file) => {
                let reader = file.get_reader().await?;
                Ok(FileReader::new(
                    reader,
                    file.content_type.clone(),
                    file.created,
                    file.expiration_duration,
                    file.get_summary().await,
                ))
            }
        }
    }

    async fn create_new_temporary_file(id: ShortGuid) -> Result<SharedTemporaryFile, NewFileError> {
        SharedTemporaryFile::new_with_uuid(id.into())
            .await
            .map_err(|e| NewFileError::FailedCreatingFile(id, e))
    }

    async fn create_writer_for_file(
        id: ShortGuid,
        file: &SharedTemporaryFile,
    ) -> Result<SharedFileWriter<TempFile>, NewFileError> {
        file.writer()
            .await
            .map_err(|e| NewFileError::FailedCreatingWriter(id, e))
    }

    async fn command_loop(inner: Arc<RwLock<Inner>>, mut channel: mpsc::Receiver<BackboneCommand>) {
        while let Some(command) = channel.recv().await {
            match command {
                BackboneCommand::RemoveWriter(id) => {
                    info!("Removing file {id} from bookkeeping");
                    let mut inner = inner.write().await;
                    inner.open.remove(&id);
                }
                BackboneCommand::ReadyForDistribution(id, _) => {
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
    RemoveWriter(ShortGuid),
    /// Marks the file ready for distribution to other backends.
    ReadyForDistribution(ShortGuid, Arc<WriteSummary>),
}

#[derive(Debug, thiserror::Error)]
pub enum NewFileError {
    #[error("Failed to create the file: {1}")]
    FailedCreatingFile(ShortGuid, async_tempfile::Error),
    #[error("Failed to create a writer to the file: {1}")]
    FailedCreatingWriter(ShortGuid, async_tempfile::Error),
    #[error("An internal error occurred; the operation may be retried")]
    InternalErrorMayRetry(ShortGuid),
}

impl From<NewFileError> for Response {
    fn from(value: NewFileError) -> Self {
        match value {
            NewFileError::FailedCreatingFile(id, e) => {
                problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("File not found")
                    .with_detail(format!("Failed to create temporary file: {e}"))
                    .with_value("id", id.to_string())
                    .with_value("error", e.to_string())
                    .into_response()
            }
            NewFileError::FailedCreatingWriter(id, e) => {
                problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("File not found")
                    .with_detail(format!(
                        "Failed to create a writer for the temporary file: {e}"
                    ))
                    .with_value("id", id.to_string())
                    .with_value("error", e.to_string())
                    .into_response()
            }
            NewFileError::InternalErrorMayRetry(id) => {
                problemdetails::new(StatusCode::INTERNAL_SERVER_ERROR)
                    .with_title("File not found")
                    .with_detail(format!(
                        "Failed to create temporary file - ID already in use"
                    ))
                    .with_value("id", id.to_string())
                    .into_response()
            }
        }
    }
}
