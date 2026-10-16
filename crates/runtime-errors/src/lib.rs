//! Stable error codes and the error type shared by every Velyr crate.
//!
//! Renaming a code is a breaking change; adding one is not (see ADR-011).

use std::error::Error;
use std::fmt;
use std::str::FromStr;

macro_rules! error_codes {
    ($($(#[$doc:meta])* $variant:ident => $text:literal,)+) => {
        /// Stable, machine-readable error category exposed to JS and the CLI.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum ErrorCode {
            $($(#[$doc])* $variant,)+
        }

        impl ErrorCode {
            /// Every code, in declaration order.
            pub const ALL: [ErrorCode; error_codes!(@count $($variant)+)] = [$(ErrorCode::$variant,)+];

            /// The stable `VELYR_*` string for this code.
            pub const fn as_str(self) -> &'static str {
                match self { $(ErrorCode::$variant => $text,)+ }
            }
        }

        impl FromStr for ErrorCode {
            type Err = UnknownErrorCode;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($text => Ok(ErrorCode::$variant),)+
                    _ => Err(UnknownErrorCode(s.to_owned())),
                }
            }
        }
    };
    (@count) => { 0usize };
    (@count $head:ident $($tail:ident)*) => { 1usize + error_codes!(@count $($tail)*) };
}

error_codes! {
    /// The operation was denied by the permission model.
    PermissionDenied => "VELYR_PERMISSION_DENIED",
    /// A resource quota or limit was exceeded.
    QuotaExceeded => "VELYR_QUOTA_EXCEEDED",
    /// The requested file, entry or resource does not exist.
    NotFound => "VELYR_NOT_FOUND",
    /// The target already exists and the operation does not allow overwriting.
    AlreadyExists => "VELYR_ALREADY_EXISTS",
    /// Input data is malformed or fails validation.
    InvalidData => "VELYR_INVALID_DATA",
    /// A network operation failed (connection, DNS, reset).
    Network => "VELYR_NETWORK",
    /// TLS negotiation or certificate validation failed.
    Tls => "VELYR_TLS",
    /// The operation did not finish within its deadline.
    Timeout => "VELYR_TIMEOUT",
    /// The operation was cancelled before completion.
    Cancelled => "VELYR_CANCELLED",
    /// The feature or platform is not supported by this build.
    Unsupported => "VELYR_UNSUPPORTED",
    /// A resource id is invalid, closed or of the wrong kind.
    BadResource => "VELYR_BAD_RESOURCE",
    /// An internal invariant was violated; this is a bug in Velyr.
    Internal => "VELYR_INTERNAL",
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Returned when parsing a string that is not a known `VELYR_*` code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown error code: {0}")]
pub struct UnknownErrorCode(String);

/// A context value attached to an error. Redacted values are not stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextValue {
    /// Safe to print.
    Public(String),
    /// Sensitive; the original value was discarded.
    Redacted,
}

impl fmt::Display for ContextValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContextValue::Public(value) => f.write_str(value),
            ContextValue::Redacted => f.write_str("<redacted>"),
        }
    }
}

/// Error carrying a stable code, a message and redacted-by-default context.
pub struct VelyrError {
    code: ErrorCode,
    message: String,
    cause: Option<Box<dyn Error + Send + Sync>>,
    context: Vec<(&'static str, ContextValue)>,
    span_id: Option<u64>,
}

impl VelyrError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            cause: None,
            context: Vec::new(),
            span_id: None,
        }
    }

    /// Attaches a sensitive value; it is dropped and shown as `<redacted>`.
    pub fn with_context(mut self, key: &'static str, _value: impl fmt::Display) -> Self {
        self.context.push((key, ContextValue::Redacted));
        self
    }

    /// Attaches a value that is safe to print.
    pub fn with_public_context(mut self, key: &'static str, value: impl fmt::Display) -> Self {
        self.context
            .push((key, ContextValue::Public(value.to_string())));
        self
    }

    pub fn with_cause(mut self, cause: impl Error + Send + Sync + 'static) -> Self {
        self.cause = Some(Box::new(cause));
        self
    }

    pub fn with_span_id(mut self, span_id: u64) -> Self {
        self.span_id = Some(span_id);
        self
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn context(&self) -> &[(&'static str, ContextValue)] {
        &self.context
    }

    pub fn span_id(&self) -> Option<u64> {
        self.span_id
    }

    /// `kind` is the permission kind (e.g. `read`); `target_redacted` must already be safe to print.
    pub fn permission_denied(kind: &str, target_redacted: &str) -> Self {
        Self::new(
            ErrorCode::PermissionDenied,
            format!("{kind} access to {target_redacted} denied"),
        )
        .with_public_context("kind", kind)
    }

    pub fn quota_exceeded(class: &str) -> Self {
        Self::new(ErrorCode::QuotaExceeded, format!("{class} quota exceeded"))
            .with_public_context("class", class)
    }

    pub fn bad_resource() -> Self {
        Self::new(ErrorCode::BadResource, "bad resource id")
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    pub fn already_exists(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::AlreadyExists, message)
    }

    pub fn invalid_data(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidData, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Timeout, message)
    }

    pub fn cancelled() -> Self {
        Self::new(ErrorCode::Cancelled, "operation cancelled")
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unsupported, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
}

impl fmt::Display for VelyrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl fmt::Debug for VelyrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let context: Vec<_> = self
            .context
            .iter()
            .map(|(k, v)| (k, v.to_string()))
            .collect();
        f.debug_struct("VelyrError")
            .field("code", &self.code)
            .field("message", &self.message)
            .field("cause", &self.cause.as_ref().map(|c| c.to_string()))
            .field("context", &context)
            .field("span_id", &self.span_id)
            .finish()
    }
}

impl Error for VelyrError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause.as_deref().map(|c| c as &(dyn Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_roundtrips_through_str() {
        for code in ErrorCode::ALL {
            assert_eq!(code.as_str().parse::<ErrorCode>().unwrap(), code);
            assert!(code.as_str().starts_with("VELYR_"));
            assert_eq!(code.to_string(), code.as_str());
        }
        assert_eq!(ErrorCode::ALL.len(), 12);
        assert!("VELYR_NOPE".parse::<ErrorCode>().is_err());
    }

    #[test]
    fn redacted_context_never_leaks() {
        let err = VelyrError::new(ErrorCode::Internal, "boom")
            .with_context("token", "s3cr3t-value")
            .with_public_context("path", "/tmp/x");
        for text in [format!("{err}"), format!("{err:?}"), format!("{err:#?}")] {
            assert!(!text.contains("s3cr3t-value"), "{text}");
        }
        assert!(format!("{err:?}").contains("/tmp/x"));
        assert_eq!(err.context()[0].1, ContextValue::Redacted);
    }

    #[test]
    fn helpers_set_code_and_chain_source() {
        let err = VelyrError::permission_denied("read", "<path>");
        assert_eq!(err.code(), ErrorCode::PermissionDenied);
        assert_eq!(
            VelyrError::quota_exceeded("fs").code(),
            ErrorCode::QuotaExceeded
        );
        assert_eq!(VelyrError::bad_resource().code(), ErrorCode::BadResource);
        assert!(format!("{err}").starts_with("VELYR_PERMISSION_DENIED"));

        let io = std::io::Error::other("disk");
        let err = VelyrError::new(ErrorCode::Internal, "x")
            .with_cause(io)
            .with_span_id(7);
        assert_eq!(std::error::Error::source(&err).unwrap().to_string(), "disk");
        assert_eq!(err.span_id(), Some(7));
    }
}
