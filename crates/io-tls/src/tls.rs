//! rustls configuration and PEM handling.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, ServerConfig};
use rustls_platform_verifier::Verifier;
use velyr_runtime_errors::{ErrorCode, VelyrError};

/// Per-isolate server configs, one per `listenTls` listener, released when it closes.
#[derive(Default)]
pub(crate) struct TlsState {
    next: u64,
    servers: HashMap<u64, Arc<ServerConfig>>,
}

impl TlsState {
    pub fn add_server(&mut self, config: ServerConfig) -> u64 {
        self.next += 1;
        self.servers.insert(self.next, Arc::new(config));
        self.next
    }

    pub fn server(&self, id: u64) -> Result<Arc<ServerConfig>, VelyrError> {
        self.servers
            .get(&id)
            .cloned()
            .ok_or_else(VelyrError::bad_resource)
    }

    pub fn release_server(&mut self, id: u64) {
        self.servers.remove(&id);
    }
}

pub(crate) fn tls_error(err: impl std::fmt::Display) -> VelyrError {
    VelyrError::new(ErrorCode::Tls, err.to_string())
}

pub(crate) fn handshake_error(err: io::Error) -> VelyrError {
    tls_error(err)
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

fn certificates(pem: &str) -> Result<Vec<CertificateDer<'static>>, VelyrError> {
    CertificateDer::pem_slice_iter(pem.as_bytes())
        .collect::<Result<_, _>>()
        .map_err(|_| VelyrError::invalid_data("invalid PEM certificate"))
}

/// How long a handshake may take unless the caller says otherwise.
const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// `handshakeTimeoutMs` of an options object: a positive number, or the default when absent.
pub(crate) fn handshake_timeout(value: Option<&serde_json::Value>) -> Result<Duration, VelyrError> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(DEFAULT_HANDSHAKE_TIMEOUT),
        Some(ms) => ms
            .as_f64()
            .filter(|ms| ms.is_finite() && *ms > 0.0)
            .map(|ms| Duration::from_secs_f64(ms / 1000.0))
            .ok_or_else(|| {
                VelyrError::invalid_data("handshakeTimeoutMs must be a positive number")
            }),
    }
}

pub(crate) struct ClientOptions {
    pub hostname: String,
    pub handshake_timeout: Duration,
    pub ca_certs: Vec<CertificateDer<'static>>,
    pub alpn: Vec<Vec<u8>>,
}

fn string_list(value: &serde_json::Value, key: &str) -> Result<Vec<String>, VelyrError> {
    let invalid = || VelyrError::invalid_data(format!("{key} must be an array of strings"));
    match value.get(key) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned).ok_or_else(invalid))
            .collect(),
        Some(_) => Err(invalid()),
    }
}

/// Client config trusting the platform roots plus `ca_pem` (same checks as `connectTls`).
pub fn client_config(ca_pem: &[&str], alpn: Vec<Vec<u8>>) -> Result<ClientConfig, VelyrError> {
    let mut ca_certs = Vec::new();
    for pem in ca_pem {
        ca_certs.extend(certificates(pem)?);
    }
    ClientOptions {
        hostname: String::new(),
        handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
        ca_certs,
        alpn,
    }
    .config()
}

pub(crate) fn alpn_list(value: &serde_json::Value) -> Result<Vec<Vec<u8>>, VelyrError> {
    Ok(string_list(value, "alpnProtocols")?
        .into_iter()
        .map(String::into_bytes)
        .collect())
}

impl ClientOptions {
    /// `json` is `{hostname, caCerts: [pem], alpnProtocols: [name]}` as built by `tls.js`.
    pub fn parse(json: &str) -> Result<Self, VelyrError> {
        let value: serde_json::Value = serde_json::from_str(json)
            .map_err(|_| VelyrError::invalid_data("invalid TLS options"))?;
        let hostname = value
            .get("hostname")
            .and_then(|h| h.as_str())
            .filter(|h| !h.is_empty())
            .ok_or_else(|| VelyrError::invalid_data("TLS hostname is required"))?
            .to_owned();
        let mut ca_certs = Vec::new();
        for pem in string_list(&value, "caCerts")? {
            ca_certs.extend(certificates(&pem)?);
        }
        Ok(Self {
            hostname,
            handshake_timeout: handshake_timeout(value.get("handshakeTimeoutMs"))?,
            ca_certs,
            alpn: alpn_list(&value)?,
        })
    }

    pub fn config(&self) -> Result<ClientConfig, VelyrError> {
        let provider = provider();
        let verifier = Verifier::new_with_extra_roots(self.ca_certs.clone(), provider.clone())
            .map_err(tls_error)?;
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(tls_error)?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        config.alpn_protocols = self.alpn.clone();
        Ok(config)
    }
}

/// Validates a PEM chain and key and builds the server config (same checks as `listenTls`).
pub fn server_config(
    cert_pem: &str,
    key_pem: &str,
    alpn: Vec<Vec<u8>>,
) -> Result<ServerConfig, VelyrError> {
    let chain = certificates(cert_pem)?;
    if chain.is_empty() {
        return Err(VelyrError::invalid_data("no certificate found in cert PEM"));
    }
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
        .map_err(|_| VelyrError::invalid_data("invalid PEM private key"))?;
    let mut config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(tls_error)?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|err| {
            VelyrError::invalid_data(format!("certificate and key do not match: {err}"))
        })?;
    config.alpn_protocols = alpn;
    Ok(config)
}
