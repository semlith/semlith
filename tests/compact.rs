//! 0.31.0's gate: a store churned by a fixed series of edits, renames and
//! deletions, compacted, takes within 10 % of what a fresh index of the same
//! final tree takes, and answers every query exactly as it did before.
//!
//! Needs the embedding model, so it is ignored by default:
//! `cargo test --test compact -- --ignored`.

use semlith::Semlith;
use semlith::compact::CompactOptions;
use std::fs;
use std::path::Path;

/// A Rust file of a few definitions, different for every `(i, version)`.
fn source(i: usize, version: usize) -> String {
    let mut out = format!("//! Module {i}, version {version}.\n\n");
    for f in 0..6 {
        out.push_str(&format!(
            "/// Computes the {f}th figure of module {i} for release {version}, \
             retrying with backoff when the store is busy.\n\
             pub fn figure_{i}_{f}(input: u64) -> u64 {{\n    \
             let base = input.wrapping_mul({});\n    \
             base.rotate_left({}) ^ {}\n}}\n\n",
            i * 7 + f + version,
            (f + version) % 31,
            i * 1000 + f * 10 + version,
        ));
    }
    out
}

fn index(store: &Path, tree: &Path) {
    let mut s = Semlith::open(store, None).unwrap();
    s.quiet = true;
    s.index_paths(&[tree.to_path_buf()], |_, _| {}).unwrap();
}

fn answers(store: &Path) -> Vec<Vec<(String, u32)>> {
    let mut s = Semlith::open(store, None).unwrap();
    s.quiet = true;
    [
        "retrying with backoff",
        "figure of module 12",
        "rotate left",
        "release 3",
    ]
    .iter()
    .map(|q| {
        s.search(q, 8)
            .unwrap()
            .into_iter()
            .map(|hit| (hit.path.clone(), hit.start_line))
            .collect()
    })
    .collect()
}

#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_churned_store_compacts_to_within_ten_percent_of_a_fresh_one() {
    let tree = tempfile::tempdir().unwrap();
    let churned = tempfile::tempdir().unwrap();
    let fresh = tempfile::tempdir().unwrap();
    let files = 40;
    for i in 0..files {
        fs::write(tree.path().join(format!("m{i:02}.rs")), source(i, 0)).unwrap();
    }
    index(churned.path(), tree.path());

    // Five rounds of edits to the whole tree, five renames and five
    // deletions: every one re-indexed incrementally, the way a watcher keeps a
    // store.
    for round in 1..=5 {
        for i in 0..files {
            fs::write(tree.path().join(format!("m{i:02}.rs")), source(i, round)).unwrap();
        }
        index(churned.path(), tree.path());
    }
    for i in 0..5 {
        fs::rename(
            tree.path().join(format!("m{i:02}.rs")),
            tree.path().join(format!("renamed_{i:02}.rs")),
        )
        .unwrap();
        fs::remove_file(tree.path().join(format!("m{:02}.rs", 35 + i))).unwrap();
    }
    index(churned.path(), tree.path());

    let before = answers(churned.path());
    let mut store = Semlith::open(churned.path(), None).unwrap();
    let report = store.compact(&CompactOptions::default()).unwrap();
    drop(store);
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    assert!(
        report.before.total() > report.after.total() * 3 / 2,
        "the churn left {} and compaction {}; the fixture is not churning enough to test anything",
        report.before.total(),
        report.after.total()
    );
    assert_eq!(
        answers(churned.path()),
        before,
        "compaction changed an answer"
    );

    index(fresh.path(), tree.path());
    let fresh_total = Semlith::open(fresh.path(), None)
        .unwrap()
        .footprint(0)
        .unwrap()
        .total();
    let compacted = report.after.total();

    // What the retention keeps is live by definition -- `symbol --history`
    // answers from it -- and a fresh index has none. So the comparison with a
    // fresh index is made on a copy of the compacted store with that history
    // taken out, and the history's own size is printed beside it: on this
    // fixture every definition is rewritten five times, so the history is the
    // largest thing a small store holds.
    let without = tempfile::tempdir().unwrap();
    for entry in fs::read_dir(churned.path()).unwrap().flatten() {
        let path = entry.path();
        if path.is_file() {
            fs::copy(&path, without.path().join(entry.file_name())).unwrap();
        }
    }
    let db = rusqlite::Connection::open(without.path().join("store.db")).unwrap();
    db.execute_batch("DELETE FROM symbols_past; VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    drop(db);
    let db_bytes = |dir: &Path| -> u64 {
        ["store.db", "store.db-wal", "store.db-shm"]
            .iter()
            .filter_map(|n| fs::metadata(dir.join(n)).ok())
            .map(|m| m.len())
            .sum()
    };
    let history = db_bytes(churned.path()).saturating_sub(db_bytes(without.path()));
    let comparable = compacted - history;
    println!(
        "churned {} B, compacted {compacted} B ({history} B of it retained history), \
         fresh {fresh_total} B; compacted less history {comparable} B ({:+.1} %)",
        report.before.total(),
        (comparable as f64 / fresh_total as f64 - 1.0) * 100.0
    );
    assert!(
        (comparable as f64) <= fresh_total as f64 * 1.10,
        "compacted less its retained history is {comparable} B against a fresh index's \
         {fresh_total} B, over 10 %"
    );
}

/// Found measuring 0.31.0 on this repository's own store: 134 files that
/// 0.30.0's `.semlithignore` left out were still held, because only a file
/// gone from disk was ever swept. A file a rule now excludes leaves the store
/// on the next pass.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_file_an_ignore_rule_now_excludes_leaves_the_store() {
    let tree = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    fs::create_dir_all(tree.path().join("sub")).unwrap();
    fs::write(
        tree.path().join("keep.md"),
        "# Keep\n\nThe kept note about harbours.\n",
    )
    .unwrap();
    fs::write(
        tree.path().join("sub/drop.md"),
        "# Drop\n\nThe dropped note about lighthouses.\n",
    )
    .unwrap();
    index(store.path(), tree.path());
    let held = |store: &Path| -> Vec<String> {
        semlith::store::all_paths(Semlith::open(store, None).unwrap().db()).unwrap()
    };
    assert_eq!(held(store.path()).len(), 2);

    fs::write(tree.path().join(".semlithignore"), "sub/\n").unwrap();
    index(store.path(), tree.path());
    let after = held(store.path());
    assert_eq!(after.len(), 1, "{after:?}");
    assert!(after[0].ends_with("keep.md"), "{after:?}");
}
