//! Instances gossiping over the encrypted transport on localhost.

use cluster::{Cluster, ClusterKey, ClusterSettings, Peer, PeerInfo};
use std::net::SocketAddr;
use std::time::Duration;

const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";
const OTHER_SECRET: &[u8] = b"fedcba9876543210fedcba9876543210";
const CLUSTER_ID: &str = "test-cluster";

/// Picks a free UDP port on localhost.
fn free_addr() -> SocketAddr {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .and_then(|socket| socket.local_addr())
        .expect("No free port")
}

async fn join(node_id: &str, secret: &[u8], seeds: &[SocketAddr]) -> (Cluster, SocketAddr) {
    let addr = free_addr();
    let cluster = Cluster::join(ClusterSettings {
        cluster_id: CLUSTER_ID.into(),
        node_id: node_id.into(),
        listen_addr: addr,
        advertise_addr: addr,
        seeds: seeds.iter().map(ToString::to_string).collect(),
        gossip_interval: Duration::from_millis(50),
        key: ClusterKey::derive(secret, CLUSTER_ID).expect("Invalid key"),
        info: PeerInfo {
            http_addr: Some(format!("{node_id}.example:8080")),
            node_name: Some(format!("k8s-{node_id}")),
            zone: Some("zone-a".into()),
        },
    })
    .await
    .expect("Failed to join");
    (cluster, addr)
}

async fn peer_ids(cluster: &Cluster) -> Vec<String> {
    let mut ids: Vec<_> = cluster
        .peers()
        .await
        .into_iter()
        .map(|peer| peer.node_id)
        .collect();
    ids.sort();
    ids
}

/// Waits until the cluster sees exactly the given peers.
async fn wait_for_peers(cluster: &Cluster, expected: &[&str]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if peer_ids(cluster).await == expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} sees {:?} instead of {expected:?}",
            cluster.node_id(),
            peer_ids(cluster).await
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn instances_find_each_other_through_one_seed() {
    let (a, a_addr) = join("a", SECRET, &[]).await;
    let (b, _) = join("b", SECRET, &[a_addr]).await;
    let (c, _) = join("c", SECRET, &[a_addr]).await;

    wait_for_peers(&a, &["b", "c"]).await;
    // b and c only know a, and learn of each other through gossip.
    wait_for_peers(&b, &["a", "c"]).await;
    wait_for_peers(&c, &["a", "b"]).await;

    let peers = b.peers().await;
    let c_seen_by_b: &Peer = peers.iter().find(|p| p.node_id == "c").unwrap();
    assert_eq!(
        c_seen_by_b.info,
        PeerInfo {
            http_addr: Some("c.example:8080".into()),
            node_name: Some("k8s-c".into()),
            zone: Some("zone-a".into()),
        }
    );

    for cluster in [a, b, c] {
        cluster.leave().await.expect("Failed to leave");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn instance_with_another_secret_is_ignored() {
    let (a, a_addr) = join("a", SECRET, &[]).await;
    let (b, _) = join("b", SECRET, &[a_addr]).await;
    let (intruder, _) = join("intruder", OTHER_SECRET, &[a_addr]).await;

    wait_for_peers(&a, &["b"]).await;
    // Give the intruder plenty of gossip rounds to get in.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(peer_ids(&a).await, ["b"]);
    assert_eq!(peer_ids(&b).await, ["a"]);
    assert!(peer_ids(&intruder).await.is_empty());

    for cluster in [a, b, intruder] {
        cluster.leave().await.expect("Failed to leave");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn peers_are_watched() {
    let (a, a_addr) = join("a", SECRET, &[]).await;
    let mut watcher = a.watch_peers().await;
    let (b, _) = join("b", SECRET, &[a_addr]).await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let peers = tokio::time::timeout_at(deadline, watcher.changed())
            .await
            .expect("No change seen in time")
            .expect("Membership ended");
        if peers.iter().any(|peer| peer.node_id == "b") {
            break;
        }
    }

    for cluster in [a, b] {
        cluster.leave().await.expect("Failed to leave");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn leaving_instance_is_dropped_at_once() {
    let (a, a_addr) = join("a", SECRET, &[]).await;
    let (b, _) = join("b", SECRET, &[a_addr]).await;
    wait_for_peers(&a, &["b"]).await;

    b.leave().await.expect("Failed to leave");

    // Far sooner than the failure detector would notice the silence.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while !peer_ids(&a).await.is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "b is still counted");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    a.leave().await.expect("Failed to leave");
}
