//! Handle scopes opened by addons. N-API scopes are not lexical, so the V8 scope objects live
//! on the heap (address-stable, as V8 requires) in the env's stack until closed.

use std::ffi::c_void;
use std::pin::Pin;

use crate::ffi::{local, napi_fn, napi_scope, to_napi, write};
use crate::types::{Status, napi_escapable_handle_scope, napi_handle_scope, napi_value};

type PlainStorage = v8::ScopeStorage<v8::HandleScope<'static>>;
type EscapableStorage = v8::ScopeStorage<v8::EscapableHandleScope<'static, 'static>>;

pub(crate) enum OpenScope {
    Plain(*mut PlainStorage),
    Escapable {
        // Declared before `storage` so it is dropped first.
        scope: v8::PinnedRef<'static, v8::EscapableHandleScope<'static, 'static>>,
        storage: *mut EscapableStorage,
        escaped: bool,
    },
}

impl OpenScope {
    fn id(&self) -> *mut c_void {
        match self {
            OpenScope::Plain(storage) => storage.cast(),
            OpenScope::Escapable { storage, .. } => storage.cast(),
        }
    }
}

impl Drop for OpenScope {
    fn drop(&mut self) {
        // SAFETY: each storage was leaked by `open` below and is freed exactly once, here; the
        // env stack drops scopes innermost first, which is the order V8 requires.
        unsafe {
            match self {
                OpenScope::Plain(storage) => drop(Box::from_raw(*storage)),
                OpenScope::Escapable { storage, .. } => drop(Box::from_raw(*storage)),
            }
        }
    }
}

/// Leaks `storage` to the heap and initializes the V8 scope at its final address.
macro_rules! pin_on_heap {
    ($storage:expr) => {{
        let raw = Box::into_raw(Box::new($storage));
        // SAFETY: the box is never moved until `OpenScope::drop` frees it, so pinning is sound.
        let scope = unsafe { Pin::new_unchecked(&mut *raw) }.init();
        (raw, scope)
    }};
}

napi_fn! {
    fn napi_open_handle_scope(env, result: *mut napi_handle_scope) {
        napi_scope!(env, scope);
        let storage = v8::HandleScope::new(&mut **scope);
        // SAFETY: only the lifetime parameter changes; the scope is closed before its isolate.
        let storage: v8::ScopeStorage<v8::HandleScope<'static>> = unsafe { std::mem::transmute(storage) };
        let (raw, _) = pin_on_heap!(storage);
        env.scopes.borrow_mut().push(OpenScope::Plain(raw));
        write(result, raw.cast())
    }
}

napi_fn! {
    fn napi_open_escapable_handle_scope(env, result: *mut napi_escapable_handle_scope) {
        napi_scope!(env, scope);
        let storage = v8::EscapableHandleScope::new(&mut **scope);
        // SAFETY: only the lifetime parameters change; the scope is closed before its isolate.
        let storage: EscapableStorage = unsafe { std::mem::transmute(storage) };
        let (raw, scope) = pin_on_heap!(storage);
        env.scopes.borrow_mut().push(OpenScope::Escapable { scope, storage: raw, escaped: false });
        write(result, raw.cast())
    }
}

/// Pops `id` if it is the innermost open scope.
fn close(env: &crate::env::Env, id: *mut c_void) -> Result<(), Status> {
    let mut scopes = env.scopes.borrow_mut();
    match scopes.last() {
        None if id.is_null() => Err(Status::InvalidArg),
        Some(top) if top.id() == id => {
            scopes.pop();
            Ok(())
        }
        _ => Err(Status::HandleScopeMismatch),
    }
}

napi_fn! {
    fn napi_close_handle_scope(env, scope: napi_handle_scope) {
        close(env, scope)
    }
}

napi_fn! {
    fn napi_close_escapable_handle_scope(env, scope: napi_escapable_handle_scope) {
        close(env, scope)
    }
}

napi_fn! {
    fn napi_escape_handle(
        env,
        scope: napi_escapable_handle_scope,
        escapee: napi_value,
        result: *mut napi_value,
    ) {
        let value = local(escapee)?;
        let mut scopes = env.scopes.borrow_mut();
        let Some(OpenScope::Escapable { scope: escapable, escaped, .. }) =
            scopes.iter_mut().find(|open| open.id() == scope)
        else {
            return Err(Status::InvalidArg);
        };
        if *escaped {
            return Err(Status::EscapeCalledTwice);
        }
        *escaped = true;
        write(result, to_napi(escapable.escape(value)))
    }
}
