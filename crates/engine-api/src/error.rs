use std::error::Error;
use std::fmt;

use velyr_runtime_errors::{ErrorCode, VelyrError};

use crate::Value;

/// Failure of an engine-level operation (creating an isolate, registering a host function).
/// JS exceptions are not `EngineError`s; they are [`JsException`](crate::JsException).
#[derive(Debug)]
pub struct EngineError(VelyrError);

impl EngineError {
    pub fn code(&self) -> ErrorCode {
        self.0.code()
    }
}

impl From<VelyrError> for EngineError {
    fn from(err: VelyrError) -> Self {
        Self(err)
    }
}

impl From<EngineError> for VelyrError {
    fn from(err: EngineError) -> Self {
        err.0
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Error for EngineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

/// Failure returned by a [`HostFn`](crate::HostFn); the engine turns it into a JS exception.
#[derive(Debug)]
pub enum HostError {
    /// Becomes a JS `TypeError` with this message.
    TypeError(String),
    /// Throws this JS value as-is.
    Thrown(Value),
    /// A runtime error; the adapter maps its code to a JS error carrying that code.
    Velyr(VelyrError),
}

impl From<VelyrError> for HostError {
    fn from(err: VelyrError) -> Self {
        Self::Velyr(err)
    }
}

impl From<HostError> for VelyrError {
    fn from(err: HostError) -> Self {
        match err {
            HostError::Velyr(err) => err,
            HostError::TypeError(message) => VelyrError::invalid_data(message),
            HostError::Thrown(_) => VelyrError::invalid_data("host function threw a JS value"),
        }
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TypeError(message) => write!(f, "TypeError: {message}"),
            Self::Thrown(_) => f.write_str("thrown JS value"),
            Self::Velyr(err) => err.fmt(f),
        }
    }
}

impl Error for HostError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Velyr(err) => Some(err),
            _ => None,
        }
    }
}
