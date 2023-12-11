// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

mod backend;
mod backend_command;
mod backend_info;
mod dyn_backend;
mod from_config;
mod registration;

pub use backend::{Backend, DistributionError};
pub use backend_command::{BackendCommand, BackendCommandSendError, BackendCommandSender};
pub use backend_info::BackendInfo;
pub use dyn_backend::DynBackend;
pub use from_config::TryCreateFromConfig;
pub use registration::{BackendRegistration, RegisterBackendError};
