//! A storage backend keeping each file as one object in an object store, such
//! as Google Cloud Storage or Azure Blob Storage. The cloud-specific crates
//! only build the store; storing and fetching files is shared.

// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

mod backend;
mod secret;

pub use backend::{
    METADATA_ATTRIBUTE, ObjectStoreBackend, ObjectStoreSettings, client_options, retry_config,
};
pub use secret::read_secret;
