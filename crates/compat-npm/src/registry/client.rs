use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderValue};
use reqwest::{Response, StatusCode};
use tokio::io::AsyncWriteExt;
use tracing::Instrument;
use velyr_runtime_errors::{ErrorCode, VelyrError};

use super::integrity::Expected;
use super::npmrc::Npmrc;
use super::packument::{Dist, Packument};

/// Packument requests in flight at once.
pub const PACKUMENT_CONCURRENCY: usize = 16;

const ABBREVIATED_ACCEPT: &str = "application/vnd.npm.install-v1+json";

/// Retry configuration; `attempts` counts the first try.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub attempts: u32,
    /// Delay before the second attempt; doubles on each following one.
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            base_delay: Duration::from_millis(250),
        }
    }
}

/// A failed attempt, tagged with whether another attempt may help.
struct Attempt {
    error: VelyrError,
    retryable: bool,
}

impl Attempt {
    fn fatal(error: VelyrError) -> Self {
        Self {
            error,
            retryable: false,
        }
    }
}

/// HTTP client of the installer. Holds auth settings; `Debug` never shows tokens.
#[derive(Debug)]
pub struct RegistryClient {
    http: reqwest::Client,
    npmrc: Npmrc,
    retry: RetryPolicy,
    packuments: Mutex<HashMap<String, Arc<Packument>>>,
}

impl RegistryClient {
    pub fn new(npmrc: Npmrc) -> Self {
        Self {
            http: reqwest::Client::new(),
            npmrc,
            retry: RetryPolicy::default(),
            packuments: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Fetches the abbreviated packument, cached in memory for this client's lifetime.
    pub async fn packument(&self, name: &str) -> Result<Arc<Packument>, VelyrError> {
        let url = format!(
            "{}{}",
            self.npmrc.registry_for(name),
            name.replace('/', "%2F")
        );
        if let Some(hit) = self.packuments.lock().unwrap().get(&url) {
            return Ok(hit.clone());
        }
        let span = tracing::info_span!("velyr.compat-npm.packument", package = name);
        let body = self
            .with_retry_policy(|| self.get_bytes(&url, Some(ABBREVIATED_ACCEPT)))
            .instrument(span)
            .await?;
        let packument: Arc<Packument> = Arc::new(
            serde_json::from_slice(&body)
                .map_err(|e| VelyrError::invalid_data("malformed packument").with_cause(e))?,
        );
        self.packuments
            .lock()
            .unwrap()
            .insert(url, packument.clone());
        Ok(packument)
    }

    /// Fetches several packuments at once, at most [`PACKUMENT_CONCURRENCY`] in flight. Names are
    /// deduplicated; the first failure aborts the rest.
    pub async fn packuments(
        &self,
        names: &[String],
    ) -> Result<Vec<(String, Arc<Packument>)>, VelyrError> {
        type Fetch<'a> =
            Pin<Box<dyn Future<Output = Result<(String, Arc<Packument>), VelyrError>> + 'a>>;
        let unique: BTreeSet<&String> = names.iter().collect();
        let mut waiting = unique.into_iter();
        let mut running: Vec<Fetch<'_>> = Vec::new();
        let mut done = Vec::new();
        std::future::poll_fn(|cx| {
            loop {
                while running.len() < PACKUMENT_CONCURRENCY {
                    let Some(name) = waiting.next() else { break };
                    running.push(Box::pin(async move {
                        Ok((name.clone(), self.packument(name).await?))
                    }));
                }
                if running.is_empty() {
                    return Poll::Ready(Ok(()));
                }
                let before = running.len();
                let mut i = 0;
                while i < running.len() {
                    match running[i].as_mut().poll(cx) {
                        Poll::Ready(Ok(found)) => {
                            done.push(found);
                            drop(running.swap_remove(i));
                        }
                        Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                        Poll::Pending => i += 1,
                    }
                }
                if running.len() == before {
                    return Poll::Pending;
                }
            }
        })
        .await?;
        Ok(done)
    }

    /// Downloads a tarball to `dest`, verifying SRI first. On any error `dest` is untouched.
    pub async fn download_tarball(&self, dist: &Dist, dest: &Path) -> Result<(), VelyrError> {
        let expected = Expected::from_dist(dist)?;
        let temp = temp_path(dest);
        let span = tracing::info_span!("velyr.compat-npm.download", url = %dist.tarball);
        let result = async {
            self.with_retry_policy(|| self.download_to(&dist.tarball, &expected, &temp))
                .await?;
            tokio::fs::rename(&temp, dest)
                .await
                .map_err(|e| VelyrError::internal("cannot move tarball into place").with_cause(e))
        }
        .instrument(span)
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&temp).await;
        }
        result
    }

    async fn download_to(
        &self,
        url: &str,
        expected: &Expected,
        temp: &Path,
    ) -> Result<(), Attempt> {
        let mut response = self.send(url, None).await?;
        let mut hasher = expected.hasher();
        let mut file = tokio::fs::File::create(temp).await.map_err(io_attempt)?;
        while let Some(chunk) = response.chunk().await.map_err(transport_attempt)? {
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(io_attempt)?;
        }
        file.flush().await.map_err(io_attempt)?;
        if expected.matches(hasher) {
            return Ok(());
        }
        Err(Attempt::fatal(
            VelyrError::invalid_data("tarball integrity check failed")
                .with_public_context("url", url),
        ))
    }

    async fn get_bytes(&self, url: &str, accept: Option<&str>) -> Result<Vec<u8>, Attempt> {
        let response = self.send(url, accept).await?;
        Ok(response.bytes().await.map_err(transport_attempt)?.to_vec())
    }

    async fn send(&self, url: &str, accept: Option<&str>) -> Result<Response, Attempt> {
        let mut request = self.http.get(url);
        if let Some(accept) = accept {
            request = request.header(ACCEPT, accept);
        }
        if let Some(token) = self.npmrc.auth_token_for(url) {
            let mut value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
                Attempt::fatal(VelyrError::invalid_data(
                    "auth token is not a valid header value",
                ))
            })?;
            value.set_sensitive(true);
            request = request.header(AUTHORIZATION, value);
        }
        let response = request.send().await.map_err(transport_attempt)?;
        status_to_result(response, url)
    }

    async fn with_retry_policy<T, F, Fut>(&self, mut op: F) -> Result<T, VelyrError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, Attempt>>,
    {
        let mut delay = self.retry.base_delay;
        let mut attempt = 1;
        loop {
            match op().await {
                Ok(value) => return Ok(value),
                Err(Attempt { error, retryable }) if retryable && attempt < self.retry.attempts => {
                    tracing::warn!(attempt, code = %error.code(), "request failed, retrying");
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                    attempt += 1;
                }
                Err(Attempt { error, .. }) => return Err(error),
            }
        }
    }
}

fn status_to_result(response: Response, url: &str) -> Result<Response, Attempt> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let retryable = status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS;
    let error = if status == StatusCode::NOT_FOUND {
        VelyrError::not_found("package or tarball not found")
    } else {
        VelyrError::new(
            ErrorCode::Network,
            format!("registry responded with HTTP {status}"),
        )
    };
    Err(Attempt {
        error: error.with_public_context("url", url),
        retryable,
    })
}

fn transport_attempt(e: reqwest::Error) -> Attempt {
    let e = e.without_url();
    let code = if e.is_timeout() {
        ErrorCode::Timeout
    } else {
        ErrorCode::Network
    };
    Attempt {
        error: VelyrError::new(code, "registry request failed").with_cause(e),
        retryable: true,
    }
}

fn io_attempt(e: std::io::Error) -> Attempt {
    Attempt::fatal(VelyrError::internal("cannot write tarball").with_cause(e))
}

/// Hidden sibling of `dest`, so the final rename stays on one filesystem.
fn temp_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    dest.with_file_name(format!(".{name}.{}.part", std::process::id()))
}
