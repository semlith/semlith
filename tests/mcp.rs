//! What semlith promises an agent's MCP client: that every protocol revision
//! it advertises, it actually speaks, and that the tool surface covers the
//! read and write sides of the store rather than search alone.
//!
//! These drive a real `semlith mcp` child process over stdio, so they build
//! real stores and download an embedding model on first run:
//!
//! ```sh
//! cargo test --test mcp -- --ignored
//! ```

use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};

const RUST: &str = "The borrow checker proves at compile time that no value has two \
                    mutable aliases. Ownership means each value has a single owner, and \
                    the compiler frees it when that owner leaves scope.";

const BREAD: &str = "Sourdough rises because a starter of flour and water ferments. \
                     Hydration is the ratio of water to flour by weight, and a wetter \
                     dough gives a more open crumb after baking.";

/// Every tool the server is expected to expose, in the order it lists them.
const TOOLS: [&str; 5] = [
    "semlith_search",
    "semlith_stats",
    "semlith_files",
    "semlith_index",
    "semlith_forget",
];

// ---------------------------------------------------------------- T01

/// A server that answers `initialize` with whatever version it was sent is
/// claiming a contract it has not read. Each revision it advertises has to be
/// one it can actually serve, which means a full session per revision.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn every_advertised_revision_gets_a_working_session() {
    let corpus = corpus("rust", &[("ownership.md", RUST)]);
    let store = store_for(&corpus);

    for revision in ["2025-11-25", "2025-06-18", "2024-11-05"] {
        let mut server = Server::open(&[store.path()]);

        let hello = server.call(
            "initialize",
            json!({ "protocolVersion": revision, "capabilities": {} }),
        );
        assert_eq!(
            hello["result"]["protocolVersion"], revision,
            "a revision the server advertises must be answered with itself: {hello}"
        );

        let listed = server.call("tools/list", json!({}));
        let names: Vec<&str> = listed["result"]["tools"]
            .as_array()
            .expect("tools/list returns an array")
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, TOOLS, "wrong tool surface on {revision}");

        let hit = server.call(
            "tools/call",
            json!({
                "name": "semlith_search",
                "arguments": { "query": "who owns a value", "k": 3 }
            }),
        );
        let text = hit["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("ownership.md"),
            "no excerpts on {revision}: {text}"
        );
    }
}

/// A client asking for something we do not implement gets the truth back: the
/// newest revision we do. Echoing the request is how a server ends up
/// pretending to speak a protocol that removed the very handshake it just used.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn an_unimplemented_revision_is_answered_with_one_we_implement() {
    let corpus = corpus("rust", &[("ownership.md", RUST)]);
    let store = store_for(&corpus);
    let mut server = Server::open(&[store.path()]);

    for asked in ["1900-01-01", "2031-03-26"] {
        let hello = server.call(
            "initialize",
            json!({ "protocolVersion": asked, "capabilities": {} }),
        );
        let answered = hello["result"]["protocolVersion"].as_str().unwrap();
        assert_ne!(answered, asked, "the server echoed {asked} back");
        assert_eq!(
            answered, "2025-11-25",
            "an unimplemented revision must be answered with our newest handshake one"
        );
    }
}

// ---------------------------------------------------------------- T02

/// 2026-07-28 removed `initialize` and made `server/discover` mandatory. A
/// client on that revision opens with the probe and nothing else, so the probe
/// has to stand on its own.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn discover_answers_as_the_first_message_of_a_session() {
    let corpus = corpus("rust", &[("ownership.md", RUST)]);
    let store = store_for(&corpus);
    let mut server = Server::open(&[store.path()]);

    let found = server.call("server/discover", modern(json!({})));
    let result = &found["result"];

    assert_eq!(result["resultType"], "complete", "{found}");
    let versions: Vec<&str> = result["supportedVersions"]
        .as_array()
        .expect("supportedVersions is an array")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        versions,
        vec!["2026-07-28", "2025-11-25", "2025-06-18", "2024-11-05"],
        "the advertised list is what the rest of this file is testing"
    );
    assert!(result["capabilities"]["tools"].is_object(), "{found}");
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], "semlith",
        "{found}"
    );
    assert!(result["ttlMs"].is_number(), "{found}");
    assert_eq!(result["cacheScope"], "public", "{found}");
}

/// The modern era carries its version on every request and never shakes hands.
/// A server that only answers after `initialize` is invisible to it.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_modern_call_works_with_no_handshake_at_all() {
    let corpus = corpus("rust", &[("ownership.md", RUST)]);
    let store = store_for(&corpus);
    let mut server = Server::open(&[store.path()]);

    let listed = server.call("tools/list", modern(json!({})));
    assert_eq!(listed["result"]["resultType"], "complete", "{listed}");
    assert!(listed["result"]["ttlMs"].is_number(), "{listed}");
    assert_eq!(listed["result"]["cacheScope"], "private", "{listed}");
    assert_eq!(
        listed["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["version"],
        env!("CARGO_PKG_VERSION"),
        "{listed}"
    );

    // Deterministic order, so a client may cache the list.
    let again = server.call("tools/list", modern(json!({})));
    assert_eq!(
        listed["result"]["tools"], again["result"]["tools"],
        "the tool list must not reorder between calls"
    );

    let hit = server.call(
        "tools/call",
        modern(json!({
            "name": "semlith_search",
            "arguments": { "query": "who owns a value", "k": 3 }
        })),
    );
    assert_eq!(hit["result"]["resultType"], "complete", "{hit}");
    assert!(
        hit["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("ownership.md"),
        "{hit}"
    );
}

/// The one error the modern era defines for this: say what you do speak, so a
/// client can pick again instead of falling back and guessing.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn an_unimplemented_meta_version_is_refused_with_the_list() {
    let corpus = corpus("rust", &[("ownership.md", RUST)]);
    let store = store_for(&corpus);
    let mut server = Server::open(&[store.path()]);

    let refused = server.call(
        "tools/list",
        json!({
            "_meta": { "io.modelcontextprotocol/protocolVersion": "2029-01-01" }
        }),
    );
    assert_eq!(refused["error"]["code"], -32022, "{refused}");
    assert_eq!(
        refused["error"]["data"]["requested"], "2029-01-01",
        "{refused}"
    );
    let supported: Vec<&str> = refused["error"]["data"]["supported"]
        .as_array()
        .expect("the error names what is supported")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(supported.contains(&"2026-07-28"), "{refused}");
}

// ---------------------------------------------------------------- T03

/// "Is this file even indexed" is a different question from "does the corpus
/// discuss this", and an agent that cannot ask the first reads an empty search
/// as an answer to it.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_files_tool_lists_narrows_and_says_what_it_left_out() {
    let corpus = corpus(
        "rust",
        &[
            ("ownership.md", RUST),
            ("borrow.rs", "fn main() { let owner = String::new(); }"),
        ],
    );
    let store = store_for(&corpus);
    let mut server = Server::open(&[store.path()]);
    server.handshake();

    let all = server.tool("semlith_files", json!({}));
    assert!(
        all.contains("ownership.md") && all.contains("borrow.rs"),
        "the file list is incomplete: {all}"
    );

    let narrowed = server.tool("semlith_files", json!({ "ext": ["rs"] }));
    assert!(
        narrowed.contains("borrow.rs") && !narrowed.contains("ownership.md"),
        "ext did not narrow the file list: {narrowed}"
    );

    let capped = server.tool("semlith_files", json!({ "limit": 1 }));
    assert!(
        capped.contains("1 more"),
        "a capped list must say how many it left out: {capped}"
    );
}

// ---------------------------------------------------------------- T04

/// One writer per store is the product's rule. With several stores open there
/// is no "the" store, and guessing one is how an agent writes to the wrong
/// repository.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_write_with_several_stores_open_must_name_one() {
    let rust = corpus("rust", &[("ownership.md", RUST)]);
    let bread = corpus("bread", &[("sourdough.md", BREAD)]);
    let a = store_for(&rust);
    let b = store_for(&bread);
    let mut server = Server::open(&[a.path(), b.path()]);
    server.handshake();

    for tool in ["semlith_index", "semlith_forget"] {
        let ambiguous =
            server.tool_value(tool, json!({ "path": [inner_of(&rust).to_str().unwrap()] }));
        assert_eq!(ambiguous["result"]["isError"], true, "{tool}: {ambiguous}");
        let text = ambiguous["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("rust") && text.contains("bread"),
            "{tool} did not name the stores that are open: {text}"
        );
    }

    // Naming one writes that one, and only that one.
    let forgotten = server.tool(
        "semlith_forget",
        json!({
            "path": inner_of(&bread).join("sourdough.md").to_str().unwrap(),
            "store": "bread"
        }),
    );
    assert!(forgotten.contains("chunk"), "{forgotten}");

    let left = server.tool("semlith_files", json!({ "store": ["rust"] }));
    assert!(
        left.contains("ownership.md"),
        "writing to one store disturbed the other: {left}"
    );
}

/// A file the agent has dropped must stop coming back, or the agent quotes a
/// document it was told is gone.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_forget_tool_removes_a_file_from_search() {
    let corpus = corpus("rust", &[("ownership.md", RUST), ("bread.md", BREAD)]);
    let store = store_for(&corpus);
    let mut server = Server::open(&[store.path()]);
    server.handshake();

    let gone = server.tool(
        "semlith_forget",
        json!({ "path": inner_of(&corpus).join("bread.md").to_str().unwrap() }),
    );
    assert!(
        gone.contains("chunk"),
        "forget did not report what it removed: {gone}"
    );

    let after = server.tool(
        "semlith_search",
        json!({ "query": "hydration of flour and water", "k": 5 }),
    );
    assert!(
        !after.contains("bread.md"),
        "a forgotten file is still searchable: {after}"
    );
}

// ---------------------------------------------------------------- T05

/// An agent that can search a corpus but cannot make one current has to leave
/// MCP to do it. The point of the tool is that it never has to.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_index_tool_makes_a_new_file_searchable_without_a_restart() {
    let corpus = corpus("rust", &[("ownership.md", RUST)]);
    let store = store_for(&corpus);
    let inner = inner_of(&corpus);
    let mut server = Server::open(&[store.path()]);
    server.handshake();

    // Created after the server started, so nothing it loaded at boot knows it.
    fs::write(
        inner.join("lifetimes.md"),
        "A lifetime annotation states how long a reference stays valid, so the \
         compiler can reject one that outlives what it points at.",
    )
    .unwrap();

    let indexed = server.tool(
        "semlith_index",
        json!({ "path": [inner.to_str().unwrap()] }),
    );
    assert!(
        indexed.contains("1 indexed") || indexed.contains("indexed 1"),
        "the index tool did not report what it did: {indexed}"
    );

    let found = server.tool(
        "semlith_search",
        json!({ "query": "how long a reference stays valid", "k": 5 }),
    );
    assert!(
        found.contains("lifetimes.md"),
        "the same session could not search what it just indexed: {found}"
    );
}

/// `semlith watch` holds the write lock for its whole life, and it is the
/// workflow 0.4.0 documented. A write tool that fought it would corrupt the
/// store rather than report a conflict.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_index_tool_refuses_a_store_a_watcher_is_holding() {
    let corpus = corpus("rust", &[("ownership.md", RUST)]);
    let store = store_for(&corpus);
    let inner = inner_of(&corpus);

    let mut watcher = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(store.path())
        .arg("watch")
        .arg(&inner)
        .arg("--debounce")
        .arg("200")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // The lock is taken as the watcher starts; give it the moment it needs.
    std::thread::sleep(std::time::Duration::from_secs(3));

    let mut server = Server::open(&[store.path()]);
    server.handshake();
    let refused = server.tool_value(
        "semlith_index",
        json!({ "path": [inner.to_str().unwrap()] }),
    );

    let _ = watcher.kill();
    let _ = watcher.wait();

    assert_eq!(refused["result"]["isError"], true, "{refused}");
    let text = refused["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("being indexed by"),
        "the refusal does not name the lock holder: {text}"
    );

    // The store survived the refusal, and is still searchable.
    let still = server.tool("semlith_search", json!({ "query": "who owns a value" }));
    assert!(still.contains("ownership.md"), "{still}");
}

/// A tool call that runs past a client's timeout looks exactly like a hung
/// server. Returning early with what remains is the difference between slow
/// and broken — and it only works if the next call carries on.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn an_index_over_its_budget_reports_what_remains_and_resumes() {
    let mut files: Vec<(String, String)> = Vec::new();
    for i in 0..8 {
        files.push((
            format!("note{i}.md"),
            format!("{RUST} This is note number {i}, about ownership and moves."),
        ));
    }
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_str()))
        .collect();
    let corpus = corpus("notes", &borrowed[..1]);
    let store = store_for(&corpus);
    let inner = inner_of(&corpus);
    for (name, body) in &borrowed[1..] {
        fs::write(inner.join(name), body).unwrap();
    }

    // A budget no real work can fit inside, so the first call must stop short.
    let mut server = Server::open_with_budget(&[store.path()], "0");
    server.handshake();

    let first = server.tool(
        "semlith_index",
        json!({ "path": [inner.to_str().unwrap()] }),
    );
    assert!(
        first.contains("remaining"),
        "a truncated index must say what is left: {first}"
    );

    // Calling again continues: eventually everything is indexed, and no call
    // ever re-embeds a file an earlier one finished.
    let mut rounds = 0;
    let mut last = first;
    while last.contains("remaining") && rounds < 20 {
        last = server.tool(
            "semlith_index",
            json!({ "path": [inner.to_str().unwrap()] }),
        );
        rounds += 1;
    }
    assert!(
        !last.contains("remaining"),
        "repeated calls never finished the corpus: {last}"
    );

    let listed = server.tool("semlith_files", json!({}));
    for (name, _) in &borrowed {
        assert!(listed.contains(name), "{name} was never indexed: {listed}");
    }
}

// ---------------------------------------------------------------- helpers

/// A request in the 2026-07-28 era: the version travels with the message
/// rather than being agreed once.
fn modern(mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": { "name": "semlith-tests", "version": "0" },
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    params
}

/// A real `semlith mcp` child process, spoken to over stdio.
struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
    id: u64,
}

impl Server {
    fn open(stores: &[&Path]) -> Self {
        Self::spawn(stores, None)
    }

    fn open_with_budget(stores: &[&Path], seconds: &str) -> Self {
        Self::spawn(stores, Some(seconds))
    }

    fn spawn(stores: &[&Path], budget: Option<&str>) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_semlith"));
        for store in stores {
            cmd.arg("--store").arg(store);
        }
        cmd.arg("mcp");
        if let Some(seconds) = budget {
            cmd.env("SEMLITH_MCP_INDEX_BUDGET", seconds);
        }
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

    /// One request, one response, parsed.
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

    /// A tool call, returning its text content.
    fn tool(&mut self, name: &str, arguments: Value) -> String {
        let response = self.tool_value(name, arguments);
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("no text content in {response}"))
            .to_string()
    }

    fn tool_value(&mut self, name: &str, arguments: Value) -> Value {
        self.call(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn corpus(name: &str, files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::Builder::new().prefix(name).tempdir().unwrap();
    let inner = dir.path().join(name);
    fs::create_dir_all(&inner).unwrap();
    for (file, body) in files {
        fs::write(inner.join(file), body).unwrap();
    }
    dir
}

fn inner_of(corpus: &tempfile::TempDir) -> PathBuf {
    fs::read_dir(corpus.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .expect("corpus has an inner directory")
}

fn store_for(corpus: &tempfile::TempDir) -> StoreDir {
    let inner = inner_of(corpus);
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
    StoreDir(store)
}

struct StoreDir(PathBuf);

impl StoreDir {
    fn path(&self) -> &Path {
        &self.0
    }
}
