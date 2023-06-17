use crate::backbone::writer_guard::WriteResult;
use shared_files::SharedTemporaryFile;
use std::sync::Arc;
use tokio::sync::oneshot::Receiver;

#[derive(Debug)]
pub(crate) struct FileRecord {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    file: SharedTemporaryFile,
    // TODO: Do something to the record when the results come in or fail
    channel: Receiver<WriteResult>,
}

impl FileRecord {
    pub fn new(file: SharedTemporaryFile, channel: Receiver<WriteResult>) -> Self {
        let inner = Inner { file, channel };
        Self {
            inner: Arc::new(inner),
        }
    }
}
