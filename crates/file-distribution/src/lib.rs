// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

mod file_hashes;
mod file_provider;
mod file_reader;
pub mod hash;
pub mod protobuf;
mod write_summary;

pub use file_hashes::FileHashes;
pub use file_provider::{FileAccessorError, FileProvider, GetFile, GetFileReaderError};
pub use file_reader::{BoxedFileReader, FileReaderTrait};
pub use write_summary::WriteSummary;
