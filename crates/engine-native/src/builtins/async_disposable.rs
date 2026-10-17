//! Placeholder slot: the async_disposable prelude is filled by a later task.

use velyr_engine_vm::interpreter::Interpreter;

use crate::prelude::run_prelude;

pub(crate) fn install(vm: &mut Interpreter) -> Result<(), String> {
    run_prelude(
        vm,
        "velyr:async_disposable",
        include_str!("async_disposable.js"),
    )
}
