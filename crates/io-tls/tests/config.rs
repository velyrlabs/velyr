//! The public rustls config builders share their validation with the `Velyr.*Tls` ops.

use velyr_io_tls::{client_config, server_config};
use velyr_runtime_errors::ErrorCode;

fn pems() -> (String, String) {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    (cert.pem(), signing_key.serialize_pem())
}

#[test]
fn server_config_carries_the_alpn_list() {
    let (cert, key) = pems();
    let config = server_config(&cert, &key, vec![b"h2".to_vec()]).unwrap();
    assert_eq!(config.alpn_protocols, vec![b"h2".to_vec()]);
}

#[test]
fn server_config_rejects_bad_input() {
    let (cert, key) = pems();
    let (_, other_key) = pems();
    assert!(server_config("", &key, vec![]).is_err());
    assert!(server_config(&cert, "not a key", vec![]).is_err());
    let mismatch = server_config(&cert, &other_key, vec![]).unwrap_err();
    assert_eq!(mismatch.code(), ErrorCode::InvalidData);
}

#[test]
fn client_config_accepts_extra_roots_and_alpn() {
    let (cert, _) = pems();
    let config = client_config(&[cert.as_str()], vec![b"http/1.1".to_vec()]).unwrap();
    assert_eq!(config.alpn_protocols, vec![b"http/1.1".to_vec()]);
    assert!(client_config(&[], vec![]).is_ok());
}

#[test]
fn client_config_rejects_an_invalid_ca_pem() {
    let garbage = "-----BEGIN CERTIFICATE-----\n!!!\n-----END CERTIFICATE-----\n";
    assert!(client_config(&[garbage], vec![]).is_err());
}
