mod common;
use common::expect;

#[test]
fn get_random_values_fills_and_returns_same_array() {
    expect(
        "const a = new Uint8Array(32); const r = crypto.getRandomValues(a);
         record(String(r === a) + ' ' + a.some((x) => x !== 0));",
        &["true true"],
    );
}

#[test]
fn get_random_values_quota_boundary() {
    expect(
        "crypto.getRandomValues(new Uint8Array(65536));
         try { crypto.getRandomValues(new Uint8Array(65537)); record('no throw'); }
         catch (e) { record(e.name + ' ' + (e instanceof DOMException)); }",
        &["QuotaExceededError true"],
    );
}

#[test]
fn get_random_values_rejects_non_integer_arrays() {
    expect(
        "for (const bad of [new Float32Array(2), new DataView(new ArrayBuffer(4)), [1], null]) {
           try { crypto.getRandomValues(bad); record('no throw'); } catch (e) { record(e.name); }
         }",
        &[
            "TypeMismatchError",
            "TypeMismatchError",
            "TypeMismatchError",
            "TypeMismatchError",
        ],
    );
}

#[test]
fn random_uuid_is_v4_and_unique() {
    expect(
        "const a = crypto.randomUUID(), b = crypto.randomUUID();
         record(String(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(a)) + ' ' + (a !== b));",
        &["true true"],
    );
}

#[test]
fn digest_known_vectors() {
    expect(
        "const hex = (b) => [...new Uint8Array(b)].map((x) => x.toString(16).padStart(2, '0')).join('');
         const abc = new TextEncoder().encode('abc');
         for (const h of ['SHA-1', 'SHA-256', 'SHA-384', 'SHA-512']) record(h + ' ' + hex(await crypto.subtle.digest(h, abc)));
         record('empty ' + hex(await crypto.subtle.digest({ name: 'sha-256' }, new Uint8Array(0))));",
        &[
            "SHA-1 a9993e364706816aba3e25717850c26c9cd0d89d",
            "SHA-256 ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "SHA-384 cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7",
            "SHA-512 ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
            "empty e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ],
    );
}

#[test]
fn digest_rejects_unsupported_algorithm_with_not_supported_error() {
    expect(
        "try { await crypto.subtle.digest('MD5', new Uint8Array(1)); record('no throw'); }
         catch (e) { record(e.name); }",
        &["NotSupportedError"],
    );
}
