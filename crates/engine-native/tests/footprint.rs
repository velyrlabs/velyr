//! Memory attribution (VELYR-196): the native isolate reports where its memory goes, and
//! the per-item sizes that multiply by every instruction of a large app stay put.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, MemoryBreakdown, Source};
use velyr_engine_native::{NativeEngine, NativeIsolate};
use velyr_engine_vm::bytecode::{Constant, FunctionCode, Instruction};

fn breakdown(iso: &NativeIsolate) -> MemoryBreakdown {
    iso.heap_stats()
        .breakdown
        .expect("native reports a breakdown")
}

#[test]
fn instruction_stays_at_twenty_bytes() {
    assert!(
        size_of::<Instruction>() <= 20,
        "{}",
        size_of::<Instruction>()
    );
}

#[test]
fn compiled_code_and_its_source_are_attributed() {
    let mut iso: NativeIsolate = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let before = breakdown(&iso);
    assert!(before.cells > 0 && before.heap_tables > 0, "{before:?}");
    assert!(before.shapes > 0 && before.interner > 0, "{before:?}");
    assert!(before.tables > 0, "{before:?}");
    let functions: String = (0..200)
        .map(|i| format!("function f{i}(o) {{ return o.a{i} + o.b * {i}; }}\n"))
        .collect();
    let source = Source {
        name: "footprint.js".into(),
        text: functions.clone(),
    };
    iso.eval_script(&source).expect("script runs");
    let after = breakdown(&iso);
    // Each function is a few instructions plus its constants and line table.
    assert!(
        after.code >= before.code + 200 * 100,
        "{before:?} -> {after:?}"
    );
    assert!(
        after.source >= before.source + functions.len(),
        "{before:?} -> {after:?}"
    );
}

/// Bytecode mix of the large-app fixture, for docs/research/large-app-footprint.md:
/// `cargo test --release -p velyr-engine-native --test footprint -- --ignored --nocapture`.
/// Modules compile as scripts (imports dropped, exports unwrapped), close to the real mix.
#[test]
#[ignore]
fn code_mix_of_the_large_app() {
    #[derive(Debug, Default)]
    struct Mix {
        functions: usize,
        instructions: usize,
        constants: usize,
        positions: usize,
        owned_bytes: usize,
    }
    fn walk(code: &FunctionCode, mix: &mut Mix) {
        mix.functions += 1;
        mix.instructions += code.instructions.len();
        mix.constants += code.constants.len();
        mix.positions += code.positions.len();
        mix.owned_bytes += code.owned_bytes();
        for constant in &code.constants {
            if let Constant::Function(inner) = constant {
                walk(inner, mix);
            }
        }
    }
    let root = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../benches/fixtures/large-app/src"
    );
    let mut mix = Mix::default();
    for group in std::fs::read_dir(root).expect("fixture") {
        let Ok(files) = std::fs::read_dir(group.expect("entry").path()) else {
            continue;
        };
        for file in files {
            let text = std::fs::read_to_string(file.expect("entry").path()).expect("module");
            let body: String = text
                .lines()
                .filter(|line| !line.starts_with("import "))
                .map(|line| line.strip_prefix("export default ").unwrap_or(line))
                .map(|line| line.strip_prefix("export ").unwrap_or(line))
                .flat_map(|line| [line, "\n"])
                .collect();
            if let Ok(code) = velyr_engine_native::bytecode_compiler::compile_script(&body) {
                walk(&code, &mut mix);
            }
        }
    }
    println!("{mix:?}");
    assert!(mix.functions > 0);
}

/// Compile time split of the large-app fixture (parse, semantic, emit), for VELYR-201.
#[test]
#[ignore]
fn compile_time_split_of_the_large_app() {
    use std::time::{Duration, Instant};
    let root = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../benches/fixtures/large-app/src"
    );
    let mut bodies = Vec::new();
    for group in std::fs::read_dir(root).expect("fixture") {
        let Ok(files) = std::fs::read_dir(group.expect("entry").path()) else {
            continue;
        };
        for file in files {
            let text = std::fs::read_to_string(file.expect("entry").path()).expect("module");
            let body: String = text
                .lines()
                .filter(|line| !line.starts_with("import "))
                .map(|line| line.strip_prefix("export default ").unwrap_or(line))
                .map(|line| line.strip_prefix("export ").unwrap_or(line))
                .flat_map(|line| [line, "\n"])
                .collect();
            bodies.push(body);
        }
    }
    let (mut parse, mut semantic, mut full) = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    let (mut late, mut stubs) = (Duration::ZERO, 0);
    let mut sizes = [(0usize, 0usize); 6];
    fn deep_bytes(code: &FunctionCode) -> usize {
        let nested = code.constants.iter().map(|constant| match constant {
            Constant::Function(inner) => deep_bytes(&inner.compiled().expect("compiles")),
            _ => 0,
        });
        code.owned_bytes() + nested.sum::<usize>()
    }
    for body in &bodies {
        let start = Instant::now();
        let allocator = oxc_allocator::Allocator::default();
        let parsed =
            oxc_parser::Parser::new(&allocator, body, oxc_span::SourceType::script()).parse();
        parse += start.elapsed();
        let start = Instant::now();
        let built = oxc_semantic::SemanticBuilder::new()
            .with_build_nodes(true)
            .with_check_syntax_error(true)
            .build(&parsed.program);
        semantic += start.elapsed();
        drop(built);
        let start = Instant::now();
        let code = velyr_engine_native::bytecode_compiler::compile_script(body);
        full += start.elapsed();
        // Every top-level function called once: one lazy compile each.
        for constant in code.iter().flat_map(|code| &code.constants) {
            if let Constant::Function(stub) = constant
                && stub.lazy.is_some()
            {
                let start = Instant::now();
                let compiled = stub.compiled().expect("compiles");
                late += start.elapsed();
                stubs += 1;
                let len = (stub.source_end - stub.source_start) as usize;
                let bucket = [64, 128, 256, 512, 1024, usize::MAX]
                    .iter()
                    .position(|&limit| len < limit)
                    .unwrap_or(5);
                sizes[bucket].0 += 1;
                sizes[bucket].1 += deep_bytes(&compiled);
            }
        }
    }
    println!(
        "modules={} parse={parse:?} semantic={semantic:?} load={full:?} lazy={stubs} in {late:?}",
        bodies.len()
    );
    println!("text <64/128/256/512/1024/more: (functions, code bytes) {sizes:?}");
}
