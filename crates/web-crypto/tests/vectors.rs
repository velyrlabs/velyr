//! Known-answer tests: RFC 4231 (HMAC), McGrew-Viega (AES-GCM), RFC 6979 A.2.5 (ECDSA P-256), RFC 8032 (Ed25519).
mod common;
use common::expect_with_helpers as expect;

#[test]
fn hmac_sha256_rfc4231_case_2() {
    expect(
        "const key = await subtle.importKey('raw', utf8('Jefe'), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign', 'verify']);
         const data = utf8('what do ya want for nothing?');
         const mac = await subtle.sign('HMAC', key, data);
         record(hex(mac));
         record(String(await subtle.verify('HMAC', key, mac, data)));
         record(String(await subtle.verify('HMAC', key, new Uint8Array(32), data)));",
        &[
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            "true",
            "false",
        ],
    );
}

const GCM_KEY: &str = "feffe9928665731c6d6a8f9467308308";
const GCM_IV: &str = "cafebabefacedbaddecaf888";
const GCM_PLAIN: &str = "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b39";
const GCM_AAD: &str = "feedfacedeadbeeffeedfacedeadbeefabaddad2";
const GCM_CIPHER_TAG: &str = "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e0915bc94fbc3221a5db94fae95ae7121a47";

#[test]
fn aes_gcm_mcgrew_viega_case_4() {
    expect(
        &format!(
            "const key = await subtle.importKey('raw', unhex('{GCM_KEY}'), 'AES-GCM', false, ['encrypt', 'decrypt']);
             const params = {{ name: 'AES-GCM', iv: unhex('{GCM_IV}'), additionalData: unhex('{GCM_AAD}') }};
             const sealed = await subtle.encrypt(params, key, unhex('{GCM_PLAIN}'));
             record(hex(sealed));
             record(hex(await subtle.decrypt(params, key, sealed)));
             const bad = new Uint8Array(sealed); bad[0] ^= 1;
             try {{ await subtle.decrypt(params, key, bad); record('no throw'); }} catch (e) {{ record(e.name); }}"
        ),
        &[GCM_CIPHER_TAG, GCM_PLAIN, "OperationError"],
    );
}

#[test]
fn aes_gcm_empty_message_case_1() {
    expect(
        "const key = await subtle.importKey('raw', new Uint8Array(16), 'AES-GCM', false, ['encrypt']);
         record(hex(await subtle.encrypt({ name: 'AES-GCM', iv: new Uint8Array(12) }, key, new Uint8Array(0))));",
        &["58e2fccefa7e3061367f1d57a4e7455a"],
    );
}

const P256_X: &str = "60fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb6";
const P256_Y: &str = "7903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299";
const P256_D: &str = "c9afa9d845ba75166b5c215767b1d6934e50c3db36e89b127b8a622b120f6721";
const P256_SIG: &str = "efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8";

#[test]
fn ecdsa_p256_verifies_rfc6979_signature_and_signs_back() {
    expect(
        &format!(
            "const alg = {{ name: 'ECDSA', namedCurve: 'P-256' }}, sign = {{ name: 'ECDSA', hash: 'SHA-256' }};
             const pub = await subtle.importKey('jwk', {{ kty: 'EC', crv: 'P-256', x: b64u(unhex('{P256_X}')), y: b64u(unhex('{P256_Y}')) }}, alg, true, ['verify']);
             const msg = utf8('sample'), sig = unhex('{P256_SIG}');
             record(String(await subtle.verify(sign, pub, sig, msg)));
             record(String(await subtle.verify(sign, pub, sig, utf8('Sample'))));
             const priv = await subtle.importKey('jwk', {{ kty: 'EC', crv: 'P-256', x: b64u(unhex('{P256_X}')), y: b64u(unhex('{P256_Y}')), d: b64u(unhex('{P256_D}')) }}, alg, true, ['sign']);
             const mine = await subtle.sign(sign, priv, msg);
             record(mine.byteLength + ' ' + await subtle.verify(sign, pub, mine, msg));
             const spki = hex(await subtle.exportKey('spki', pub));
             const again = await subtle.importKey('spki', unhex(spki), alg, true, ['verify']);
             record(String(await subtle.verify(sign, again, sig, msg)));"
        ),
        &["true", "false", "64 true", "true"],
    );
}

#[test]
fn ed25519_rfc8032_test_1() {
    expect(
        "const seed = '9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60';
         const pkcs8 = unhex('302e020100300506032b657004220420' + seed);
         const priv = await subtle.importKey('pkcs8', pkcs8, 'Ed25519', true, ['sign']);
         const sig = await subtle.sign('Ed25519', priv, new Uint8Array(0));
         record(hex(sig));
         const pub = await subtle.importKey('raw', unhex('d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a'), 'Ed25519', true, ['verify']);
         record(String(await subtle.verify('Ed25519', pub, sig, new Uint8Array(0))));
         record(String(await subtle.verify('Ed25519', pub, sig, new Uint8Array(1))));
         record(hex(await subtle.exportKey('pkcs8', priv)) === hex(pkcs8) ? 'pkcs8 same' : 'pkcs8 differs');",
        &[
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
            "true",
            "false",
            "pkcs8 same",
        ],
    );
}
