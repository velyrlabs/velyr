//! Interoperability with DER produced by OpenSSL 4.0.3, so the SPKI/PKCS#8 encodings are pinned to something other
//! than this crate's own encoder. Reference material was generated with:
//!   openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:<prime256v1|secp384r1> -pkeyopt ec_param_enc:named_curve
//!   openssl genpkey -algorithm <ED25519|X25519>
//!   openssl pkey -pubout -outform DER ; openssl pkcs8 -topk8 -nocrypt -outform DER
//!   printf sample > msg; openssl dgst -sha384 -sign P-384.pem msg   (DER signature converted to r || s)
mod common;
use common::expect_with_helpers as expect;

const OPENSSL: [(&str, &str, &str, &str); 4] = [
    (
        "{ name: 'ECDSA', namedCurve: 'P-256' }",
        "['verify']",
        "3059301306072a8648ce3d020106082a8648ce3d030107034200045a79090bfde4a28e2c46353efe053612efed76a0eb229344ad8fc6e5f4eafccb507630fd63bd78078a96ad2f4d57a9b318ef3e5c9ea98f01073dd5122cc6c733",
        "308187020100301306072a8648ce3d020106082a8648ce3d030107046d306b020101042091acfcec5923e6d14eed367bd57426780eec52f0f3629abfec0d45b7e9ae6532a144034200045a79090bfde4a28e2c46353efe053612efed76a0eb229344ad8fc6e5f4eafccb507630fd63bd78078a96ad2f4d57a9b318ef3e5c9ea98f01073dd5122cc6c733",
    ),
    (
        "{ name: 'ECDSA', namedCurve: 'P-384' }",
        "['verify']",
        "3076301006072a8648ce3d020106052b8104002203620004f9b5186321d08375f00dfb32c9720fd184d7042c19f0f44df3dcc6e9d0992c295e7a24a948db7618b295915666301aaf77267736eba97260dbd082405d45300985917e7ebf3c9ce8bbd82c8cf9073a38c2ee744fbfa2519a7d6e5b051d0615e5",
        "3081b6020100301006072a8648ce3d020106052b8104002204819e30819b02010104301285ebad10e1f2746538419a2eb864d31be97fb8d5cc3badc9ad29e347d4b08e0b4e38494aad8ceed32f41c35c16ef0aa16403620004f9b5186321d08375f00dfb32c9720fd184d7042c19f0f44df3dcc6e9d0992c295e7a24a948db7618b295915666301aaf77267736eba97260dbd082405d45300985917e7ebf3c9ce8bbd82c8cf9073a38c2ee744fbfa2519a7d6e5b051d0615e5",
    ),
    (
        "{ name: 'Ed25519' }",
        "['verify']",
        "302a300506032b6570032100861c4dbe06b3b21c1d6af35c28cea9198e0ae8f44b8241fb80174fe36e306df4",
        "302e020100300506032b6570042204208583a5ab7f09826b5af6944ca2b64b18ad15b7ac0cd44f8e9b4fb9baed51fd94",
    ),
    (
        "{ name: 'X25519' }",
        "[]",
        "302a300506032b656e032100472c67300f259e74cd2927932c09be46caaabd1703d5ae016ed86822be793e5e",
        "302e020100300506032b656e04220420f0e2d72e416aad261115a648731e813f97b924ab2a0fd7da0f49ba55916bfe4c",
    ),
];

#[test]
fn spki_and_pkcs8_match_openssl_byte_for_byte() {
    for (alg, public_usages, spki, pkcs8) in OPENSSL {
        let private_usages = if alg.contains("X25519") {
            "['deriveBits']"
        } else {
            "['sign']"
        };
        expect(
            &format!(
                "const alg = {alg};
                 const pub = await subtle.importKey('spki', unhex('{spki}'), alg, true, {public_usages});
                 const priv = await subtle.importKey('pkcs8', unhex('{pkcs8}'), alg, true, {private_usages});
                 record(String(hex(await subtle.exportKey('spki', pub)) === '{spki}'));
                 record(String(hex(await subtle.exportKey('pkcs8', priv)) === '{pkcs8}'));"
            ),
            &["true", "true"],
        );
    }
}

#[test]
fn ecdsa_p384_verifies_an_openssl_signature() {
    expect(
        "const alg = { name: 'ECDSA', namedCurve: 'P-384' }, sign = { name: 'ECDSA', hash: 'SHA-384' };
         const pub = await subtle.importKey('spki', unhex('3076301006072a8648ce3d020106052b8104002203620004f9b5186321d08375f00dfb32c9720fd184d7042c19f0f44df3dcc6e9d0992c295e7a24a948db7618b295915666301aaf77267736eba97260dbd082405d45300985917e7ebf3c9ce8bbd82c8cf9073a38c2ee744fbfa2519a7d6e5b051d0615e5'), alg, true, ['verify']);
         const sig = unhex('4eac924f0f07627b2c9eedd990a0cdd64acfc954e974a62a047be89aca734b0080f1716770d154b06e6e5dc99c19e9d2aa3334b2f3c5cbcfe42fac16caa1c3086c21a546f30a1e165e6582c793a89a9ab01b3782c1892fe283ce17ab6612358d');
         record(String(await subtle.verify(sign, pub, sig, utf8('sample'))));
         record(String(await subtle.verify(sign, pub, sig, utf8('sampl3'))));",
        &["true", "false"],
    );
}
