//! C ABI types of `js_native_api_types.h` and `node_api_types.h` (Node-API version 8).
#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_void};

use crate::env::Env;

pub type napi_env = *const Env;
/// Address of a V8 handle slot: the bit pattern of a `v8::Local` (see `ffi::to_napi`).
pub type napi_value = *mut c_void;
pub type napi_ref = *mut c_void;
pub type napi_handle_scope = *mut c_void;
pub type napi_escapable_handle_scope = *mut c_void;
pub type napi_callback_info = *const c_void;
pub type napi_callback = Option<unsafe extern "C" fn(napi_env, napi_callback_info) -> napi_value>;
pub type napi_finalize = Option<unsafe extern "C" fn(napi_env, *mut c_void, *mut c_void)>;
pub type napi_addon_register_func =
    Option<unsafe extern "C" fn(napi_env, napi_value) -> napi_value>;

/// `NAPI_AUTO_LENGTH`: the string is NUL-terminated.
pub const NAPI_AUTO_LENGTH: usize = usize::MAX;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "mirrors napi_status; some codes belong to APIs not implemented yet"
)]
pub enum Status {
    Ok = 0,
    InvalidArg,
    ObjectExpected,
    StringExpected,
    NameExpected,
    FunctionExpected,
    NumberExpected,
    BooleanExpected,
    ArrayExpected,
    GenericFailure,
    PendingException,
    Cancelled,
    EscapeCalledTwice,
    HandleScopeMismatch,
    CallbackScopeMismatch,
    QueueFull,
    Closing,
    BigintExpected,
    DateExpected,
    ArraybufferExpected,
    DetachableArraybufferExpected,
    WouldDeadlock,
    NoExternalBuffersAllowed,
    CannotRunJs,
}

impl Status {
    /// The text Node reports in `napi_extended_error_info.error_message`.
    pub(crate) fn message(self) -> *const c_char {
        let text: &'static std::ffi::CStr = match self {
            Status::Ok => return std::ptr::null(),
            Status::InvalidArg => c"Invalid argument",
            Status::ObjectExpected => c"An object was expected",
            Status::StringExpected => c"A string was expected",
            Status::NameExpected => c"A string or symbol was expected",
            Status::FunctionExpected => c"A function was expected",
            Status::NumberExpected => c"A number was expected",
            Status::BooleanExpected => c"A boolean was expected",
            Status::ArrayExpected => c"An array was expected",
            Status::GenericFailure => c"Unknown failure",
            Status::PendingException => c"An exception is pending",
            Status::Cancelled => c"The async work item was cancelled",
            Status::EscapeCalledTwice => c"napi_escape_handle already called on scope",
            Status::HandleScopeMismatch => c"Invalid handle scope usage",
            Status::CallbackScopeMismatch => c"Invalid callback scope usage",
            Status::QueueFull => c"Thread-safe function queue is full",
            Status::Closing => c"Thread-safe function handle is closing",
            Status::BigintExpected => c"A bigint was expected",
            Status::DateExpected => c"A date was expected",
            Status::ArraybufferExpected => c"An arraybuffer was expected",
            Status::DetachableArraybufferExpected => c"A detachable arraybuffer was expected",
            Status::WouldDeadlock => c"Main thread would deadlock",
            Status::NoExternalBuffersAllowed => c"External buffers are not allowed",
            Status::CannotRunJs => c"Cannot run JavaScript",
        };
        text.as_ptr()
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum ValueType {
    Undefined,
    Null,
    Boolean,
    Number,
    String,
    Symbol,
    Object,
    Function,
    External,
    Bigint,
}

/// `napi_typedarray_type`; read from C as a plain integer and range-checked.
pub type napi_typedarray_type = i32;

#[repr(C)]
#[derive(Debug)]
pub struct ExtendedErrorInfo {
    pub error_message: *const c_char,
    pub engine_reserved: *mut c_void,
    pub engine_error_code: u32,
    pub error_code: Status,
}

pub const NAPI_WRITABLE: i32 = 1 << 0;
pub const NAPI_ENUMERABLE: i32 = 1 << 1;
pub const NAPI_CONFIGURABLE: i32 = 1 << 2;
pub const NAPI_STATIC: i32 = 1 << 10;

#[repr(C)]
pub struct PropertyDescriptor {
    pub utf8name: *const c_char,
    pub name: napi_value,
    pub method: napi_callback,
    pub getter: napi_callback,
    pub setter: napi_callback,
    pub value: napi_value,
    pub attributes: i32,
    pub data: *mut c_void,
}

/// `napi_module`, used only by the deprecated `napi_module_register`.
#[repr(C)]
pub struct NapiModule {
    pub nm_version: i32,
    pub nm_flags: u32,
    pub nm_filename: *const c_char,
    pub nm_register_func: napi_addon_register_func,
    pub nm_modname: *const c_char,
    pub nm_priv: *mut c_void,
    pub reserved: [*mut c_void; 4],
}
