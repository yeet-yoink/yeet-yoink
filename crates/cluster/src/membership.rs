use crate::transport::{ClusterKey, SecureUdpTransport};
use chitchat::{
    ChitchatConfig, ChitchatHandle, ChitchatId, FailureDetectorConfig, NodeState, spawn_chitchat,
};
use std::collections::BTreeMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::debug;

/// The key under which an instance publishes its HTTP address.
const KEY_HTTP_ADDR: &str = "http_addr";
/// The key under which an instance publishes its Kubernetes node.
const KEY_NODE_NAME: &str = "node_name";
/// The key under which an instance publishes its availability zone.
const KEY_ZONE: &str = "zone";
/// The key an instance sets when it shuts down gracefully, so that others stop
/// counting it right away instead of waiting for the failure detector.
const KEY_LEAVING: &str = "leaving";

/// How many gossip rounds a leaving instance waits for others to learn of it.
const LEAVE_ANNOUNCEMENT_ROUNDS: u32 = 2;

/// How long a node that stopped gossiping is remembered before it is forgotten.
/// Pods come and go, so this is far shorter than chitchat's default of a day.
const DEAD_NODE_GRACE_PERIOD: Duration = Duration::from_secs(3600);

/// How long deleted keys are retained so that all peers learn of the deletion.
const DELETION_GRACE_PERIOD: Duration = Duration::from_secs(3600);

/// How often seed host names are resolved while an instance starts.
///
/// chitchat resolves them only once a minute. Pods that start together are
/// often missing from each other's first DNS answer, which would leave them
/// apart for up to that long.
const FAST_SEED_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

/// How long after joining seeds are resolved at the faster rate.
const FAST_SEED_REFRESH_DURATION: Duration = Duration::from_secs(120);

/// Resolves a `host:port` seed to addresses.
type Resolver =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Vec<SocketAddr>> + Send>> + Send + Sync>;

fn dns_resolver() -> Resolver {
    Arc::new(|seed| {
        Box::pin(async move {
            match tokio::net::lookup_host(&seed).await {
                Ok(addrs) => addrs.collect(),
                Err(e) => {
                    debug!("Failed to resolve seed {seed}: {e}");
                    Vec::new()
                }
            }
        })
    })
}

/// Settings for joining a cluster.
#[derive(Debug)]
pub struct ClusterSettings {
    /// Instances only gossip with instances of the same cluster.
    pub cluster_id: String,
    /// A name unique within the cluster, e.g. the pod name.
    pub node_id: String,
    /// The local address to receive gossip on.
    pub listen_addr: SocketAddr,
    /// The address other instances reach this instance's gossip on.
    pub advertise_addr: SocketAddr,
    /// `host:port` pairs of instances to contact first; host names are
    /// resolved again on every gossip round, so a headless service name
    /// reaches all of its pods.
    pub seeds: Vec<String>,
    pub gossip_interval: Duration,
    pub key: ClusterKey,
    /// What this instance tells others about itself.
    pub info: PeerInfo,
}

/// What an instance tells others about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerInfo {
    /// The address its HTTP API is reachable on, e.g. `10.0.0.12:8080`.
    pub http_addr: Option<String>,
    /// The Kubernetes node it runs on.
    pub node_name: Option<String>,
    /// The availability zone it runs in.
    pub zone: Option<String>,
}

impl PeerInfo {
    fn from_node_state(state: &NodeState) -> Self {
        let get = |key| state.get(key).map(ToString::to_string);
        Self {
            http_addr: get(KEY_HTTP_ADDR),
            node_name: get(KEY_NODE_NAME),
            zone: get(KEY_ZONE),
        }
    }

    fn key_values(&self) -> Vec<(String, String)> {
        [
            (KEY_HTTP_ADDR, &self.http_addr),
            (KEY_NODE_NAME, &self.node_name),
            (KEY_ZONE, &self.zone),
        ]
        .into_iter()
        .filter_map(|(key, value)| value.clone().map(|value| (key.to_string(), value)))
        .collect()
    }
}

/// Another live instance of the cluster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub node_id: String,
    pub gossip_addr: SocketAddr,
    pub info: PeerInfo,
}

/// Membership in a cluster of instances.
pub struct Cluster {
    handle: Arc<ChitchatHandle>,
    seed_refresh: JoinHandle<()>,
    gossip_interval: Duration,
}

impl Cluster {
    /// Starts gossiping with the cluster.
    pub async fn join(settings: ClusterSettings) -> anyhow::Result<Self> {
        Self::join_with_resolver(settings, dns_resolver()).await
    }

    async fn join_with_resolver(
        settings: ClusterSettings,
        resolver: Resolver,
    ) -> anyhow::Result<Self> {
        let generation_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let config = ChitchatConfig {
            chitchat_id: ChitchatId::new(settings.node_id, generation_id, settings.advertise_addr),
            cluster_id: settings.cluster_id,
            gossip_interval: settings.gossip_interval,
            listen_addr: settings.listen_addr,
            seed_nodes: settings.seeds,
            failure_detector_config: FailureDetectorConfig {
                dead_node_grace_period: DEAD_NODE_GRACE_PERIOD,
                ..FailureDetectorConfig::default()
            },
            marked_for_deletion_grace_period: DELETION_GRACE_PERIOD,
            catchup_callback: None,
            extra_liveness_predicate: None,
        };
        let seeds = config.seed_nodes.clone();
        let gossip_interval = config.gossip_interval;
        let transport = SecureUdpTransport::new(settings.key, settings.advertise_addr);
        let handle =
            Arc::new(spawn_chitchat(config, settings.info.key_values(), &transport).await?);
        let seed_refresh = tokio::spawn(refresh_seeds(handle.clone(), seeds, resolver));
        Ok(Self {
            handle,
            seed_refresh,
            gossip_interval,
        })
    }

    /// The name of this instance within the cluster.
    pub fn node_id(&self) -> &str {
        &self.handle.chitchat_id().node_id
    }

    /// The other instances currently considered alive.
    pub async fn peers(&self) -> Vec<Peer> {
        let chitchat = self.handle.chitchat();
        let chitchat = chitchat.lock().await;
        let live: BTreeMap<_, _> = chitchat
            .live_nodes()
            .filter_map(|id| {
                chitchat
                    .node_state(id)
                    .map(|state| (id.clone(), state.clone()))
            })
            .collect();
        peers_from(self.handle.chitchat_id(), &live)
    }

    /// Watches the set of other live instances.
    pub async fn watch_peers(&self) -> PeerWatcher {
        let receiver = self.handle.chitchat().lock().await.live_nodes_watcher();
        PeerWatcher {
            receiver,
            self_id: self.handle.chitchat_id().clone(),
        }
    }

    /// Tells the other instances that this one is leaving, then stops gossiping.
    pub async fn leave(self) -> anyhow::Result<()> {
        self.handle
            .with_chitchat(|chitchat| chitchat.self_node_state().set(KEY_LEAVING, "true"))
            .await;
        tokio::time::sleep(self.gossip_interval * LEAVE_ANNOUNCEMENT_ROUNDS).await;

        self.seed_refresh.abort();
        self.seed_refresh.await.ok();
        match Arc::try_unwrap(self.handle) {
            Ok(handle) => handle.shutdown().await,
            Err(handle) => handle.initiate_shutdown(),
        }
    }
}

/// Resolves the seeds at a fast rate for a while after joining, and gossips
/// with every address that is not yet a live member.
async fn refresh_seeds(handle: Arc<ChitchatHandle>, seeds: Vec<String>, resolver: Resolver) {
    if seeds.is_empty() {
        return;
    }
    let self_addr = handle.chitchat_id().gossip_advertise_addr;
    let mut interval = tokio::time::interval(FAST_SEED_REFRESH_INTERVAL);
    let deadline = tokio::time::Instant::now() + FAST_SEED_REFRESH_DURATION;
    while tokio::time::Instant::now() < deadline {
        interval.tick().await;
        let live: Vec<SocketAddr> = {
            let chitchat = handle.chitchat();
            let chitchat = chitchat.lock().await;
            chitchat
                .live_nodes()
                .map(|id| id.gossip_advertise_addr)
                .collect()
        };
        for seed in &seeds {
            for addr in resolver(seed.clone()).await {
                if addr != self_addr && !live.contains(&addr) && handle.gossip(addr).is_err() {
                    // The gossip server stopped.
                    return;
                }
            }
        }
    }
}

/// Reports changes to the set of other live instances.
pub struct PeerWatcher {
    receiver: watch::Receiver<BTreeMap<ChitchatId, NodeState>>,
    self_id: ChitchatId,
}

impl PeerWatcher {
    /// Waits for the next change and returns the new set of peers, or `None`
    /// once the cluster membership ended.
    pub async fn changed(&mut self) -> Option<Vec<Peer>> {
        self.receiver.changed().await.ok()?;
        Some(peers_from(
            &self.self_id,
            &self.receiver.borrow_and_update(),
        ))
    }
}

fn peers_from(self_id: &ChitchatId, live: &BTreeMap<ChitchatId, NodeState>) -> Vec<Peer> {
    live.iter()
        .filter(|(id, _)| id.node_id != self_id.node_id)
        .filter(|(_, state)| state.get(KEY_LEAVING).is_none())
        .map(|(id, state)| Peer {
            node_id: id.node_id.to_string(),
            gossip_addr: id.gossip_advertise_addr,
            info: PeerInfo::from_node_state(state),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

    fn settings(node_id: &str, seeds: Vec<String>) -> ClusterSettings {
        let addr = std::net::UdpSocket::bind("127.0.0.1:0")
            .and_then(|socket| socket.local_addr())
            .expect("No free port");
        ClusterSettings {
            cluster_id: "test".into(),
            node_id: node_id.into(),
            listen_addr: addr,
            advertise_addr: addr,
            seeds,
            gossip_interval: Duration::from_millis(50),
            key: ClusterKey::derive(SECRET, "test").expect("Invalid key"),
            info: PeerInfo::default(),
        }
    }

    /// A seed name that resolves to nothing at first, as the headless service
    /// of pods that are still starting does, is found well before chitchat's
    /// own once-a-minute resolution.
    #[tokio::test(flavor = "multi_thread")]
    async fn late_resolving_seed_is_found_quickly() {
        let a = Cluster::join(settings("a", Vec::new()))
            .await
            .expect("Failed to join");
        let a_addr = a.handle.chitchat_id().gossip_advertise_addr;

        // The seed resolves to nothing until the first lookup has happened.
        let lookups = Arc::new(AtomicUsize::new(0));
        let resolver: Resolver = {
            let lookups = lookups.clone();
            Arc::new(move |_seed| {
                let addrs = if lookups.fetch_add(1, Ordering::SeqCst) == 0 {
                    Vec::new()
                } else {
                    vec![a_addr]
                };
                Box::pin(async move { addrs })
            })
        };
        let b = Cluster::join_with_resolver(settings("b", vec!["seed.invalid:1".into()]), resolver)
            .await
            .expect("Failed to join");

        // Only the refresher's next round can find a.
        let deadline = tokio::time::Instant::now() + FAST_SEED_REFRESH_INTERVAL * 3;
        while !b.peers().await.iter().any(|peer| peer.node_id == "a") {
            assert!(
                tokio::time::Instant::now() < deadline,
                "a was not found in time"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(lookups.load(Ordering::SeqCst) >= 2);

        b.leave().await.expect("Failed to leave");
        a.leave().await.expect("Failed to leave");
    }
}
