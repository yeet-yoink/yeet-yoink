use crate::backbone::file_writer::{err_broken_pipe, FileWriter, FinalizationError, WriteSummary};
use crate::backbone::CompletionMode;
use std::cell::Cell;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot::Sender;
use tokio::time::Instant;

/// A writer guard to communicate back to the [`Backbone`](crate::backbone::Backbone);
///
/// This exists to ensure that we can drop the [`FileWriter`] (e.g. if the HTTP request
/// is cancelled) and still have the [`Backbone`](crate::backbone::Backbone) informed
/// about it.
pub struct FileWriterGuard {
    inner: Option<FileWriter>,
    sender: Option<Sender<WriteResult>>,
    expiration: Duration,
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
    pub fn new(writer: FileWriter, sender: Sender<WriteResult>, expiration: Duration) -> Self {
        Self {
            inner: Some(writer),
            sender: Some(sender),
            expiration,
        }
    }

    pub async fn write(&mut self, chunk: &[u8]) -> std::io::Result<usize> {
        if let Some(ref mut writer) = self.inner {
            writer.write(chunk).await
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
