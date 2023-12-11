// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

mod file_hashes;
pub mod hash;
pub mod protobuf;
mod write_summary;

pub use file_hashes::FileHashes;
pub use write_summary::WriteSummary;
