//! `oxc_transformer::TransformOptions` derived per file from `tsconfig.json`.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use oxc_transformer::{
    DecoratorOptions, ES2026Options, EnvOptions, HelperLoaderMode, HelperLoaderOptions, Module,
    TransformOptions, TypeScriptOptions,
};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::{HELPER_MODULE_NAME, MediaType};
use velyr_tooling_tsconfig::TsConfig;

/// Syntax the engine cannot run and the transform must lower.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lowering {
    /// `using` / `await using`, through the `usingCtx` helper.
    pub explicit_resource_management: bool,
}

/// Types are stripped, nothing is downleveled (the engine runs modern JavaScript).
/// `.cts` is CommonJS (`export =` becomes `module.exports`); everything else is ESM, where oxc
/// reports `export =` and `import x = require()` as diagnostics. `config` of `None` is defaults.
pub fn transform_options(media_type: MediaType, config: Option<&TsConfig>) -> TransformOptions {
    lowered_options(media_type, config, Lowering::default())
}

/// [`transform_options`] plus the syntax in `lowering`.
pub fn lowered_options(
    media_type: MediaType,
    config: Option<&TsConfig>,
    lowering: Lowering,
) -> TransformOptions {
    let module = if media_type == MediaType::Cts {
        Module::CommonJS
    } else {
        Module::Esm
    };
    let mut options = TransformOptions {
        env: EnvOptions {
            module,
            es2026: ES2026Options {
                explicit_resource_management: lowering.explicit_resource_management,
            },
            ..EnvOptions::default()
        },
        helper_loader: HelperLoaderOptions {
            module_name: Cow::Borrowed(HELPER_MODULE_NAME),
            mode: HelperLoaderMode::Runtime,
        },
        ..TransformOptions::default()
    };
    let Some(config) = config else {
        return options;
    };
    let compiler = &config.options;
    let legacy = compiler.experimental_decorators == Some(true);
    options.decorator = DecoratorOptions {
        legacy,
        emit_decorator_metadata: legacy && compiler.emit_decorator_metadata == Some(true),
        strict_null_checks: compiler.strict_null_checks(),
    };
    if !compiler.define_class_fields() {
        options.typescript.remove_class_fields_without_initializer = true;
        options.assumptions.set_public_class_fields = true;
    }
    if compiler.verbatim_module_syntax == Some(true) {
        options.typescript = TypeScriptOptions {
            only_remove_type_imports: true,
            ..options.typescript
        };
    }
    options
}

/// Passes a found config through. A broken tsconfig yields `None` (defaults apply, D8) and is
/// reported on stderr once per distinct error, which names the tsconfig file.
pub fn warn_once(
    result: Result<Option<Arc<TsConfig>>, VelyrError>,
    specifier: &str,
) -> Option<Arc<TsConfig>> {
    match result {
        Ok(config) => config,
        Err(error) => {
            let message = error.to_string();
            if first_time(&message) {
                eprintln!("velyr: ignoring tsconfig for {specifier}: {message}");
            }
            None
        }
    }
}

fn first_time(key: &str) -> bool {
    static SEEN: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SEEN.get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_error_is_reported_once() {
        let error = || VelyrError::invalid_data("/x/tsconfig.json: broken once");
        let key = error().to_string();
        let error = || Err(error());
        assert!(warn_once(error(), "a.ts").is_none());
        assert!(warn_once(error(), "b.ts").is_none());
        assert!(!first_time(&key));
        assert!(first_time("/y/tsconfig.json: another"));
    }
}
