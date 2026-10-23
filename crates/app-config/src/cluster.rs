use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

/// The default cluster ID; instances only gossip within the same cluster.
pub const DEFAULT_CLUSTER_ID: &str = "yeet-yoink";

/// The default port to gossip on.
pub const DEFAULT_GOSSIP_PORT: u16 = 7946;

fn default_cluster_id() -> String {
    DEFAULT_CLUSTER_ID.into()
}

fn default_listen_addr() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_GOSSIP_PORT)
}

const fn default_gossip_interval_ms() -> u64 {
    1000
}

/// Settings for finding and gossiping with other instances.
///
/// Gossip is encrypted and authenticated with a key derived from the secret
/// and the cluster ID; instances without the same secret are ignored.
#[derive(Debug, Serialize, Deserialize)]
pub struct ClusterConfig {
    /// Instances only gossip within the same cluster. Defaults to
    /// [`DEFAULT_CLUSTER_ID`].
    #[serde(default = "default_cluster_id")]
    pub cluster_id: String,
    /// A name unique within the cluster, e.g. the pod name. Defaults to the
    /// advertised gossip address.
    #[serde(default)]
    pub node_id: Option<String>,
    /// The local address to receive gossip on. Defaults to
    /// `0.0.0.0:7946`.
    #[serde(default = "default_listen_addr")]
    pub listen_addr: SocketAddr,
    /// The IP address other instances reach this one on, e.g. the pod IP.
    /// Required when listening on an unspecified address such as `0.0.0.0`.
    ///
    /// Gossip from an address other than the one an instance advertises is
    /// dropped, so instances must reach each other without NAT in between.
    #[serde(default)]
    pub advertise_ip: Option<IpAddr>,
    /// `host:port` pairs of instances to contact first. Host names are
    /// resolved again on every gossip round, so the name of a headless
    /// service reaches all of its pods; use its fully qualified name, e.g.
    /// `yeet-yoink-gossip.other-namespace.svc.cluster.local:7946`, to reach
    /// another namespace. One reachable seed suffices: instances learn of
    /// all others through gossip.
    #[serde(default)]
    pub seeds: Vec<String>,
    /// A file holding the shared secret, at least 32 bytes long, e.g. a
    /// mounted Kubernetes Secret.
    pub secret_file: PathBuf,
    /// How often to gossip, in milliseconds. Defaults to one second.
    #[serde(default = "default_gossip_interval_ms")]
    pub gossip_interval_ms: u64,
    /// The Kubernetes node this instance runs on.
    #[serde(default)]
    pub node_name: Option<String>,
    /// The availability zone this instance runs in.
    #[serde(default)]
    pub zone: Option<String>,
}

impl ClusterConfig {
    /// How often to gossip.
    pub fn gossip_interval(&self) -> Duration {
        Duration::from_millis(self.gossip_interval_ms)
    }

    /// The address other instances reach this instance's gossip on, or `None`
    /// if neither an advertised IP nor a specific listen address is set.
    pub fn advertise_addr(&self) -> Option<SocketAddr> {
        let ip = self.advertise_ip.or_else(|| {
            let ip = self.listen_addr.ip();
            (!ip.is_unspecified()).then_some(ip)
        })?;
        Some(SocketAddr::new(ip, self.listen_addr.port()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_config_uses_defaults() {
        let config: ClusterConfig =
            serde_yaml::from_str("secret_file: /etc/yy/secret").expect("Failed to deserialize");

        assert_eq!(config.cluster_id, DEFAULT_CLUSTER_ID);
        assert_eq!(config.listen_addr, "0.0.0.0:7946".parse().unwrap());
        assert_eq!(config.gossip_interval(), Duration::from_secs(1));
        assert!(config.seeds.is_empty());
        assert_eq!(config.node_id, None);
        // Listening on 0.0.0.0 tells others nothing about where to reach us.
        assert_eq!(config.advertise_addr(), None);
    }

    #[test]
    fn full_config_is_read() {
        let yaml = r#"
            cluster_id: production
            node_id: yeet-yoink-0
            listen_addr: "0.0.0.0:7000"
            advertise_ip: "10.1.2.3"
            seeds:
              - "yeet-yoink-gossip.team-a.svc.cluster.local:7000"
              - "yeet-yoink-gossip.team-b.svc.cluster.local:7000"
            secret_file: /etc/yy/secret
            gossip_interval_ms: 250
            node_name: worker-1
            zone: europe-west3-a
        "#;
        let config: ClusterConfig = serde_yaml::from_str(yaml).expect("Failed to deserialize");

        assert_eq!(config.cluster_id, "production");
        assert_eq!(config.node_id.as_deref(), Some("yeet-yoink-0"));
        assert_eq!(
            config.advertise_addr(),
            Some("10.1.2.3:7000".parse().unwrap())
        );
        assert_eq!(config.seeds.len(), 2);
        assert_eq!(config.gossip_interval(), Duration::from_millis(250));
        assert_eq!(config.node_name.as_deref(), Some("worker-1"));
        assert_eq!(config.zone.as_deref(), Some("europe-west3-a"));
    }

    #[test]
    fn specific_listen_address_is_advertised() {
        let config: ClusterConfig =
            serde_yaml::from_str("secret_file: /s\nlisten_addr: \"192.168.0.5:7946\"")
                .expect("Failed to deserialize");
        assert_eq!(
            config.advertise_addr(),
            Some("192.168.0.5:7946".parse().unwrap())
        );
    }

    #[test]
    fn secret_file_is_required() {
        let result: Result<ClusterConfig, _> = serde_yaml::from_str("cluster_id: x");
        assert!(result.is_err());
    }
}
