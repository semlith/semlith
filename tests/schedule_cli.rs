//! `semlith schedule`, driven as the binary rather than as the module.
//!
//! `tests/schedules.rs` covers the record, the file and the runner. This covers
//! the half a person touches: that the four verbs reach the same file the
//! daemon owns, and that the two refusals a terminal will actually meet say
//! what would have worked instead.
//!
//! No daemon is spawned and no model is downloaded — every one of these is a
//! read or a write of one small JSON file under a temporary home.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A home of its own, so nothing here can see or write the developer's.
///
/// `HOME` as well as `SEMLITH_HOME`: `home::user_home` reads the first and the
/// registry the second, and a test that set only one of them has, in this
/// repository's history, written into a real user's configuration.
fn sandbox(tag: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix(&format!("semlith-schedule-{tag}-"))
        .tempdir()
        .expect("a temporary directory");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    (dir, home)
}

fn schedule(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_semlith"))
        .env("SEMLITH_HOME", home)
        .env("HOME", home)
        .env_remove("SEMLITH_STORE")
        .arg("schedule")
        .args(args)
        .output()
        .expect("semlith schedule runs")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_home_with_no_schedules_file_says_so_rather_than_failing() {
    let (_dir, home) = sandbox("empty");
    let out = schedule(&home, &["list"]);
    assert!(
        out.status.success(),
        "listing nothing is not an error: {}",
        text(&out)
    );
    assert!(
        text(&out).contains("no schedules"),
        "an empty list should say so and name the command that writes one, got: {}",
        text(&out)
    );
    // Absence is the normal state. Listing must not be what creates the file.
    assert!(
        !home.join("schedules.json").exists(),
        "listing created a schedules file"
    );
}

#[test]
fn the_four_verbs_reach_the_one_file_the_daemon_owns() {
    let (dir, home) = sandbox("verbs");
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();

    let added = schedule(
        &home,
        &[
            "add",
            "change",
            "--every",
            "3600",
            "--to",
            out.to_str().unwrap(),
            "--format",
            "csv",
        ],
    );
    assert!(added.status.success(), "{}", text(&added));

    // The file the daemon reads is the file the command wrote — asserted
    // against the record rather than against the line the command printed,
    // because a command that prints a success it did not perform is the whole
    // failure this test exists to catch.
    let file = semlith::schedule::Schedules::load_from(&home.join("schedules.json"))
        .expect("the schedules file parses");
    assert_eq!(file.schedules.len(), 1);
    let (id, held) = file.schedules.iter().next().unwrap();
    assert_eq!(held.kind, "change");
    assert_eq!(held.format, "csv");
    // A cadence is an interval on disk, whatever any surface calls it.
    assert_eq!(held.every_seconds, 3600);
    assert!(held.enabled);

    let listed = schedule(&home, &["list"]);
    assert!(listed.status.success(), "{}", text(&listed));
    let body = text(&listed);
    assert!(
        body.contains(id),
        "the list does not name the id it just wrote"
    );
    // The destination in full, not shortened against this process's working
    // directory: the daemon runs the schedule from somewhere else, and a
    // destination that happened to be the current directory printed as nothing.
    assert!(
        body.contains(out.to_str().unwrap()),
        "the list does not print the destination in full, got: {body}"
    );
    assert!(
        body.contains("every 1 hour"),
        "3600 seconds should read as an hour, got: {body}"
    );

    let off = schedule(&home, &["set", id, "off"]);
    assert!(off.status.success(), "{}", text(&off));
    let file = semlith::schedule::Schedules::load_from(&home.join("schedules.json")).unwrap();
    assert!(
        !file.schedules[id].enabled,
        "the record still says enabled after `set off`"
    );

    let removed = schedule(&home, &["remove", id]);
    assert!(removed.status.success(), "{}", text(&removed));
    let file = semlith::schedule::Schedules::load_from(&home.join("schedules.json")).unwrap();
    assert!(
        file.schedules.is_empty(),
        "the schedule survived its removal"
    );
}

#[test]
fn a_destination_the_daemon_could_not_find_is_refused_at_the_terminal() {
    let (dir, home) = sandbox("relative");
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();

    // A relative path is resolved against *this* process's working directory,
    // because that is what the person typing it meant. What must never reach
    // the file is the relative form: the daemon's working directory is wherever
    // `semlith start` was typed, months ago.
    let added = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .env("SEMLITH_HOME", &home)
        .env("HOME", &home)
        .current_dir(dir.path())
        .args([
            "schedule", "add", "change", "--every", "3600", "--to", "out",
        ])
        .output()
        .expect("semlith schedule runs");
    assert!(added.status.success(), "{}", text(&added));
    let file = semlith::schedule::Schedules::load_from(&home.join("schedules.json")).unwrap();
    let held = file.schedules.values().next().unwrap();
    assert!(
        held.dir.is_absolute(),
        "a relative --to reached the record as {:?}",
        held.dir
    );

    // A directory that does not exist is refused before anything is written,
    // naming what would have worked.
    let missing = schedule(
        &home,
        &[
            "add",
            "change",
            "--every",
            "3600",
            "--to",
            dir.path().join("nowhere").to_str().unwrap(),
        ],
    );
    assert!(
        !missing.status.success(),
        "a missing destination was accepted"
    );
    let why = text(&missing);
    assert!(
        why.contains("nowhere"),
        "the refusal does not name the directory, got: {why}"
    );
}

#[test]
fn a_cadence_below_the_floor_and_an_unknown_id_each_say_what_would_have_worked() {
    let (dir, home) = sandbox("refusals");
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();

    let quick = schedule(
        &home,
        &[
            "add",
            "change",
            "--every",
            "5",
            "--to",
            out.to_str().unwrap(),
        ],
    );
    assert!(
        !quick.status.success(),
        "a five-second cadence was accepted"
    );
    let why = text(&quick);
    assert!(
        why.contains(&semlith::schedule::MIN_EVERY.to_string()),
        "the refusal does not name the floor, got: {why}"
    );

    for verb in [vec!["remove", "s9"], vec!["set", "s9", "off"]] {
        let out = schedule(&home, &verb);
        assert!(!out.status.success(), "{verb:?} on an unknown id succeeded");
        assert!(
            text(&out).contains("semlith schedule list"),
            "{verb:?} on an unknown id does not say how to find the ids, got: {}",
            text(&out)
        );
    }
}

#[test]
fn a_pdf_to_a_terminal_is_refused_before_a_store_is_opened() {
    let (_dir, home) = sandbox("pdf");
    // No store is named and none exists, and the point is that this fails on
    // the argument rather than on the store: a screenful of PDF bytes is not
    // an outcome worth opening a database for.
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .env("SEMLITH_HOME", &home)
        .env("HOME", &home)
        .env_remove("SEMLITH_STORE")
        .args(["report", "change", "--format", "pdf"])
        .output()
        .expect("semlith report runs");
    assert!(!out.status.success());
    assert!(
        text(&out).contains("--out"),
        "the refusal does not name the flag that would have worked, got: {}",
        text(&out)
    );
}
