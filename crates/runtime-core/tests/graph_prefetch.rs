//! The module graph is read before the engine links it (VELYR-192): a graph of N modules is
//! instantiated once, not once per module that was not read yet. Counted through the
//! `velyr.core.instantiate` tracing event the loader emits per instantiate call.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, Once};

use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RuntimeConfig, run_module_file};
use velyr_runtime_modules::{
    FsResolver, ModuleId, ModuleResolver, ResolveContext, ResolvedModule, Result as ModuleResult,
};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

static INSTANTIATES: AtomicUsize = AtomicUsize::new(0);
static SERIAL: Mutex<()> = Mutex::new(());

struct CountInstantiates;

impl<S: tracing::Subscriber> Layer<S> for CountInstantiates {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() == "velyr.core.instantiate" {
            INSTANTIATES.fetch_add(1, Ordering::SeqCst);
        }
    }
}

fn fixture(name: &str) -> PathBuf {
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!(
            "velyr-graph-prefetch-{}-{name}",
            std::process::id()
        ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Delegates to `FsResolver`, counting calls and distinct `(referrer, specifier)` edges.
#[derive(Default)]
struct Counting {
    calls: AtomicUsize,
    edges: Mutex<HashSet<(String, String)>>,
}

impl ModuleResolver for Counting {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &ModuleId,
        ctx: &ResolveContext,
    ) -> ModuleResult<ResolvedModule> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let edge = (referrer.as_str().to_owned(), specifier.to_owned());
        self.edges.lock().unwrap().insert(edge);
        FsResolver.resolve(specifier, referrer, ctx)
    }
}

/// Runs `main.mjs` of `dir`; how many instantiate calls it took.
fn instantiates(dir: &Path) -> usize {
    run_main(dir, None)
}

fn run_main(dir: &Path, module_resolver: Option<Arc<dyn ModuleResolver + Send + Sync>>) -> usize {
    static SUBSCRIBER: Once = Once::new();
    SUBSCRIBER.call_once(|| {
        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(CountInstantiates),
        )
        .unwrap();
    });
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    INSTANTIATES.store(0, Ordering::SeqCst);
    let config = RuntimeConfig {
        capabilities: Some(
            CapabilitySet::builder()
                .allow(Capability::ImportLocal(PathPattern::new(
                    dir.to_str().unwrap(),
                )))
                .build(),
        ),
        platform_init: Some(V8Engine::init_platform),
        module_resolver,
        ..RuntimeConfig::default()
    };
    run_module_file(
        V8Engine::new(),
        config,
        Vec::new(),
        "",
        |_, _, _| {},
        dir.join("main.mjs"),
    )
    .expect("run");
    INSTANTIATES.load(Ordering::SeqCst)
}

/// `m0` .. `m{n-1}`: each imports the next two (a chain with shortcuts) and adds its index.
fn write_graph(dir: &Path, n: usize) {
    for i in 0..n {
        let imports: String = [i + 1, i + 2]
            .iter()
            .filter(|&&j| j < n)
            .map(|j| format!("import {{ total as t{j} }} from './m{j}.mjs';\n"))
            .collect();
        let sum = match (i + 1 < n, i + 2 < n) {
            (true, _) => format!("{i} + t{}", i + 1),
            _ => i.to_string(),
        };
        std::fs::write(
            dir.join(format!("m{i}.mjs")),
            format!("{imports}/* import './nope.mjs' */ export const total = {sum};\n"),
        )
        .unwrap();
    }
}

#[test]
fn a_static_graph_is_instantiated_once() {
    let dir = fixture("static");
    write_graph(&dir, 60);
    std::fs::write(
        dir.join("main.mjs"),
        "import { total } from './m0.mjs';\n\
         if (total !== 1770) throw new Error('total ' + total);",
    )
    .unwrap();
    assert_eq!(instantiates(&dir), 1);
}

#[test]
fn an_import_graph_is_instantiated_once_per_import() {
    let dir = fixture("dynamic");
    write_graph(&dir, 60);
    std::fs::write(
        dir.join("main.mjs"),
        "const { total } = await import('./m0.mjs');\n\
         if (total !== 1770) throw new Error('total ' + total);",
    )
    .unwrap();
    assert_eq!(instantiates(&dir), 2, "main, then the import() graph");
}

#[test]
fn imports_the_scan_cannot_read_are_still_loaded() {
    let dir = fixture("fallback");
    std::fs::write(dir.join("hidden.mjs"), "export const n = 7;").unwrap();
    // A specifier with an escape: the scan skips it, the engine decodes it to `./hidden.mjs`.
    std::fs::write(
        dir.join("main.mjs"),
        "import { n } from './hidden\\u002emjs';\n\
         if (n !== 7) throw new Error('n ' + n);",
    )
    .unwrap();
    assert_eq!(
        instantiates(&dir),
        2,
        "the missed module costs one more link"
    );
}

#[test]
fn each_edge_is_resolved_once_per_graph_load() {
    let dir = fixture("edges");
    write_graph(&dir, 100);
    std::fs::write(
        dir.join("main.mjs"),
        "import { total } from './m0.mjs';\n\
         if (total !== 4950) throw new Error('total ' + total);",
    )
    .unwrap();
    let resolver = Arc::new(Counting::default());
    run_main(&dir, Some(resolver.clone()));
    let edges = resolver.edges.lock().unwrap().len();
    assert!(edges >= 198, "every edge of the graph is seen: {edges}");
    assert_eq!(resolver.calls.load(Ordering::SeqCst), edges);
}
