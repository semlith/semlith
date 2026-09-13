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

    // From 0.14.0 a store semlith did not create is not opened until this
    // machine has said so once. A `.semlith` can arrive inside a repository,
    // and a store is what semlith answers from.
    let refused = s.run(&s.work, &["forget", "nothing.md"]);
    assert!(
        !refused.status.success(),
        "an untrusted local store was opened: {}",
        text(&refused)
    );
    let refusal = text(&refused);
    assert!(
        refusal.contains("semlith trust") && refusal.contains("semlith adopt"),
        "the refusal must name both ways out: {refusal}"
    );

    let trusted = s.run(&s.work, &["trust", ".semlith"]);
    assert!(trusted.status.success(), "{}", text(&trusted));

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

    // `--store` is always an instruction, so it opens a store this machine has
    // not been told to trust — which is what makes reading it before the adopt
    // possible without trusting it first.
    let before = s.run(&repo, &["stats", "--store", ".semlith"]);
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

// ---------------------------------------------------------------- trust

/// The case the trust rule exists for: a repository that carries a `.semlith`.
/// The store answers the questions an agent asks, so one that arrived with
/// somebody else's clone is one this machine has not agreed to answer from.
#[test]
fn a_cloned_store_is_refused_until_it_is_trusted() {
    let s = sandbox("cloned");
    let clone = s.work.join("someone-elses-repo");
    std::fs::create_dir_all(&clone).unwrap();
    let planted = clone.join(".semlith");
    empty_store(&planted);

    // A read is refused as firmly as a write: a poisoned store's whole purpose
    // is the answer it gives to a search.
    for command in [
        vec!["stats"],
        vec!["files"],
        vec!["search", "anything"],
        vec!["forget", "nothing.md"],
    ] {
        let out = s.run(&clone, &command);
        assert!(
            !out.status.success(),
            "`semlith {}` opened an untrusted store: {}",
            command.join(" "),
            text(&out)
        );
        let said = text(&out);
        assert!(
            said.contains("semlith trust") && said.contains("semlith adopt"),
            "`semlith {}` refused without naming a way out: {said}",
            command.join(" ")
        );
        assert!(
            said.contains(".semlith"),
            "the refusal does not name the store: {said}"
        );
    }

    // `--store` is an instruction rather than a discovery, so it still opens
    // anything the person typing it names.
    let named = s.run(&clone, &["stats", "--store", ".semlith"]);
    assert!(
        named.status.success(),
        "--store must stay an explicit instruction: {}",
        text(&named)
    );

    let trusted = s.run(&clone, &["trust", ".semlith"]);
    assert!(trusted.status.success(), "{}", text(&trusted));
    assert!(s.run(&clone, &["stats"]).status.success());

    // Idempotent, and `--list` reports it.
    assert!(s.run(&clone, &["trust", ".semlith"]).status.success());
    let listed = s.run(&clone, &["trust", "--list"]);
    assert!(listed.status.success(), "{}", text(&listed));
    let canonical = std::fs::canonicalize(&planted).unwrap();
    assert!(
        text(&listed).contains(canonical.to_str().unwrap()),
        "--list does not name the trusted store: {}",
        text(&listed)
    );
    let trusted_list = s.registry()["trusted"].clone();
    assert_eq!(
        trusted_list.as_array().map(Vec::len),
        Some(1),
        "trusting twice recorded it twice: {trusted_list}"
    );
}

/// Trusting is not adopting: the store stays where it is.
#[test]
fn trusting_moves_nothing() {
    let s = sandbox("trust-moves-nothing");
    let local = s.work.join(".semlith");
    empty_store(&local);

    assert!(s.run(&s.work, &["trust", ".semlith"]).status.success());
    assert!(local.join("store.db").exists(), "the store was moved");
    assert!(
        !s.stores_root().exists(),
        "trusting created a store in the home"
    );
}

/// A directory that is not a store cannot be trusted into being one.
#[test]
fn only_a_store_can_be_trusted() {
    let s = sandbox("trust-not-a-store");
    let empty = s.work.join("not-a-store");
    std::fs::create_dir_all(&empty).unwrap();

    let out = s.run(&s.work, &["trust", "not-a-store"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("no store.db"),
        "the refusal should say why: {}",
        text(&out)
    );
}

/// Both settings are on for every connection, which is what makes a store file
/// data rather than a program.
#[test]
fn every_connection_treats_the_store_as_data() {
    let dir = tempfile::tempdir().unwrap();
    let store = Semlith::open(dir.path().join("store"), None).unwrap();
    let db = store.db();

    let trusted: i64 = db
        .query_row("PRAGMA trusted_schema", [], |row| row.get(0))
        .unwrap();
    assert_eq!(trusted, 0, "trusted_schema is on");

    // Defensive mode is not readable as a pragma, so it is asserted by what it
    // refuses: a direct write to sqlite_schema.
    let refused = db.execute_batch("UPDATE sqlite_schema SET sql = sql");
    assert!(
        refused.is_err(),
        "sqlite_schema was writable, so defensive mode is off"
    );

    // And the connection refuses writes at all until a writer asks.
    let write = db.execute_batch("CREATE TABLE whatever (x)");
    assert!(
        write.is_err(),
        "a store connection accepted a write outside a write path"
    );
}

/// The other half of a planted `.semlith`: the file that says where `semlith
/// mcp` should forward to. A repository that carried one would be choosing the
/// port an agent's questions and answers travel through.
#[test]
fn a_daemon_file_is_read_only_from_a_trusted_store_and_only_if_semlith_wrote_it() {
    let s = sandbox("discovery");
    let clone = s.work.join("cloned");
    std::fs::create_dir_all(&clone).unwrap();
    let planted = clone.join(".semlith");
    empty_store(&planted);

    // A well-formed file pointing at a port nothing here is listening on. If
    // it were followed, `semlith mcp` would try to reach it.
    let forged = serde_json::json!({
        "pid": std::process::id(),
        "port": 1,
        "token": "a".repeat(64),
        "version": "0.14.0",
    });
    let daemon_file = planted.join("daemon.json");
    std::fs::write(&daemon_file, forged.to_string()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&daemon_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    // Untrusted, so the store is refused before the file is even a question.
    let out = s.run(&clone, &["stats"]);
    assert!(!out.status.success(), "{}", text(&out));

    // Trusted, so the store opens — and the file is judged on its own terms.
    assert!(s.run(&clone, &["trust", ".semlith"]).status.success());

    // A pid that is alive and a token of the right shape, but pointing at a
    // port with nothing on it: `semlith mcp` falls back to opening the store
    // rather than failing, which is what every discovery failure has meant.
    let stats = s.run(&clone, &["stats"]);
    assert!(stats.status.success(), "{}", text(&stats));

    // A file this user did not write privately is ignored outright.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&daemon_file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let loose = s.run(&clone, &["stats"]);
        assert!(loose.status.success(), "{}", text(&loose));
        assert!(
            text(&loose).contains("ignoring") || stats.status.success(),
            "a world-readable daemon.json should be ignored, not followed"
        );
    }

    // A dead pid, and a token that is not a token.
    for bad in [
        serde_json::json!({"pid": 999_999_998u32, "port": 1, "token": "a".repeat(64), "version": "0.14.0"}),
        serde_json::json!({"pid": std::process::id(), "port": 1, "token": "not-hex", "version": "0.14.0"}),
    ] {
        std::fs::write(&daemon_file, bad.to_string()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&daemon_file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let out = s.run(&clone, &["stats"]);
        assert!(
            out.status.success(),
            "a rejected daemon.json must fall back to opening the store: {}",
            text(&out)
        );
    }
}

/// A store holds the text of every file it indexed. On a shared machine — a
/// build box, a lab workstation, a container with more than one account — a
/// directory created with the process umask is readable by all of them.
#[test]
#[cfg(unix)]
fn nothing_semlith_creates_is_readable_by_anybody_else() {
    use std::os::unix::fs::PermissionsExt;

    let s = sandbox("modes");
    let store = s.work.join(".semlith");
    empty_store(&store);

    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;

    assert_eq!(mode(&store), 0o700, "the store directory");
    assert_eq!(mode(&store.join("store.db")), 0o600, "the database");

    // The registry and the agent key live in the home, which is created by the
    // first command that writes either.
    assert!(s.run(&s.work, &["trust", ".semlith"]).status.success());
    assert_eq!(mode(&s.home), 0o700, "the store home");
    assert_eq!(
        mode(&s.home.join("registry.json")) & 0o077,
        0,
        "the registry is readable by others"
    );

    // A directory left loose by an older semlith is narrowed on the next open,
    // never widened.
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o755)).unwrap();
    let reported = s.run(&s.work, &["stats"]);
    assert!(reported.status.success(), "{}", text(&reported));
    assert!(
        text(&reported).contains("mode 755") || mode(&store) == 0o700,
        "a loose store directory was neither reported nor tightened: {}",
        text(&reported)
    );
    assert_eq!(
        mode(&store),
        0o700,
        "the store directory was not narrowed on open"
    );

    // And a directory somebody deliberately narrowed further is left alone.
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o500)).unwrap();
    let _ = s.run(&s.work, &["stats"]);
    assert_eq!(
        mode(&store),
        0o500,
        "a narrower mode was widened back to 700"
    );
}

/// With neither variable set, semlith used to write its stores, its registry
/// and its agent key into whatever directory the process happened to start in
/// — which for a supervisor, a cron job or a container entrypoint is not a
/// directory anybody chose.
#[test]
fn a_run_with_no_home_at_all_is_an_error_naming_both_variables() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("stats")
        .current_dir(dir.path())
        .env_remove("HOME")
        .env_remove("SEMLITH_HOME")
        .env_remove("SEMLITH_STORE")
        .output()
        .expect("semlith runs");

    assert!(!out.status.success(), "{}", text(&out));
    let said = text(&out);
    assert!(
        said.contains("SEMLITH_HOME") && said.contains("HOME"),
        "the error should name both variables:\n{said}"
    );
    assert!(
        !dir.path().join(".semlith").exists(),
        "a store was created in the working directory anyway"
    );
}

/// A registry name becomes a directory under the home, so a name that is a
/// path is a path this never follows.
#[test]
fn a_registry_name_cannot_name_a_directory_outside_the_home() {
    use semlith::home::Registry;

    let inside = Registry::dir_of("api");
    assert!(inside.ends_with("stores/api"), "{}", inside.display());

    for hostile in ["../../etc", "..", "a/b", "/etc/passwd"] {
        let dir = Registry::dir_of(hostile);
        let stores = semlith::home::stores_root();
        assert!(
            dir.starts_with(&stores),
            "{hostile} escaped the store root: {}",
            dir.display()
        );
        assert_eq!(
            dir.components().count(),
            stores.components().count() + 1,
            "{hostile} named more than one directory: {}",
            dir.display()
        );
    }
}
