use crate::backbone::BackboneCommand;
use crate::file_writer_guard::WriteResult;
use axum_extra::headers::ContentType;
use file_distribution::{GetFileReaderError, WriteSummary};
use shared_files::{SharedTemporaryFile, SharedTemporaryFileReader};
use shortguid::ShortGuid;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot::Receiver;
use tokio::sync::watch;
use tokio::time::Instant;
use tracing::{info, warn};

/// The read lease of a file: how long it stays available to readers, unless the
/// backbone shuts down first.
pub(crate) struct Lease {
    pub duration: Duration,
    pub shutdown: watch::Receiver<bool>,
}

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
        lease: Lease,
        content_type: Option<ContentType>,
        created: Instant,
    ) -> Self {
        let expiration_duration = lease.duration;
        let inner = Arc::new(RwLock::new(Inner {
            file: Some(file),
            summary: None,
        }));
        // The spawned task is detached intentionally; its lifetime is managed
        // through the backbone command channel and the rendezvous guard.
        tokio::spawn(Self::lifetime_handler(
            id,
            inner.clone(),
            backbone_command,
            writer_command,
            lease,
        ));
        Self {
            id,
            inner,
            content_type,
            created,
            expiration_duration,
        }
    }

    /// Gets an additional reader for the file.
    pub async fn get_reader(&self) -> Result<SharedTemporaryFileReader, GetFileReaderError> {
        let inner = self.inner.read().await;
        match &inner.file {
            None => Err(GetFileReaderError::FileExpired(self.id)),
            Some(file) => Ok(file
                .reader()
                .await
                .map_err(|e| GetFileReaderError::FileError(self.id, e))?),
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
    /// - Remove the file from the registry after the time is over, or earlier
    ///   if the backbone shuts down.
    async fn lifetime_handler(
        id: ShortGuid,
        mut inner: Arc<RwLock<Inner>>,
        backbone_command: Sender<BackboneCommand>,
        writer_command: Receiver<WriteResult>,
        lease: Lease,
    ) {
        let Lease {
            duration,
            mut shutdown,
        } = lease;

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

        // Keep the file open for readers. The lease ends early on shutdown, since
        // this task holds a backbone sender that would otherwise keep the server
        // alive until the lease expires.
        tokio::select! {
            _ = Self::apply_temporal_lease(&id, duration) => {
                info!(file_id = %id, "Read lease timed out for file {id}; removing it");
            }
            _ = shutdown.wait_for(|&stop| stop) => {
                info!(file_id = %id, "Shutting down; ending read lease for file {id}");
            }
        }

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
