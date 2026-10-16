//! Phase 2: npm-style hoisted placement. Each package goes to the highest `node_modules` that
//! neither collides with a different version nor shadows a copy another package already relies on.

use std::collections::{BTreeMap, VecDeque};

use super::PlannedPackage;
use super::graph::{Graph, NodeId};

/// Package names from the root down; `[]` is the root, `["a", "b"]` is `node_modules/a/node_modules/b`.
type Chain = Vec<String>;

struct Placement<'a> {
    graph: &'a Graph,
    placed: BTreeMap<Chain, NodeId>,
}

pub(super) fn place(graph: &Graph) -> BTreeMap<String, PlannedPackage> {
    let mut placement = Placement {
        graph,
        placed: BTreeMap::new(),
    };
    let mut queue: VecDeque<(Chain, String, NodeId)> = graph
        .root
        .iter()
        .map(|(name, id)| (Vec::new(), name.clone(), id.clone()))
        .collect();
    while let Some((dependent, name, target)) = queue.pop_front() {
        let already_visible = placement
            .lookup(&dependent, &name)
            .is_some_and(|(_, id)| *id == target);
        if already_visible {
            continue;
        }
        let mut chain = placement.choose(&dependent, &name, &target);
        chain.push(name);
        placement.placed.insert(chain.clone(), target.clone());
        for (dep, id) in &graph.nodes[&target].edges {
            queue.push_back((chain.clone(), dep.clone(), id.clone()));
        }
    }
    placement.into_packages()
}

impl Placement<'_> {
    /// Node resolution: nearest `node_modules/<name>` walking up from `from`. Returns the holder dir.
    fn lookup(&self, from: &[String], name: &str) -> Option<(Chain, &NodeId)> {
        let mut dir = from.to_vec();
        loop {
            let mut slot = dir.clone();
            slot.push(name.to_owned());
            if let Some(id) = self.placed.get(&slot) {
                return Some((dir, id));
            }
            dir.pop()?;
        }
    }

    fn choose(&self, dependent: &[String], name: &str, target: &NodeId) -> Chain {
        for len in 0..dependent.len() {
            let dir = &dependent[..len];
            let mut slot = dir.to_vec();
            slot.push(name.to_owned());
            if !self.placed.contains_key(&slot) && !self.shadows(dir, name, target) {
                return dir.to_vec();
            }
        }
        dependent.to_vec()
    }

    /// True when a package under `dir` currently resolves `name` to a different copy above `dir`.
    fn shadows(&self, dir: &[String], name: &str, target: &NodeId) -> bool {
        self.placed
            .iter()
            .filter(|(chain, _)| chain.starts_with(dir))
            .filter(|(_, id)| {
                self.graph.nodes[*id]
                    .edges
                    .get(name)
                    .is_some_and(|w| w != target)
            })
            .any(|(chain, _)| {
                self.lookup(chain, name)
                    .is_some_and(|(holder, _)| holder.len() < dir.len())
            })
    }

    fn into_packages(self) -> BTreeMap<String, PlannedPackage> {
        self.placed
            .into_iter()
            .map(|(chain, id)| {
                let path = chain
                    .iter()
                    .map(|name| format!("node_modules/{name}"))
                    .collect::<Vec<_>>()
                    .join("/");
                let manifest = &self.graph.nodes[&id].manifest;
                let bin = manifest.bin.as_ref().map_or_else(Default::default, |bin| {
                    crate::linker::declared_bins(&id.0, bin)
                });
                let package = PlannedPackage {
                    name: id.0,
                    version: id.1,
                    tarball: manifest.dist.tarball.clone(),
                    integrity: manifest.dist.integrity.clone(),
                    has_install_script: manifest.has_install_script,
                    dependencies: manifest.dependencies.clone(),
                    optional_dependencies: manifest.optional_dependencies.clone(),
                    peer_dependencies: super::graph::required_peers(
                        &manifest.peer_dependencies,
                        &manifest.peer_dependencies_meta,
                    ),
                    os: manifest.os.clone(),
                    cpu: manifest.cpu.clone(),
                    bin,
                };
                (path, package)
            })
            .collect()
    }
}
