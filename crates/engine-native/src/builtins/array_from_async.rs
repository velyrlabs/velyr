//! `Array.fromAsync` (ES2026): a JS prelude that follows the spec's await steps literally.

use velyr_engine_vm::interpreter::Interpreter;

use crate::prelude::run_prelude;

pub(crate) fn install(vm: &mut Interpreter) -> Result<(), String> {
    run_prelude(vm, "velyr:array_from_async", include_str!("array_from_async.js"))
}
