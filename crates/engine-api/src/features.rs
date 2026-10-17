use std::ops::{BitAnd, BitOr, BitOrAssign};

/// Capabilities an engine adapter declares through [`Engine::features`](crate::Engine::features).
///
/// The runtime consults this before relying on optional behavior.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct EngineFeatures(u32);

impl EngineFeatures {
    /// ES module compile/instantiate/evaluate.
    pub const MODULES: Self = Self(1 << 0);
    /// Promises and the microtask queue.
    pub const PROMISES: Self = Self(1 << 1);
    /// `BigInt` values.
    pub const BIGINT: Self = Self(1 << 2);
    /// Structured serialization of values.
    pub const SERIALIZE: Self = Self(1 << 3);
    /// `ArrayBuffer` creation and access.
    pub const ARRAY_BUFFER: Self = Self(1 << 4);
    /// [`Isolate::set_continuation_context`](crate::Isolate::set_continuation_context): a value
    /// that follows promise reactions and `await` (what `AsyncLocalStorage` is built on).
    pub const CONTINUATION_CONTEXT: Self = Self(1 << 5);
    /// `using` / `await using` declarations. Without it the TypeScript transform lowers them
    /// through the `usingCtx` helper.
    pub const EXPLICIT_RESOURCE_MANAGEMENT: Self = Self(1 << 6);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// True when every flag in `other` is set in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for EngineFeatures {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitAnd for EngineFeatures {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl BitOrAssign for EngineFeatures {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[cfg(test)]
mod tests {
    use super::EngineFeatures as F;

    #[test]
    fn explicit_resource_management_bit_is_distinct() {
        let others = [
            F::MODULES,
            F::PROMISES,
            F::BIGINT,
            F::SERIALIZE,
            F::ARRAY_BUFFER,
            F::CONTINUATION_CONTEXT,
        ];
        for other in others {
            assert!(!F::EXPLICIT_RESOURCE_MANAGEMENT.contains(other));
        }
        assert_eq!(F::EXPLICIT_RESOURCE_MANAGEMENT.bits().count_ones(), 1);
    }
}
