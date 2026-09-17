//! A store is about its roots.
//!
//! The 2026-09-17 drive found the `semlith` store holding 262 files that belong
//! to `ultraship`: every search across all stores returned them twice, once
//! under each label, and the label on the duplicate was simply wrong. The cause
//! was the boundary rule, which treated the whole home directory as inside
//! every store's boundary — so any store on the machine could swallow any
//! other's corpus and be allowed.
//!
//! These tests hold both halves of the fix: the rule that stops it happening
//! again, and the reconciliation that clears up what is already there.
//!
//! ```sh
//! cargo test --test reconcile -- --ignored
//! ```

use semlith::Semlith;
use std::fs;
use std::path::{Path, PathBuf};

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, text).unwrap();
    path
}

#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_store_drops_the_files_it_holds_from_outside_its_roots() {
    let mine = tempfile::tempdir().unwrap();
    let theirs = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();

    write(mine.path(), "ours.md", "Sourdough needs flour and water.");
    write(theirs.path(), "theirs.md", "Rye needs a longer prove.");

    // Indexed together, which is how the contamination got in: one run, two
    // trees, one store.
    let mut store = Semlith::open(dir.path(), None).unwrap();
    store.quiet = true;
    store
        .index_paths(
            &[mine.path().to_path_buf(), theirs.path().to_path_buf()],
            |_, _| {},
        )
        .unwrap();
    assert_eq!(store.stats().unwrap().0, 2, "both trees are in the store");

    let roots = vec![mine.path().to_path_buf()];

    // The dry run counts and changes nothing, so the number can be looked at
    // before anything is dropped.
    assert_eq!(store.out_of_root(&roots).unwrap().len(), 1);
    assert_eq!(store.stats().unwrap().0, 2, "report changes nothing");

    assert_eq!(store.prune_out_of_root(&roots).unwrap(), 1);
    assert_eq!(store.stats().unwrap().0, 1, "only the in-root file is left");
    assert!(store.out_of_root(&roots).unwrap().is_empty());

    // The file on disk is untouched. It was never this store's to hold, and it
    // is still there for the store that owns it.
    assert!(theirs.path().join("theirs.md").exists());
}

#[test]
fn a_store_with_no_registered_roots_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let store = Semlith::open(dir.path(), None).unwrap();
    // Nothing to be outside of. Dropping a whole store's contents on the
    // strength of an empty root list is the one outcome worth ruling out.
    assert!(store.out_of_root(&[]).unwrap().is_empty());
}
