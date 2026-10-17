//! SK32 (cloud + core audit, 2026-10-17): a tree file swapped for a symlink
//! is never followed, by `read` (S05) or by the sweep that drops files gone
//! from disk.

#[allow(unused_imports)]
use super::*;

#[cfg(unix)]
#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn sk32_a_file_swapped_for_a_symlink_leaves_the_store_at_the_next_index() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let corpus = dir.path().join("corpus");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&corpus).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let file = corpus.join("notes.md");
    std::fs::write(&file, "# Notes\n\nNothing to see here, line three.\n").unwrap();
    std::fs::write(corpus.join("kept.md"), "# Kept\n\nStill a plain file.\n").unwrap();
    std::fs::write(outside.join("config"), "OUTSIDE-THE-STORE line\n").unwrap();
    let out = run(&home, &["index", corpus.to_str().unwrap()], &[]);
    assert!(out.status.success());

    // The file becomes a link to something outside, and the tree is indexed
    // again: the walk does not follow the link, so the sweep must not either.
    // Before 0.43.0 it asked `exists()`, which answered for the target, and
    // the row for notes.md stayed.
    std::fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(outside.join("config"), &file).unwrap();
    let out = run(&home, &["index", corpus.to_str().unwrap()], &[]);
    assert!(out.status.success());

    let out = run(&home, &["files"], &[]);
    let listed = String::from_utf8_lossy(&out.stdout);
    assert!(listed.contains("kept.md"), "{listed}");
    assert!(
        !listed.contains("notes.md"),
        "the symlinked file kept its row:\n{listed}"
    );
}
