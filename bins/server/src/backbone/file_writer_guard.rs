use crate::backbone::file_writer::{err_broken_pipe, FileWriter, FinalizationError};
use crate::backbone::CompletionMode;
use crate::metrics::transfer::{TransferMethod, TransferMetrics};
use file_distribution::WriteSummary;
use std::io::ErrorKind;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot::Sender;

/// A writer guard to communicate back to the [`Backbone`](crate::backbone::Backbone);
///
/// This exists to ensure that we can drop the [`FileWriter`] (e.g. if the HTTP request
/// is cancelled) and still have the [`Backbone`](crate::backbone::Backbone) informed
/// about it.
pub struct FileWriterGuard {
    /// The file writer; `None` when closed.
    inner: Option<FileWriter>,
    /// The sender to communicate with the backbone.
    sender: Option<Sender<WriteResult>>,
    /// The expiration time of this file.
    expiration: Duration,
    /// The actual file size as per bookkeeping.
    file_size: u64,
    /// The expected content size as per `Content-Length` header, in bytes.
    expected_size: Option<u64>,
    /// The expected MD5 hash of the content, as per `Content-MD5` header.
    expected_content_md5: Option<[u8; 16]>,
}

/// A write result.
#[derive(Debug)]
pub enum WriteResult {
    /// The writer succeeded.
    Success(Arc<WriteSummary>),
    /// The writer failed.
    Failed,
}

impl FileWriterGuard {
    pub fn new(
        writer: FileWriter,
        sender: Sender<WriteResult>,
        expiration: Duration,
        expected_size: Option<u64>,
        content_md5: Option<[u8; 16]>,
    ) -> Self {
        Self {
            inner: Some(writer),
            sender: Some(sender),
            expiration,
            file_size: 0,
            expected_size,
            expected_content_md5: content_md5,
        }
    }

    pub async fn write(&mut self, chunk: &[u8]) -> std::io::Result<usize> {
        if let Some(ref mut writer) = self.inner {
            let bytes_written = writer.write(chunk).await?;
            self.file_size += bytes_written as u64;

            TransferMetrics::track_bytes_transferred(TransferMethod::Store, bytes_written);

            // Ensure we don't store more bytes than anticipated.
            // This check only happens when we have a Content-Length header (or similar)
            // available.
            if let Some(expected_size) = self.expected_size {
                if self.file_size > expected_size {
                    self.fail_if_not_already_closed();
                    return Err(std::io::Error::new(
                        ErrorKind::UnexpectedEof,
                        "Attempted to write more bytes than announced",
                    ));
                }
            }

            Ok(bytes_written)
        } else {
            err_broken_pipe()
        }
    }

    pub async fn finalize(
        mut self,
        mode: CompletionMode,
    ) -> Result<Arc<WriteSummary>, FinalizationError> {
        if let Some(writer) = self.inner.take() {
            let summary = writer.finalize(mode, self.expiration).await?;

            // Verify the file length if possible.
            if let Some(expected_size) = self.expected_size {
                if self.file_size != expected_size {
                    self.fail_if_not_already_closed();
                    return Err(FinalizationError::InvalidFileLength(
                        self.file_size,
                        expected_size,
                    ));
                }
            }

            // Verify integrity if possible.
            if let Some(md5) = self.expected_content_md5 {
                if md5.ne(&summary.hashes.md5[..]) {
                    self.fail_if_not_already_closed();
                    return Err(FinalizationError::IntegrityCheckFailed(
                        hex::encode(md5),
                        hex::encode(&summary.hashes.md5[..]),
                    ));
                }
            }

            self.try_signal_success(&summary)?;
            Ok(summary)
        } else {
            Err(FinalizationError::BackboneCommunicationFailed)
        }
    }

    /// Signal a success to the backbone.
    fn try_signal_success(mut self, summary: &Arc<WriteSummary>) -> Result<(), FinalizationError> {
        // Send the hashes back to the backbone.
        match self.sender.take() {
            None => Err(FinalizationError::BackboneCommunicationFailed),
            Some(sender) => match sender.send(WriteResult::Success(summary.clone())) {
                Ok(_) => Ok(()),
                Err(_) => Err(FinalizationError::BackboneCommunicationFailed),
            },
        }
    }

    /// Signal a failure to the backbone.
    ///
    /// ## Remarks
    ///
    /// Since [`finalize`](Self::finalize) consumes self, this method
    /// cannot be used if the operation was successful. Likewise, since
    /// this method consumes self, [`finalize`](Self::finalize) cannot be
    /// called afterwards.
    fn fail_if_not_already_closed(&mut self) {
        self.sender
            .take()
            .and_then(move |s| s.send(WriteResult::Failed).ok());
    }
}

/// This ensures that accidentally dropping the guard does not leave
/// the backbone in an uninformed state.
impl Drop for FileWriterGuard {
    fn drop(&mut self) {
        self.fail_if_not_already_closed()
    }
}

impl Deref for FileWriterGuard {
    type Target = FileWriter;

    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().expect("failed to deref writer")
    }
}

impl DerefMut for FileWriterGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.as_mut().expect("failed to deref writer")
    }
}
