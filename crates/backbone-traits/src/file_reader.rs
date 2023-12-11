use file_distribution::WriteSummary;
use shared_files::FileSize;
use std::borrow::Cow;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::time::Instant;

pub trait FileReaderTrait: AsyncRead + Send + Unpin {
    fn summary(&self) -> &Option<Arc<WriteSummary>>;
    fn expiration_date(&self) -> Instant;
    fn file_size(&self) -> FileSize;
    fn file_age(&self) -> Duration;
    fn content_type(&self) -> Option<Cow<str>>;
}

pub struct BoxedFileReader(Box<dyn FileReaderTrait>);

impl FileReaderTrait for BoxedFileReader {
    fn summary(&self) -> &Option<Arc<WriteSummary>> {
        self.0.summary()
    }
    fn expiration_date(&self) -> Instant {
        self.0.expiration_date()
    }
    fn file_size(&self) -> FileSize {
        self.0.file_size()
    }
    fn file_age(&self) -> Duration {
        self.0.file_age()
    }
    fn content_type(&self) -> Option<Cow<str>> {
        self.0.content_type()
    }
}

impl AsyncRead for BoxedFileReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl BoxedFileReader {
    pub fn new<T>(value: T) -> Self
    where
        T: FileReaderTrait + 'static,
    {
        Self(Box::new(value))
    }
}
