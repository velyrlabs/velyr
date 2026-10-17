//! Source maps of loaded modules, looked up by the `file://` URL V8 reports in stack frames.
//!
//! A module's map comes from its transform, or from a trailing `//# sourceMappingURL=` (inline
//! `data:` URL or a file next to the module). Maps are parsed on the first lookup and cached.
//! A file-based map is read only when [`authorize_path`] allows it. Anything missing or malformed
//! maps to `None`, which leaves the original frame untouched.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use oxc_sourcemap::{OwnedSourceMap, Token};
use url::Url;
use velyr_runtime_permissions::CapabilitySet;

use crate::{ResolvedModule, authorize_path};

/// `(file, line, column)`, all 1-based like V8's positions, to the original `(file, line, column)`.
pub type PositionMapper = Arc<dyn Fn(&str, u32, u32) -> Option<(String, u32, u32)> + Send + Sync>;

const MARKERS: [&str; 2] = ["//# sourceMappingURL=", "//@ sourceMappingURL="];
const BASE64_PREFIX: &str = "base64,";

/// Registry of the maps of every module a host loaded. Cheap to clone; clones share state.
#[derive(Clone, Default)]
pub struct SourceMaps {
    entries: Arc<Mutex<HashMap<String, Entry>>>,
}

enum Entry {
    Pending(Raw),
    Ready(Option<Arc<ParsedMap>>),
}

enum Raw {
    Json(String),
    DataUrl(String),
    File(PathBuf, Arc<CapabilitySet>),
}

struct ParsedMap {
    map: oxc_sourcemap::SourceMap<'static>,
    /// Sorted by generated position.
    tokens: Vec<Token>,
}

impl SourceMaps {
    /// Records where `module`'s map is. `transform_map` (JSON) wins over a `sourceMappingURL` in
    /// `code`. Nothing is read or parsed here.
    pub(crate) fn register(
        &self,
        module: &ResolvedModule,
        code: &str,
        transform_map: Option<String>,
        permissions: &Arc<CapabilitySet>,
    ) {
        let raw = match transform_map {
            Some(json) => Raw::Json(json),
            None => match raw_from_comment(module, code, permissions) {
                Some(raw) => raw,
                None => return,
            },
        };
        self.lock()
            .insert(module.id.as_str().to_owned(), Entry::Pending(raw));
    }

    /// For `Isolate::set_prepare_stack_trace`: maps a generated position to the original one.
    pub fn mapper(&self) -> PositionMapper {
        let maps = self.clone();
        Arc::new(move |file, line, column| maps.lookup(file, line, column))
    }

    fn lookup(&self, file: &str, line: u32, column: u32) -> Option<(String, u32, u32)> {
        let map = self.parsed(file)?;
        let (line, column) = (line.checked_sub(1)?, column.checked_sub(1)?);
        let key = (line, column);
        let after = map
            .tokens
            .partition_point(|t| (t.get_dst_line(), t.get_dst_col()) <= key);
        let token = map.tokens.get(after.checked_sub(1)?)?;
        if token.get_dst_line() != line {
            return None;
        }
        let source = map.map.get_source(token.get_source_id()?)?;
        Some((
            source.to_owned(),
            token.get_src_line() + 1,
            token.get_src_col() + 1,
        ))
    }

    fn parsed(&self, file: &str) -> Option<Arc<ParsedMap>> {
        let mut entries = self.lock();
        let entry = entries.get_mut(file)?;
        if let Entry::Pending(raw) = entry {
            *entry = Entry::Ready(parse(raw).map(Arc::new));
        }
        match entry {
            Entry::Ready(map) => map.clone(),
            Entry::Pending(_) => None,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn raw_from_comment(
    module: &ResolvedModule,
    code: &str,
    permissions: &Arc<CapabilitySet>,
) -> Option<Raw> {
    let target = MARKERS
        .iter()
        .filter_map(|marker| {
            code.rfind(marker)
                .map(|at| (at, &code[at + marker.len()..]))
        })
        .max_by_key(|(at, _)| *at)?
        .1
        .split_whitespace()
        .next()?;
    if target.starts_with("data:") {
        return Some(Raw::DataUrl(target.to_owned()));
    }
    let path = Url::parse(module.id.as_str())
        .ok()?
        .join(target)
        .ok()?
        .to_file_path()
        .ok()?;
    Some(Raw::File(path, Arc::clone(permissions)))
}

fn parse(raw: &Raw) -> Option<ParsedMap> {
    let json = match raw {
        Raw::Json(json) => json.clone(),
        Raw::DataUrl(url) => decode_base64_json(url)?,
        Raw::File(path, permissions) => {
            authorize_path(path, permissions).ok()?;
            std::fs::read_to_string(path).ok()?
        }
    };
    let map = OwnedSourceMap::from_json_string(&json).ok()?.into_inner();
    let tokens = map.get_tokens().collect();
    Some(ParsedMap { map, tokens })
}

// shortcut: only `;base64,` data URLs; percent-encoded JSON data URLs are ignored, add if a tool emits them.
fn decode_base64_json(url: &str) -> Option<String> {
    let payload = &url[url.find(BASE64_PREFIX)? + BASE64_PREFIX.len()..];
    let (mut bits, mut have) = (0u32, 0u32);
    let mut bytes = Vec::with_capacity(payload.len() * 3 / 4);
    for c in payload.bytes().take_while(|&c| c != b'=') {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(value);
        have += 6;
        if have >= 8 {
            have -= 8;
            bytes.push((bits >> have) as u8);
            bits &= (1 << have) - 1;
        }
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModuleId;
    use velyr_runtime_permissions::{Capability, PathPattern};

    /// Generated line 1 -> orig.ts:1:1, generated line 2 -> orig.ts:2:1.
    const MAP: &str = r#"{"version":3,"sources":["orig.ts"],"names":[],"mappings":"AAAA;AACA"}"#;
    const MAP_BASE64: &str = "eyJ2ZXJzaW9uIjozLCJzb3VyY2VzIjpbIm9yaWcudHMiXSwibmFtZXMiOltdLCJtYXBwaW5ncyI6IkFBQUE7QUFDQSJ9";

    fn allow(dir: &std::path::Path) -> Arc<CapabilitySet> {
        let pattern = PathPattern::new(dir.to_str().unwrap());
        CapabilitySet::builder()
            .allow(Capability::ImportLocal(pattern))
            .build()
    }

    fn module(dir: &std::path::Path) -> ResolvedModule {
        let path = dir.join("bundle.js");
        ResolvedModule {
            id: ModuleId::from_path(&path).unwrap(),
            path,
        }
    }

    fn map_at(maps: &SourceMaps, module: &ResolvedModule, line: u32) -> Option<(String, u32, u32)> {
        (maps.mapper())(module.id.as_str(), line, 3)
    }

    #[test]
    fn inline_data_url_map_is_applied_with_one_based_positions() {
        let dir = tempfile::tempdir().unwrap();
        let module = module(dir.path());
        let code =
            format!("a;\nb;\n//# sourceMappingURL=data:application/json;base64,{MAP_BASE64}\n");
        let maps = SourceMaps::default();
        maps.register(&module, &code, None, &allow(dir.path()));
        assert_eq!(map_at(&maps, &module, 2), Some(("orig.ts".into(), 2, 1)));
    }

    #[test]
    fn transform_map_is_applied_and_wins_over_the_comment() {
        let dir = tempfile::tempdir().unwrap();
        let module = module(dir.path());
        let maps = SourceMaps::default();
        let code = "//# sourceMappingURL=missing.js.map";
        maps.register(&module, code, Some(MAP.into()), &allow(dir.path()));
        assert_eq!(map_at(&maps, &module, 1), Some(("orig.ts".into(), 1, 1)));
    }

    #[test]
    fn file_map_is_read_only_when_the_path_is_authorized() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("bundle.js.map"), MAP).unwrap();
        let module = module(&root);
        let code = "x;\n//# sourceMappingURL=bundle.js.map";

        let denied = SourceMaps::default();
        denied.register(&module, code, None, &CapabilitySet::builder().build());
        assert_eq!(map_at(&denied, &module, 1), None);

        let allowed = SourceMaps::default();
        allowed.register(&module, code, None, &allow(&root));
        assert_eq!(map_at(&allowed, &module, 1), Some(("orig.ts".into(), 1, 1)));
    }

    #[test]
    fn missing_malformed_or_unmapped_input_yields_none() {
        let dir = tempfile::tempdir().unwrap();
        let module = module(dir.path());
        let permissions = allow(dir.path());
        let maps = SourceMaps::default();
        assert_eq!(map_at(&maps, &module, 1), None);
        maps.register(&module, "x", Some("{not json".into()), &permissions);
        assert_eq!(map_at(&maps, &module, 1), None);
        let bad = "//# sourceMappingURL=data:application/json;base64,@@@@";
        maps.register(&module, bad, None, &permissions);
        assert_eq!(map_at(&maps, &module, 1), None);
        maps.register(&module, "x", Some(MAP.into()), &permissions);
        assert_eq!(map_at(&maps, &module, 9), None);
        assert_eq!((maps.mapper())(module.id.as_str(), 0, 0), None);
        assert_eq!((maps.mapper())("file:///other.js", 1, 1), None);
    }
}
