//! What multi-store search promises: one query over several stores, one ranked
//! list back, and every excerpt saying which store it came from.
//!
//! These build real stores and embed real text, so they are slow and they
//! download an embedding model on first run:
//!
//! ```sh
//! cargo test --test fleet -- --ignored
//! ```

use semlith::filter::Filter;
use semlith::fleet::Fleet;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Two corpora that share no vocabulary, so "which store answered" is
/// decidable from the text of the hit alone.
const BREAD: &str = "Sourdough rises because a starter of flour and water ferments. \
                     Hydration is the ratio of water to flour by weight, and a wetter \
                     dough gives a more open crumb after baking.";

const RUST: &str = "The borrow checker proves at compile time that no value has two \
                    mutable aliases. Ownership means each value has a single owner, and \
                    the compiler frees it when that owner leaves scope.";

/// A phrase that exists in exactly one of the two corpora, with no term in
/// common with the other.
const ONLY_IN_RUST: &str = "aliasing is rejected before the program ever runs";

// ---------------------------------------------------------------- T01

/// The release, in one test: both stores answer, and the answer says which
/// store each excerpt is from.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn one_query_answers_from_both_stores_and_labels_each_hit() {
    let bread = corpus("bread", &[("sourdough.md", BREAD)]);
    let rust = corpus("rust", &[("ownership.md", RUST)]);
    let a = store_for(&bread);
    let b = store_for(&rust);

    let mut fleet = open(&[a.path().to_path_buf(), b.path().to_path_buf()]);
    let hits = fleet
        .search_filtered("how does this work", 6, &Filter::default())
        .unwrap();

    let labels: Vec<String> = hits.iter().filter_map(|h| h.store.clone()).collect();
    assert_eq!(
        labels.len(),
        hits.len(),
        "every hit must name its store when several were searched: {hits:#?}"
    );
    assert!(
        labels.iter().any(|l| l == "bread") && labels.iter().any(|l| l == "rust"),
        "one query must reach both stores, got {labels:?}"
    );
}

/// Adding a store must not bury the store that has the answer. If it does,
/// multi-store search is worse than asking each store separately, which is the
/// thing it replaces.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_phrase_only_in_one_store_keeps_its_rank_when_another_is_added() {
    let bread = corpus("bread", &[("sourdough.md", BREAD)]);
    let rust = corpus("rust", &[("ownership.md", RUST)]);
    let a = store_for(&bread);
    let b = store_for(&rust);

    let mut alone = open(&[b.path().to_path_buf()]);
    let solo = alone
        .search_filtered(ONLY_IN_RUST, 5, &Filter::default())
        .unwrap();
    let rank = solo
        .iter()
        .position(|h| h.path.ends_with("ownership.md"))
        .expect("the store that holds the phrase did not return it on its own");

    let mut both = open(&[a.path().to_path_buf(), b.path().to_path_buf()]);
    let fused = both
        .search_filtered(ONLY_IN_RUST, 5, &Filter::default())
        .unwrap();
    let fused_rank = fused
        .iter()
        .position(|h| h.path.ends_with("ownership.md"))
        .expect("the answer disappeared once a second store was named");

    assert_eq!(
        fused_rank, rank,
        "adding a store moved the answer from rank {rank} to rank {fused_rank}: {fused:#?}"
    );
}

/// `k` is how many results the caller wants, not how many each store gets.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn k_is_global_across_stores() {
    let bread = corpus("bread", &[("sourdough.md", BREAD)]);
    let rust = corpus("rust", &[("ownership.md", RUST)]);
    let notes = corpus(
        "notes",
        &[(
            "notes.md",
            "Assorted notes about nothing in particular. \
        A second sentence so the file chunks into more than one piece of text.",
        )],
    );
    let a = store_for(&bread);
    let b = store_for(&rust);
    let c = store_for(&notes);

    let mut fleet = open(&[
        a.path().to_path_buf(),
        b.path().to_path_buf(),
        c.path().to_path_buf(),
    ]);
    let hits = fleet
        .search_filtered("anything at all", 5, &Filter::default())
        .unwrap();
    assert!(
        hits.len() <= 5,
        "three stores returned {} hits for k=5: {hits:#?}",
        hits.len()
    );
}

// ---------------------------------------------------------------- T02

/// `open` creates the directory it is given, which is right for `index` and
/// wrong for every read command: a mistyped store becomes an empty store that
/// answers every question with nothing, and in a multi-store query the other
/// stores hide it.
#[test]
fn a_store_path_that_does_not_exist_is_an_error_and_creates_nothing() {
    let parent = tempfile::tempdir().unwrap();
    let missing = parent.path().join("typo").join(".semlith");

    let err = match Fleet::open(std::slice::from_ref(&missing)) {
        Ok(_) => panic!("a store that does not exist opened anyway"),
        Err(e) => e.to_string(),
    };
    assert!(
        err.contains("typo"),
        "the error does not name the path that was wrong: {err}"
    );
    assert!(
        !missing.exists(),
        "a read command created {}",
        missing.display()
    );
    assert!(
        !parent.path().join("typo").exists(),
        "a read command created the parent directory too"
    );
}

/// Fusing a store with itself doubles every rank contribution it has, which
/// hands it the whole result list.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_same_store_named_twice_is_searched_once() {
    let bread = corpus("bread", &[("sourdough.md", BREAD)]);
    let a = store_for(&bread);

    // Run from the directory holding the store, so `.semlith` is the same store
    // the absolute path names.
    let cwd = inner_of(&bread);
    let once = search_cli(&cwd, &[a.path().to_str().unwrap()], "hydration and crumb");
    // The same store, absolute and relative, in one invocation.
    let twice = search_cli(
        &cwd,
        &[a.path().to_str().unwrap(), ".semlith"],
        "hydration and crumb",
    );
    assert_eq!(once, twice, "naming a store twice changed the result list");
}

// ---------------------------------------------------------------- T03

/// A filter that selects nothing in one store must not report an empty
/// selection for the query as a whole, or the filter and the flag cannot be
/// used together.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_filter_that_matches_only_one_store_still_returns_that_store() {
    let docs = corpus("docs", &[("sourdough.md", BREAD)]);
    let code = corpus("code", &[("lib.rs", RUST)]);
    let a = store_for(&docs);
    let b = store_for(&code);

    let mut fleet = open(&[a.path().to_path_buf(), b.path().to_path_buf()]);
    let filter = Filter::new(&[], &["rs".into()], &[]).unwrap();

    assert!(
        fleet.matching_files(&filter).unwrap() > 0,
        "the filter matches a file in one of the two stores"
    );
    let hits = fleet
        .search_filtered("aliases and ownership", 5, &filter)
        .unwrap();
    assert!(
        !hits.is_empty(),
        "a filter matching one store returned nothing"
    );
    assert!(
        hits.iter().all(|h| h.path.ends_with(".rs")),
        "the filter leaked past one store: {hits:#?}"
    );
}

/// The default model changed in 0.3.0, so a developer's stores disagreeing
/// about it is the ordinary case, not an exotic one.
#[test]
#[ignore = "downloads two embedding models on first run"]
fn stores_built_with_different_models_are_searched_together() {
    let bread = corpus("bread", &[("sourdough.md", BREAD)]);
    let rust = corpus("rust", &[("ownership.md", RUST)]);
    let a = store_with_model(&bread, None);
    let b = store_with_model(&rust, Some("BGESmallENV15"));

    let mut fleet = open(&[a.path().to_path_buf(), b.path().to_path_buf()]);
    let hits = fleet
        .search_filtered("how does this work", 6, &Filter::default())
        .unwrap();

    let labels: Vec<String> = hits.iter().filter_map(|h| h.store.clone()).collect();
    assert!(
        labels.iter().any(|l| l == "bread") && labels.iter().any(|l| l == "rust"),
        "a store whose model differs contributed nothing: {labels:?}"
    );
}

/// Rank fusion gives every store's best hit the same weight, so a store with
/// nothing to say still offers a rank-1 result. This is the measurement that
/// decides whether that matters.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn an_unrelated_store_does_not_degrade_the_store_that_has_the_answer() {
    let rust = corpus("rust", &[("ownership.md", RUST)]);
    let bread = corpus("bread", &[("sourdough.md", BREAD)]);
    let b = store_for(&rust);
    let a = store_for(&bread);

    let mut alone = open(&[b.path().to_path_buf()]);
    let solo = alone
        .search_filtered(ONLY_IN_RUST, 3, &Filter::default())
        .unwrap();
    assert!(!solo.is_empty(), "the store answered nothing on its own");

    let mut both = open(&[b.path().to_path_buf(), a.path().to_path_buf()]);
    let fused = both
        .search_filtered(ONLY_IN_RUST, 6, &Filter::default())
        .unwrap();

    for hit in &solo {
        let at = fused
            .iter()
            .position(|h| h.path == hit.path && h.start_line == hit.start_line);
        let at =
            at.unwrap_or_else(|| panic!("adding a store dropped {}:{}", hit.path, hit.start_line));
        let unrelated_above = fused[..at]
            .iter()
            .filter(|h| h.store.as_deref() == Some("bread"))
            .count();
        assert_eq!(
            unrelated_above, 0,
            "{} unrelated hits outranked the answer: {fused:#?}",
            unrelated_above
        );
    }
}

// ---------------------------------------------------------------- T04

/// The roadmap outcome is stated for an agent working across repositories, and
/// an agent reaches semlith over MCP, not over the CLI.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn one_mcp_server_searches_several_stores_and_can_be_narrowed_to_one() {
    let bread = corpus("bread", &[("sourdough.md", BREAD)]);
    let rust = corpus("rust", &[("ownership.md", RUST)]);
    let a = store_for(&bread);
    let b = store_for(&rust);

    let mut server = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(a.path())
        .arg("--store")
        .arg(b.path())
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = server.stdin.take().unwrap();
    let mut stdout = std::io::BufReader::new(server.stdout.take().unwrap());

    rpc(
        &mut stdin,
        &mut stdout,
        1,
        "initialize",
        r#"{"protocolVersion":"2024-11-05"}"#,
    );

    let both = rpc(
        &mut stdin,
        &mut stdout,
        2,
        "tools/call",
        r#"{"name":"semlith_search","arguments":{"query":"how does this work","k":6}}"#,
    );
    assert!(
        both.contains("bread") && both.contains("rust"),
        "one call did not reach both stores: {both}"
    );

    let narrowed = rpc(
        &mut stdin,
        &mut stdout,
        3,
        "tools/call",
        r#"{"name":"semlith_search","arguments":{"query":"how does this work","k":6,"store":["rust"]}}"#,
    );
    assert!(
        narrowed.contains("ownership.md"),
        "narrowing to a store lost its own hits: {narrowed}"
    );
    assert!(
        !narrowed.contains("sourdough.md"),
        "the store argument did not narrow anything: {narrowed}"
    );

    let unknown = rpc(
        &mut stdin,
        &mut stdout,
        4,
        "tools/call",
        r#"{"name":"semlith_search","arguments":{"query":"anything","store":["nope"]}}"#,
    );
    assert!(
        unknown.contains("isError"),
        "an unknown store was not an error: {unknown}"
    );
    assert!(
        unknown.contains("bread") && unknown.contains("rust"),
        "the error does not say which stores are open: {unknown}"
    );

    let stats = rpc(
        &mut stdin,
        &mut stdout,
        5,
        "tools/call",
        r#"{"name":"semlith_stats","arguments":{}}"#,
    );
    assert!(
        stats.contains("bread") && stats.contains("rust"),
        "stats over several stores must report each: {stats}"
    );

    drop(stdin);
    let _ = server.wait();
}

// ---------------------------------------------------------------- T05

/// "Is this corpus even indexed" has to stay answerable per store, or an empty
/// answer from several stores is undiagnosable.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn stats_and_files_report_every_store() {
    let bread = corpus("bread", &[("sourdough.md", BREAD)]);
    let rust = corpus("rust", &[("ownership.md", RUST)]);
    let a = store_for(&bread);
    let b = store_for(&rust);

    let stats = cli(
        bread.path(),
        &[
            "--store",
            a.path().to_str().unwrap(),
            "--store",
            b.path().to_str().unwrap(),
            "stats",
        ],
    );
    assert!(
        stats.contains("bread") && stats.contains("rust"),
        "stats did not label both stores: {stats}"
    );

    let files = cli(
        bread.path(),
        &[
            "--store",
            a.path().to_str().unwrap(),
            "--store",
            b.path().to_str().unwrap(),
            "files",
        ],
    );
    assert!(
        files.contains("sourdough.md") && files.contains("ownership.md"),
        "files did not list both stores: {files}"
    );

    // One store prints exactly what 0.4.0 printed: no labels anywhere.
    let single = cli(
        bread.path(),
        &["--store", a.path().to_str().unwrap(), "stats"],
    );
    assert!(
        single.starts_with("store "),
        "single-store stats changed shape: {single}"
    );
    assert!(
        !single.contains("bread\n"),
        "single-store stats gained a label: {single}"
    );
}

/// The two features a developer uses together: leave a watcher running, and ask
/// one question across repositories. A multi-store search that took a store
/// lock, or that missed one store's generation counter, would turn 0.4.0's
/// freshness guarantee off for exactly that person.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_watched_store_stays_searchable_and_fresh_inside_a_fleet() {
    let bread = corpus("bread", &[("sourdough.md", BREAD)]);
    let rust = corpus("rust", &[("ownership.md", RUST)]);
    let a = store_for(&bread);
    let b = store_for(&rust);

    // A watcher holds the write lock on the second store for as long as it runs.
    let watched = inner_of(&rust);
    let mut watcher = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(b.path())
        .arg("watch")
        .arg(&watched)
        .arg("--debounce")
        .arg("200")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    // A long-lived reader, opened before the edit exists — the position an
    // agent's MCP server is in.
    let mut fleet = open(&[a.path().to_path_buf(), b.path().to_path_buf()]);
    let before = fleet
        .search_filtered("lifetimes outlive their references", 5, &Filter::default())
        .unwrap();
    assert!(
        !before.iter().any(|h| h.path.ends_with("lifetimes.md")),
        "the corpus already held the edit"
    );

    fs::write(
        watched.join("lifetimes.md"),
        "A lifetime annotation states how long a reference stays valid, so the \
         compiler can reject one that outlives what it points at.",
    )
    .unwrap();

    // Same reader, no reopen: the watcher's write has to reach it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut found = false;
    while std::time::Instant::now() < deadline {
        let hits = fleet
            .search_filtered("lifetime annotation validity", 5, &Filter::default())
            .unwrap();
        if hits.iter().any(|h| h.path.ends_with("lifetimes.md")) {
            found = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }

    let _ = watcher.kill();
    let _ = watcher.wait();
    assert!(
        found,
        "a fleet reader never saw what the watcher wrote to one of its stores"
    );
}

// ---------------------------------------------------------------- helpers

fn open(dirs: &[PathBuf]) -> Fleet {
    let mut fleet = Fleet::open(dirs).unwrap();
    fleet.quiet = true;
    fleet
}

/// A corpus directory named `name`, whose name becomes the store's label.
fn corpus(name: &str, files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::Builder::new().prefix(name).tempdir().unwrap();
    // The label comes from the directory holding the store, and tempdir names
    // carry a random suffix, so the corpus lives in a child with a fixed name.
    let inner = dir.path().join(name);
    fs::create_dir_all(&inner).unwrap();
    for (file, body) in files {
        fs::write(inner.join(file), body).unwrap();
    }
    dir
}

/// Index `corpus`'s inner directory into a `.semlith` store beside it, exactly
/// as a developer would, and hand back the store directory.
fn store_for(corpus: &tempfile::TempDir) -> StoreDir {
    store_with_model(corpus, None)
}

fn store_with_model(corpus: &tempfile::TempDir, model: Option<&str>) -> StoreDir {
    let inner = inner_of(corpus);
    let store = inner.join(".semlith");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_semlith"));
    cmd.arg("--store")
        .arg(&store)
        .arg("index")
        .arg(&inner)
        .arg("--quiet");
    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "indexing failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    StoreDir(store)
}

/// The single child directory of a corpus tempdir — the one with the stable
/// name that becomes the store label.
fn inner_of(corpus: &tempfile::TempDir) -> PathBuf {
    fs::read_dir(corpus.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .expect("corpus has an inner directory")
}

struct StoreDir(PathBuf);

impl StoreDir {
    fn path(&self) -> &Path {
        &self.0
    }
}

/// Run the release binary with `cwd`, returning stdout and stderr together.
fn cli(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A search through the binary, as JSON so the comparison is of results rather
/// than of formatting.
fn search_cli(cwd: &Path, stores: &[&str], query: &str) -> String {
    let mut args: Vec<&str> = Vec::new();
    for s in stores {
        args.push("--store");
        args.push(s);
    }
    args.extend_from_slice(&["search", query, "--json", "-k", "5"]);
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .current_dir(cwd)
        .args(&args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "search failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// One JSON-RPC round trip, returning the raw response line.
fn rpc(
    stdin: &mut std::process::ChildStdin,
    stdout: &mut impl BufRead,
    id: u64,
    method: &str,
    params: &str,
) -> String {
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#
    )
    .unwrap();
    stdin.flush().unwrap();

    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(!line.is_empty(), "the server closed instead of answering");
    line
}
