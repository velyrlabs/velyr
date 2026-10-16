use velyr_runtime_errors::{ErrorCode, VelyrError};

/// A failure that JS turns into `DOMException(message, name)`. It crosses the op boundary as the message
/// `"<name>: <message>"`, so the JS side can rebuild the exception from the rejection alone.
#[derive(Debug)]
pub(crate) struct CryptoError {
    pub name: &'static str,
    pub message: String,
}

pub(crate) type CryptoResult<T = Vec<Vec<u8>>> = Result<T, CryptoError>;

impl CryptoError {
    pub fn new(name: &'static str, message: impl Into<String>) -> Self {
        Self {
            name,
            message: message.into(),
        }
    }

    pub fn operation(message: impl Into<String>) -> Self {
        Self::new("OperationError", message)
    }

    pub fn data(message: impl Into<String>) -> Self {
        Self::new("DataError", message)
    }

    pub fn not_supported(message: impl Into<String>) -> Self {
        Self::new("NotSupportedError", message)
    }
}

impl From<CryptoError> for VelyrError {
    fn from(err: CryptoError) -> Self {
        let code = if err.name == "NotSupportedError" {
            ErrorCode::Unsupported
        } else {
            ErrorCode::InvalidData
        };
        VelyrError::new(code, format!("{}: {}", err.name, err.message))
    }
}

impl From<aws_lc_rs::error::Unspecified> for CryptoError {
    fn from(_: aws_lc_rs::error::Unspecified) -> Self {
        Self::operation("The operation failed for an operation-specific reason")
    }
}
