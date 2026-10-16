//! QUIC endpoints with TLS 1.3 and the `h3` ALPN. 0-RTT is left disabled (replayable requests).

use std::net::SocketAddr;
use std::sync::Arc;

use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::RootCertStore;
use rustls::crypto::aws_lc_rs::default_provider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use velyr_runtime_errors::{ErrorCode, VelyrError};

const ALPN_H3: &[u8] = b"h3";

fn config_error(err: impl std::fmt::Display) -> VelyrError {
    VelyrError::new(ErrorCode::Network, format!("quic tls config: {err}"))
}

/// A QUIC server endpoint bound to `addr` presenting `chain`.
pub fn server_endpoint(
    addr: SocketAddr,
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<quinn::Endpoint, VelyrError> {
    let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(config_error)?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(config_error)?;
    tls.alpn_protocols = vec![ALPN_H3.to_vec()];
    let quic = QuicServerConfig::try_from(Arc::new(tls)).map_err(config_error)?;
    let config = quinn::ServerConfig::with_crypto(Arc::new(quic));
    quinn::Endpoint::server(config, addr).map_err(config_error)
}

/// A client-only QUIC endpoint (dual-stack, ephemeral port) trusting `roots`.
pub fn client_endpoint(roots: RootCertStore) -> Result<quinn::Endpoint, VelyrError> {
    let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(config_error)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN_H3.to_vec()];
    let quic = QuicClientConfig::try_from(Arc::new(tls)).map_err(config_error)?;
    let mut endpoint =
        quinn::Endpoint::client(SocketAddr::from(([0u16; 8], 0))).map_err(config_error)?;
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(quic)));
    Ok(endpoint)
}
