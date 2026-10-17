//! Reads a module graph ahead of its link (VELYR-192): every module the import scan finds, many
//! at a time, so the engine links the graph once instead of stopping at each module not read yet.

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use tokio::task::JoinSet;
use velyr_runtime_modules::{ModuleId, ModuleSource, ResolvedModule};

use super::{Modules, scan};

/// Module reads (and transforms) in flight while a graph is read ahead of its link.
const PREFETCH_CONCURRENCY: usize = 16;

impl Modules {
    /// Reads every module the import scan finds below `root` (already read), so the link does
    /// not stop at each module not read yet. Failures are left for the link to report.
    pub(super) async fn prefetch(&mut self, root: &ResolvedModule, source: &ModuleSource) {
        let mut seen = HashSet::from([root.id.clone()]);
        let mut queue = VecDeque::new();
        self.enqueue_imports(&root.id, &source.text, &mut seen, &mut queue);
        let mut reads = JoinSet::new();
        loop {
            while reads.len() < PREFETCH_CONCURRENCY {
                let Some(module) = queue.pop_front() else {
                    break;
                };
                let (host, load) = (Arc::clone(&self.host), self.load.clone());
                reads.spawn(async move {
                    let source = host.load(&module, &load).await;
                    (module, source)
                });
            }
            let Some(done) = reads.join_next().await else {
                return;
            };
            let Ok((module, Ok(source))) = done else {
                continue;
            };
            self.loaded
                .lock()
                .unwrap()
                .insert(module.id.clone(), Arc::clone(&source));
            self.enqueue_imports(&module.id, &source.text, &mut seen, &mut queue);
        }
    }

    fn enqueue_imports(
        &self,
        referrer: &ModuleId,
        text: &str,
        seen: &mut HashSet<ModuleId>,
        queue: &mut VecDeque<ResolvedModule>,
    ) {
        for specifier in scan::static_imports(text) {
            let Ok(module) = self.resolve(specifier, referrer) else {
                continue;
            };
            let read = self.loaded.lock().unwrap().contains_key(&module.id);
            if !read && seen.insert(module.id.clone()) {
                queue.push_back(module);
            }
        }
    }
}
