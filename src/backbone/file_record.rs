use crate::backbone::writer_guard::WriteResult;
use shared_files::SharedTemporaryFile;
use std::sync::Arc;
use tokio::sync::oneshot::Receiver;
use tracing::{info, warn};

#[derive(Debug)]
pub(crate) struct FileRecord {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    file: SharedTemporaryFile,
}

impl FileRecord {
    pub fn new(file: SharedTemporaryFile, channel: Receiver<WriteResult>) -> Self {
        let inner = Arc::new(Inner { file });
        let _ = tokio::spawn(Self::processing_loop(inner.clone(), channel));
        Self { inner }
    }

    async fn processing_loop(inner: Arc<Inner>, mut channel: Receiver<WriteResult>) {
        if let Ok(result) = channel.await {
            match result {
                WriteResult::Success(hashes) => info!("File writing completed: {}", hashes),
                WriteResult::Failed => warn!("Writing to the file failed"),
            }
        }

        info!("The file is about to go out of existence")
    }
}
