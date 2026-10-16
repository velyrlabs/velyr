//! Known-answer tests: RFC 6070 (PBKDF2), RFC 5869 (HKDF), RFC 7748 (X25519), NIST SP 800-38A (CBC, CTR).
mod common;
use common::expect_with_helpers as expect;
use common::fixtures::{RSA_PKCS8, RSA_SPKI};

#[test]
fn pbkdf2_rfc6070_and_sha256() {
    expect(
        "const base = await subtle.importKey('raw', utf8('password'), 'PBKDF2', false, ['deriveBits', 'deriveKey']);
         const run = (hash, iterations, bits) => subtle.deriveBits({ name: 'PBKDF2', hash, salt: utf8('salt'), iterations }, base, bits);
         record(hex(await run('SHA-1', 2, 160)));
         record(hex(await run('SHA-256', 1, 256)));",
        &[
            "ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957",
            "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b",
        ],
    );
}

#[test]
fn hkdf_rfc5869_case_1() {
    expect(
        "const ikm = await subtle.importKey('raw', new Uint8Array(22).fill(0x0b), 'HKDF', false, ['deriveBits']);
         const salt = unhex('000102030405060708090a0b0c'), info = unhex('f0f1f2f3f4f5f6f7f8f9');
         record(hex(await subtle.deriveBits({ name: 'HKDF', hash: 'SHA-256', salt, info }, ikm, 42 * 8)));",
        &["3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"],
    );
}

#[test]
fn x25519_rfc7748_shared_secret() {
    expect(
        "const alice = await subtle.importKey('pkcs8', unhex('302e020100300506032b656e04220420' + '77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a'), 'X25519', false, ['deriveBits']);
         const bob = await subtle.importKey('raw', unhex('de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f'), 'X25519', true, []);
         record(hex(await subtle.deriveBits({ name: 'X25519', public: bob }, alice, 256)));
         record(hex(await subtle.deriveBits({ name: 'X25519', public: bob }, alice, null)));",
        &[
            "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742",
            "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742",
        ],
    );
}

#[test]
fn ecdh_both_sides_agree_and_derive_key_works() {
    expect(
        "for (const [name, gen] of [['P-256', { name: 'ECDH', namedCurve: 'P-256' }], ['P-384', { name: 'ECDH', namedCurve: 'P-384' }], ['X25519', { name: 'X25519' }]]) {
           const a = await subtle.generateKey(gen, true, ['deriveBits', 'deriveKey']);
           const b = await subtle.generateKey(gen, true, ['deriveBits', 'deriveKey']);
           const ab = hex(await subtle.deriveBits({ name: gen.name, public: b.publicKey }, a.privateKey, 128));
           const ba = hex(await subtle.deriveBits({ name: gen.name, public: a.publicKey }, b.privateKey, 128));
           const key1 = await subtle.deriveKey({ name: gen.name, public: b.publicKey }, a.privateKey, { name: 'AES-GCM', length: 256 }, true, ['encrypt', 'decrypt']);
           const key2 = await subtle.deriveKey({ name: gen.name, public: a.publicKey }, b.privateKey, { name: 'AES-GCM', length: 256 }, true, ['encrypt', 'decrypt']);
           const same = hex(await subtle.exportKey('raw', key1)) === hex(await subtle.exportKey('raw', key2));
           record(name + ' ' + (ab === ba) + ' ' + ab.length + ' ' + same);
         }",
        &["P-256 true 32 true", "P-384 true 32 true", "X25519 true 32 true"],
    );
}

#[test]
fn derive_key_into_hmac_and_pbkdf2_into_aes() {
    expect(
        "const base = await subtle.importKey('raw', utf8('pw'), 'PBKDF2', false, ['deriveKey', 'deriveBits']);
         const aes = await subtle.deriveKey({ name: 'PBKDF2', hash: 'SHA-256', salt: utf8('s'), iterations: 10 }, base, { name: 'AES-GCM', length: 128 }, false, ['encrypt', 'decrypt']);
         const iv = new Uint8Array(12);
         const sealed = await subtle.encrypt({ name: 'AES-GCM', iv }, aes, utf8('x'));
         record(aes.algorithm.length + ' ' + aes.extractable + ' ' + new TextDecoder().decode(await subtle.decrypt({ name: 'AES-GCM', iv }, aes, sealed)));
         const hmac = await subtle.deriveKey({ name: 'PBKDF2', hash: 'SHA-1', salt: utf8('s'), iterations: 1 }, base, { name: 'HMAC', hash: 'SHA-256' }, true, ['sign']);
         record(hmac.algorithm.length + ' ' + hmac.algorithm.hash.name);
         try { await subtle.importKey('raw', utf8('pw'), 'PBKDF2', true, ['deriveBits']); record('no throw'); } catch (e) { record(e.name); }
         try { await subtle.deriveBits({ name: 'PBKDF2', hash: 'SHA-1', salt: utf8('s'), iterations: 0 }, base, 8); record('no throw'); } catch (e) { record(e.name); }",
        &["128 false x", "512 SHA-256", "SyntaxError", "OperationError"],
    );
}

#[test]
fn aes_cbc_nist_sp800_38a() {
    expect(
        "const key = await subtle.importKey('raw', unhex('2b7e151628aed2a6abf7158809cf4f3c'), 'AES-CBC', false, ['encrypt', 'decrypt']);
         const params = { name: 'AES-CBC', iv: unhex('000102030405060708090a0b0c0d0e0f') };
         const sealed = await subtle.encrypt(params, key, unhex('6bc1bee22e409f96e93d7e117393172a'));
         record(sealed.byteLength + ' ' + hex(sealed).slice(0, 32));
         record(hex(await subtle.decrypt(params, key, sealed)));
         try { await subtle.decrypt({ name: 'AES-CBC', iv: new Uint8Array(8) }, key, sealed); record('no throw'); } catch (e) { record(e.name); }",
        &[
            "32 7649abac8119b246cee98e9b12e9197d",
            "6bc1bee22e409f96e93d7e117393172a",
            "OperationError",
        ],
    );
}

#[test]
fn aes_ctr_nist_sp800_38a_and_counter_length() {
    expect(
        "const key = await subtle.importKey('raw', unhex('2b7e151628aed2a6abf7158809cf4f3c'), 'AES-CTR', false, ['encrypt', 'decrypt']);
         const plain = unhex('6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51');
         const counter = unhex('f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff');
         const sealed = await subtle.encrypt({ name: 'AES-CTR', counter, length: 128 }, key, plain);
         record(hex(sealed));
         record(hex(await subtle.decrypt({ name: 'AES-CTR', counter, length: 128 }, key, sealed)) === hex(plain) ? 'round trip' : 'differs');
         const low = unhex('00000000000000000000000000000000');
         const narrow = await subtle.encrypt({ name: 'AES-CTR', counter: low, length: 64 }, key, plain);
         const wide = await subtle.encrypt({ name: 'AES-CTR', counter: low, length: 128 }, key, plain);
         record(hex(narrow) === hex(wide) ? 'no wrap same' : 'differs');
         const edge = unhex('0000000000000000ffffffffffffffff');
         for (const [c, bits, size] of [[edge, 64, 32], [low, 1, 48]]) {
           try { await subtle.encrypt({ name: 'AES-CTR', counter: c, length: bits }, key, new Uint8Array(size)); record('no throw'); } catch (e) { record(e.name); }
         }",
        &[
            "874d6191b620e3261bef6864990db6ce9806f66b7970fdff8617187bb9fffdff",
            "round trip",
            "no wrap same",
            "NotSupportedError",
            "OperationError",
        ],
    );
}

#[test]
fn wrap_and_unwrap_key_round_trip() {
    expect(
        "const kek = await subtle.generateKey({ name: 'AES-GCM', length: 256 }, true, ['wrapKey', 'unwrapKey']);
         const params = { name: 'AES-GCM', iv: new Uint8Array(12) };
         const hmac = await subtle.generateKey({ name: 'HMAC', hash: 'SHA-256' }, true, ['sign', 'verify']);
         const mac = await subtle.sign('HMAC', hmac, utf8('m'));
         for (const format of ['jwk', 'raw']) {
           const wrapped = await subtle.wrapKey(format, hmac, kek, params);
           const back = await subtle.unwrapKey(format, wrapped, kek, params, { name: 'HMAC', hash: 'SHA-256' }, true, ['verify']);
           record(format + ' ' + await subtle.verify('HMAC', back, mac, utf8('m')));
         }
         const pair = await subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']);
         const wrapped = await subtle.wrapKey('pkcs8', pair.privateKey, kek, params);
         const priv = await subtle.unwrapKey('pkcs8', wrapped, kek, params, { name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign']);
         const sig = await subtle.sign({ name: 'ECDSA', hash: 'SHA-256' }, priv, utf8('m'));
         record('pkcs8 ' + await subtle.verify({ name: 'ECDSA', hash: 'SHA-256' }, pair.publicKey, sig, utf8('m')));
         try { await subtle.wrapKey('raw', hmac, hmac, params); record('no throw'); } catch (e) { record(e.name); }",
        &["jwk true", "raw true", "pkcs8 true", "InvalidAccessError"],
    );
}

/// What this crate still rejects, derived from aws-lc-rs limits: each must be NotSupportedError, never a result.
#[test]
fn unsupported_algorithms_and_parameters_reject_with_not_supported_error() {
    let body = "const gen = (name, extra) => subtle.generateKey({ name, modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: 'SHA-256', ...extra }, true, name === 'RSA-OAEP' ? ['encrypt', 'decrypt'] : ['sign', 'verify']);
         const aes = async (name, length, usage) => subtle.generateKey({ name, length }, false, [usage]);
         const cases = {
           'RSA exponent 3': () => gen('RSA-PSS', { publicExponent: new Uint8Array([3]) }),
           'RSA modulus 1024': () => gen('RSA-PSS', { modulusLength: 1024 }),
           'RSA modulus 2050': () => gen('RSA-OAEP', { modulusLength: 2050 }),
           'RSA-PSS with SHA-1': async () => {
             const k = await subtle.importKey('spki', unhex(RSA_SPKI), { name: 'RSA-PSS', hash: 'SHA-1' }, true, ['verify']);
             return subtle.verify({ name: 'RSA-PSS', saltLength: 20 }, k, new Uint8Array(256), new Uint8Array(1));
           },
           'RSASSA-PKCS1 sign with SHA-1': async () => {
             const k = await subtle.importKey('pkcs8', unhex(RSA_PKCS8), { name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-1' }, true, ['sign']);
             return subtle.sign('RSASSA-PKCS1-v1_5', k, new Uint8Array(1));
           },
           'AES-KW 192 wrap': async () => {
             const kek = await aes('AES-KW', 192, 'wrapKey');
             const target = await subtle.generateKey({ name: 'HMAC', hash: 'SHA-256' }, true, ['sign']);
             return subtle.wrapKey('raw', target, kek, 'AES-KW');
           },
           'AES-KW encrypt': async () => subtle.encrypt('AES-KW', await aes('AES-KW', 128, 'wrapKey'), new Uint8Array(16)),
           'ECDSA P-256 + SHA-384 sign': async () => {
             const pair = await subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign']);
             return subtle.sign({ name: 'ECDSA', hash: 'SHA-384' }, pair.privateKey, new Uint8Array(1));
           },
           'ECDSA P-521 + SHA-1 sign': async () => {
             const pair = await subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-521' }, false, ['sign']);
             return subtle.sign({ name: 'ECDSA', hash: 'SHA-1' }, pair.privateKey, new Uint8Array(1));
           },
           'ECDSA P-192 generate': () => subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-192' }, true, ['sign']),
           'AES-GCM 16-byte iv': async () => subtle.encrypt({ name: 'AES-GCM', iv: new Uint8Array(16) }, await aes('AES-GCM', 128, 'encrypt'), new Uint8Array(1)),
           'AES-GCM 96-bit tag': async () => subtle.encrypt({ name: 'AES-GCM', iv: new Uint8Array(12), tagLength: 96 }, await aes('AES-GCM', 128, 'encrypt'), new Uint8Array(1)),
           'HMAC 12-bit key': () => subtle.generateKey({ name: 'HMAC', hash: 'SHA-256', length: 12 }, true, ['sign']),
           'PBKDF2 as derived key type': async () => {
             const base = await subtle.importKey('raw', utf8('pw'), 'PBKDF2', false, ['deriveKey']);
             return subtle.deriveKey({ name: 'PBKDF2', hash: 'SHA-1', salt: utf8('s'), iterations: 1 }, base, 'HKDF', false, ['deriveBits']);
           },
         };
         for (const [label, run] of Object.entries(cases)) {
           try { await run(); record(label + ' -> no throw'); } catch (e) { record(label + ' -> ' + e.name); }
         }";
    let prelude = format!("const RSA_SPKI = '{RSA_SPKI}', RSA_PKCS8 = '{RSA_PKCS8}';");
    expect(
        &format!("{prelude}\n{body}"),
        &[
            "RSA exponent 3 -> NotSupportedError",
            "RSA modulus 1024 -> NotSupportedError",
            "RSA modulus 2050 -> NotSupportedError",
            "RSA-PSS with SHA-1 -> NotSupportedError",
            "RSASSA-PKCS1 sign with SHA-1 -> NotSupportedError",
            "AES-KW 192 wrap -> NotSupportedError",
            "AES-KW encrypt -> NotSupportedError",
            "ECDSA P-256 + SHA-384 sign -> NotSupportedError",
            "ECDSA P-521 + SHA-1 sign -> NotSupportedError",
            "ECDSA P-192 generate -> NotSupportedError",
            "AES-GCM 16-byte iv -> NotSupportedError",
            "AES-GCM 96-bit tag -> NotSupportedError",
            "HMAC 12-bit key -> NotSupportedError",
            "PBKDF2 as derived key type -> NotSupportedError",
        ],
    );
}

#[test]
fn kdf_output_length_is_capped_before_allocating() {
    expect(
        "const pbkdf2 = await subtle.importKey('raw', utf8('pw'), 'PBKDF2', false, ['deriveBits']);
         const hkdf = await subtle.importKey('raw', utf8('pw'), 'HKDF', false, ['deriveBits']);
         const huge = 8 * 1024 * 1024 * 1024;
         for (const run of [
           () => subtle.deriveBits({ name: 'PBKDF2', hash: 'SHA-256', salt: utf8('s'), iterations: 1 }, pbkdf2, huge),
           () => subtle.deriveBits({ name: 'HKDF', hash: 'SHA-256', salt: utf8('s'), info: utf8('i') }, hkdf, huge),
         ]) {
           try { await run(); record('no throw'); } catch (e) { record(e.name); }
         }",
        &["OperationError", "OperationError"],
    );
}
