//! `velyr run --inspect[=ADDR]`, `--inspect-brk[=ADDR]`, `--inspect-wait[=ADDR]` (VELYR-120).
//!
//! Only these flags turn the inspector on: never `-A` (ADR-013) nor `velyr.json`. The bind still
//! goes through the `InspectorBind` capability, so `--deny-inspect` wins.

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use velyr_runtime_modules::SourceTransform;

use crate::permissions::Permissions;
use crate::run::{EngineKind, Prepared};

const DEFAULT_ADDR: &str = "127.0.0.1:9229";
const DEFAULT_PORT: u16 = 9229;

#[derive(Debug, Default, clap::Args)]
pub struct InspectFlags {
    /// Like --inspect, and pause on the first statement of the entry module.
    #[arg(long, num_args = 0..=1, require_equals = true, value_name = "ADDR", default_missing_value = DEFAULT_ADDR, conflicts_with = "inspect_wait")]
    inspect_brk: Option<String>,

    /// Like --inspect, and wait for a debugger before running the entry module.
    #[arg(long, num_args = 0..=1, require_equals = true, value_name = "ADDR", default_missing_value = DEFAULT_ADDR)]
    inspect_wait: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Run,
    Wait,
    Break,
}

#[derive(Debug)]
pub struct Inspect {
    addr: SocketAddr,
    #[cfg_attr(not(feature = "inspector"), allow(dead_code))]
    mode: Mode,
}

/// The inspector the command line asks for, if any. `--inspect-brk`/`--inspect-wait` grant the
/// bind the way `--inspect` does.
pub fn from_flags(
    flags: InspectFlags,
    permissions: &mut Permissions,
) -> Result<Option<Inspect>, String> {
    let (value, mode) = match (flags.inspect_brk, flags.inspect_wait, &permissions.inspect) {
        (Some(value), _, _) => (value, Mode::Break),
        (_, Some(value), _) => (value, Mode::Wait),
        (_, _, Some(values)) => (
            values
                .first()
                .cloned()
                .unwrap_or_else(|| DEFAULT_ADDR.into()),
            Mode::Run,
        ),
        _ => return Ok(None),
    };
    let addr = parse_addr(&value)?;
    if mode != Mode::Run {
        permissions.inspect.get_or_insert_with(Vec::new).push(value);
    }
    Ok(Some(Inspect { addr, mode }))
}

/// `IP` or `IP:PORT` (`[v6]:PORT`); the port defaults to 9229.
fn parse_addr(value: &str) -> Result<SocketAddr, String> {
    value
        .parse::<SocketAddr>()
        .or_else(|_| {
            value
                .parse::<IpAddr>()
                .map(|ip| SocketAddr::new(ip, DEFAULT_PORT))
        })
        .map_err(|_| format!("error: invalid inspector address {value:?}: expected IP[:PORT]"))
}

/// Refuses what the inspector cannot or may not do, before any socket is opened.
fn check(prepared: &Prepared, engine: EngineKind, addr: SocketAddr) -> Result<(), String> {
    if !matches!(engine, EngineKind::V8) {
        return Err("error: the inspector needs the V8 engine (--engine v8)".into());
    }
    let host = addr.ip().to_string();
    let request = velyr_runtime_permissions::Request::InspectorBind(&host);
    if !prepared.capabilities.check(&request).is_allow() {
        return Err(format!("error: inspector bind to {host} is denied"));
    }
    if !addr.ip().is_loopback() {
        eprintln!(
            "warning: the inspector listens on {addr}; anyone who can reach it can run code in this process"
        );
    }
    Ok(())
}

#[cfg(feature = "inspector")]
pub use enabled::{run, source_transform};

#[cfg(feature = "inspector")]
mod enabled {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    use velyr_engine_v8::{InspectorEvent, InspectorOptions, V8Engine};
    use velyr_runtime_inspector::{Event, InspectorServer, Target};
    use velyr_runtime_modules::{MediaType, Result, Transformed};

    use super::*;
    use crate::run::{Worker, run_on};

    /// Set once the inspector is on; the inspector is process-wide, like this flag.
    static INLINE_SOURCE_MAPS: AtomicBool = AtomicBool::new(false);

    pub fn run(
        file: &Path,
        prepared: &Prepared,
        engine: EngineKind,
        inspect: Inspect,
    ) -> std::result::Result<(), String> {
        check(prepared, engine, inspect.addr)?;
        let server = InspectorServer::bind(inspect.addr, target(file)).map_err(|err| {
            format!(
                "error: cannot start the inspector on {}: {err}",
                inspect.addr
            )
        })?;
        eprintln!("Debugger listening on {}", server.ws_url());
        let (events, received) = mpsc::channel();
        server.serve(move |event| {
            let _ = events.send(match event {
                Event::Connected(outgoing) => InspectorEvent::Connected(outgoing),
                Event::Message(text) => InspectorEvent::Message(text),
                Event::Disconnected => InspectorEvent::Disconnected,
            });
        });
        INLINE_SOURCE_MAPS.store(true, Ordering::Relaxed);
        let engine = V8Engine::new().with_inspector(InspectorOptions {
            events: received,
            wait_for_session: inspect.mode != Mode::Run,
            break_on_start: inspect.mode == Mode::Break,
        });
        let init = Some(V8Engine::init_platform as fn());
        run_on(engine, init, file, prepared, None, Worker::default())
    }

    fn target(file: &Path) -> Target {
        let path = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
        Target {
            title: file.display().to_string(),
            url: format!("file://{}", path.display()),
        }
    }

    /// While inspecting, transformed modules carry their source map inline so the debugger
    /// shows the original source (TypeScript, JSX).
    pub fn source_transform(
        inner: Option<Arc<dyn SourceTransform>>,
    ) -> Option<Arc<dyn SourceTransform>> {
        if !INLINE_SOURCE_MAPS.load(Ordering::Relaxed) {
            return inner;
        }
        inner.map(|inner| Arc::new(InlineSourceMaps(inner)) as Arc<dyn SourceTransform>)
    }

    struct InlineSourceMaps(Arc<dyn SourceTransform>);

    impl SourceTransform for InlineSourceMaps {
        fn handles(&self, media_type: MediaType) -> bool {
            self.0.handles(media_type)
        }

        fn transform(
            &self,
            specifier: &str,
            source: &str,
            media_type: MediaType,
        ) -> Result<Transformed> {
            let mut out = self.0.transform(specifier, source, media_type)?;
            if let Some(map) = &out.source_map {
                out.code
                    .push_str(&velyr_runtime_inspector::inline_source_map_comment(map));
            }
            Ok(out)
        }
    }
}

#[cfg(not(feature = "inspector"))]
pub fn run(
    _file: &Path,
    prepared: &Prepared,
    engine: EngineKind,
    inspect: Inspect,
) -> Result<(), String> {
    check(prepared, engine, inspect.addr)?;
    Err(
        "error: the inspector is not compiled in (build velyr-cli with the `inspector` feature)"
            .into(),
    )
}

#[cfg(not(feature = "inspector"))]
pub fn source_transform(
    inner: Option<Arc<dyn SourceTransform>>,
) -> Option<Arc<dyn SourceTransform>> {
    inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permissions(inspect: Option<Vec<String>>) -> Permissions {
        Permissions {
            inspect,
            ..Permissions::default()
        }
    }

    #[test]
    fn nothing_is_inspected_without_a_flag() {
        let mut granted = Permissions {
            allow_all: true,
            ..Permissions::default()
        };
        assert!(
            from_flags(InspectFlags::default(), &mut granted)
                .unwrap()
                .is_none()
        );
        assert_eq!(granted.inspect, None);
    }

    #[test]
    fn brk_and_wait_grant_their_address() {
        let flags = InspectFlags {
            inspect_brk: Some("0.0.0.0:9230".into()),
            inspect_wait: None,
        };
        let mut granted = permissions(None);
        let inspect = from_flags(flags, &mut granted).unwrap().unwrap();
        assert_eq!(inspect.mode, Mode::Break);
        assert_eq!(inspect.addr, "0.0.0.0:9230".parse().unwrap());
        assert_eq!(granted.inspect, Some(vec!["0.0.0.0:9230".to_owned()]));
    }

    #[test]
    fn bare_inspect_defaults_to_loopback_9229() {
        let mut granted = permissions(Some(vec![]));
        let inspect = from_flags(InspectFlags::default(), &mut granted)
            .unwrap()
            .unwrap();
        assert_eq!(inspect.mode, Mode::Run);
        assert_eq!(inspect.addr, DEFAULT_ADDR.parse().unwrap());
        assert_eq!(parse_addr("::1").unwrap(), "[::1]:9229".parse().unwrap());
        assert!(parse_addr("localhost:9229").is_err());
    }
}
