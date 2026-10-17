//! Each test drives one Node-API group of the C addon in `tests/addons/` from JS.

mod support;

use support::{run_with_addon, serial};

fn check(code: &str) {
    if let Err(err) = run_with_addon(code) {
        panic!("{err}");
    }
}

#[test]
fn module_lifecycle_and_instance_data() {
    check(
        r#"
        assertEq(addon.getInstanceData(), 42, 'instance data');
        assertEq(addon.getVersion(), 8, 'napi_get_version');
        assertEq(addon.legacy, undefined, 'registered through napi_register_module_v1');
        "#,
    );
}

#[test]
fn primitive_values() {
    check(
        r#"
        const p = addon.primitives();
        assertEq(p.undef, undefined, 'undefined');
        assertEq(p.nul, null, 'null');
        assertEq(p.yes, true, 'true');
        assertEq(p.no, false, 'false');
        assertEq(p.dbl, 1.5, 'double');
        assertEq(p.i32, -7, 'int32');
        assertEq(p.u32, 4000000000, 'uint32');
        assertEq(p.i64, 9007199254740991, 'int64');
        assertEq(p.big, -5n, 'bigint');
        assertEq(p.global, globalThis, 'global');
        assertEq(p.sym.description, 'tag', 'symbol');
        const kinds = [undefined, null, true, 1, 's', Symbol(), {}, () => {}, 1n].map(addon.describe);
        assertEq(kinds.join(), 'undefined,null,boolean,number,string,symbol,object,function,bigint', 'typeof');
        const n = addon.numbers(-3.7);
        assertEq(n.double, -3.7, 'double');
        assertEq(n.int32, -3, 'int32');
        assertEq(n.uint32, 4294967293, 'uint32');
        assertEq(n.int64, -3, 'int64');
        assertEq(addon.numbers(2 ** 40).int32, 0, 'int32 wraps');
        assertEq(addon.numbers(2 ** 40).int64, 2 ** 40, 'int64 keeps');
        assertEq(addon.numbers(NaN).int64, 0, 'NaN int64');
        assertThrows(() => addon.numbers('x'), e => e.message.includes('A number was expected'), 'number expected');
        assertEq(addon.negate(false), true, 'bool');
        assertThrows(() => addon.negate(0), e => e.message.includes('A boolean was expected'), 'boolean expected');
        assertEq(addon.bigintAddOne(41n), 42n, 'bigint roundtrip');
        assertEq(addon.strictEquals('a', 'a'), true, 'equal strings');
        assertEq(addon.strictEquals({}, {}), false, 'distinct objects');
        assertEq(addon.strictEquals(NaN, NaN), false, 'NaN');
        const c = addon.coerce(12);
        assertEq(c[0], '12', 'to string');
        assertEq(c[1], 12, 'to number');
        assertEq(c[2], true, 'to bool');
        assertEq(c[3].valueOf(), 12, 'to object');
        assertThrows(() => addon.coerce({ toString() { throw new Error('boom'); } }), e => e.message === 'boom', 'coercion rethrows');
        "#,
    );
}

#[test]
fn strings_in_every_encoding() {
    check(
        r#"
        assertEq(addon.echoUtf8('héllo \u{1F600}'), 'héllo \u{1F600}', 'utf8');
        assertEq(addon.echoUtf8(''), '', 'empty utf8');
        assertEq(addon.echoUtf16('h\u{1F600}\ud800'), 'h\u{1F600}\ud800', 'utf16 keeps lone surrogates');
        assertEq(addon.echoLatin1('café'), 'café', 'latin1');
        assertEq(addon.utf8Prefix('abcdef', 4), 'abc', 'truncated to capacity');
        assertEq(addon.utf8Prefix('éé', 4), 'é', 'never splits a character');
        const [utf8, utf16, latin1] = addon.fixedStrings();
        assertEq(utf8, 'café', 'explicit utf8 length');
        assertEq(utf16, 'h\u{1F600}', 'auto utf16 length');
        assertEq(latin1, 'café', 'latin1 create');
        "#,
    );
}

#[test]
fn objects_and_properties() {
    check(
        r#"
        const obj = { existing: 'yes', removeMe: 1 };
        const r = addon.objectOps(obj);
        assertEq(r.hasKey, true, 'has_property');
        assertEq(r.hasOwnKey, true, 'has_own_property');
        assertEq(r.hasNamed, true, 'has_named_property');
        assertEq(r.hasInherited, true, 'inherited has');
        assertEq(r.ownInherited, false, 'inherited is not own');
        assertEq(r.deleted, true, 'delete_property');
        assertEq(r.existing, 'yes', 'get_named_property');
        assertEq(r.added, 1, 'get_property');
        assertEq(r.names.join(), 'existing,added,viaKey', 'get_property_names');
        assertEq(obj.viaKey, 2, 'set_property');
        assertEq('removeMe' in obj, false, 'deleted');
        const d = addon.defineProps({});
        assertEq(d.constant, 7, 'value property');
        d.constant = 8;
        assertEq(d.constant, 7, 'read-only value');
        assertEq(Object.keys(d).join(), 'constant,accessor', 'enumerability');
        assertEq(d.method(), 'method', 'method property');
        assertEq(d.accessor, 10, 'getter');
        d.accessor = 5;
        assertEq(d.accessor, 5, 'setter');
        class A {}
        const a = new A();
        assertEq(addon.getProto(a), A.prototype, 'get_prototype');
        assertEq(addon.isInstance(a, A), true, 'instanceof');
        assertEq(addon.isInstance({}, A), false, 'not instanceof');
        assertThrows(() => addon.isInstance(a, 1), e => e instanceof TypeError, 'instanceof non-function');
        "#,
    );
}

#[test]
fn arrays() {
    check(
        r#"
        assertEq(addon.makeSquares(4).join(), '0,1,4,9', 'create and set_element');
        const arr = [5, 6, 7];
        const info = addon.arrayInfo(arr);
        assertEq(info.isArray, true, 'is_array');
        assertEq(info.length, 3, 'get_array_length');
        assertEq(info.first, 5, 'get_element');
        assertEq(info.hasLast, true, 'has_element');
        assertEq(info.deletedFirst, true, 'delete_element');
        assertEq(info.hasFirstAfter, false, 'hole after delete');
        assertEq(0 in arr, false, 'element removed');
        assertEq(addon.arrayInfo({}).isArray, false, 'object is not an array');
        "#,
    );
}

#[test]
fn functions_and_callbacks() {
    check(
        r#"
        assertEq(addon.callWith((a, b) => a * b, 6, 7), 42, 'call_function');
        assertEq(addon.callWith(function () { return this; }, 0, 0), globalThis, 'receiver');
        assertThrows(() => addon.callWith(() => { throw new Error('inner'); }), e => e.message === 'inner', 'callee exception propagates');
        const add5 = addon.makeAdder(5);
        assertEq(add5(10), 15, 'callback data');
        assertEq(add5.name, 'adder', 'function name');
        class P { constructor(v) { this.v = v; } }
        assertEq(addon.construct(P, 9).v, 9, 'new_instance');
        const holder = { argInfo: addon.argInfo };
        const info = holder.argInfo(1, 2);
        assertEq(info.argc, 2, 'argc');
        assertEq(info.self, holder, 'this');
        assertEq(info.paddedType, 0, 'missing args are undefined');
        assertEq(addon.newTarget(), false, 'no new.target on a plain call');
        "#,
    );
}

#[test]
fn classes_with_wrap() {
    check(
        r#"
        const c = new addon.Counter(3);
        assertEq(c instanceof addon.Counter, true, 'instanceof class');
        assertEq(addon.Counter.name, 'Counter', 'class name');
        assertEq(c.increment(), 4, 'method + unwrap');
        assertEq(c.value, 4, 'getter');
        c.value = 10;
        assertEq(c.increment(), 11, 'setter');
        assertEq(c.unit, 'count', 'prototype value');
        assertEq(addon.Counter.kind(), 'counter', 'static method');
        assertEq(addon.Counter.VERSION, 1, 'static value');
        assertThrows(() => addon.Counter(1), e => e instanceof TypeError, 'new_target is null without new');
        assertEq(addon.detach(c), 11, 'remove_wrap');
        assertThrows(() => c.increment(), e => e.message.includes('Invalid argument'), 'unwrap after remove_wrap');
        assertEq(addon.wrapTwice({}), 1, 'second wrap is napi_invalid_arg');
        const before = addon.counter(2);
        (() => { for (let i = 0; i < 10; i++) new addon.Counter(i); })();
        gc();
        assertEq(addon.counter(2) - before, 10, 'wrap finalizers run after GC');
        "#,
    );
}

#[test]
fn errors_and_exceptions() {
    check(
        r#"
        assertThrows(addon.throwError, e => e instanceof Error && e.message === 'plain failure' && e.code === 'E_ADDON', 'throw_error');
        assertThrows(addon.throwTypeError, e => e instanceof TypeError && e.code === 'E_TYPE', 'throw_type_error');
        assertThrows(addon.throwRangeError, e => e instanceof RangeError && e.code === undefined, 'throw_range_error');
        const sentinel = { tag: 1 };
        assertThrows(() => addon.throwValue(sentinel), e => e === sentinel, 'throw');
        const [plain, type, range] = addon.createErrors();
        assertEq(plain instanceof Error && !(plain instanceof TypeError), true, 'create_error');
        assertEq(type instanceof TypeError, true, 'create_type_error');
        assertEq(range instanceof RangeError, true, 'create_range_error');
        assertEq(range.code, 'E_CREATED', 'error code');
        assertEq(range.message, 'created', 'error message');
        assertEq(addon.isError(new TypeError('x')), true, 'is_error');
        assertEq(addon.isError({ message: 'x' }), false, 'plain object is not an error');
        const caught = addon.catchFrom(() => { throw 'oops'; });
        assertEq(caught.status, 10, 'napi_pending_exception');
        assertEq(caught.pending, 1, 'is_exception_pending');
        assertEq(caught.stillPending, 0, 'get_and_clear_last_exception clears');
        assertEq(caught.exception, 'oops', 'cleared exception');
        const fine = addon.catchFrom(() => 1);
        assertEq(fine.status, 0, 'no exception');
        assertEq(fine.exception, undefined, 'nothing to clear');
        const last = addon.lastError();
        assertEq(last.status, 6, 'napi_number_expected');
        assertEq(last.code, 6, 'error_code');
        assertEq(last.message, 'A number was expected', 'error_message');
        "#,
    );
}

#[test]
fn array_buffers_typed_arrays_and_buffers() {
    check(
        r#"
        const ab = addon.makeArrayBuffer(4);
        assertEq(ab instanceof ArrayBuffer, true, 'create_arraybuffer');
        assertEq(new Uint8Array(ab).join(), '0,1,2,3', 'arraybuffer data');
        assertEq(addon.arrayBufferSum(ab), 6, 'get_arraybuffer_info');
        assertEq(addon.arrayBufferSum(new Uint8Array(2)), -1, 'is_arraybuffer');
        const kinds = [Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array, Int32Array,
          Uint32Array, Float32Array, Float64Array, BigInt64Array, BigUint64Array];
        kinds.forEach((Kind, type) => assertEq(addon.makeTypedArray(type, 1, 0) instanceof Kind, true, Kind.name));
        const view = addon.makeTypedArray(5, 2, 4);
        assertEq(view[0], 0x01010101, 'typed array data');
        const info = addon.typedArrayInfo(view);
        assertEq(info.isTypedArray, 1, 'is_typedarray');
        assertEq(info.type, 5, 'type');
        assertEq(info.length, 2, 'length');
        assertEq(info.byteOffset, 4, 'byte offset');
        assertEq(info.firstByte, 1, 'data points at the first element');
        assertEq(info.buffer, view.buffer, 'arraybuffer');
        assertEq(addon.typedArrayInfo([1]).isTypedArray, 0, 'array is not typed');
        assertThrows(() => addon.makeTypedArray(5, 1, 2), e => e instanceof RangeError, 'misaligned offset');
        assertThrows(() => addon.makeTypedArray(1, 17, 0), e => e instanceof RangeError, 'past the end');
        globalThis.Buffer = class Buffer extends Uint8Array {};
        const buf = addon.makeBuffer('héllo');
        assertEq(buf instanceof Buffer, true, 'Buffer prototype');
        assertEq(buf.length, 6, 'create_buffer_copy');
        assertEq(addon.bufferText(buf).text, 'héllo', 'get_buffer_info');
        assertEq(addon.bufferText(addon.makeFilledBuffer(3)).text, 'abc', 'create_buffer');
        assertEq(addon.bufferText(addon.makeExternalBuffer()).text, 'ext', 'create_external_buffer');
        assertEq(addon.bufferText('x').isBuffer, 0, 'is_buffer');
        "#,
    );
}

#[test]
fn handle_scopes_references_and_finalizers() {
    check(
        r#"
        assertEq(addon.scopeLoop(20000), 20000, 'handle scopes');
        const esc = addon.escape();
        assertEq(esc.value, 'escaped', 'escape_handle');
        assertEq(esc.second, 12, 'napi_escape_called_twice');
        assertEq(esc.mismatch, 13, 'napi_handle_scope_mismatch');
        const refs = addon.references({});
        assertEq(refs.afterRef, 2, 'reference_ref');
        assertEq(refs.afterUnref, 1, 'reference_unref');
        assertEq(refs.same, 1, 'get_reference_value');
        const kept = {};
        addon.holdWeakly(kept);
        assertEq(addon.weakTarget(), kept, 'weak reference while alive');
        (() => addon.holdWeakly({}))();
        gc();
        assertEq(addon.weakTarget(), undefined, 'weak reference after GC');
        const finalized = addon.counter(1);
        (() => { for (let i = 0; i < 5; i++) addon.attachFinalizer({}); })();
        gc();
        assertEq(addon.counter(1) - finalized, 5, 'add_finalizer');
        const ext = addon.makeExternal(77);
        assertEq(addon.describe(ext), 'external', 'typeof external');
        assertEq(addon.externalValue(ext), 77, 'get_value_external');
        assertThrows(() => addon.externalValue({}), e => e.message.includes('Invalid argument'), 'not external');
        const externals = addon.counter(3);
        (() => addon.makeExternal(1))();
        gc();
        assertEq(addon.counter(3) - externals, 1, 'external finalizer');
        "#,
    );
}

#[test]
fn shutdown_runs_pending_finalizers_and_instance_data() {
    let _serial = serial();
    let path = support::addon();
    let instance = support::counter(path, support::INSTANCE_DATA_FINALIZED);
    let wraps = support::counter(path, support::WRAP_FINALIZED);
    support::with_context(|scope| {
        support::install_addon(scope, path);
        support::run(scope, "globalThis.kept = new addon.Counter(1);").unwrap();
        velyr_compat_napi::shutdown(scope);
    });
    assert_eq!(
        support::counter(path, support::INSTANCE_DATA_FINALIZED),
        instance + 1
    );
    assert_eq!(support::counter(path, support::WRAP_FINALIZED), wraps + 1);
}
