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

/// A filter must narrow both halves of the search *before* either picks its
/// top-k. Post-filtering a global ranking is the failure this guards: with the
/// subsystem a small minority of the corpus, it returns nothing.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_filtered_search_ranks_within_the_subset() {
    use semlith::filter::Filter;

    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    fs::create_dir(corpus.path().join("sub")).unwrap();

    for i in 0..4 {
        write(
            &corpus.path().join("sub"),
            &format!("retry_{i}.rs"),
            "fn backoff() { let delay = base * 2u32.pow(attempt); }\n\
             // Retries use full jitter, capped at MAX_BACKOFF.",
        );
    }
    // Enough noise on the same subject that an unfiltered top-k is all noise.
    for i in 0..60 {
        write(
            corpus.path(),
            &format!("noise_{i}.md"),
            "Notes on retry backoff and jitter, at length and in prose.",
        );
    }

    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    s.index_paths(&[corpus.path().to_path_buf()], |_| {})
        .unwrap();

    let scoped = Filter::new(&["sub/**".into()], &[], &[]).unwrap();
    let hits = s
        .search_filtered("retry backoff jitter", 4, &scoped)
        .unwrap();
    assert_eq!(
        hits.len(),
        4,
        "the subset holds four chunks; all four are due"
    );
    assert!(
        hits.iter().all(|h| h.path.contains("/sub/")),
        "a hit escaped the filter: {hits:#?}"
    );

    // The same query unscoped is dominated by the noise, which is exactly why
    // filtering after ranking would not work.
    let global = s.search("retry backoff jitter", 4).unwrap();
    assert!(
        global.iter().filter(|h| h.path.contains("/sub/")).count() < 4,
        "fixture is too easy: the global top-4 already is the subset"
    );

    // A language name reaches the same files as the extension it covers.
    let by_lang = s
        .search_filtered(
            "retry backoff jitter",
            4,
            &Filter::new(&[], &[], &["rust".into()]).unwrap(),
        )
        .unwrap();
    assert!(by_lang.iter().all(|h| h.path.ends_with(".rs")));

    // A filter that selects nothing returns nothing, rather than falling back
    // to an unfiltered search.
    let nowhere = Filter::new(&["nowhere/**".into()], &[], &[]).unwrap();
    assert_eq!(s.matching_files(&nowhere).unwrap(), 0);
    assert!(
        s.search_filtered("retry backoff jitter", 4, &nowhere)
            .unwrap()
            .is_empty()
    );
}

/// The store format is written down so that the first time it changes, an old
/// binary refuses the store instead of misreading it. That is only true if a
/// store from before the key existed still opens — which is every store any
/// user has today.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_store_written_before_the_format_key_still_opens_and_is_not_rewritten() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(corpus.path(), "bread.md", "Sourdough starter needs flour.");

    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    s.index_paths(&[corpus.path().to_path_buf()], |_| {})
        .unwrap();
    assert_eq!(
        semlith::store::get_meta(s.db(), semlith::store::FORMAT_KEY).unwrap(),
        Some(semlith::store::FORMAT_VERSION.to_string()),
        "a store this binary created must say what format it is"
    );
    drop(s);

    // Exactly what a 0.5.0 store looks like: everything else, minus the key.
    let db = rusqlite::Connection::open(store.path().join("store.db")).unwrap();
    db.execute(
        "DELETE FROM meta WHERE k = ?1",
        [semlith::store::FORMAT_KEY],
    )
    .unwrap();
    drop(db);

    let mut old = Semlith::open(store.path(), None).unwrap();
    old.quiet = true;
    assert_eq!(top(&mut old, "how do I bake bread"), "bread.md");
    assert_eq!(
        semlith::store::get_meta(old.db(), semlith::store::FORMAT_KEY).unwrap(),
        None,
        "opening a store must not migrate it behind the user's back"
    );
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
