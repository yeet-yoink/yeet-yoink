// only enables the `doc_cfg` feature when
// the `docsrs` configuration attribute is defined
#![cfg_attr(docsrs, feature(doc_cfg))]

mod membership;
mod transport;

pub use membership::{Cluster, ClusterSettings, Peer, PeerInfo, PeerWatcher};
pub use transport::{ClusterKey, ClusterKeyError, MAX_MESSAGE_AGE, SecureUdpTransport};
