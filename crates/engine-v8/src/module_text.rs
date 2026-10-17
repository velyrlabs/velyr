//! Module source text handed to V8. A shared ASCII buffer becomes an external one-byte string
//! that V8 reads in place; the string owns one `Arc` clone, released by V8's finalizer.

use std::sync::Arc;

use velyr_engine_api::{SharedSource, Source};

/// A module to compile: a [`Source`] (copied into V8) or a [`SharedSource`].
pub(crate) struct ModuleInput<'a> {
    pub(crate) name: &'a str,
    text: Text<'a>,
}

enum Text<'a> {
    Copied(&'a str),
    Shared(&'a Arc<str>),
}

impl<'a> From<&'a Source> for ModuleInput<'a> {
    fn from(source: &'a Source) -> Self {
        let text = Text::Copied(&source.text);
        Self {
            name: &source.name,
            text,
        }
    }
}

impl<'a> From<&'a SharedSource> for ModuleInput<'a> {
    fn from(source: &'a SharedSource) -> Self {
        let text = Text::Shared(&source.text);
        Self {
            name: &source.name,
            text,
        }
    }
}

impl ModuleInput<'_> {
    pub(crate) fn text(&self) -> &str {
        match self.text {
            Text::Copied(text) => text,
            Text::Shared(text) => text,
        }
    }

    /// `None` when the text exceeds V8's maximum string length.
    pub(crate) fn text_to_v8<'s>(
        &self,
        scope: &v8::PinScope<'s, '_, ()>,
    ) -> Option<v8::Local<'s, v8::String>> {
        match self.text {
            Text::Shared(text) if can_be_external(text) => external_string(scope, text),
            _ => v8::String::new(scope, self.text()),
        }
    }
}

/// V8 reads external one-byte strings as Latin-1, which matches UTF-8 only for ASCII. Empty and
/// over-long text take the copy path: for those V8 would dispose or leak the resource itself.
fn can_be_external(text: &str) -> bool {
    !text.is_empty() && text.len() <= v8::String::MAX_LENGTH && text.is_ascii()
}

#[allow(unsafe_code)]
fn external_string<'s>(
    scope: &v8::PinScope<'s, '_, ()>,
    text: &Arc<str>,
) -> Option<v8::Local<'s, v8::String>> {
    let len = text.len();
    let data = Arc::into_raw(Arc::clone(text)).cast::<u8>().cast_mut();
    // SAFETY: `data`/`len` describe a live, immutable ASCII buffer (checked by `can_be_external`)
    // kept alive by the `Arc` clone leaked just above; nothing writes through the pointer (`str`
    // is never mutated while shared). V8 owns that clone from here on and hands it back exactly
    // once to `release_keepalive` when the string is finalized or the isolate is torn down. With
    // `0 < len <= MAX_LENGTH`, `NewExternalOneByte` always takes the resource, so it is neither
    // freed early nor dropped on this path.
    unsafe { v8::String::new_external_onebyte_raw(scope, data.cast(), len, release_keepalive) }
}

#[allow(unsafe_code)]
unsafe extern "C" fn release_keepalive(data: *mut std::ffi::c_char, len: usize) {
    let text = std::ptr::slice_from_raw_parts(data.cast::<u8>().cast_const(), len) as *const str;
    // SAFETY: V8 calls this once per external string with the pointer and length given to
    // `new_external_onebyte_raw` in `external_string`, i.e. from `Arc::into_raw` of an `Arc<str>`
    // of exactly `len` bytes; this reclaims that one strong count. `Arc` drops are thread-safe,
    // so the finalizer's thread does not matter.
    drop(unsafe { Arc::from_raw(text) });
}
