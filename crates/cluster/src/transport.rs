//! An encrypted, authenticated UDP transport for chitchat gossip.
//!
//! Every datagram is encrypted with AES-256-GCM under a key derived from a
//! pre-shared secret and the cluster ID. A datagram is dropped if it
//!
//! - fails authentication (a different secret or cluster, tampering),
//! - is older, or further in the future, than [`MAX_MESSAGE_AGE`],
//! - arrives from another address than the sender's gossip address it
//!   carries, so a captured datagram cannot be replayed from a forged source
//!   to make a member send its much larger reply elsewhere, or
//! - repeats a fragment already received from that sender.
//!
//! Comparing the source with the carried address requires that members see
//! each other's gossip addresses unchanged, without NAT in between, as is the
//! case for pods talking to pods in Kubernetes.
//!
//! chitchat fills datagrams up to the largest UDP payload. Since encryption
//! adds a few bytes, a message is split into fragments that are each
//! encrypted and authenticated on their own, so only authenticated fragments
//! of a verified sender are ever buffered for reassembly.
//!
//! ## Wire format
//!
//! ```text
//! datagram  = version (1) || nonce (12) || AES-256-GCM(fragment) || tag (16)
//! fragment  = sent_at_unix_ms (8) || sender_ip (16) || sender_port (2)
//!             || message_id (8) || index (1) || count (1) || chunk
//! nonce     = per-process random prefix (8) || counter (4)
//! AAD       = version
//! ```
//!
//! IPv4 sender addresses are carried as IPv4-mapped IPv6 addresses.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::Context;
use async_trait::async_trait;
use chitchat::transport::{Socket, Transport};
use chitchat::{ChitchatMessage, Deserializable, Serializable};
use hkdf::Hkdf;
use sha2::Sha256;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

/// The version of the wire format.
const WIRE_VERSION: u8 = 2;

/// The largest UDP payload, which is also what chitchat fills datagrams up to.
const MAX_DATAGRAM_SIZE: usize = 65_507;

const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const FRAGMENT_HEADER_LEN: usize = 8 + 16 + 2 + 8 + 1 + 1;

/// The bytes a datagram carries in addition to its chunk of the message.
const OVERHEAD: usize = 1 + NONCE_LEN + TAG_LEN + FRAGMENT_HEADER_LEN;

/// The largest chunk of a message that fits a single datagram.
const MAX_CHUNK_SIZE: usize = MAX_DATAGRAM_SIZE - OVERHEAD;

/// The most fragments a message may be split into. chitchat's messages fit
/// into two; anything announcing more is dropped.
const MAX_FRAGMENTS: u8 = 2;

/// How old, or how far in the future, a datagram may be before it is dropped.
/// Bounds how long received fragments must be remembered to recognize
/// replays, and tolerates clock differences between nodes.
pub const MAX_MESSAGE_AGE: Duration = Duration::from_secs(30);

/// How many recently received fragments are remembered at most. Beyond that,
/// new datagrams are dropped rather than possibly accepting a replay.
const MAX_REMEMBERED_FRAGMENTS: usize = 65_536;

/// How long fragments of an incomplete message are kept.
const REASSEMBLY_TIMEOUT: Duration = Duration::from_secs(5);

/// How many incomplete messages are kept at most.
const MAX_PENDING_MESSAGES: usize = 256;

/// How many incomplete messages are kept at most per sender.
const MAX_PENDING_MESSAGES_PER_SENDER: usize = 8;

/// The smallest accepted secret, in bytes.
const MIN_SECRET_LEN: usize = 32;

/// The key protecting the gossip of one cluster.
#[derive(Clone)]
pub struct ClusterKey(Aes256Gcm);

impl ClusterKey {
    /// Derives the key from a pre-shared secret of at least 32 bytes and the
    /// cluster ID, so that clusters sharing a secret still cannot talk to
    /// each other.
    pub fn derive(secret: &[u8], cluster_id: &str) -> Result<Self, ClusterKeyError> {
        if secret.len() < MIN_SECRET_LEN {
            return Err(ClusterKeyError::SecretTooShort(secret.len()));
        }
        let hkdf = Hkdf::<Sha256>::new(Some(b"yeet-yoink gossip v1"), secret);
        let mut key = [0u8; 32];
        hkdf.expand(cluster_id.as_bytes(), &mut key)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        Ok(Self(Aes256Gcm::new(&key.into())))
    }
}

impl Debug for ClusterKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClusterKey(..)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClusterKeyError {
    #[error("The cluster secret must be at least {MIN_SECRET_LEN} bytes long, but is {0}")]
    SecretTooShort(usize),
}

/// A chitchat transport that encrypts and authenticates every datagram.
pub struct SecureUdpTransport {
    key: ClusterKey,
    /// The gossip address peers see this instance's datagrams coming from.
    advertise_addr: SocketAddr,
}

impl SecureUdpTransport {
    pub fn new(key: ClusterKey, advertise_addr: SocketAddr) -> Self {
        Self {
            key,
            advertise_addr,
        }
    }
}

#[async_trait]
impl Transport for SecureUdpTransport {
    async fn open(&self, listen_addr: SocketAddr) -> anyhow::Result<Box<dyn Socket>> {
        let socket = tokio::net::UdpSocket::bind(listen_addr)
            .await
            .with_context(|| format!("failed to bind to {listen_addr}/UDP for gossip"))?;
        Ok(Box::new(SecureUdpSocket {
            socket,
            sealer: Sealer::new(self.key.clone(), self.advertise_addr),
            replays: ReplayGuard::default(),
            reassembly: Reassembly::default(),
            buf_recv: vec![0; MAX_DATAGRAM_SIZE + 1].into_boxed_slice(),
        }))
    }
}

struct SecureUdpSocket {
    socket: tokio::net::UdpSocket,
    sealer: Sealer,
    replays: ReplayGuard,
    reassembly: Reassembly,
    buf_recv: Box<[u8]>,
}

#[async_trait]
impl Socket for SecureUdpSocket {
    async fn send(&mut self, to: SocketAddr, message: ChitchatMessage) -> anyhow::Result<()> {
        for datagram in self.sealer.seal(&message.serialize_to_vec(), now_unix_ms()) {
            self.socket
                .send_to(&datagram, to)
                .await
                .context("failed to send chitchat message to peer")?;
        }
        Ok(())
    }

    async fn recv(&mut self) -> anyhow::Result<(SocketAddr, ChitchatMessage)> {
        loop {
            let (len, from) = self
                .socket
                .recv_from(&mut self.buf_recv)
                .await
                .context("fatal UDP recv error")?;
            let Some(fragment) = self.sealer.open(&self.buf_recv[..len], now_unix_ms()) else {
                debug!(from = %from, len, "Dropped a gossip datagram that failed authentication");
                continue;
            };
            if canonical(from) != fragment.sender {
                debug!(from = %from, sender = %fragment.sender, "Dropped a gossip datagram from another address than its sender's");
                continue;
            }
            let now = Instant::now();
            if !self.replays.first_seen(&fragment, now) {
                debug!(from = %from, "Dropped a repeated gossip datagram");
                continue;
            }
            let Some(payload) = self.reassembly.add(fragment, now) else {
                continue;
            };
            match ChitchatMessage::deserialize(&mut &payload[..]) {
                Ok(message) => return Ok((from, message)),
                Err(e) => warn!(from = %from, "Dropped an invalid gossip message: {e}"),
            }
        }
    }
}

/// Encrypts messages into datagrams and decrypts datagrams into fragments.
struct Sealer {
    key: ClusterKey,
    /// The gossip address of this instance, carried in every fragment.
    sender: SocketAddr,
    nonces: NonceSequence,
    next_message_id: u64,
}

impl Sealer {
    fn new(key: ClusterKey, sender: SocketAddr) -> Self {
        Self {
            key,
            sender: canonical(sender),
            nonces: NonceSequence::new(),
            next_message_id: rand::random(),
        }
    }

    /// Splits a message into encrypted datagrams.
    fn seal(&mut self, message: &[u8], sent_at_unix_ms: u64) -> Vec<Vec<u8>> {
        let message_id = self.next_message_id;
        self.next_message_id = self.next_message_id.wrapping_add(1);

        let chunks: Vec<&[u8]> = if message.is_empty() {
            vec![&[]]
        } else {
            message.chunks(MAX_CHUNK_SIZE).collect()
        };
        let count = u8::try_from(chunks.len()).expect("chitchat messages fit two datagrams");

        chunks
            .into_iter()
            .enumerate()
            .map(|(index, chunk)| {
                let mut plaintext = Vec::with_capacity(FRAGMENT_HEADER_LEN + chunk.len());
                plaintext.extend_from_slice(&sent_at_unix_ms.to_be_bytes());
                plaintext.extend_from_slice(&ipv6(self.sender.ip()).octets());
                plaintext.extend_from_slice(&self.sender.port().to_be_bytes());
                plaintext.extend_from_slice(&message_id.to_be_bytes());
                plaintext.push(index as u8);
                plaintext.push(count);
                plaintext.extend_from_slice(chunk);

                let nonce = self.nonces.next();
                let ciphertext = self
                    .key
                    .0
                    .encrypt(
                        &Nonce::from(nonce),
                        Payload {
                            msg: &plaintext,
                            aad: &[WIRE_VERSION],
                        },
                    )
                    .expect("AES-GCM encryption of a datagram cannot fail");

                let mut datagram = Vec::with_capacity(1 + NONCE_LEN + ciphertext.len());
                datagram.push(WIRE_VERSION);
                datagram.extend_from_slice(&nonce);
                datagram.extend_from_slice(&ciphertext);
                datagram
            })
            .collect()
    }

    /// Decrypts and checks a datagram. Returns `None` for anything that is not
    /// an authentic, fresh fragment of this cluster.
    fn open(&self, datagram: &[u8], now_unix_ms: u64) -> Option<Fragment> {
        if datagram.len() < OVERHEAD || datagram[0] != WIRE_VERSION {
            return None;
        }
        let nonce: [u8; NONCE_LEN] = datagram[1..1 + NONCE_LEN].try_into().ok()?;
        let plaintext = self
            .key
            .0
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: &datagram[1 + NONCE_LEN..],
                    aad: &[WIRE_VERSION],
                },
            )
            .ok()?;

        let sent_at = u64::from_be_bytes(plaintext[0..8].try_into().ok()?);
        if now_unix_ms.abs_diff(sent_at) > MAX_MESSAGE_AGE.as_millis() as u64 {
            return None;
        }
        let ip: [u8; 16] = plaintext[8..24].try_into().ok()?;
        let port = u16::from_be_bytes(plaintext[24..26].try_into().ok()?);
        let sender = canonical(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(ip)), port));
        let message_id = u64::from_be_bytes(plaintext[26..34].try_into().ok()?);
        let (index, count) = (plaintext[34], plaintext[35]);
        if count == 0 || count > MAX_FRAGMENTS || index >= count {
            return None;
        }
        Some(Fragment {
            sender,
            message_id,
            index,
            count,
            chunk: plaintext[FRAGMENT_HEADER_LEN..].to_vec(),
        })
    }
}

/// Produces nonces that never repeat for a key within one process: a random
/// 64-bit prefix and a 32-bit counter, with a fresh prefix whenever the
/// counter wraps. Another process repeats a nonce only if it draws the same
/// 64-bit prefix.
struct NonceSequence {
    prefix: [u8; 8],
    counter: u32,
}

impl NonceSequence {
    fn new() -> Self {
        Self {
            prefix: rand::random(),
            counter: 0,
        }
    }

    fn next(&mut self) -> [u8; NONCE_LEN] {
        let mut nonce = [0u8; NONCE_LEN];
        nonce[..8].copy_from_slice(&self.prefix);
        nonce[8..].copy_from_slice(&self.counter.to_be_bytes());

        self.counter = self.counter.wrapping_add(1);
        if self.counter == 0 {
            self.prefix = rand::random();
        }
        nonce
    }
}

/// An authenticated piece of a message.
#[derive(Debug, PartialEq, Eq)]
struct Fragment {
    /// The gossip address of the sender, as authenticated by the key.
    sender: SocketAddr,
    message_id: u64,
    index: u8,
    count: u8,
    chunk: Vec<u8>,
}

/// Remembers recently received fragments to recognize replays.
#[derive(Default)]
struct ReplayGuard {
    seen: HashMap<(SocketAddr, u64, u8), Instant>,
}

impl ReplayGuard {
    /// Records a fragment and tells whether it was seen for the first time.
    fn first_seen(&mut self, fragment: &Fragment, now: Instant) -> bool {
        // Fragments older than this are rejected for their age anyway; the
        // window covers datagrams sent up to MAX_MESSAGE_AGE in the future.
        let horizon = MAX_MESSAGE_AGE * 2;
        if self.seen.len() >= MAX_REMEMBERED_FRAGMENTS {
            self.seen
                .retain(|_, seen_at| now.duration_since(*seen_at) < horizon);
            if self.seen.len() >= MAX_REMEMBERED_FRAGMENTS {
                warn!("Too many recent gossip fragments to recognize replays; dropping one");
                return false;
            }
        }
        let key = (fragment.sender, fragment.message_id, fragment.index);
        match self.seen.get(&key) {
            Some(seen_at) if now.duration_since(*seen_at) < horizon => false,
            _ => {
                self.seen.insert(key, now);
                true
            }
        }
    }
}

/// Collects the fragments of split messages.
#[derive(Default)]
struct Reassembly {
    pending: HashMap<(SocketAddr, u64), Pending>,
}

struct Pending {
    chunks: Vec<Option<Vec<u8>>>,
    started: Instant,
}

impl Reassembly {
    /// Adds a fragment and returns the whole message once all of its
    /// fragments have arrived.
    fn add(&mut self, fragment: Fragment, now: Instant) -> Option<Vec<u8>> {
        if fragment.count == 1 {
            return Some(fragment.chunk);
        }

        self.pending
            .retain(|_, pending| now.duration_since(pending.started) < REASSEMBLY_TIMEOUT);

        let sender = fragment.sender;
        let key = (sender, fragment.message_id);
        if !self.pending.contains_key(&key) {
            let from_sender = self.pending.keys().filter(|(s, _)| *s == sender).count();
            if from_sender >= MAX_PENDING_MESSAGES_PER_SENDER
                || self.pending.len() >= MAX_PENDING_MESSAGES
            {
                warn!("Too many incomplete gossip messages; dropping a fragment from {sender}");
                return None;
            }
        }

        let pending = self.pending.entry(key).or_insert_with(|| Pending {
            chunks: vec![None; fragment.count as usize],
            started: now,
        });
        if pending.chunks.len() != fragment.count as usize {
            // Fragments of one message disagree on their count.
            self.pending.remove(&key);
            return None;
        }
        pending.chunks[fragment.index as usize] = Some(fragment.chunk);
        if pending.chunks.iter().any(Option::is_none) {
            return None;
        }

        let pending = self.pending.remove(&key)?;
        Some(pending.chunks.into_iter().flatten().flatten().collect())
    }
}

/// Maps an address to the IPv6 form it is carried in.
fn ipv6(ip: IpAddr) -> Ipv6Addr {
    match ip {
        IpAddr::V4(ip) => ip.to_ipv6_mapped(),
        IpAddr::V6(ip) => ip,
    }
}

/// Normalizes IPv4-mapped IPv6 addresses to IPv4, so that addresses compare
/// equal however a socket reports them.
fn canonical(addr: SocketAddr) -> SocketAddr {
    match addr.ip() {
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(ip) => SocketAddr::new(IpAddr::V4(ip), addr.port()),
            None => addr,
        },
        IpAddr::V4(_) => addr,
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";
    const NOW: u64 = 1_800_000_000_000;

    fn addr() -> SocketAddr {
        "10.0.0.1:7946".parse().unwrap()
    }

    fn other_addr() -> SocketAddr {
        "10.0.0.2:7946".parse().unwrap()
    }

    fn sealer(secret: &[u8], cluster_id: &str) -> Sealer {
        Sealer::new(
            ClusterKey::derive(secret, cluster_id).expect("Invalid key"),
            addr(),
        )
    }

    /// Seals and reopens a message, reassembling fragments.
    fn round_trip(sender: &mut Sealer, receiver: &Sealer, message: &[u8]) -> Option<Vec<u8>> {
        let mut reassembly = Reassembly::default();
        let mut result = None;
        for datagram in sender.seal(message, NOW) {
            assert!(datagram.len() <= MAX_DATAGRAM_SIZE);
            let fragment = receiver.open(&datagram, NOW)?;
            result = reassembly.add(fragment, Instant::now());
        }
        result
    }

    #[test]
    fn short_secrets_are_rejected() {
        assert!(matches!(
            ClusterKey::derive(b"too short", "c"),
            Err(ClusterKeyError::SecretTooShort(9))
        ));
    }

    #[test]
    fn key_is_not_printed() {
        let key = ClusterKey::derive(SECRET, "c").unwrap();
        assert_eq!(format!("{key:?}"), "ClusterKey(..)");
    }

    #[test]
    fn message_round_trips() {
        let mut sender = sealer(SECRET, "cluster");
        let receiver = sealer(SECRET, "cluster");
        assert_eq!(
            round_trip(&mut sender, &receiver, b"hello").as_deref(),
            Some(&b"hello"[..])
        );
        assert_eq!(
            round_trip(&mut sender, &receiver, b"").as_deref(),
            Some(&b""[..])
        );
    }

    #[test]
    fn fragment_carries_the_sender_address() {
        for sender_addr in ["10.0.0.1:7946", "[fd00::1]:7946", "[::ffff:10.0.0.1]:7946"] {
            let sender_addr: SocketAddr = sender_addr.parse().unwrap();
            let key = ClusterKey::derive(SECRET, "cluster").unwrap();
            let mut sender = Sealer::new(key.clone(), sender_addr);
            let receiver = Sealer::new(key, other_addr());

            let datagram = sender.seal(b"hello", NOW).remove(0);
            let fragment = receiver.open(&datagram, NOW).expect("Not opened");
            assert_eq!(fragment.sender, canonical(sender_addr));
        }
        // IPv4-mapped addresses compare equal to their IPv4 form.
        assert_eq!(canonical("[::ffff:10.0.0.1]:7946".parse().unwrap()), addr());
    }

    #[test]
    fn largest_message_is_split_and_reassembled() {
        let mut sender = sealer(SECRET, "cluster");
        let receiver = sealer(SECRET, "cluster");
        let message: Vec<u8> = (0..MAX_DATAGRAM_SIZE).map(|i| (i % 251) as u8).collect();

        let datagrams = sender.seal(&message, NOW);
        assert_eq!(datagrams.len(), 2);
        assert_eq!(round_trip(&mut sender, &receiver, &message), Some(message));
    }

    #[test]
    fn fragments_arriving_out_of_order_are_reassembled() {
        let mut sender = sealer(SECRET, "cluster");
        let receiver = sealer(SECRET, "cluster");
        let message = vec![7u8; MAX_DATAGRAM_SIZE];
        let mut datagrams = sender.seal(&message, NOW);
        datagrams.reverse();

        let mut reassembly = Reassembly::default();
        let first = receiver.open(&datagrams[0], NOW).unwrap();
        assert_eq!(reassembly.add(first, Instant::now()), None);
        let second = receiver.open(&datagrams[1], NOW).unwrap();
        assert_eq!(reassembly.add(second, Instant::now()), Some(message));
    }

    #[test]
    fn other_secret_is_rejected() {
        let mut sender = sealer(b"another secret, also 32 bytes long", "cluster");
        let receiver = sealer(SECRET, "cluster");
        assert_eq!(round_trip(&mut sender, &receiver, b"hello"), None);
    }

    #[test]
    fn other_cluster_is_rejected() {
        let mut sender = sealer(SECRET, "staging");
        let receiver = sealer(SECRET, "production");
        assert_eq!(round_trip(&mut sender, &receiver, b"hello"), None);
    }

    #[test]
    fn tampered_datagram_is_rejected() {
        let mut sender = sealer(SECRET, "cluster");
        let receiver = sealer(SECRET, "cluster");
        let mut datagram = sender.seal(b"hello", NOW).remove(0);
        let last = datagram.len() - 1;
        datagram[last] ^= 1;
        assert_eq!(receiver.open(&datagram, NOW), None);

        let mut datagram = sender.seal(b"hello", NOW).remove(0);
        datagram[0] = WIRE_VERSION + 1;
        assert_eq!(receiver.open(&datagram, NOW), None);

        assert_eq!(receiver.open(&[WIRE_VERSION; 10], NOW), None);
    }

    #[test]
    fn stale_and_future_datagrams_are_rejected() {
        let mut sender = sealer(SECRET, "cluster");
        let receiver = sealer(SECRET, "cluster");
        let window = MAX_MESSAGE_AGE.as_millis() as u64;
        let datagram = sender.seal(b"hello", NOW).remove(0);

        assert!(receiver.open(&datagram, NOW + window).is_some());
        assert_eq!(receiver.open(&datagram, NOW + window + 1), None);
        assert_eq!(receiver.open(&datagram, NOW - window - 1), None);
    }

    #[test]
    fn repeated_fragments_are_recognized() {
        let mut sender = sealer(SECRET, "cluster");
        let receiver = sealer(SECRET, "cluster");
        let datagram = sender.seal(b"hello", NOW).remove(0);
        let next = sender.seal(b"hello", NOW).remove(0);
        let now = Instant::now();

        let mut replays = ReplayGuard::default();
        let fragment = receiver.open(&datagram, NOW).unwrap();
        assert!(replays.first_seen(&fragment, now));
        assert!(!replays.first_seen(&fragment, now + Duration::from_secs(1)));

        // The same content sent again is a new message, not a replay.
        let fragment = receiver.open(&next, NOW).unwrap();
        assert!(replays.first_seen(&fragment, now));
    }

    #[test]
    fn replay_memory_is_bounded() {
        let fragment = |message_id| Fragment {
            sender: addr(),
            message_id,
            index: 0,
            count: 1,
            chunk: Vec::new(),
        };
        let now = Instant::now();
        let mut replays = ReplayGuard::default();
        for message_id in 0..MAX_REMEMBERED_FRAGMENTS as u64 {
            assert!(replays.first_seen(&fragment(message_id), now));
        }

        // Full of recent fragments: new ones are dropped rather than risked.
        let next = fragment(MAX_REMEMBERED_FRAGMENTS as u64);
        assert!(!replays.first_seen(&next, now));

        // Once they are old enough to be rejected anyway, there is room again.
        let later = now + MAX_MESSAGE_AGE * 2;
        assert!(replays.first_seen(&next, later));
    }

    #[test]
    fn nonces_do_not_repeat() {
        let mut nonces = NonceSequence::new();
        let first = nonces.next();
        let second = nonces.next();
        assert_ne!(first, second);
        assert_eq!(first[..8], second[..8]);
    }

    #[test]
    fn nonce_prefix_changes_when_the_counter_wraps() {
        let mut nonces = NonceSequence::new();
        nonces.counter = u32::MAX;
        let last = nonces.next();
        let wrapped = nonces.next();
        assert_ne!(last[..8], wrapped[..8]);
        assert_eq!(wrapped[8..], [0, 0, 0, 0]);
    }

    #[test]
    fn incomplete_messages_expire() {
        let mut sender = sealer(SECRET, "cluster");
        let receiver = sealer(SECRET, "cluster");
        let message = vec![1u8; MAX_DATAGRAM_SIZE];
        let datagrams = sender.seal(&message, NOW);
        let start = Instant::now();

        let mut reassembly = Reassembly::default();
        let first = receiver.open(&datagrams[0], NOW).unwrap();
        assert_eq!(reassembly.add(first, start), None);

        // The second fragment arrives too late; its message is incomplete.
        let second = receiver.open(&datagrams[1], NOW).unwrap();
        let late = start + REASSEMBLY_TIMEOUT + Duration::from_millis(1);
        assert_eq!(reassembly.add(second, late), None);
        assert!(reassembly.pending.len() <= 1);
    }

    #[test]
    fn one_sender_cannot_fill_the_reassembly_table() {
        let key = ClusterKey::derive(SECRET, "cluster").unwrap();
        let mut flooder = Sealer::new(key.clone(), addr());
        let mut honest = Sealer::new(key.clone(), other_addr());
        let receiver = Sealer::new(key, "10.0.0.3:7946".parse().unwrap());
        let message = vec![5u8; MAX_DATAGRAM_SIZE];
        let now = Instant::now();

        let mut reassembly = Reassembly::default();
        for _ in 0..MAX_PENDING_MESSAGES_PER_SENDER + 4 {
            let first = flooder.seal(&message, NOW).remove(0);
            reassembly.add(receiver.open(&first, NOW).unwrap(), now);
        }
        assert_eq!(reassembly.pending.len(), MAX_PENDING_MESSAGES_PER_SENDER);

        // Another sender still gets its split messages through.
        let datagrams = honest.seal(&message, NOW);
        assert_eq!(
            reassembly.add(receiver.open(&datagrams[0], NOW).unwrap(), now),
            None
        );
        assert_eq!(
            reassembly.add(receiver.open(&datagrams[1], NOW).unwrap(), now),
            Some(message)
        );
    }

    fn free_addr() -> SocketAddr {
        std::net::UdpSocket::bind("127.0.0.1:0")
            .and_then(|socket| socket.local_addr())
            .expect("No free port")
    }

    /// A datagram from another address than the sender's it carries is
    /// dropped, so a captured request cannot be replayed from a forged source
    /// to make a member send its reply elsewhere.
    #[tokio::test]
    async fn datagram_from_another_address_is_dropped() {
        let key = ClusterKey::derive(SECRET, "cluster").unwrap();
        let (receiver_addr, sender_addr) = (free_addr(), free_addr());
        let mut receiver = SecureUdpTransport::new(key.clone(), receiver_addr)
            .open(receiver_addr)
            .await
            .unwrap();
        let mut sender = SecureUdpTransport::new(key.clone(), sender_addr)
            .open(sender_addr)
            .await
            .unwrap();

        // A genuine datagram arrives.
        sender
            .send(receiver_addr, ChitchatMessage::BadCluster)
            .await
            .unwrap();
        let (from, message) = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .expect("Genuine datagram not received")
            .unwrap();
        assert_eq!(from, sender_addr);
        assert!(matches!(message, ChitchatMessage::BadCluster));

        // An authentic datagram naming the sender, sent from elsewhere, does not.
        let datagram = Sealer::new(key, sender_addr)
            .seal(
                &ChitchatMessage::BadCluster.serialize_to_vec(),
                now_unix_ms(),
            )
            .remove(0);
        let forger = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        forger.send_to(&datagram, receiver_addr).await.unwrap();
        let received = tokio::time::timeout(Duration::from_millis(300), receiver.recv()).await;
        assert!(
            received.is_err(),
            "Datagram from a forged source was accepted"
        );
    }
}
