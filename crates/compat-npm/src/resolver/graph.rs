//! Phase 1: level-by-level (BFS, sorted by name) resolution of the ideal dependency graph.

use std::collections::{BTreeMap, BTreeSet};

use nodejs_semver::Version;
use velyr_runtime_errors::VelyrError;

use super::{PackumentSource, PlanWarning, ResolveOptions};
use crate::PackageJson;
use crate::PeerDependencyMeta;
use crate::registry::{Packument, VersionManifest};
use crate::semver::satisfies;

/// `(name, version)`.
pub(super) type NodeId = (String, String);
type SpecMap = BTreeMap<String, String>;

pub(super) struct Node {
    pub manifest: VersionManifest,
    pub edges: BTreeMap<String, NodeId>,
}

#[derive(Default)]
pub(super) struct Graph {
    pub nodes: BTreeMap<NodeId, Node>,
    pub root: BTreeMap<String, NodeId>,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Request {
    name: String,
    from: Option<NodeId>,
    spec: String,
    optional: bool,
}

pub(super) struct Builder<'a> {
    source: &'a dyn PackumentSource,
    options: &'a ResolveOptions,
    graph: Graph,
    resolved: BTreeMap<String, BTreeSet<String>>,
    warnings: Vec<PlanWarning>,
}

pub(super) fn build(
    root: &PackageJson,
    source: &dyn PackumentSource,
    options: &ResolveOptions,
) -> Result<(Graph, Vec<PlanWarning>), VelyrError> {
    let mut builder = Builder {
        source,
        options,
        graph: Graph::default(),
        resolved: BTreeMap::new(),
        warnings: Vec::new(),
    };
    let peers = required_peers(&root.peer_dependencies, &root.peer_dependencies_meta);
    let first = collect(
        None,
        &[
            (&peers, false),
            (&root.dev_dependencies, false),
            (&root.dependencies, false),
            (&root.optional_dependencies, true),
        ],
    );
    builder.expand(first)?;
    builder.check_peers()?;
    Ok((builder.graph, builder.warnings))
}

pub(super) fn required_peers(
    peers: &SpecMap,
    meta: &BTreeMap<String, PeerDependencyMeta>,
) -> SpecMap {
    let is_optional = |name: &String| meta.get(name).and_then(|m| m.optional) == Some(true);
    peers
        .iter()
        .filter(|(name, _)| !is_optional(name))
        .map(|(name, spec)| (name.clone(), spec.clone()))
        .collect()
}

/// Later layers override earlier ones, so `optionalDependencies` wins over `dependencies`.
fn collect(from: Option<&NodeId>, layers: &[(&SpecMap, bool)]) -> Vec<Request> {
    let mut merged: BTreeMap<&String, (&String, bool)> = BTreeMap::new();
    for (specs, optional) in layers {
        for (name, spec) in *specs {
            merged.insert(name, (spec, *optional));
        }
    }
    merged
        .into_iter()
        .map(|(name, (spec, optional))| Request {
            name: name.clone(),
            from: from.cloned(),
            spec: spec.clone(),
            optional,
        })
        .collect()
}

impl Builder<'_> {
    fn expand(&mut self, mut level: Vec<Request>) -> Result<(), VelyrError> {
        while !level.is_empty() {
            level.sort();
            let mut next = Vec::new();
            // The whole level is visited before a failure is returned, so a prefetching source
            // sees every packument the level needs in one pass.
            let mut failure = None;
            for request in level {
                match self.resolve(&request.name, &request.spec) {
                    Ok(manifest) => self.attach(&request, manifest, &mut next),
                    Err(error) if request.optional => self.skip_optional(&request, &error),
                    Err(error) => failure = failure.or(Some(error)),
                }
            }
            if let Some(error) = failure {
                return Err(error);
            }
            level = next;
        }
        Ok(())
    }

    fn attach(&mut self, request: &Request, manifest: VersionManifest, next: &mut Vec<Request>) {
        let id = (request.name.clone(), manifest.version.clone());
        match &request.from {
            None => self.graph.root.insert(request.name.clone(), id.clone()),
            Some(from) => self
                .graph
                .nodes
                .get_mut(from)
                .and_then(|node| node.edges.insert(request.name.clone(), id.clone())),
        };
        if self.graph.nodes.contains_key(&id) {
            return;
        }
        let peers = required_peers(
            &manifest.peer_dependencies,
            &manifest.peer_dependencies_meta,
        );
        // shortcut: failures below an optional package do not prune that package, upgrade when
        // npm's "drop the whole optional subtree" behavior is needed.
        next.extend(collect(
            Some(&id),
            &[
                (&peers, false),
                (&manifest.dependencies, false),
                (&manifest.optional_dependencies, true),
            ],
        ));
        self.resolved
            .entry(id.0.clone())
            .or_default()
            .insert(id.1.clone());
        self.graph.nodes.insert(
            id,
            Node {
                manifest,
                edges: BTreeMap::new(),
            },
        );
    }

    fn skip_optional(&mut self, request: &Request, error: &VelyrError) {
        let required_by = request
            .from
            .as_ref()
            .map_or_else(|| "root".to_owned(), label);
        let warning = PlanWarning::OptionalSkipped {
            package: request.name.clone(),
            spec: request.spec.clone(),
            required_by,
            reason: error.message().to_owned(),
        };
        tracing::warn!(?warning, "skipping optional dependency");
        self.warnings.push(warning);
    }

    fn resolve(&self, name: &str, spec: &str) -> Result<VersionManifest, VelyrError> {
        if spec.contains(':') || spec.contains('/') {
            return Err(VelyrError::unsupported(format!(
                "unsupported dependency spec `{spec}` for `{name}`"
            )));
        }
        let packument = self.source.packument(name)?;
        let version = self.pick(name, spec, &packument)?;
        let manifest = packument.versions.get(&version).ok_or_else(|| {
            VelyrError::not_found(format!("{name}@{version} missing from packument"))
        })?;
        if !self.options.platform.accepts(&manifest.os, &manifest.cpu) {
            return Err(VelyrError::unsupported(format!(
                "{name}@{version} does not support this platform"
            )));
        }
        Ok(manifest.clone())
    }

    /// Dist-tag, else an already-resolved satisfying version (dedupe), else the `latest` tag if it
    /// satisfies, else the highest satisfying version (npm-pick-manifest order).
    fn pick(&self, name: &str, spec: &str, packument: &Packument) -> Result<String, VelyrError> {
        if let Some(version) = packument.dist_tags.get(spec) {
            return Ok(version.clone());
        }
        let mut matching: Vec<(Version, &String)> = Vec::new();
        for version in packument.versions.keys() {
            let Ok(parsed) = Version::parse(version) else {
                continue;
            };
            let fits =
                satisfies(version, spec).map_err(|e| VelyrError::invalid_data(e.to_string()))?;
            if fits {
                matching.push((parsed, version));
            }
        }
        let highest = |keep: &dyn Fn(&str) -> bool| {
            matching
                .iter()
                .filter(|(_, v)| keep(v))
                .max_by(|a, b| a.0.cmp(&b.0))
                .map(|(_, v)| (*v).clone())
        };
        let reused = self.resolved.get(name);
        let latest = packument.dist_tags.get("latest");
        highest(&|v| reused.is_some_and(|set| set.contains(v)))
            .or_else(|| highest(&|v| latest.is_some_and(|l| l == v)))
            .or_else(|| highest(&|_| true))
            .ok_or_else(|| {
                VelyrError::not_found(format!("no version of `{name}` satisfies `{spec}`"))
            })
    }

    /// Warns (or fails under `strict_peer_deps`) when a dependent holds a version of a peer that
    /// its dependency's `peerDependencies` range rejects.
    fn check_peers(&mut self) -> Result<(), VelyrError> {
        let mut conflicts = Vec::new();
        let root_label = "root".to_owned();
        let holders = std::iter::once((root_label, &self.graph.root)).chain(
            self.graph
                .nodes
                .iter()
                .map(|(id, node)| (label(id), &node.edges)),
        );
        for (holder, edges) in holders {
            for target in edges.values() {
                let manifest = &self.graph.nodes[target].manifest;
                for (peer, range) in &manifest.peer_dependencies {
                    let Some(found) = edges.get(peer) else {
                        continue;
                    };
                    if satisfies(&found.1, range).unwrap_or(true) {
                        continue;
                    }
                    conflicts.push(PlanWarning::PeerConflict {
                        package: label(target),
                        peer: peer.clone(),
                        range: range.clone(),
                        found: found.1.clone(),
                        required_by: holder.clone(),
                    });
                }
            }
        }
        for conflict in conflicts {
            if self.options.strict_peer_deps {
                return Err(VelyrError::invalid_data(format!(
                    "peer dependency conflict: {conflict:?}"
                )));
            }
            tracing::warn!(?conflict, "peer dependency conflict");
            self.warnings.push(conflict);
        }
        Ok(())
    }
}

fn label(id: &NodeId) -> String {
    format!("{}@{}", id.0, id.1)
}
