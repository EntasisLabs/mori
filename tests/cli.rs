use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use tempfile::tempdir;

fn mori(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mori"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run mori")
}

fn mori_stdin(dir: &std::path::Path, args: &[&str], stdin: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mori"))
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mori");
    {
        let mut input = child.stdin.take().expect("stdin");
        input.write_all(stdin.as_bytes()).expect("write stdin");
    }
    child.wait_with_output().expect("wait mori")
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

    assert_ok(&mori(dir.path(), &["checkout", "-b", "kiln"]));
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

    assert_ok(&mori(dir.path(), &["checkout", "main"]));
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
fn stash_lets_you_checkout_and_come_back() {
    let dir = tempdir().unwrap();
    assert_ok(&mori(dir.path(), &["init"]));
    fs::write(
        dir.path().join("roof.md"),
        "the unfinished copper roof needs another day\n",
    )
    .unwrap();
    assert_ok(&mori(dir.path(), &["add", "roof.md"]));

    let blocked = mori(dir.path(), &["checkout", "-b", "other"]);
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

    assert_ok(&mori(dir.path(), &["checkout", "-b", "other"]));
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

#[test]
fn merge_fast_forward_and_the_cases_that_do_nothing() {
    let dir = tempdir().unwrap();
    assert_ok(&mori(dir.path(), &["init"]));
    fs::write(
        dir.path().join("orchard.md"),
        "the orchard plan waits on the north fence\n",
    )
    .unwrap();
    assert_ok(&mori(dir.path(), &["add", "orchard.md"]));
    assert_ok(&mori(dir.path(), &["commit", "-m", "remember the orchard"]));
    assert_ok(&mori(dir.path(), &["checkout", "-b", "kiln"]));
    fs::write(dir.path().join("kiln.md"), "the kiln runs hot at dusk\n").unwrap();
    assert_ok(&mori(dir.path(), &["add", "kiln.md"]));
    assert_ok(&mori(dir.path(), &["commit", "-m", "note the kiln"]));
    assert_ok(&mori(dir.path(), &["checkout", "main"]));

    let into_itself = mori(dir.path(), &["merge", "main"]);
    assert!(!into_itself.status.success());
    let into_itself = String::from_utf8(into_itself.stderr).unwrap();
    assert!(into_itself.contains("itself"), "{into_itself}");

    fs::write(dir.path().join("roof.md"), "the unfinished copper roof\n").unwrap();
    assert_ok(&mori(dir.path(), &["add", "roof.md"]));
    let dirty = mori(dir.path(), &["merge", "kiln"]);
    assert!(!dirty.status.success());
    let dirty = String::from_utf8(dirty.stderr).unwrap();
    assert!(dirty.contains("staged"), "{dirty}");
    assert_ok(&mori(dir.path(), &["reset"]));

    let merged = mori(dir.path(), &["merge", "kiln"]);
    assert_ok(&merged);
    let merged = String::from_utf8(merged.stdout).unwrap();
    assert!(merged.contains("fast-forward"), "{merged}");

    let recalled = mori(dir.path(), &["recall", "kiln dusk"]);
    assert_ok(&recalled);
    let recalled = String::from_utf8(recalled.stdout).unwrap();
    assert!(recalled.contains("kiln"), "{recalled}");

    let again = mori(dir.path(), &["merge", "kiln"]);
    assert_ok(&again);
    let again = String::from_utf8(again.stdout).unwrap();
    assert!(again.contains("already up to date"), "{again}");
}

#[test]
fn diverged_merge_shows_both_sides() {
    let dir = tempdir().unwrap();
    assert_ok(&mori(dir.path(), &["init"]));
    fs::write(
        dir.path().join("orchard.md"),
        "the orchard plan waits on the north fence\n",
    )
    .unwrap();
    assert_ok(&mori(dir.path(), &["add", "orchard.md"]));
    assert_ok(&mori(dir.path(), &["commit", "-m", "remember the orchard"]));

    assert_ok(&mori(dir.path(), &["checkout", "-b", "kiln"]));
    fs::write(dir.path().join("kiln.md"), "the kiln runs hot at dusk\n").unwrap();
    assert_ok(&mori(dir.path(), &["add", "kiln.md"]));
    assert_ok(&mori(dir.path(), &["commit", "-m", "note the kiln"]));

    assert_ok(&mori(dir.path(), &["checkout", "main"]));
    fs::write(
        dir.path().join("shed.md"),
        "the shed door sticks after rain\n",
    )
    .unwrap();
    assert_ok(&mori(dir.path(), &["add", "shed.md"]));
    assert_ok(&mori(dir.path(), &["commit", "-m", "note the shed"]));

    let merged = mori(dir.path(), &["merge", "kiln", "-m", "bring the kiln back"]);
    assert_ok(&merged);
    let merged = String::from_utf8(merged.stdout).unwrap();
    assert!(merged.contains("bring the kiln back"), "{merged}");

    let log = mori(dir.path(), &["log", "-n", "20"]);
    assert_ok(&log);
    let log = String::from_utf8(log.stdout).unwrap();
    assert!(log.contains("bring the kiln back"), "{log}");
    assert!(log.contains("note the shed"), "{log}");
    assert!(log.contains("note the kiln"), "{log}");
    assert!(log.contains("remember the orchard"), "{log}");
    assert!(log.contains("Merge:"), "{log}");

    let kiln = mori(dir.path(), &["recall", "kiln dusk"]);
    assert_ok(&kiln);
    let kiln = String::from_utf8(kiln.stdout).unwrap();
    assert!(kiln.contains("kiln"), "{kiln}");

    let shed = mori(dir.path(), &["recall", "shed door"]);
    assert_ok(&shed);
    let shed = String::from_utf8(shed.stdout).unwrap();
    assert!(shed.contains("shed"), "{shed}");

    let refused = mori(dir.path(), &["rebase", "kiln"]);
    assert!(!refused.status.success());
    let refused = String::from_utf8(refused.stderr).unwrap();
    assert!(refused.contains("merge commits"), "{refused}");
}

#[test]
fn rebase_replays_onto_main_without_moving_main() {
    let dir = tempdir().unwrap();
    assert_ok(&mori(dir.path(), &["init"]));
    fs::write(
        dir.path().join("orchard.md"),
        "the orchard plan waits on the north fence\n",
    )
    .unwrap();
    assert_ok(&mori(dir.path(), &["add", "orchard.md"]));
    assert_ok(&mori(dir.path(), &["commit", "-m", "remember the orchard"]));

    assert_ok(&mori(dir.path(), &["checkout", "-b", "kiln"]));
    fs::write(dir.path().join("kiln.md"), "the kiln runs hot at dusk\n").unwrap();
    assert_ok(&mori(dir.path(), &["add", "kiln.md"]));
    assert_ok(&mori(dir.path(), &["commit", "-m", "note the kiln"]));

    assert_ok(&mori(dir.path(), &["checkout", "main"]));
    fs::write(
        dir.path().join("shed.md"),
        "the shed door sticks after rain\n",
    )
    .unwrap();
    assert_ok(&mori(dir.path(), &["add", "shed.md"]));
    assert_ok(&mori(dir.path(), &["commit", "-m", "note the shed"]));

    assert_ok(&mori(dir.path(), &["checkout", "kiln"]));
    fs::write(dir.path().join("roof.md"), "the unfinished copper roof\n").unwrap();
    assert_ok(&mori(dir.path(), &["add", "roof.md"]));
    let dirty = mori(dir.path(), &["rebase", "main"]);
    assert!(!dirty.status.success());
    let dirty = String::from_utf8(dirty.stderr).unwrap();
    assert!(dirty.contains("staged"), "{dirty}");
    assert_ok(&mori(dir.path(), &["reset"]));

    let onto_itself = mori(dir.path(), &["rebase", "kiln"]);
    assert!(!onto_itself.status.success());
    let onto_itself = String::from_utf8(onto_itself.stderr).unwrap();
    assert!(onto_itself.contains("itself"), "{onto_itself}");

    let rebased = mori(dir.path(), &["rebase", "main"]);
    assert_ok(&rebased);
    let rebased = String::from_utf8(rebased.stdout).unwrap();
    assert!(rebased.contains("rebased 1 onto main"), "{rebased}");
    assert!(rebased.contains("note the kiln"), "{rebased}");

    assert_ok(&mori(dir.path(), &["checkout", "-b", "side"]));
    assert_ok(&mori(dir.path(), &["checkout", "kiln"]));

    let kiln = mori(dir.path(), &["recall", "kiln dusk"]);
    assert_ok(&kiln);
    let kiln = String::from_utf8(kiln.stdout).unwrap();
    assert!(kiln.contains("kiln"), "{kiln}");

    let shed = mori(dir.path(), &["recall", "shed door"]);
    assert_ok(&shed);
    let shed = String::from_utf8(shed.stdout).unwrap();
    assert!(shed.contains("shed"), "{shed}");

    assert_ok(&mori(dir.path(), &["checkout", "main"]));
    let hidden = mori(dir.path(), &["recall", "kiln dusk"]);
    assert_ok(&hidden);
    let hidden = String::from_utf8(hidden.stdout).unwrap();
    assert!(hidden.contains("nothing recalled"), "{hidden}");

    let still = mori(dir.path(), &["recall", "shed door"]);
    assert_ok(&still);
    let still = String::from_utf8(still.stdout).unwrap();
    assert!(still.contains("shed"), "{still}");

    assert_ok(&mori(dir.path(), &["checkout", "kiln"]));
    let same = mori(dir.path(), &["rebase", "side"]);
    assert_ok(&same);
    let same = String::from_utf8(same.stdout).unwrap();
    assert!(same.contains("already up to date"), "{same}");
}

#[test]
fn tags_excerpts_and_one_word_recall_filter() {
    let dir = tempdir().unwrap();
    assert_ok(&mori(dir.path(), &["init"]));

    let runbook = "\
# API runbook

The service overview stays quiet.

## Escalation

Primary on-call has 15 minutes to ack. After that PagerDuty escalates to the secondary.

## Rollback

Use deployctl to roll back the last release.
";
    let handoff = "\
# Handoff

Search escalation still points at the old team.

## Pager

The rotation calendar is stale.
Fix the xylophone entry in pagerduty before the next rotation.
";
    fs::write(dir.path().join("runbook.md"), runbook).unwrap();
    fs::write(dir.path().join("handoff.md"), handoff).unwrap();

    assert_ok(&mori(
        dir.path(),
        &["add", "runbook.md", "--tag", "homelab,pxe"],
    ));
    assert_ok(&mori(
        dir.path(),
        &["add", "handoff.md", "--tag", "Jellyfin"],
    ));

    let status = mori(dir.path(), &["status"]);
    assert_ok(&status);
    let status = String::from_utf8(status.stdout).unwrap();
    assert!(status.contains("[homelab, pxe]"), "{status}");
    assert!(status.contains("[jellyfin]"), "{status}");

    assert_ok(&mori(dir.path(), &["commit", "-m", "on-call notes"]));

    let log = mori(dir.path(), &["log"]);
    assert_ok(&log);
    let log = String::from_utf8(log.stdout).unwrap();
    assert!(log.contains("tags: homelab, pxe"), "{log}");
    assert!(log.contains("tags: jellyfin"), "{log}");
    assert!(!log.contains('⊕'), "{log}");

    let by_tag = mori(dir.path(), &["recall", "escalation", "--tag", "pxe"]);
    assert_ok(&by_tag);
    let by_tag = String::from_utf8(by_tag.stdout).unwrap();
    assert!(by_tag.contains("runbook"), "{by_tag}");
    assert!(!by_tag.contains("Handoff"), "{by_tag}");
    assert!(!by_tag.contains('§'), "{by_tag}");

    let other = mori(dir.path(), &["find", "--tag", "jellyfin"]);
    assert_ok(&other);
    let other = String::from_utf8(other.stdout).unwrap();
    assert!(other.contains("Handoff"), "{other}");
    assert!(!other.contains("runbook"), "{other}");

    let both = mori(dir.path(), &["find", "--tag", "homelab", "--tag", "pxe"]);
    assert_ok(&both);
    let both = String::from_utf8(both.stdout).unwrap();
    assert!(both.contains("runbook"), "{both}");
    assert!(!both.contains("Handoff"), "{both}");

    let none = mori(
        dir.path(),
        &[
            "recall",
            "escalation",
            "--tag",
            "homelab",
            "--tag",
            "jellyfin",
        ],
    );
    assert_ok(&none);
    let none = String::from_utf8(none.stdout).unwrap();
    assert!(none.contains("nothing recalled"), "{none}");

    let one_word = mori(dir.path(), &["recall", "xylophone"]);
    assert_ok(&one_word);
    let one_word = String::from_utf8(one_word.stdout).unwrap();
    assert!(one_word.contains("Handoff"), "{one_word}");
    assert!(!one_word.contains("runbook"), "{one_word}");
    assert!(!one_word.contains("xylophone"), "{one_word}");
    assert!(!one_word.contains('§'), "{one_word}");
    assert!(
        one_word.lines().any(|line| line.starts_with("retrieved ")),
        "{one_word}"
    );

    let summary = mori(dir.path(), &["recall", "service overview"]);
    assert_ok(&summary);
    let summary = String::from_utf8(summary.stdout).unwrap();
    assert!(summary.contains("runbook"), "{summary}");
    assert!(!summary.contains('§'), "{summary}");
    assert!(!summary.contains("15 minutes"), "{summary}");
    assert!(!summary.contains("deployctl"), "{summary}");

    let excerpt = mori(dir.path(), &["recall", "pagerduty escalation", "--excerpt"]);
    assert_ok(&excerpt);
    let excerpt = String::from_utf8(excerpt.stdout).unwrap();
    assert!(excerpt.contains("§"), "{excerpt}");
    assert!(
        excerpt.contains("Escalation") || excerpt.contains("Pager"),
        "{excerpt}"
    );
    assert!(
        excerpt.contains("PagerDuty") || excerpt.contains("pagerduty"),
        "{excerpt}"
    );
    assert!(!excerpt.contains("deployctl"), "{excerpt}");

    let matched = mori(
        dir.path(),
        &["recall", "api runbook", "--excerpt", "--match", "deployctl"],
    );
    assert_ok(&matched);
    let matched = String::from_utf8(matched.stdout).unwrap();
    assert!(matched.contains("deployctl"), "{matched}");
    assert!(matched.contains("Rollback"), "{matched}");
    assert!(!matched.contains("15 minutes"), "{matched}");

    let found = mori(dir.path(), &["find", "--match", "xylophone"]);
    assert_ok(&found);
    let found = String::from_utf8(found.stdout).unwrap();
    assert!(found.contains("Handoff"), "{found}");
    assert!(!found.contains("runbook"), "{found}");
    assert!(!found.contains('§'), "{found}");
    assert!(!found.contains("xylophone"), "{found}");

    let tagged_match = mori(
        dir.path(),
        &["find", "--tag", "homelab", "--match", "deployctl"],
    );
    assert_ok(&tagged_match);
    let tagged_match = String::from_utf8(tagged_match.stdout).unwrap();
    assert!(tagged_match.contains("runbook"), "{tagged_match}");
    assert!(!tagged_match.contains("Handoff"), "{tagged_match}");

    let narrow = mori(dir.path(), &["recall", "xylophone", "--excerpt", "-C", "0"]);
    assert_ok(&narrow);
    let narrow = String::from_utf8(narrow.stdout).unwrap();
    assert!(narrow.contains("xylophone"), "{narrow}");
    assert!(narrow.contains("§"), "{narrow}");
    assert!(!narrow.contains("rotation calendar"), "{narrow}");
}

#[test]
fn notes_chain_until_commit() {
    let dir = tempdir().unwrap();
    assert_ok(&mori(dir.path(), &["init"]));

    let empty = mori(dir.path(), &["note", "-m", "   "]);
    assert!(!empty.status.success());
    let empty = String::from_utf8(empty.stderr).unwrap();
    assert!(empty.contains("the note is empty"), "{empty}");

    let path = mori(dir.path(), &["note", "notes.md"]);
    assert!(!path.status.success());
    let path = String::from_utf8(path.stderr).unwrap();
    assert!(path.contains("mori add"), "{path}");

    let both = mori(dir.path(), &["note", "-m", "hi", "-"]);
    assert!(!both.status.success());
    let both = String::from_utf8(both.stderr).unwrap();
    assert!(both.contains("-m sets the note text"), "{both}");

    let untouched = mori(dir.path(), &["status"]);
    assert_ok(&untouched);
    let untouched = String::from_utf8(untouched.stdout).unwrap();
    assert!(untouched.contains("nothing staged"), "{untouched}");

    assert_ok(&mori(
        dir.path(),
        &[
            "note",
            "-m",
            "remember to check on-call docs before asking for escalation",
            "--tag",
            "oncall",
        ],
    ));
    assert_ok(&mori(
        dir.path(),
        &[
            "note",
            "-m",
            "also verify pagerduty routing",
            "--session",
            "procedures",
            "--tag",
            "oncall",
        ],
    ));

    let piped = mori_stdin(
        dir.path(),
        &["note", "--tag", "oncall"],
        "stdin reminder about the xylophone rotation\n\nsecond paragraph stays stored\n",
    );
    assert_ok(&piped);
    let dashed = mori_stdin(
        dir.path(),
        &["note", "-", "--session", "procedures", "--tag", "oncall"],
        "dash note about the rotation calendar\n",
    );
    assert_ok(&dashed);

    let status = mori(dir.path(), &["status"]);
    assert_ok(&status);
    let status = String::from_utf8(status.stdout).unwrap();
    assert!(
        status.contains("remember to check on-call docs"),
        "{status}"
    );
    assert!(status.contains("also verify pagerduty routing"), "{status}");
    assert!(
        status.contains("stdin reminder about the xylophone rotation"),
        "{status}"
    );
    assert!(
        status.contains("dash note about the rotation calendar"),
        "{status}"
    );
    assert!(!status.contains("second paragraph"), "{status}");
    assert!(status.contains("(main)"), "{status}");
    assert!(status.contains("(procedures)"), "{status}");
    assert!(status.contains("[oncall]"), "{status}");
    assert_eq!(
        status
            .lines()
            .filter(|line| line.contains("note") && line.contains("[oncall]"))
            .count(),
        4,
        "{status}"
    );

    let commit = mori(dir.path(), &["commit", "-m", "oncall reminders"]);
    assert_ok(&commit);
    let commit = String::from_utf8(commit.stdout).unwrap();
    assert!(commit.contains("note -  [oncall]"), "{commit}");
    assert_eq!(commit.matches("note -  [oncall]").count(), 4, "{commit}");

    let cleared = mori(dir.path(), &["status"]);
    assert_ok(&cleared);
    let cleared = String::from_utf8(cleared.stdout).unwrap();
    assert!(cleared.contains("nothing staged"), "{cleared}");

    let log = mori(dir.path(), &["log"]);
    assert_ok(&log);
    let log = String::from_utf8(log.stdout).unwrap();
    assert!(log.contains("oncall reminders"), "{log}");
    assert!(log.contains("remember to check on-call docs"), "{log}");
    assert!(log.contains("also verify pagerduty routing"), "{log}");
    assert!(
        log.contains("stdin reminder about the xylophone rotation"),
        "{log}"
    );
    assert!(
        log.contains("dash note about the rotation calendar"),
        "{log}"
    );
    assert!(log.contains("note -"), "{log}");
    assert!(log.contains("tags: oncall"), "{log}");
    assert!(!log.contains('§'), "{log}");
    assert!(!log.contains('⊕'), "{log}");

    let session = mori(
        dir.path(),
        &["recall", "pagerduty", "--session", "procedures"],
    );
    assert_ok(&session);
    let session = String::from_utf8(session.stdout).unwrap();
    assert!(session.contains("pagerduty"), "{session}");
    assert!(!session.contains("on-call docs"), "{session}");
    assert!(!session.contains('§'), "{session}");

    let summary = mori(dir.path(), &["recall", "xylophone"]);
    assert_ok(&summary);
    let summary = String::from_utf8(summary.stdout).unwrap();
    assert!(
        summary.contains("stdin reminder about the xylophone rotation"),
        "{summary}"
    );
    assert!(!summary.contains("second paragraph"), "{summary}");
    assert!(!summary.contains('§'), "{summary}");

    let full = mori(dir.path(), &["recall", "xylophone", "--full"]);
    assert_ok(&full);
    let full = String::from_utf8(full.stdout).unwrap();
    assert!(full.contains("second paragraph stays stored"), "{full}");
}
