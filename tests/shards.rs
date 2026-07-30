//! What the sharded store is for: an index run that survives being killed, and
//! a change to one file that costs one shard rather than the whole corpus.
//!
//! Both are properties of a real run against a real store, so both are measured
//! against one — a child process for the run that gets killed, because a test
//! cannot interrupt itself.
//!
//! ```sh
//! cargo test --release --test shards -- --ignored --nocapture
//! ```

#![cfg(unix)]

use semlith::Semlith;
use std::fs;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Files in the corpus the interrupted run works through. Large enough that
/// killing it partway leaves work undone, small enough to embed in seconds.
const CORPUS_FILES: usize = 2000;

/// Vectors per shard while these tests run. Small, so a corpus a test can
/// afford still spans several shards.
const SHARD_VECTORS: &str = "256";

fn corpus(dir: &Path, files: usize) {
    for i in 0..files {
        fs::write(
            dir.join(format!("note_{i:05}.md")),
            format!(
                "Note {i}. Fermentation, ownership, retries and backoff, described \
                 at moderate length so the chunk is worth embedding and the run \
                 takes long enough to interrupt.\n"
            ),
        )
        .unwrap();
    }
}

fn index_child(store: &Path, corpus: &Path, checkpoint_secs: &str) -> Child {
    Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(store)
        .arg("index")
        .arg(corpus)
        .arg("--quiet")
        .env("SEMLITH_SHARD_VECTORS", SHARD_VECTORS)
        .env("SEMLITH_CHECKPOINT_SECS", checkpoint_secs)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

/// How many chunks the store will stand behind — the ones whose file has a
/// committed hash, which is exactly what survives a kill.
fn durable(store: &Path) -> i64 {
    let s = Semlith::open(store, None).unwrap();
    semlith::store::durable_chunks(s.db()).unwrap()
}

/// The release's first promise: an interrupted first index costs what was in
/// flight, not the run.
#[test]
#[ignore = "kills a real index run; downloads an embedding model on first run"]
fn an_interrupted_index_resumes_where_it_stopped() {
    let dir = tempfile::tempdir().unwrap();
    corpus(dir.path(), CORPUS_FILES);
    let store = dir.path().join(".semlith");

    // Checkpointing every second, so the kill lands after several of them.
    let mut child = index_child(&store, dir.path(), "1");

    // Wait for the first checkpoint to reach disk, then let a couple more go by
    // before killing: the claim is about resuming from the last one, which
    // needs there to have been more than one.
    let deadline = Instant::now() + Duration::from_secs(120);
    while durable(&store) == 0 {
        assert!(Instant::now() < deadline, "no checkpoint landed in 120s");
        assert!(
            child.try_wait().unwrap().is_none(),
            "the run finished before it could be interrupted — corpus too small"
        );
        thread::sleep(Duration::from_millis(100));
    }
    thread::sleep(Duration::from_millis(1500));
    let kept = durable(&store);
    assert!(
        child.try_wait().unwrap().is_none(),
        "the run finished before it could be interrupted — corpus too small"
    );
    Command::new("kill")
        .arg("-INT")
        .arg(child.id().to_string())
        .status()
        .unwrap();
    let status = child.wait().unwrap();
    println!("\n--- killed mid-run: {status}, {kept} chunks already durable");
    assert!(kept > 0, "nothing survived the interrupt");

    // What survived is searchable, with no repair pass and no second run.
    {
        let mut reader = Semlith::open(&store, None).unwrap();
        reader.quiet = true;
        let hits = reader.search("fermentation and backoff", 3).unwrap();
        assert!(
            !hits.is_empty(),
            "the store kept {kept} chunks but answered nothing"
        );
    }

    // The second run walks past what the first one committed.
    let mut second = Semlith::open(&store, None).unwrap();
    second.quiet = true;
    let report = second
        .index_paths(&[dir.path().to_path_buf()], |_| {})
        .unwrap();
    println!(
        "resumed run: {} indexed, {} unchanged of {} files",
        report.indexed, report.unchanged, CORPUS_FILES
    );
    assert!(
        report.unchanged > 0,
        "the resumed run re-embedded everything: {report:?}"
    );
    assert_eq!(
        report.indexed + report.unchanged,
        CORPUS_FILES,
        "the resumed run did not account for every file: {report:?}"
    );
    // And the store is whole afterwards.
    assert_eq!(
        durable(&store) as usize,
        second.len(),
        "chunks and vectors disagree after resuming"
    );
    let mut reader = Semlith::open(&store, None).unwrap();
    reader.quiet = true;
    assert!(!reader.search("ownership and retries", 5).unwrap().is_empty());
}

/// The release's second promise: changing one file writes one shard.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn one_file_changing_rewrites_one_shard() {
    let dir = tempfile::tempdir().unwrap();
    corpus(dir.path(), 700);
    let store = dir.path().join(".semlith");

    // Built through the binary so the shard size applies to the whole run.
    let built = index_child(&store, dir.path(), "3600").wait().unwrap();
    assert!(built.success());

    let shards = shard_files(&store);
    assert!(
        shards.len() >= 3,
        "700 files at {SHARD_VECTORS} vectors a shard gave {} shard(s)",
        shards.len()
    );
    println!("\n--- {} shards", shards.len());

    let before: Vec<(std::path::PathBuf, u64, std::time::SystemTime)> = shards
        .iter()
        .map(|p| {
            let m = fs::metadata(p).unwrap();
            (p.clone(), m.len(), m.modified().unwrap())
        })
        .collect();

    // A whole-index rewrite and a one-shard rewrite are indistinguishable
    // inside one filesystem timestamp tick.
    thread::sleep(Duration::from_millis(1100));

    let mut s = Semlith::open(&store, None).unwrap();
    s.quiet = true;
    let removed = s.forget(&dir.path().join("note_00000.md")).unwrap();
    assert!(removed > 0, "nothing was forgotten");

    let rewritten: Vec<&std::path::PathBuf> = before
        .iter()
        .filter(|(p, _, when)| fs::metadata(p).unwrap().modified().unwrap() > *when)
        .map(|(p, _, _)| p)
        .collect();
    let total: u64 = before.iter().map(|(_, len, _)| len).sum();
    let written: u64 = rewritten
        .iter()
        .map(|p| fs::metadata(p).unwrap().len())
        .sum();
    println!(
        "forgetting one file rewrote {} of {} shards: {} KB of {} KB",
        rewritten.len(),
        before.len(),
        written / 1024,
        total / 1024,
    );
    assert_eq!(
        rewritten.len(),
        1,
        "one file's chunks live in one shard, but {} shards were rewritten",
        rewritten.len()
    );

    // And the store still answers, minus the file that was dropped.
    let mut reader = Semlith::open(&store, None).unwrap();
    reader.quiet = true;
    let hits = reader.search("fermentation and backoff", 5).unwrap();
    assert!(!hits.is_empty(), "the store stopped answering");
    assert!(
        !hits.iter().any(|h| h.path.ends_with("note_00000.md")),
        "the forgotten file is still being returned"
    );
}

fn shard_files(store: &Path) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = fs::read_dir(store.join("index"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("tvim"))
        .collect();
    out.sort();
    out
}
