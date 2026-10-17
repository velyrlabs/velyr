use std::sync::Arc;

/// A stack frame position in original source terms. `line` and `column` are 1-based, like V8's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedPosition {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

/// Maps a generated `(file, line, column)` (1-based) to its original position, or `None` when the
/// file has no source map. Must be cheap: it runs once per stack frame.
pub type StackFrameMapper = Arc<dyn Fn(&str, u32, u32) -> Option<MappedPosition> + Send + Sync>;
