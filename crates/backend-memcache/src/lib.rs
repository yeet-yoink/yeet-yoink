// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

mod backend;
mod connection_string;

pub use backend::{MemcacheBackend, MemcacheBackendConstructionError};
