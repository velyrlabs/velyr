//! VELYR-142: P-521, AES-KW (RFC 3394), JWK `alg` validation and the PBKDF2 iteration cap.
mod common;
use common::expect_with_helpers as expect;
use common::fixtures::{P521_PKCS8, P521_SIG_SHA512, P521_SPKI};

#[test]
fn p521_matches_openssl_encodings_and_verifies_its_signature() {
    expect(
        &format!(
            "const alg = {{ name: 'ECDSA', namedCurve: 'P-521' }}, sign = {{ name: 'ECDSA', hash: 'SHA-512' }};
             const pub = await subtle.importKey('spki', unhex('{P521_SPKI}'), alg, true, ['verify']);
             const priv = await subtle.importKey('pkcs8', unhex('{P521_PKCS8}'), alg, true, ['sign']);
             record(String(hex(await subtle.exportKey('spki', pub)) === '{P521_SPKI}'));
             record(String(hex(await subtle.exportKey('pkcs8', priv)) === '{P521_PKCS8}'));
             record(String(await subtle.verify(sign, pub, unhex('{P521_SIG_SHA512}'), utf8('sample'))));
             record(String(await subtle.verify(sign, pub, unhex('{P521_SIG_SHA512}'), utf8('sampl3'))));"
        ),
        &["true", "true", "true", "false"],
    );
}

#[test]
fn p521_sign_verify_jwk_and_ecdh_round_trips() {
    expect(
        "const alg = { name: 'ECDSA', namedCurve: 'P-521' };
         const pair = await subtle.generateKey(alg, true, ['sign', 'verify']);
         const jwk = await subtle.exportKey('jwk', pair.privateKey);
         record([jwk.crv, jwk.x.length, jwk.y.length, jwk.d.length].join(' '));
         const priv = await subtle.importKey('jwk', jwk, alg, true, ['sign']);
         for (const hash of ['SHA-256', 'SHA-384', 'SHA-512']) {
           const sig = await subtle.sign({ name: 'ECDSA', hash }, priv, utf8('m'));
           record(hash + ' ' + sig.byteLength + ' ' + await subtle.verify({ name: 'ECDSA', hash }, pair.publicKey, sig, utf8('m')));
         }
         const a = await subtle.generateKey({ name: 'ECDH', namedCurve: 'P-521' }, false, ['deriveBits']);
         const b = await subtle.generateKey({ name: 'ECDH', namedCurve: 'P-521' }, false, ['deriveBits']);
         const ab = hex(await subtle.deriveBits({ name: 'ECDH', public: b.publicKey }, a.privateKey, 256));
         const ba = hex(await subtle.deriveBits({ name: 'ECDH', public: a.publicKey }, b.privateKey, 256));
         record('ecdh ' + (ab === ba));
         record('full ' + (await subtle.deriveBits({ name: 'ECDH', public: b.publicKey }, a.privateKey, null)).byteLength);",
        &[
            "P-521 88 88 88",
            "SHA-256 132 true",
            "SHA-384 132 true",
            "SHA-512 132 true",
            "ecdh true",
            "full 66",
        ],
    );
}

#[test]
fn aes_kw_rfc3394_vectors_and_integrity() {
    expect(
        "const kw = { name: 'AES-KW' }, hmac = { name: 'HMAC', hash: 'SHA-256' };
         const cases = [
           ['000102030405060708090a0b0c0d0e0f', '00112233445566778899aabbccddeeff',
            '1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5'],
           ['000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f',
            '00112233445566778899aabbccddeeff000102030405060708090a0b0c0d0e0f',
            '28c9f404c4b810f4cbccb35cfb87f8263f5786e2d80ed326cbc7f0e71a99f43bfb988b9b7a02dd21'],
         ];
         for (const [kek, data, wrapped] of cases) {
           const key = await subtle.importKey('raw', unhex(kek), kw, false, ['wrapKey', 'unwrapKey']);
           const target = await subtle.importKey('raw', unhex(data), hmac, true, ['sign']);
           const out = await subtle.wrapKey('raw', target, key, kw);
           record(hex(out) === wrapped ? 'wrap ok' : 'wrap ' + hex(out));
           const back = await subtle.unwrapKey('raw', out, key, kw, hmac, true, ['sign']);
           record(hex(await subtle.exportKey('raw', back)) === data ? 'unwrap ok' : 'unwrap differs');
           const bad = new Uint8Array(out); bad[3] ^= 1;
           try { await subtle.unwrapKey('raw', bad, key, kw, hmac, true, ['sign']); record('no throw'); } catch (e) { record(e.name); }
         }",
        &["wrap ok", "unwrap ok", "OperationError", "wrap ok", "unwrap ok", "OperationError"],
    );
}

#[test]
fn aes_kw_wraps_jwk_and_rejects_encrypt_and_aes_192() {
    expect(
        "const kw = { name: 'AES-KW' }, hmac = { name: 'HMAC', hash: 'SHA-256' };
         const kek = await subtle.generateKey({ name: 'AES-KW', length: 256 }, true, ['wrapKey', 'unwrapKey']);
         const target = await subtle.generateKey(hmac, true, ['sign', 'verify']);
         const wrapped = await subtle.wrapKey('jwk', target, kek, kw);
         const back = await subtle.unwrapKey('jwk', wrapped, kek, kw, hmac, true, ['sign', 'verify']);
         const mac = await subtle.sign('HMAC', target, utf8('m'));
         record('jwk ' + (wrapped.byteLength % 8) + ' ' + await subtle.verify('HMAC', back, mac, utf8('m')));
         const kekJwk = await subtle.exportKey('jwk', kek);
         record(kekJwk.alg + ' ' + kek.algorithm.name + ' ' + kek.algorithm.length);
         try { await subtle.encrypt(kw, kek, new Uint8Array(16)); record('no throw'); } catch (e) { record(e.name); }
         const k192 = await subtle.generateKey({ name: 'AES-KW', length: 192 }, true, ['wrapKey']);
         try { await subtle.wrapKey('raw', target, k192, kw); record('no throw'); } catch (e) { record(e.name); }",
        &["jwk 0 true", "A256KW AES-KW 256", "NotSupportedError", "NotSupportedError"],
    );
}

#[test]
fn jwk_alg_is_validated_for_ecdsa_and_ed25519() {
    expect(
        "const pair = await subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']);
         const pub = await subtle.exportKey('jwk', pair.publicKey);
         const ed = await subtle.exportKey('jwk', (await subtle.generateKey('Ed25519', true, ['sign', 'verify'])).publicKey);
         const tryImport = async (jwk, alg) => {
           try { await subtle.importKey('jwk', jwk, alg, true, ['verify']); return 'ok'; } catch (e) { return e.name; }
         };
         const ec = { name: 'ECDSA', namedCurve: 'P-256' };
         for (const a of [undefined, 'ES256', 'ES384', 'HS256']) record('ec ' + a + ' ' + await tryImport({ ...pub, alg: a }, ec));
         for (const a of [undefined, 'Ed25519', 'EdDSA', 'RS256']) record('ed ' + a + ' ' + await tryImport({ ...ed, alg: a }, 'Ed25519'));",
        &[
            "ec undefined ok",
            "ec ES256 ok",
            "ec ES384 DataError",
            "ec HS256 DataError",
            "ed undefined ok",
            "ed Ed25519 ok",
            "ed EdDSA ok",
            "ed RS256 DataError",
        ],
    );
}

#[test]
fn pbkdf2_iterations_are_capped() {
    expect(
        "const base = await subtle.importKey('raw', utf8('pw'), 'PBKDF2', false, ['deriveBits']);
         for (const iterations of [10000001, 4294967295]) {
           try { await subtle.deriveBits({ name: 'PBKDF2', hash: 'SHA-256', salt: utf8('s'), iterations }, base, 8); record('no throw'); }
           catch (e) { record(e.name); }
         }
         for (const iterations of [1000, 1500000]) {
           record(String((await subtle.deriveBits({ name: 'PBKDF2', hash: 'SHA-1', salt: utf8('s'), iterations }, base, 8)).byteLength));
         }",
        &["OperationError", "OperationError", "1", "1"],
    );
}
