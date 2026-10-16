//! Hook for bare specifiers (`pkg`, `@scope/pkg/sub`, `#internal`). This crate knows nothing about
//! npm; `velyr-compat-npm` implements the trait and the CLI registers it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::resolve::resolve_with;
use crate::{ModuleId, ModuleResolver, ResolveContext, ResolvedModule, Result};

pub trait BareSpecifierResolver: Send + Sync {
    /// `referrer` is the absolute path of the importing module. Returns the (not yet canonical)
    /// path of the target; the caller canonicalizes it, and `ModuleHost::load` checks permissions.
    fn resolve_bare(&self, specifier: &str, referrer: &Path) -> Result<PathBuf>;
}

/// [`FsResolver`](crate::FsResolver) that sends bare specifiers, left over after the import map,
/// to a [`BareSpecifierResolver`].
#[derive(Clone)]
pub struct StrategyResolver {
    bare: Arc<dyn BareSpecifierResolver>,
}

impl StrategyResolver {
    pub fn new(bare: Arc<dyn BareSpecifierResolver>) -> Self {
        Self { bare }
    }
}

impl ModuleResolver for StrategyResolver {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &ModuleId,
        ctx: &ResolveContext,
    ) -> Result<ResolvedModule> {
        resolve_with(Some(self.bare.as_ref()), specifier, referrer, ctx)
    }
}
