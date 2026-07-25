//! End-to-end check of the index → search → update → forget cycle.
//!
//! Ignored by default: the first run downloads an embedding model. Run with
//!
//! ```sh
//! cargo test -- --ignored
//! ```

use semlith::Semlith;
use std::fs;
use std::path::Path;

/// The one test that would catch the failure that matters: chunk ids in the
/// vector index drifting out of sync with their rows in SQLite.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn index_search_update_forget() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();

    write(
        corpus.path(),
        "bread.md",
        "Sourdough starter needs flour and water fed daily.",
    );
    write(
        corpus.path(),
        "rust.md",
        "Rust ownership gives every value exactly one owner.",
    );
    write(
        corpus.path(),
        "cells.md",
        "Mitochondria are the powerhouse of the cell.",
    );
    // Binary content must be skipped rather than embedded as mojibake.
    fs::write(corpus.path().join("blob.bin"), [0u8, 1, 2, 3, 0, 5]).unwrap();

    let roots = vec![corpus.path().to_path_buf()];
    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;

    let report = s.index_paths(&roots, |_| {}).unwrap();
    assert_eq!(report.indexed, 3, "three text files, one binary skipped");
    assert_eq!(report.skipped, 1);
    assert_eq!(s.len(), report.chunks, "one vector per chunk");

    assert_eq!(top(&mut s, "how do I bake bread"), "bread.md");
    assert_eq!(top(&mut s, "what organelle produces energy"), "cells.md");

    // Unchanged corpus: nothing is re-embedded.
    let report = s.index_paths(&roots, |_| {}).unwrap();
    assert_eq!(report.indexed, 0);
    assert_eq!(report.unchanged, 3);

    // Edit one file, delete another. Both must be reflected.
    write(
        corpus.path(),
        "rust.md",
        "The borrow checker proves aliasing rules at compile time.",
    );
    fs::remove_file(corpus.path().join("cells.md")).unwrap();

    let report = s.index_paths(&roots, |_| {}).unwrap();
    assert_eq!(report.indexed, 1, "only the edited file is re-embedded");
    assert_eq!(report.removed, 1, "the deleted file is pruned");

    let hits = s.search("mitochondria powerhouse", 5).unwrap();
    assert!(
        !hits.iter().any(|h| h.path.ends_with("cells.md")),
        "deleted file still returned: {hits:#?}"
    );
    assert_eq!(top(&mut s, "compile time aliasing proof"), "rust.md");

    // Reopening must see the same corpus: the index survived the round trip
    // to disk, and its ids still resolve against SQLite.
    let vectors = s.len();
    drop(s);
    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    assert_eq!(s.len(), vectors);
    assert_eq!(top(&mut s, "how do I bake bread"), "bread.md");

    // And an explicit forget removes it from both halves of the store.
    let removed = s.forget(&corpus.path().join("bread.md")).unwrap();
    assert!(removed > 0);
    assert_eq!(s.len(), vectors - removed);
    let hits = s.search("sourdough flour water", 5).unwrap();
    assert!(!hits.iter().any(|h| h.path.ends_with("bread.md")));
}

fn write(dir: &Path, name: &str, body: &str) {
    fs::write(dir.join(name), body).unwrap();
}

/// File name of the best hit for `query`.
fn top(s: &mut Semlith, query: &str) -> String {
    let hits = s.search(query, 3).unwrap();
    let best = hits
        .first()
        .unwrap_or_else(|| panic!("no hits for {query:?}"));
    Path::new(&best.path)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}
