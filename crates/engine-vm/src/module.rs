//! ES module records (VELYR-110): Source Text Module Records compiled by the embedder,
//! linked (ResolveExport, InitializeEnvironment) and evaluated here.
//!
//! A module's top-level bindings live in its context, created with the record so that
//! importers can link to it before it runs: slot 0 is `import.meta`, an import takes two
//! slots (the exporter's context and the binding's slot in it, read by `LdaModuleImport`).
//! Linking runs each module's `init` code (`var`s and hoisted functions); `body` is the
//! module code proper, an async function when it uses top-level await.
//!
//! Lives in `interpreter` (as `interpreter::module`) for access to the frame internals.

#[path = "module/evaluate.rs"]
mod evaluate;
#[path = "module/namespace.rs"]
mod namespace;

use std::collections::HashSet;
use std::rc::Rc;

use velyr_engine_api::Handle;
use velyr_engine_gc::{Gc, Trace, Tracer};

use super::environment::Context;
use super::function::{Closure, FunctionEnv};
use super::ops::HOLE;
use super::{Abrupt, ErrorKind, Interpreter, JsResult, object_value};
use crate::bytecode::{FunctionCode, Instruction as I};
use crate::{JsStr, PropertyDescriptor, Value};

/// A module record of one interpreter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(pub usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportName {
    Named(JsStr),
    /// `import * as ns` / `export * as ns from`.
    Namespace,
}

/// ImportEntry: `request` indexes `ModuleCode::requests`; `slot` is the first of its pair.
#[derive(Debug, Clone)]
pub struct ModuleImport {
    pub request: u32,
    pub name: ImportName,
    pub slot: u32,
}

#[derive(Debug, Clone)]
pub enum ExportTarget {
    /// A binding of this module, by context slot.
    Local(u32),
    /// Re-export of an import (`export { x } from`, `export * as ns from`).
    Indirect { request: u32, name: ImportName },
}

/// A compiled module: its code and its import/export entries.
#[derive(Clone)]
pub struct ModuleCode {
    /// Module identity; `import.meta.url`.
    pub url: String,
    /// InitializeEnvironment: `var` bindings and hoisted functions.
    pub init: Rc<FunctionCode>,
    pub body: Rc<FunctionCode>,
    pub context_size: u32,
    /// Module specifiers, in source order, without duplicates.
    pub requests: Vec<String>,
    pub imports: Vec<ModuleImport>,
    pub exports: Vec<(JsStr, ExportTarget)>,
    /// `export * from`: indexes into `requests`.
    pub star_exports: Vec<u32>,
}

pub(crate) struct ModuleRecord {
    code: Rc<ModuleCode>,
    context: Value,
    /// The module of each request, set by the embedder before linking.
    deps: Option<Vec<usize>>,
    linked: bool,
    namespace: Value,
    /// Evaluation promise once evaluation started.
    evaluation: Value,
}

impl Trace for ModuleRecord {
    fn trace(&self, tracer: &mut Tracer) {
        self.context.trace(tracer);
        self.namespace.trace(tracer);
        self.evaluation.trace(tracer);
    }
}

/// ResolveExport result: a binding by (module, context slot), or a module namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    Binding { module: usize, slot: u32 },
    Namespace(usize),
    Ambiguous,
    NotFound,
}

impl Interpreter {
    /// Creates the record of `code`, with its context and `import.meta`.
    pub fn create_module(&mut self, code: ModuleCode) -> Result<ModuleId, Abrupt> {
        self.scoped(|vm| {
            let slots = vec![HOLE; code.context_size.max(1) as usize];
            let context = vm.alloc_cell(Context {
                slots,
                parent: Value::Undefined,
            })?;
            let module_url = code.url.clone();
            let url = Value::String(JsStr::from_str(&module_url));
            let id = vm.state.modules.len();
            vm.state.modules.push(ModuleRecord {
                code: Rc::new(code),
                context: Value::Object(context),
                deps: None,
                linked: false,
                namespace: Value::Undefined,
                evaluation: Value::Undefined,
            });
            // HostGetImportMetaProperties: a null-prototype object with `url`.
            let meta = vm.new_object_with(None)?;
            let key = vm.realm.key_str("url");
            vm.define_property(meta, key, &PropertyDescriptor::data(url, true, true, true))?;
            if let Some((filename, dirname)) = velyr_engine_api::file_url_paths(&module_url) {
                for (name, value) in [("dirname", dirname), ("filename", filename)] {
                    let key = vm.realm.key_str(name);
                    let value = Value::String(JsStr::from_str(&value));
                    let attrs = PropertyDescriptor::data(value, true, true, true);
                    vm.define_property(meta, key, &attrs)?;
                }
            }
            vm.module_context(id).slots[0] = object_value(meta);
            Ok(ModuleId(id))
        })
    }

    /// The modules `id`'s requests resolved to, in `ModuleCode::requests` order.
    pub fn set_module_dependencies(&mut self, id: ModuleId, deps: &[ModuleId]) {
        self.state.modules[id.0].deps = Some(deps.iter().map(|dep| dep.0).collect());
    }

    pub fn is_module_linked(&self, id: ModuleId) -> bool {
        self.state.modules[id.0].linked
    }

    /// Roots the module behind a new embedder handle.
    pub fn module_handle(&mut self, id: ModuleId) -> Option<Handle> {
        let context = self.state.modules[id.0].context.clone();
        self.create_handle(&context)
    }

    /// The module behind a handle from `module_handle`.
    pub fn handle_module(&self, handle: Handle) -> Option<ModuleId> {
        let Value::Object(target) = self.resolve_handle(handle)? else {
            return None;
        };
        let mut records = self.state.modules.iter();
        records
            .position(|record| matches!(record.context, Value::Object(r) if r == target))
            .map(ModuleId)
    }

    /// Link(): resolves the imports of `id` and of every unlinked module it reaches, then
    /// initializes their environments. On failure no module of the graph is linked.
    pub fn link_module(&mut self, id: ModuleId) -> Result<(), Abrupt> {
        let mut order = Vec::new();
        self.collect_unlinked(id.0, &mut HashSet::new(), &mut order)?;
        self.scoped(|vm| {
            for &module in &order {
                vm.link_imports(module)?;
            }
            for &module in &order {
                let init = vm.state.modules[module].code.init.clone();
                vm.run_module_code(module, &init)?;
            }
            for &module in &order {
                vm.state.modules[module].linked = true;
            }
            Ok(())
        })
    }

    /// The namespace object of a linked module.
    pub fn module_namespace(&mut self, id: ModuleId) -> Result<Value, Abrupt> {
        if !self.state.modules[id.0].linked {
            return Err(self.internal("module must be instantiated before reading its namespace"));
        }
        self.scoped(|vm| vm.namespace_of(id.0))
    }

    /// Installs the function `import()` calls with `(specifier, referrer, resolver id)`;
    /// the embedder settles the resolver (see `finish_dynamic_import`).
    pub fn set_dynamic_import_hook(&mut self, hook: super::NativeFn) {
        self.import_hook = Some(hook);
    }

    pub(crate) fn step_module(&mut self, instruction: &I) -> JsResult<()> {
        match instruction {
            I::LdaModuleImport { depth, slot } => {
                let (context, index) = {
                    let pair = &self.context_at(*depth).slots[*slot as usize..];
                    (pair[0].clone(), pair[1].clone())
                };
                let (Value::Object(context), Value::Number(index)) = (context, index) else {
                    return Err(self.internal("import binding was not linked"));
                };
                let context: Gc<Context> = Gc::from_raw(context);
                let cell = self
                    .realm
                    .heap
                    .get(context)
                    .expect("module context is live");
                self.state.acc = cell.slots[index as usize].clone();
                Ok(())
            }
            I::DynamicImport { options } => {
                let options = options.map_or(Value::Undefined, |reg| self.reg(reg).clone());
                self.dynamic_import(&options)
            }
            other => Err(self.internal(&format!("unexpected instruction {other:?}"))),
        }
    }

    fn module_context(&mut self, module: usize) -> &mut Context {
        let Value::Object(context) = self.state.modules[module].context else {
            unreachable!("a module context is an object");
        };
        let context: Gc<Context> = Gc::from_raw(context);
        self.realm
            .heap
            .get_mut(context)
            .expect("module context is live")
    }

    fn dep(&self, module: usize, request: u32) -> usize {
        let deps = self.state.modules[module].deps.as_ref();
        deps.expect("dependencies are set before linking")[request as usize]
    }

    fn collect_unlinked(
        &mut self,
        module: usize,
        seen: &mut HashSet<usize>,
        order: &mut Vec<usize>,
    ) -> JsResult<()> {
        if self.state.modules[module].linked || !seen.insert(module) {
            return Ok(());
        }
        let Some(deps) = self.state.modules[module].deps.clone() else {
            return Err(self.internal("module dependencies were not resolved"));
        };
        for dep in deps {
            self.collect_unlinked(dep, seen, order)?;
        }
        order.push(module);
        Ok(())
    }

    /// InitializeEnvironment, import part: binds every import, checks indirect exports.
    fn link_imports(&mut self, module: usize) -> JsResult<()> {
        let code = self.state.modules[module].code.clone();
        for import in &code.imports {
            let dep = self.dep(module, import.request);
            let resolution = match &import.name {
                ImportName::Namespace => Resolution::Namespace(dep),
                ImportName::Named(name) => self.resolve_export(dep, name, &mut Vec::new()),
            };
            let (context, slot) = match resolution {
                Resolution::Binding { module, slot } => {
                    (self.state.modules[module].context.clone(), slot)
                }
                Resolution::Namespace(target) => (self.namespace_holder(target)?, 0),
                failure => {
                    return Err(self.unresolved(&code, import.request, &import.name, failure));
                }
            };
            let pair = &mut self.module_context(module).slots[import.slot as usize..];
            pair[0] = context;
            pair[1] = Value::Number(f64::from(slot));
        }
        for (_, target) in &code.exports {
            if let ExportTarget::Indirect {
                request,
                name: name @ ImportName::Named(import_name),
            } = target
            {
                let dep = self.dep(module, *request);
                let resolution = self.resolve_export(dep, import_name, &mut Vec::new());
                if matches!(resolution, Resolution::Ambiguous | Resolution::NotFound) {
                    return Err(self.unresolved(&code, *request, name, resolution));
                }
            }
        }
        Ok(())
    }

    /// A one-slot context holding the namespace of `module`, the target of an import pair.
    fn namespace_holder(&mut self, module: usize) -> JsResult<Value> {
        let namespace = self.namespace_of(module)?;
        let holder = self.alloc_cell(Context {
            slots: vec![namespace],
            parent: Value::Undefined,
        })?;
        let holder = Value::Object(holder);
        self.keep(&holder);
        Ok(holder)
    }

    fn unresolved(
        &mut self,
        code: &ModuleCode,
        request: u32,
        name: &ImportName,
        failure: Resolution,
    ) -> Abrupt {
        let specifier = &code.requests[request as usize];
        let name = match name {
            ImportName::Named(name) => name.to_string_lossy(),
            ImportName::Namespace => "*".into(),
        };
        let message = match failure {
            Resolution::Ambiguous => format!(
                "The requested module '{specifier}' contains conflicting star exports for name '{name}'"
            ),
            _ => format!(
                "The requested module '{specifier}' does not provide an export named '{name}'"
            ),
        };
        self.error(ErrorKind::Syntax, &message)
    }

    /// ResolveExport(module, name, resolveSet).
    fn resolve_export(
        &self,
        module: usize,
        name: &JsStr,
        resolve_set: &mut Vec<(usize, JsStr)>,
    ) -> Resolution {
        if resolve_set.iter().any(|(m, n)| *m == module && n == name) {
            return Resolution::NotFound;
        }
        resolve_set.push((module, name.clone()));
        let code = &self.state.modules[module].code;
        if let Some((_, target)) = code.exports.iter().find(|(export, _)| export == name) {
            return match target {
                ExportTarget::Local(slot) => Resolution::Binding {
                    module,
                    slot: *slot,
                },
                ExportTarget::Indirect { request, name } => {
                    let dep = self.dep(module, *request);
                    match name {
                        ImportName::Namespace => Resolution::Namespace(dep),
                        ImportName::Named(name) => self.resolve_export(dep, name, resolve_set),
                    }
                }
            };
        }
        if name.to_string_lossy() == "default" {
            return Resolution::NotFound;
        }
        let mut star = Resolution::NotFound;
        for &request in &code.star_exports {
            let dep = self.dep(module, request);
            match self.resolve_export(dep, name, resolve_set) {
                Resolution::Ambiguous => return Resolution::Ambiguous,
                Resolution::NotFound => {}
                found if star == Resolution::NotFound => star = found,
                found if found != star => return Resolution::Ambiguous,
                _ => {}
            }
        }
        star
    }

    /// GetExportedNames(module, exportStarSet).
    fn exported_names(&self, module: usize, visited: &mut Vec<usize>) -> Vec<JsStr> {
        if visited.contains(&module) {
            return Vec::new();
        }
        visited.push(module);
        let code = &self.state.modules[module].code;
        let mut names: Vec<JsStr> = code.exports.iter().map(|(name, _)| name.clone()).collect();
        for &request in &code.star_exports {
            for name in self.exported_names(self.dep(module, request), visited) {
                if name.to_string_lossy() != "default" && !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        names
    }

    /// Runs module-level code with the module context and `this` = undefined.
    fn run_module_code(&mut self, module: usize, code: &Rc<FunctionCode>) -> JsResult<Value> {
        let closure = Closure {
            info: self.code_info(code),
            context: self.state.modules[module].context.clone(),
            env: FunctionEnv::new(Value::Undefined),
            home: Value::Undefined,
            elements: None,
            _charge: Closure::charge(),
        };
        let undefined = Value::Undefined;
        let env = self.frame_env(&closure, &undefined, &undefined, Value::Undefined);
        self.run_closure(&closure, &undefined, env, &[], None)
    }
}
