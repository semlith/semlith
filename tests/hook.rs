//! The steering hook, driven the way a client drives it: one `PreToolUse`
//! event on stdin, one answer on stdout, inside somebody else's tool call.
//!
//! Everything here runs against a redirected `HOME` and `SEMLITH_HOME`, because
//! the hook reads the registry and the registry lives in the store home — and
//! because a test that reads the developer's own registry would pass or fail
//! depending on what they happen to have indexed.
//!
//! ```sh
//! cargo test --test hook -- --ignored
//! ```

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// A machine with one store, one registered root, and nothing else.
struct Machine {
    _dir: tempfile::TempDir,
    home: PathBuf,
    store_home: PathBuf,
    corpus: PathBuf,
}

impl Machine {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temp directory");
        let home = dir.path().join("home");
        let corpus = dir.path().join("corpus");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(corpus.join("src")).unwrap();
        std::fs::write(
            corpus.join("src/store.rs"),
            "fn hold() {}\nfn take() { hold(); }\n",
        )
        .unwrap();
        std::fs::write(
            corpus.join("src/notes.md"),
            "Sourdough starter needs flour and water and a warm shelf.\n",
        )
        .unwrap();
        let machine = Self {
            store_home: home.join(".semlith"),
            home,
            corpus,
            _dir: dir,
        };
        // A real index pass, so the registry records a real root and the store
        // holds the file the hook is asked about.
        let out = machine.run("index", &[machine.corpus.to_str().unwrap()], "");
        assert!(
            out.status.success(),
            "indexing the fixture failed: {}",
            said(&out)
        );
        machine
    }

    fn run(&self, command: &str, args: &[&str], stdin: &str) -> Output {
        use std::io::Write;
        let mut child = Command::new(env!("CARGO_BIN_EXE_semlith"))
            .arg(command)
            .args(args)
            .env("HOME", &self.home)
            .env("SEMLITH_HOME", &self.store_home)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("running semlith");
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().expect("semlith finished")
    }

    fn hook(&self, event: &Value) -> Output {
        self.run("hook", &[], &event.to_string())
    }

    /// How many rows the ledger holds. `--json` prints one array per store and
    /// the fixture has exactly one.
    fn rows(&self) -> i64 {
        let out = self.run("ledger", &["--json"], "");
        let body = String::from_utf8_lossy(&out.stdout);
        serde_json::from_str::<Value>(body.trim())
            .ok()
            .and_then(|v| v.as_array().map(|a| a.len() as i64))
            .unwrap_or(0)
    }

    fn held(&self) -> String {
        self.corpus.join("src/store.rs").display().to_string()
    }
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn read_event(path: &str) -> Value {
    json!({
        "session_id": "hook-test",
        "hook_event_name": "PreToolUse",
        "tool_name": "Read",
        "tool_input": { "file_path": path },
    })
}

/// The whole of the default behaviour: one line naming a call, no decision of
/// any kind, and an exit status a client reads as "carry on".
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn a_read_of_an_indexed_file_is_answered_with_one_line_and_no_decision() {
    let machine = Machine::new();
    let out = machine.hook(&read_event(&machine.held()));

    assert!(out.status.success(), "the hook failed: {}", said(&out));
    let body = String::from_utf8_lossy(&out.stdout);
    let answer: Value = serde_json::from_str(body.trim()).expect("the hook answers JSON");
    let specific = &answer["hookSpecificOutput"];
    assert_eq!(specific["hookEventName"], "PreToolUse", "{answer}");
    let line = specific["additionalContext"]
        .as_str()
        .unwrap_or_else(|| panic!("no line for the agent to read: {answer}"));
    assert!(
        line.contains("semlith_brief"),
        "the line names no call: {line}"
    );
    assert!(
        specific.get("permissionDecision").is_none(),
        "the default hook must decide nothing at all: {answer}"
    );
}

/// A file outside every registered root is somebody else's business, and the
/// hook has to be able to say nothing without saying it loudly.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn a_read_of_a_file_no_store_holds_says_nothing() {
    let machine = Machine::new();
    let out = machine.hook(&read_event("/tmp/not-in-any-store.rs"));

    assert!(out.status.success(), "the hook failed: {}", said(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        "the hook spoke about a file no store holds: {}",
        said(&out)
    );
}

/// The one decision the hook is ever allowed to make, and only the first time
/// in a session.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn strict_refuses_the_first_read_of_a_session_and_then_gets_out_of_the_way() {
    let machine = Machine::new();
    let event = read_event(&machine.held());

    let first = machine.run("hook", &["--strict"], &event.to_string());
    let answer: Value =
        serde_json::from_str(String::from_utf8_lossy(&first.stdout).trim()).expect("JSON");
    assert_eq!(
        answer["hookSpecificOutput"]["permissionDecision"], "deny",
        "strict must refuse the first one: {answer}"
    );
    assert!(
        first.status.success(),
        "a refusal is still an exit 0: {}",
        said(&first)
    );

    let second = machine.run("hook", &["--strict"], &event.to_string());
    let answer: Value =
        serde_json::from_str(String::from_utf8_lossy(&second.stdout).trim()).expect("JSON");
    assert!(
        answer["hookSpecificOutput"]["additionalContext"].is_string(),
        "the second read of a session is a line, not a refusal: {answer}"
    );
}

/// With no daemon there is nothing holding the store open, and the hook must
/// not open it itself — a read that waits on an index run is worse than a
/// figure stated as a floor.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn with_no_daemon_the_hook_writes_no_row() {
    let machine = Machine::new();
    let before = machine.rows();

    let out = machine.hook(&read_event(&machine.held()));
    assert!(out.status.success(), "the hook failed: {}", said(&out));

    assert_eq!(
        machine.rows(),
        before,
        "the hook wrote a ledger row with no daemon running"
    );
}

/// `SEMLITH_LEDGER=0` is a promise about the whole machine, and the hook keeps
/// it the same way every other writer does.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn recording_switched_off_leaves_the_nudge_and_removes_the_row() {
    let machine = Machine::new();
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("hook")
        .env("HOME", &machine.home)
        .env("SEMLITH_HOME", &machine.store_home)
        .env("SEMLITH_LEDGER", "0")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(read_event(&machine.held()).to_string().as_bytes())?;
            child.wait_with_output()
        })
        .expect("running semlith hook");

    assert!(out.status.success(), "{}", said(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("additionalContext"),
        "switching recording off must not switch steering off: {}",
        said(&out)
    );
    assert_eq!(machine.rows(), 0, "a row was written with recording off");
}

/// The hook runs inside a client's tool call, so its cost is a person's wait.
/// Measured rather than asserted: the figure goes in the release record.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn the_hook_answers_inside_its_budget() {
    let machine = Machine::new();
    let event = read_event(&machine.held()).to_string();

    let mut runs: Vec<Duration> = (0..10)
        .map(|_| {
            let started = Instant::now();
            let out = machine.run("hook", &[], &event);
            let elapsed = started.elapsed();
            assert!(out.status.success(), "{}", said(&out));
            elapsed
        })
        .collect();
    runs.sort();
    let median = runs[runs.len() / 2];
    println!(
        "hook median over ten runs: {} ms (budget 50 ms)",
        median.as_millis()
    );
    assert!(
        median < Duration::from_millis(50),
        "the hook took {} ms, over its 50 ms budget",
        median.as_millis()
    );
}

/// Nothing here may touch the developer's own machine, so the fixture's home is
/// the only place anything was written.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn the_hook_writes_nothing_outside_the_redirected_home() {
    let machine = Machine::new();
    let real = dirs_next_home();
    let before = marker_count(&real);

    machine.run(
        "hook",
        &["--strict"],
        &read_event(&machine.held()).to_string(),
    );

    assert_eq!(
        marker_count(&real),
        before,
        "the hook wrote into the real store home"
    );
}

/// The real `~/.semlith/hook-sessions`, which no test may add to.
fn dirs_next_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/nonexistent"))
        .join(".semlith/hook-sessions")
}

fn marker_count(dir: &Path) -> usize {
    std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
}
