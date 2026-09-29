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

#[test]
fn branches_keep_recall_on_their_own_history() {
    let dir = tempdir().unwrap();
    assert_ok(&mori(dir.path(), &["init"]));

    fs::write(
        dir.path().join("orchard.md"),
        "the orchard plan waits on the north fence\n",
    )
    .unwrap();
    assert_ok(&mori(dir.path(), &["add", "orchard.md"]));
    assert_ok(&mori(dir.path(), &["commit", "-m", "remember the orchard"]));

    assert_ok(&mori(dir.path(), &["switch", "-c", "kiln"]));
    fs::write(dir.path().join("kiln.md"), "the kiln runs hot at dusk\n").unwrap();
    assert_ok(&mori(dir.path(), &["add", "kiln.md"]));
    assert_ok(&mori(dir.path(), &["commit", "-m", "note the kiln"]));

    let on_kiln = mori(dir.path(), &["recall", "kiln dusk"]);
    assert_ok(&on_kiln);
    let on_kiln = String::from_utf8(on_kiln.stdout).unwrap();
    assert!(on_kiln.contains("kiln"), "{on_kiln}");

    let shared = mori(dir.path(), &["recall", "orchard plan"]);
    assert_ok(&shared);
    let shared = String::from_utf8(shared.stdout).unwrap();
    assert!(shared.contains("orchard"), "{shared}");

    assert_ok(&mori(dir.path(), &["switch", "main"]));
    let hidden = mori(dir.path(), &["recall", "kiln dusk"]);
    assert_ok(&hidden);
    let hidden = String::from_utf8(hidden.stdout).unwrap();
    assert!(hidden.contains("nothing recalled"), "{hidden}");

    let still_there = mori(dir.path(), &["recall", "orchard plan"]);
    assert_ok(&still_there);
    let still_there = String::from_utf8(still_there.stdout).unwrap();
    assert!(still_there.contains("orchard"), "{still_there}");

    let found = mori(dir.path(), &["find", "--contains", "kiln"]);
    assert_ok(&found);
    let found = String::from_utf8(found.stdout).unwrap();
    assert!(found.contains("nothing found"), "{found}");

    let listed = mori(dir.path(), &["branch"]);
    assert_ok(&listed);
    let listed = String::from_utf8(listed.stdout).unwrap();
    assert!(listed.contains("* main"), "{listed}");
    assert!(listed.contains("kiln"), "{listed}");
}

#[test]
fn stash_lets_you_switch_and_come_back() {
    let dir = tempdir().unwrap();
    assert_ok(&mori(dir.path(), &["init"]));
    fs::write(
        dir.path().join("roof.md"),
        "the unfinished copper roof needs another day\n",
    )
    .unwrap();
    assert_ok(&mori(dir.path(), &["add", "roof.md"]));

    let blocked = mori(dir.path(), &["switch", "-c", "other"]);
    assert!(!blocked.status.success());
    let blocked = String::from_utf8(blocked.stderr).unwrap();
    assert!(blocked.contains("stash"), "{blocked}");

    assert_ok(&mori(dir.path(), &["stash", "-m", "hold the roof"]));
    let status = mori(dir.path(), &["status"]);
    assert_ok(&status);
    let status = String::from_utf8(status.stdout).unwrap();
    assert!(status.contains("nothing staged"), "{status}");
    assert!(status.contains("stash: 1 entry"), "{status}");

    let listed = mori(dir.path(), &["stash", "list"]);
    assert_ok(&listed);
    let listed = String::from_utf8(listed.stdout).unwrap();
    assert!(listed.contains("hold the roof"), "{listed}");
    assert!(listed.contains("branch main"), "{listed}");

    assert_ok(&mori(dir.path(), &["switch", "-c", "other"]));
    assert_ok(&mori(dir.path(), &["stash", "pop"]));
    let restored = mori(dir.path(), &["status"]);
    assert_ok(&restored);
    let restored = String::from_utf8(restored.stdout).unwrap();
    assert!(restored.contains("roof.md"), "{restored}");
    assert!(restored.contains("On branch other"), "{restored}");

    assert_ok(&mori(dir.path(), &["reset"]));
    assert_ok(&mori(dir.path(), &["add", "roof.md"]));
    assert_ok(&mori(dir.path(), &["stash"]));
    assert_ok(&mori(dir.path(), &["stash", "drop"]));
    let empty = mori(dir.path(), &["stash", "list"]);
    assert_ok(&empty);
    let empty = String::from_utf8(empty.stdout).unwrap();
    assert!(empty.contains("nothing stashed"), "{empty}");
}
