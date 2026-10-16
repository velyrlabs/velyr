//! Registry client tests against a local mock server (no real network).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use sha2::{Digest, Sha512};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use velyr_compat_npm::registry::{Dist, Npmrc, RegistryClient, RetryPolicy};
use velyr_runtime_errors::ErrorCode;

#[derive(Clone, Debug)]
struct Recorded {
    path: String,
    headers: HashMap<String, String>,
}

struct Reply {
    status: u16,
    body: Vec<u8>,
}

fn ok(body: impl Into<Vec<u8>>) -> Reply {
    Reply {
        status: 200,
        body: body.into(),
    }
}

fn status(code: u16) -> Reply {
    Reply {
        status: code,
        body: Vec::new(),
    }
}

struct Mock {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl Mock {
    fn url(&self) -> String {
        format!("http://{}/", self.addr)
    }
    fn recorded(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

async fn serve(handler: impl Fn(usize, &Recorded) -> Reply + Send + Sync + 'static) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    let handler = Arc::new(handler);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let (log, handler) = (log.clone(), handler.clone());
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buf = [0u8; 2048];
                while !raw.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => raw.extend_from_slice(&buf[..n]),
                    }
                }
                let text = String::from_utf8_lossy(&raw).into_owned();
                let mut lines = text.split("\r\n");
                let path = lines.next().unwrap().split(' ').nth(1).unwrap().to_owned();
                let headers = lines
                    .filter_map(|l| l.split_once(": "))
                    .map(|(k, v)| (k.to_ascii_lowercase(), v.to_owned()))
                    .collect();
                let recorded = Recorded { path, headers };
                let index = {
                    let mut guard = log.lock().unwrap();
                    guard.push(recorded.clone());
                    guard.len() - 1
                };
                let reply = handler(index, &recorded);
                let head = format!(
                    "HTTP/1.1 {} X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    reply.status,
                    reply.body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&reply.body).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    Mock { addr, requests }
}

fn fast_retry() -> RetryPolicy {
    RetryPolicy {
        attempts: 3,
        base_delay: Duration::from_millis(1),
    }
}

fn no_env(_: &str) -> Option<String> {
    None
}

fn client_for(registry: &str) -> RegistryClient {
    let npmrc = Npmrc::parse(&format!("registry={registry}\n"), &no_env);
    RegistryClient::new(npmrc).with_retry(fast_retry())
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("velyr-registry-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sri_sha512(bytes: &[u8]) -> String {
    let digest = Sha512::digest(bytes);
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(digest)
    )
}

const PACKUMENT: &str = include_str!("fixtures/registry/left-pad.json");

#[test]
fn npmrc_parses_registries_and_expands_env_without_leaking_token() {
    let env = |key: &str| (key == "NPM_TOKEN").then(|| "s3cr3t-token".to_owned());
    let npmrc = Npmrc::parse(
        "# comment\nregistry=https://default.example/\n@acme:registry=https://acme.example/npm/\n//acme.example/npm/:_authToken=${NPM_TOKEN}\n",
        &env,
    );
    assert_eq!(npmrc.registry_for("left-pad"), "https://default.example/");
    assert_eq!(npmrc.registry_for("@acme/ui"), "https://acme.example/npm/");
    assert_eq!(
        npmrc.auth_token_for("https://acme.example/npm/@acme%2Fui"),
        Some("s3cr3t-token")
    );
    assert_eq!(npmrc.auth_token_for("https://acme.example/other"), None);
    assert!(!format!("{npmrc:?}").contains("s3cr3t-token"));
}

#[test]
fn npmrc_project_overrides_user() {
    let dir = temp_dir("npmrc-load");
    let user = dir.join("user.npmrc");
    let project = dir.join("project.npmrc");
    std::fs::write(
        &user,
        "registry=https://user.example/\n@a:registry=https://a.example/\n",
    )
    .unwrap();
    std::fs::write(&project, "registry=https://project.example/\n").unwrap();
    let npmrc = Npmrc::load(Some(&user), Some(&project), &no_env).unwrap();
    assert_eq!(npmrc.registry_for("x"), "https://project.example/");
    assert_eq!(npmrc.registry_for("@a/b"), "https://a.example/");
    let missing = Npmrc::load(Some(&dir.join("nope")), None, &no_env).unwrap();
    assert_eq!(missing.registry_for("x"), "https://registry.npmjs.org/");
}

#[tokio::test]
async fn packument_uses_abbreviated_accept_and_caches() {
    let mock = serve(|_, _| ok(PACKUMENT)).await;
    let client = client_for(&mock.url());
    let first = client.packument("left-pad").await.unwrap();
    let second = client.packument("left-pad").await.unwrap();
    assert_eq!(first.name, "left-pad");
    assert_eq!(first.dist_tags["latest"], "1.3.0");
    assert!(
        first.versions["1.3.0"]
            .dist
            .tarball
            .ends_with("left-pad-1.3.0.tgz")
    );
    assert_eq!(second.name, "left-pad");
    let requests = mock.recorded();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/left-pad");
    assert_eq!(
        requests[0].headers["accept"],
        "application/vnd.npm.install-v1+json"
    );
}

#[tokio::test]
async fn scoped_package_name_is_encoded() {
    let mock = serve(|_, _| ok(PACKUMENT)).await;
    client_for(&mock.url()).packument("@acme/ui").await.unwrap();
    assert_eq!(mock.recorded()[0].path, "/@acme%2Fui");
}

#[tokio::test]
async fn retries_5xx_and_429_then_succeeds() {
    let mock = serve(|i, _| match i {
        0 => status(503),
        1 => status(429),
        _ => ok(PACKUMENT),
    })
    .await;
    client_for(&mock.url()).packument("left-pad").await.unwrap();
    assert_eq!(mock.recorded().len(), 3);
}

#[tokio::test]
async fn gives_up_after_three_attempts_with_network_error() {
    let mock = serve(|_, _| status(500)).await;
    let err = client_for(&mock.url())
        .packument("left-pad")
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::Network);
    assert_eq!(mock.recorded().len(), 3);
}

#[tokio::test]
async fn does_not_retry_4xx() {
    let mock = serve(|_, _| status(404)).await;
    let err = client_for(&mock.url())
        .packument("left-pad")
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::NotFound);
    assert_eq!(mock.recorded().len(), 1);

    let mock = serve(|_, _| status(400)).await;
    let err = client_for(&mock.url())
        .packument("left-pad")
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::Network);
    assert_eq!(mock.recorded().len(), 1);
}

#[tokio::test]
async fn invalid_packument_json_is_invalid_data() {
    let mock = serve(|_, _| ok("not json")).await;
    let err = client_for(&mock.url())
        .packument("left-pad")
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidData);
}

#[tokio::test]
async fn tarball_with_valid_sri_is_written() {
    let body = b"tarball-bytes".to_vec();
    let served = body.clone();
    let mock = serve(move |_, _| ok(served.clone())).await;
    let dir = temp_dir("tar-ok");
    let dest = dir.join("pkg.tgz");
    let dist = Dist {
        tarball: format!("{}pkg.tgz", mock.url()),
        integrity: Some(sri_sha512(&body)),
        shasum: None,
    };
    client_for(&mock.url())
        .download_tarball(&dist, &dest)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
}

#[tokio::test]
async fn tarball_with_wrong_integrity_writes_nothing() {
    let mock = serve(|_, _| ok("tampered")).await;
    let dir = temp_dir("tar-bad");
    let dest = dir.join("pkg.tgz");
    let dist = Dist {
        tarball: format!("{}pkg.tgz", mock.url()),
        integrity: Some(sri_sha512(b"original")),
        shasum: None,
    };
    let err = client_for(&mock.url())
        .download_tarball(&dist, &dest)
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidData);
    assert!(!dest.exists());
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "no temp leftovers"
    );
    assert_eq!(
        mock.recorded().len(),
        1,
        "integrity mismatch is not retried"
    );
}

#[tokio::test]
async fn sha1_shasum_fallback_only_when_no_sha512() {
    let body = b"legacy".to_vec();
    let served = body.clone();
    let mock = serve(move |_, _| ok(served.clone())).await;
    let dir = temp_dir("tar-sha1");
    let sha1 = {
        use sha1::{Digest, Sha1};
        Sha1::digest(&body)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let dist = Dist {
        tarball: format!("{}legacy.tgz", mock.url()),
        integrity: None,
        shasum: Some(sha1.clone()),
    };
    let client = client_for(&mock.url());
    client
        .download_tarball(&dist, &dir.join("ok.tgz"))
        .await
        .unwrap();
    assert!(dir.join("ok.tgz").exists());

    let wrong = Dist {
        shasum: Some("0".repeat(40)),
        ..dist.clone()
    };
    let err = client
        .download_tarball(&wrong, &dir.join("bad.tgz"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidData);
    assert!(!dir.join("bad.tgz").exists());

    // sha512 present and wrong must fail even if the sha1 matches.
    let both = Dist {
        integrity: Some(sri_sha512(b"other")),
        shasum: Some(sha1),
        ..dist
    };
    let err = client
        .download_tarball(&both, &dir.join("both.tgz"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidData);
    assert!(!dir.join("both.tgz").exists());
}

#[tokio::test]
async fn tarball_without_any_integrity_is_rejected() {
    let mock = serve(|_, _| ok("x")).await;
    let dir = temp_dir("tar-none");
    let dist = Dist {
        tarball: format!("{}x.tgz", mock.url()),
        integrity: None,
        shasum: None,
    };
    let err = client_for(&mock.url())
        .download_tarball(&dist, &dir.join("x.tgz"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidData);
    assert!(!dir.join("x.tgz").exists());
}

#[tokio::test]
async fn authorization_is_sent_only_to_the_matching_host() {
    let default_mock = serve(|_, _| ok(PACKUMENT)).await;
    let scoped_mock = serve(|_, _| ok(PACKUMENT)).await;
    let env = |key: &str| (key == "ACME_TOKEN").then(|| "acme-secret".to_owned());
    let npmrc = Npmrc::parse(
        &format!(
            "registry={}\n@acme:registry={}\n//{}/:_authToken=${{ACME_TOKEN}}\n",
            default_mock.url(),
            scoped_mock.url(),
            scoped_mock.addr
        ),
        &env,
    );
    let client = RegistryClient::new(npmrc).with_retry(fast_retry());

    client.packument("@acme/ui").await.unwrap();
    client.packument("left-pad").await.unwrap();

    assert_eq!(
        scoped_mock.recorded()[0]
            .headers
            .get("authorization")
            .map(String::as_str),
        Some("Bearer acme-secret")
    );
    assert!(
        !default_mock.recorded()[0]
            .headers
            .contains_key("authorization")
    );
}

#[tokio::test]
async fn tarball_on_unrelated_host_gets_no_authorization() {
    let tarball_host = serve(|_, _| ok("data")).await;
    let registry = serve(|_, _| ok(PACKUMENT)).await;
    let npmrc = Npmrc::parse(
        &format!(
            "registry={}\n//{}/:_authToken=tok\n",
            registry.url(),
            registry.addr
        ),
        &no_env,
    );
    let client = RegistryClient::new(npmrc).with_retry(fast_retry());
    let dir = temp_dir("tar-auth");
    let dist = Dist {
        tarball: format!("{}a.tgz", tarball_host.url()),
        integrity: Some(sri_sha512(b"data")),
        shasum: None,
    };
    client
        .download_tarball(&dist, &dir.join("a.tgz"))
        .await
        .unwrap();
    assert!(
        !tarball_host.recorded()[0]
            .headers
            .contains_key("authorization")
    );
}

#[test]
fn version_manifest_parses_platform_and_peer_meta() {
    let packument: velyr_compat_npm::registry::Packument = serde_json::from_str(
        r#"{"name":"fsevents","versions":{"2.0.0":{"version":"2.0.0","os":["darwin"],"cpu":["arm64","x64"],
        "peerDependenciesMeta":{"react":{"optional":true}},"dist":{"tarball":"http://x/t.tgz"}}}}"#,
    )
    .unwrap();
    let manifest = &packument.versions["2.0.0"];
    assert_eq!(manifest.os, ["darwin"]);
    assert_eq!(manifest.cpu, ["arm64", "x64"]);
    assert_eq!(
        manifest.peer_dependencies_meta["react"].optional,
        Some(true)
    );
}
