//! BigInt on engine-native (VELYR-113): operators, mixed comparisons with Number, the
//! `BigInt` global and the size limit tied to the heap budget. Expectations follow V8.

use std::time::{Duration, Instant};

use velyr_engine_api::{Engine, Isolate, IsolateOptions, JsString, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate_with(heap_limit_bytes: Option<usize>) -> NativeIsolate {
    let options = IsolateOptions { heap_limit_bytes };
    NativeEngine.create_isolate(options).expect("isolate")
}

/// `String(completion)` of `code`, or `throw <message>`.
fn run(iso: &mut NativeIsolate, code: &str) -> String {
    let source = Source {
        name: "bigint.js".into(),
        text: format!("String((() => {{ return {code}; }})())"),
    };
    match iso.eval_script(&source) {
        Ok(Value::String(text)) => text.to_string_lossy(),
        Ok(other) => format!("non-string {other:?}"),
        Err(error) => format!("throw {}", error.message),
    }
}

fn expect(cases: &[(&str, &str)]) {
    let mut iso = isolate_with(None);
    for (code, expected) in cases {
        assert_eq!(run(&mut iso, code), *expected, "{code}");
    }
}

/// The error name each expression throws.
fn expect_errors(heap_limit_bytes: Option<usize>, cases: &[(&str, &str)]) {
    let mut iso = isolate_with(heap_limit_bytes);
    for (code, expected) in cases {
        let probe = format!(
            "(() => {{ try {{ {code}; return 'no error'; }} catch (e) {{ return e.name; }} }})()"
        );
        assert_eq!(run(&mut iso, &probe), *expected, "{code}");
    }
}

#[test]
fn arithmetic_and_bitwise_operators_follow_the_spec() {
    expect(&[
        (
            "[1n + 2n, 10n - 20n, 6n * 7n, 7n / 2n, -7n / 2n, 7n % -2n, -7n % 2n, 2n ** 64n].join()",
            "3,-10,42,3,-3,1,-1,18446744073709551616",
        ),
        (
            "[-(5n), ~5n, 5n & 3n, -5n & 0xffn, 5n | -8n, 5n ^ -1n, 1n << 70n, -9n >> 1n, 255n >> 4n, 1n << -1n].join()",
            "-5,-6,1,251,-3,-6,1180591620717411303424,-5,15,0",
        ),
        (
            "(() => { let a = 1n; a++; ++a; const b = a--; return [a, b, typeof a, -a]; })().join()",
            "2,3,bigint,-2",
        ),
        ("1n + 'x' + 2n", "1x2"),
        ("`${-12n}`", "-12"),
    ]);
}

#[test]
fn mixing_bigint_with_number_throws_and_bad_operands_are_range_errors() {
    expect_errors(
        None,
        &[
            ("1n + 1", "TypeError"),
            ("2 * 1n", "TypeError"),
            ("+1n", "TypeError"),
            ("1n >>> 0n", "TypeError"),
            ("Math.max(1n)", "TypeError"),
            ("1n / 0n", "RangeError"),
            ("1n % 0n", "RangeError"),
            ("2n ** -1n", "RangeError"),
        ],
    );
}

#[test]
fn comparisons_mix_bigint_number_and_string_exactly() {
    expect(&[(
        "[1n < 2, 2n > 1.5, 1n == 1, 1n === 1, 2n > '1', 1n < 'x', 10n ** 30n > 1e30,
          10n ** 30n < Infinity, 0n < NaN, -1n < -0.5, 3n >= 3n, 2 <= 1n].join()",
        "true,true,true,false,true,false,false,true,false,true,true,false",
    )]);
}

#[test]
fn the_bigint_global_converts_and_wraps() {
    expect(&[
        (
            "[BigInt(10), BigInt('0x1f'), BigInt(' 12 '), BigInt(true), BigInt(-0), BigInt(2 ** 60)].join()",
            "10,31,12,1,0,1152921504606846976",
        ),
        (
            "[BigInt.asIntN(8, 255n), BigInt.asUintN(8, -1n), BigInt.asIntN(64, 2n ** 63n),
              BigInt.asUintN(64, -1n), BigInt.asIntN(0, 5n), BigInt.asUintN(2 ** 53 - 1, 7n)].join()",
            "-1,255,-9223372036854775808,18446744073709551615,0,7",
        ),
        (
            "[(255n).toString(16), (-255n).toString(2), (35n).toString(36), String(10n ** 20n)].join()",
            "ff,-11111111,z,100000000000000000000",
        ),
        (
            "[BigInt.prototype.valueOf.call(Object(3n)), typeof Object(1n), Object(2n) + 1n,
              Object.getPrototypeOf(1n) === BigInt.prototype, BigInt.prototype.constructor === BigInt,
              BigInt.name, BigInt.length, Object.prototype.toString.call(1n)].join()",
            "3,object,3,true,true,BigInt,1,[object BigInt]",
        ),
        ("(12345n).toLocaleString('en-US')", "12,345"),
    ]);
}

#[test]
fn the_bigint_global_rejects_what_the_spec_rejects() {
    expect_errors(
        None,
        &[
            ("new BigInt(1)", "TypeError"),
            ("BigInt(1.5)", "RangeError"),
            ("BigInt(NaN)", "RangeError"),
            ("BigInt('1.5')", "SyntaxError"),
            ("BigInt(undefined)", "TypeError"),
            ("BigInt(Symbol())", "TypeError"),
            ("(1n).toString(1)", "RangeError"),
            ("BigInt.prototype.toString.call(1)", "TypeError"),
            ("BigInt.asUintN(-1, 1n)", "RangeError"),
        ],
    );
}

#[test]
fn giant_bigints_are_range_errors_before_allocating() {
    let started = Instant::now();
    // 1 MiB of heap budget allows 8 Mibit; 2n ** 10000000n needs 10 Mbit.
    expect_errors(
        Some(1 << 20),
        &[
            ("2n ** 10000000n", "RangeError"),
            ("1n << 10000000n", "RangeError"),
            ("BigInt.asUintN(10000000, -1n)", "RangeError"),
            ("2n ** 1000n", "no error"),
        ],
    );
    expect_errors(
        None,
        &[
            ("2n ** (2n ** 40n)", "RangeError"),
            ("(1n << (1n << 31n))", "RangeError"),
            ("(2n ** 4096n) * (2n ** 4096n)", "no error"),
        ],
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn string_results_survive_the_engine_boundary() {
    let mut iso = isolate_with(None);
    let source = Source {
        name: "bigint.js".into(),
        text: "(2n ** 64n).toString()".into(),
    };
    let value = iso.eval_script(&source).map_err(|e| e.message);
    assert_eq!(
        value,
        Ok(Value::String(JsString::from("18446744073709551616")))
    );
}
