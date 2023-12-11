// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

mod file_accessor;
mod file_reader;

pub use file_accessor::{FileAccessor, FileAccessorError, GetFileReaderError};
pub use file_reader::{BoxedFileReader, FileReaderTrait};
