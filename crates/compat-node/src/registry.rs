//! The `node:` builtin registry: which modules exist, the JS bootstrap that defines them, the
//! ESM facade each one gets, and the resolver that maps `node:x` / `x` onto those facades.
//!
//! The module host reads every module from a file and authorizes its path, so an ESM `import` of
//! a builtin resolves to a generated facade file inside a directory the host chooses and grants
//! (`ImportLocal` on that directory only). The facade just re-exports the isolate's builtin.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::{ModuleId, ModuleResolver, ResolveContext, ResolvedModule, Result};

/// A public builtin and its named exports (everything except `default`).
pub struct Builtin {
    pub name: &'static str,
    pub exports: &'static [&'static str],
}

const PATH_EXPORTS: &[&str] = &[
    "basename",
    "delimiter",
    "dirname",
    "extname",
    "format",
    "isAbsolute",
    "join",
    "normalize",
    "parse",
    "posix",
    "relative",
    "resolve",
    "sep",
    "toNamespacedPath",
    "win32",
];
const ASSERT_EXPORTS: &[&str] = &[
    "AssertionError",
    "deepEqual",
    "deepStrictEqual",
    "doesNotMatch",
    "doesNotReject",
    "doesNotThrow",
    "equal",
    "fail",
    "ifError",
    "match",
    "notDeepEqual",
    "notDeepStrictEqual",
    "notEqual",
    "notStrictEqual",
    "ok",
    "rejects",
    "strict",
    "strictEqual",
    "throws",
];
const TYPES_EXPORTS: &[&str] = &[
    "isAnyArrayBuffer",
    "isArgumentsObject",
    "isArrayBuffer",
    "isAsyncFunction",
    "isBigInt64Array",
    "isBigIntObject",
    "isBigUint64Array",
    "isBooleanObject",
    "isBoxedPrimitive",
    "isDataView",
    "isDate",
    "isExternal",
    "isFloat32Array",
    "isFloat64Array",
    "isGeneratorFunction",
    "isGeneratorObject",
    "isInt16Array",
    "isInt32Array",
    "isInt8Array",
    "isMap",
    "isMapIterator",
    "isModuleNamespaceObject",
    "isNativeError",
    "isNumberObject",
    "isPromise",
    "isProxy",
    "isRegExp",
    "isSet",
    "isSetIterator",
    "isSharedArrayBuffer",
    "isStringObject",
    "isSymbolObject",
    "isTypedArray",
    "isUint16Array",
    "isUint32Array",
    "isUint8Array",
    "isUint8ClampedArray",
    "isWeakMap",
    "isWeakSet",
];

const PROCESS_EXPORTS: &[&str] = &[
    "_events",
    "_eventsCount",
    "_maxListeners",
    "arch",
    "argv",
    "argv0",
    "chdir",
    "cwd",
    "emitWarning",
    "env",
    "execArgv",
    "execPath",
    "exit",
    "exitCode",
    "hrtime",
    "memoryUsage",
    "nextTick",
    "noDeprecation",
    "pid",
    "platform",
    "release",
    "stderr",
    "stdin",
    "stdout",
    "title",
    "uptime",
    "version",
    "versions",
];

pub const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "path",
        exports: PATH_EXPORTS,
    },
    Builtin {
        name: "path/posix",
        exports: PATH_EXPORTS,
    },
    Builtin {
        name: "path/win32",
        exports: PATH_EXPORTS,
    },
    Builtin {
        name: "url",
        exports: &[
            "URL",
            "URLSearchParams",
            "Url",
            "fileURLToPath",
            "format",
            "parse",
            "pathToFileURL",
            "resolve",
            "urlToHttpOptions",
        ],
    },
    Builtin {
        name: "events",
        exports: &[
            "EventEmitter",
            "addAbortListener",
            "captureRejectionSymbol",
            "captureRejections",
            "defaultMaxListeners",
            "errorMonitor",
            "getEventListeners",
            "getMaxListeners",
            "init",
            "listenerCount",
            "on",
            "once",
            "setMaxListeners",
        ],
    },
    Builtin {
        name: "assert",
        exports: ASSERT_EXPORTS,
    },
    Builtin {
        name: "assert/strict",
        exports: ASSERT_EXPORTS,
    },
    Builtin {
        name: "util",
        exports: &[
            "TextDecoder",
            "TextEncoder",
            "callbackify",
            "debug",
            "debuglog",
            "deprecate",
            "format",
            "formatWithOptions",
            "inherits",
            "inspect",
            "isArray",
            "isDeepStrictEqual",
            "parseArgs",
            "promisify",
            "stripVTControlCharacters",
            "styleText",
            "toUSVString",
            "types",
        ],
    },
    Builtin {
        name: "util/types",
        exports: TYPES_EXPORTS,
    },
    Builtin {
        name: "buffer",
        exports: &[
            "Blob",
            "Buffer",
            "File",
            "INSPECT_MAX_BYTES",
            "atob",
            "btoa",
            "constants",
            "kMaxLength",
            "kStringMaxLength",
        ],
    },
    Builtin {
        name: "string_decoder",
        exports: &["StringDecoder"],
    },
    Builtin {
        name: "process",
        exports: PROCESS_EXPORTS,
    },
    Builtin {
        name: "stream",
        exports: &[
            "Duplex",
            "PassThrough",
            "Readable",
            "Stream",
            "Transform",
            "Writable",
            "_isUint8Array",
            "_uint8ArrayToBuffer",
            "addAbortSignal",
            "compose",
            "destroy",
            "finished",
            "getDefaultHighWaterMark",
            "isDestroyed",
            "isDisturbed",
            "isErrored",
            "isReadable",
            "isWritable",
            "pipeline",
            "promises",
            "setDefaultHighWaterMark",
        ],
    },
    Builtin {
        name: "stream/promises",
        exports: &["finished", "pipeline"],
    },
    Builtin {
        name: "stream/web",
        exports: &[
            "ByteLengthQueuingStrategy",
            "CompressionStream",
            "CountQueuingStrategy",
            "DecompressionStream",
            "ReadableByteStreamController",
            "ReadableStream",
            "ReadableStreamBYOBReader",
            "ReadableStreamBYOBRequest",
            "ReadableStreamDefaultController",
            "ReadableStreamDefaultReader",
            "TextDecoderStream",
            "TextEncoderStream",
            "TransformStream",
            "TransformStreamDefaultController",
            "WritableStream",
            "WritableStreamDefaultController",
            "WritableStreamDefaultWriter",
        ],
    },
    Builtin {
        name: "crypto",
        exports: crate::crypto_js::CRYPTO_EXPORTS,
    },
    Builtin {
        name: "fs",
        exports: crate::fs_js::FS_EXPORTS,
    },
    Builtin {
        name: "fs/promises",
        exports: crate::fs_js::FS_PROMISES_EXPORTS,
    },
    Builtin {
        name: "timers",
        exports: &[
            "clearImmediate",
            "clearInterval",
            "clearTimeout",
            "setImmediate",
            "setInterval",
            "setTimeout",
        ],
    },
];

/// Factories, in the `(module, exports, require)` shape the bootstrap wraps them in.
const FACTORIES: &[(&str, &str)] = &[
    ("internal/errors", include_str!("errors.js")),
    ("internal/types", include_str!("types.js")),
    ("internal/inspect_layout", include_str!("inspect_layout.js")),
    ("internal/inspect", include_str!("inspect.js")),
    ("internal/encodings", include_str!("encodings.js")),
    ("internal/buffer_rw", include_str!("buffer_rw.js")),
    ("buffer", include_str!("buffer.js")),
    ("string_decoder", include_str!("string_decoder.js")),
    ("internal/comparisons", include_str!("comparisons.js")),
    ("internal/parse_args", include_str!("parse_args.js")),
    ("path", include_str!("path.js")),
    ("path/posix", "module.exports = require('path').posix;"),
    ("path/win32", "module.exports = require('path').win32;"),
    ("url", include_str!("url.js")),
    ("events", include_str!("events.js")),
    ("assert", include_str!("assert.js")),
    (
        "assert/strict",
        "module.exports = require('assert').strict;",
    ),
    ("util", include_str!("util.js")),
    ("util/types", "module.exports = require('util').types;"),
    ("process", include_str!("process.js")),
    ("timers", include_str!("timers_immediate.js")),
];

const REGISTRY_JS: &str = include_str!("registry.js");

/// The script that defines every builtin lazily and publishes them on
/// `globalThis[Symbol.for("velyr.node")]` (and into `require`'s builtin table, if present).
pub fn bootstrap_source() -> String {
    let mut factories = String::new();
    let all = FACTORIES
        .iter()
        .chain(crate::streams::STREAM_FACTORIES)
        .chain(crate::crypto_js::CRYPTO_FACTORIES)
        .chain(crate::fs_js::FS_FACTORIES);
    for (name, body) in all {
        factories.push_str(&format!(
            "{name:?}: function (module, exports, require) {{\n{body}\n}},\n"
        ));
    }
    let public: Vec<String> = BUILTINS.iter().map(|b| format!("{:?}", b.name)).collect();
    REGISTRY_JS
        .replace("/*FACTORIES*/", &factories)
        .replace("/*PUBLIC*/", &public.join(", "))
}

/// `Some(canonical name)` when `specifier` names a builtin, with or without `node:`.
pub fn builtin_name(specifier: &str) -> Option<&'static str> {
    let bare = specifier.strip_prefix("node:").unwrap_or(specifier);
    BUILTINS.iter().find(|b| b.name == bare).map(|b| b.name)
}

fn facade_file(name: &str) -> String {
    format!("{}.mjs", name.replace('/', "__"))
}

pub fn facade_source(builtin: &Builtin) -> String {
    let mut code = format!(
        "const m = globalThis[Symbol.for(\"velyr.node\")].require({:?});\nexport default m;\n",
        builtin.name
    );
    for export in builtin.exports {
        code.push_str(&format!("export const {export} = m.{export};\n"));
    }
    code
}

/// Resolves `node:x` and bare `x` builtins to facade modules; everything else goes to `inner`.
#[derive(Clone)]
pub struct NodeBuiltinResolver {
    inner: Arc<dyn ModuleResolver + Send + Sync>,
    dir: PathBuf,
}

impl NodeBuiltinResolver {
    /// Writes the facade files into `dir` (created if needed; existing facades are overwritten).
    /// The host must grant `ImportLocal` on [`facade_dir`](Self::facade_dir) and nothing wider,
    /// and `dir` must not be writable by other users.
    pub fn new(inner: Arc<dyn ModuleResolver + Send + Sync>, dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let dir = fs::canonicalize(dir)?;
        for builtin in BUILTINS {
            let target = dir.join(facade_file(builtin.name));
            let staging = dir.join(format!(
                ".{}.{}",
                facade_file(builtin.name),
                std::process::id()
            ));
            fs::write(&staging, facade_source(builtin))?;
            fs::rename(&staging, &target)?;
        }
        Ok(Self { inner, dir })
    }

    pub fn facade_dir(&self) -> &Path {
        &self.dir
    }
}

impl ModuleResolver for NodeBuiltinResolver {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &ModuleId,
        ctx: &ResolveContext,
    ) -> Result<ResolvedModule> {
        let Some(name) = builtin_name(specifier) else {
            return self.inner.resolve(specifier, referrer, ctx);
        };
        let path = self.dir.join(facade_file(name));
        let id = ModuleId::from_path(&path)
            .ok_or_else(|| VelyrError::invalid_data("facade path is not representable as a URL"))?;
        Ok(ResolvedModule { id, path })
    }
}
