use axum::headers::ContentType;
use file_distribution::{FileReaderTrait, WriteSummary};
use metrics::transfer::{TransferMethod, TransferMetrics};
use shared_files::{FileSize, SharedTemporaryFileReader};
use std::borrow::Cow;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::time::Instant;

/// A read accessor for a temporary file.
pub struct FileReader {
    /// The file reader.
    inner: SharedTemporaryFileReader,
    content_type: Option<String>,
    created: Instant,
    expiration_duration: Duration,
    summary: Option<Arc<WriteSummary>>,
}

impl FileReader {
    pub fn new(
        reader: SharedTemporaryFileReader,
        content_type: Option<ContentType>,
        created: Instant,
        expiration_duration: Duration,
        summary: Option<Arc<WriteSummary>>,
    ) -> Self {
        Self {
            inner: reader,
            content_type: content_type.map(|c| c.to_string()),
            created,
            expiration_duration,
            summary,
        }
    }

    pub fn summary(&self) -> &Option<Arc<WriteSummary>> {
        &self.summary
    }

    pub fn expiration_date(&self) -> Instant {
        self.created + self.expiration_duration
    }

    pub fn file_size(&self) -> FileSize {
        self.inner.file_size()
    }

    pub fn file_age(&self) -> Duration {
        Instant::now() - self.created
    }

    pub fn content_type(&self) -> Option<Cow<str>> {
        self.content_type
            .as_ref()
            .map(|content_type| Cow::from(content_type.as_str()))
    }
}

impl FileReaderTrait for FileReader {
    fn summary(&self) -> &Option<Arc<WriteSummary>> {
        self.summary()
    }

    fn expiration_date(&self) -> Instant {
        self.expiration_date()
    }

    fn file_size(&self) -> FileSize {
        self.file_size()
    }

    fn file_age(&self) -> Duration {
        self.file_age()
    }

    fn content_type(&self) -> Option<Cow<str>> {
        self.content_type()
    }
}

impl AsyncRead for FileReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(read) => {
                let bytes_read = buf.filled().len();
                TransferMetrics::track_bytes_transferred(TransferMethod::Fetch, bytes_read);
                Poll::Ready(read)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
