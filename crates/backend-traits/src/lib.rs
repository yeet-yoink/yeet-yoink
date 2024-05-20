// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

mod backend;
mod backend_command;
mod backend_info;
mod distribute_file;
mod from_config;
mod receive_file;
mod registration;

pub use backend::{Backend, BackendTag};
pub use backend_command::{
    BackendCommand, BackendCommandSendError, BackendCommandSender, FileReceiverPlaceholder,
};
pub use backend_info::BackendInfo;
pub use distribute_file::{DistributeFile, DistributionError};
pub use from_config::TryCreateFromConfig;
pub use receive_file::{ReceiveError, ReceiveFile};
pub use registration::{BackendRegistration, RegisterBackendError};
