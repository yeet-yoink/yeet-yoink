use crate::file_reader::FileReader;
use crate::file_record::FileRecord;
use crate::file_writer::FileWriter;
use crate::file_writer_guard::FileWriterGuard;
use async_tempfile::TempFile;
use axum_extra::headers::ContentType;
use backend_traits::{BackendCommand, BackendCommandSender};
use file_distribution::{BoxedFileReader, GetFileReaderError, WriteSummary};
use rendezvous::RendezvousGuard;
use shared_files::{SharedFileWriter, SharedTemporaryFile};
use shortguid::ShortGuid;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio::sync::{RwLock, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::info;

/// A local file distribution manager.
///
/// This instance keeps track of currently processed files.
pub struct Backbone {
    inner: Arc<RwLock<Inner>>,
    sender: Sender<BackboneCommand>,
    loop_handle: JoinHandle<()>,
    /// The duration for which to keep each buffered file available to readers.
    temporal_lease: Duration,
}

struct Inner {
    open: HashMap<ShortGuid, FileRecord>,
}

impl Backbone {
    pub fn new(
        backend_sender: BackendCommandSender,
        cleanup_rendezvous: RendezvousGuard,
        temporal_lease: Duration,
    ) -> Self {
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
            temporal_lease,
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

        let temporal_lease = self.temporal_lease;

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
        match inner.open.get(&id) {
            None => Err(GetFileReaderError::UnknownFile(id)),
            Some(file) => {
                let reader = file.get_reader().await?;
                let reader = FileReader::new(
                    reader,
                    file.content_type.clone(),
                    file.created,
                    file.expiration_duration,
                    file.get_summary().await,
                );
                Ok(BoxedFileReader::new(reader))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CompletionMode;
    use crate::file_writer::FinalizationError;
    use rendezvous::Rendezvous;
    use tokio::io::AsyncReadExt;
    use tokio::time::timeout;

    const LONG_LEASE: Duration = Duration::from_secs(3600);
    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    struct Fixture {
        backbone: Backbone,
        backend_receiver: mpsc::Receiver<BackendCommand>,
    }

    fn fixture(lease: Duration) -> Fixture {
        let (sender, backend_receiver) = mpsc::channel(16);
        let rendezvous = Rendezvous::new();
        let backbone = Backbone::new(sender.into(), rendezvous.fork_guard(), lease);

        // Dropping a Rendezvous blocks until all guards complete, which would hang
        // the test while file leases are still pending.
        std::mem::forget(rendezvous);

        Fixture {
            backbone,
            backend_receiver,
        }
    }

    async fn write_file(
        backbone: &Backbone,
        id: ShortGuid,
        data: &[u8],
    ) -> Result<Arc<WriteSummary>, FinalizationError> {
        let mut writer = new_file(backbone, id, Some(data.len() as u64), None).await;
        write_and_sync(&mut writer, data).await;
        writer.finalize(CompletionMode::NoSync).await
    }

    /// Writes and syncs a chunk, mirroring the `/yeet` handler. Completing a writer
    /// with unsynced bytes trips a debug assertion in `shared-files`.
    async fn write_and_sync(writer: &mut FileWriterGuard, data: &[u8]) {
        writer.write(data).await.expect("Failed to write");
        writer.sync_data().await.expect("Failed to sync");
    }

    async fn new_file(
        backbone: &Backbone,
        id: ShortGuid,
        expected_size: Option<u64>,
        content_md5: Option<[u8; 16]>,
    ) -> FileWriterGuard {
        match backbone
            .new_file(id, expected_size, None, content_md5, None)
            .await
        {
            Ok(writer) => writer,
            Err(e) => panic!("Failed to create file: {e}"),
        }
    }

    async fn read_file(backbone: &Backbone, id: ShortGuid) -> Vec<u8> {
        let mut reader = match backbone.get_file(id).await {
            Ok(reader) => reader,
            Err(e) => panic!("Failed to get file: {e}"),
        };
        let mut buffer = Vec::new();
        reader
            .read_to_end(&mut buffer)
            .await
            .expect("Failed to read");
        buffer
    }

    /// Polls `condition` until it holds or the test timeout elapses.
    async fn eventually<F, Fut>(mut condition: F)
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = bool>,
    {
        timeout(TEST_TIMEOUT, async {
            while !condition().await {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("Condition not met in time");
    }

    async fn is_unknown(backbone: &Backbone, id: ShortGuid) -> bool {
        matches!(
            backbone.get_file(id).await,
            Err(GetFileReaderError::UnknownFile(unknown)) if unknown == id
        )
    }

    #[tokio::test]
    async fn written_file_can_be_read_back() {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();

        let summary = write_file(&fixture.backbone, id, b"abc")
            .await
            .expect("Failed to finalize");

        assert_eq!(summary.file_size_bytes, 3);
        assert_eq!(
            format!("{:x}", summary.hashes.md5),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert_eq!(read_file(&fixture.backbone, id).await, b"abc");
    }

    #[tokio::test]
    async fn finalized_file_is_sent_for_distribution() {
        let mut fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();

        write_file(&fixture.backbone, id, b"abc")
            .await
            .expect("Failed to finalize");

        let command = timeout(TEST_TIMEOUT, fixture.backend_receiver.recv())
            .await
            .expect("No backend command received in time");
        let Some(BackendCommand::DistributeFile(distributed_id, summary)) = command else {
            panic!("Backend channel closed unexpectedly");
        };
        assert_eq!(distributed_id, id);
        assert_eq!(summary.file_size_bytes, 3);
    }

    #[tokio::test]
    async fn unknown_file_is_reported() {
        let fixture = fixture(LONG_LEASE);
        assert!(is_unknown(&fixture.backbone, ShortGuid::new_random()).await);
    }

    #[tokio::test]
    async fn duplicate_id_is_rejected() {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();

        let _writer = new_file(&fixture.backbone, id, None, None).await;
        let result = fixture.backbone.new_file(id, None, None, None, None).await;

        assert!(matches!(
            result,
            Err(NewFileError::InternalErrorMayRetry(duplicate)) if duplicate == id
        ));
    }

    #[tokio::test]
    async fn short_write_fails_finalization() {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();

        let mut writer = new_file(&fixture.backbone, id, Some(5), None).await;
        write_and_sync(&mut writer, b"abc").await;
        let result = writer.finalize(CompletionMode::NoSync).await;

        assert!(matches!(
            result,
            Err(FinalizationError::InvalidFileLength(3, 5))
        ));
        eventually(|| is_unknown(&fixture.backbone, id)).await;
    }

    #[tokio::test]
    async fn oversized_write_is_rejected() {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();

        let mut writer = new_file(&fixture.backbone, id, Some(2), None).await;
        let error = writer
            .write(b"abc")
            .await
            .expect_err("Write should have failed");

        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);

        // The rejected chunk was written but never synced; dropping the writer must
        // neither panic nor keep the file around.
        drop(writer);
        eventually(|| is_unknown(&fixture.backbone, id)).await;
    }

    #[tokio::test]
    async fn md5_mismatch_fails_finalization() {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();

        let mut writer = new_file(&fixture.backbone, id, None, Some([0u8; 16])).await;
        write_and_sync(&mut writer, b"abc").await;
        let result = writer.finalize(CompletionMode::NoSync).await;

        assert!(matches!(
            result,
            Err(FinalizationError::IntegrityCheckFailed(..))
        ));
        eventually(|| is_unknown(&fixture.backbone, id)).await;
    }

    #[tokio::test]
    async fn matching_md5_passes_finalization() {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();
        let md5 = hex::decode("900150983cd24fb0d6963f7d28e17f72")
            .expect("Invalid hex")
            .try_into()
            .expect("Invalid MD5 length");

        let mut writer = new_file(&fixture.backbone, id, None, Some(md5)).await;
        write_and_sync(&mut writer, b"abc").await;
        let result = writer.finalize(CompletionMode::NoSync).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn aborted_upload_removes_file() {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();

        // Simulate an upload cancelled mid-write: bytes were written but not synced.
        let mut writer = new_file(&fixture.backbone, id, None, None).await;
        writer.write(b"abc").await.expect("Failed to write");
        drop(writer);

        eventually(|| is_unknown(&fixture.backbone, id)).await;
    }

    #[tokio::test]
    async fn aborted_upload_fails_open_readers() -> Result<(), GetFileReaderError> {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();

        let mut writer = new_file(&fixture.backbone, id, None, None).await;
        write_and_sync(&mut writer, b"abc").await;
        let mut reader = fixture.backbone.get_file(id).await?;
        drop(writer);

        // The reader must not mistake the truncated upload for a complete file.
        let mut buffer = Vec::new();
        let error = timeout(TEST_TIMEOUT, reader.read_to_end(&mut buffer))
            .await
            .expect("Reader timed out")
            .expect_err("Reader should observe the aborted upload");
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        Ok(())
    }

    #[tokio::test]
    async fn file_is_removed_after_lease_expires() {
        let fixture = fixture(Duration::from_millis(50));
        let id = ShortGuid::new_random();

        write_file(&fixture.backbone, id, b"abc")
            .await
            .expect("Failed to finalize");

        eventually(|| is_unknown(&fixture.backbone, id)).await;
    }
}
