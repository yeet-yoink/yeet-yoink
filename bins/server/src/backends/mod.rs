pub use crate::backends::registry::{BackendRegistry, TryCreateFromConfig};

#[cfg_attr(docsrs, doc(cfg(feature = "memcache")))]
#[cfg(feature = "memcache")]
pub mod memcache;
mod registry;
