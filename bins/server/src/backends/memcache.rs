mod backend;
mod config;
mod connection_string;

pub use backend::{MemcacheBackend, MemcacheBackendConstructionError};
pub use config::MemcacheBackendConfig;
pub use connection_string::MemcacheConnectionString;
