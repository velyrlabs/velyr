//! `velyr install [--frozen] [--production] [--allow-scripts=..]` and the sync pipeline shared with add/remove:
//! resolve -> velyr.lock -> global cache -> node_modules.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use velyr_compat_npm::PackageJson;
use velyr_compat_npm::cache::NpmCache;
use velyr_compat_npm::linker::{self, LinkEntry};
use velyr_compat_npm::lockfile::{Importer, LockedPackage, Lockfile};
use velyr_compat_npm::registry::{Dist, Npmrc, Packument, RegistryClient};
use velyr_compat_npm::resolver::{InstallPlan, PackumentSource, Platform, ResolveOptions};
use velyr_compat_npm::scripts::{self, Allow, Target};
use velyr_runtime_errors::VelyrError;

use super::manifest::Manifest;

const LOCK_FILE: &str = "velyr.lock";

#[derive(Default, Clone, Copy)]
pub(super) struct Mode {
    pub frozen: bool,
    pub production: bool,
}

pub(super) struct Ctx {
    pub project: PathBuf,
    pub client: RegistryClient,
    cache: NpmCache,
    /// Lifecycle scripts stay denied unless `install` opts in.
    pub scripts: Allow,
}

impl Ctx {
    pub(super) fn open() -> Result<Self, VelyrError> {
        let project = std::env::current_dir()
            .map_err(|e| VelyrError::internal("cannot read the working directory").with_cause(e))?;
        let env = |key: &str| std::env::var(key).ok();
        let npmrc = Npmrc::load(
            Npmrc::user_path().as_deref(),
            Some(&project.join(".npmrc")),
            &env,
        )?;
        let cache = match std::env::var_os("VELYR_NPM_CACHE_DIR") {
            Some(dir) => NpmCache::at(dir),
            None => NpmCache::default_location()?,
        };
        Ok(Self {
            project,
            client: RegistryClient::new(npmrc),
            cache,
            scripts: Allow::None,
        })
    }
}

pub fn install(
    frozen: bool,
    production: bool,
    allow_scripts: Vec<String>,
    allow_all_scripts: bool,
) -> ExitCode {
    finish(block_on(async {
        let mut ctx = Ctx::open()?;
        ctx.scripts = script_policy(&ctx.project, allow_scripts, allow_all_scripts)?;
        let manifest = Manifest::load(&ctx.project)?;
        sync(&ctx, &manifest, None, Mode { frozen, production }).await
    }))
}

/// The `--allow-*` flags plus `allowScripts` of the nearest velyr.json.
fn script_policy(
    project: &std::path::Path,
    mut names: Vec<String>,
    all: bool,
) -> Result<Allow, VelyrError> {
    if all {
        eprintln!("warning: --allow-all-scripts runs the install scripts of every package");
        return Ok(Allow::All);
    }
    let loaded = crate::config::discover(project)
        .map_err(|e| VelyrError::invalid_data(format!("velyr.json: {e}")))?;
    names.extend(
        loaded
            .and_then(|l| l.config.allow_scripts)
            .unwrap_or_default(),
    );
    Ok(if names.is_empty() {
        Allow::None
    } else {
        Allow::Packages(names.into_iter().collect())
    })
}

/// Runs the permitted scripts (dependencies first, then the root project) and warns about the rest.
fn run_scripts(
    ctx: &Ctx,
    package_json: &PackageJson,
    entries: &[LinkEntry],
    added: &[String],
) -> Result<(), VelyrError> {
    scripts::forget(&ctx.project, added);
    let mut targets: Vec<Target> = entries
        .iter()
        .map(|e| Target {
            name: e.name.clone(),
            version: e.version.clone(),
            dir: ctx.project.join(&e.path),
            root: false,
        })
        .collect();
    targets.push(Target {
        name: package_json.name.clone().unwrap_or_else(|| ".".to_owned()),
        version: package_json
            .version
            .clone()
            .unwrap_or_else(|| "0.0.0".to_owned()),
        dir: ctx.project.clone(),
        root: true,
    });
    let exe = if cfg!(feature = "node") {
        std::env::current_exe().ok()
    } else {
        None
    };
    let report = scripts::run(&ctx.project, &targets, &ctx.scripts, exe.as_deref())?;
    for label in &report.ran {
        println!("ran scripts of {label}");
    }
    if report.skipped.is_empty() {
        return Ok(());
    }
    eprintln!(
        "warning: {} package(s) have install scripts that were not run:",
        report.skipped.len()
    );
    for s in &report.skipped {
        eprintln!(
            "  {} ({})  allow with: velyr install --allow-scripts={}",
            s.label,
            s.events.join(", "),
            s.name
        );
    }
    Ok(())
}

pub(super) fn block_on<T>(fut: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(fut)
}

pub(super) fn finish(result: Result<(), VelyrError>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Resolves `manifest`, updates velyr.lock, fills the cache and links node_modules. When
/// `save` is set, that manifest is written to package.json once resolution succeeded.
pub(super) async fn sync(
    ctx: &Ctx,
    manifest: &Manifest,
    save: Option<&Manifest>,
    mode: Mode,
) -> Result<(), VelyrError> {
    let started = Instant::now();
    let package_json = manifest.typed()?;
    let lock_path = ctx.project.join(LOCK_FILE);
    let existing = match std::fs::read_to_string(&lock_path) {
        Ok(raw) => Some((Lockfile::parse(&raw)?, raw)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(VelyrError::internal("cannot read velyr.lock").with_cause(e)),
    };
    if mode.frozen {
        let Some((lock, _)) = &existing else {
            return Err(VelyrError::not_found(
                "--frozen needs a velyr.lock, run `velyr install` first",
            ));
        };
        lock.check_frozen(&package_json)?;
    }

    let plan = resolve(ctx, &package_json, existing.as_ref().map(|(l, _)| l), mode).await?;
    let lock = build_lock(&package_json, &plan)?;
    // A lock from before the graph was recorded is compared as it would have been written.
    let same = |old: &Lockfile| {
        if old.complete {
            old == &lock
        } else {
            old == &lock.without_graph()
        }
    };
    if mode.frozen && existing.as_ref().is_some_and(|(old, _)| !same(old)) {
        return Err(VelyrError::invalid_data(
            "velyr.lock is out of date with the resolved tree",
        ));
    }
    let entries = fetch_all(ctx, &plan).await?;

    if let Some(edited) = save {
        edited.save()?;
    }
    // --production plans a subset, which must not shrink the lock.
    let rendered = lock.render();
    if !mode.production && existing.as_ref().is_none_or(|(_, raw)| raw != &rendered) {
        std::fs::write(&lock_path, rendered)
            .map_err(|e| VelyrError::internal("cannot write velyr.lock").with_cause(e))?;
    }
    let report = linker::link(&ctx.project, &entries)?;

    for label in &report.added {
        println!("+ {label}");
    }
    for label in &report.removed {
        println!("- {label}");
    }
    println!(
        "added {}, removed {} in {:.2}s",
        report.added.len(),
        report.removed.len(),
        started.elapsed().as_secs_f64()
    );
    run_scripts(ctx, &package_json, &entries, &report.added)
}

async fn resolve(
    ctx: &Ctx,
    package_json: &PackageJson,
    lock: Option<&Lockfile>,
    mode: Mode,
) -> Result<InstallPlan, VelyrError> {
    let mut root = package_json.clone();
    if mode.production {
        root.dev_dependencies.clear();
    }
    // A complete lock holds the whole graph: try it without the registry first.
    if let Some(plan) = lock.and_then(|lock| offline_plan(lock, package_json, &root)) {
        return Ok(plan);
    }
    let mut have = HashMap::new();
    if let Some(lock) = lock {
        match resolve_with(ctx, &root, &mut have, Some(pins(lock))).await {
            Ok(plan) => return Ok(plan),
            // A changed range may need versions the lock never saw.
            Err(_) if !mode.frozen => {}
            Err(e) => return Err(e),
        }
    }
    resolve_with(ctx, &root, &mut have, None).await
}

/// The packuments of a complete lock, nothing else: a name outside it is unknown, never fetched.
struct LockedGraph(BTreeMap<String, Arc<Packument>>);

impl PackumentSource for LockedGraph {
    fn packument(&self, name: &str) -> Result<Arc<Packument>, VelyrError> {
        self.0
            .get(name)
            .cloned()
            .ok_or_else(|| VelyrError::not_found(format!("`{name}` is not in velyr.lock")))
    }
}

/// The plan rebuilt from a complete lock that still matches `package.json`. `None` when the lock
/// cannot answer (old format, edited manifest, dist-tag ranges): the caller then asks the
/// registry, as it always did. Skipped optionals stay skipped, they are simply not in the lock.
fn offline_plan(
    lock: &Lockfile,
    package_json: &PackageJson,
    root: &PackageJson,
) -> Option<InstallPlan> {
    if !lock.is_satisfied_by(package_json) {
        return None;
    }
    let options = ResolveOptions {
        platform: Platform::current(),
        strict_peer_deps: false,
    };
    let source = LockedGraph(lock.packuments()?);
    velyr_compat_npm::resolver::resolve(root, &source, &options).ok()
}

/// `name -> locked versions`.
fn pins(lock: &Lockfile) -> BTreeMap<String, BTreeSet<String>> {
    let mut pins: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for key in lock.packages.keys() {
        if let Some((name, version)) = key.rsplit_once('@') {
            pins.entry(name.to_owned())
                .or_default()
                .insert(version.to_owned());
        }
    }
    pins
}

struct Prefetched<'a> {
    have: &'a HashMap<String, Arc<Packument>>,
    pins: Option<BTreeMap<String, BTreeSet<String>>>,
    missing: RefCell<BTreeSet<String>>,
}

impl PackumentSource for Prefetched<'_> {
    fn packument(&self, name: &str) -> Result<Arc<Packument>, VelyrError> {
        let Some(found) = self.have.get(name) else {
            self.missing.borrow_mut().insert(name.to_owned());
            return Err(VelyrError::not_found(format!(
                "packument `{name}` not loaded"
            )));
        };
        let Some(locked) = self.pins.as_ref().and_then(|p| p.get(name)) else {
            return Ok(found.clone());
        };
        let mut pinned = (**found).clone();
        pinned.versions.retain(|v, _| locked.contains(v));
        pinned.dist_tags.retain(|_, v| locked.contains(v));
        Ok(Arc::new(pinned))
    }
}

/// The resolver is sync: run it, fetch whatever it asked for, run again until nothing is missing.
/// Misses are checked even after a failure because the resolver swallows errors of optionals.
async fn resolve_with(
    ctx: &Ctx,
    root: &PackageJson,
    have: &mut HashMap<String, Arc<Packument>>,
    pins: Option<BTreeMap<String, BTreeSet<String>>>,
) -> Result<InstallPlan, VelyrError> {
    let options = ResolveOptions {
        platform: Platform::current(),
        strict_peer_deps: false,
    };
    loop {
        let source = Prefetched {
            have,
            pins: pins.clone(),
            missing: RefCell::new(BTreeSet::new()),
        };
        let result = velyr_compat_npm::resolver::resolve(root, &source, &options);
        let missing = source.missing.into_inner();
        if missing.is_empty() {
            return result;
        }
        let names: Vec<String> = missing.into_iter().collect();
        have.extend(ctx.client.packuments(&names).await?);
    }
}

fn build_lock(package_json: &PackageJson, plan: &InstallPlan) -> Result<Lockfile, VelyrError> {
    let mut lock = Lockfile {
        complete: true,
        ..Lockfile::default()
    };
    lock.importers.insert(
        ".".to_owned(),
        Importer {
            dependencies: package_json.dependencies.clone(),
            dev_dependencies: package_json.dev_dependencies.clone(),
            optional_dependencies: package_json.optional_dependencies.clone(),
        },
    );
    for planned in plan.packages.values() {
        let integrity = planned.integrity.clone().ok_or_else(|| {
            VelyrError::invalid_data(format!(
                "{}@{} has no integrity hash",
                planned.name, planned.version
            ))
        })?;
        lock.packages.insert(
            format!("{}@{}", planned.name, planned.version),
            LockedPackage {
                bin: planned.bin.clone(),
                cpu: planned.cpu.clone(),
                dependencies: planned.dependencies.clone(),
                has_install_script: planned.has_install_script,
                integrity,
                optional_dependencies: planned.optional_dependencies.clone(),
                os: planned.os.clone(),
                peer_dependencies: planned.peer_dependencies.clone(),
                resolved: planned.tarball.clone(),
            },
        );
    }
    Ok(lock)
}

async fn fetch_all(ctx: &Ctx, plan: &InstallPlan) -> Result<Vec<LinkEntry>, VelyrError> {
    let mut sources: HashMap<String, PathBuf> = HashMap::new();
    let mut entries = Vec::new();
    for (path, planned) in &plan.packages {
        let integrity = planned.integrity.clone().ok_or_else(|| {
            VelyrError::invalid_data(format!("{} has no integrity hash", planned.name))
        })?;
        if !sources.contains_key(&integrity) {
            let dist = Dist {
                tarball: planned.tarball.clone(),
                integrity: Some(integrity.clone()),
                shasum: None,
            };
            let dir = ctx
                .cache
                .ensure(&integrity, |dest| {
                    let (client, dist) = (&ctx.client, &dist);
                    async move { client.download_tarball(dist, &dest).await }
                })
                .await?;
            sources.insert(integrity.clone(), dir);
        }
        entries.push(LinkEntry {
            path: path.clone(),
            name: planned.name.clone(),
            version: planned.version.clone(),
            source: sources[&integrity].clone(),
            integrity,
        });
    }
    Ok(entries)
}
