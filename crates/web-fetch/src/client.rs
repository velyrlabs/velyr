//! The per-isolate `HttpClient`: built on the first fetch, so isolates that never fetch pay nothing.

use std::collections::HashMap;
use std::sync::Arc;

use rustls::ClientConfig;
use velyr_io_http::{ClientOptions, HttpClient, Limits};
use velyr_io_net::Resolver;
use velyr_runtime_errors::VelyrError;
use velyr_runtime_permissions::CapabilitySet;
use velyr_runtime_resources::QuotaContext;

/// Largest response body accepted (after the connection, before decoding).
const MAX_BODY_BYTES: u64 = 256 * 1024 * 1024;

/// Isolate slot: the client once the first request created it, and the response bodies that
/// were handed to JS (see `op_fetch_detach`).
#[derive(Default)]
pub(crate) struct FetchState {
    pub(crate) client: Option<Arc<HttpClient>>,
    pub(crate) bodies: HashMap<u64, crate::ops::DetachedBody>,
    pub(crate) next_body: u64,
    /// Requests whose body is still being uploaded after the response body left the resource
    /// table, by request id.
    pub(crate) uploads: HashMap<velyr_runtime_resources::ResourceId, Arc<crate::ops::Fetch>>,
}

/// Name of the environment variable with extra CA certificates, like `NODE_EXTRA_CA_CERTS`.
const EXTRA_CA_ENV: &str = "VELYR_EXTRA_CA_CERTS";

/// The PEM in `VELYR_EXTRA_CA_CERTS`, read by the host (not by the script, so no env permission
/// is involved). A missing file, or one without a certificate, is an error: a CA the operator
/// asked for must never be dropped silently.
fn extra_ca_pem() -> Result<Option<String>, VelyrError> {
    let Some(path) = std::env::var_os(EXTRA_CA_ENV).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let shown = path.to_string_lossy().into_owned();
    let pem = std::fs::read_to_string(&path).map_err(|err| {
        VelyrError::invalid_data(format!("{EXTRA_CA_ENV}: cannot read {shown}: {err}"))
    })?;
    if !pem.contains("-----BEGIN CERTIFICATE-----") {
        return Err(VelyrError::invalid_data(format!(
            "{EXTRA_CA_ENV}: {shown} has no PEM certificate"
        )));
    }
    Ok(Some(pem))
}

/// The platform verifier (system roots) plus the extra roots, and h2 with an h1 fallback. Extra
/// roots only add trust; verification stays on. Without extra roots, `None` if the platform
/// verifier cannot start: https requests then fail while http ones still work.
fn tls_config() -> Result<Option<Arc<ClientConfig>>, VelyrError> {
    let pem = extra_ca_pem()?;
    let alpn = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let extra: Vec<&str> = pem.iter().map(String::as_str).collect();
    match velyr_io_tls::client_config(&extra, alpn) {
        Ok(config) => Ok(Some(Arc::new(config))),
        Err(err) if pem.is_some() => {
            Err(VelyrError::invalid_data(format!("{EXTRA_CA_ENV}: {err}")))
        }
        Err(_) => Ok(None),
    }
}

pub(crate) fn new_client(
    permissions: Arc<CapabilitySet>,
    resolver: Resolver,
    quotas: Arc<QuotaContext>,
) -> Result<Arc<HttpClient>, VelyrError> {
    let limits = Limits {
        max_body_bytes: MAX_BODY_BYTES,
        quotas: Some(quotas),
        ..Limits::default()
    };
    Ok(Arc::new(HttpClient::new(
        permissions,
        resolver,
        ClientOptions {
            limits,
            tls: tls_config()?,
            ..ClientOptions::default()
        },
    )))
}
