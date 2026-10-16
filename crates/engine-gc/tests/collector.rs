use velyr_engine_gc::{
    FinalizationRegistry, Gc, GcRef, HandleScope, Heap, HeapConfig, Trace, Tracer, WeakMap, WeakRef,
};
use velyr_runtime_errors::ErrorCode;

#[derive(Default)]
struct Node {
    edges: Vec<GcRef>,
}

impl Trace for Node {
    fn trace(&self, tracer: &mut Tracer) {
        self.edges.trace(tracer);
    }
}

const NO_ROOTS: &[GcRef] = &[];

fn heap() -> Heap {
    Heap::new(HeapConfig::default())
}

fn node(heap: &mut Heap, edges: Vec<GcRef>) -> Gc<Node> {
    heap.alloc(Node { edges }, &NO_ROOTS).unwrap()
}

#[test]
fn unreachable_cycle_is_collected() {
    let mut heap = heap();
    let a = node(&mut heap, vec![]);
    let b = node(&mut heap, vec![a.raw()]);
    heap.get_mut(a).unwrap().edges.push(b.raw());

    heap.collect(&NO_ROOTS);

    assert!(!heap.is_alive(a.raw()));
    assert!(!heap.is_alive(b.raw()));
    assert_eq!(heap.stats().live_cells, 0);
}

#[test]
fn objects_reachable_from_vm_roots_survive_with_stable_handles() {
    let mut heap = heap();
    let leaf = node(&mut heap, vec![]);
    let root = node(&mut heap, vec![leaf.raw()]);
    let garbage = node(&mut heap, vec![]);

    heap.collect(&vec![root.raw()]);

    assert!(heap.get(root).is_some());
    assert_eq!(heap.get(root).unwrap().edges, vec![leaf.raw()]);
    assert!(heap.is_alive(leaf.raw()));
    assert!(!heap.is_alive(garbage.raw()));
}

#[test]
fn freed_slot_reuse_does_not_resurrect_stale_handle() {
    let mut heap = heap();
    let old = node(&mut heap, vec![]);
    heap.collect(&NO_ROOTS);
    let new = node(&mut heap, vec![]);

    assert_eq!(old.raw().index(), new.raw().index());
    assert!(heap.get(old).is_none());
    assert!(heap.get(new).is_some());
}

#[test]
fn handle_scope_roots_locals_until_dropped() {
    let mut heap = heap();
    let kept = {
        let mut scope = HandleScope::new(&mut heap);
        let local = node(&mut scope, vec![]);
        scope.root_local(local.raw());
        scope.collect(&NO_ROOTS);
        assert!(scope.is_alive(local.raw()));
        local
    };

    heap.collect(&NO_ROOTS);

    assert!(!heap.is_alive(kept.raw()));
}

#[test]
fn global_handle_roots_until_released() {
    let mut heap = heap();
    let target = node(&mut heap, vec![]);
    let global = heap.make_global(target);

    heap.collect(&NO_ROOTS);
    assert_eq!(heap.global_get(&global), target);
    assert!(heap.is_alive(target.raw()));

    heap.release_global(global);
    heap.collect(&NO_ROOTS);
    assert!(!heap.is_alive(target.raw()));
}

#[test]
fn weak_ref_is_cleared_after_target_is_collected() {
    let mut heap = heap();
    let target = node(&mut heap, vec![]);
    let weak = heap.alloc(WeakRef::new(target.raw()), &NO_ROOTS).unwrap();

    heap.collect(&vec![weak.raw(), target.raw()]);
    assert_eq!(heap.get(weak).unwrap().deref(), Some(target.raw()));

    heap.collect(&vec![weak.raw()]);
    assert_eq!(heap.get(weak).unwrap().deref(), None);
    assert!(!heap.is_alive(target.raw()));
}

#[test]
fn ephemeron_value_lives_only_while_key_is_reachable() {
    let mut heap = heap();
    let map = heap.alloc(WeakMap::<GcRef>::new(), &NO_ROOTS).unwrap();
    let key = node(&mut heap, vec![]);
    let value = node(&mut heap, vec![]);
    heap.get_mut(map).unwrap().insert(key.raw(), value.raw());

    heap.collect(&vec![map.raw(), key.raw()]);
    assert!(heap.is_alive(value.raw()));
    assert_eq!(heap.get(map).unwrap().get(key.raw()), Some(&value.raw()));

    heap.collect(&vec![map.raw()]);
    assert!(!heap.is_alive(key.raw()));
    assert!(!heap.is_alive(value.raw()));
    assert_eq!(heap.get(map).unwrap().len(), 0);
}

#[test]
fn ephemeron_value_referencing_its_own_key_does_not_keep_it_alive() {
    let mut heap = heap();
    let map = heap.alloc(WeakMap::<GcRef>::new(), &NO_ROOTS).unwrap();
    let key = node(&mut heap, vec![]);
    let value = node(&mut heap, vec![key.raw()]);
    heap.get_mut(map).unwrap().insert(key.raw(), value.raw());

    heap.collect(&vec![map.raw()]);

    assert!(!heap.is_alive(key.raw()));
    assert!(!heap.is_alive(value.raw()));
}

#[test]
fn chained_ephemerons_reach_fixpoint() {
    let mut heap = heap();
    let first = heap.alloc(WeakMap::<GcRef>::new(), &NO_ROOTS).unwrap();
    let second = heap.alloc(WeakMap::<GcRef>::new(), &NO_ROOTS).unwrap();
    let k1 = node(&mut heap, vec![]);
    let k2 = node(&mut heap, vec![]);
    let v2 = node(&mut heap, vec![]);
    heap.get_mut(second).unwrap().insert(k2.raw(), v2.raw());
    heap.get_mut(first).unwrap().insert(k1.raw(), k2.raw());

    // The worklist is LIFO: `second` is traced before `first` marks k2, so v2 is only
    // reached by the fixpoint pass.
    heap.collect(&vec![k1.raw(), first.raw(), second.raw()]);

    assert!(heap.is_alive(k2.raw()));
    assert!(heap.is_alive(v2.raw()));
}

#[test]
fn finalization_registry_queues_cleanup_for_dead_targets() {
    let mut heap = heap();
    let registry = heap
        .alloc(FinalizationRegistry::<GcRef>::new(), &NO_ROOTS)
        .unwrap();
    let target = node(&mut heap, vec![]);
    let held = node(&mut heap, vec![]);
    heap.get_mut(registry)
        .unwrap()
        .register(target.raw(), held.raw());

    heap.collect(&vec![registry.raw(), target.raw()]);
    assert!(heap.take_finalization_queue().is_empty());

    heap.collect(&vec![registry.raw()]);
    assert_eq!(heap.take_finalization_queue(), vec![registry.raw()]);
    assert!(heap.is_alive(held.raw()));
    assert_eq!(
        heap.get_mut(registry).unwrap().take_cleanups(),
        vec![held.raw()]
    );
}

#[test]
fn finalization_registry_retain_drops_registrations_and_pending_cleanups() {
    let mut heap = heap();
    let registry = heap
        .alloc(FinalizationRegistry::<GcRef>::new(), &NO_ROOTS)
        .unwrap();
    let (one, two) = (node(&mut heap, vec![]).raw(), node(&mut heap, vec![]).raw());
    let dead = node(&mut heap, vec![]);
    let live = node(&mut heap, vec![]);
    let cell = heap.get_mut(registry).unwrap();
    cell.register(dead.raw(), one);
    cell.register(dead.raw(), two);
    cell.register(live.raw(), one);
    heap.collect(&vec![registry.raw(), live.raw()]);

    heap.get_mut(registry).unwrap().retain(|held| *held != one);
    assert_eq!(heap.get_mut(registry).unwrap().take_cleanups(), vec![two]);
    heap.collect(&vec![registry.raw()]);
    assert!(heap.get_mut(registry).unwrap().take_cleanups().is_empty());
}

#[test]
fn pending_finalization_keeps_registry_alive_until_drained() {
    let mut heap = heap();
    let registry = heap
        .alloc(FinalizationRegistry::<GcRef>::new(), &NO_ROOTS)
        .unwrap();
    let held = node(&mut heap, vec![]);
    let target = node(&mut heap, vec![]);
    heap.get_mut(registry)
        .unwrap()
        .register(target.raw(), held.raw());

    heap.collect(&vec![registry.raw()]);
    heap.collect(&NO_ROOTS);

    assert!(heap.is_alive(registry.raw()));
    assert!(heap.is_alive(held.raw()));
    assert_eq!(heap.take_finalization_queue(), vec![registry.raw()]);
}

#[test]
fn allocation_triggers_collection_after_threshold_bytes() {
    let mut heap = Heap::new(HeapConfig {
        gc_threshold_bytes: 256,
        hard_limit_bytes: None,
    });
    for _ in 0..100 {
        node(&mut heap, vec![]);
    }

    let stats = heap.stats();
    assert!(stats.collections > 0);
    assert!(stats.live_bytes <= 512);
}

#[test]
fn allocation_reaches_hard_limit_with_quota_exceeded() {
    let mut heap = Heap::new(HeapConfig {
        gc_threshold_bytes: 1 << 20,
        hard_limit_bytes: Some(1024),
    });
    let mut live = Vec::new();
    let error = loop {
        match heap.alloc(Node::default(), &live) {
            Ok(cell) => live.push(cell.raw()),
            Err(error) => break error,
        }
    };

    assert_eq!(error.code(), ErrorCode::QuotaExceeded);
    assert!(heap.stats().live_bytes <= 1024);
    assert!(heap.stats().collections > 0);
}

#[test]
fn value_being_allocated_keeps_its_children_alive_during_triggered_collection() {
    let mut heap = Heap::new(HeapConfig {
        gc_threshold_bytes: 1,
        hard_limit_bytes: None,
    });
    let child = node(&mut heap, vec![]);
    let parent = node(&mut heap, vec![child.raw()]);

    assert!(heap.is_alive(child.raw()));
    assert!(heap.get(parent).is_some());
}

#[test]
fn stats_report_collections_pause_and_live_bytes() {
    let mut heap = heap();
    let kept = node(&mut heap, vec![]);
    node(&mut heap, vec![]);

    heap.collect(&vec![kept.raw()]);
    heap.collect(&vec![kept.raw()]);

    let stats = heap.stats();
    assert_eq!(stats.collections, 2);
    assert_eq!(stats.live_cells, 1);
    assert!(stats.live_bytes > 0);
    assert!(stats.total_pause >= stats.last_pause);
}
