use crate::CompletionMode;
use crate::file_reader::FileReader;
use crate::file_record::{FileOrigin, FileRecord, Lease};
use crate::file_writer::{FileWriter, FinalizationError, SynchronizationError};
use crate::file_writer_guard::{FileWriterGuard, WriteResult};
use async_tempfile::TempFile;
use axum_extra::headers::ContentType;
use backend_traits::{BackendCommand, BackendCommandSender, FetchedFile};
use file_distribution::{BoxedFileReader, FileHashes, GetFileReaderError, WriteSummary};
use rendezvous::RendezvousGuard;
use shared_files::{SharedFileWriter, SharedTemporaryFile};
use shortguid::ShortGuid;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::Sender;
use tokio::sync::{RwLock, mpsc, oneshot, watch};
use tokio::time::Instant;
use tracing::info;

/// The size of the chunks in which fetched files are buffered.
const CACHE_CHUNK_SIZE: usize = 64 * 1024;

/// A local file distribution manager.
///
/// This instance keeps track of currently processed files.
pub struct Backbone {
    inner: Arc<RwLock<Inner>>,
    sender: Sender<BackboneCommand>,
    /// The duration for which to keep each buffered file available to readers.
    temporal_lease: Duration,
    /// Signals pending file leases to end early when the backbone shuts down.
    shutdown: watch::Sender<bool>,
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

        // Detached; the cleanup rendezvous tracks when the loop finishes.
        tokio::spawn(Self::command_loop(
            inner.clone(),
            receiver,
            backend_sender,
            cleanup_rendezvous,
        ));
        Self {
            inner,
            sender,
            temporal_lease,
            shutdown: watch::Sender::new(false),
        }
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
        let registration = Registration {
            origin: FileOrigin::Upload,
            lease: self.temporal_lease,
        };
        self.register_file(
            id,
            expected_size,
            content_type,
            content_md5,
            file_name,
            registration,
        )
        .await
    }

    /// Buffers a file fetched from a backend and returns a reader to it.
    ///
    /// The content is streamed into a private buffer and checked against the
    /// size and hashes recorded with the file; nothing is visible to other
    /// readers until it passes. With `publish`, the verified file then stays
    /// available to subsequent reads (near cache) for the local lease, which
    /// never extends beyond the backend copy's expiry. It is not distributed to
    /// the backends again.
    ///
    /// If another fetch published the same file first, a reader to that copy is
    /// returned instead.
    pub async fn cache_file(
        &self,
        id: ShortGuid,
        mut file: FetchedFile,
        publish: bool,
    ) -> Result<BoxedFileReader, CacheFileError> {
        let content_type: Option<ContentType> =
            file.content_type.as_deref().and_then(|c| c.parse().ok());

        // A randomly named file, so that concurrent fetches of the same ID
        // do not collide.
        let buffer = SharedTemporaryFile::from(
            TempFile::new()
                .await
                .map_err(|e| NewFileError::FailedCreatingFile(id, e))?,
        );
        let mut writer = FileWriter::new(
            &id,
            Self::create_writer_for_file(id, &buffer).await?,
            file.file_name.take(),
            content_type.as_ref().map(ToString::to_string),
        );

        let mut chunk = vec![0; CACHE_CHUNK_SIZE];
        let mut written: u64 = 0;
        loop {
            let read = file.content.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            written += read as u64;
            if let Some(size) = file.size
                && written > size
            {
                return Err(CacheFileError::SizeMismatch {
                    expected: size,
                    actual: written,
                });
            }

            let mut remaining = &chunk[..read];
            while !remaining.is_empty() {
                let count = writer.write(remaining).await?;
                if count == 0 {
                    return Err(CacheFileError::Write(std::io::ErrorKind::WriteZero.into()));
                }
                remaining = &remaining[count..];
            }
        }

        if let Some(size) = file.size
            && written != size
        {
            return Err(CacheFileError::SizeMismatch {
                expected: size,
                actual: written,
            });
        }

        // The lease starts now, after the transfer, and must not outlast the
        // backend's copy.
        let retention = file.expires_at.map(|expires_at| {
            expires_at
                .duration_since(SystemTime::now())
                .unwrap_or_default()
        });
        let lease = retention.map_or(self.temporal_lease, |r| r.min(self.temporal_lease));

        writer.sync_data().await?;
        let summary = writer.finalize(CompletionMode::NoSync, lease).await?;
        if summary.hashes != file.hashes {
            return Err(CacheFileError::IntegrityCheckFailed {
                expected: file.hashes,
                actual: summary.hashes.clone(),
            });
        }

        // Open the reader before publishing, so that it stays valid even if
        // a short lease ends right away.
        let created = Instant::now();
        let reader = BoxedFileReader::new(FileReader::new(
            buffer
                .reader()
                .await
                .map_err(|e| GetFileReaderError::FileError(id, e))?,
            content_type.clone(),
            created,
            lease,
            Some(summary.clone()),
        ));

        if publish
            && !self
                .publish_cached_file(id, buffer, summary, content_type, lease, created)
                .await
        {
            // Another fetch published the file first; serve that copy, so that
            // subsequent reads see the same file.
            if let Ok(existing) = self.get_file(id).await {
                return Ok(existing);
            }
        }
        Ok(reader)
    }

    /// Registers a verified file fetched from a backend. Returns `false` if a
    /// file with the same ID is already registered.
    async fn publish_cached_file(
        &self,
        id: ShortGuid,
        buffer: SharedTemporaryFile,
        summary: Arc<WriteSummary>,
        content_type: Option<ContentType>,
        lease: Duration,
        created: Instant,
    ) -> bool {
        let mut inner = self.inner.write().await;
        let Entry::Vacant(entry) = inner.open.entry(id) else {
            return false;
        };

        let (sender, receiver) = oneshot::channel();
        let mut record = FileRecord::new(
            id,
            buffer,
            self.sender.clone(),
            receiver,
            Lease {
                duration: lease,
                shutdown: self.shutdown.subscribe(),
            },
            content_type,
            created,
        );
        record.origin = FileOrigin::Backend;
        record.summary_slot().set(summary.clone()).await;
        entry.insert(record);

        // The file is complete; start its lease right away.
        sender.send(WriteResult::Success(summary)).ok();
        true
    }

    async fn register_file(
        &self,
        id: ShortGuid,
        expected_size: Option<u64>,
        content_type: Option<ContentType>,
        content_md5: Option<[u8; 16]>,
        file_name: Option<String>,
        registration: Registration,
    ) -> Result<FileWriterGuard, NewFileError> {
        // We reuse the ID such that it is easier to find and debug the
        // created file if necessary.
        let content_type_name = content_type.as_ref().map(ToString::to_string);
        let file = Self::create_new_temporary_file(id).await?;
        let writer = Self::create_writer_for_file(id, &file).await?;

        let mut inner = self.inner.write().await;
        let (sender, receiver) = oneshot::channel();

        let temporal_lease = registration.lease;

        // This needs to happen synchronously so that the moment we return the writer,
        // we know the entry exists.
        let summary_slot = match inner.open.entry(id) {
            Entry::Occupied(_) => {
                // TODO: Actively mark the file as failed? This could invalidate all readers and writers.
                drop(writer);
                drop(file);
                return Err(NewFileError::InternalErrorMayRetry(id));
            }
            Entry::Vacant(v) => {
                let mut record = FileRecord::new(
                    id,
                    file,
                    self.sender.clone(),
                    receiver,
                    Lease {
                        duration: temporal_lease,
                        shutdown: self.shutdown.subscribe(),
                    },
                    content_type,
                    Instant::now(),
                );
                record.origin = registration.origin;
                let summary_slot = record.summary_slot();
                v.insert(record);
                summary_slot
            }
        };

        let writer = FileWriter::new(&id, writer, file_name, content_type_name);
        Ok(FileWriterGuard::new(
            writer,
            sender,
            summary_slot,
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
                    let origin = inner.read().await.open.get(&id).map(|file| file.origin);
                    if origin == Some(FileOrigin::Backend) {
                        info!(file_id = %id, "The file {id} was fetched from a backend and is not distributed again");
                        continue;
                    }

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

/// Ends all pending file leases so that the command loop, and with it the
/// cleanup rendezvous, can complete during a graceful shutdown.
impl Drop for Backbone {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
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

/// How a file is registered in the backbone.
struct Registration {
    /// Where the file came from.
    origin: FileOrigin,
    /// How long the file stays available to readers.
    lease: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum CacheFileError {
    #[error(transparent)]
    NewFile(#[from] NewFileError),
    #[error("Failed to transfer the file: {0}")]
    Write(#[from] std::io::Error),
    #[error("The content was {actual} bytes long, but {expected} bytes were announced")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("The content does not match the recorded hashes (expected {expected}, got {actual})")]
    IntegrityCheckFailed {
        expected: FileHashes,
        actual: FileHashes,
    },
    #[error(transparent)]
    Read(#[from] GetFileReaderError),
    #[error(transparent)]
    Sync(#[from] SynchronizationError),
    #[error(transparent)]
    Finalize(#[from] FinalizationError),
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
    async fn partial_writes_are_hashed_once() {
        use file_distribution::hash::{HashMd5, HashSha256};

        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();

        // Larger than the 2 MiB that tokio's `File` accepts per write, which forces
        // partial writes.
        let data: Vec<u8> = (0..5 * 1024 * 1024).map(|i| (i % 251) as u8).collect();

        // Retry the remainder of partial writes, as the `/yeet` handler does.
        let mut writer = new_file(&fixture.backbone, id, Some(data.len() as u64), None).await;
        let mut remaining = &data[..];
        while !remaining.is_empty() {
            let written = writer.write(remaining).await.expect("Failed to write");
            remaining = &remaining[written..];
        }
        writer.sync_data().await.expect("Failed to sync");
        let summary = writer
            .finalize(CompletionMode::NoSync)
            .await
            .expect("Failed to finalize");

        let mut md5 = HashMd5::new();
        md5.update(&data);
        let mut sha256 = HashSha256::new();
        sha256.update(&data);

        assert_eq!(summary.file_size_bytes, data.len());
        assert_eq!(summary.hashes.md5, md5.finalize());
        assert_eq!(summary.hashes.sha256, sha256.finalize());
    }

    #[tokio::test]
    async fn summary_is_recorded_once_finalized() {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();

        write_file(&fixture.backbone, id, b"abc")
            .await
            .expect("Failed to finalize");

        // Inspect the record without yielding, so the lifetime task cannot run first.
        let inner = fixture.backbone.inner.read().await;
        let record = inner.open.get(&id).expect("Missing file record");
        assert!(record.get_summary().await.is_some());
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

    /// Creates a fetched file whose hashes are those of `hashed`.
    fn fetched_file_hashed_as(data: &'static [u8], hashed: &[u8]) -> FetchedFile {
        FetchedFile {
            file_name: Some("cached.txt".into()),
            content_type: Some("text/plain".into()),
            hashes: file_distribution::FileHashes::of(hashed),
            size: Some(data.len() as u64),
            content: Box::new(data),
            expires_at: None,
        }
    }

    fn fetched_file(data: &'static [u8]) -> FetchedFile {
        fetched_file_hashed_as(data, data)
    }

    async fn read_to_end(mut reader: BoxedFileReader) -> Vec<u8> {
        use tokio::io::AsyncReadExt;
        let mut data = Vec::new();
        reader.read_to_end(&mut data).await.expect("Failed to read");
        data
    }

    /// A reader that yields some content and then fails.
    struct FailingReader(Option<&'static [u8]>);

    impl tokio::io::AsyncRead for FailingReader {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(match self.0.take() {
                Some(data) => {
                    buf.put_slice(data);
                    Ok(())
                }
                None => Err(std::io::ErrorKind::ConnectionReset.into()),
            })
        }
    }

    #[tokio::test]
    async fn cached_file_can_be_read_back() {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();

        let reader = fixture
            .backbone
            .cache_file(id, fetched_file(b"abc"), true)
            .await
            .expect("Failed to cache file");

        assert_eq!(read_to_end(reader).await, b"abc");
        assert_eq!(read_file(&fixture.backbone, id).await, b"abc");
    }

    #[tokio::test]
    async fn cached_file_carries_the_recorded_metadata() {
        use file_distribution::FileReaderTrait;

        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();

        let reader = fixture
            .backbone
            .cache_file(id, fetched_file(b"abc"), true)
            .await
            .expect("Failed to cache file");

        let summary = reader.summary().clone().expect("Missing summary");
        assert_eq!(summary.file_name.as_deref(), Some("cached.txt"));
        assert_eq!(summary.hashes, file_distribution::FileHashes::of(b"abc"));
        assert_eq!(reader.content_type().as_deref(), Some("text/plain"));
    }

    #[tokio::test]
    async fn unpublished_file_is_served_but_not_registered() {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();

        let reader = fixture
            .backbone
            .cache_file(id, fetched_file(b"abc"), false)
            .await
            .expect("Failed to cache file");

        assert!(is_unknown(&fixture.backbone, id).await);
        assert_eq!(read_to_end(reader).await, b"abc");
    }

    #[tokio::test]
    async fn cached_file_is_not_distributed_again() {
        let mut fixture = fixture(LONG_LEASE);
        let cached = ShortGuid::new_random();
        let uploaded = ShortGuid::new_random();

        fixture
            .backbone
            .cache_file(cached, fetched_file(b"abc"), true)
            .await
            .expect("Failed to cache file");
        write_file(&fixture.backbone, uploaded, b"def")
            .await
            .expect("Failed to finalize");

        // Only the uploaded file is handed to the backends.
        let command = timeout(TEST_TIMEOUT, fixture.backend_receiver.recv())
            .await
            .expect("No backend command received in time");
        assert!(matches!(
            command,
            Some(BackendCommand::DistributeFile(distributed_id, _)) if distributed_id == uploaded
        ));

        let next = timeout(Duration::from_millis(100), fixture.backend_receiver.recv()).await;
        assert!(next.is_err(), "The cached file was distributed");
    }

    #[tokio::test]
    async fn cached_file_does_not_outlive_the_backend_copy() {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();
        let file = FetchedFile {
            expires_at: Some(SystemTime::now() + Duration::from_millis(50)),
            ..fetched_file(b"abc")
        };

        fixture
            .backbone
            .cache_file(id, file, true)
            .await
            .expect("Failed to cache file");

        // The local lease is capped to the backend's retention instead of an hour.
        eventually(|| is_unknown(&fixture.backbone, id)).await;
    }

    #[tokio::test]
    async fn corrupt_cached_file_is_rejected() {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();

        let result = fixture
            .backbone
            .cache_file(id, fetched_file_hashed_as(b"abd", b"abc"), true)
            .await;

        assert!(matches!(
            result,
            Err(CacheFileError::IntegrityCheckFailed { .. })
        ));
        assert!(is_unknown(&fixture.backbone, id).await);
    }

    #[tokio::test]
    async fn content_of_the_wrong_length_is_rejected() {
        let fixture = fixture(LONG_LEASE);

        for announced in [2, 4] {
            let id = ShortGuid::new_random();
            let file = FetchedFile {
                size: Some(announced),
                ..fetched_file(b"abc")
            };

            let result = fixture.backbone.cache_file(id, file, true).await;

            assert!(
                matches!(result, Err(CacheFileError::SizeMismatch { expected, .. }) if expected == announced),
                "{announced} bytes announced"
            );
            assert!(is_unknown(&fixture.backbone, id).await);
        }
    }

    #[tokio::test]
    async fn content_of_unknown_length_is_accepted() {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();
        let file = FetchedFile {
            size: None,
            ..fetched_file(b"abc")
        };

        let reader = fixture
            .backbone
            .cache_file(id, file, true)
            .await
            .expect("Failed to cache file");
        assert_eq!(read_to_end(reader).await, b"abc");
    }

    #[tokio::test]
    async fn interrupted_transfer_is_rejected() {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();
        let file = FetchedFile {
            content: Box::new(FailingReader(Some(b"ab"))),
            ..fetched_file(b"abc")
        };

        let result = fixture.backbone.cache_file(id, file, true).await;

        assert!(matches!(result, Err(CacheFileError::Write(_))));
        assert!(is_unknown(&fixture.backbone, id).await);
    }

    #[tokio::test]
    async fn unverified_content_is_not_visible_while_buffering() {
        use tokio::io::AsyncWriteExt;

        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();
        let (mut sender, receiver) = tokio::io::duplex(16);
        let file = FetchedFile {
            content: Box::new(receiver),
            ..fetched_file(b"abc")
        };

        let (cached, ()) = tokio::join!(fixture.backbone.cache_file(id, file, true), async {
            sender.write_all(b"ab").await.expect("Failed to send");
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert!(
                is_unknown(&fixture.backbone, id).await,
                "Partial file visible"
            );

            sender.write_all(b"c").await.expect("Failed to send");
            drop(sender);
        });

        cached.expect("Failed to cache file");
        assert_eq!(read_file(&fixture.backbone, id).await, b"abc");
    }

    #[tokio::test]
    async fn concurrent_fetches_share_the_first_published_copy() {
        let fixture = fixture(LONG_LEASE);
        let id = ShortGuid::new_random();

        let first = fixture
            .backbone
            .cache_file(id, fetched_file(b"abc"), true)
            .await
            .expect("Failed to cache file");
        let second = fixture
            .backbone
            .cache_file(id, fetched_file(b"abc"), true)
            .await
            .expect("Failed to cache file");

        assert_eq!(read_to_end(first).await, b"abc");
        assert_eq!(read_to_end(second).await, b"abc");
        assert_eq!(read_file(&fixture.backbone, id).await, b"abc");
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
    async fn dropping_backbone_ends_pending_leases() {
        let (sender, _backend_receiver) = mpsc::channel(16);
        let rendezvous = Rendezvous::new();
        let backbone = Backbone::new(sender.into(), rendezvous.fork_guard(), LONG_LEASE);

        write_file(&backbone, ShortGuid::new_random(), b"abc")
            .await
            .expect("Failed to finalize");
        drop(backbone);

        // Without ending the lease, the command loop stays alive for its full duration.
        // Use the bounded wait: unlike a dropped `rendezvous_async`, it does not block
        // when the rendezvous is dropped after a timeout.
        let result = rendezvous.rendezvous_timeout_async(TEST_TIMEOUT).await;
        assert!(result.is_ok(), "A pending lease kept the backbone alive");
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
