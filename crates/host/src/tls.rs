//! TLS configuration: TLS 1.3 only, mutual authentication by pinned keys.
//!
//! There is no certificate authority. The tablet pins the host's certificate
//! fingerprint from the QR code; the host accepts any well-formed client
//! certificate during the handshake (proving the client holds its private
//! key) and then checks its fingerprint against the paired-devices list
//! before sending anything. See `server.rs`.

use std::sync::Arc;

use rustls::client::danger::HandshakeSignatureValid;
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, ServerConfig, SignatureScheme};

use crate::identity::HostIdentity;

pub fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub fn server_config(identity: &HostIdentity) -> Result<ServerConfig, rustls::Error> {
    let provider = provider();
    let mut config = ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(AnyClientKey { provider }))
        .with_single_cert(vec![identity.cert.clone()], identity.private_key())?;
    config.alpn_protocols = vec![b"tabextend/1".to_vec()];
    Ok(config)
}

/// Requires a client certificate and a valid handshake signature from its
/// key, but leaves trust to the paired-devices check after the handshake.
#[derive(Debug)]
struct AnyClientKey {
    provider: Arc<CryptoProvider>,
}

impl ClientCertVerifier for AnyClientKey {
    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
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
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
