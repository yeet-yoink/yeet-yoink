// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

mod backbone;
mod file_accessor;
mod file_reader;
mod file_record;
mod file_writer;
mod file_writer_guard;

pub use backbone::Backbone;
pub use file_accessor::FileAccessorBridge;
pub use file_reader::FileReader;
pub use file_writer::CompletionMode;
