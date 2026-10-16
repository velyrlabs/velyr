//! exportKey(jwk) -> importKey -> sign/verify, plus generateKey for every priority algorithm.
mod common;
use common::expect_with_helpers as expect;

#[test]
fn jwk_round_trip_sign_verify_for_ecdsa_ed25519_and_hmac() {
    expect(
        "const data = utf8('payload');
         for (const [alg, gen, signAlg] of [
           ['ECDSA P-256', { name: 'ECDSA', namedCurve: 'P-256' }, { name: 'ECDSA', hash: 'SHA-256' }],
           ['ECDSA P-384', { name: 'ECDSA', namedCurve: 'P-384' }, { name: 'ECDSA', hash: 'SHA-384' }],
           ['Ed25519', { name: 'Ed25519' }, { name: 'Ed25519' }],
         ]) {
           const pair = await subtle.generateKey(gen, true, ['sign', 'verify']);
           const privJwk = await subtle.exportKey('jwk', pair.privateKey);
           const pubJwk = await subtle.exportKey('jwk', pair.publicKey);
           const priv = await subtle.importKey('jwk', privJwk, gen, true, ['sign']);
           const pub = await subtle.importKey('jwk', pubJwk, gen, true, ['verify']);
           const sig = await subtle.sign(signAlg, priv, data);
           record(alg + ' ' + await subtle.verify(signAlg, pub, sig, data) + ' ' + await subtle.verify(signAlg, pair.publicKey, sig, data)
             + ' ' + pair.privateKey.type + '/' + pair.publicKey.type + ' ' + privJwk.kty + ' ' + ('d' in pubJwk));
         }
         const hmacKey = await subtle.generateKey({ name: 'HMAC', hash: 'SHA-512' }, true, ['sign', 'verify']);
         const jwk = await subtle.exportKey('jwk', hmacKey);
         const back = await subtle.importKey('jwk', jwk, { name: 'HMAC', hash: 'SHA-512' }, true, ['sign', 'verify']);
         const mac = await subtle.sign('HMAC', hmacKey, data);
         record('HMAC ' + jwk.alg + ' ' + hmacKey.algorithm.length + ' ' + await subtle.verify('HMAC', back, mac, data));",
        &[
            "ECDSA P-256 true true private/public EC false",
            "ECDSA P-384 true true private/public EC false",
            "Ed25519 true true private/public OKP false",
            "HMAC HS512 1024 true",
        ],
    );
}

#[test]
fn aes_generate_export_import_and_gcm_round_trip() {
    expect(
        "for (const bits of [128, 192, 256]) {
           const key = await subtle.generateKey({ name: 'AES-GCM', length: bits }, true, ['encrypt', 'decrypt']);
           const jwk = await subtle.exportKey('jwk', key);
           const back = await subtle.importKey('jwk', jwk, 'AES-GCM', false, ['encrypt', 'decrypt']);
           const iv = crypto.getRandomValues(new Uint8Array(12));
           const sealed = await subtle.encrypt({ name: 'AES-GCM', iv }, key, utf8('hello'));
           const plain = new TextDecoder().decode(await subtle.decrypt({ name: 'AES-GCM', iv }, back, sealed));
           record(bits + ' ' + jwk.alg + ' ' + jwk.kty + ' ' + plain + ' ' + sealed.byteLength);
         }",
        &["128 A128GCM oct hello 21", "192 A192GCM oct hello 21", "256 A256GCM oct hello 21"],
    );
}

#[test]
fn key_object_shape_and_export_rules() {
    expect(
        "const key = await subtle.generateKey({ name: 'AES-GCM', length: 256 }, false, ['decrypt', 'encrypt']);
         record([key instanceof CryptoKey, key.type, key.extractable, key.algorithm.name, key.algorithm.length, key.usages.join()].join(' '));
         for (const format of ['raw', 'jwk']) {
           try { await subtle.exportKey(format, key); record('no throw'); } catch (e) { record(e.name); }
         }
         try { await subtle.sign('HMAC', key, new Uint8Array(1)); record('no throw'); } catch (e) { record(e.name); }
         try { await subtle.generateKey({ name: 'AES-GCM', length: 256 }, true, []); record('no throw'); } catch (e) { record(e.name); }
         try { await subtle.generateKey({ name: 'AES-GCM', length: 256 }, true, ['sign']); record('no throw'); } catch (e) { record(e.name); }
         try { await subtle.generateKey({ name: 'AES-GCM', length: 100 }, true, ['encrypt']); record('no throw'); } catch (e) { record(e.name); }",
        &[
            "true secret false AES-GCM 256 encrypt,decrypt",
            "InvalidAccessError",
            "InvalidAccessError",
            "InvalidAccessError",
            "SyntaxError",
            "SyntaxError",
            "OperationError",
        ],
    );
}
