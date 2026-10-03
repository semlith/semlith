//! The public run-under-control API an embedder schedules runs with:
//! `index_paths_under`, `index_rest_under` and `undo_run`.
//!
//! Ignored by default: the run embeds, so the first one downloads the model.

use semlith::{Flow, Semlith};
use std::cell::Cell;

fn corpus(n: usize) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..n {
        std::fs::write(
            dir.path().join(format!("{i:02}.md")),
            format!("Note {i}: the tide tables for harbour {i} are posted weekly."),
        )
        .unwrap();
    }
    dir
}

/// A control that runs `k` files and then answers `then`.
fn after(k: usize, then: Flow) -> impl Fn() -> Flow {
    let asked = Cell::new(0usize);
    move || {
        asked.set(asked.get() + 1);
        if asked.get() > k { then } else { Flow::Run }
    }
}

#[test]
#[ignore = "downloads an embedding model on first run"]
fn stop_undoes_a_run_and_yield_then_rest_finishes_it() {
    const N: usize = 6;
    let files = corpus(N);
    let roots = vec![files.path().to_path_buf()];

    // Stop part-way: everything this run embedded is undone.
    let store = tempfile::tempdir().unwrap();
    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    let stopped = s
        .index_paths_under(&roots, &after(3, Flow::Stop), |_, _| {})
        .unwrap();
    assert!(stopped.stopped, "{stopped:?}");
    // One slice was the whole run, so its `written` is everything to undo.
    s.undo_run(&stopped.written).unwrap();
    assert_eq!(s.len(), 0, "a stopped run leaves no vector behind");
    assert_eq!(s.stats().unwrap().0, 0, "and no file row");

    // Yield part-way, then carry on from what it did not reach.
    let first = s
        .index_paths_under(&roots, &after(2, Flow::Yield), |_, _| {})
        .unwrap();
    assert!(!first.pending.is_empty(), "{first:?}");
    assert!(first.pending.len() < N);
    let rest = s
        .index_rest_under(first.pending.clone(), &|| Flow::Run, |_, _| {})
        .unwrap();
    assert!(rest.pending.is_empty());
    assert_eq!(first.indexed + rest.indexed, N);
    assert_eq!(s.stats().unwrap().0, N as i64);
}
