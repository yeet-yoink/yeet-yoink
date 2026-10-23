# Instances find each other with chitchat gossip over an authenticated, encrypted transport

Instances need to know each other, across Kubernetes namespaces, and eventually which instance holds which file (#7, #9). We use [chitchat](https://github.com/quickwit-oss/chitchat): every instance publishes facts about itself (address, node, zone, later the files it holds) as replicated key-value state, instances that join late catch up through anti-entropy, and seed hosts are re-resolved through DNS, so a headless service name in any namespace finds all of its pods. Because gossip crosses namespace boundaries, it runs over our own transport that encrypts and authenticates every datagram with a pre-shared key; instances without the key are ignored.

## Considered Options

- **libp2p**: brings Noise encryption and pre-shared-key private networks, but its gossip is publish/subscribe, so location state would need an extra catch-up mechanism, and its dependency and concept surface is far larger than the problem.
- **foca / memberlist**: SWIM membership only; the state layer and transport security would both be ours to build.
- **Kubernetes API discovery** (watching EndpointSlices across namespaces): fully automatic, but needs a ClusterRole to read EndpointSlices cluster-wide. DNS seeds come first; the API can be added as another seed source later.

## Consequences

- chitchat itself has no encryption or authentication; the protection lives in our transport, which is security-relevant code and needs review.
- chitchat is pinned to 0.11. From 0.12 on, the messages handed to a transport can no longer be encoded outside the crate ([quickwit-oss/chitchat#207](https://github.com/quickwit-oss/chitchat/issues/207)), which rules out our transport. Upgrading waits for that to change upstream.
- chitchat 0.11 resolves seed host names once a minute, so pods starting together could stay apart that long; we resolve seeds every few seconds while an instance starts. Instances that shut down announce it, so others stop counting them at once instead of after the failure detector's timeout.
- Gossip uses UDP: NetworkPolicies must allow it between namespaces that share a cluster.
- Every datagram carries its sender's gossip address inside the encryption, and receivers drop datagrams from any other source as well as repeated ones. A captured datagram therefore cannot be replayed from a forged address to make a member send its far larger reply elsewhere. This requires that members reach each other without NAT in between, as pods do within a Kubernetes cluster; gossip across NAT boundaries is not supported.
- Every instance holds the state of all others, which suits clusters of up to a few hundred instances.
