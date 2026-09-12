//! The store home: where a new store goes, which store a command resolves to,
//! and what `semlith adopt` does to one that was written before the home
//! existed.
//!
//! Every test here runs the real binary in a subprocess with `SEMLITH_HOME`
//! and `HOME` pointed at a temporary directory, because resolution reads the
//! environment and a test that set it in-process would race every other test
//! in the binary.
//!
//! Most of them are offline: creating a store, reading it and moving it needs
//! no embedding model. The one test that proves a bare `semlith index` creates
//! a store in the home does embed, so it downloads the model on first run:
//!
//! ```sh
//! cargo test --test home -- --ignored
//! ```

use semlith::Semlith;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A temporary home and a working tree beside it.
struct Sandbox {
    _dir: tempfile::TempDir,
    home: PathBuf,
    work: PathBuf,
}

fn sandbox(tag: &str) -> Sandbox {
    let dir = tempfile::Builder::new()
        .prefix(&format!("semlith-home-{tag}-"))
        .tempdir()
        .expect("a temporary directory");
    let home = dir.path().join("home");
    let work = dir.path().join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    Sandbox {
        _dir: dir,
        home,
        work,
    }
}

impl Sandbox {
    /// Run semlith in `cwd` with this sandbox's home.
    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_semlith"))
            .args(args)
            .current_dir(cwd)
            .env("SEMLITH_HOME", &self.home)
            // Set too, so a resolution bug that reaches past SEMLITH_HOME
            // lands in the sandbox rather than in the developer's real home.
            .env("HOME", &self.home)
            // The weights are a cache, not part of the sandbox: without this
            // the reset HOME would make every run download 52 MB again.
            .env("SEMLITH_MODEL_CACHE", real_model_cache())
            .env_remove("SEMLITH_STORE")
            .output()
            .expect("semlith runs")
    }

    fn stores_root(&self) -> PathBuf {
        self.home.join("stores")
    }

    fn registry(&self) -> serde_json::Value {
        let text = std::fs::read_to_string(self.home.join("registry.json"))
            .expect("a registry was written");
        serde_json::from_str(&text).expect("the registry is JSON")
    }
}

/// Where this machine already keeps the model weights.
fn real_model_cache() -> PathBuf {
    if let Ok(dir) = std::env::var("SEMLITH_MODEL_CACHE") {
        return PathBuf::from(dir);
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
        .join(".cache")
        .join("semlith")
        .join("models")
}

/// A store with real rows in it, made without embedding anything.
///
/// `Semlith::open` creates the SQLite side and the index layout; the model is
/// only loaded when something is embedded. That is what lets the move, the
/// resolution order and the registry all be proven offline.
fn empty_store(dir: &Path) {
    let store = Semlith::open(dir, None).expect("a store opens");
    assert_eq!(store.len(), 0);
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

// ---------------------------------------------------------------- T01

/// The second step of the resolution order: a `.semlith` beside the corpus
/// wins over anything in the home, so nobody's existing setup changes when
/// they install 0.9.0.
///
/// The hint is the other half of this. A store found this way still works
/// exactly as it did, and the line is the only place a user learns that moving
/// it into the home is one command rather than a migration.
#[test]
fn a_local_store_still_wins_and_says_how_to_move_it() {
    let s = sandbox("local-wins");
    empty_store(&s.work.join(".semlith"));

    // `forget` writes, so it goes through the same single-store resolution
    // `index` does — and unlike `index` it embeds nothing.
    let out = s.run(&s.work, &["forget", "nothing.md"]);
    assert!(out.status.success(), "{}", text(&out));

    let said = text(&out);
    assert!(
        said.contains(".semlith") && said.contains("semlith adopt"),
        "a local store must be reported with the adopt hint: {said}"
    );
    assert!(
        !s.stores_root().exists(),
        "a local store must not cause a store in the home to be created"
    );
}

/// The third step: a registered root that is the working directory or an
/// ancestor of it. Without this, every subdirectory of a repository would get
/// a store of its own and a search from `src/` would miss `tests/`.
#[test]
fn a_subdirectory_resolves_to_the_store_registered_above_it() {
    let s = sandbox("ancestor");
    let repo = s.work.join("api");
    let deep = repo.join("src").join("routes");
    std::fs::create_dir_all(&deep).unwrap();

    // Register `api` the way an index run would, by adopting a store into it.
    empty_store(&repo.join(".semlith"));
    let out = s.run(&repo, &["adopt", ".semlith"]);
    assert!(out.status.success(), "{}", text(&out));

    // From two directories down, with no flags, the same store answers.
    let out = s.run(&deep, &["stats"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains(&s.stores_root().join("api").display().to_string()),
        "a subdirectory must resolve to the registered store above it: {}",
        text(&out)
    );
}

/// The fourth step, and the release's headline change: with nothing registered
/// and no `.semlith`, a bare `semlith index` creates the store in the home and
/// records the directory it indexed as that store's root.
#[test]
#[ignore = "embeds, so it downloads an embedding model on first run"]
fn a_bare_index_creates_a_store_in_the_home_and_registers_its_root() {
    let s = sandbox("bare-index");
    let repo = s.work.join("api");
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src").join("lib.rs"), "fn ownership() {}\n").unwrap();

    let out = s.run(&repo, &["index", "."]);
    assert!(out.status.success(), "{}", text(&out));

    let store = s.stores_root().join("api");
    assert!(
        store.join("store.db").exists(),
        "expected a store at {}: {}",
        store.display(),
        text(&out)
    );

    let registry = s.registry();
    let roots = registry["stores"]["api"]["roots"]
        .as_array()
        .expect("api has roots")
        .iter()
        .map(|r| PathBuf::from(r.as_str().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(
        roots,
        vec![std::fs::canonicalize(&repo).unwrap()],
        "the indexed directory is the store's root"
    );

    // Run again from a subdirectory: the ancestor rule must reuse the same
    // store rather than making a second one called `src`.
    let out = s.run(&repo.join("src"), &["index", "."]);
    assert!(out.status.success(), "{}", text(&out));
    let names: Vec<String> = s.registry()["stores"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(names, vec!["api".to_string()], "one corpus, one store");
}

// ---------------------------------------------------------------- T02

/// What `adopt` promises: the store moves, and nothing about it changes. Same
/// chunk counts, same files, same model, no embedding — and the directory it
/// came from is gone, so there is no second copy to drift.
#[test]
fn adopt_moves_a_store_without_changing_what_is_in_it() {
    let s = sandbox("adopt");
    let repo = s.work.join("api");
    std::fs::create_dir_all(&repo).unwrap();
    let local = repo.join(".semlith");
    empty_store(&local);

    let before = s.run(&repo, &["stats"]);
    assert!(before.status.success(), "{}", text(&before));
    let before_body = strip_store_line(&text(&before));

    let out = s.run(&repo, &["adopt", ".semlith"]);
    assert!(out.status.success(), "{}", text(&out));

    assert!(
        !local.exists(),
        "the source directory must not be left behind"
    );
    let moved = s.stores_root().join("api");
    assert!(moved.join("store.db").exists(), "{}", text(&out));

    let after = s.run(&repo, &["stats"]);
    assert!(after.status.success(), "{}", text(&after));
    assert_eq!(
        strip_store_line(&text(&after)),
        before_body,
        "adopt must not change anything about the store but where it lives"
    );

    // Registered against the directory it was indexing, which for a `.semlith`
    // is the directory that held it.
    let roots = s.registry()["stores"]["api"]["roots"].clone();
    assert_eq!(
        roots[0].as_str().unwrap(),
        std::fs::canonicalize(&repo).unwrap().to_str().unwrap()
    );
}

/// A store already in the home must not be adopted twice: a second copy of one
/// corpus is two stores whose vectors immediately start to disagree.
#[test]
fn adopting_a_registered_store_is_refused() {
    let s = sandbox("twice");
    let repo = s.work.join("api");
    std::fs::create_dir_all(&repo).unwrap();
    empty_store(&repo.join(".semlith"));
    assert!(s.run(&repo, &["adopt", ".semlith"]).status.success());

    let again = s.run(
        &repo,
        &["adopt", &s.stores_root().join("api").display().to_string()],
    );
    assert!(!again.status.success(), "{}", text(&again));
    assert!(
        text(&again).contains("already the registered store"),
        "{}",
        text(&again)
    );
}

/// `--root` on a store that is already registered re-points it, for a corpus
/// that moved. Nothing is copied and the store keeps its name.
#[test]
fn root_repoints_a_registered_store() {
    let s = sandbox("repoint");
    let old = s.work.join("api");
    let new = s.work.join("api-moved");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::create_dir_all(&new).unwrap();
    empty_store(&old.join(".semlith"));
    assert!(s.run(&old, &["adopt", ".semlith"]).status.success());

    let moved = s.stores_root().join("api");
    let out = s.run(
        &s.work,
        &[
            "adopt",
            &moved.display().to_string(),
            "--root",
            &new.display().to_string(),
        ],
    );
    assert!(out.status.success(), "{}", text(&out));

    let roots = s.registry()["stores"]["api"]["roots"].clone();
    assert_eq!(roots.as_array().unwrap().len(), 1);
    assert_eq!(
        roots[0].as_str().unwrap(),
        std::fs::canonicalize(&new).unwrap().to_str().unwrap()
    );

    // And the new location now resolves to it.
    let out = s.run(&new, &["stats"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains(&moved.display().to_string()),
        "{}",
        text(&out)
    );
}

/// Two directories called `api` are two corpora. The second must get its own
/// store rather than being merged into the first, which would make every
/// search answer about the wrong repository.
#[test]
fn a_second_corpus_with_the_same_name_gets_its_own_store() {
    let s = sandbox("collide");
    for parent in ["a", "b"] {
        let repo = s.work.join(parent).join("api");
        std::fs::create_dir_all(&repo).unwrap();
        empty_store(&repo.join(".semlith"));
        let out = s.run(&repo, &["adopt", ".semlith"]);
        assert!(out.status.success(), "{}", text(&out));
    }

    let names: Vec<String> = s.registry()["stores"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(names, vec!["api".to_string(), "api-2".to_string()]);
}

/// `--store` is still the first step of the resolution order, and it still
/// means exactly what it meant in 0.8.0: nothing is registered, nothing moves.
#[test]
fn an_explicit_store_flag_registers_nothing() {
    let s = sandbox("flag");
    let elsewhere = s.work.join("elsewhere");
    empty_store(&elsewhere);

    let out = s.run(
        &s.work,
        &["--store", &elsewhere.display().to_string(), "stats"],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        !s.home.join("registry.json").exists(),
        "an explicit --store must not create a registry"
    );
}

/// The store's own path is the one line of `stats` that legitimately changes
/// when a store moves; everything else must be identical.
fn strip_store_line(body: &str) -> String {
    body.lines()
        .filter(|l| !l.trim_start().starts_with("store "))
        .collect::<Vec<_>>()
        .join("\n")
}
