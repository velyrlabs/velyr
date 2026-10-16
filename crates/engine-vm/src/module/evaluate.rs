//! Module evaluation (Evaluate(), with top-level await) and dynamic `import()`.
//!
//! Every module of the graph gets its evaluation promise up front; the modules then run in
//! post-order. A module waits for the promises of its dependencies (except back-edges of a
//! cycle, which are still running) and, with top-level await, for its body's promise. A
//! graph without top-level await settles synchronously.
//! shortcut: independent async modules run one after the other instead of concurrently,
//! and a failure does not stop unrelated modules later in the order; upgrade to the
//! spec's [[AsyncParentModules]] bookkeeping if that ordering becomes observable.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use super::{Interpreter, JsResult, ModuleId};
use crate::Value;
use crate::interpreter::builtins::promise_ops::Capability;
use crate::interpreter::builtins::{Intrinsic, arg};
use crate::interpreter::function::NativeFn;
use crate::interpreter::{Abrupt, ErrorKind, PromiseStatus, object_value};

/// The modules one `evaluate_module` call runs, in order, and the next one to run.
struct Runner {
    order: Vec<usize>,
    next: usize,
}

type RunnerCell = Rc<RefCell<Runner>>;

impl Interpreter {
    /// Evaluate(): the promise settles once `id` and its dependencies ran.
    pub fn evaluate_module(&mut self, id: ModuleId) -> Result<Value, Abrupt> {
        if !self.state.modules[id.0].linked {
            return Err(self.internal("module must be instantiated before evaluation"));
        }
        if !matches!(self.state.modules[id.0].evaluation, Value::Undefined) {
            return Ok(self.state.modules[id.0].evaluation.clone());
        }
        let mut order = Vec::new();
        self.collect_unevaluated(id.0, &mut HashSet::new(), &mut order);
        self.scoped(|vm| {
            for &module in &order {
                let proto = vm.intrinsic(Intrinsic::PromisePrototype);
                let (promise, record) = vm.new_promise(proto)?;
                // Observed through `promise_state` and `import()`, never unhandled.
                record.borrow_mut().is_handled = true;
                vm.state.modules[module].evaluation = promise;
            }
            let runner = Rc::new(RefCell::new(Runner { order, next: 0 }));
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
    pub(super) fn dynamic_import(&mut self) -> JsResult<()> {
        let specifier = self.state.acc.clone();
        let (promise, resolver) = self.create_promise()?;
        self.keep(&promise);
        let referrer = self.top().info.code.script.as_ref();
        let referrer = referrer.map_or_else(String::new, |script| script.name.clone());
        match self.request_import(&specifier, &referrer, resolver) {
            Err(Abrupt::Throw(error)) => self.settle_promise(resolver, &error, true)?,
            result => result?,
        }
        self.state.acc = promise;
        Ok(())
    }

    fn request_import(&mut self, specifier: &Value, referrer: &str, resolver: u64) -> JsResult<()> {
        let specifier = self.coerce_string(specifier)?;
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

    /// Post-order of the modules reachable from `module` that have not started evaluating.
    fn collect_unevaluated(
        &self,
        module: usize,
        seen: &mut HashSet<usize>,
        order: &mut Vec<usize>,
    ) {
        let record = &self.state.modules[module];
        if !matches!(record.evaluation, Value::Undefined) || !seen.insert(module) {
            return;
        }
        for &dep in record.deps.as_deref().unwrap_or_default() {
            self.collect_unevaluated(dep, seen, order);
        }
        order.push(module);
    }

    /// Runs modules until the order is done or one has to wait for a promise.
    fn run_evaluation(&mut self, runner: &RunnerCell) -> JsResult<()> {
        loop {
            let (module, running) = {
                let runner = runner.borrow();
                match runner.order.get(runner.next) {
                    Some(&module) => (module, runner.order[runner.next..].to_vec()),
                    None => return Ok(()),
                }
            };
            let deps = self.state.modules[module].deps.clone().unwrap_or_default();
            let mut failure = None;
            for dep in deps.into_iter().filter(|dep| !running.contains(dep)) {
                let evaluation = self.state.modules[dep].evaluation.clone();
                match self.promise_status(&evaluation) {
                    Some(PromiseStatus::Pending) => {
                        return self.wait_for(&evaluation, runner, None);
                    }
                    Some(PromiseStatus::Rejected(reason)) => {
                        failure = Some(reason);
                        break;
                    }
                    _ => {}
                }
            }
            let outcome = match failure {
                Some(reason) => Err(reason),
                None => {
                    let body = self.state.modules[module].code.body.clone();
                    let is_async = self.code_info(&body).coroutine.is_some();
                    match self.run_module_code(module, &body) {
                        Ok(promise) if is_async => match self.promise_status(&promise) {
                            Some(PromiseStatus::Pending) => {
                                return self.wait_for(&promise, runner, Some(module));
                            }
                            Some(PromiseStatus::Rejected(reason)) => Err(reason),
                            _ => Ok(()),
                        },
                        Ok(_) => Ok(()),
                        Err(Abrupt::Throw(error)) => Err(error),
                        Err(abrupt) => return Err(abrupt),
                    }
                }
            };
            self.finish_module(module, outcome);
            runner.borrow_mut().next += 1;
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

    /// Continues the runner once `promise` settles; `module` is the module whose body
    /// returned it (settled with the outcome), `None` for a dependency.
    fn wait_for(
        &mut self,
        promise: &Value,
        runner: &RunnerCell,
        module: Option<usize>,
    ) -> JsResult<()> {
        let record = self.promise_record(promise).expect("a promise");
        let mut reactions = [Value::Undefined, Value::Undefined];
        for (rejects, reaction) in reactions.iter_mut().enumerate() {
            let runner = runner.clone();
            let call: NativeFn = Rc::new(move |vm, _, args| {
                if let Some(module) = module {
                    let outcome = if rejects == 1 {
                        Err(arg(args, 0))
                    } else {
                        Ok(())
                    };
                    vm.finish_module(module, outcome);
                    runner.borrow_mut().next += 1;
                }
                vm.run_evaluation(&runner).map(|()| Value::Undefined)
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
