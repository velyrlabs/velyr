//! `Intl.Segmenter` boundaries over ICU4X segmentation, in UTF-16 indices (JS strings).
//! shortcut: segmentation is the locale-independent (root) one; ICU4X tailors only
//! complex scripts, which `new_auto` already detects.

use icu_segmenter::options::{SentenceBreakInvariantOptions, WordBreakInvariantOptions};
use icu_segmenter::{GraphemeClusterSegmenter, SentenceSegmenter, WordSegmenter};
use velyr_engine_vm::Value;
use velyr_engine_vm::interpreter::Interpreter;

use super::{OpResult, string_arg};

/// `(granularity, text)`: `start:wordLike` per segment, `;` separated (`wordLike` is 1/0,
/// always 0 outside the word granularity).
pub(crate) fn segment(_: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let units = match args.get(1) {
        Some(Value::String(s)) => s.to_utf16(),
        _ => Vec::new(),
    };
    let mut segments = Vec::new();
    match string_arg(args, 0).as_str() {
        "word" => {
            let mut breaks =
                WordSegmenter::new_auto(WordBreakInvariantOptions::default()).segment_utf16(&units);
            let mut start = breaks.next().unwrap_or(0);
            while let Some(end) = breaks.next() {
                segments.push(format!("{start}:{}", u8::from(breaks.is_word_like())));
                start = end;
            }
        }
        granularity => {
            let breaks: Vec<usize> = if granularity == "sentence" {
                SentenceSegmenter::new(SentenceBreakInvariantOptions::default())
                    .segment_utf16(&units)
                    .collect()
            } else {
                GraphemeClusterSegmenter::new()
                    .segment_utf16(&units)
                    .collect()
            };
            let starts = breaks.iter().take(breaks.len().saturating_sub(1));
            segments.extend(starts.map(|start| format!("{start}:0")));
        }
    }
    Ok(Value::str(&segments.join(";")))
}
