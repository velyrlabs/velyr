//! ES modules: compile, instantiate (graph preloaded through the hook) and evaluate.
//!
//! V8's resolve callback captures nothing, so `instantiate` resolves the whole graph through
//! the hook first and records `(referrer, specifier) -> module name` edges; the callback only
//! reads them back from [`Shared`].

use std::collections::{HashMap, HashSet};
use std::num::NonZeroI32;
use std::rc::Rc;

use velyr_engine_api::{
    Handle, HostError, JsException, JsString, ModuleResolveHook, Source, Value,
};
use velyr_runtime_errors::VelyrError;

use crate::code_cache;
use crate::convert::{exception_from, from_v8, internal_exception};
use crate::host::{SharedSlot, call_named, host_error_to_js};
use crate::shared::Shared;

/// Modules compiled in this isolate, keyed by `Source::name`, plus resolved import edges.
#[derive(Default)]
pub(crate) struct ModuleGraph {
    by_name: HashMap<String, v8::Global<v8::Module>>,
    /// Names per `Module::get_identity_hash`; hashes can collide, so lookups confirm identity.
    by_hash: HashMap<NonZeroI32, Vec<String>>,
    edges: HashMap<(String, String), String>,
    /// `source_digest` of each module's text, the key its code cache is sealed with. Filled on
    /// demand: only a consumed or produced code cache needs it.
    digests: HashMap<String, u64>,
    /// Source string of each module, kept to compute its digest when a cache is produced.
    texts: HashMap<String, v8::Global<v8::String>>,
}

impl ModuleGraph {
    pub(crate) fn module(&self, name: &str) -> Option<&v8::Global<v8::Module>> {
        self.by_name.get(name)
    }

    pub(crate) fn digest(&self, name: &str) -> Option<u64> {
        self.digests.get(name).copied()
    }

    pub(crate) fn text(&self, name: &str) -> Option<&v8::Global<v8::String>> {
        self.texts.get(name)
    }

    pub(crate) fn remember_digest(&mut self, name: &str, digest: u64) {
        self.digests.insert(name.to_owned(), digest);
    }
}

/// Installs the isolate-wide `import.meta` and dynamic `import()` hooks.
pub(crate) fn install_callbacks(isolate: &mut v8::Isolate) {
    isolate.set_host_initialize_import_meta_object_callback(init_import_meta);
    isolate.set_host_import_module_dynamically_callback(dynamic_import);
}

/// Compiles `source` unless a module with the same name exists. The returned handle
/// carries the module name; the module itself lives in [`ModuleGraph`].
pub(crate) fn compile(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    source: &Source,
    cache: Option<&[u8]>,
) -> Result<Handle, JsException> {
    ensure_compiled(scope, shared, source, cache)?;
    let name = v8::String::new(scope, &source.name)
        .ok_or_else(|| internal_exception("module name too long"))?;
    let global = v8::Global::new(scope, v8::Local::<v8::Value>::from(name));
    Ok(shared.handles.borrow_mut().alloc(global))
}

pub(crate) fn instantiate(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    handle: Handle,
    hook: &mut dyn ModuleResolveHook,
) -> Result<(), JsException> {
    let name = module_name(scope, shared, handle)?;
    preload(scope, shared, &name, hook)?;
    let module = module_by_name(scope, shared, &name)?;
    v8::tc_scope!(let tc, scope);
    match module.instantiate_module(tc, resolve_callback) {
        Some(true) => Ok(()),
        _ => Err(exception_from(tc, shared)),
    }
}

pub(crate) fn evaluate(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    handle: Handle,
) -> Result<Value, JsException> {
    let name = module_name(scope, shared, handle)?;
    let module = module_by_name(scope, shared, &name)?;
    // V8 aborts the process when evaluating a module that was not linked.
    if matches!(
        module.get_status(),
        v8::ModuleStatus::Uninstantiated | v8::ModuleStatus::Instantiating
    ) {
        return Err(internal_exception(format!(
            "module `{name}` must be instantiated before evaluation"
        )));
    }
    v8::tc_scope!(let tc, scope);
    shared.microtasks_dirty.set(true);
    match module.evaluate(tc) {
        Some(promise) => Ok(from_v8(tc, &mut shared.handles.borrow_mut(), promise)),
        None => Err(exception_from(tc, shared)),
    }
}

/// Namespace object of an instantiated (or evaluated) module.
pub(crate) fn namespace(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    handle: Handle,
) -> Result<Value, JsException> {
    let name = module_name(scope, shared, handle)?;
    let module = module_by_name(scope, shared, &name)?;
    if matches!(
        module.get_status(),
        v8::ModuleStatus::Uninstantiated | v8::ModuleStatus::Instantiating
    ) {
        return Err(internal_exception(format!(
            "module `{name}` must be instantiated before reading its namespace"
        )));
    }
    let namespace = module.get_module_namespace();
    Ok(from_v8(scope, &mut shared.handles.borrow_mut(), namespace))
}

fn ensure_compiled(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    source: &Source,
    cache: Option<&[u8]>,
) -> Result<(), JsException> {
    if shared.modules.borrow().by_name.contains_key(&source.name) {
        return Ok(());
    }
    let digest = cache.map(|_| code_cache::digest(&source.text));
    let payload = cache.zip(digest).map(|(blob, digest)| code_cache::unseal(digest, blob));
    v8::tc_scope!(let tc, scope);
    let (Some(name), Some(text)) = (
        v8::String::new(tc, &source.name),
        v8::String::new(tc, &source.text),
    ) else {
        return Err(internal_exception("source too long"));
    };
    let origin = v8::ScriptOrigin::new(
        tc,
        name.into(),
        0,
        0,
        false,
        0,
        None,
        false,
        false,
        true,
        None,
    );
    // Bytes that fail the seal never reach V8; V8 may still reject what passes it.
    let mut v8_source = match payload.flatten() {
        Some(bytes) => v8::script_compiler::Source::new_with_cached_data(
            text,
            Some(&origin),
            v8::script_compiler::CachedData::new(bytes),
        ),
        None => v8::script_compiler::Source::new(text, Some(&origin)),
    };
    let options = if v8_source.get_cached_data().is_some() {
        v8::script_compiler::CompileOptions::ConsumeCodeCache
    } else {
        v8::script_compiler::CompileOptions::NoCompileOptions
    };
    let compiled = v8::script_compiler::compile_module2(
        tc,
        &mut v8_source,
        options,
        v8::script_compiler::NoCacheReason::NoReason,
    );
    if cache.is_some() {
        let accepted = v8_source
            .get_cached_data()
            .is_some_and(|data| !data.rejected());
        code_cache::record(shared, accepted);
    }
    let Some(module) = compiled else {
        let mut exception = exception_from(tc, shared);
        // Syntax errors carry no stack frames; point at the offending position instead.
        if let Some(position) = syntax_error_position(tc) {
            exception.stack = Some(format!("{}\n    at {position}", exception.message));
        }
        return Err(exception);
    };
    let global = v8::Global::new(tc, module);
    let text = v8::Global::new(tc, text);
    let mut graph = shared.modules.borrow_mut();
    graph
        .by_hash
        .entry(module.get_identity_hash())
        .or_default()
        .push(source.name.clone());
    graph.by_name.insert(source.name.clone(), global);
    graph.texts.insert(source.name.clone(), text);
    if let Some(digest) = digest {
        graph.remember_digest(&source.name, digest);
    }
    Ok(())
}

/// `name:line:column` (1-based) of the error caught by `tc`.
fn syntax_error_position(
    tc: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
) -> Option<String> {
    let message = tc.message()?;
    let name = message
        .get_script_resource_name(tc)
        .map(|v| v.to_rust_string_lossy(tc))?;
    let line = message.get_line_number(tc)?;
    Some(format!("{name}:{line}:{}", message.get_start_column() + 1))
}

/// Resolves every transitive import through `hook`, compiling each new module.
fn preload(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    root: &str,
    hook: &mut dyn ModuleResolveHook,
) -> Result<(), JsException> {
    let mut pending = vec![root.to_owned()];
    let mut seen = HashSet::new();
    while let Some(name) = pending.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        for specifier in requested_specifiers(scope, shared, &name)? {
            pending.push(resolve_edge(scope, shared, &name, &specifier, hook)?);
        }
    }
    Ok(())
}

fn requested_specifiers(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    name: &str,
) -> Result<Vec<String>, JsException> {
    let requests = module_by_name(scope, shared, name)?.get_module_requests();
    Ok((0..requests.length())
        .filter_map(|i| requests.get(scope, i))
        .filter_map(|data| v8::Local::<v8::ModuleRequest>::try_from(data).ok())
        .map(|request| request.get_specifier().to_rust_string_lossy(scope))
        .collect())
}

fn resolve_edge(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    referrer: &str,
    specifier: &str,
    hook: &mut dyn ModuleResolveHook,
) -> Result<String, JsException> {
    let key = (referrer.to_owned(), specifier.to_owned());
    if let Some(name) = shared.modules.borrow().edges.get(&key) {
        return Ok(name.clone());
    }
    let source = hook
        .resolve(specifier, referrer)
        .map_err(|err| resolve_exception(specifier, referrer, err))?;
    let cache = hook.code_cache(&source.name);
    ensure_compiled(scope, shared, &source, cache.as_deref())?;
    shared
        .modules
        .borrow_mut()
        .edges
        .insert(key, source.name.clone());
    Ok(source.name)
}

fn resolve_exception(specifier: &str, referrer: &str, err: HostError) -> JsException {
    let mut exception = internal_exception(format!(
        "cannot resolve module `{specifier}` imported from `{referrer}`: {err}"
    ));
    if let HostError::Thrown(value) = err {
        exception.value = Some(value);
    }
    exception
}

fn module_name(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    handle: Handle,
) -> Result<String, JsException> {
    let handles = shared.handles.borrow();
    let global = handles
        .get(handle)
        .ok_or_else(|| internal_exception("stale or invalid module handle"))?;
    let value = v8::Local::new(scope, global);
    let name = v8::Local::<v8::String>::try_from(value)
        .map_err(|_| internal_exception("not a module handle"))?;
    Ok(name.to_rust_string_lossy(scope))
}

fn module_by_name<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    shared: &Shared,
    name: &str,
) -> Result<v8::Local<'s, v8::Module>, JsException> {
    let graph = shared.modules.borrow();
    let global = graph
        .by_name
        .get(name)
        .ok_or_else(|| internal_exception(format!("unknown module `{name}`")))?;
    Ok(v8::Local::new(scope, global))
}

fn name_of_module(shared: &Shared, module: v8::Local<'_, v8::Module>) -> Option<String> {
    let graph = shared.modules.borrow();
    graph
        .by_hash
        .get(&module.get_identity_hash())?
        .iter()
        .find(|name| graph.by_name[*name] == module)
        .cloned()
}

fn shared_of(scope: &v8::PinScope<'_, '_>) -> Option<Rc<Shared>> {
    scope
        .get_slot::<SharedSlot>()
        .map(|slot| Rc::clone(&slot.0))
}

#[allow(unsafe_code)]
fn resolve_callback<'s>(
    context: v8::Local<'s, v8::Context>,
    specifier: v8::Local<'s, v8::String>,
    _import_attributes: v8::Local<'s, v8::FixedArray>,
    referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
    // SAFETY: V8 calls this synchronously from `Module::instantiate_module` with the context
    // of the isolate that is currently entered, so the scope cannot outlive it.
    v8::callback_scope!(unsafe scope, context);
    let shared = shared_of(scope)?;
    let specifier = specifier.to_rust_string_lossy(scope);
    let resolved = name_of_module(&shared, referrer).and_then(|referrer| {
        let graph = shared.modules.borrow();
        let name = graph.edges.get(&(referrer, specifier.clone()))?;
        Some(v8::Local::new(scope, graph.by_name.get(name)?))
    });
    if resolved.is_none() {
        throw_error(scope, &format!("module `{specifier}` was not preloaded"));
    }
    resolved
}

#[allow(unsafe_code)]
extern "C" fn init_import_meta(
    context: v8::Local<v8::Context>,
    module: v8::Local<v8::Module>,
    meta: v8::Local<v8::Object>,
) {
    // SAFETY: V8 calls this synchronously while evaluating `module`, in the entered isolate.
    v8::callback_scope!(unsafe scope, context);
    v8::scope!(let scope, scope);
    let Some(shared) = shared_of(scope) else {
        return;
    };
    let Some(name) = name_of_module(&shared, module) else {
        return;
    };
    if let (Some(key), Some(url)) = (v8::String::new(scope, "url"), v8::String::new(scope, &name)) {
        // Fails only if JS threw; `import.meta.url` is then simply absent.
        let _ = meta.create_data_property(scope, key.into(), url.into());
    }
    if let Some((filename, dirname)) = velyr_engine_api::file_url_paths(&name) {
        for (key, value) in [("dirname", dirname), ("filename", filename)] {
            let key = v8::String::new(scope, key);
            let value = v8::String::new(scope, &value);
            if let (Some(key), Some(value)) = (key, value) {
                let _ = meta.create_data_property(scope, key.into(), value.into());
            }
        }
    }
    let entry = shared.entry_module.borrow().clone();
    if let (Some(entry), Some(key)) = (entry, v8::String::new(scope, "main")) {
        let is_main = v8::Boolean::new(scope, entry == name);
        let _ = meta.create_data_property(scope, key.into(), is_main.into());
    }
}

/// Host op that receives `import()` requests: `(specifier, referrer, resolver id)`. The host
/// loads the module graph, then settles the resolver (see `Shared::settle`).
const DYNAMIC_IMPORT_OP: &str = "op_module_import";

fn dynamic_import<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    _host_defined_options: v8::Local<'s, v8::Data>,
    resource_name: v8::Local<'s, v8::Value>,
    specifier: v8::Local<'s, v8::String>,
    _import_attributes: v8::Local<'s, v8::FixedArray>,
) -> Option<v8::Local<'s, v8::Promise>> {
    let shared = shared_of(scope)?;
    let resolver = v8::PromiseResolver::new(scope)?;
    let id = shared.register_dynamic_import(scope, resolver);
    let referrer = v8::Local::<v8::String>::try_from(resource_name)
        .map(|name| name.to_rust_string_lossy(scope))
        .unwrap_or_default();
    let args = [
        Value::String(JsString::from(
            specifier.to_rust_string_lossy(scope).as_str(),
        )),
        Value::String(JsString::from(referrer.as_str())),
        Value::Number(id.0 as f64),
    ];
    let failure = match call_named(scope, &shared, DYNAMIC_IMPORT_OP, &args) {
        Some(Ok(_)) => None,
        Some(Err(err)) => Some(err),
        None => Some(HostError::Velyr(VelyrError::unsupported(
            "dynamic import() is not supported by this host",
        ))),
    };
    if let Some(err) = failure {
        shared.forget_resolver(id);
        let reason = host_error_to_js(scope, &shared, err)
            .unwrap_or_else(|message| plain_error(scope, &message));
        resolver.reject(scope, reason)?;
    }
    Some(resolver.get_promise(scope))
}

fn plain_error<'s>(scope: &mut v8::PinScope<'s, '_>, message: &str) -> v8::Local<'s, v8::Value> {
    let message = v8::String::new(scope, message).unwrap_or_else(|| v8::String::empty(scope));
    v8::Exception::error(scope, message)
}

/// Evaluates the instantiated module `name` and resolves `resolver` with its namespace once
/// evaluation (including top-level await) succeeds; a failure rejects with the same reason.
pub(crate) fn finish_dynamic_import(
    scope: &mut v8::PinScope<'_, '_>,
    shared: &Shared,
    resolver: v8::Local<v8::PromiseResolver>,
    name: &str,
) {
    v8::tc_scope!(let tc, scope);
    let module = match module_by_name(tc, shared, name) {
        Ok(module) => module,
        Err(err) => {
            let reason = plain_error(tc, &err.message);
            let _ = resolver.reject(tc, reason);
            return;
        }
    };
    if matches!(
        module.get_status(),
        v8::ModuleStatus::Uninstantiated | v8::ModuleStatus::Instantiating
    ) {
        let reason = plain_error(tc, &format!("module `{name}` must be instantiated first"));
        let _ = resolver.reject(tc, reason);
        return;
    }
    let namespace = module.get_module_namespace();
    let chained = module
        .evaluate(tc)
        .and_then(|value| v8::Local::<v8::Promise>::try_from(value).ok())
        .and_then(|evaluation| {
            let to_namespace = v8::Function::builder(return_data)
                .data(namespace)
                .build(tc)?;
            evaluation.then(tc, to_namespace)
        });
    match chained {
        Some(promise) => {
            let _ = resolver.resolve(tc, promise.into());
        }
        None => {
            let reason = tc
                .exception()
                .unwrap_or_else(|| plain_error(tc, "module evaluation failed"));
            let _ = resolver.reject(tc, reason);
        }
    }
}

pub(crate) fn return_data(
    _scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments<'_>,
    mut rv: v8::ReturnValue<'_>,
) {
    rv.set(args.data());
}

fn throw_error(scope: &mut v8::PinScope<'_, '_>, message: &str) {
    let message = v8::String::new(scope, message).unwrap_or_else(|| v8::String::empty(scope));
    let error = v8::Exception::error(scope, message);
    scope.throw_exception(error);
}
