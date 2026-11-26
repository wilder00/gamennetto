//! Networking: encrypted QUIC connections, pairing, messages, discovery.
//!
//! One connection carries:
//! - a bidirectional **control stream** (small JSON messages, both ways), and
//! - a unidirectional **video stream** from host to viewer, frames in order.
//!
//! QUIC retransmits lost packets quickly, which on a LAN costs a few
//! milliseconds. If the network can't keep up, the host skips frames
//! *before* encoding them, so the picture never breaks.

pub mod discovery;
pub mod identity;
pub mod pairing;
pub mod proto;

pub use identity::{data_dir, Fingerprint, Identity, Role, TrustStore};
pub use pairing::{
    check_number, client_endpoint, format_number, host_pair, peer_fingerprint, server_endpoint, session_binding,
    viewer_pair, Approver, HostPairResult, PairCode, ViewerPairing,
};
pub use proto::*;

/// UDP port the host listens on by default.
pub const DEFAULT_PORT: u16 = 48100;
/// Name used in TLS between host and viewer (not a real DNS name).
pub const SERVER_NAME: &str = "gamennetto";
/// Bumped only for changes that old versions can't handle at all: viewers
/// and hosts before pairing (0.1.0) can't talk to these.
pub const ALPN: &[u8] = b"gamennetto/2";
/// Additions within one ALPN raise this; each side sends newer messages only
/// when the other side's version is high enough.
/// 3: `InputEvent::Locks`.
pub const PROTOCOL_VERSION: u32 = 3;
