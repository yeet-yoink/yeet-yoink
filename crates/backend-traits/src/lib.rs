// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

mod backend_command;
mod backend_info;
mod distribute_file;
mod from_config;
mod registration;

pub use backend_command::{BackendCommand, BackendCommandSendError, BackendCommandSender};
pub use backend_info::BackendInfo;
pub use distribute_file::{Backend, DistributeFile, DistributionError};
pub use from_config::TryCreateFromConfig;
pub use registration::{BackendRegistration, RegisterBackendError};
