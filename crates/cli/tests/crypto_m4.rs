use assert_cmd::Command;

fn run_script(script: &str) -> assert_cmd::assert::Assert {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("crypto.js");
    std::fs::write(&path, script).unwrap();
    Command::cargo_bin("velyr")
        .unwrap()
        .args(["run"])
        .arg(&path)
        .assert()
}

#[test]
fn random_values_and_uuid_are_available() {
    run_script(
        r#"const a = crypto.getRandomValues(new Uint8Array(16));
           console.log(a.length, /^[0-9a-f-]{36}$/.test(crypto.randomUUID()));
           try { crypto.getRandomValues(new Uint8Array(65537)); } catch (e) { console.log(e.name); }"#,
    )
    .success()
    .stdout("16 true\nQuotaExceededError\n");
}

#[test]
fn digest_matches_the_sha256_vector() {
    run_script(
        r#"(async () => {
             const out = await crypto.subtle.digest("SHA-256", new TextEncoder().encode("abc"));
             console.log([...new Uint8Array(out)].map((b) => b.toString(16).padStart(2, "0")).join(""));
           })();"#,
    )
    .success()
    .stdout("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\n");
}

#[test]
fn rsa_pss_sign_verify_round_trip() {
    run_script(
        r#"(async () => {
             const { subtle } = crypto;
             const alg = { name: "RSA-PSS", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" };
             const pair = await subtle.generateKey(alg, true, ["sign", "verify"]);
             const data = new TextEncoder().encode("hello");
             const sig = await subtle.sign({ name: "RSA-PSS", saltLength: 32 }, pair.privateKey, data);
             console.log(await subtle.verify({ name: "RSA-PSS", saltLength: 32 }, pair.publicKey, sig, data));
           })();"#,
    )
    .success()
    .stdout("true\n");
}

#[test]
fn jwk_export_import_sign_verify_round_trip_and_unsupported_algorithm() {
    run_script(
        r#"(async () => {
             const { subtle } = crypto;
             const alg = { name: "ECDSA", namedCurve: "P-256" };
             const pair = await subtle.generateKey(alg, true, ["sign", "verify"]);
             const priv = await subtle.importKey("jwk", await subtle.exportKey("jwk", pair.privateKey), alg, true, ["sign"]);
             const pub = await subtle.importKey("jwk", await subtle.exportKey("jwk", pair.publicKey), alg, true, ["verify"]);
             const data = new TextEncoder().encode("hello");
             const sig = await subtle.sign({ name: "ECDSA", hash: "SHA-256" }, priv, data);
             console.log(await subtle.verify({ name: "ECDSA", hash: "SHA-256" }, pub, sig, data));
             try { await subtle.generateKey({ name: "Ed448" }, true, ["sign"]); }
             catch (e) { console.log(e.name, e instanceof DOMException); }
             console.log(typeof globalThis.op_crypto_run, typeof globalThis.op_crypto_random);
           })();"#,
    )
    .success()
    .stdout("true\nNotSupportedError true\nundefined undefined\n");
}
