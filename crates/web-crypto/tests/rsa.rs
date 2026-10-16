//! VELYR-142: RSA-PSS, RSASSA-PKCS1-v1_5 and RSA-OAEP. The reference keys, signatures and ciphertexts come from
//! OpenSSL 4.0.3 (see `common/fixtures.rs`); PKCS#1 v1.5 signing is deterministic, so it is compared byte for byte.
mod common;
use common::expect_with_helpers as expect;
use common::fixtures::{
    RSA_JWK, RSA_OAEP_SHA256_CT, RSA_OAEP_SHA256_LABEL_LBL_CT, RSA_PKCS1_SHA256_SIG, RSA_PKCS8,
    RSA_PSS_SHA256_SALT32_SIG, RSA_SPKI,
};

/// JS prelude: the three algorithm identifiers over SHA-256 and the OpenSSL key pair imported for each.
fn prelude() -> String {
    format!(
        "const PSS = {{ name: 'RSA-PSS', hash: 'SHA-256' }}, PKCS1 = {{ name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' }}, OAEP = {{ name: 'RSA-OAEP', hash: 'SHA-256' }};
         const RSA_SPKI = '{RSA_SPKI}', RSA_PKCS8 = '{RSA_PKCS8}';
         const spki = unhex(RSA_SPKI), pkcs8 = unhex(RSA_PKCS8), jwk = {RSA_JWK};
         const pair = async (alg, pubUsages, privUsages) => ({{
           publicKey: await subtle.importKey('spki', spki, alg, true, pubUsages),
           privateKey: await subtle.importKey('pkcs8', pkcs8, alg, true, privUsages),
         }});"
    )
}

fn run(body: &str, expected: &[&str]) {
    expect(&format!("{}\n{body}", prelude()), expected);
}

#[test]
fn spki_pkcs8_and_jwk_match_openssl_for_all_three_algorithms() {
    run(
        "for (const [alg, pub, priv] of [[PSS, ['verify'], ['sign']], [PKCS1, ['verify'], ['sign']], [OAEP, ['encrypt'], ['decrypt']]]) {
           const k = await pair(alg, pub, priv);
           const exported = await subtle.exportKey('jwk', k.privateKey);
           const { alg: jwkAlg, key_ops, ext, ...fields } = exported;
           const fromJwk = await subtle.importKey('jwk', { ...jwk, alg: jwkAlg }, alg, true, priv);
           record([alg.name, jwkAlg, hex(await subtle.exportKey('spki', k.publicKey)) === hex(spki),
             hex(await subtle.exportKey('pkcs8', k.privateKey)) === hex(pkcs8),
             JSON.stringify(fields) === JSON.stringify(jwk),
             hex(await subtle.exportKey('pkcs8', fromJwk)) === hex(pkcs8),
             k.privateKey.algorithm.modulusLength, hex(k.privateKey.algorithm.publicExponent), k.privateKey.algorithm.hash.name].join(' '));
         }",
        &[
            "RSA-PSS PS256 true true true true 2048 010001 SHA-256",
            "RSASSA-PKCS1-v1_5 RS256 true true true true 2048 010001 SHA-256",
            "RSA-OAEP RSA-OAEP-256 true true true true 2048 010001 SHA-256",
        ],
    );
}

#[test]
fn rsa_pss_verifies_an_openssl_signature_and_signs_back() {
    run(
        &format!(
            "const k = await pair(PSS, ['verify'], ['sign']);
             const params = {{ name: 'RSA-PSS', saltLength: 32 }};
             const sig = unhex('{RSA_PSS_SHA256_SALT32_SIG}');
             record(String(await subtle.verify(params, k.publicKey, sig, utf8('sample'))));
             record(String(await subtle.verify(params, k.publicKey, sig, utf8('sampl3'))));
             const mine = await subtle.sign(params, k.privateKey, utf8('sample'));
             record(mine.byteLength + ' ' + await subtle.verify(params, k.publicKey, mine, utf8('sample')));
             for (const saltLength of [20, 0]) {{
               try {{ await subtle.verify({{ name: 'RSA-PSS', saltLength }}, k.publicKey, sig, utf8('sample')); record('no throw'); }}
               catch (e) {{ record(e.name); }}
             }}"
        ),
        &["true", "false", "256 true", "NotSupportedError", "NotSupportedError"],
    );
}

#[test]
fn rsassa_pkcs1_matches_openssl_deterministically() {
    run(
        &format!(
            "const k = await pair(PKCS1, ['verify'], ['sign']);
             const sig = await subtle.sign('RSASSA-PKCS1-v1_5', k.privateKey, utf8('sample'));
             record(String(hex(sig) === '{RSA_PKCS1_SHA256_SIG}'));
             record(String(await subtle.verify('RSASSA-PKCS1-v1_5', k.publicKey, unhex('{RSA_PKCS1_SHA256_SIG}'), utf8('sample'))));
             record(String(await subtle.verify('RSASSA-PKCS1-v1_5', k.publicKey, unhex('{RSA_PKCS1_SHA256_SIG}'), utf8('Sample'))));"
        ),
        &["true", "true", "false"],
    );
}

#[test]
fn rsa_oaep_decrypts_openssl_ciphertexts_with_and_without_label() {
    run(
        &format!(
            "const k = await pair(OAEP, ['encrypt'], ['decrypt']);
             const text = (b) => new TextDecoder().decode(b);
             record(text(await subtle.decrypt('RSA-OAEP', k.privateKey, unhex('{RSA_OAEP_SHA256_CT}'))));
             record(text(await subtle.decrypt({{ name: 'RSA-OAEP', label: utf8('lbl') }}, k.privateKey, unhex('{RSA_OAEP_SHA256_LABEL_LBL_CT}'))));
             for (const [params, ct] of [[{{ name: 'RSA-OAEP', label: utf8('other') }}, '{RSA_OAEP_SHA256_LABEL_LBL_CT}'], [{{ name: 'RSA-OAEP' }}, '{RSA_OAEP_SHA256_LABEL_LBL_CT}']]) {{
               try {{ await subtle.decrypt(params, k.privateKey, unhex(ct)); record('no throw'); }} catch (e) {{ record(e.name); }}
             }}
             const sealed = await subtle.encrypt({{ name: 'RSA-OAEP', label: utf8('x') }}, k.publicKey, utf8('round trip'));
             record(sealed.byteLength + ' ' + text(await subtle.decrypt({{ name: 'RSA-OAEP', label: utf8('x') }}, k.privateKey, sealed)));"
        ),
        &["attack at dawn", "attack at dawn", "OperationError", "OperationError", "256 round trip"],
    );
}

#[test]
fn rsa_oaep_supports_every_hash_and_wraps_keys() {
    run(
        "for (const hash of ['SHA-1', 'SHA-256', 'SHA-384', 'SHA-512']) {
           const alg = { name: 'RSA-OAEP', hash };
           const k = await pair(alg, ['encrypt', 'wrapKey'], ['decrypt', 'unwrapKey']);
           const sealed = await subtle.encrypt('RSA-OAEP', k.publicKey, utf8('hash ' + hash));
           record(hash + ' ' + new TextDecoder().decode(await subtle.decrypt('RSA-OAEP', k.privateKey, sealed)) + ' ' + (await subtle.exportKey('jwk', k.publicKey)).alg);
         }
         const k = await pair(OAEP, ['wrapKey'], ['unwrapKey']);
         const aes = await subtle.generateKey({ name: 'AES-GCM', length: 256 }, true, ['encrypt', 'decrypt']);
         const iv = new Uint8Array(12), sealed = await subtle.encrypt({ name: 'AES-GCM', iv }, aes, utf8('payload'));
         for (const format of ['raw', 'jwk']) {
           const wrapped = await subtle.wrapKey(format, aes, k.publicKey, 'RSA-OAEP');
           const back = await subtle.unwrapKey(format, wrapped, k.privateKey, 'RSA-OAEP', 'AES-GCM', false, ['decrypt']);
           record(format + ' ' + wrapped.byteLength + ' ' + new TextDecoder().decode(await subtle.decrypt({ name: 'AES-GCM', iv }, back, sealed)));
         }",
        &[
            "SHA-1 hash SHA-1 RSA-OAEP",
            "SHA-256 hash SHA-256 RSA-OAEP-256",
            "SHA-384 hash SHA-384 RSA-OAEP-384",
            "SHA-512 hash SHA-512 RSA-OAEP-512",
            "raw 256 payload",
            "jwk 256 payload",
        ],
    );
}

#[test]
fn generate_key_splits_usages_and_round_trips_through_jwk() {
    run(
        "const gen = { modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: 'SHA-256' };
         for (const [name, usages, check] of [['RSA-PSS', ['sign', 'verify'], 'sign'], ['RSASSA-PKCS1-v1_5', ['verify', 'sign'], 'sign'], ['RSA-OAEP', ['encrypt', 'decrypt', 'wrapKey', 'unwrapKey'], 'encrypt']]) {
           const k = await subtle.generateKey({ name, ...gen }, true, usages);
           record([name, k.publicKey.type, k.publicKey.usages.join(), k.privateKey.type, k.privateKey.usages.join(), k.publicKey.extractable, k.privateKey.algorithm.modulusLength].join(' '));
           const priv = await subtle.importKey('jwk', await subtle.exportKey('jwk', k.privateKey), { name, hash: 'SHA-256' }, true, k.privateKey.usages);
           const pub = await subtle.importKey('jwk', await subtle.exportKey('jwk', k.publicKey), { name, hash: 'SHA-256' }, true, k.publicKey.usages);
           if (check === 'sign') {
             const params = { name, saltLength: 32 };
             record('sign ' + await subtle.verify(params, pub, await subtle.sign(params, priv, utf8('m')), utf8('m')));
           } else {
             record('oaep ' + new TextDecoder().decode(await subtle.decrypt(name, priv, await subtle.encrypt(name, pub, utf8('m')))));
           }
         }",
        &[
            "RSA-PSS public verify private sign true 2048",
            "sign true",
            "RSASSA-PKCS1-v1_5 public verify private sign true 2048",
            "sign true",
            "RSA-OAEP public encrypt,wrapKey private decrypt,unwrapKey true 2048",
            "oaep m",
        ],
    );
}

#[test]
fn jwk_import_validates_members_and_formats() {
    run(
        "const { p, ...noP } = jwk;
         const cases = {
           'missing crt member': [noP, PSS, ['sign']],
           'wrong kty': [{ ...jwk, kty: 'EC' }, PSS, ['sign']],
           'alg of another scheme': [{ ...jwk, alg: 'RS256' }, PSS, ['sign']],
           'alg of another hash': [{ ...jwk, alg: 'PS384' }, PSS, ['sign']],
           'oth member': [{ ...jwk, oth: [] }, PSS, ['sign']],
           'ext false vs extractable': [{ ...jwk, ext: false }, PSS, ['sign']],
           'public usages on private jwk': [jwk, PSS, ['verify']],
         };
         for (const [label, [data, alg, usages]] of Object.entries(cases)) {
           try { await subtle.importKey('jwk', data, alg, true, usages); record(label + ' -> ok'); } catch (e) { record(label + ' -> ' + e.name); }
         }
         try { await subtle.importKey('raw', new Uint8Array(8), PSS, true, ['verify']); record('raw -> ok'); } catch (e) { record('raw -> ' + e.name); }
         try { await subtle.importKey('spki', new Uint8Array(8), PSS, true, ['verify']); record('garbage spki -> ok'); } catch (e) { record('garbage spki -> ' + e.name); }
         try { await subtle.importKey('spki', spki, PSS, true, ['sign']); record('spki sign -> ok'); } catch (e) { record('spki sign -> ' + e.name); }",
        &[
            "missing crt member -> DataError",
            "wrong kty -> DataError",
            "alg of another scheme -> DataError",
            "alg of another hash -> DataError",
            "oth member -> NotSupportedError",
            "ext false vs extractable -> DataError",
            "public usages on private jwk -> SyntaxError",
            "raw -> NotSupportedError",
            "garbage spki -> DataError",
            "spki sign -> SyntaxError",
        ],
    );
}
