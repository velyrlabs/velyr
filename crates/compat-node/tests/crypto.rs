//! `node:crypto`: known-answer vectors (NIST, RFC 1321/2202/4231/5869/6070/7914/8439) and
//! signature interoperability with keys and signatures made by the local openssl. The fixtures in
//! `tests/fixtures/crypto/` are throwaway test keys. Scripts run through `common::script`.

mod common;

use common::script::{assert_ok, check};
use std::path::Path;

const PRELUDE: &str = r#"
const { Buffer } = globalThis[Symbol.for('velyr.node')].require('buffer');
const crypto = globalThis[Symbol.for('velyr.node')].require('crypto');
const hex = (b) => Buffer.from(b).toString('hex');
const code = (fn, expected) => { try { fn(); } catch (e) { eq(e.code, expected, 'error code'); return e; } throw new Error('did not throw ' + expected); };
"#;

fn run(body: &str) -> String {
    check(&format!("{PRELUDE}{body}"))
}

fn fixture(name: &str) -> Vec<u8> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/crypto");
    std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn fixtures_js() -> String {
    let text = |n: &str| String::from_utf8(fixture(n)).unwrap();
    let hex = |n: &str| fixture(n).iter().map(|b| format!("{b:02x}")).collect::<String>();
    let files = ["rsa-private.pem", "rsa-public.pem", "rsa-private-pkcs1.pem", "ec-private.pem", "ec-public.pem", "ec-private-sec1.pem", "ed25519-private.pem", "ed25519-public.pem"];
    let mut js = String::from("const FIX = {\n");
    for f in files {
        js.push_str(&format!("  {f:?}: {:?},\n", text(f)));
    }
    for f in ["rsa-sha256.sig", "ec-sha256.sig", "ed25519.sig"] {
        js.push_str(&format!("  {f:?}: {:?},\n", hex(f)));
    }
    js.push_str(&format!("  message: {:?},\n}};\n", text("message.txt")));
    js
}

#[test]
fn hashes_match_known_vectors() {
    assert_ok(run(
        r#"const h = (alg, data = 'abc') => crypto.createHash(alg).update(data).digest('hex');
        eq(h('md5'), '900150983cd24fb0d6963f7d28e17f72');
        eq(h('sha1'), 'a9993e364706816aba3e25717850c26c9cd0d89d');
        eq(h('sha224'), '23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7');
        eq(h('sha256'), 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
        eq(h('sha256', ''), 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855');
        eq(h('sha384'), 'cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7');
        eq(h('sha512'), 'ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f');
        eq(h('sha3-256'), '3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532');
        eq(h('SHA256'), h('sha256'), 'case-insensitive');
        const hash = crypto.createHash('sha256');
        hash.update('a'); hash.update(Buffer.from('b')); hash.update(new Uint8Array([99]));
        const copy = hash.copy();
        eq(hash.digest('hex'), h('sha256'));
        eq(copy.update('d').digest('base64'), crypto.createHash('sha256').update('abcd').digest('base64'));
        code(() => hash.update('x'), 'ERR_CRYPTO_HASH_FINALIZED');
        code(() => hash.digest(), 'ERR_CRYPTO_HASH_FINALIZED');
        eq(Buffer.isBuffer(crypto.createHash('md5').update('').digest()), true);
        let err;
        try { crypto.createHash('nope'); } catch (e) { err = e; }
        eq(/Digest method not supported/.test(err.message), true);
        eq(crypto.getHashes().includes('sha256'), true);
        eq(crypto.hash('sha1', 'abc'), 'a9993e364706816aba3e25717850c26c9cd0d89d');"#,
    ));
}

#[test]
fn hmac_matches_rfc_vectors() {
    assert_ok(run(
        r#"const hm = (alg, key, data) => crypto.createHmac(alg, key).update(data).digest('hex');
        eq(hm('sha256', 'Jefe', 'what do ya want for nothing?'), '5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843');
        eq(hm('md5', 'Jefe', 'what do ya want for nothing?'), '750c783e6ab0b503eaa86e310a5db738');
        eq(hm('sha1', 'Jefe', 'what do ya want for nothing?'), 'effcdf6ae5eb2fa2d27416d5f184df9c259a7c79');
        // Keys longer than the block size are hashed first (RFC 4231 case 6).
        eq(hm('sha256', Buffer.alloc(131, 0xaa), 'Test Using Larger Than Block-Size Key - Hash Key First'),
           '60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54');
        eq(hm('sha3-256', 'k', 'd').length, 64);
        code(() => crypto.createHmac('nope', 'k'), 'ERR_CRYPTO_INVALID_DIGEST');"#,
    ));
}

#[test]
fn random_helpers() {
    assert_ok(run(
        r#"eq(crypto.randomBytes(16).length, 16);
        eq(crypto.randomBytes(0).length, 0);
        eq(Buffer.compare(crypto.randomBytes(16), crypto.randomBytes(16)) !== 0, true);
        const buf = Buffer.alloc(8);
        eq(crypto.randomFillSync(buf, 4, 4), buf);
        same([...buf.subarray(0, 4)], [0, 0, 0, 0]);
        eq(await new Promise((r, j) => crypto.randomBytes(4, (e, b) => e ? j(e) : r(b.length))), 4);
        eq(await new Promise((r) => crypto.randomFill(new Uint8Array(4), (e, b) => r(b.length))), 4);
        eq(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(crypto.randomUUID()), true);
        const counts = [0, 0, 0];
        for (let i = 0; i < 3000; i++) counts[crypto.randomInt(3)]++;
        for (const c of counts) eq(c > 800 && c < 1200, true, 'uniform ' + counts);
        for (let i = 0; i < 200; i++) { const n = crypto.randomInt(5, 8); eq(n >= 5 && n < 8, true); }
        eq(await new Promise((r) => crypto.randomInt(10, (e, n) => r(n >= 0 && n < 10))), true);
        code(() => crypto.randomInt(5, 5), 'ERR_OUT_OF_RANGE');
        code(() => crypto.randomInt('a'), 'ERR_INVALID_ARG_TYPE');
        code(() => crypto.randomBytes(-1), 'ERR_OUT_OF_RANGE');
        code(() => crypto.randomBytes(2 ** 31), 'ERR_OUT_OF_RANGE');"#,
    ));
}

#[test]
fn ciphers_match_known_vectors_and_reject_tampering() {
    assert_ok(run(
        r#"const zero = (n) => Buffer.alloc(n);
        // NIST GCM test cases 2 and 14 (AES-128 / AES-256, zero key, IV and block).
        for (const [alg, keyLen, ct, tag] of [
          ['aes-128-gcm', 16, '0388dace60b6a392f328c2b971b2fe78', 'ab6e47d42cec13bdf53a67b21257bddf'],
          ['aes-256-gcm', 32, 'cea7403d4d606b6e074ec5d3baf39d18', 'd0d1c8a799996bf0265b98b5d48ab919'],
        ]) {
          const c = crypto.createCipheriv(alg, zero(keyLen), zero(12));
          const out = Buffer.concat([c.update(zero(16)), c.final()]);
          eq(hex(out), ct, alg);
          eq(hex(c.getAuthTag()), tag, alg + ' tag');
          const d = crypto.createDecipheriv(alg, zero(keyLen), zero(12));
          d.setAuthTag(c.getAuthTag());
          eq(hex(Buffer.concat([d.update(out), d.final()])), '00'.repeat(16));
          // Any flipped bit in the tag or the ciphertext fails and yields no plaintext.
          for (const [badCt, badTag] of [[out, Buffer.from(c.getAuthTag()).fill(1)], [Buffer.from(out).fill(2), c.getAuthTag()]]) {
            const bad = crypto.createDecipheriv(alg, zero(keyLen), zero(12));
            bad.setAuthTag(badTag);
            const first = bad.update(badCt);
            eq(first.length, 0, 'no partial plaintext');
            let err; try { bad.final(); } catch (e) { err = e; }
            eq(/authenticate/.test(err.message), true, 'tag error');
          }
        }
        // AAD binds the tag.
        const key = Buffer.alloc(16, 7), iv = Buffer.alloc(12, 9);
        const enc = crypto.createCipheriv('aes-128-gcm', key, iv); enc.setAAD(Buffer.from('hdr'));
        const ct = Buffer.concat([enc.update('secret'), enc.final()]);
        const dec = crypto.createDecipheriv('aes-128-gcm', key, iv); dec.setAAD(Buffer.from('other')); dec.setAuthTag(enc.getAuthTag());
        dec.update(ct); code(() => dec.final(), undefined);

        // ChaCha20-Poly1305, RFC 8439 section 2.8.2.
        const ck = Buffer.from(Array.from({ length: 32 }, (_, i) => 0x80 + i));
        const cn = Buffer.from('070000004041424344454647', 'hex');
        const c20 = crypto.createCipheriv('chacha20-poly1305', ck, cn, { authTagLength: 16 });
        c20.setAAD(Buffer.from('50515253c0c1c2c3c4c5c6c7', 'hex'));
        const msg = "Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        const c20out = Buffer.concat([c20.update(msg), c20.final()]);
        eq(hex(c20out).slice(0, 32), 'd31a8d34648e60db7b86afbc53ef7ec2');
        eq(hex(c20.getAuthTag()), '1ae10b594f09e26a7e902ecbd0600691');

        // CBC, NIST SP 800-38A F.2.1 (first block), then PKCS#7 round trips for every key size.
        const nk = Buffer.from('2b7e151628aed2a6abf7158809cf4f3c', 'hex');
        const niv = Buffer.from('000102030405060708090a0b0c0d0e0f', 'hex');
        const raw = crypto.createCipheriv('aes-128-cbc', nk, niv).setAutoPadding(false);
        eq(hex(Buffer.concat([raw.update(Buffer.from('6bc1bee22e409f96e93d7e117393172a', 'hex')), raw.final()])), '7649abac8119b246cee98e9b12e9197d');
        for (const bits of [128, 192, 256]) {
          const k = Buffer.alloc(bits / 8, 3), v = Buffer.alloc(16, 5);
          for (const text of ['', 'a', 'sixteen bytes!!!', 'hello world, longer than one block']) {
            const e = crypto.createCipheriv(`aes-${bits}-cbc`, k, v);
            const enc2 = Buffer.concat([e.update(text), e.final()]);
            eq(enc2.length, (Math.floor(text.length / 16) + 1) * 16);
            const d = crypto.createDecipheriv(`aes-${bits}-cbc`, k, v);
            eq(Buffer.concat([d.update(enc2), d.final()]).toString(), text);
          }
        }
        const badPad = crypto.createDecipheriv('aes-128-cbc', nk, niv);
        badPad.update(Buffer.alloc(16, 1));
        let padErr; try { badPad.final(); } catch (e) { padErr = e; }
        eq(/bad decrypt/.test(padErr.message), true);
        eq(crypto.getCiphers().includes('aes-256-gcm'), true);
        code(() => crypto.createCipheriv('aes-128-cbc', Buffer.alloc(5), niv), 'ERR_CRYPTO_INVALID_KEYLEN');
        code(() => crypto.createCipheriv('aes-128-cbc', nk, Buffer.alloc(5)), 'ERR_CRYPTO_INVALID_IV');
        let unk; try { crypto.createCipheriv('rot13', nk, niv); } catch (e) { unk = e; }
        eq(/Unknown cipher/.test(unk.message), true);"#,
    ));
}

#[test]
fn contexts_are_incremental_and_ciphers_return_output_from_update() {
    assert_ok(run(
        r#"const key = Buffer.alloc(16, 1), iv12 = Buffer.alloc(12, 2), iv16 = Buffer.alloc(16, 3);
        const data = Buffer.from('The quick brown fox jumps over the lazy dog, twice over.');
        // GCM encrypt: update() returns ciphertext of the same length; tag matches the one-shot result.
        const gcm = crypto.createCipheriv('aes-128-gcm', key, iv12);
        gcm.setAAD(Buffer.from('header'));
        const head = gcm.update(data.subarray(0, 20));
        eq(head.length, 20, 'gcm update length');
        const sealed = Buffer.concat([head, gcm.update(data.subarray(20)), gcm.final()]);
        const tag = gcm.getAuthTag();
        eq(sealed.length, data.length);
        const open = crypto.createDecipheriv('aes-128-gcm', key, iv12);
        open.setAAD(Buffer.from('header')); open.setAuthTag(tag);
        eq(Buffer.concat([open.update(sealed), open.final()]).toString(), data.toString());
        // CBC: whole blocks come back from update(), in both directions.
        const enc = crypto.createCipheriv('aes-128-cbc', key, iv16);
        eq(enc.update(data.subarray(0, 10)).length, 0);
        const blocks = enc.update(data.subarray(10, 40));
        eq(blocks.length, 32);
        const cbc = Buffer.concat([blocks, enc.update(data.subarray(40)), enc.final()]);
        const dec = crypto.createDecipheriv('aes-128-cbc', key, iv16);
        const first = dec.update(cbc.subarray(0, 48));
        eq(first.length, 32, 'cbc decrypt holds back the last block');
        const rest = dec.update(cbc.subarray(48));
        eq(Buffer.concat([first, rest, dec.final()]).toString(), data.toString());
        // Hash/Hmac in chunks equal the one-shot digest; copy() is independent.
        for (const alg of ['md5', 'sha256', 'sha3-256']) {
          const whole = crypto.createHash(alg).update(data).digest('hex');
          const h = crypto.createHash(alg).update(data.subarray(0, 7));
          const c = h.copy();
          eq(h.update(data.subarray(7)).digest('hex'), whole, alg);
          if (c.update('x').digest('hex') === whole) throw new Error('copy shares state');
          const mac = crypto.createHmac(alg, 'secret');
          mac.update(data.subarray(0, 3)); mac.update(data.subarray(3));
          eq(mac.digest('hex'), crypto.createHmac(alg, 'secret').update(data).digest('hex'), 'hmac ' + alg);
        }
        code(() => enc.update('x'), 'ERR_CRYPTO_INVALID_STATE');
        const late = crypto.createCipheriv('aes-128-cbc', key, iv16);
        late.update('abc');
        code(() => late.setAutoPadding(false), 'ERR_CRYPTO_INVALID_STATE');"#,
    ));
}

#[test]
fn key_derivation_matches_known_vectors() {
    assert_ok(run(
        r#"eq(hex(crypto.pbkdf2Sync('password', 'salt', 1, 20, 'sha1')), '0c60c80f961f0e71f3a9b524af6012062fe037a6');
        eq(hex(crypto.pbkdf2Sync('password', 'salt', 4096, 20, 'sha1')), '4b007901b765489abead49d926f721d065a429c1');
        eq(hex(crypto.pbkdf2Sync('password', 'salt', 1, 32, 'sha256')), '120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b');
        eq(hex(await new Promise((r, j) => crypto.pbkdf2('password', 'salt', 1, 20, 'sha1', (e, k) => e ? j(e) : r(k)))), '0c60c80f961f0e71f3a9b524af6012062fe037a6');
        code(() => crypto.pbkdf2Sync('p', 's', 0, 10, 'sha1'), 'ERR_OUT_OF_RANGE');

        const hk = crypto.hkdfSync('sha256', Buffer.alloc(22, 0x0b), Buffer.from('000102030405060708090a0b0c', 'hex'), Buffer.from('f0f1f2f3f4f5f6f7f8f9', 'hex'), 42);
        eq(hk instanceof ArrayBuffer, true);
        eq(hex(hk), '3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865');
        eq(hex(await new Promise((r, j) => crypto.hkdf('sha256', Buffer.alloc(22, 0x0b), '', '', 16, (e, k) => e ? j(e) : r(k)))).length, 32);
        code(() => crypto.hkdfSync('sha256', 'k', 's', 'i', 255 * 32 + 1), 'ERR_CRYPTO_INVALID_KEYLEN');"#,
    ));
}

#[test]
fn scrypt_matches_rfc_7914_vectors() {
    assert_ok(run(
        r#"eq(hex(crypto.scryptSync('', '', 64, { N: 16, r: 1, p: 1 })),
           '77d6576238657b203b19ca42c18a0497f16b4844e3074ae8dfdffa3fede21442fcd0069ded0948f8326a753a0fc81f17e8d3e0fb2e0d3628cf35e20c38d18906');
        eq(hex(crypto.scryptSync('password', 'NaCl', 64, { N: 1024, r: 8, p: 16, maxmem: 256 * 1024 * 1024 })),
           'fdbabe1c9d3472007856e7190d01e9fe7c6ad7cbc8237830e77376634b3731622eaf30d92e22a3886ff109279d9830dac727afb94a83ee6d8360cbdfa2cc0640');
        eq(hex(await new Promise((r, j) => crypto.scrypt('', '', 16, { N: 16, r: 1, p: 1 }, (e, k) => e ? j(e) : r(k)))), '77d6576238657b203b19ca42c18a0497');
        // Parameters are validated before anything is allocated.
        code(() => crypto.scryptSync('p', 's', 16, { N: 15 }), 'ERR_CRYPTO_INVALID_SCRYPT_PARAMS');
        code(() => crypto.scryptSync('p', 's', 16, { N: 2 ** 30, r: 8 }), 'ERR_CRYPTO_INVALID_SCRYPT_PARAMS');
        code(() => crypto.scryptSync('p', 's', 16, { N: 16384, r: 8, p: 1, maxmem: 1024 }), 'ERR_CRYPTO_INVALID_SCRYPT_PARAMS');
        code(() => crypto.scryptSync('p', 's', 16, { N: 16, r: 2 ** 30, p: 2 ** 30 }), 'ERR_CRYPTO_INVALID_SCRYPT_PARAMS');"#,
    ));
}

#[test]
fn timing_safe_equal_and_webcrypto() {
    assert_ok(run(
        r#"eq(crypto.timingSafeEqual(Buffer.from('abc'), Buffer.from('abc')), true);
        eq(crypto.timingSafeEqual(Buffer.from('abc'), Buffer.from('abd')), false);
        eq(crypto.timingSafeEqual(new Uint8Array([1, 2]), new Uint16Array([513]).buffer), true);
        const e = code(() => crypto.timingSafeEqual(Buffer.from('a'), Buffer.from('ab')), 'ERR_CRYPTO_TIMING_SAFE_EQUAL_LENGTH');
        eq(e instanceof RangeError, true);
        code(() => crypto.timingSafeEqual('a', 'a'), 'ERR_INVALID_ARG_TYPE');

        eq(crypto.webcrypto, globalThis.crypto);
        eq(crypto.subtle, globalThis.crypto.subtle);
        const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode('abc'));
        eq(hex(digest), 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
        eq(crypto.getRandomValues(new Uint8Array(4)).length, 4);"#,
    ));
}

#[test]
fn signatures_interoperate_with_openssl_keys() {
    let body = format!(
        r#"{fixtures}
        const msg = FIX.message;
        const sigOf = (name) => Buffer.from(FIX[name], 'hex');
        // RSA PKCS#1 v1.5 is deterministic: our signature is byte-identical to openssl's.
        for (const key of ['rsa-private.pem', 'rsa-private-pkcs1.pem']) {{
          const s = crypto.createSign('SHA256').update(msg).sign(FIX[key]);
          eq(hex(s), hex(sigOf('rsa-sha256.sig')), key);
        }}
        eq(hex(crypto.sign('sha256', Buffer.from(msg), FIX['rsa-private.pem'])), hex(sigOf('rsa-sha256.sig')));
        eq(crypto.createVerify('SHA256').update(msg).verify(FIX['rsa-public.pem'], sigOf('rsa-sha256.sig')), true);
        eq(crypto.verify('sha256', Buffer.from(msg), FIX['rsa-public.pem'], sigOf('rsa-sha256.sig')), true);
        eq(crypto.createVerify('SHA256').update(msg + '!').verify(FIX['rsa-public.pem'], sigOf('rsa-sha256.sig')), false, 'tampered');
        eq(crypto.createVerify('SHA256').update(msg).verify(FIX['rsa-private.pem'], sigOf('rsa-sha256.sig')), true, 'private key verifies too');

        // ECDSA: verify openssl's DER signature, and round trip our own.
        eq(crypto.createVerify('SHA256').update(msg).verify(FIX['ec-public.pem'], sigOf('ec-sha256.sig')), true);
        for (const key of ['ec-private.pem', 'ec-private-sec1.pem']) {{
          const s = crypto.sign('sha256', Buffer.from(msg), FIX[key]);
          eq(crypto.verify('sha256', Buffer.from(msg), FIX['ec-public.pem'], s), true, key);
          eq(crypto.verify('sha256', Buffer.from(msg + '!'), FIX['ec-public.pem'], s), false);
        }}

        // Ed25519 is deterministic too.
        eq(hex(crypto.sign(null, Buffer.from(msg), FIX['ed25519-private.pem'])), hex(sigOf('ed25519.sig')));
        eq(crypto.verify(null, Buffer.from(msg), FIX['ed25519-public.pem'], sigOf('ed25519.sig')), true);
        eq(crypto.verify(null, Buffer.from(msg + '!'), FIX['ed25519-public.pem'], sigOf('ed25519.sig')), false);

        // KeyObject: parse, inspect, export, derive the public half.
        const priv = crypto.createPrivateKey(FIX['rsa-private.pem']);
        eq(priv instanceof crypto.KeyObject, true);
        eq(priv.type, 'private');
        eq(priv.asymmetricKeyType, 'rsa');
        eq(crypto.createPublicKey(priv).export({{ type: 'spki', format: 'pem' }}), FIX['rsa-public.pem']);
        eq(crypto.createPublicKey(FIX['ec-public.pem']).asymmetricKeyType, 'ec');
        eq(crypto.createPrivateKey(FIX['ed25519-private.pem']).asymmetricKeyType, 'ed25519');
        eq(crypto.createPrivateKey(FIX['ec-private-sec1.pem']).export({{ type: 'pkcs8', format: 'pem' }}), FIX['ec-private.pem']);
        const der = priv.export({{ type: 'pkcs8', format: 'der' }});
        eq(crypto.createPrivateKey({{ key: der, format: 'der', type: 'pkcs8' }}).type, 'private');
        eq(crypto.createSecretKey(Buffer.from('k')).type, 'secret');
        code(() => crypto.createPublicKey('not a key'), undefined);
        code(() => crypto.createPrivateKey(FIX['rsa-public.pem']), undefined);

        // Generated pairs sign and verify, and export as parseable PEM.
        for (const [type, opts] of [['ed25519', undefined], ['ec', {{ namedCurve: 'P-256' }}], ['rsa', {{ modulusLength: 2048 }}]]) {{
          const pair = crypto.generateKeyPairSync(type, opts);
          const s = crypto.sign(type === 'ed25519' ? null : 'sha256', Buffer.from(msg), pair.privateKey);
          eq(crypto.verify(type === 'ed25519' ? null : 'sha256', Buffer.from(msg), pair.publicKey, s), true, type);
          const pem = pair.publicKey.export({{ type: 'spki', format: 'pem' }});
          eq(pem.startsWith('-----BEGIN PUBLIC KEY-----'), true);
          eq(crypto.createPublicKey(pem).asymmetricKeyType, type);
        }}
        const asyncPair = await new Promise((r, j) => crypto.generateKeyPair('ed25519', {{}}, (e, pub, priv2) => e ? j(e) : r([pub, priv2])));
        eq(asyncPair[0].type, 'public');"#,
        fixtures = fixtures_js()
    );
    assert_ok(run(&body));
}

#[test]
fn jwk_der_pss_and_async_signing() {
    let body = format!(
        r#"{fixtures}
        const msg = Buffer.from(FIX.message);
        // JWK round trips keep the key identical, for every supported type.
        for (const name of ['rsa-private.pem', 'ec-private.pem', 'ed25519-private.pem']) {{
          const priv = crypto.createPrivateKey(FIX[name]);
          const jwk = priv.export({{ format: 'jwk' }});
          eq(crypto.createPrivateKey({{ key: jwk, format: 'jwk' }}).equals(priv), true, name);
          const pub = crypto.createPublicKey(priv);
          const pubJwk = pub.export({{ format: 'jwk' }});
          eq('d' in pubJwk, false);
          eq(crypto.createPublicKey({{ key: pubJwk, format: 'jwk' }}).equals(pub), true, name + ' public');
        }}
        eq(crypto.createPrivateKey(FIX['ec-private.pem']).export({{ format: 'jwk' }}).crv, 'P-256');
        eq(crypto.createPrivateKey(FIX['ed25519-private.pem']).export({{ format: 'jwk' }}).kty, 'OKP');
        eq(crypto.createPrivateKey(FIX['rsa-private.pem']).asymmetricKeyDetails.modulusLength, 2048);
        // DER and PKCS#1/SEC1 exports parse back.
        const rsa = crypto.createPrivateKey(FIX['rsa-private.pem']);
        eq(rsa.export({{ type: 'pkcs1', format: 'pem' }}), FIX['rsa-private-pkcs1.pem']);
        const spki = crypto.createPublicKey(rsa).export({{ type: 'spki', format: 'der' }});
        eq(crypto.createPublicKey({{ key: spki, format: 'der', type: 'spki' }}).asymmetricKeyType, 'rsa');
        const sec1 = crypto.createPrivateKey(FIX['ec-private.pem']).export({{ type: 'sec1', format: 'pem' }});
        eq(sec1, FIX['ec-private-sec1.pem']);
        // PSS signatures round trip (salt length = digest length) and fail on tampering.
        const pss = {{ key: FIX['rsa-private.pem'], padding: crypto.constants.RSA_PKCS1_PSS_PADDING }};
        const sig = crypto.sign('sha256', msg, pss);
        eq(crypto.verify('sha256', msg, {{ key: FIX['rsa-public.pem'], padding: 6 }}, sig), true);
        eq(crypto.verify('sha256', msg, FIX['rsa-public.pem'], sig), false, 'pss is not pkcs1');
        // Async sign and verify run off the isolate thread.
        const asyncSig = await new Promise((r, j) => crypto.sign('sha256', msg, FIX['ec-private.pem'], (e, s) => e ? j(e) : r(s)));
        eq(await new Promise((r, j) => crypto.verify('sha256', msg, FIX['ec-public.pem'], asyncSig, (e, ok) => e ? j(e) : r(ok))), true);
        // HMAC/secret keys.
        const secret = crypto.generateKeySync('hmac', {{ length: 256 }});
        eq(secret.symmetricKeySize, 32);
        eq(crypto.createHmac('sha256', secret).update('x').digest('hex').length, 64);"#,
        fixtures = fixtures_js()
    );
    assert_ok(run(&body));
}
