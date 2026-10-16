//! The 2026-10-14 audit's search, read and brief defects (0.42.0 items 24, 26
//! and 28: G12, G13, G14, G15, G16, F13), each against the audit's own input.
//!
//! `tests/fixtures/fidelity/` holds unedited copies of the files the audit
//! asked about, from its corpus: ripgrep's `GUIDE.md` and
//! `crates/core/main.rs` (MIT or Unlicense), and flask's tutorial screenshots,
//! `views.py`, `sansio/blueprints.py` and the flaskr blog templates
//! (BSD-3-Clause). They build real stores, so they need the embedding model
//! (and CLIP, for the screenshots):
//!
//! ```sh
//! cargo test --test fidelity -- --ignored
//! ```

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fidelity")
        .join(name)
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A scratch home holding a copy of one fixture corpus, so no store, registry
/// or ledger row lands anywhere but a temporary directory.
struct Scratch {
    home: tempfile::TempDir,
    corpus: PathBuf,
}

impl Scratch {
    fn of(name: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let corpus = home.path().join(name);
        copy_tree(&fixture(name), &corpus);
        Self { home, corpus }
    }

    fn store(&self) -> PathBuf {
        self.home.path().join("store")
    }

    /// `semlith` with the store home under the scratch directory. The model
    /// cache is the developer's, read where it already is.
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_semlith"))
            .args(args)
            .env("SEMLITH_HOME", self.home.path().join(".semlith"))
            .env("SEMLITH_LEDGER", "0")
            .env_remove("SEMLITH_STORE")
            .current_dir(&self.corpus)
            .output()
            .expect("semlith runs")
    }

    /// `semlith --store <scratch store> …`.
    fn on_store(&self, args: &[&str]) -> Output {
        let store = self.store();
        let mut all = vec!["--store", store.to_str().unwrap()];
        all.extend_from_slice(args);
        self.run(&all)
    }

    fn indexed(name: &str) -> Self {
        Self::indexed_without(name, None)
    }

    /// [`Scratch::indexed`] with one directory of the copy taken out first.
    fn indexed_without(name: &str, left_out: Option<&str>) -> Self {
        let scratch = Self::of(name);
        if let Some(dir) = left_out {
            fs::remove_dir_all(scratch.corpus.join(dir)).unwrap();
        }
        let corpus = scratch.corpus.to_str().unwrap().to_string();
        let o = scratch.on_store(&["index", &corpus]);
        assert!(o.status.success(), "{}", both(&o));
        scratch
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn both(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// The lines a `read` printed under its locator line.
fn body_of(read: &Output) -> Vec<String> {
    assert!(read.status.success(), "{}", both(read));
    let text = stdout(read);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    assert!(!lines.is_empty(), "read printed nothing at all");
    lines.remove(0);
    lines
}

// ------------------------------------------------------------------- G14

/// Every line of the range, a blank one included: `read main.rs:8-10`
/// printed two lines of three and `read main.rs:6`, a blank line, printed
/// none. And a blank line that ends a chunk — the one before a Markdown
/// heading, where the next chunk starts on the cut — is still in the store's
/// answer.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn read_prints_every_line_of_its_range_blank_ones_included() {
    let scratch = Scratch::of("ripgrep");
    fs::write(
        scratch.corpus.join("notes.md"),
        "# First\n\nThe first section.\n\n# Second\n\nThe second section.\n",
    )
    .unwrap();
    let corpus = scratch.corpus.to_str().unwrap().to_string();
    let o = scratch.on_store(&["index", &corpus]);
    assert!(o.status.success(), "{}", both(&o));

    let file = fs::read_to_string(scratch.corpus.join("crates/core/main.rs")).unwrap();
    let want: Vec<&str> = file.lines().skip(7).take(3).collect();
    assert_eq!(want[0], "", "the fixture's line 8 is blank");
    let read = scratch.on_store(&["read", "crates/core/main.rs:8-10"]);
    assert_eq!(body_of(&read), want, "{}", both(&read));

    let read = scratch.on_store(&["read", "crates/core/main.rs:6"]);
    assert!(stdout(&read).lines().next().unwrap().contains(":6-6"));
    assert_eq!(body_of(&read), [""], "{}", both(&read));

    let read = scratch.on_store(&["read", "notes.md:4"]);
    assert!(
        stdout(&read)
            .lines()
            .next()
            .is_some_and(|l| l.contains(":4-4")),
        "{}",
        both(&read)
    );
    assert_eq!(body_of(&read), [""], "{}", both(&read));
}

// ------------------------------------------------------------------- G12

fn hits(o: &Output) -> Vec<(String, u64, u64, f64)> {
    assert!(o.status.success(), "{}", both(o));
    let rows: Value = serde_json::from_str(&stdout(o)).expect("search --json");
    rows.as_array()
        .unwrap()
        .iter()
        .map(|h| {
            (
                h["path"].as_str().unwrap().to_string(),
                h["start_line"].as_u64().unwrap(),
                h["end_line"].as_u64().unwrap(),
                h["score"].as_f64().unwrap(),
            )
        })
        .collect()
}

/// Asking for fewer results only cuts the answer. "how does ripgrep detect
/// binary files" scored GUIDE.md 723-736 at 0.077 with k=5 and 0.089 with
/// k=8, and 754-765 was third at k=5 and gone at k=3.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn asking_for_fewer_results_never_reorders_or_rescores_the_first() {
    let scratch = Scratch::indexed("ripgrep");
    // The audit's question, and one this cut of the corpus moves at k=3.
    for query in [
        "how does ripgrep detect binary files",
        "how do I search compressed files",
    ] {
        let eight = hits(&scratch.on_store(&["search", "--json", "-k", "8", query]));
        assert_eq!(eight.len(), 8);
        for k in [1usize, 2, 3, 5] {
            let fewer = hits(&scratch.on_store(&["search", "--json", "-k", &k.to_string(), query]));
            assert_eq!(
                fewer,
                eight[..k],
                "{query:?}: k={k} is not the first {k} of k=8"
            );
        }
    }
}

// ------------------------------------------------------------------- G13

/// A question about code is answered with code. flask's "register a
/// blueprint url prefix" put two tutorial screenshots first at 0.231 and
/// 0.214, above every chunk of `blueprints.py` (0.088 at best), and brief led
/// with `flaskr_login.png:0-0`. A query that describes a screenshot still
/// finds it, and no surface prints an image's span as `0-0`.
#[test]
#[ignore = "downloads the embedding model and CLIP on first run"]
fn a_text_query_is_answered_with_text_and_no_picture_reports_0_0() {
    let scratch = Scratch::indexed("flask");
    let query = "register a blueprint url prefix";
    let o = scratch.on_store(&["search", "--json", "-k", "8", query]);
    assert!(o.status.success(), "{}", both(&o));
    let rows: Value = serde_json::from_str(&stdout(&o)).unwrap();
    let rows = rows.as_array().unwrap();
    assert!(
        rows.iter().all(|r| r["image"].is_null()),
        "a picture answered a question about code: {rows:#?}"
    );
    assert!(
        rows[0]["path"]
            .as_str()
            .unwrap()
            .ends_with("sansio/blueprints.py"),
        "{rows:#?}"
    );

    let brief = scratch.on_store(&["brief", query]);
    assert!(brief.status.success(), "{}", both(&brief));
    assert!(!stdout(&brief).contains(":0-0"), "{}", stdout(&brief));

    // The picture a query describes is still found, and named without a
    // line range.
    let asked = "a screenshot of the flaskr login page";
    let o = scratch.on_store(&["search", "--json", "-k", "3", asked]);
    let rows: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|r| r["path"].as_str().unwrap().ends_with("flaskr_login.png")),
        "{rows:#?}"
    );
    let brief = scratch.on_store(&["brief", asked]);
    assert!(
        stdout(&brief).contains("flaskr_login.png"),
        "{}",
        both(&brief)
    );
    assert!(!stdout(&brief).contains(":0-0"), "{}", stdout(&brief));
}

// ------------------------------------------------------------------- F13

const CREATE: &str = "examples/tutorial/flaskr/templates/blog/create.html";

/// The lines of the fixture's templates holding `request.form['title']`,
/// which is what `grep -E "request.form\[.title.\]"` prints.
fn title_lines(corpus: &Path) -> usize {
    ["create.html", "update.html"]
        .iter()
        .map(|f| {
            fs::read_to_string(
                corpus
                    .join("examples/tutorial/flaskr/templates/blog")
                    .join(f),
            )
            .unwrap()
            .lines()
            .filter(|l| {
                l.contains("request.form['title']") || l.contains("request.form[\"title\"]")
            })
            .count()
        })
        .sum()
}

/// Markup in a code store is stored as written. flask's Jinja template
/// `create.html` was stored tag-stripped, so `read create.html:8-12` printed
/// text that is not in the file and an exact search found 7 of grep's 9.
#[test]
#[ignore = "downloads the embedding model and CLIP on first run"]
fn markup_in_a_code_store_reads_and_greps_as_written() {
    let scratch = Scratch::indexed("flask");
    let file = fs::read_to_string(scratch.corpus.join(CREATE)).unwrap();
    let want: Vec<&str> = file.lines().skip(7).take(5).collect();
    let read = scratch.on_store(&["read", &format!("{CREATE}:8-12")]);
    assert_eq!(body_of(&read), want, "{}", both(&read));

    let exact = scratch.on_store(&["search", "--exact", r"request.form\[.title.\]"]);
    assert!(exact.status.success(), "{}", both(&exact));
    let found = stdout(&exact)
        .lines()
        .filter(|l| l.contains("request.form"))
        .count();
    assert_eq!(found, title_lines(&scratch.corpus), "{}", both(&exact));
}

/// A store whose markup was stored the old way — tag-stripped, and no row
/// saying how — re-reads its markup files on its next pass, and only them.
/// The old way is made by the rule this release keeps for a docs store, with
/// the row a pre-0.42.0 store never had taken away.
#[test]
#[ignore = "downloads the embedding model and CLIP on first run"]
fn a_store_written_before_raw_markup_rereads_its_markup_and_nothing_else() {
    let scratch = Scratch::of("flask");
    let corpus = scratch.corpus.to_str().unwrap().to_string();
    let o = scratch.run(&["index", "--name", "flaskr", &corpus]);
    assert!(o.status.success(), "{}", both(&o));
    let registry = scratch.home.path().join(".semlith/registry.json");
    let named = scratch.home.path().join(".semlith/stores/flaskr");
    let set_kind = |kind: &str| {
        let mut r: Value = serde_json::from_str(&fs::read_to_string(&registry).unwrap()).unwrap();
        r["stores"]["flaskr"]["kind"] = Value::from(kind);
        fs::write(&registry, serde_json::to_string_pretty(&r).unwrap()).unwrap();
    };

    // A docs store keeps the text a browser shows.
    set_kind("docs");
    let o = scratch.run(&["index", "--name", "flaskr", &corpus]);
    assert!(o.status.success(), "{}", both(&o));
    assert!(both(&o).contains("indexed 2 files"), "{}", both(&o));
    let read = scratch.run(&[
        "--store",
        named.to_str().unwrap(),
        "read",
        &format!("{CREATE}:8-12"),
    ]);
    assert!(
        !body_of(&read).iter().any(|l| l.contains("<form")),
        "{}",
        both(&read)
    );

    // What 0.41.x left behind: stripped chunks and no row.
    let db = named.join("store.db");
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute("DELETE FROM meta WHERE k = 'markup'", [])
        .unwrap();
    set_kind("both");
    let o = scratch.run(&["index", "--name", "flaskr", &corpus]);
    assert!(o.status.success(), "{}", both(&o));
    assert!(
        both(&o).contains("indexed 2 files"),
        "only the two templates are read again: {}",
        both(&o)
    );
    let file = fs::read_to_string(scratch.corpus.join(CREATE)).unwrap();
    let want: Vec<&str> = file.lines().skip(7).take(5).collect();
    let read = scratch.run(&[
        "--store",
        named.to_str().unwrap(),
        "read",
        &format!("{CREATE}:8-12"),
    ]);
    assert_eq!(body_of(&read), want, "{}", both(&read));

    // And once read, nothing is read again.
    let o = scratch.run(&["index", "--name", "flaskr", &corpus]);
    assert!(both(&o).contains("indexed 0 files"), "{}", both(&o));
}

// ---------------------------------------------------------------- G15, G16

/// The order the help states, and every drop named. `-b 600` dropped the
/// edges while it kept locators, and `-b 150` dropped five locators and
/// printed `-- dropped: ` with nothing after it.
///
/// Without the screenshots, so the brief is about code whatever G13 does.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn brief_keeps_locators_and_edges_before_text_and_names_what_it_drops() {
    let scratch = Scratch::indexed_without("flask", Some("docs"));
    let question = "how does flask dispatch a request to a view";
    let full = stdout(&scratch.on_store(&["brief", question]));
    assert!(full.contains("dispatch_request (graph)"), "{full}");
    for budget in ["150", "300", "600"] {
        let o = scratch.on_store(&["brief", "-b", budget, question]);
        assert!(o.status.success(), "{}", both(&o));
        let text = stdout(&o);
        let tail = text.lines().last().unwrap_or_default().to_string();
        if let Some((_, dropped)) = tail.split_once("-- dropped:") {
            assert!(!dropped.trim().is_empty(), "-b {budget}: {text}");
        }
        let located = text
            .lines()
            .filter(|l| {
                l.split_once(". ")
                    .is_some_and(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            })
            .count();
        if located < 8 {
            assert!(
                tail.contains("locator"),
                "-b {budget} hid a locator: {text}"
            );
        }
        // Locators past the first are bought after the edges, so a brief that
        // shows them shows the edges too.
        if located > 1 {
            assert!(
                text.contains("dispatch_request (graph)"),
                "-b {budget} kept locators and dropped the edges: {text}"
            );
        }
    }
}

/// The edges are the definition's own, each row once, and the note under a
/// span without text names the span that has it. `View.dispatch_request`'s
/// callers listed `MethodView`, which holds a `dispatch_request` of its own,
/// and "(text: top span only)" sat under spans whose text was the third's.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn brief_edges_belong_to_the_spans_own_definition() {
    let scratch = Scratch::indexed_without("flask", Some("docs"));
    let o = scratch.on_store(&["brief", "how does flask dispatch a request to a view"]);
    assert!(o.status.success(), "{}", both(&o));
    let text = stdout(&o);
    let block: Vec<&str> = text
        .lines()
        .skip_while(|l| !l.starts_with("dispatch_request (graph)"))
        .skip(1)
        .take_while(|l| l.starts_with("   "))
        .collect();
    assert!(!block.is_empty(), "{text}");
    assert!(
        !block.iter().any(|l| l.contains("called by MethodView")),
        "another class's own dispatch_request counted as a caller: {text}"
    );
    let mut rows = block.clone();
    rows.sort();
    rows.dedup();
    assert_eq!(rows.len(), block.len(), "a row printed twice: {text}");
    assert!(!text.contains("(text: top span only)"), "{text}");
}
