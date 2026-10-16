//! Per-isolate resource table with generational ids (ADR-010).
//!
//! A stale id fails with `VELYR_BAD_RESOURCE` and never aliases a newer resource.

use std::any::Any;
use std::fmt;
use std::sync::Arc;
use tracing::Span;
use velyr_runtime_errors::VelyrError;

mod quota;
pub use quota::{QuotaClass, QuotaConfig, QuotaContext, QuotaGuard};

const INDEX_BITS: u32 = 24;
const MAX_INDEX: u32 = (1 << INDEX_BITS) - 1;
const GENERATION_LIMIT: u32 = 1 << 29;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Identifies the isolate that owns a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IsolateId(pub u64);

/// Generational handle; JS sees it as one number (`to_js_number`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceId {
    pub index: u32,
    pub generation: u32,
}

impl ResourceId {
    pub fn to_js_number(self) -> f64 {
        ((u64::from(self.generation) << INDEX_BITS) + u64::from(self.index)) as f64
    }

    pub fn from_js_number(value: f64) -> Result<Self, VelyrError> {
        if !(0.0..=MAX_SAFE_INTEGER).contains(&value) || value.fract() != 0.0 {
            return Err(VelyrError::bad_resource());
        }
        let raw = value as u64;
        Ok(Self {
            index: (raw & u64::from(MAX_INDEX)) as u32,
            generation: (raw >> INDEX_BITS) as u32,
        })
    }
}

/// A native resource owned by the table.
pub trait Resource: 'static {
    fn kind(&self) -> &'static str;
    fn close(self: Box<Self>);
    /// Needed for the `get::<T>` downcast; implement as `self`.
    fn as_any(&self) -> &dyn Any;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceState {
    Open,
    Closing,
    Closed,
}

/// Capability string that never prints its value.
#[derive(Clone, PartialEq, Eq)]
pub struct Redacted(String);

impl Redacted {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

struct Entry {
    resource: Box<dyn Resource>,
    quota_class: QuotaClass,
    /// Releases the quota when the entry is dropped.
    _guard: QuotaGuard,
    capability: Option<Redacted>,
    span_id: Option<u64>,
    state: ResourceState,
}

struct Slot {
    generation: u32,
    entry: Option<Entry>,
}

/// One row of `leak_report`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeakEntry {
    pub id: ResourceId,
    pub owner: IsolateId,
    pub kind: &'static str,
    pub quota_class: QuotaClass,
    pub capability: Option<Redacted>,
    pub span_id: Option<u64>,
    pub state: ResourceState,
}

pub struct ResourceTable {
    owner: IsolateId,
    quota: Arc<QuotaContext>,
    slots: Vec<Slot>,
    free: Vec<u32>,
}

impl ResourceTable {
    pub fn new(owner: IsolateId, quota: Arc<QuotaContext>) -> Self {
        Self {
            owner,
            quota,
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    /// Reserves quota, then exposes the resource. On failure the resource is closed.
    pub fn insert(
        &mut self,
        resource: Box<dyn Resource>,
        quota_class: QuotaClass,
        capability: Option<String>,
    ) -> Result<ResourceId, VelyrError> {
        let _span = tracing::debug_span!("velyr.resources.insert").entered();
        let guard = match self.quota.reserve(quota_class, 1) {
            Ok(guard) => guard,
            Err(err) => {
                resource.close();
                return Err(err);
            }
        };
        let Some(index) = self.allocate_slot() else {
            drop(guard);
            resource.close();
            return Err(VelyrError::quota_exceeded("resource_table"));
        };
        let slot = &mut self.slots[index as usize];
        slot.entry = Some(Entry {
            resource,
            quota_class,
            _guard: guard,
            capability: capability.map(Redacted),
            span_id: Span::current().id().map(|id| id.into_u64()),
            state: ResourceState::Open,
        });
        Ok(ResourceId {
            index,
            generation: slot.generation,
        })
    }

    pub fn get<T: Resource>(&self, id: ResourceId) -> Result<&T, VelyrError> {
        self.open_entry(id)?
            .resource
            .as_any()
            .downcast_ref::<T>()
            .ok_or_else(VelyrError::bad_resource)
    }

    /// Removes the resource without closing it and releases its quota.
    pub fn take(&mut self, id: ResourceId) -> Result<Box<dyn Resource>, VelyrError> {
        self.open_entry(id)?;
        Ok(self.remove(id.index).resource)
    }

    pub fn close(&mut self, id: ResourceId) -> Result<(), VelyrError> {
        let _span = tracing::debug_span!("velyr.resources.close").entered();
        self.open_entry(id)?;
        self.slots[id.index as usize]
            .entry
            .as_mut()
            .expect("checked open")
            .state = ResourceState::Closing;
        self.remove(id.index).resource.close();
        Ok(())
    }

    /// Shutdown: closes every open resource.
    pub fn close_all(&mut self) {
        let _span = tracing::debug_span!("velyr.resources.close_all").entered();
        for index in 0..self.slots.len() as u32 {
            if self.slots[index as usize].entry.is_some() {
                self.remove(index).resource.close();
            }
        }
    }

    pub fn leak_report(&self) -> Vec<LeakEntry> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let entry = slot.entry.as_ref()?;
                Some(LeakEntry {
                    id: ResourceId {
                        index: index as u32,
                        generation: slot.generation,
                    },
                    owner: self.owner,
                    kind: entry.resource.kind(),
                    quota_class: entry.quota_class,
                    capability: entry.capability.clone(),
                    span_id: entry.span_id,
                    state: entry.state,
                })
            })
            .collect()
    }

    fn open_entry(&self, id: ResourceId) -> Result<&Entry, VelyrError> {
        self.slots
            .get(id.index as usize)
            .filter(|slot| slot.generation == id.generation)
            .and_then(|slot| slot.entry.as_ref())
            .filter(|entry| entry.state == ResourceState::Open)
            .ok_or_else(VelyrError::bad_resource)
    }

    fn allocate_slot(&mut self) -> Option<u32> {
        if let Some(index) = self.free.pop() {
            return Some(index);
        }
        let index = u32::try_from(self.slots.len())
            .ok()
            .filter(|i| *i <= MAX_INDEX)?;
        self.slots.push(Slot {
            generation: 0,
            entry: None,
        });
        Some(index)
    }

    /// Frees the slot, bumps its generation (retiring it at the limit); dropping the entry releases quota.
    fn remove(&mut self, index: u32) -> Entry {
        let slot = &mut self.slots[index as usize];
        let entry = slot.entry.take().expect("slot is occupied");
        slot.generation += 1;
        if slot.generation < GENERATION_LIMIT {
            self.free.push(index);
        }
        entry
    }

    #[cfg(test)]
    fn set_generation_for_test(&mut self, index: u32, generation: u32) {
        self.slots[index as usize].generation = generation;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;
    use velyr_runtime_errors::ErrorCode;

    #[derive(Debug)]
    struct Probe {
        kind: &'static str,
        closed: Rc<Cell<u32>>,
    }

    impl Resource for Probe {
        fn kind(&self) -> &'static str {
            self.kind
        }
        fn close(self: Box<Self>) {
            self.closed.set(self.closed.get() + 1);
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    #[derive(Debug)]
    struct Other;

    impl Resource for Other {
        fn kind(&self) -> &'static str {
            "other"
        }
        fn close(self: Box<Self>) {}
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn probe(closed: &Rc<Cell<u32>>) -> Box<Probe> {
        Box::new(Probe {
            kind: "probe",
            closed: closed.clone(),
        })
    }

    fn table() -> ResourceTable {
        ResourceTable::new(IsolateId(1), Arc::new(QuotaContext::default()))
    }

    #[test]
    fn stale_id_after_slot_reuse_is_bad_resource() {
        let closed = Rc::new(Cell::new(0));
        let mut t = table();
        let old = t.insert(probe(&closed), QuotaClass::Files, None).unwrap();
        t.close(old).unwrap();
        let new = t.insert(probe(&closed), QuotaClass::Files, None).unwrap();
        assert_eq!(new.index, old.index);
        assert_ne!(new.generation, old.generation);
        assert_eq!(
            t.get::<Probe>(old).unwrap_err().code(),
            ErrorCode::BadResource
        );
        assert!(t.get::<Probe>(new).is_ok());
        assert_eq!(closed.get(), 1);
    }

    #[test]
    fn close_twice_and_unknown_index_are_bad_resource() {
        let closed = Rc::new(Cell::new(0));
        let mut t = table();
        let id = t.insert(probe(&closed), QuotaClass::Files, None).unwrap();
        t.close(id).unwrap();
        assert_eq!(t.close(id).unwrap_err().code(), ErrorCode::BadResource);
        let ghost = ResourceId {
            index: 99,
            generation: 0,
        };
        assert_eq!(t.take(ghost).err().unwrap().code(), ErrorCode::BadResource);
    }

    #[test]
    fn wrong_type_is_bad_resource() {
        let mut t = table();
        let id = t.insert(Box::new(Other), QuotaClass::Files, None).unwrap();
        assert_eq!(
            t.get::<Probe>(id).unwrap_err().code(),
            ErrorCode::BadResource
        );
        assert!(t.get::<Other>(id).is_ok());
    }

    #[test]
    fn close_all_closes_each_and_empties_leak_report() {
        let closed = Rc::new(Cell::new(0));
        let mut t = table();
        let ids: Vec<_> = (0..3)
            .map(|_| t.insert(probe(&closed), QuotaClass::Files, None).unwrap())
            .collect();
        assert_eq!(t.leak_report().len(), 3);
        t.close_all();
        assert_eq!(closed.get(), 3);
        assert!(t.leak_report().is_empty());
        assert!(t.get::<Probe>(ids[0]).is_err());
    }

    #[test]
    fn leak_report_lists_open_with_metadata_and_redacts_capability() {
        let closed = Rc::new(Cell::new(0));
        let mut t = table();
        let id = t
            .insert(
                probe(&closed),
                QuotaClass::Sockets,
                Some("net:secret-host".into()),
            )
            .unwrap();
        let report = t.leak_report();
        assert_eq!(report.len(), 1);
        assert_eq!(report[0].id, id);
        assert_eq!(report[0].kind, "probe");
        assert_eq!(report[0].quota_class, QuotaClass::Sockets);
        assert_eq!(report[0].owner, IsolateId(1));
        assert_eq!(report[0].state, ResourceState::Open);
        assert!(!format!("{report:?}").contains("secret-host"));
    }

    fn table_with_files_limit(limit: u64) -> (ResourceTable, Arc<QuotaContext>) {
        let quota = Arc::new(QuotaContext::new(QuotaConfig {
            files: Some(limit),
            ..Default::default()
        }));
        (ResourceTable::new(IsolateId(1), quota.clone()), quota)
    }

    #[test]
    fn third_insert_over_files_limit_is_rejected_and_closed() {
        let (mut t, quota) = table_with_files_limit(2);
        let closed = Rc::new(Cell::new(0));
        let a = t.insert(probe(&closed), QuotaClass::Files, None).unwrap();
        let b = t.insert(probe(&closed), QuotaClass::Files, None).unwrap();
        let err = t
            .insert(probe(&closed), QuotaClass::Files, None)
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::QuotaExceeded);
        assert_eq!(closed.get(), 1, "rejected resource must be closed");
        assert_eq!(t.leak_report().len(), 2);
        t.close(a).unwrap();
        assert_eq!(quota.used(QuotaClass::Files), 1);
        let taken = t.take(b).unwrap();
        assert_eq!(quota.used(QuotaClass::Files), 0);
        assert_eq!(taken.kind(), "probe");
        assert_eq!(closed.get(), 2, "take must not close");
    }

    #[test]
    fn js_number_roundtrip_and_bounds() {
        let max = ResourceId {
            index: (1 << 24) - 1,
            generation: (1 << 29) - 1,
        };
        assert_eq!(max.to_js_number(), 9_007_199_254_740_991.0);
        assert_eq!(ResourceId::from_js_number(max.to_js_number()).unwrap(), max);
        let id = ResourceId {
            index: 5,
            generation: 3,
        };
        assert_eq!(id.to_js_number(), 3.0 * 16_777_216.0 + 5.0);
        assert_eq!(ResourceId::from_js_number(id.to_js_number()).unwrap(), id);
        for bad in [f64::NAN, f64::INFINITY, -1.0, 1.5, 9_007_199_254_740_992.0] {
            let err = ResourceId::from_js_number(bad).unwrap_err();
            assert_eq!(err.code(), ErrorCode::BadResource, "{bad}");
        }
    }

    #[test]
    fn slot_is_retired_when_generation_would_overflow() {
        let closed = Rc::new(Cell::new(0));
        let mut t = table();
        let id = t.insert(probe(&closed), QuotaClass::Files, None).unwrap();
        t.set_generation_for_test(id.index, (1 << 29) - 1);
        let last = ResourceId {
            index: id.index,
            generation: (1 << 29) - 1,
        };
        t.close(id).unwrap_err();
        t.close(last).unwrap();
        let next = t.insert(probe(&closed), QuotaClass::Files, None).unwrap();
        assert_ne!(next.index, id.index, "retired slot must never be reused");
    }
}
