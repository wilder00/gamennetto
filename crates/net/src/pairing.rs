//! Encryption, and approving a new PC.
//!
//! Both sides present their long-lived certificate (mutual TLS 1.3 inside
//! QUIC), so each knows the other's fingerprint for sure. A PC is let in
//! only if it was approved before, or is approved now in one of two ways:
//!
//! - **Number check**: both screens show the same 6-digit number, computed
//!   from this very encrypted session. Someone sitting in the middle would
//!   have two different sessions, so the numbers would differ. The person at
//!   the host allows it, and the person at the viewer confirms the number.
//! - **Code**: the host shows a 6-digit code and the viewer types it. SPAKE2
//!   proves both know the code without sending it, so watching the network
//!   reveals nothing, and each wrong try is one guess the host can count.
//!
//! Either way, both PCs then remember each other's fingerprint, and later
//! connections need nothing.

use anyhow::{anyhow, bail, Context, Result};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};

use crate::identity::{Fingerprint, Identity};
use crate::proto::{read_msg, write_msg, HostMsg, ViewerMsg};
use crate::ALPN;

// ---- TLS ---------------------------------------------------------------

fn transport() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_secs(2)));
    t.max_idle_timeout(Some(Duration::from_secs(10).try_into().unwrap()));
    Arc::new(t)
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// The host side. Every viewer must present a certificate and prove it holds
/// its key; whether that certificate is trusted is decided after the
/// handshake (`peer_fingerprint` + the trust store, or pairing).
pub fn server_endpoint(id: &Identity, addr: SocketAddr) -> Result<quinn::Endpoint> {
    let provider = provider();
    let mut tls = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(AnyKeyHolder { provider }))
        .with_single_cert(vec![id.cert.clone()], PrivateKeyDer::Pkcs8(id.key.clone_key()))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)?;
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    cfg.transport_config(transport());
    quinn::Endpoint::server(cfg, addr).with_context(|| format!("cannot listen on {addr}"))
}

/// The viewer side: presents this PC's certificate. The host's certificate
/// is checked after the handshake, like on the host.
pub fn client_endpoint(id: &Identity) -> Result<quinn::Endpoint> {
    let provider = provider();
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous() // no certificate authorities: fingerprints instead (see module docs)
        .with_custom_certificate_verifier(Arc::new(AnyKeyHolder { provider }))
        .with_client_auth_cert(vec![id.cert.clone()], PrivateKeyDer::Pkcs8(id.key.clone_key()))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(crypto));
    cfg.transport_config(transport());
    let mut ep = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
    ep.set_default_client_config(cfg);
    Ok(ep)
}

/// The other side's fingerprint (it proved it holds the matching key).
pub fn peer_fingerprint(conn: &quinn::Connection) -> Result<Fingerprint> {
    let certs = conn
        .peer_identity()
        .and_then(|any| any.downcast::<Vec<CertificateDer<'static>>>().ok())
        .context("the other side sent no certificate")?;
    let first = certs.first().context("the other side sent no certificate")?;
    Ok(Fingerprint::of(first))
}

/// 32 bytes both ends of *this* encrypted session share, and nobody else.
pub fn session_binding(conn: &quinn::Connection) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    conn.export_keying_material(&mut out, b"EXPORTER-gamennetto-pairing", b"")
        .map_err(|_| anyhow!("could not read the session's keys"))?;
    Ok(out)
}

/// The 6-digit number both screens show for the number check.
pub fn check_number(binding: &[u8; 32]) -> u32 {
    let d = ring::digest::digest(&ring::digest::SHA256, &[b"gamennetto check number".as_slice(), binding].concat());
    u32::from_be_bytes([d.as_ref()[0], d.as_ref()[1], d.as_ref()[2], d.as_ref()[3]]) % 1_000_000
}

pub fn format_number(n: u32) -> String {
    format!("{:03} {:03}", n / 1000, n % 1000)
}

/// Accepts any certificate, but always checks the handshake signature, which
/// proves the other side holds the certificate's key.
#[derive(Debug)]
struct AnyKeyHolder {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for AnyKeyHolder {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

impl ClientCertVerifier for AnyKeyHolder {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn client_auth_mandatory(&self) -> bool {
        true
    }
    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

// ---- The 6-digit code ----------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PairCode(u32);

impl PairCode {
    pub fn random() -> Self {
        use ring::rand::SecureRandom;
        let mut b = [0u8; 4];
        ring::rand::SystemRandom::new().fill(&mut b).expect("system random numbers");
        Self(u32::from_le_bytes(b) % 1_000_000)
    }

    /// Accepts "482913", "482 913" or "482-913".
    pub fn parse(input: &str) -> Result<Self> {
        let digits: String = input.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
        if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_digit()) {
            bail!("the code has 6 digits, like 482 913");
        }
        Ok(Self(digits.parse()?))
    }
}

impl std::fmt::Display for PairCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format_number(self.0))
    }
}

impl std::fmt::Debug for PairCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairCode(***)") // never print it by accident
    }
}

fn spake_password(code: PairCode) -> spake2::Password {
    spake2::Password::new(format!("{:06}", code.0).into_bytes())
}

fn spake_ids() -> (spake2::Identity, spake2::Identity) {
    (spake2::Identity::new(b"gamennetto viewer"), spake2::Identity::new(b"gamennetto host"))
}

/// Proof that this side derived the same SPAKE2 key, tied to this session.
fn confirm_mac(key: &[u8], binding: &[u8; 32], side: &[u8]) -> Vec<u8> {
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    ring::hmac::sign(&k, &[side, binding.as_slice()].concat()).as_ref().to_vec()
}

fn verify_mac(key: &[u8], binding: &[u8; 32], side: &[u8], mac: &[u8]) -> bool {
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    ring::hmac::verify(&k, &[side, binding.as_slice()].concat(), mac).is_ok()
}

// ---- The pairing exchange --------------------------------------------------

/// Asked on the host for the number check: shows the number, returns
/// whether the person there allowed it.
pub type Approver = Box<dyn FnOnce(u32) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPairResult {
    /// Approved: remember this viewer.
    Paired,
    /// A wrong code: count it (the host changes its code after a few).
    WrongCode,
    /// Refused at the host, or not possible.
    Refused,
}

/// Host side. `code`: None if code pairing is off right now (too many wrong
/// tries). `approve`: None if nobody can answer at the host.
pub async fn host_pair(
    send: &mut quinn::SendStream,
    recv: &mut quinn::RecvStream,
    binding: &[u8; 32],
    code: Option<PairCode>,
    approve: Option<Approver>,
) -> Result<HostPairResult> {
    write_msg(send, &HostMsg::PairingRequired { by_code: code.is_some(), by_approval: approve.is_some() }).await?;
    match read_msg::<ViewerMsg>(recv).await? {
        ViewerMsg::PairByApproval => {
            let Some(approve) = approve else {
                write_msg(
                    send,
                    &HostMsg::PairRefused { reason: "nobody can approve at the host; use the code".into() },
                )
                .await?;
                return Ok(HostPairResult::Refused);
            };
            if approve(check_number(binding)).await {
                write_msg(send, &HostMsg::Paired).await?;
                Ok(HostPairResult::Paired)
            } else {
                write_msg(send, &HostMsg::PairRefused { reason: "refused at the host".into() }).await?;
                Ok(HostPairResult::Refused)
            }
        }
        ViewerMsg::PairByCode { spake } => {
            let Some(code) = code else {
                write_msg(send, &HostMsg::PairRefused { reason: "too many wrong codes; try again in a minute".into() })
                    .await?;
                return Ok(HostPairResult::Refused);
            };
            let (viewer_id, host_id) = spake_ids();
            let (state, mine) =
                spake2::Spake2::<spake2::Ed25519Group>::start_b(&spake_password(code), &viewer_id, &host_id);
            write_msg(send, &HostMsg::PairSpake { spake: mine }).await?;
            let Ok(key) = state.finish(&spake) else {
                return Ok(HostPairResult::WrongCode);
            };
            let ViewerMsg::PairConfirm { mac } = read_msg(recv).await? else {
                bail!("the viewer broke off pairing");
            };
            // The viewer proves first; only then does the host answer.
            if !verify_mac(&key, binding, b"viewer", &mac) {
                write_msg(send, &HostMsg::PairRefused { reason: "wrong code".into() }).await?;
                return Ok(HostPairResult::WrongCode);
            }
            write_msg(send, &HostMsg::PairConfirm { mac: confirm_mac(&key, binding, b"host") }).await?;
            Ok(HostPairResult::Paired)
        }
        other => bail!("expected a pairing answer, got {other:?}"),
    }
}

/// What the viewer offers once the host asks for pairing.
pub enum ViewerPairing {
    Code(PairCode),
    /// The number check. The caller shows `check_number` first and, after
    /// success, asks the person whether the host showed the same number.
    Approval,
}

/// Viewer side, after the host sent `PairingRequired`. Ok means the host
/// accepted this PC (and, with a code, proved it knows the code too).
pub async fn viewer_pair(
    send: &mut quinn::SendStream,
    recv: &mut quinn::RecvStream,
    binding: &[u8; 32],
    how: ViewerPairing,
) -> Result<()> {
    match how {
        ViewerPairing::Approval => {
            write_msg(send, &ViewerMsg::PairByApproval).await?;
            match read_msg::<HostMsg>(recv).await? {
                HostMsg::Paired => Ok(()),
                HostMsg::PairRefused { reason } => bail!("{reason}"),
                other => bail!("unexpected answer from the host: {other:?}"),
            }
        }
        ViewerPairing::Code(code) => {
            let (viewer_id, host_id) = spake_ids();
            let (state, mine) =
                spake2::Spake2::<spake2::Ed25519Group>::start_a(&spake_password(code), &viewer_id, &host_id);
            write_msg(send, &ViewerMsg::PairByCode { spake: mine }).await?;
            let theirs = match read_msg::<HostMsg>(recv).await? {
                HostMsg::PairSpake { spake } => spake,
                HostMsg::PairRefused { reason } => bail!("{reason}"),
                other => bail!("unexpected answer from the host: {other:?}"),
            };
            let key = state.finish(&theirs).map_err(|_| anyhow!("pairing failed"))?;
            write_msg(send, &ViewerMsg::PairConfirm { mac: confirm_mac(&key, binding, b"viewer") }).await?;
            match read_msg::<HostMsg>(recv).await? {
                HostMsg::PairConfirm { mac } if verify_mac(&key, binding, b"host", &mac) => Ok(()),
                HostMsg::PairConfirm { .. } => bail!("the host could not prove it knows the code: not connecting"),
                HostMsg::PairRefused { reason } => bail!("{reason}"),
                other => bail!("unexpected answer from the host: {other:?}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SERVER_NAME;

    #[test]
    fn code_parsing_is_forgiving() {
        let a = PairCode::parse("482 913").unwrap();
        assert!(a == PairCode::parse("482-913").unwrap() && a == PairCode::parse("482913").unwrap());
        assert_eq!(a.to_string(), "482 913");
        assert_eq!(PairCode::parse("000007").unwrap().to_string(), "000 007");
        assert!(PairCode::parse("12345").is_err());
        assert!(PairCode::parse("12345a").is_err());
        assert_eq!(format!("{a:?}"), "PairCode(***)");
    }

    /// A host and a viewer on loopback with their own identities.
    async fn session(
        host: &Identity,
        viewer: &Identity,
    ) -> ((quinn::Connection, quinn::Endpoint), (quinn::Connection, quinn::Endpoint)) {
        let server = server_endpoint(host, "127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = server.local_addr().unwrap();
        let s2 = server.clone();
        let accept = tokio::spawn(async move { s2.accept().await.unwrap().await.unwrap() });
        let client = client_endpoint(viewer).unwrap();
        let vconn = client.connect(addr, SERVER_NAME).unwrap().await.unwrap();
        let hconn = accept.await.unwrap();
        ((hconn, server), (vconn, client))
    }

    #[tokio::test]
    async fn each_side_sees_the_others_real_fingerprint() {
        let (h, v) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let ((hc, _s), (vc, _c)) = session(&h, &v).await;
        assert_eq!(peer_fingerprint(&hc).unwrap(), v.fingerprint);
        assert_eq!(peer_fingerprint(&vc).unwrap(), h.fingerprint);
        // Both ends compute the same check number.
        assert_eq!(check_number(&session_binding(&hc).unwrap()), check_number(&session_binding(&vc).unwrap()));
    }

    /// Runs one pairing attempt; returns (host result, viewer result).
    async fn pair_once(
        host_code: Option<PairCode>,
        approve: Option<bool>,
        how: ViewerPairing,
    ) -> (HostPairResult, Result<()>) {
        let (h, v) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let ((hc, _s), (vc, _c)) = session(&h, &v).await;
        let (hb, vb) = (session_binding(&hc).unwrap(), session_binding(&vc).unwrap());
        let host = tokio::spawn(async move {
            let (mut send, mut recv) = hc.accept_bi().await.unwrap();
            let _: ViewerMsg = read_msg(&mut recv).await.unwrap(); // opening message
            let approver: Option<Approver> = approve.map(|yes| {
                let f: Approver = Box::new(move |_n: u32| {
                    let fut: Pin<Box<dyn Future<Output = bool> + Send>> = Box::pin(async move { yes });
                    fut
                });
                f
            });
            let r = host_pair(&mut send, &mut recv, &hb, host_code, approver).await.unwrap();
            let _ = send.finish();
            tokio::time::sleep(Duration::from_millis(100)).await;
            r
        });
        let (mut send, mut recv) = vc.open_bi().await.unwrap();
        write_msg(&mut send, &ViewerMsg::RequestKeyframe).await.unwrap(); // opens the stream
        let HostMsg::PairingRequired { .. } = read_msg(&mut recv).await.unwrap() else { panic!() };
        let viewer = viewer_pair(&mut send, &mut recv, &vb, how).await;
        (host.await.unwrap(), viewer)
    }

    #[tokio::test]
    async fn right_code_pairs_both_sides() {
        let code = PairCode::random();
        let (h, v) = pair_once(Some(code), None, ViewerPairing::Code(code)).await;
        assert_eq!(h, HostPairResult::Paired);
        assert!(v.is_ok(), "{v:?}");
    }

    #[tokio::test]
    async fn wrong_code_is_refused_and_counted() {
        let code = PairCode::parse("111111").unwrap();
        let (h, v) = pair_once(Some(code), None, ViewerPairing::Code(PairCode::parse("222222").unwrap())).await;
        assert_eq!(h, HostPairResult::WrongCode);
        assert!(v.is_err());
    }

    #[tokio::test]
    async fn approval_yes_and_no() {
        let (h, v) = pair_once(None, Some(true), ViewerPairing::Approval).await;
        assert_eq!(h, HostPairResult::Paired);
        assert!(v.is_ok());
        let (h, v) = pair_once(None, Some(false), ViewerPairing::Approval).await;
        assert_eq!(h, HostPairResult::Refused);
        assert!(v.is_err());
        // Nobody at the host to approve: refused, not hanging.
        let (h, v) = pair_once(Some(PairCode::random()), None, ViewerPairing::Approval).await;
        assert_eq!(h, HostPairResult::Refused);
        assert!(v.is_err());
    }
}
