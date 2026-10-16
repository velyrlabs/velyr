use assert_cmd::Command;

fn run_script(script: &str) -> assert_cmd::assert::Assert {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("streams.js");
    std::fs::write(&path, script).unwrap();
    Command::cargo_bin("velyr")
        .unwrap()
        .args(["run"])
        .arg(&path)
        .assert()
}

#[test]
fn streams_are_available_and_pipe_through_text_decoder_stream() {
    run_script(
        r#"console.log(typeof ReadableStream);
           (async () => {
             const bytes = new TextEncoder().encode("héllo");
             const stream = new ReadableStream({
               start(c) { c.enqueue(bytes.slice(0, 2)); c.enqueue(bytes.slice(2)); c.close(); },
             }).pipeThrough(new TextDecoderStream());
             let text = "";
             for await (const part of stream) text += part;
             console.log(text);
           })();"#,
    )
    .success()
    .stdout("function\nh\u{e9}llo\n");
}
