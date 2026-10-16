//! ArrayBuffer, TypedArrays, DataView, SharedArrayBuffer and Atomics on engine-native
//! (VELYR-111), plus the BigInt follow-ups from VELYR-113. Expectations follow V8.

use std::time::{Duration, Instant};

use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_native::{NativeEngine, NativeIsolate};

fn isolate_with(heap_limit_bytes: Option<usize>) -> NativeIsolate {
    let options = IsolateOptions { heap_limit_bytes };
    NativeEngine.create_isolate(options).expect("isolate")
}

/// `String(completion)` of `code`, or `throw <message>`.
fn run(iso: &mut NativeIsolate, code: &str) -> String {
    let source = Source {
        name: "typed_arrays.js".into(),
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

/// The error name each statement throws.
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
fn array_buffer_basics() {
    expect(&[
        ("new ArrayBuffer(8).byteLength", "8"),
        ("new ArrayBuffer().byteLength", "0"),
        (
            "Object.prototype.toString.call(new ArrayBuffer(1))",
            "[object ArrayBuffer]",
        ),
        (
            "(() => { const b = new ArrayBuffer(8); new Uint8Array(b).set([1,2,3,4,5]); return [...new Uint8Array(b.slice(1, -3))]; })()",
            "2,3,4,5",
        ),
        (
            "[ArrayBuffer.isView(new Uint8Array(1)), ArrayBuffer.isView(new DataView(new ArrayBuffer(1))), ArrayBuffer.isView([])].join()",
            "true,true,false",
        ),
        ("ArrayBuffer[Symbol.species] === ArrayBuffer", "true"),
    ]);
    expect_errors(
        None,
        &[
            ("ArrayBuffer(8)", "TypeError"),
            ("new ArrayBuffer(-1)", "RangeError"),
            ("ArrayBuffer.prototype.slice.call({}, 0)", "TypeError"),
        ],
    );
}

#[test]
fn array_buffer_backing_stores_count_against_the_heap_budget() {
    expect_errors(
        Some(1 << 20),
        &[
            ("new ArrayBuffer(2 ** 21)", "RangeError"),
            ("new Uint8Array(2 ** 21)", "RangeError"),
            ("globalThis.kept = new ArrayBuffer(600000)", "no error"),
            ("new ArrayBuffer(600000)", "RangeError"),
            ("new ArrayBuffer(1000)", "no error"),
        ],
    );
}

#[test]
fn typed_arrays_convert_elements_per_kind() {
    expect(&[
        (
            "[...new Int8Array([127, 128, -129, 1.9])].join()",
            "127,-128,127,1",
        ),
        ("[...new Uint8Array([256, -1, 1.5])].join()", "0,255,1"),
        (
            "[...new Uint8ClampedArray([300, -5, 1.5, 2.5, NaN])].join()",
            "255,0,2,2,0",
        ),
        (
            "[...new Int16Array([32768, -32769])].join()",
            "-32768,32767",
        ),
        ("[...new Uint16Array([65536, -1])].join()", "0,65535"),
        (
            "[...new Int32Array([2 ** 31, 2 ** 32 + 5])].join()",
            "-2147483648,5",
        ),
        ("[...new Uint32Array([-1])].join()", "4294967295"),
        (
            "[...new Float32Array([0.1, 1e40])].join()",
            "0.10000000149011612,Infinity",
        ),
        (
            "[...new Float64Array([0.1, -0])].map(x => Object.is(x, -0) ? '-0' : x).join()",
            "0.1,-0",
        ),
        (
            "[...new Float16Array([0.1, 65520, 1e-8, 65504, 2049])].join()",
            "0.0999755859375,Infinity,0,65504,2048",
        ),
        (
            "[...new BigInt64Array([2n ** 63n, -1n])].join()",
            "-9223372036854775808,-1",
        ),
        (
            "[...new BigUint64Array([-1n, 2n ** 64n + 3n])].join()",
            "18446744073709551615,3",
        ),
        (
            "[Int8Array.BYTES_PER_ELEMENT, Float16Array.BYTES_PER_ELEMENT, BigInt64Array.prototype.BYTES_PER_ELEMENT].join()",
            "1,2,8",
        ),
    ]);
    expect_errors(
        None,
        &[
            ("new Int8Array([1n])", "TypeError"),
            ("new BigInt64Array([1])", "TypeError"),
            ("Int8Array(2)", "TypeError"),
            ("new Int32Array(new ArrayBuffer(8), 1)", "RangeError"),
            ("new Int32Array(new ArrayBuffer(6))", "RangeError"),
            ("new Int32Array(new ArrayBuffer(8), 4, 2)", "RangeError"),
            ("Object.getPrototypeOf(Int8Array)()", "TypeError"),
        ],
    );
}

#[test]
fn typed_arrays_are_integer_indexed_exotic_objects() {
    expect(&[
        (
            "(() => { const a = new Uint8Array(2); a[0] = 7; a[5] = 9; a['-0'] = 1; a.foo = 1; return [a[0], a[5], a.length, a['-0'], a.foo, 5 in a, '1' in a, '1.5' in a]; })().join()",
            "7,,2,,1,false,true,false",
        ),
        ("Object.keys(new Int16Array(3)).join()", "0,1,2"),
        (
            "JSON.stringify(new Uint8Array([1, 2]))",
            "{\"0\":1,\"1\":2}",
        ),
        (
            "(() => { const a = new Uint8Array(1); return delete a[0]; })()",
            "false",
        ),
        (
            "(() => { const a = new Uint8Array(1); return delete a[3]; })()",
            "true",
        ),
        (
            "JSON.stringify(Object.getOwnPropertyDescriptor(new Uint8Array([4]), 0))",
            "{\"value\":4,\"writable\":true,\"enumerable\":true,\"configurable\":true}",
        ),
        (
            "Reflect.defineProperty(new Uint8Array(1), 0, { value: 1, configurable: false })",
            "false",
        ),
        (
            "(() => { const a = new Uint8Array(1); Object.defineProperty(a, 0, { value: 3 }); return a[0]; })()",
            "3",
        ),
        (
            "Object.prototype.toString.call(new Float64Array(1))",
            "[object Float64Array]",
        ),
        (
            "Object.getPrototypeOf(Int8Array.prototype) === Object.getPrototypeOf(Uint8Array.prototype)",
            "true",
        ),
        ("new Uint8Array(4)[Symbol.toStringTag]", "Uint8Array"),
        (
            "Object.getPrototypeOf(Int8Array.prototype)[Symbol.toStringTag]",
            "undefined",
        ),
    ]);
}

#[test]
fn typed_array_constructors_and_statics() {
    expect(&[
        ("new Uint8Array(new Set([1, 2, 3])).join()", "1,2,3"),
        ("new Uint8Array({ length: 2, 0: 5, 1: 6 }).join()", "5,6"),
        ("new Float64Array(new Int8Array([-1, 2])).join()", "-1,2"),
        (
            "(() => { const b = new ArrayBuffer(8); const a = new Uint16Array(b, 2, 2); a[0] = 258; return [a.byteOffset, a.byteLength, a.length, new Uint8Array(b)[2], a.buffer === b]; })().join()",
            "2,4,2,2,true",
        ),
        (
            "(() => { const b = new ArrayBuffer(8); return new Uint8Array(b, 3).length; })()",
            "5",
        ),
        ("Uint8Array.from([1, 2], x => x * 2).join()", "2,4"),
        ("Int16Array.of(1, -2, 3).join()", "1,-2,3"),
        ("Uint8Array.from({ length: 2, 0: 9 }).join()", "9,0"),
    ]);
}

#[test]
fn typed_array_prototype_methods() {
    expect(&[
        (
            "(() => { const a = new Uint8Array(5); a.set([1, 2], 1); a.set(new Uint8Array([9]), 4); return a.join(); })()",
            "0,1,2,0,9",
        ),
        (
            "(() => { const a = new Uint8Array([1, 2, 3, 4]); const s = a.subarray(1, 3); s[0] = 9; return [a.join(), s.length, s.byteOffset]; })().join(';')",
            "1,9,3,4;2;1",
        ),
        (
            "(() => { const a = new Uint8Array([1, 2, 3]); const s = a.slice(1); s[0] = 9; return [a.join(), s.join()]; })().join(';')",
            "1,2,3;9,3",
        ),
        ("new Int8Array([3, -1, 20, 2]).sort().join()", "-1,2,3,20"),
        (
            "new Float64Array([3, NaN, -0, 0, -Infinity]).sort().join()",
            "-Infinity,0,0,3,NaN",
        ),
        (
            "new Int8Array([1, 2, 3]).sort((a, b) => b - a).join()",
            "3,2,1",
        ),
        (
            "new Int8Array([1, 2, 3]).map(x => x * 100).join()",
            "100,-56,44",
        ),
        (
            "new Int8Array([1, 2, 3, 4]).filter(x => x % 2).join()",
            "1,3",
        ),
        (
            "new Int8Array([1, 2, 3]).map(x => x) instanceof Int8Array",
            "true",
        ),
        ("new Uint8Array(3).fill(7, 1).join()", "0,7,7"),
        (
            "[new Uint8Array([1, 2, 3]).indexOf(2), new Uint8Array([1, 2, 3]).includes(4), new Float32Array([NaN]).includes(NaN)].join()",
            "1,false,true",
        ),
        ("new Uint8Array([1, 2, 3]).reverse().join('-')", "3-2-1"),
        (
            "[new Uint8Array([1, 2, 3]).at(-1), new Uint8Array([1, 2, 3]).reduce((a, b) => a + b)].join()",
            "3,6",
        ),
        ("new Uint8Array([1, 2]).toString()", "1,2"),
        (
            "Uint8Array.prototype.toString === Array.prototype.toString",
            "true",
        ),
        ("[...new Uint8Array([5, 6]).entries()].join(';')", "0,5;1,6"),
        ("[...new Uint8Array([5, 6]).keys()].join()", "0,1"),
        (
            "(() => { let s = 0; for (const x of new Uint16Array([1, 2, 3])) s += x; return s; })()",
            "6",
        ),
        (
            "new Uint8Array([3, 1, 2]).toSorted().join() + ';' + new Uint8Array([1, 2]).toReversed().join()",
            "1,2,3;2,1",
        ),
        ("new Uint8Array([1, 2, 3]).with(1, 9).join()", "1,9,3"),
        (
            "new Uint8Array([1, 2, 3, 4, 5]).copyWithin(0, 3).join()",
            "4,5,3,4,5",
        ),
        (
            "[new Uint8Array([1, 2, 3]).find(x => x > 1), new Uint8Array([1, 2, 3]).findLastIndex(x => x < 3)].join()",
            "2,1",
        ),
        ("new BigInt64Array([3n, -1n, 2n]).sort().join()", "-1,2,3"),
    ]);
    expect_errors(
        None,
        &[
            ("Uint8Array.prototype.join.call([1, 2])", "TypeError"),
            ("new Uint8Array(2).set([1, 2, 3])", "RangeError"),
            ("new Uint8Array(2).with(5, 1)", "RangeError"),
            (
                "Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Int8Array.prototype), 'length').get.call([])",
                "TypeError",
            ),
        ],
    );
}

#[test]
fn data_view_reads_and_writes_with_either_endianness() {
    expect(&[
        (
            "(() => { const v = new DataView(new ArrayBuffer(8)); v.setUint16(0, 0x1234); return [v.getUint8(0), v.getUint8(1), v.getUint16(0, true)]; })().join()",
            "18,52,13330",
        ),
        (
            "(() => { const v = new DataView(new ArrayBuffer(8)); v.setInt32(0, -2, true); return [v.getInt32(0, true), v.getUint32(0, true), v.getInt8(0)]; })().join()",
            "-2,4294967294,-2",
        ),
        (
            "(() => { const v = new DataView(new ArrayBuffer(8)); v.setFloat64(0, Math.PI); return v.getFloat64(0) === Math.PI; })()",
            "true",
        ),
        (
            "(() => { const v = new DataView(new ArrayBuffer(8)); v.setFloat32(0, 1.5, true); v.setFloat16(4, 0.5); return [v.getFloat32(0, true), v.getFloat16(4), v.getUint16(4)]; })().join()",
            "1.5,0.5,14336",
        ),
        (
            "(() => { const v = new DataView(new ArrayBuffer(16)); v.setBigInt64(0, -5n); v.setBigUint64(8, 2n ** 64n - 1n, true); return [v.getBigInt64(0), v.getBigUint64(8, true), v.getBigInt64(8)]; })().join()",
            "-5,18446744073709551615,-1",
        ),
        (
            "(() => { const v = new DataView(new ArrayBuffer(8), 2, 4); return [v.byteOffset, v.byteLength, v.buffer.byteLength]; })().join()",
            "2,4,8",
        ),
        (
            "Object.prototype.toString.call(new DataView(new ArrayBuffer(1)))",
            "[object DataView]",
        ),
    ]);
    expect_errors(
        None,
        &[
            ("new DataView(new ArrayBuffer(4)).getInt32(1)", "RangeError"),
            ("new DataView(new ArrayBuffer(4), 5)", "RangeError"),
            ("new DataView({})", "TypeError"),
            ("DataView(new ArrayBuffer(1))", "TypeError"),
            (
                "new DataView(new ArrayBuffer(8)).setBigInt64(0, 1)",
                "TypeError",
            ),
        ],
    );
}

#[test]
fn resizable_buffers_transfer_and_detach() {
    expect(&[
        (
            "(() => { const b = new ArrayBuffer(4, { maxByteLength: 16 }); const a = new Uint8Array(b); const f = new Uint8Array(b, 0, 2); b.resize(12); a[11] = 5; const grown = [b.byteLength, a.length, f.length, b.resizable, b.maxByteLength]; b.resize(1); return [...grown, a.length, f.length, f.byteLength]; })().join()",
            "12,12,2,true,16,1,0,0",
        ),
        (
            "(() => { const b = new ArrayBuffer(4); new Uint8Array(b)[0] = 7; const t = b.transfer(); return [b.detached, b.byteLength, t.byteLength, new Uint8Array(t)[0], t.detached]; })().join()",
            "true,0,4,7,false",
        ),
        (
            "(() => { const b = new ArrayBuffer(4); const a = new Uint8Array(b); a[0] = 3; const t = b.transfer(8); return [a.length, a[0], a.byteOffset, t.byteLength, new Uint8Array(t)[0], new Uint8Array(t)[7]]; })().join()",
            "0,,0,8,3,0",
        ),
        (
            "(() => { const b = new ArrayBuffer(2, { maxByteLength: 8 }); const t = b.transferToFixedLength(); return [t.resizable, b.resizable, b.transfer === undefined, new ArrayBuffer(1).maxByteLength]; })().join()",
            "false,true,false,1",
        ),
        (
            "(() => { const b = new ArrayBuffer(2, { maxByteLength: 8 }); return b.transfer().resizable; })()",
            "true",
        ),
    ]);
    expect_errors(
        None,
        &[
            ("new ArrayBuffer(4, { maxByteLength: 2 })", "RangeError"),
            ("new ArrayBuffer(4).resize(2)", "TypeError"),
            (
                "new ArrayBuffer(4, { maxByteLength: 8 }).resize(9)",
                "RangeError",
            ),
            (
                "const b = new ArrayBuffer(4); b.transfer(); b.slice(0)",
                "TypeError",
            ),
            (
                "const b = new ArrayBuffer(4); b.transfer(); new Uint8Array(b)",
                "TypeError",
            ),
            (
                "const b = new ArrayBuffer(4); const a = new Uint8Array(b); b.transfer(); a.fill(1)",
                "TypeError",
            ),
            (
                "const b = new ArrayBuffer(4); b.transfer(); b.transfer()",
                "TypeError",
            ),
        ],
    );
}

#[test]
fn shared_array_buffers_and_atomics() {
    expect(&[
        (
            "Object.prototype.toString.call(new SharedArrayBuffer(4))",
            "[object SharedArrayBuffer]",
        ),
        (
            "(() => { const s = new SharedArrayBuffer(8, { maxByteLength: 16 }); s.grow(12); return [s.byteLength, s.growable, s.maxByteLength, new Int32Array(s).length]; })().join()",
            "12,true,16,3",
        ),
        (
            "(() => { const s = new SharedArrayBuffer(8); new Uint8Array(s)[1] = 2; return [...new Uint8Array(s.slice(1, 3))].join(); })()",
            "2,0",
        ),
        (
            "(() => { const a = new Int32Array(new SharedArrayBuffer(16)); return [Atomics.store(a, 0, 5), Atomics.add(a, 0, 2), Atomics.sub(a, 0, 1), Atomics.and(a, 0, 3), Atomics.or(a, 0, 8), Atomics.xor(a, 0, 1), Atomics.exchange(a, 0, 4), Atomics.compareExchange(a, 0, 4, 9), Atomics.compareExchange(a, 0, 1, 2), Atomics.load(a, 0)]; })().join()",
            "5,5,7,6,2,10,11,4,9,9",
        ),
        (
            "(() => { const a = new BigInt64Array(new SharedArrayBuffer(8)); Atomics.add(a, 0, 5n); return Atomics.load(a, 0); })()",
            "5",
        ),
        ("Atomics.add(new Uint8Array(1), 0, 300)", "0"),
        (
            "Atomics.notify(new Int32Array(new SharedArrayBuffer(4)), 0)",
            "0",
        ),
        (
            "[Atomics.isLockFree(4), Atomics[Symbol.toStringTag]].join()",
            "true,Atomics",
        ),
    ]);
    expect_errors(
        None,
        &[
            ("SharedArrayBuffer(1)", "TypeError"),
            ("Atomics.add(new Float64Array(1), 0, 1)", "TypeError"),
            ("Atomics.load(new Int32Array(1), 4)", "RangeError"),
            (
                "Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 0)",
                "TypeError",
            ),
            ("Atomics.wait(new Int32Array(4), 0, 0, 0)", "TypeError"),
            ("new SharedArrayBuffer(4).grow(8)", "TypeError"),
        ],
    );
}

#[test]
fn bigint_increment_and_complement_respect_the_size_limit() {
    // 1 KiB budget = 8192 bits; `a` is 2^8192 - 1, the largest BigInt that fits.
    let a = "let a = (1n << 8191n) - 1n + (1n << 8191n)";
    expect_errors(
        Some(1024),
        &[
            (&format!("{a}; a++"), "RangeError"),
            (&format!("{a}; ++a"), "RangeError"),
            (&format!("{a}; ~a"), "RangeError"),
            (&format!("{a}; -a; --a"), "no error"),
            (&format!("{a}; a = -a; a--"), "RangeError"),
        ],
    );
}

#[test]
fn bigint_to_string_is_not_quadratic_per_byte() {
    let mut iso = isolate_with(None);
    let started = Instant::now();
    assert_eq!(run(&mut iso, "String(2n ** 100000n).length"), "30103");
    assert_eq!(run(&mut iso, "Number(2n ** 1000n) === 2 ** 1000"), "true");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
}
