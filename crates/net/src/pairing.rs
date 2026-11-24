//! Pairing and encryption.
//!
//! The host makes a fresh certificate and shows a short **pairing code**
//! derived from it. The viewer types the code; the connection only succeeds
//! if the host's certificate matches. That way nobody else on the network can
//! pretend to be your host, and all traffic is encrypted (TLS 1.3 inside QUIC).

use anyhow::{bail, Context, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

use crate::{ALPN, SERVER_NAME};

/// Number of hex digits in the pairing code: 12 digits = 48 bits, shown as
/// `ABCD-EF12-3456`. Too many to guess or forge during a pairing attempt.
const CODE_DIGITS: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCode(String);

impl PairingCode {
    fn from_cert(der: &[u8]) -> Self {
        let digest = ring::digest::digest(&ring::digest::SHA256, der);
        let hex: String = digest.as_ref().iter().map(|b| format!("{b:02X}")).collect();
        Self(hex[..CODE_DIGITS].to_string())
    }

    /// Accepts what a person types: any case, with or without dashes/spaces.
    pub fn parse(input: &str) -> Result<Self> {
        let clean: String =
            input.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
        if clean.len() != CODE_DIGITS || !clean.chars().all(|c| c.is_ascii_hexdigit()) {
            bail!("a pairing code has {CODE_DIGITS} characters, 0-9 and A-F (like ABCD-EF12-3456)");
        }
        Ok(Self(clean))
    }
}

impl std::fmt::Display for PairingCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = &self.0;
        write!(f, "{}-{}-{}", &s[0..4], &s[4..8], &s[8..12])
    }
}

/// The host's certificate, key and the code derived from them.
pub struct HostIdentity {
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
    pub code: PairingCode,
}

impl HostIdentity {
    pub fn generate() -> Result<Self> {
        let ck = rcgen::generate_simple_self_signed(vec![SERVER_NAME.to_string()])?;
        let cert = ck.cert.der().clone();
        let key = PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der());
        let code = PairingCode::from_cert(&cert);
        Ok(Self { cert, key, code })
    }
}

fn transport() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_secs(2)));
    t.max_idle_timeout(Some(Duration::from_secs(10).try_into().unwrap()));
    Arc::new(t)
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub fn server_endpoint(id: &HostIdentity, addr: SocketAddr) -> Result<quinn::Endpoint> {
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(vec![id.cert.clone()], PrivateKeyDer::Pkcs8(id.key.clone_key()))?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)?;
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    cfg.transport_config(transport());
    quinn::Endpoint::server(cfg, addr).with_context(|| format!("cannot listen on {addr}"))
}

/// A client endpoint that only trusts a host whose certificate matches `code`.
pub fn client_endpoint(code: PairingCode) -> Result<quinn::Endpoint> {
    let provider = provider();
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous() // custom check below replaces the usual CA check
        .with_custom_certificate_verifier(Arc::new(CodeVerifier { code, provider }))
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)?;
    let mut cfg = quinn::ClientConfig::new(Arc::new(crypto));
    cfg.transport_config(transport());
    let mut ep = quinn::Endpoint::client("0.0.0.0:0".parse()?)?;
    ep.set_default_client_config(cfg);
    Ok(ep)
}

#[derive(Debug)]
struct CodeVerifier {
    code: PairingCode,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for CodeVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if PairingCode::from_cert(end_entity) == self.code {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("wrong pairing code (or someone else answered)".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_parsing_is_forgiving() {
        let a = PairingCode::parse("abcd-ef12-3456").unwrap();
        let b = PairingCode::parse("ABCD EF12 3456").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.to_string(), "ABCD-EF12-3456");
        assert!(PairingCode::parse("1234").is_err());
        assert!(PairingCode::parse("ZZZZ-ZZZZ-ZZZZ").is_err());
    }

    #[tokio::test]
    async fn connects_with_right_code_only() {
        let id = HostIdentity::generate().unwrap();
        let server = server_endpoint(&id, "127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = server.local_addr().unwrap();
        tokio::spawn(async move {
            while let Some(inc) = server.accept().await {
                if let Ok(conn) = inc.await {
                    // keep the connection alive briefly
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    drop(conn);
                }
            }
        });

        let good = client_endpoint(id.code.clone()).unwrap();
        assert!(good.connect(addr, SERVER_NAME).unwrap().await.is_ok());

        let wrong = PairingCode::parse("000000000000").unwrap();
        let bad = client_endpoint(wrong).unwrap();
        assert!(bad.connect(addr, SERVER_NAME).unwrap().await.is_err());
    }
}
