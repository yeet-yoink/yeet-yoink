use axum::headers::ContentType;
use shared_files::{FileSize, SharedTemporaryFileReader};
use std::borrow::Cow;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, ReadBuf};

/// A read accessor for a temporary file.
pub struct FileReader {
    /// The file reader.
    inner: SharedTemporaryFileReader,
    content_type: Option<String>,
}

impl FileReader {
    pub fn new(reader: SharedTemporaryFileReader, content_type: Option<ContentType>) -> Self {
        Self {
            inner: reader,
            content_type: content_type.map(|c| c.to_string()),
        }
    }

    pub fn file_size(&self) -> FileSize {
        self.inner.file_size()
    }

    pub fn content_type(&self) -> Option<Cow<str>> {
        match &self.content_type {
            None => None,
            Some(content_type) => Some(Cow::from(content_type.as_str())),
        }
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
                // TODO: Increment metrics for reading from the file
                Poll::Ready(read)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
