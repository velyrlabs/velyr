//! Module evaluation (Evaluate(), with top-level await) and dynamic `import()`.
//!
//! Every module of the graph gets its evaluation promise up front; the modules then run in
//! post-order. A module runs once the evaluation promises of its dependencies (except
//! back-edges of a cycle, which are still running) are fulfilled; until then it is skipped
//! and later modules that do not depend on it run, so independent top-level awaits overlap
//! as in the spec. A body that awaits settles its module's promise when it finishes. A
//! graph without top-level await settles synchronously.
//! shortcut: a failure rejects only the modules that depend on it; unrelated modules still
//! run (the spec stops the whole cycle root), upgrade if that becomes observable.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::{Interpreter, JsResult, ModuleId};
use crate::Value;
use crate::interpreter::builtins::promise_ops::Capability;
use crate::interpreter::builtins::slots::IterationKind;
use crate::interpreter::builtins::{Intrinsic, arg};
use crate::interpreter::function::NativeFn;
use crate::interpreter::{Abrupt, ErrorKind, PromiseStatus, object_value};

/// The modules one `evaluate_module` call runs, in post-order: a dependency later in the
/// order is still on the DFS stack (a back-edge of a cycle) and is never waited for.
struct Runner {
    order: Vec<usize>,
    position: HashMap<usize, usize>,
    started: Vec<bool>,
    /// The last module of each module's cycle (itself outside a cycle).
    cycle_root: HashMap<usize, usize>,
    /// Pending dependencies whose settlement already resumes this runner.
    watched: HashSet<usize>,
}

impl Runner {
    /// What `module` waits for to use `dep`: a dependency in another cycle counts as done
    /// only when its whole cycle is (InnerModuleEvaluation step 11.c.iv).
    fn wait_target(&self, module: usize, dep: usize) -> usize {
        match self.cycle_root.get(&dep) {
            Some(&root) if self.cycle_root.get(&module) != Some(&root) => root,
            _ => dep,
        }
    }
}

/// Tarjan's DFS over the unevaluated modules: the post-order and each module's cycle root.
#[derive(Default)]
struct Traversal {
    dfs_index: HashMap<usize, usize>,
    stack: Vec<usize>,
    order: Vec<usize>,
    cycle_root: HashMap<usize, usize>,
}

type RunnerCell = Rc<RefCell<Runner>>;

/// Called with a promise's outcome once it settles.
type Settled = Rc<dyn Fn(&mut Interpreter, Result<(), Value>) -> JsResult<()>>;

enum Readiness {
    Ready,
    Blocked,
    Failed(Value),
}

impl Interpreter {
    /// Evaluate(): the promise settles once `id` and its dependencies ran.
    pub fn evaluate_module(&mut self, id: ModuleId) -> Result<Value, Abrupt> {
        if !self.state.modules[id.0].linked {
            return Err(self.internal("module must be instantiated before evaluation"));
        }
        if !matches!(self.state.modules[id.0].evaluation, Value::Undefined) {
            return Ok(self.state.modules[id.0].evaluation.clone());
        }
        let mut traversal = Traversal::default();
        self.collect_unevaluated(id.0, &mut traversal);
        let Traversal {
            order, cycle_root, ..
        } = traversal;
        self.scoped(|vm| {
            for &module in &order {
                let proto = vm.intrinsic(Intrinsic::PromisePrototype);
                let (promise, record) = vm.new_promise(proto)?;
                // Observed through `promise_state` and `import()`, never unhandled.
                record.borrow_mut().is_handled = true;
                vm.state.modules[module].evaluation = promise;
            }
            let runner = Rc::new(RefCell::new(Runner {
                position: order.iter().enumerate().map(|(i, &m)| (m, i)).collect(),
                started: vec![false; order.len()],
                order,
                cycle_root,
                watched: HashSet::new(),
            }));
            vm.run_evaluation(&runner)
        })?;
        Ok(self.state.modules[id.0].evaluation.clone())
    }

    /// Resolves the `import()` promise behind `resolver` with the namespace of the linked
    /// module `id` once it is evaluated; evaluation failures reject it.
    pub fn finish_dynamic_import(&mut self, resolver: u64, id: ModuleId) -> Result<(), Abrupt> {
        let chained = self.scoped(|vm| {
            let evaluation = vm.evaluate_module(id)?;
            vm.module_namespace(id)?;
            let proto = vm.intrinsic(Intrinsic::PromisePrototype);
            let (promise, record) = vm.new_promise(proto)?;
            let (resolve, reject) = vm.resolving_functions(&record)?;
            let module = id.0;
            let to_namespace: NativeFn = Rc::new(move |vm, _, _| vm.namespace_of(module));
            let on_fulfilled =
                object_value(vm.create_native_function("", 1, to_namespace, false)?);
            vm.keep(&on_fulfilled);
            let evaluation = vm.promise_record(&evaluation).expect("a promise");
            let capability = Capability {
                promise: promise.clone(),
                resolve,
                reject,
            };
            vm.perform_then(&evaluation, &on_fulfilled, &Value::Undefined, capability);
            Ok(promise)
        });
        match chained {
            Ok(promise) => self.settle_promise(resolver, &promise, false),
            Err(Abrupt::Throw(error)) => self.settle_promise(resolver, &error, true),
            Err(abrupt) => Err(abrupt),
        }
    }

    /// `DynamicImport`: hands the request to the host hook; failures reject the promise.
    pub(super) fn dynamic_import(&mut self, options: &Value) -> JsResult<()> {
        let specifier = self.state.acc.clone();
        let (promise, resolver) = self.create_promise()?;
        self.keep(&promise);
        let referrer = self.top().info.code.script.as_ref();
        let referrer = referrer.map_or_else(String::new, |script| script.name.clone());
        match self.request_import(&specifier, options, &referrer, resolver) {
            Err(Abrupt::Throw(error)) => self.settle_promise(resolver, &error, true)?,
            result => result?,
        }
        self.state.acc = promise;
        Ok(())
    }

    fn request_import(
        &mut self,
        specifier: &Value,
        options: &Value,
        referrer: &str,
        resolver: u64,
    ) -> JsResult<()> {
        let specifier = self.coerce_string(specifier)?;
        self.check_import_options(options)?;
        let Some(hook) = self.import_hook.clone() else {
            let message = "VELYR_UNSUPPORTED: dynamic import() is not supported by this host";
            return Err(self.error(ErrorKind::Error, message));
        };
        let args = [
            Value::String(specifier),
            Value::str(referrer),
            Value::Number(resolver as f64),
        ];
        hook(self, &Value::Undefined, &args).map(drop)
    }

    /// EvaluateImportCall: `options.with` must map keys to strings; only `type: "json"` is
    /// supported as an import attribute.
    fn check_import_options(&mut self, options: &Value) -> JsResult<()> {
        if matches!(options, Value::Undefined) {
            return Ok(());
        }
        let Some(options) = self.as_object(options) else {
            let message = "The second argument of import() must be an object";
            return Err(self.error(ErrorKind::Type, message));
        };
        let attributes = self.get_named(options, "with")?;
        if matches!(attributes, Value::Undefined) {
            return Ok(());
        }
        let Some(attributes) = self.as_object(&attributes) else {
            let message = "The 'with' option of import() must be an object";
            return Err(self.error(ErrorKind::Type, message));
        };
        let mut unsupported = None;
        for key in self.enumerable_own(attributes, IterationKind::Keys)? {
            let object_key = self.object_key(&key)?;
            let Value::String(value) = self.get_key(attributes, object_key)? else {
                let message = "Import attribute values must be strings";
                return Err(self.error(ErrorKind::Type, message));
            };
            let is_json_type = matches!(&key, Value::String(k) if k.to_string_lossy() == "type")
                && value.to_string_lossy() == "json";
            if !is_json_type {
                unsupported.get_or_insert((key, value));
            }
        }
        let Some((Value::String(key), value)) = unsupported else {
            return Ok(());
        };
        let message = format!(
            "Import attribute {}: \"{}\" is not supported; only JavaScript modules can be imported",
            key.to_string_lossy(),
            value.to_string_lossy()
        );
        Err(self.error(ErrorKind::Syntax, &message))
    }

    /// Visits the modules reachable from `module` that have not started evaluating; returns
    /// the lowest DFS index `module` reaches (its cycle's root index).
    fn collect_unevaluated(&self, module: usize, traversal: &mut Traversal) -> usize {
        let index = traversal.dfs_index.len();
        traversal.dfs_index.insert(module, index);
        traversal.stack.push(module);
        let mut low = index;
        let record = &self.state.modules[module];
        for &dep in record.deps.as_deref().unwrap_or_default() {
            if !matches!(self.state.modules[dep].evaluation, Value::Undefined) {
                continue;
            }
            match traversal.dfs_index.get(&dep) {
                None => low = low.min(self.collect_unevaluated(dep, traversal)),
                // Still on the stack: a back-edge into the current cycle.
                Some(&dep_index) if !traversal.cycle_root.contains_key(&dep) => {
                    low = low.min(dep_index);
                }
                Some(_) => {}
            }
        }
        traversal.order.push(module);
        if low == index {
            while let Some(member) = traversal.stack.pop() {
                traversal.cycle_root.insert(member, module);
                if member == module {
                    break;
                }
            }
        }
        low
    }

    /// Runs, in order, every module not started yet whose dependencies are done; the rest
    /// run when the dependency they wait for settles.
    fn run_evaluation(&mut self, runner: &RunnerCell) -> JsResult<()> {
        let count = runner.borrow().order.len();
        for index in 0..count {
            let (module, started) = {
                let runner = runner.borrow();
                (runner.order[index], runner.started[index])
            };
            if started {
                continue;
            }
            let outcome = match self.readiness(runner, index, module)? {
                Readiness::Blocked => continue,
                Readiness::Failed(reason) => Err(reason),
                Readiness::Ready => {
                    runner.borrow_mut().started[index] = true;
                    match self.execute_module(module)? {
                        Some(outcome) => outcome,
                        None => continue,
                    }
                }
            };
            runner.borrow_mut().started[index] = true;
            self.finish_module(module, outcome);
        }
        Ok(())
    }

    /// Whether `module` (at `index` of the order) can run; a pending dependency resumes the
    /// runner when it settles.
    fn readiness(
        &mut self,
        runner: &RunnerCell,
        index: usize,
        module: usize,
    ) -> JsResult<Readiness> {
        let deps = self.state.modules[module].deps.clone().unwrap_or_default();
        for dep in deps {
            let dep = runner.borrow().wait_target(module, dep);
            let position = runner.borrow().position.get(&dep).copied();
            match position {
                Some(position) if position >= index => continue,
                Some(position) if !runner.borrow().started[position] => {
                    return Ok(Readiness::Blocked);
                }
                _ => {}
            }
            let evaluation = self.state.modules[dep].evaluation.clone();
            match self.promise_status(&evaluation) {
                Some(PromiseStatus::Pending) => {
                    if runner.borrow_mut().watched.insert(dep) {
                        let runner = runner.clone();
                        let resume: Settled = Rc::new(move |vm, _| {
                            runner.borrow_mut().watched.remove(&dep);
                            vm.run_evaluation(&runner)
                        });
                        self.on_settled(&evaluation, resume)?;
                    }
                    return Ok(Readiness::Blocked);
                }
                Some(PromiseStatus::Rejected(reason)) => return Ok(Readiness::Failed(reason)),
                _ => {}
            }
        }
        Ok(Readiness::Ready)
    }

    /// Runs the body of `module`: its outcome, or `None` while a top-level await is pending
    /// (the module is finished when the body's promise settles).
    fn execute_module(&mut self, module: usize) -> JsResult<Option<Result<(), Value>>> {
        let body = self.state.modules[module].code.body.clone();
        let is_async = self.code_info(&body).coroutine.is_some();
        match self.run_module_code(module, &body) {
            Ok(promise) if is_async => match self.promise_status(&promise) {
                Some(PromiseStatus::Pending) => {
                    let finish: Settled = Rc::new(move |vm, outcome| {
                        vm.finish_module(module, outcome);
                        Ok(())
                    });
                    self.on_settled(&promise, finish)?;
                    Ok(None)
                }
                Some(PromiseStatus::Rejected(reason)) => Ok(Some(Err(reason))),
                _ => Ok(Some(Ok(()))),
            },
            Ok(_) => Ok(Some(Ok(()))),
            Err(Abrupt::Throw(error)) => Ok(Some(Err(error))),
            Err(abrupt) => Err(abrupt),
        }
    }

    fn finish_module(&mut self, module: usize, outcome: Result<(), Value>) {
        let evaluation = self.state.modules[module].evaluation.clone();
        let record = self.promise_record(&evaluation).expect("a promise");
        match outcome {
            Ok(()) => self.fulfill_promise(&record, Value::Undefined),
            Err(reason) => self.reject_promise(&record, reason),
        }
    }

    /// Calls `then` with the outcome of `promise` once it settles.
    fn on_settled(&mut self, promise: &Value, then: Settled) -> JsResult<()> {
        let record = self.promise_record(promise).expect("a promise");
        let mut reactions = [Value::Undefined, Value::Undefined];
        for (rejects, reaction) in reactions.iter_mut().enumerate() {
            let then = then.clone();
            let call: NativeFn = Rc::new(move |vm, _, args| {
                let outcome = if rejects == 1 {
                    Err(arg(args, 0))
                } else {
                    Ok(())
                };
                then(vm, outcome).map(|()| Value::Undefined)
            });
            *reaction = object_value(self.create_native_function("", 1, call, false)?);
            self.keep(reaction);
        }
        let noop = object_value(self.state.intrinsics.function_prototype);
        let none = Capability {
            promise: Value::Undefined,
            resolve: noop.clone(),
            reject: noop,
        };
        self.perform_then(&record, &reactions[0], &reactions[1], none);
        Ok(())
    }
}
