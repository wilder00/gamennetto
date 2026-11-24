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
pub mod pairing;
pub mod proto;

pub use pairing::{client_endpoint, server_endpoint, HostIdentity, PairingCode};
pub use proto::*;

/// UDP port the host listens on by default.
pub const DEFAULT_PORT: u16 = 48100;
/// Name used in TLS between host and viewer (not a real DNS name).
pub const SERVER_NAME: &str = "gamennetto";
pub const ALPN: &[u8] = b"gamennetto/1";
pub const PROTOCOL_VERSION: u32 = 1;
