use crate::backbone::file_reader::FileReader;
use crate::backbone::{Backbone, GetReaderError};
use axum::async_trait;
use shortguid::ShortGuid;
use std::borrow::Borrow;
use std::sync::{Arc, RwLock, Weak};
use tracing::trace;

#[async_trait]
pub trait FileAccessor: Sync + Send {
    async fn get_file(&self, id: ShortGuid) -> Result<FileReader, FileAccessorError>;
}

#[derive(Default)]
pub struct FileAccessorBridge {
    backbone: RwLock<Weak<Backbone>>,
}

impl FileAccessorBridge {
    pub fn set_backbone<B>(&self, backbone: B)
    where
        B: Borrow<Arc<Backbone>>,
    {
        let mut instance = self.backbone.write().expect("lol");
        *instance = Arc::downgrade(backbone.borrow());
    }

    fn get_backbone(&self) -> Result<Arc<Backbone>, GetBackboneError> {
        let instance = self
            .backbone
            .read()
            .map_err(|_| GetBackboneError::FailedToLock)?;
        if let Some(backbone) = instance.upgrade() {
            trace!("Registered backbone instance with file accessor bridge");
            Ok(backbone)
        } else {
            Err(GetBackboneError::BackboneUnavailable)
        }
    }
}

#[async_trait]
impl FileAccessor for FileAccessorBridge {
    async fn get_file(&self, id: ShortGuid) -> Result<FileReader, FileAccessorError> {
        match self.get_backbone() {
            Ok(backbone) => Ok(backbone.get_file(id).await?),
            Err(GetBackboneError::BackboneUnavailable) => {
                Err(FileAccessorError::BackboneUnavailable)
            }
            Err(GetBackboneError::FailedToLock) => Err(FileAccessorError::FailedToLock),
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum GetBackboneError {
    #[error("The backbone is unavailable")]
    BackboneUnavailable,
    #[error("Unable to obtain a lock on a mutex")]
    FailedToLock,
}

#[derive(Debug, thiserror::Error)]
pub enum FileAccessorError {
    #[error("The backbone is unavailable")]
    BackboneUnavailable,
    #[error("Unable to obtain a lock on a mutex")]
    FailedToLock,
    #[error(transparent)]
    GetReaderError(#[from] GetReaderError),
}
