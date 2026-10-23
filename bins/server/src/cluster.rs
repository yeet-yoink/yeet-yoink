//! Joins the cluster of instances, if one is configured.

use anyhow::Context;
use app_config::cluster::ClusterConfig;
use cluster::{Cluster, ClusterKey, ClusterSettings, PeerInfo};
use std::net::SocketAddr;
use tracing::info;

/// Joins the configured cluster, announcing the HTTP API on `http_port`.
pub async fn join(config: &ClusterConfig, http_port: Option<u16>) -> anyhow::Result<Cluster> {
    let advertise_addr = config.advertise_addr().context(
        "cluster.advertise_ip must be set when gossip listens on an unspecified address",
    )?;
    let secret = tokio::fs::read(&config.secret_file)
        .await
        .with_context(|| format!("Failed to read the cluster secret {:?}", config.secret_file))?;
    let key = ClusterKey::derive(without_trailing_newline(&secret), &config.cluster_id)?;

    let settings = ClusterSettings {
        cluster_id: config.cluster_id.clone(),
        node_id: config
            .node_id
            .clone()
            .unwrap_or_else(|| advertise_addr.to_string()),
        listen_addr: config.listen_addr,
        advertise_addr,
        seeds: config.seeds.clone(),
        gossip_interval: config.gossip_interval(),
        key,
        info: PeerInfo {
            http_addr: http_port.map(|port| SocketAddr::new(advertise_addr.ip(), port).to_string()),
            node_name: config.node_name.clone(),
            zone: config.zone.clone(),
        },
    };
    info!(
        "Joining cluster {cluster} as {node} on {addr} with {seeds} seed(s)",
        cluster = settings.cluster_id,
        node = settings.node_id,
        addr = advertise_addr,
        seeds = settings.seeds.len()
    );
    Cluster::join(settings).await
}

/// Logs whenever instances join or leave the cluster.
pub async fn log_membership_changes(cluster: &Cluster) {
    let mut watcher = cluster.watch_peers().await;
    tokio::spawn(async move {
        let mut known = Vec::new();
        while let Some(peers) = watcher.changed().await {
            let mut names: Vec<_> = peers.into_iter().map(|peer| peer.node_id).collect();
            names.sort_unstable();
            // The watch also fires when only the state of an instance changed.
            if names == known {
                continue;
            }
            info!(
                "The cluster now has {count} other instance(s): {names}",
                count = names.len(),
                names = names.join(", ")
            );
            known = names;
        }
    });
}

/// Strips one trailing line break, as left by `echo` or editors, from a secret.
fn without_trailing_newline(secret: &[u8]) -> &[u8] {
    secret
        .strip_suffix(b"\r\n")
        .or_else(|| secret.strip_suffix(b"\n"))
        .unwrap_or(secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_trailing_newline_is_stripped() {
        assert_eq!(without_trailing_newline(b"secret\n"), b"secret");
        assert_eq!(without_trailing_newline(b"secret\r\n"), b"secret");
        assert_eq!(without_trailing_newline(b"secret\n\n"), b"secret\n");
        assert_eq!(without_trailing_newline(b"secret "), b"secret ");
    }

    fn config(secret_file: &std::path::Path) -> ClusterConfig {
        serde_yaml::from_str(&format!(
            "secret_file: {secret_file:?}\nlisten_addr: \"127.0.0.1:0\""
        ))
        .expect("Invalid config")
    }

    #[tokio::test]
    async fn missing_secret_is_reported() {
        let result = join(&config("/nonexistent/secret".as_ref()), None).await;
        let error = result.err().expect("Joined without a secret").to_string();
        assert!(error.contains("cluster secret"), "{error}");
    }

    #[tokio::test]
    async fn unspecified_address_without_advertise_ip_is_reported() {
        let config: ClusterConfig =
            serde_yaml::from_str("secret_file: /s").expect("Invalid config");
        let error = join(&config, None).await.err().expect("Joined").to_string();
        assert!(error.contains("advertise_ip"), "{error}");
    }

    #[tokio::test]
    async fn short_secret_is_reported() {
        let dir = std::env::temp_dir().join(format!("yy-secret-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("short");
        std::fs::write(&file, "too short\n").unwrap();

        let error = join(&config(&file), None)
            .await
            .err()
            .expect("Joined")
            .to_string();
        assert!(error.contains("at least 32 bytes"), "{error}");
        std::fs::remove_dir_all(dir).ok();
    }
}
