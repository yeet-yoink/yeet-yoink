use crate::backbone::backbone::BackboneCommand;
use crate::backbone::file_writer_guard::WriteResult;
use axum::headers::ContentType;
use file_distribution::WriteSummary;
use shared_files::{SharedTemporaryFile, SharedTemporaryFileReader};
use shortguid::ShortGuid;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot::Receiver;
use tokio::sync::RwLock;
use tokio::time::Instant;
use tracing::{info, warn};

#[derive(Debug)]
pub(crate) struct FileRecord {
    /// The ID of the file.
    pub id: ShortGuid,
    /// The content type that was optionally specified during file creation.
    pub content_type: Option<ContentType>,
    /// The time when the file was created.
    pub created: Instant,
    /// The time after which the file will be inaccessible.
    pub expiration_duration: Duration,
    inner: Arc<RwLock<Inner>>,
}

#[derive(Debug)]
struct Inner {
    file: Option<SharedTemporaryFile>,
    summary: Option<Arc<WriteSummary>>,
}

impl FileRecord {
    pub fn new(
        id: ShortGuid,
        file: SharedTemporaryFile,
        backbone_command: Sender<BackboneCommand>,
        writer_command: Receiver<WriteResult>,
        duration: Duration,
        content_type: Option<ContentType>,
        created: Instant,
    ) -> Self {
        let inner = Arc::new(RwLock::new(Inner {
            file: Some(file),
            summary: None,
        }));
        let _ = tokio::spawn(Self::lifetime_handler(
            id,
            inner.clone(),
            backbone_command,
            writer_command,
            duration,
        ));
        Self {
            id,
            inner,
            content_type,
            created,
            expiration_duration: duration,
        }
    }

    /// Gets an additional reader for the file.
    pub async fn get_reader(&self) -> Result<SharedTemporaryFileReader, GetReaderError> {
        let inner = self.inner.read().await;
        match &inner.file {
            None => Err(GetReaderError::FileExpired(self.id)),
            Some(file) => Ok(file
                .reader()
                .await
                .map_err(|e| GetReaderError::FileError(self.id, e))?),
        }
    }

    /// Gets the file write summary or `None`, if the file writing hasn't completed yet.
    pub async fn get_summary(&self) -> Option<Arc<WriteSummary>> {
        let inner = self.inner.read().await;
        inner.summary.clone()
    }

    /// Controls the lifetime of the entry in the backbone.
    ///
    /// This method will:
    ///
    /// - Wait until the file is buffered to disk completely,
    /// - Apply a temporal lease to the file (keeping it alive for a certain time).
    /// - Remove the file from the registry after the time is over.
    async fn lifetime_handler(
        id: ShortGuid,
        mut inner: Arc<RwLock<Inner>>,
        backbone_command: Sender<BackboneCommand>,
        writer_command: Receiver<WriteResult>,
        duration: Duration,
    ) {
        // Before starting the timeout, wait for the write to the file to complete.
        let summary = match writer_command.await {
            Ok(WriteResult::Success(summary)) => {
                info!(file_id = %id, "File writing completed: {}", summary.hashes);
                summary
            }
            Ok(WriteResult::Failed) => {
                warn!(file_id = %id, "Writing to the file failed");
                Self::close_file(&mut inner).await;
                Self::remove_writer(id, backbone_command).await;
                return;
            }
            Err(e) => {
                warn!(file_id = %id, "The file writer channel failed: {e}");
                Self::close_file(&mut inner).await;
                Self::remove_writer(id, backbone_command).await;
                return;
            }
        };

        // Persist the write summary.
        {
            let mut inner = inner.write().await;
            inner.summary = Some(summary.clone());
        }

        // Indicate the file is ready for processing.
        if let Err(error) = backbone_command
            .send(BackboneCommand::ReadyForDistribution(id, summary))
            .await
        {
            warn!(file_id = %id, "The backbone writer channel was closed while indicating a termination for file with ID {id}: {error}");
            return;
        }

        // TODO: The lifetime handler also needs to listen to graceful shutdowns.
        //       If that's not the case, open file entries may keep the server
        //       alive even if the servers have already shut down.

        // Keep the file open for readers.
        Self::apply_temporal_lease(&id, duration).await;
        info!(file_id = %id, "Read lease timed out for file {id}; removing it");

        // Gracefully close the file.
        Self::remove_writer(id, backbone_command).await;
    }

    async fn apply_temporal_lease(id: &ShortGuid, duration: Duration) {
        info!(file_id = %id, "File {id} will accept new readers for {duration:?}");
        tokio::time::sleep(duration).await
    }

    async fn close_file(inner: &mut Arc<RwLock<Inner>>) {
        let mut inner = inner.write().await;
        inner.file.take();
    }

    async fn remove_writer(id: ShortGuid, backbone_command: Sender<BackboneCommand>) {
        if let Err(error) = backbone_command
            .send(BackboneCommand::RemoveWriter(id))
            .await
        {
            warn!(file_id = %id, "The backbone writer channel was closed while indicating a termination for file with ID {id}: {error}");
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GetReaderError {
    #[error("No file found for the specified ID {0}")]
    UnknownFile(ShortGuid),
    #[error("The file lease has expired for the specified ID {0}")]
    FileExpired(ShortGuid),
    #[error("Failed to open the file for ID {0}: {1}")]
    FileError(ShortGuid, async_tempfile::Error),
}
