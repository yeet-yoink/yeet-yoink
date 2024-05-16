use file_distribution::WriteSummary;
use shortguid::ShortGuid;
use std::sync::Arc;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::Sender;

pub enum BackendCommand {
    DistributeFile(ShortGuid, Arc<WriteSummary>),
}

pub struct BackendCommandSender {
    sender: Sender<BackendCommand>,
}

impl BackendCommandSender {
    pub async fn send(&self, command: BackendCommand) -> Result<(), BackendCommandSendError> {
        Ok(self.sender.send(command).await?)
    }
}

impl From<Sender<BackendCommand>> for BackendCommandSender {
    fn from(value: Sender<BackendCommand>) -> Self {
        Self { sender: value }
    }
}

#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct BackendCommandSendError(#[from] SendError<BackendCommand>);
