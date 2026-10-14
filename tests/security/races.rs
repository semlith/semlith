//! Item 14: index-time races (ghost paths, stopped runs, same-size edits).

use super::*;

fn listed(daemon: &Daemon, name: &str) -> bool {
    daemon
        .api("/api/files?tree=1&depth=9")
        .to_string()
        .contains(name)
}

/// A same-size edit that lands between the read and the row's write — its
/// mtime earlier than `indexed_at` — is stale, not fresh.
#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn races_a_same_size_edit_after_the_read_is_stale() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let corpus = dir.path().join("corpus");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&corpus).unwrap();
    let file = corpus.join("note.md");
    std::fs::write(&file, "# Note\n\nalpha alpha alpha\n").unwrap();
    // Read an hour ago, as far as its mtime says.
    let hour_ago = std::time::SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(hour_ago)
        .unwrap();
    let out = run(&home, &["index", corpus.to_str().unwrap()], &[]);
    assert!(out.status.success());

    // Edited to the same size, stamped a millisecond after that read: well
    // before the row was written.
    std::fs::write(&file, "# Note\n\nomega omega omega\n").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(hour_ago + Duration::from_millis(1))
        .unwrap();
    let out = run(&home, &["read", &format!("{}:1-3", file.display())], &[]);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        said.contains("stale") || said.contains("omega"),
        "a same-size edit read as fresh, old text served as current:\n{said}"
    );
}

/// A file deleted after its read and before its rows land leaves the store
/// at the end of that run.
#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn races_a_file_deleted_mid_run_leaves_no_ghost() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let corpus = dir.path().join("corpus");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&corpus).unwrap();
    std::fs::write(corpus.join("keep.md"), "# Keep\n\nstays\n").unwrap();
    std::fs::write(corpus.join("ghost.md"), "# Ghost\n\ndeleted mid-run\n").unwrap();
    let out = run(
        &home,
        &["index", corpus.to_str().unwrap()],
        &[("SEMLITH_FAULT_DELETE", "ghost.md")],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !corpus.join("ghost.md").exists(),
        "the hook did not delete the file (an old binary has none)"
    );
    let out = run(&home, &["files"], &[]);
    let files = String::from_utf8_lossy(&out.stdout);
    assert!(files.contains("keep.md"), "{files}");
    assert!(
        !files.contains("ghost.md"),
        "a deleted file is still in the store:\n{files}"
    );
}

/// A deletion that arrives while a run holds the writer is kept when that run
/// is stopped, rather than dropped with the run's own events.
#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn races_a_stopped_run_keeps_the_deletions_that_queued_behind_it() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let root = dir.path().join("corpus");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("doomed.md"),
        "# Doomed\n\nto be deleted during a run\n",
    )
    .unwrap();
    let out = run(&home, &["index", root.to_str().unwrap()], &[]);
    assert!(out.status.success());

    let daemon = Daemon::start(&home, &[]);
    assert!(
        until(Duration::from_secs(60), || listed(&daemon, "doomed.md")),
        "not served"
    );
    // A run with enough to embed that it is still going when it is stopped.
    let more = root.join("more");
    std::fs::create_dir_all(&more).unwrap();
    for i in 0..200 {
        let body = format!("# Note {i}\n\n{}", "Ownership and borrowing. ".repeat(120));
        std::fs::write(more.join(format!("n{i:03}.md")), body).unwrap();
    }
    let token = format!("Semlith-Token: {}\r\n", daemon.token);
    let body = serde_json::json!({ "path": root.display().to_string() }).to_string();
    let (status, said) = daemon.send("POST", "/api/index", &token, &body);
    assert_eq!(status, 200, "{said}");
    assert!(
        until(
            Duration::from_secs(120),
            || daemon.api("/api/files")["total"].as_i64().unwrap_or(0) > 3
        ),
        "the run never started embedding"
    );
    std::fs::remove_file(root.join("doomed.md")).unwrap();
    let (status, said) = daemon.send("POST", "/api/index/control", &token, r#"{"action":"stop"}"#);
    assert_eq!(status, 200, "{said}");
    assert!(
        until(Duration::from_secs(120), || !listed(&daemon, "doomed.md")),
        "the deletion was dropped with the stopped run's events"
    );
}
