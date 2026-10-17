use std::sync::Arc;

use crate::Source;

/// Module source whose text is shared with the host: engines that can read it in place
/// (see [`Isolate::compile_module_shared`](crate::Isolate::compile_module_shared)) skip the copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedSource {
    pub name: String,
    pub text: Arc<str>,
}

impl SharedSource {
    /// Owned copy for engines that only take a [`Source`].
    pub fn to_source(&self) -> Source {
        Source {
            name: self.name.clone(),
            text: self.text.to_string(),
        }
    }
}

impl From<Source> for SharedSource {
    fn from(source: Source) -> Self {
        Self {
            name: source.name,
            text: Arc::from(source.text),
        }
    }
}
