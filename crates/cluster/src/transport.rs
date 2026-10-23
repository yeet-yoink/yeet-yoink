//! An encrypted, authenticated UDP transport for chitchat gossip.
//!
//! Every datagram is encrypted with AES-256-GCM under a key derived from a
//! pre-shared secret and the cluster ID. Datagrams that fail authentication
//! (a different secret or cluster, tampering) or that are older than
//! [`MAX_MESSAGE_AGE`] are dropped. Within that window a replayed datagram is
//! accepted again; chitchat ignores state it has already seen, so a replay
//! cannot roll anything back.
//!
//! chitchat fills datagrams up to the largest UDP payload. Since encryption
//! adds a few bytes, a message is split into fragments that are each
//! encrypted and authenticated on their own, so only authenticated fragments
//! are ever buffered for reassembly.
//!
//! ## Wire format
//!
//! ```text
//! datagram  = version (1) || nonce (12) || AES-256-GCM(fragment) || tag (16)
//! fragment  = sent_at_unix_ms (8) || message_id (8) || index (1) || count (1) || chunk
//! nonce     = per-process random prefix (8) || counter (4)
//! AAD       = version
//! ```

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
use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

/// The version of the wire format.
const WIRE_VERSION: u8 = 1;

/// The largest UDP payload, which is also what chitchat fills datagrams up to.
const MAX_DATAGRAM_SIZE: usize = 65_507;

const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const FRAGMENT_HEADER_LEN: usize = 8 + 8 + 1 + 1;

/// The bytes a datagram carries in addition to its chunk of the message.
const OVERHEAD: usize = 1 + NONCE_LEN + TAG_LEN + FRAGMENT_HEADER_LEN;

/// The largest chunk of a message that fits a single datagram.
const MAX_CHUNK_SIZE: usize = MAX_DATAGRAM_SIZE - OVERHEAD;

/// The most fragments a message may be split into. chitchat's messages fit
/// into two; anything announcing more is dropped.
const MAX_FRAGMENTS: u8 = 2;

/// How old, or how far in the future, a datagram may be before it is dropped.
/// Bounds the window in which a captured datagram can be replayed, and
/// tolerates clock differences between nodes.
pub const MAX_MESSAGE_AGE: Duration = Duration::from_secs(60);

/// How long fragments of an incomplete message are kept.
const REASSEMBLY_TIMEOUT: Duration = Duration::from_secs(5);

/// How many incomplete messages are kept at most.
const MAX_PENDING_MESSAGES: usize = 256;

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
}

impl SecureUdpTransport {
    pub fn new(key: ClusterKey) -> Self {
        Self { key }
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
            sealer: Sealer::new(self.key.clone()),
            reassembly: Reassembly::default(),
            buf_recv: vec![0; MAX_DATAGRAM_SIZE + 1].into_boxed_slice(),
        }))
    }
}

struct SecureUdpSocket {
    socket: tokio::net::UdpSocket,
    sealer: Sealer,
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
            let Some(payload) = self.reassembly.add(from, fragment, Instant::now()) else {
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
    nonces: NonceSequence,
    next_message_id: u64,
}

impl Sealer {
    fn new(key: ClusterKey) -> Self {
        Self {
            key,
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
        let message_id = u64::from_be_bytes(plaintext[8..16].try_into().ok()?);
        let (index, count) = (plaintext[16], plaintext[17]);
        if count == 0 || count > MAX_FRAGMENTS || index >= count {
            return None;
        }
        Some(Fragment {
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
    message_id: u64,
    index: u8,
    count: u8,
    chunk: Vec<u8>,
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
    fn add(&mut self, from: SocketAddr, fragment: Fragment, now: Instant) -> Option<Vec<u8>> {
        if fragment.count == 1 {
            return Some(fragment.chunk);
        }

        self.pending
            .retain(|_, pending| now.duration_since(pending.started) < REASSEMBLY_TIMEOUT);
        if self.pending.len() >= MAX_PENDING_MESSAGES {
            warn!("Too many incomplete gossip messages; dropping a fragment from {from}");
            return None;
        }

        let key = (from, fragment.message_id);
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

    fn sealer(secret: &[u8], cluster_id: &str) -> Sealer {
        Sealer::new(ClusterKey::derive(secret, cluster_id).expect("Invalid key"))
    }

    fn addr() -> SocketAddr {
        "10.0.0.1:7946".parse().unwrap()
    }

    /// Seals and reopens a message, reassembling fragments.
    fn round_trip(sender: &mut Sealer, receiver: &Sealer, message: &[u8]) -> Option<Vec<u8>> {
        let mut reassembly = Reassembly::default();
        let mut result = None;
        for datagram in sender.seal(message, NOW) {
            assert!(datagram.len() <= MAX_DATAGRAM_SIZE);
            let fragment = receiver.open(&datagram, NOW)?;
            result = reassembly.add(addr(), fragment, Instant::now());
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
        assert_eq!(reassembly.add(addr(), first, Instant::now()), None);
        let second = receiver.open(&datagrams[1], NOW).unwrap();
        assert_eq!(
            reassembly.add(addr(), second, Instant::now()),
            Some(message)
        );
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
        assert_eq!(reassembly.add(addr(), first, start), None);

        // The second fragment arrives too late; its message is incomplete.
        let second = receiver.open(&datagrams[1], NOW).unwrap();
        let late = start + REASSEMBLY_TIMEOUT + Duration::from_millis(1);
        assert_eq!(reassembly.add(addr(), second, late), None);
        assert!(reassembly.pending.len() <= 1);
    }

    #[test]
    fn fragments_of_different_senders_are_kept_apart() {
        let mut sender = sealer(SECRET, "cluster");
        let receiver = sealer(SECRET, "cluster");
        let message = vec![3u8; MAX_DATAGRAM_SIZE];
        let datagrams = sender.seal(&message, NOW);
        let other: SocketAddr = "10.0.0.2:7946".parse().unwrap();

        let mut reassembly = Reassembly::default();
        let first = receiver.open(&datagrams[0], NOW).unwrap();
        assert_eq!(reassembly.add(addr(), first, Instant::now()), None);
        let second = receiver.open(&datagrams[1], NOW).unwrap();
        assert_eq!(reassembly.add(other, second, Instant::now()), None);
    }
}
