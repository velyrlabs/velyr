//! File watching: [`FsWatcher`] (notify plus a 50 ms debounce) and the `op_fs_watch_*` ops behind
//! `Velyr.watchFs`. Opening authorizes `FsRead` on the canonical form of every path; a watch is a
//! resource, so closing it (or the isolate ending) stops the OS watcher.

use std::any::Any;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, event::ModifyKind};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use tokio::sync::{Mutex, mpsc};
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::{CapabilityKind, OpContext, op_async};
use velyr_runtime_resources::{QuotaClass, Resource, ResourceId};

use crate::ops::authorized_raw;

const DEBOUNCE: Duration = Duration::from_millis(50);
/// Events kept for a consumer that is slower than the file system.
const QUEUE: usize = 4096;

/// One debounced change: what happened and to which paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsEvent {
    /// `create`, `modify`, `remove`, `rename` or `other`.
    pub kind: &'static str,
    pub paths: Vec<PathBuf>,
}

type Events = Arc<Mutex<mpsc::Receiver<FsEvent>>>;

/// Watches paths until dropped.
pub struct FsWatcher {
    debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    events: Events,
}

fn kind_name(kind: &EventKind) -> Option<&'static str> {
    match kind {
        EventKind::Create(_) => Some("create"),
        EventKind::Remove(_) => Some("remove"),
        EventKind::Modify(ModifyKind::Name(_)) => Some("rename"),
        EventKind::Modify(_) => Some("modify"),
        EventKind::Any | EventKind::Other => Some("other"),
        // Reads and opens: our own reads must not wake the watcher.
        EventKind::Access(_) => None,
    }
}

/// FSEvents reports every flag a path ever collected (a plain edit of an old file can say
/// `create`, a delete can say `modify`), so create, modify and remove are decided from the file
/// system: gone is `remove`; born after the watch started and not reported yet is `create`;
/// anything else is `modify`. Other kinds (`rename`, `other`) pass through.
struct Classifier {
    started: SystemTime,
    announced: HashSet<PathBuf>,
}

impl Classifier {
    fn classify(&mut self, kind: &'static str, path: Option<&Path>) -> &'static str {
        let Some(path) = path.filter(|_| matches!(kind, "create" | "modify" | "remove")) else {
            return kind;
        };
        let Ok(meta) = path.symlink_metadata() else {
            return "remove";
        };
        match meta.created() {
            Ok(born) if born >= self.started && self.announced.insert(path.to_owned()) => "create",
            Ok(_) => "modify",
            // No birth time on this file system: trust the platform unless it says `remove`.
            Err(_) if kind == "remove" => "modify",
            Err(_) => kind,
        }
    }
}

impl FsWatcher {
    pub fn open(paths: &[PathBuf], recursive: bool) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel(QUEUE);
        let mut classifier = Classifier {
            started: SystemTime::now(),
            announced: HashSet::new(),
        };
        let handler = move |result: DebounceEventResult| match result {
            Ok(events) => {
                for event in events {
                    let Some(kind) = kind_name(&event.kind) else {
                        continue;
                    };
                    let kind = classifier.classify(kind, event.paths.first().map(PathBuf::as_path));
                    // shortcut: a full queue drops the event, upgrade with an overflow marker.
                    let _ = tx.try_send(FsEvent {
                        kind,
                        paths: event.paths.clone(),
                    });
                }
            }
            Err(errors) => {
                for error in errors {
                    tracing::warn!(target: "velyr.fs.watch", %error, "watch error");
                }
            }
        };
        let debouncer = new_debouncer(DEBOUNCE, None, handler)?;
        let mut watcher = Self {
            debouncer,
            events: Arc::new(Mutex::new(rx)),
        };
        for path in paths {
            watcher.add(path, recursive)?;
        }
        Ok(watcher)
    }

    /// Starts watching one more path.
    pub fn add(&mut self, path: &Path, recursive: bool) -> notify::Result<()> {
        let mode = if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        self.debouncer.watch(path, mode)
    }

    /// The next event; `None` once the watcher is gone.
    pub async fn next(&self) -> Option<FsEvent> {
        self.events.lock().await.recv().await
    }

    fn events(&self) -> Events {
        self.events.clone()
    }
}

struct WatchRes(FsWatcher);

impl Resource for WatchRes {
    fn kind(&self) -> &'static str {
        "fs_watch"
    }

    fn close(self: Box<Self>) {}

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub(crate) const OPS: [(HostFunctionDef, CapabilityKind); 3] = [
    (
        HostFunctionDef {
            name: "op_fs_watch_open",
            arity: 2,
            callback: op_fs_watch_open,
        },
        CapabilityKind::FsRead,
    ),
    (
        HostFunctionDef {
            name: "op_fs_watch_next",
            arity: 1,
            callback: op_fs_watch_next,
        },
        CapabilityKind::None,
    ),
    (
        HostFunctionDef {
            name: "op_fs_watch_close",
            arity: 1,
            callback: op_fs_watch_close,
        },
        CapabilityKind::None,
    ),
];

fn op_context(ctx: &mut dyn CallContext) -> Result<&mut OpContext, HostError> {
    ctx.isolate_slot::<OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))
}

fn id_arg(args: &[Value], op: &str) -> Result<ResourceId, HostError> {
    match args.first() {
        Some(Value::Number(id)) => Ok(ResourceId::from_js_number(*id)?),
        _ => Err(HostError::TypeError(format!("{op} expects a resource id"))),
    }
}

/// `op_fs_watch_open(pathsJson, recursive) -> id`: `pathsJson` is a JSON array of strings. Every
/// path is authorized before anything is watched.
fn op_fs_watch_open(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    const OP: &str = "op_fs_watch_open";
    let (Some(Value::String(json)), Some(Value::Bool(recursive))) = (args.first(), args.get(1))
    else {
        return Err(HostError::TypeError(format!(
            "{OP} expects (pathsJson, recursive)"
        )));
    };
    let raw: Vec<String> = serde_json::from_str(&json.to_string_lossy())
        .map_err(|_| HostError::TypeError(format!("{OP} expects a JSON array of strings")))?;
    if raw.is_empty() {
        return Err(HostError::TypeError(format!(
            "{OP} expects at least one path"
        )));
    }
    let mut paths = Vec::with_capacity(raw.len());
    for path in &raw {
        paths.push(authorized_raw(ctx, OP, path, false)?);
    }
    let watcher = FsWatcher::open(&paths, *recursive)
        .map_err(|e| VelyrError::internal(format!("cannot watch: {e}")))?;
    let id =
        op_context(ctx)?
            .resources
            .insert(Box::new(WatchRes(watcher)), QuotaClass::Files, None)?;
    Ok(Value::Number(id.to_js_number()))
}

/// `op_fs_watch_next(id) -> Promise<string | null>`: JSON `{kind, paths}` of the next event.
fn op_fs_watch_next(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let id = id_arg(args, "op_fs_watch_next")?;
    let events = op_context(ctx)?.resources.get::<WatchRes>(id)?.0.events();
    op_async(ctx, move |token| async move {
        let mut events = events.lock().await;
        let event = tokio::select! {
            () = token.cancelled() => return Err(VelyrError::cancelled()),
            event = events.recv() => event,
        };
        Ok(match event {
            Some(event) => {
                let paths: Vec<String> = event
                    .paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                CompletionValue::String(
                    serde_json::json!({"kind": event.kind, "paths": paths}).to_string(),
                )
            }
            None => CompletionValue::Null,
        })
    })
}

/// `op_fs_watch_close(id)`: stops the watch; a pending `next` settles with `null`.
fn op_fs_watch_close(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let id = id_arg(args, "op_fs_watch_close")?;
    op_context(ctx)?.resources.close(id)?;
    Ok(Value::Undefined)
}
