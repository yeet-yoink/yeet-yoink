mod dyn_backend;

use async_trait::async_trait;
use backbone_traits::{FileAccessor, FileAccessorError};
pub use dyn_backend::DynBackend;
use file_distribution::WriteSummary;
use shortguid::ShortGuid;
use std::error::Error;
use std::sync::Arc;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::Sender;

#[async_trait]
pub trait Backend: Send + Sync {
    /// Gets the tag of the backend.
    fn tag(&self) -> &str;

    /// Handles a file that is ready for distribution.
    async fn distribute_file(
        &self,
        id: ShortGuid,
        summary: Arc<WriteSummary>,
        file_accessor: Arc<dyn FileAccessor>,
    ) -> Result<(), DistributionError>;
}

pub enum BackendCommand {
    DistributeFile(ShortGuid, Arc<WriteSummary>),
}

pub struct BackendCommandSender {
    sender: Sender<BackendCommand>,
}

impl BackendCommandSender {
    pub async fn send(&self, command: BackendCommand) -> Result<(), CommandSendError> {
        Ok(self.sender.send(command).await?)
    }
}

impl From<Sender<BackendCommand>> for BackendCommandSender {
    fn from(value: Sender<BackendCommand>) -> Self {
        Self { sender: value }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DistributionError {
    #[error(transparent)]
    BackendSpecific(Box<dyn Error>),
    #[error(transparent)]
    FileAccessor(#[from] FileAccessorError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
}

#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct CommandSendError(#[from] SendError<BackendCommand>);
