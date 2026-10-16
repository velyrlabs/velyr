//! `node:async_hooks` (`AsyncLocalStorage`, `AsyncResource`) and the frame module under it.

pub(crate) const ASYNC_EXPORTS: &[&str] = &[
    "AsyncLocalStorage",
    "AsyncResource",
    "createHook",
    "executionAsyncId",
    "executionAsyncResource",
    "triggerAsyncId",
];

pub(crate) const ASYNC_FACTORIES: &[(&str, &str)] = &[
    (
        "internal/async_context",
        include_str!("../js/async_context.js"),
    ),
    ("async_hooks", include_str!("../js/async_hooks.js")),
];
