//! Two bytes shared between the loop and the `nextTick` JS side (RT-09).

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Index 0: a tick is queued. Index 1: JS armed the flags, so the loop may trust index 0.
#[derive(Clone, Debug, Default)]
pub struct TickFlags(Arc<[AtomicU8; 2]>);

impl TickFlags {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called by the JS side once it keeps index 0 up to date.
    pub fn arm(&self) {
        self.0[1].store(1, Ordering::Relaxed);
    }

    pub fn set_tick(&self) {
        self.0[0].store(1, Ordering::Relaxed);
    }

    pub fn has_tick(&self) -> bool {
        self.0[0].load(Ordering::Relaxed) != 0
    }

    pub fn armed(&self) -> bool {
        self.0[1].load(Ordering::Relaxed) != 0
    }

    /// Start of the two bytes, for engines that expose them to JS as a `Uint8Array`.
    pub fn as_ptr(&self) -> *mut u8 {
        self.0.as_ptr().cast_mut().cast()
    }
}

#[cfg(test)]
mod tests {
    use super::TickFlags;

    #[test]
    fn tick_flags_default_is_unarmed() {
        let flags = TickFlags::new();
        assert!(!flags.armed() && !flags.has_tick());
        flags.arm();
        flags.set_tick();
        assert!(flags.armed() && flags.has_tick());
    }
}
