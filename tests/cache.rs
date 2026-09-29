//! The machine-wide vector cache, end to end: an edit re-embeds only what
//! changed, a second worktree re-embeds almost nothing, what the cache hands
//! back is the vector it was given, and compaction leaves it alone.
//!
//! Ignored by default: the first run downloads the embedding model.
//!
//! ```sh
//! cargo test --test cache -- --ignored --test-threads=1
//! ```

use semlith::Semlith;
use std::path::Path;

/// One process-wide home for the cache, set before anything reads it. Every
/// test in this binary shares it; each builds its own stores.
fn home() -> &'static Path {
    static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let dir = HOME.get_or_init(|| tempfile::tempdir().unwrap());
    // SAFETY: set once, before any test in this binary opens a store; every
    // test sets the same values.
    unsafe {
        std::env::set_var("SEMLITH_HOME", dir.path());
        std::env::set_var(semlith::cache::CAP_ENV, "256");
    }
    dir.path()
}

/// A small code repository: `files` files of eight functions each, every
/// function different and long enough to be a chunk of its own.
fn repository(at: &Path, files: usize) {
    for f in 0..files {
        let mut text = String::new();
        for g in 0..8 {
            text.push_str(&format!(
                "/// Returns the {g}th measure of widget {f}, in millimetres, after the\n\
                 /// calibration the bench applies to every widget of this family: the\n\
                 /// scale is squared, offset by the widget's own constant, and the\n\
                 /// result is clamped so a mis-read gauge cannot report a negative.\n\
                 pub fn widget_{f}_measure_{g}(scale: f64) -> f64 {{\n    \
                 let base = {f}.0 * {g}.5 + scale;\n    \
                 let squared = base * base - {g}.0;\n    \
                 let offset = squared + {f}.25 * {g}.125;\n    \
                 offset.max(0.0)\n}}\n\n"
            ));
        }
        std::fs::write(at.join(format!("widget_{f}.rs")), text).unwrap();
    }
}

fn index(store: &Path, root: &Path) -> semlith::IndexReport {
    let mut s = Semlith::open(store, None).unwrap();
    s.quiet = true;
    s.index_paths(&[root.to_path_buf()], |_, _| {}).unwrap()
}

#[test]
#[ignore = "downloads an embedding model on first run"]
fn an_edit_re_embeds_only_what_changed_and_a_worktree_almost_nothing() {
    home();
    let repo = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    repository(repo.path(), 20);

    let first = index(store.path(), repo.path());
    assert!(first.chunks >= 100, "{} chunks", first.chunks);
    let embedded_first = first.cache_lookups - first.cache_hits;

    // Edit one function in each of five files.
    for f in 0..5 {
        let path = repo.path().join(format!("widget_{f}.rs"));
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace(&format!("{f}.0 * 3.5"), &format!("{f}.0 * 3.75"));
        assert!(text.contains("3.75"));
        std::fs::write(&path, text).unwrap();
    }
    let edited = index(store.path(), repo.path());
    assert_eq!(edited.indexed, 5, "five files changed");
    let embedded_edit = edited.cache_lookups - edited.cache_hits;
    assert!(
        embedded_edit * 5 <= edited.cache_lookups,
        "an edit of one function in five files re-embedded {embedded_edit} of {} chunks",
        edited.cache_lookups
    );

    // The same repository in a second worktree, into a second store.
    let worktree = tempfile::tempdir().unwrap();
    repository(worktree.path(), 20);
    for f in 0..5 {
        std::fs::copy(
            repo.path().join(format!("widget_{f}.rs")),
            worktree.path().join(format!("widget_{f}.rs")),
        )
        .unwrap();
    }
    let second = tempfile::tempdir().unwrap();
    let other = index(second.path(), worktree.path());
    let embedded_other = other.cache_lookups - other.cache_hits;
    assert!(
        embedded_other * 10 < other.cache_lookups,
        "a second worktree re-embedded {embedded_other} of {} chunks",
        other.cache_lookups
    );
    assert!(embedded_first > embedded_edit);
}

#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_cached_vector_is_the_vector_the_cache_was_given_and_compaction_leaves_it() {
    home();
    let repo = tempfile::tempdir().unwrap();
    repository(repo.path(), 6);
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    index(a.path(), repo.path());
    let second = index(b.path(), repo.path());
    assert_eq!(
        second.cache_hits, second.cache_lookups,
        "all from the cache"
    );

    // Store B was filled from the cache; its full-precision sidecar holds, for
    // every chunk, exactly the bytes store A embedded.
    let read = |dir: &Path| std::fs::read(dir.join("exact.f32")).unwrap();
    let (ea, eb) = (read(a.path()), read(b.path()));
    let record = 8 + 384 * 4;
    assert_eq!(ea.len(), eb.len());
    for (ra, rb) in ea.chunks(record).zip(eb.chunks(record)) {
        assert_eq!(
            &ra[8..],
            &rb[8..],
            "a cached vector differs from the one embedded"
        );
    }

    let before = semlith::cache::stats();
    let mut s = Semlith::open(a.path(), None).unwrap();
    s.compact(&semlith::compact::CompactOptions::default())
        .unwrap();
    let after = semlith::cache::stats();
    assert_eq!(before.rows, after.rows, "compaction touched the cache");
}
