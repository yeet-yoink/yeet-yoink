#[cfg_attr(docsrs, doc(cfg(feature = "memcache")))]
#[cfg(feature = "memcache")]
pub mod memcache;

pub trait Backend: Send + Sync {
    /// Gets an informational string about the backend.
    fn backend_info(&self) -> &str;

    /// Gets the tag of the backend.
    fn tag(&self) -> &str;
}
