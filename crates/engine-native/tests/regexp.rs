//! RegExp built-in on the bytecode VM (VELYR-112): `regress` matching, lastIndex/sticky,
//! the `d` indices, the symbol methods behind the String methods and `RegExp.escape`.

use velyr_engine_native::bytecode_compiler::compile_script;
use velyr_engine_vm::interpreter::{Abrupt, Interpreter};

/// `String(completion)` on the bytecode path only (no tree-interpreter fallback).
fn run(source: &str) -> String {
    let code = match compile_script(source) {
        Ok(code) => code,
        Err(error) => return format!("compile error: {}", error.message),
    };
    let mut vm = Interpreter::new();
    match vm.run_script(&code) {
        Ok(value) => match vm.to_js_string(&value) {
            Ok(text) => text.to_string_lossy(),
            Err(_) => "<unprintable completion>".into(),
        },
        Err(Abrupt::Throw(value)) => format!("throw {}", vm.exception_message(&value)),
        Err(abrupt) => format!("{abrupt:?}"),
    }
}

#[test]
fn exec_returns_captures_index_and_named_groups() {
    let source = "const m = /(?<y>\\d{4})-(\\d{2})/.exec('on 2024-05!');
        [m[0], m[1], m[2], m.index, m.input, m.groups.y, m.length, /z/.exec('a')].join('|')";
    assert_eq!(run(source), "2024-05|2024|05|3|on 2024-05!|2024|3|");
}

#[test]
fn global_and_sticky_regexps_track_last_index() {
    let source = "const g = /a/g; g.exec('aa'); const i1 = g.lastIndex; g.exec('aa');
        const i2 = g.lastIndex; const n = g.exec('aa');
        const y = /b/y; y.lastIndex = 1;
        [i1, i2, n, g.lastIndex, y.test('ab'), y.lastIndex, y.test('ab'), y.lastIndex].join()";
    assert_eq!(run(source), "1,2,,0,true,2,false,0");
}

#[test]
fn has_indices_flag_reports_match_ranges() {
    let source = "const m = /(?<w>b+)(x)?/d.exec('abbc');
        [m.indices[0], m.indices[1], m.indices[2], m.indices.groups.w, m.hasIndices].join('|')";
    assert_eq!(run(source), "1,3|1,3||1,3|");
}

#[test]
fn string_methods_delegate_to_the_regexp_symbols() {
    let source = "['a1b22c'.replace(/\\d+/g, '[$&]'), 'x-y_z'.split(/[-_]/), 'abc'.search(/c/),
         'a1b2'.match(/\\d/g), [...'a1b2'.matchAll(/(\\d)/g)].map(m => m[1] + m.index),
         'John Smith'.replace(/(?<f>\\w+) (?<l>\\w+)/, '$<l>, $1'),
         'aXbX'.replace(/x/gi, (m, i) => i), 'a,b'.split(/(,)/), 'ab'.match('b').index,
         'aaa'.replaceAll(/a/g, 'b')].join('|')";
    assert_eq!(
        run(source),
        "a[1]b[22]c|x,y,z|2|1,2|11,23|Smith, John|a1b3|a,,,b|1|bbb"
    );
}

#[test]
fn constructor_reports_source_flags_and_brand() {
    let source = "const r = new RegExp('a/b\\n', 'gimsuyd');
        [r.source, r.flags, String(/x/v), RegExp.prototype.source, RegExp(r) === r,
         new RegExp(r, 'i').flags, Object.prototype.toString.call(r), r.lastIndex,
         RegExp.prototype.global, new RegExp().source].join('|')";
    assert_eq!(
        run(source),
        "a\\/b\\n|dgimsuy|/x/v|(?:)|true|i|[object RegExp]|0||(?:)"
    );
}

#[test]
fn invalid_patterns_and_flags_are_syntax_errors() {
    let source = "const names = [];
        for (const [p, f] of [['(', ''], ['a', 'gg'], ['a', 'uv'], ['a', 'x']]) {
            try { new RegExp(p, f); names.push('ok'); } catch (e) { names.push(e.name); }
        }
        names.join()";
    assert_eq!(
        run(source),
        "SyntaxError,SyntaxError,SyntaxError,SyntaxError"
    );
    assert!(run("/(/").starts_with("compile error: SyntaxError: "));
    assert!(run("/a/gg").starts_with("compile error: SyntaxError: "));
}

#[test]
fn unicode_and_unicode_sets_modes_match_code_points() {
    let source = "[/^.$/u.test('\\u{1F600}'), /^.$/.test('\\u{1F600}'),
         '\\u{1F600}'.replace(/(?:)/gu, '-').length, /[\\p{L}--[a-z]]/v.test('A'),
         /[\\p{L}--[a-z]]/v.test('a'), /\\u{61}/u.test('a')].join()";
    assert_eq!(run(source), "true,false,4,true,false,true");
}

#[test]
fn regexp_escape_encodes_syntax_and_leading_alphanumerics() {
    assert_eq!(run("RegExp.escape('1a.b c/')"), "\\x31a\\.b\\x20c\\/");
    assert_eq!(
        run("try { RegExp.escape(1); } catch (e) { e.name }"),
        "TypeError"
    );
}

#[test]
fn sticky_regexps_keep_their_semantics_at_last_index() {
    let source = "const at = (re, text, index) => { re.lastIndex = index; const m = re.exec(text);
            return m ? m.join('/') + '@' + m.index + '>' + re.lastIndex : 'null>' + re.lastIndex; };
        [at(/(?<=a)b/y, 'ab', 1), at(/^b/y, 'ab', 1), at(/^b/my, 'a\\nb', 2), at(/\\bb/y, 'ab', 1),
         at(/(x)|(b)/y, 'ab', 1), at(/a|ab/y, 'ab', 0), at(/(a)\\1/y, 'xaa', 1),
         at(/\\1/y, 'x\\x01', 1), at(/b/y, 'abb', 0), at(/(?:)/y, 'ab', 2),
         at(/(?<n>b)/y, 'ab', 1)].join('|')";
    assert_eq!(
        run(source),
        "b@1>2|null>0|b@2>3|null>0|b//b@1>2|a@0>1|aa/a@1>3|\u{1}@1>2|null>0|@2>2|b/b@1>2"
    );
}

#[test]
fn split_with_a_sticky_splitter_tries_each_position_once() {
    // Each split step is a sticky exec at q; a forward search from q makes this O(n^2).
    let started = std::time::Instant::now();
    assert_eq!(run("'a'.repeat(50000).split(/b/).length"), "1");
    let elapsed = started.elapsed();
    assert!(elapsed.as_secs() < 3, "split took {elapsed:?}");
}

#[test]
fn terminate_stops_a_catastrophic_backtracking_regexp() {
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    // The interpreter is not Send: it lives on its own thread, which is left spinning if
    // the regexp ignores the flag (the test then fails instead of hanging).
    let (flag_tx, flag_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let code = compile_script("/(a+)+$/.test('a'.repeat(40) + 'b')").unwrap();
        let mut vm = Interpreter::new();
        flag_tx.send(vm.termination_flag()).unwrap();
        let completion = vm.run_script(&code);
        done_tx
            .send(matches!(completion, Err(Abrupt::Terminated)))
            .unwrap();
    });
    let flag = flag_rx.recv().unwrap();
    std::thread::sleep(Duration::from_millis(20));
    flag.store(true, Ordering::Relaxed);
    let raised = Instant::now();
    let terminated = done_rx.recv_timeout(Duration::from_millis(100));
    assert_eq!(
        terminated,
        Ok(true),
        "after terminate(): {:?}",
        raised.elapsed()
    );
}
