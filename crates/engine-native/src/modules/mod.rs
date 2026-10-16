//! ES modules (VELYR-110): the module graph of an isolate, keyed by `Source::name` like
//! engine-v8, and the `import()` contract (`op_module_import` host op, then
//! `Isolate::resolve(resolver, module name)`). Linking and evaluation run in the VM.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use velyr_engine_api::{Handle, HostError, JsException, ModuleResolveHook, Source, Value};
use velyr_engine_vm::Value as Val;
use velyr_engine_vm::interpreter::{Abrupt, ErrorKind, ModuleId, NativeFn};

use crate::bytecode_compiler::compile_module;
use crate::isolate::NativeIsolate;

/// Host op that receives `import()` requests: `(specifier, referrer, resolver id)`.
pub(crate) const DYNAMIC_IMPORT_OP: &str = "op_module_import";

struct Entry {
    requests: Vec<String>,
    /// Set once every request resolved.
    deps: Option<Vec<ModuleId>>,
}

#[derive(Default)]
pub(crate) struct ModuleGraph {
    by_name: HashMap<String, ModuleId>,
    entries: HashMap<usize, Entry>,
    /// `(referrer, specifier)` -> module name, so retries do not ask the hook again.
    edges: HashMap<(String, String), String>,
    /// Resolvers of pending `import()` promises.
    dynamic_imports: Rc<RefCell<HashSet<u64>>>,
}

fn internal(message: impl Into<String>) -> JsException {
    JsException {
        message: message.into(),
        stack: None,
        value: None,
    }
}

impl NativeIsolate {
    pub(crate) fn compile_module_source(&mut self, source: &Source) -> Result<Handle, JsException> {
        let id = self.ensure_compiled(source)?;
        self.vm
            .module_handle(id)
            .ok_or_else(|| internal("cannot root the module"))
    }

    fn ensure_compiled(&mut self, source: &Source) -> Result<ModuleId, JsException> {
        if let Some(&id) = self.modules.by_name.get(&source.name) {
            return Ok(id);
        }
        let code = compile_module(&source.name, &source.text)?;
        let requests = code.requests.clone();
        let id = self
            .vm
            .create_module(code)
            .map_err(|abrupt| self.exception(abrupt))?;
        self.modules.by_name.insert(source.name.clone(), id);
        let entry = Entry {
            requests,
            deps: None,
        };
        self.modules.entries.insert(id.0, entry);
        Ok(id)
    }

    fn module_of(&self, handle: Handle) -> Result<ModuleId, JsException> {
        self.vm
            .handle_module(handle)
            .ok_or_else(|| internal("stale or invalid module handle"))
    }

    pub(crate) fn instantiate(
        &mut self,
        handle: Handle,
        hook: &mut dyn ModuleResolveHook,
    ) -> Result<(), JsException> {
        let root = self.module_of(handle)?;
        if self.vm.is_module_linked(root) {
            return Ok(());
        }
        self.preload(root, hook)?;
        self.vm
            .link_module(root)
            .map_err(|abrupt| self.exception(abrupt))
    }

    /// Resolves every transitive import through `hook`, compiling each new module.
    fn preload(
        &mut self,
        root: ModuleId,
        hook: &mut dyn ModuleResolveHook,
    ) -> Result<(), JsException> {
        let mut pending = vec![root];
        let mut seen = HashSet::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id.0) {
                continue;
            }
            let entry = &self.modules.entries[&id.0];
            if let Some(deps) = &entry.deps {
                pending.extend(deps);
                continue;
            }
            let referrer = self.name_of(id);
            let mut deps = Vec::new();
            for specifier in entry.requests.clone() {
                deps.push(self.resolve_edge(&referrer, &specifier, hook)?);
            }
            self.vm.set_module_dependencies(id, &deps);
            pending.extend(&deps);
            self.modules.entries.get_mut(&id.0).expect("compiled").deps = Some(deps);
        }
        Ok(())
    }

    fn name_of(&self, id: ModuleId) -> String {
        let mut names = self.modules.by_name.iter();
        names
            .find(|(_, module)| **module == id)
            .map(|(name, _)| name.clone())
            .unwrap_or_default()
    }

    fn resolve_edge(
        &mut self,
        referrer: &str,
        specifier: &str,
        hook: &mut dyn ModuleResolveHook,
    ) -> Result<ModuleId, JsException> {
        let key = (referrer.to_owned(), specifier.to_owned());
        if let Some(name) = self.modules.edges.get(&key) {
            return Ok(self.modules.by_name[name]);
        }
        let source = hook.resolve(specifier, referrer).map_err(|err| {
            let message =
                format!("cannot resolve module `{specifier}` imported from `{referrer}`: {err}");
            let mut exception = internal(message);
            if let HostError::Thrown(value) = err {
                exception.value = Some(value);
            }
            exception
        })?;
        // shortcut: no code cache, so `hook.code_cache` is never asked; ask it once
        // engine-native can consume caches.
        let id = self.ensure_compiled(&source)?;
        self.modules.edges.insert(key, source.name);
        Ok(id)
    }

    pub(crate) fn evaluate(&mut self, handle: Handle) -> Result<Value, JsException> {
        let id = self.module_of(handle)?;
        let result = self.vm.evaluate_module(id);
        self.complete(result)
    }

    /// The `import()` hook calling the host op `function`; it remembers the resolver so
    /// `settle_dynamic_import` can tell a module name from a plain resolution value.
    pub(crate) fn install_dynamic_import(&mut self, function: NativeFn) {
        let pending = self.modules.dynamic_imports.clone();
        self.vm
            .set_dynamic_import_hook(Rc::new(move |vm, this, args| {
                let id = match args.get(2) {
                    Some(Val::Number(id)) => *id as u64,
                    _ => u64::MAX,
                };
                pending.borrow_mut().insert(id);
                let result = function(vm, this, args);
                if result.is_err() {
                    // The VM rejects the promise itself.
                    pending.borrow_mut().remove(&id);
                }
                result
            }));
    }

    /// Resolving an `import()` resolver with `Value::String(module name)` evaluates that
    /// instantiated module and resolves the import with its namespace. Returns `false` when
    /// `resolver` is not such a resolution.
    pub(crate) fn settle_dynamic_import(
        &mut self,
        resolver: u64,
        value: &Value,
        rejects: bool,
    ) -> bool {
        if !self.modules.dynamic_imports.borrow_mut().remove(&resolver) || rejects {
            return false;
        }
        let Value::String(name) = value else {
            return false;
        };
        let name = name.to_string_lossy();
        let result = match self.modules.by_name.get(&name) {
            Some(&id) if self.vm.is_module_linked(id) => {
                self.vm.finish_dynamic_import(resolver, id)
            }
            found => {
                let message = match found {
                    Some(_) => format!("module `{name}` must be instantiated first"),
                    None => format!("unknown module `{name}`"),
                };
                match self.vm.error(ErrorKind::Error, &message) {
                    Abrupt::Throw(reason) => self.vm.settle_promise(resolver, &reason, true),
                    abrupt => Err(abrupt),
                }
            }
        };
        if let Err(abrupt) = result {
            let _ = self.exception(abrupt);
        }
        true
    }
}
