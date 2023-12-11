pub trait BackendInfo {
    /// Gets a short name of the backend.
    fn backend_name() -> &'static str;

    /// Gets an informational string about the backend.
    fn backend_version() -> &'static str {
        ""
    }
}
