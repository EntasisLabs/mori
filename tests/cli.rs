use std::fs;
use std::process::Command;

use tempfile::tempdir;

fn mori(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mori"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run mori")
}

fn assert_ok(output: &std::process::Output) {
    if !output.status.success() {
        panic!(
            "mori failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn compile_does_not_open_the_database() {
    let dir = tempdir().unwrap();
    let notes = dir.path().join("notes.md");
    fs::write(&notes, "the orchard plan waits on the north fence\n").unwrap();

    let output = mori(dir.path(), &["compile", "notes.md", "--kind", "document"]);
    assert_ok(&output);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("⊕⟨"), "{stdout}");
    assert!(stdout.contains("orchard"), "{stdout}");
    assert!(!dir.path().join(".mori").exists());
}

#[test]
fn commit_then_recall_survives_disconnect() {
    let dir = tempdir().unwrap();
    let notes = dir.path().join("notes.md");
    fs::write(&notes, "the orchard plan waits on the north fence\n").unwrap();

    assert_ok(&mori(dir.path(), &["init"]));
    assert_ok(&mori(
        dir.path(),
        &["add", "notes.md", "--kind", "document"],
    ));

    let status = mori(dir.path(), &["status"]);
    assert_ok(&status);
    let status_text = String::from_utf8(status.stdout).unwrap();
    assert!(status_text.contains("notes.md"), "{status_text}");
    assert!(status_text.contains("no commits yet"), "{status_text}");

    assert_ok(&mori(dir.path(), &["commit", "-m", "remember the orchard"]));

    let log = mori(dir.path(), &["log"]);
    assert_ok(&log);
    let log_text = String::from_utf8(log.stdout).unwrap();
    assert!(log_text.contains("remember the orchard"), "{log_text}");
    assert!(log_text.contains("notes.md"), "{log_text}");
    assert!(!log_text.contains("⊕⟨"), "{log_text}");

    let recall = mori(dir.path(), &["recall", "orchard plan"]);
    assert_ok(&recall);
    let recall_text = String::from_utf8(recall.stdout).unwrap();
    assert!(recall_text.contains("orchard"), "{recall_text}");
    assert!(!recall_text.contains("⊕⟨"), "{recall_text}");

    let found = mori(dir.path(), &["find", "--contains", "fence"]);
    assert_ok(&found);
    let found_text = String::from_utf8(found.stdout).unwrap();
    assert!(
        found_text.contains("fence") || found_text.contains("orchard"),
        "{found_text}"
    );
}

#[test]
fn chat_and_raw_sttp_can_both_be_stored() {
    let dir = tempdir().unwrap();
    assert_ok(&mori(dir.path(), &["init", "--session", "design"]));

    let chat = dir.path().join("thread.json");
    fs::write(
        &chat,
        r#"[{"role":"user","content":"where did we leave the orchard plan"},{"role":"assistant","content":"it waits on the north fence"}]"#,
    )
    .unwrap();
    assert_ok(&mori(dir.path(), &["add", "thread.json"]));
    assert_ok(&mori(dir.path(), &["commit", "-m", "save the chat"]));

    let compiled = mori(dir.path(), &["compile", "notes-missing"]);
    // compile of a missing file should fail and must not be required for the chat path
    assert!(!compiled.status.success());

    let note = dir.path().join("scratch.md");
    fs::write(&note, "bring cedar stakes for the north fence\n").unwrap();
    let compiled = mori(dir.path(), &["compile", "scratch.md"]);
    assert_ok(&compiled);
    let sttp = String::from_utf8(compiled.stdout).unwrap();
    let sttp_path = dir.path().join("scratch.sttp");
    fs::write(&sttp_path, &sttp).unwrap();

    assert_ok(&mori(
        dir.path(),
        &["add", "scratch.sttp", "--kind", "sttp"],
    ));
    assert_ok(&mori(dir.path(), &["commit", "-m", "store compiled note"]));

    let recall = mori(dir.path(), &["recall", "cedar stakes"]);
    assert_ok(&recall);
    let text = String::from_utf8(recall.stdout).unwrap();
    assert!(text.contains("cedar"), "{text}");

    let log = mori(dir.path(), &["log", "-n", "5"]);
    assert_ok(&log);
    let log_text = String::from_utf8(log.stdout).unwrap();
    assert!(log_text.contains("save the chat"), "{log_text}");
    assert!(log_text.contains("store compiled note"), "{log_text}");
}
