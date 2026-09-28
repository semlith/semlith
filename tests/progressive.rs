//! A store filling from cold: keyword answers from the rows already written,
//! the files changed most recently embedded first, and a search saying how
//! much is still to come.
//!
//! Ignored by default: the first run downloads the embedding model.
//!
//! ```sh
//! cargo test --test progressive -- --ignored --test-threads=1
//! ```

use semlith::Semlith;
use std::path::Path;
use std::time::{Duration, SystemTime};

fn write(at: &Path, name: &str, text: &str, age: Duration) {
    let path = at.join(name);
    std::fs::write(&path, text).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(SystemTime::now() - age)
        .unwrap();
}

#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_files_changed_most_recently_are_embedded_first() {
    // The command line's ordering, which the library alone does not take.
    semlith::accel::manage();
    // SAFETY: read only by this test binary's index pass, on this thread.
    unsafe { std::env::set_var(semlith::accel::ACCEL_ENV, "cpu") };
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let day = Duration::from_secs(86_400);
    write(
        corpus.path(),
        "a_old.md",
        "Oldest notes about gardening.",
        day * 30,
    );
    write(
        corpus.path(),
        "b_new.md",
        "Newest notes about sailing.",
        day,
    );
    write(
        corpus.path(),
        "c_mid.md",
        "Middle notes about baking.",
        day * 10,
    );
    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    let mut order = Vec::new();
    s.index_paths(&[corpus.path().to_path_buf()], |path, p| {
        if p.outcome == semlith::FileOutcome::Indexing {
            order.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    })
    .unwrap();
    assert_eq!(order, vec!["b_new.md", "c_mid.md", "a_old.md"]);
}

#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_store_answers_keyword_questions_while_it_is_still_embedding() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    // Enough text that the run takes many seconds on a CPU.
    for f in 0..240 {
        let mut text = format!("# Report {f}\n\nThe zanzibar{f} ledger closes quarterly.\n\n");
        for p in 0..12 {
            text.push_str(&format!(
                "Paragraph {p} of report {f} discusses supply lines, tariffs and the \
                 weather over the harbour in some detail, at length, for the record.\n\n"
            ));
        }
        std::fs::write(corpus.path().join(format!("report_{f:03}.md")), text).unwrap();
    }
    let dir = store.path().to_path_buf();
    let root = corpus.path().to_path_buf();
    let writer = std::thread::spawn(move || {
        let mut s = Semlith::open(&dir, None).unwrap();
        s.quiet = true;
        s.index_paths(&[root], |_, _| {}).unwrap()
    });

    // Poll a second connection, as an agent's server would.
    let started = std::time::Instant::now();
    let mut answered = None;
    let mut pending_seen = false;
    while !writer.is_finished() && started.elapsed() < Duration::from_secs(600) {
        std::thread::sleep(Duration::from_millis(250));
        let Ok(mut reader) = Semlith::open_existing(store.path()) else {
            continue;
        };
        if reader.pending_share().ok().flatten().is_some() {
            pending_seen = true;
        }
        if answered.is_none()
            && let Ok(hits) = reader.search("zanzibar0", 3)
            && hits.iter().any(|h| h.path.ends_with("report_000.md"))
        {
            answered = Some(started.elapsed());
        }
        if answered.is_some() && pending_seen {
            break;
        }
    }
    let running = !writer.is_finished();
    writer.join().unwrap();
    assert!(
        answered.is_some() && running,
        "no keyword answer while the run was going (answered {answered:?})"
    );
    assert!(pending_seen, "a search mid-run never saw anything pending");
}
