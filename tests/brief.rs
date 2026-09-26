//! What one call promises: the spans a search would find, the text of the top
//! ones, and the one-hop neighbourhood of the symbols they sit inside -- all of
//! it under a token budget the caller sets, and all of it saying what found it.
//!
//! These build a real store and embed real text, so they are slow and they
//! download an embedding model on first run:
//!
//! ```sh
//! cargo test --test brief -- --ignored
//! ```

use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

/// A corpus small enough to reason about and real enough to have edges: three
/// Rust functions where one calls the next.
const SOURCE: &str = r#"
/// Decide whether a stored file still looks the way it did when it was read.
pub fn is_stale(recorded_size: u64, size_now: u64) -> bool {
    recorded_size != size_now
}

/// Walk the files a store holds and report the ones that have drifted.
pub fn drifted(files: &[(String, u64, u64)]) -> Vec<String> {
    files
        .iter()
        .filter(|(_, recorded, now)| is_stale(*recorded, *now))
        .map(|(path, _, _)| path.clone())
        .collect()
}

/// The command a person runs to see the drift.
pub fn report(files: &[(String, u64, u64)]) -> String {
    let stale = drifted(files);
    format!("{} files have drifted", stale.len())
}
"#;

const QUESTION: &str = "how does the store decide a file has drifted";

// ---------------------------------------------------------------- T07, T08

/// The release, in one test. One call returns located spans, the text of the
/// top ones, and the callers and callees of the symbols they sit inside -- and
/// every part says which list or edge found it.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn one_call_returns_spans_their_text_and_one_hop_edges_each_labelled() {
    let corpus = corpus(&[("drift.rs", SOURCE)]);
    let store = store_for(&corpus);

    let brief = brief_json(&store, &[QUESTION]);

    let spans = brief["spans"].as_array().expect("spans");
    assert!(!spans.is_empty(), "nothing was located: {brief}");
    for span in spans {
        assert!(span["path"].is_string(), "a span with no path: {span}");
        assert!(
            span["start_line"].as_u64().is_some(),
            "a span with no line range: {span}"
        );
        assert!(
            !span["lists"].as_array().map(Vec::is_empty).unwrap_or(true),
            "a span that does not say which list found it: {span}"
        );
    }

    assert!(
        spans.iter().any(|s| s["text"].is_string()),
        "no span carried its text at the default budget: {brief}"
    );

    let symbols = brief["symbols"].as_array().expect("symbols");
    assert!(
        !symbols.is_empty(),
        "no enclosing symbol brought its neighbourhood: {brief}"
    );
    for symbol in symbols {
        assert_eq!(
            symbol["found_by"], "graph",
            "an edge is not a ranked guess and has to say so: {symbol}"
        );
        let edges = symbol["callers"].as_array().unwrap().len()
            + symbol["callees"].as_array().unwrap().len();
        assert!(edges > 0, "a symbol block with no edges in it: {symbol}");
    }

    // The point of the release: this is what four calls used to assemble.
    assert!(
        symbols.iter().any(|s| {
            let name = s["name"].as_str().unwrap_or("");
            name == "is_stale" || name == "drifted" || name == "report"
        }),
        "none of the corpus's three symbols came back: {brief}"
    );

    let tokens = brief["tokens"].as_i64().expect("a token count");
    let budget = brief["budget"].as_i64().expect("a budget");
    assert!(
        tokens <= budget,
        "the brief spent {tokens} tokens of a {budget} budget"
    );
    assert_eq!(
        budget, 4_000,
        "the default budget is 4 000 tokens and callers depend on it"
    );
    assert!(
        matches!(
            brief["counted_with"].as_str(),
            Some("model") | Some("chars4")
        ),
        "a token figure with no label on what counted it: {brief}"
    );
}

// ---------------------------------------------------------------- T07

/// A budget too small for the top span's text is not an error and not a span
/// cut in half. It is a shorter answer that says what it dropped -- the
/// locators survive, because a locator is what an agent cannot act without.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_budget_below_one_span_still_locates_and_says_what_it_cut() {
    let corpus = corpus(&[("drift.rs", SOURCE)]);
    let store = store_for(&corpus);

    let brief = brief_json(&store, &[QUESTION, "--budget", "1"]);

    let spans = brief["spans"].as_array().expect("spans");
    assert!(
        !spans.is_empty(),
        "a tiny budget dropped the locators too, so the answer says nothing: {brief}"
    );
    assert!(
        spans.iter().all(|s| s["text"].is_null()),
        "text survived a one-token budget: {brief}"
    );
    assert!(
        spans.iter().all(|s| s["path"].is_string()),
        "a span lost its path: {brief}"
    );
    assert_eq!(
        brief["cut"]["span_text"].as_u64(),
        Some(spans.len() as u64),
        "the brief dropped every span's text and did not say so: {brief}"
    );
}

/// A budget is a ceiling at every size, not only at the default.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_budget_is_a_ceiling_the_answer_never_crosses() {
    let corpus = corpus(&[("drift.rs", SOURCE)]);
    let store = store_for(&corpus);

    for budget in ["1", "50", "200", "4000"] {
        let brief = brief_json(&store, &[QUESTION, "--budget", budget]);
        let spent = brief["tokens"].as_i64().unwrap();
        let floor = brief["floor"].as_i64().expect("a stated floor");
        let ceiling: i64 = budget.parse().unwrap();
        assert_eq!(
            brief["budget"].as_i64(),
            Some(ceiling),
            "the brief reported a budget it was not given"
        );
        // The best-ranked locator is the one thing a budget cannot drop, and
        // the brief says what it costs rather than leaving a caller to work out
        // why the smaller number lost.
        assert!(
            spent <= ceiling.max(floor),
            "a {ceiling}-token budget bought {spent} tokens over a floor of {floor}: {brief}"
        );
    }
}

/// Zero is not a number of tokens, and the failure says so before a model is
/// loaded.
#[test]
fn a_budget_of_zero_is_refused() {
    let corpus = corpus(&[("drift.rs", SOURCE)]);
    let store = corpus.path().join("src").join(".semlith");
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(&store)
        .args(["brief", QUESTION, "--budget", "0"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "a zero budget was accepted");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("at least 1"),
        "the refusal does not say what a budget is: {stderr}"
    );
}

// ---------------------------------------------------------------- T09

/// The agent surface and the terminal surface are one implementation. A test,
/// because two renderers of one assembly is how they drift.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_mcp_tool_and_the_cli_answer_the_same_question_the_same_way() {
    let corpus = corpus(&[("drift.rs", SOURCE)]);
    let store = store_for(&corpus);

    let cli = brief_json(&store, &[QUESTION]);

    let mut server = Server::open(&[&store]);
    server.handshake();
    let tool = server.tool("semlith_brief", json!({ "question": QUESTION }));

    // The tool renders text and the CLI renders JSON, so what is compared is
    // the assembly under both: the same spans, in the same order, and the same
    // symbols brought with them.
    for span in cli["spans"].as_array().unwrap() {
        let path = span["path"].as_str().unwrap();
        let start = span["start_line"].as_u64().unwrap();
        let end = span["end_line"].as_u64().unwrap();
        assert!(
            tool.contains(&format!("{path}:{start}-{end}")),
            "the CLI located {path}:{start}-{end} and the tool did not:\n{tool}"
        );
    }
    for symbol in cli["symbols"].as_array().unwrap() {
        let name = symbol["name"].as_str().unwrap();
        assert!(
            tool.contains(&format!("{name} [graph]")),
            "the CLI brought {name}'s neighbourhood and the tool did not:\n{tool}"
        );
    }
    assert!(
        tool.contains("counted with"),
        "the tool's answer does not say what counted its tokens:\n{tool}"
    );
}

/// Across two stores a path means nothing without its store, and a name both
/// repositories define must bring only its own store's edges. The walk stop of
/// 0.30.0 found both: unlabelled spans, and edges looked up across the fleet.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_brief_across_stores_labels_each_span_and_keeps_edges_in_their_store() {
    let plain = corpus(&[("drift.rs", SOURCE)]);
    let audited = corpus(&[(
        "drift.rs",
        &format!(
            "{SOURCE}\n/// Audit the drift before a release.\npub fn audit_drift(files: &[(String, u64, u64)]) -> usize {{\n    drifted(files).len()\n}}\n"
        ),
    )]);
    let a = store_for(&plain);
    let b = store_for(&audited);

    let mut server = Server::open(&[&a, &b]);
    server.handshake();
    // A store's label is its path, so the audited corpus's unique directory
    // name tells its lines from the other store's.
    let with_audit = audited
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let tool = server.tool("semlith_brief", json!({ "question": QUESTION }));
    let mut block: Option<&str> = None;
    for line in tool.lines() {
        let located = line.contains("drift.rs:") && !line.starts_with(' ');
        if located || line.ends_with("[graph]") {
            assert!(
                line.starts_with('['),
                "a line of a two-store brief does not name its store: {line}\n{tool}"
            );
        }
        if line.ends_with("[graph]") {
            block = Some(line);
        } else if (line.starts_with("    called by ") || line.starts_with("    calls "))
            && line.contains("audit_drift")
        {
            let head = block.expect("an edge outside a symbol");
            let label = head.split(']').next().unwrap();
            assert!(
                label.contains(&with_audit),
                "{with_audit}'s caller is listed under another store's symbol: {head}\n{tool}"
            );
        } else if !line.starts_with(' ') {
            block = None;
        }
    }
}

/// A root-relative span held by two stores is a guess either way, so the read
/// is refused naming both, as it is for two roots inside one store.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_relative_span_two_stores_hold_is_refused_naming_both() {
    let one = corpus(&[("drift.rs", SOURCE)]);
    let two = corpus(&[("drift.rs", SOURCE)]);
    let a = store_for(&one);
    let b = store_for(&two);

    let mut server = Server::open(&[&a, &b]);
    server.handshake();
    let said = server
        .call(
            "tools/call",
            json!({ "name": "semlith_read", "arguments": { "target": "drift.rs:1-3" } }),
        )
        .to_string();
    for corpus in [&one, &two] {
        let dir = corpus
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            said.contains(&dir) && said.contains("2 stores"),
            "the refusal does not name {dir}: {said}"
        );
    }

    // Narrowed to one store, it reads.
    let label = server.tool("semlith_stats", json!({}));
    let first = label.lines().next().unwrap().split(": ").next().unwrap();
    let read = server.tool(
        "semlith_read",
        json!({ "target": "drift.rs:1-3", "store": [first] }),
    );
    assert!(
        read.contains("drift.rs:1-3"),
        "a named store did not read: {read}"
    );
}

/// The tool list grew by one and the gate grew with it, on purpose.
#[test]
fn the_tool_list_advertises_brief() {
    let names = semlith::mcp::tool_names();
    assert!(
        names.iter().any(|n| n == "semlith_brief"),
        "semlith_brief is not advertised: {names:?}"
    );
    // Sixteen from 0.26.0: impact, trace and report joined the thirteen.
    // The number is here so a tool cannot be added without the token gate in
    // `tests/retrieval.rs` and its byte proxy in `mcp.rs` being moved too.
    assert_eq!(
        names.len(),
        16,
        "the tool count moved without the gate moving with it: {names:?}"
    );
}

// ---------------------------------------------------------------- helpers

/// `semlith brief --json`, parsed.
fn brief_json(store: &Path, args: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(store)
        .arg("brief")
        .args(args)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "semlith brief failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "brief --json is not JSON: {e}: {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn corpus(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::Builder::new().prefix("brief").tempdir().unwrap();
    let inner = dir.path().join("src");
    fs::create_dir_all(&inner).unwrap();
    for (file, body) in files {
        fs::write(inner.join(file), body).unwrap();
    }
    dir
}

/// Index the corpus into a `.semlith` store beside it, as a developer would.
fn store_for(corpus: &tempfile::TempDir) -> PathBuf {
    let inner = corpus.path().join("src");
    let store = inner.join(".semlith");
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(&store)
        .arg("index")
        .arg(&inner)
        .arg("--quiet")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "indexing failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    store
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
    id: u64,
}

impl Server {
    fn open(stores: &[&Path]) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_semlith"));
        for store in stores {
            cmd.arg("--store").arg(store);
        }
        cmd.arg("mcp");
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            id: 0,
        }
    }

    fn handshake(&mut self) {
        self.call(
            "initialize",
            json!({ "protocolVersion": "2025-11-25", "capabilities": {} }),
        );
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let request = json!({
            "jsonrpc": "2.0",
            "id": self.id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{request}").unwrap();
        self.stdin.flush().unwrap();

        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .expect("the server answered");
        assert!(!line.trim().is_empty(), "the server closed on {method}");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("bad JSON for {method}: {e}: {line}"))
    }

    fn tool(&mut self, name: &str, arguments: Value) -> String {
        let response = self.call(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        );
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("no text content in {response}"))
            .to_string()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
