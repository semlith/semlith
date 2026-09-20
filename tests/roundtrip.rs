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

    let report = s.index_paths(&roots, |_, _| {}).unwrap();
    assert_eq!(report.indexed, 3, "three text files, one binary skipped");
    assert_eq!(report.skipped, 1);
    assert_eq!(s.len(), report.chunks, "one vector per chunk");

    assert_eq!(top(&mut s, "how do I bake bread"), "bread.md");
    assert_eq!(top(&mut s, "what organelle produces energy"), "cells.md");

    // Unchanged corpus: nothing is re-embedded.
    let report = s.index_paths(&roots, |_, _| {}).unwrap();
    assert_eq!(report.indexed, 0);
    assert_eq!(report.unchanged, 3);

    // Edit one file, delete another. Both must be reflected.
    write(
        corpus.path(),
        "rust.md",
        "The borrow checker proves aliasing rules at compile time.",
    );
    fs::remove_file(corpus.path().join("cells.md")).unwrap();

    let report = s.index_paths(&roots, |_, _| {}).unwrap();
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
    let (removed, images) = s.forget(&corpus.path().join("bread.md")).unwrap();
    assert!(removed > 0);
    assert_eq!(images, 0, "a markdown file has no image vectors");
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
    s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
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
    s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
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

/// A store from before definition chunking opens, answers, and is re-chunked by
/// the next full index pass — not before, and not by opening it.
///
/// The migration 0.25.0 owes. A store 0.24.0 wrote holds code chunks the
/// model was shown without the definition they sit inside, and nothing on disk
/// has changed, so only the format row can ask for the work.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_store_without_code_context_is_re_embedded_by_the_next_full_pass() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "walk.rs",
        "/// How many nodes a traversal may visit before it gives up.\n\
         pub fn walk(graph: &Graph, max_nodes: usize) -> Vec<Node> {\n\
             visit(graph, max_nodes)\n\
         }\n",
    );

    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();
    drop(s);

    // Put the store back the way 0.24.0 left it: one format behind, its code
    // chunks embedded from their own text alone.
    let db = rusqlite::Connection::open(store.path().join("store.db")).unwrap();
    db.execute(
        "UPDATE meta SET v = ?1 WHERE k = ?2",
        rusqlite::params![
            (semlith::store::CODE_CONTEXT - 1).to_string(),
            semlith::store::FORMAT_KEY
        ],
    )
    .unwrap();
    drop(db);

    // It opens unchanged, it answers, and opening it migrated nothing.
    let mut old = Semlith::open(store.path(), None).unwrap();
    old.quiet = true;
    assert_eq!(
        semlith::store::chunking(old.db()).unwrap(),
        "headings; code carries no definition until it is re-indexed"
    );
    assert_eq!(top(&mut old, "walk"), "walk.rs");
    assert_eq!(
        semlith::store::format(old.db()).unwrap(),
        semlith::store::CODE_CONTEXT - 1,
        "opening a store must not migrate it behind the user's back"
    );

    // The next full pass re-embeds it, although nothing on disk changed, and
    // says that it did.
    let report = old
        .index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();
    assert!(report.rechunked, "the pass did not say why it re-indexed");
    assert_eq!(
        report.unchanged, 0,
        "the pass left the old embedding in place"
    );
    assert_eq!(report.indexed, 1, "the pass did not re-index the file");
    assert_eq!(
        semlith::store::format(old.db()).unwrap(),
        semlith::store::FORMAT_VERSION,
        "a swept store must record the format it is now on"
    );
    assert_eq!(top(&mut old, "walk"), "walk.rs", "and it still answers");
}

/// The migration 0.22.0 owes. A store on format 2 holds fixed windows and its
/// files have not changed, so the hash check would leave it holding them for
/// ever: the rule for what a chunk is changed, not the corpus. So the first
/// pass that sweeps the whole store re-chunks everything it walks, and only
/// then does the format row move.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_store_on_fixed_windows_is_re_chunked_by_the_next_full_pass() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    // Two definitions, each with a doc comment above it, and enough prose
    // between them that a fixed 800-character window cannot hold both.
    write(
        corpus.path(),
        "limits.rs",
        &format!(
            "/// How many nodes a traversal may visit.\n\
             pub const MAX_NODES: usize = 4000;\n\
             {}\n\
             /// How long a parse may take before it is abandoned.\n\
             pub const PARSE_TIMEOUT: u64 = 5;\n",
            (1..=40)
                .map(|i| format!("// filler line {i} standing between the two definitions"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    );

    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();
    drop(s);

    // Put the store back the way 0.21.0 would have left it: the format row one
    // behind, and the chunks cut by the fixed window.
    let db = rusqlite::Connection::open(store.path().join("store.db")).unwrap();
    db.execute(
        "UPDATE meta SET v = ?1 WHERE k = ?2",
        rusqlite::params![
            (semlith::store::DEFINITION_CHUNKS - 1).to_string(),
            semlith::store::FORMAT_KEY
        ],
    )
    .unwrap();
    drop(db);

    // It opens unchanged, and it answers.
    let mut old = Semlith::open(store.path(), None).unwrap();
    old.quiet = true;
    assert_eq!(
        semlith::store::chunking(old.db()).unwrap(),
        "fixed windows",
        "a store one format behind must say which rule its chunks were cut by"
    );
    assert_eq!(top(&mut old, "MAX_NODES"), "limits.rs");
    assert_eq!(
        semlith::store::format(old.db()).unwrap(),
        semlith::store::DEFINITION_CHUNKS - 1,
        "opening a store must not migrate it behind the user's back"
    );

    // The next full pass re-chunks it, although nothing on disk changed.
    let report = old
        .index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();
    assert_eq!(
        report.unchanged, 0,
        "the pass treated the file as unchanged and left the old chunking in place"
    );
    assert_eq!(report.indexed, 1, "the pass did not re-index the file");
    assert_eq!(
        semlith::store::chunking(old.db()).unwrap(),
        "headings, and code with its definition",
        "a swept store must record the rule its chunks are now cut by"
    );
    assert!(
        report.rechunked,
        "a pass that re-embedded an unchanged corpus must say so, or the user is left \
         watching their whole repository index again with no reason given"
    );

    // And the chunks are cut where the definitions are.
    let holding: String = old
        .db()
        .query_row(
            "SELECT text FROM chunks WHERE text LIKE '%MAX_NODES%' LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        holding.starts_with("/// How many nodes"),
        "the chunk holding the constant does not begin at its doc comment:\n{holding}"
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

/// The weights are what computes every vector in every store, so a cache
/// holding a file that is not the one this release pins is refused by name
/// rather than loaded. A model that changed under a user would change what
/// their corpus means without changing anything they can see.
#[test]
#[ignore = "copies the real model cache, so it needs one"]
fn a_model_file_that_is_not_the_pinned_one_is_refused_by_name() {
    let real = real_cache();
    let snapshot = real
        .join("models--onnx-community--granite-embedding-small-english-r2-ONNX")
        .join("snapshots")
        .join(semlith::embed::GRANITE_REVISION);
    if !snapshot.is_dir() {
        eprintln!(
            "no cached granite snapshot at {}; run the suite once first",
            snapshot.display()
        );
        return;
    }

    let cache = tempfile::tempdir().unwrap();
    let into = cache
        .path()
        .join("models--onnx-community--granite-embedding-small-english-r2-ONNX")
        .join("snapshots")
        .join(semlith::embed::GRANITE_REVISION);
    copy_tree(&snapshot, &into);

    // One byte. A corrupted download and a substituted one look the same from
    // here, which is the point.
    let tokenizer = into.join("tokenizer.json");
    let mut bytes = fs::read(&tokenizer).unwrap();
    let at = bytes.len() / 2;
    bytes[at] ^= 0x01;
    fs::write(&tokenizer, &bytes).unwrap();

    let corpus = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "a.md",
        "Ownership means each value has one owner.",
    );
    let store = tempfile::tempdir().unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["index", corpus.path().to_str().unwrap(), "--quiet"])
        .arg("--store")
        .arg(store.path())
        .env("SEMLITH_MODEL_CACHE", cache.path())
        .output()
        .expect("running semlith index");
    assert!(!out.status.success(), "a tampered model file was loaded");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("tokenizer.json") && said.contains("does not match the digest"),
        "the refusal does not name the file and the digest:\n{said}"
    );
    assert!(
        said.contains("expected") && said.contains("got"),
        "the refusal does not show both digests:\n{said}"
    );
}

/// A cache another account can write to is a model another account chooses, and
/// the choosing is invisible: a corpus embedded by a different model still
/// answers, just differently.
#[test]
#[cfg(unix)]
fn a_model_cache_anyone_can_write_to_is_refused() {
    use std::os::unix::fs::PermissionsExt;

    let cache = tempfile::tempdir().unwrap();
    fs::set_permissions(cache.path(), fs::Permissions::from_mode(0o777)).unwrap();

    let corpus = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "a.md",
        "Ownership means each value has one owner.",
    );
    let store = tempfile::tempdir().unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["index", corpus.path().to_str().unwrap(), "--quiet"])
        .arg("--store")
        .arg(store.path())
        .env("SEMLITH_MODEL_CACHE", cache.path())
        .output()
        .expect("running semlith index");
    assert!(
        !out.status.success(),
        "a world-writable model cache was used"
    );
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("777") && said.contains("chmod 700"),
        "the refusal should name the mode and the fix:\n{said}"
    );
}

fn real_cache() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("SEMLITH_MODEL_CACHE") {
        return std::path::PathBuf::from(dir);
    }
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join(".cache")
        .join("semlith")
        .join("models")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            // The cache is symlinks into `blobs/`, so the bytes are copied
            // rather than the links, which is what makes the tamper local to
            // this test's own cache.
            let bytes = fs::read(entry.path()).unwrap();
            fs::write(&target, bytes).unwrap();
        }
    }
}

/// The stamp that keeps a load from hashing 52 MB every time must not become a
/// way to skip the check. A file that changed is a file whose size or age
/// changed, so the stamp stops matching and the digests are read again.
#[test]
#[ignore = "copies the real model cache, so it needs one"]
fn the_verification_stamp_does_not_outlive_the_bytes_it_is_about() {
    let real = real_cache();
    let snapshot = real
        .join("models--onnx-community--granite-embedding-small-english-r2-ONNX")
        .join("snapshots")
        .join(semlith::embed::GRANITE_REVISION);
    if !snapshot.is_dir() {
        eprintln!("no cached granite snapshot; run the suite once first");
        return;
    }

    let cache = tempfile::tempdir().unwrap();
    let into = cache
        .path()
        .join("models--onnx-community--granite-embedding-small-english-r2-ONNX")
        .join("snapshots")
        .join(semlith::embed::GRANITE_REVISION);
    copy_tree(&snapshot, &into);

    let corpus = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "a.md",
        "Ownership means each value has one owner.",
    );

    let index = |store: &Path| {
        std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
            .args(["index", corpus.path().to_str().unwrap(), "--quiet"])
            .arg("--store")
            .arg(store)
            .env("SEMLITH_MODEL_CACHE", cache.path())
            .output()
            .expect("running semlith index")
    };

    // The first run verifies and leaves a stamp; the second trusts it.
    let first = tempfile::tempdir().unwrap();
    assert!(index(first.path()).status.success());
    assert!(
        into.join(".semlith-verified").exists(),
        "the first load left no stamp, so every load pays for the digests"
    );

    let second = tempfile::tempdir().unwrap();
    assert!(index(second.path()).status.success());

    // Now change a file without touching the stamp. Its size and age move, so
    // the stamp no longer describes it and the digest is read again.
    let tokenizer = into.join("tokenizer.json");
    let mut bytes = fs::read(&tokenizer).unwrap();
    let at = bytes.len() / 2;
    bytes[at] ^= 0x01;
    fs::write(&tokenizer, &bytes).unwrap();

    let third = tempfile::tempdir().unwrap();
    let out = index(third.path());
    assert!(
        !out.status.success(),
        "the stamp let a changed file through"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("does not match the digest"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `forget` has to remove the file the listing just printed.
///
/// The library-level cycle above already covers `Semlith::forget`, and it
/// passes — which is exactly why this one goes through the binary. The command
/// resolves its store from the working directory rather than from the path it
/// was handed, so a `forget` run from anywhere but the corpus asks a store that
/// has never heard of the file, removes nothing, and says so while exiting 0.
/// That is #77, and a caller that believes the exit status believes the file is
/// gone.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn forget_removes_the_file_the_listing_printed() {
    let corpus = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    // Deliberately not the corpus: a user forgetting a file is usually standing
    // somewhere else, and so is every agent.
    let elsewhere = tempfile::tempdir().unwrap();

    write(
        corpus.path(),
        "bread.md",
        "Sourdough starter needs flour and water fed daily.",
    );
    write(
        corpus.path(),
        "cells.md",
        "Mitochondria are the powerhouse of the cell.",
    );

    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
            .args(args)
            .current_dir(elsewhere.path())
            .env("SEMLITH_HOME", home.path())
            .output()
            .unwrap()
    };

    let indexed = run(&["index", "--quiet", &corpus.path().display().to_string()]);
    assert!(
        indexed.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&indexed.stderr)
    );

    let before = run(&["files"]);
    let listing = String::from_utf8_lossy(&before.stdout).into_owned();
    let target = listing
        .lines()
        .find(|line| line.trim().ends_with("bread.md"))
        .unwrap_or_else(|| {
            panic!("bread.md is not in the listing, so this proves nothing:\n{listing}")
        })
        .trim()
        .to_string();

    let forgotten = run(&["forget", &target]);
    let said = String::from_utf8_lossy(&forgotten.stderr).into_owned();
    assert!(
        forgotten.status.success(),
        "forget of a listed path failed: {said}"
    );
    assert!(
        !said.contains("nothing"),
        "forget removed nothing from the store that holds the file: {said}"
    );

    let after = String::from_utf8_lossy(&run(&["files"]).stdout).into_owned();
    assert!(
        !after.lines().any(|line| line.trim().ends_with("bread.md")),
        "still listed after forget:\n{after}"
    );
    assert!(
        after.lines().any(|line| line.trim().ends_with("cells.md")),
        "forget took the wrong file with it:\n{after}"
    );

    // And a path nothing ever indexed is not a success.
    let missing = corpus.path().join("there-is-no-such-file.md");
    let out = run(&["forget", &missing.display().to_string()]);
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !out.status.success(),
        "forgetting an unindexed path exited 0: {said}"
    );
    assert!(
        said.contains("nothing to forget"),
        "the message should name what did not happen: {said}"
    );
}

/// `drop` is checked in the same pass as `forget`, because it is the same
/// shape of claim: the command says a store is gone, so the directory and the
/// registry entry both have to be.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn drop_removes_the_directory_and_the_registry_entry() {
    let corpus = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();

    write(
        corpus.path(),
        "bread.md",
        "Sourdough starter needs flour and water fed daily.",
    );

    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
            .args(args)
            .current_dir(elsewhere.path())
            .env("SEMLITH_HOME", home.path())
            .output()
            .unwrap()
    };

    let indexed = run(&["index", "--quiet", &corpus.path().display().to_string()]);
    assert!(
        indexed.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&indexed.stderr)
    );

    let registry = home.path().join("registry.json");
    let named: Vec<String> =
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&registry).unwrap()).unwrap()
            ["stores"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
    let [name] = named.as_slice() else {
        panic!("one index run should make one store, not {named:?}");
    };
    let dir = home.path().join("stores").join(name);
    assert!(dir.is_dir(), "{} was never created", dir.display());

    let dropped = run(&["drop", name, "--yes"]);
    assert!(
        dropped.status.success(),
        "drop failed: {}",
        String::from_utf8_lossy(&dropped.stderr)
    );
    assert!(!dir.exists(), "{} is still on disk", dir.display());
    let after =
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&registry).unwrap()).unwrap();
    assert!(
        after["stores"].as_object().unwrap().is_empty(),
        "the registry still lists the dropped store: {after}"
    );
}

/// A path that cannot be read creates nothing, registers nothing, and exits
/// non-zero.
///
/// Opening a store is what creates it, so a check that came after the open left
/// an empty store registered under the typo's own name and reported success
/// (#76).
#[test]
#[ignore = "downloads an embedding model on first run"]
fn indexing_a_path_that_cannot_be_read_leaves_nothing_behind() {
    let home = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let missing = elsewhere.path().join("there-is-no-such-directory");

    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
            .args(args)
            .current_dir(elsewhere.path())
            .env("SEMLITH_HOME", home.path())
            .output()
            .unwrap()
    };

    let out = run(&["index", "--quiet", &missing.display().to_string()]);
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !out.status.success(),
        "a missing path indexed cleanly: {said}"
    );
    assert!(
        said.contains("there-is-no-such-directory"),
        "the error should name the path: {said}"
    );
    assert!(
        !home.path().join("stores").exists(),
        "a store directory was created for a path that cannot be read"
    );
    assert!(
        !home.path().join("registry.json").exists(),
        "the registry records a store for a path that cannot be read"
    );

    // A readable root beside an unreadable one is still indexed, and the run
    // still fails, so a script sees it.
    let good = elsewhere.path().join("good");
    std::fs::create_dir_all(&good).unwrap();
    write(
        &good,
        "notes.md",
        "Sourdough starter needs flour and water.",
    );

    let out = run(&[
        "index",
        "--quiet",
        &good.display().to_string(),
        &missing.display().to_string(),
    ]);
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        !out.status.success(),
        "a partial failure reported success: {said}"
    );
    assert!(
        said.contains("there-is-no-such-directory"),
        "the unreadable root is not named: {said}"
    );

    let registry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.path().join("registry.json")).unwrap())
            .unwrap();
    let recorded = registry["stores"].as_object().unwrap();
    assert_eq!(
        recorded.len(),
        1,
        "one readable root, one store: {registry}"
    );
    let roots = recorded.values().next().unwrap()["roots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert!(
        roots.iter().all(|r| !r.contains("there-is-no-such")),
        "an unreadable root was registered: {roots:?}"
    );

    let listed = String::from_utf8_lossy(&run(&["files"]).stdout).into_owned();
    assert!(
        listed.lines().any(|l| l.trim().ends_with("notes.md")),
        "the readable root was not indexed:\n{listed}"
    );
}
