use crate::file_reader::FileReader;
use crate::file_record::FileRecord;
use crate::file_writer::FileWriter;
use crate::file_writer_guard::FileWriterGuard;
use async_tempfile::TempFile;
use axum::headers::ContentType;
use backend_traits::{BackendCommand, BackendCommandSender};
use file_distribution::{BoxedFileReader, GetFileReaderError, WriteSummary};
use rendezvous::RendezvousGuard;
use shared_files::{SharedFileWriter, SharedTemporaryFile};
use shortguid::ShortGuid;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{channel, Sender};
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::info;

/// The duration for which to keep each file alive.
// pub const TEMPORAL_LEASE: Duration = Duration::from_secs(5 * 60); // TODO: #61 Make local storage duration configurable
pub const TEMPORAL_LEASE: Duration = Duration::from_secs(10);

/// A local file distribution manager.
///
/// This instance keeps track of currently processed files.
pub struct Backbone {
    inner: Arc<RwLock<Inner>>,
    sender: Sender<BackboneCommand>,
    loop_handle: JoinHandle<()>,
}

struct Inner {
    open: HashMap<ShortGuid, FileRecord>,
}

impl Backbone {
    pub fn new(backend_sender: BackendCommandSender, cleanup_rendezvous: RendezvousGuard) -> Self {
        let (sender, receiver) = mpsc::channel(1024);
        let inner = Arc::new(RwLock::new(Inner {
            open: HashMap::default(),
        }));

        let loop_handle = tokio::spawn(Self::command_loop(
            inner.clone(),
            receiver,
            backend_sender,
            cleanup_rendezvous,
        ));
        Self {
            inner,
            sender,
            loop_handle,
        }
    }

    pub async fn join(self) {
        self.loop_handle.await.ok();
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
    pub async fn get_file(&self, id: ShortGuid) -> Result<BoxedFileReader, GetFileReaderError> {
        let inner = self.inner.read().await;
        if let Some(file) = inner.open.get(&id) {
            let reader = file.get_reader().await?;
            let reader = FileReader::new(
                reader,
                file.content_type.clone(),
                file.created,
                file.expiration_duration,
                file.get_summary().await,
            );
            return Ok(BoxedFileReader::new(reader));
        }

        // TODO: #54 Query the backend registry for remote files
        let (tx, _rx) = channel(1);
        self.sender
            .send(BackboneCommand::ReceiveFile(id, tx))
            .await
            .map_err(|e| GetFileReaderError::InternalError(id, e.into()))?;

        // TODO: Handle channel

        // TODO: #58 Have remote backends reply in order of priority
        Err(GetFileReaderError::UnknownFile(id))
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

    async fn command_loop(
        inner: Arc<RwLock<Inner>>,
        mut channel: mpsc::Receiver<BackboneCommand>,
        backend_sender: BackendCommandSender,
        cleanup_rendezvous: RendezvousGuard,
    ) {
        while let Some(command) = channel.recv().await {
            match command {
                BackboneCommand::RemoveWriter(id) => {
                    info!(file_id = %id, "Removing file {id} from bookkeeping");
                    let mut inner = inner.write().await;
                    inner.open.remove(&id);
                }
                BackboneCommand::ReadyForDistribution(id, summary) => {
                    info!(file_id = %id, "The file {id} was buffered completely and can now be distributed");
                    backend_sender
                        .send(BackendCommand::DistributeFile(id, summary))
                        .await
                        .ok();
                }
                BackboneCommand::ReceiveFile(id, sender) => {
                    info!(file_id = %id, "The file {id} was not available locally and is now requested from the registered backends");
                    backend_sender
                        .send(BackendCommand::ReceiveFile(id, sender))
                        .await
                        .ok();
                }
            }
        }

        info!("The backbone command loop stopped");
        cleanup_rendezvous.completed();
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
    /// Downloads a file.
    ReceiveFile(ShortGuid, Sender<BoxedFileReader>),
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
