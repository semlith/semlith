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

/// Every tool the server exposes, in the order it lists them.
///
/// Read from the server's own definitions rather than repeated here. What these
/// tests are about is that a revision serves the same surface as every other
/// one — not what that surface happens to contain this release — and a second
/// hand-written copy only means a release that adds a tool fails here for the
/// wrong reason.
fn tools() -> Vec<String> {
    semlith::mcp::tool_names()
}

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
        let names: Vec<String> = listed["result"]["tools"]
            .as_array()
            .expect("tools/list returns an array")
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, tools(), "wrong tool surface on {revision}");

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

/// A filter that behaves one way at the terminal and another over MCP is two
/// products. `!` is a new spelling on both, so the two are compared over one
/// store rather than each asserted against a hand-written expectation.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn an_excluded_filter_reads_the_same_at_the_cli_and_over_mcp() {
    let corpus = corpus(
        "rust",
        &[
            ("ownership.md", RUST),
            ("borrow.rs", "fn main() { let owner = String::new(); }"),
            ("vendor/other.rs", "fn vendored() {}"),
        ],
    );
    let store = store_for(&corpus);

    const QUESTION: &str = "ownership and vendored code";

    let cli = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
            .args(["search", QUESTION, "-k", "8", "-s"])
            .arg(store.path())
            .args(args)
            .output()
            .expect("semlith search runs");
        assert!(out.status.success(), "semlith search failed: {out:?}");
        // The "nothing came back" line goes to stderr, and it is half of what
        // this test is about, so both streams are one body here.
        String::from_utf8(out.stdout).unwrap() + &String::from_utf8(out.stderr).unwrap()
    };

    let mut server = Server::open(&[store.path()]);
    server.handshake();
    let tool = |server: &mut Server, extra: Value| {
        let mut args = json!({ "query": QUESTION, "k": 8 });
        for (key, value) in extra.as_object().unwrap() {
            args[key] = value.clone();
        }
        server.tool("semlith_search", args)
    };

    // An exclusion, applied after the inclusions of its own kind.
    let listed = cli(&["--ext", "rs", "--ext", "!md"]);
    let tooled = tool(&mut server, json!({ "ext": ["rs", "!md"] }));
    for body in [&listed, &tooled] {
        assert!(body.contains("borrow.rs"), "the inclusion was lost: {body}");
        assert!(
            !body.contains("ownership.md"),
            "the exclusion was not applied: {body}"
        );
    }

    // An exclusion on its own is everything except.
    let listed = cli(&["--path", "!**/vendor/**"]);
    let tooled = tool(&mut server, json!({ "path": ["!**/vendor/**"] }));
    for body in [&listed, &tooled] {
        assert!(
            body.contains("ownership.md") && body.contains("borrow.rs"),
            "an exclusion alone must still select everything else: {body}"
        );
        assert!(
            !body.contains("other.rs"),
            "the excluded path came back: {body}"
        );
    }

    // And an exclusion that empties the set says so rather than reading as a
    // corpus that has nothing to say.
    let listed = cli(&["--ext", "!md", "--ext", "!rs"]);
    let tooled = tool(&mut server, json!({ "ext": ["!md", "!rs"] }));
    for body in [&listed, &tooled] {
        assert!(
            body.contains("No indexed file matches that path/ext/lang filter"),
            "an emptied filter must name itself: {body}"
        );
    }
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
        let at = inner.join(file);
        // A file may name a subdirectory, so a corpus can hold a vendored tree
        // to filter out as well as a flat list.
        if let Some(parent) = at.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(at, body).unwrap();
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

// ---------------------------------------------------------------- T10

/// The whole of what the agent key can reach is what a copied config file can
/// reach, so `semlith_index` is held to a boundary and to a deny-list. The
/// person typing `semlith index` is the owner of the machine and is not.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn the_index_tool_refuses_a_credential_and_a_path_outside_the_boundary() {
    let home = tempfile::Builder::new()
        .prefix("mcp-boundary")
        .tempdir()
        .unwrap();
    let work = home.path().join("work");
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("ownership.md"), RUST).unwrap();
    // A credential inside the corpus, which is the case a deny-list is for:
    // the walker skips hidden files, and this one is not hidden.
    fs::write(
        work.join("service-account-credentials.json"),
        r#"{"private_key":"-----BEGIN PRIVATE KEY-----"}"#,
    )
    .unwrap();
    fs::write(work.join(".env"), "STRIPE_KEY=sk_live_pretend\n").unwrap();
    // And the directory the finding is named for.
    let ssh = home.path().join(".ssh");
    fs::create_dir_all(&ssh).unwrap();
    fs::write(ssh.join("id_ed25519"), "not really a key\n").unwrap();

    // `semlith mcp` opens a store that already exists, so the corpus is indexed
    // once from the command line first — which is also the asymmetry under
    // test: the same directory, indexed by its owner, keeps the deny-list and
    // loses the boundary.
    let store = home.path().join("store");
    let seeded = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["index", work.to_str().unwrap(), "--quiet"])
        .arg("--store")
        .arg(&store)
        .env("HOME", home.path())
        .env("SEMLITH_HOME", home.path().join("semlith-home"))
        .env("SEMLITH_MODEL_CACHE", model_cache())
        .output()
        .expect("seeding the store");
    assert!(
        seeded.status.success(),
        "seeding failed:\n{}",
        String::from_utf8_lossy(&seeded.stderr)
    );
    // The command line applies the deny-list too, so the two credentials in the
    // corpus were refused there as well.
    let said = String::from_utf8_lossy(&seeded.stderr);
    assert!(
        said.contains("refused:") && said.contains("--include-secrets"),
        "the command line did not apply the deny-list:\n{said}"
    );

    let mut server = Command::new(env!("CARGO_BIN_EXE_semlith"));
    server
        .arg("--store")
        .arg(&store)
        .arg("mcp")
        .env("HOME", home.path())
        .env("SEMLITH_HOME", home.path().join("semlith-home"))
        // HOME is moved so the deny-list's `~/.ssh` is this test's own, which
        // also moves the model cache — so it is named back at the real one.
        .env("SEMLITH_MODEL_CACHE", model_cache());
    let mut child = server
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut s = Server {
        child,
        stdin,
        stdout,
        id: 0,
    };
    s.handshake();

    // The corpus indexes, and the two credentials inside it are refused by
    // name with the rule that refused each.
    let indexed = s.tool("semlith_index", json!({ "path": [work.to_str().unwrap()] }));
    assert!(
        indexed.contains("refused:"),
        "nothing was refused in a directory holding two credentials:\n{indexed}"
    );
    assert!(
        indexed.contains("service-account-credentials.json") && indexed.contains("*credentials*"),
        "the refusal does not name the file and the rule:\n{indexed}"
    );

    // A path outside the boundary, named explicitly, which is what the walker
    // never saw.
    let outside = s.tool("semlith_index", json!({ "path": ["/etc/hosts"] }));
    assert!(
        outside.contains("refused:") && outside.contains("outside"),
        "/etc/hosts was not refused as outside the boundary:\n{outside}"
    );

    // And a key by name, under a denied directory.
    let key = s.tool(
        "semlith_index",
        json!({ "path": [ssh.join("id_ed25519").to_str().unwrap()] }),
    );
    assert!(
        key.contains("refused:") && key.contains(".ssh"),
        "a key under ~/.ssh was not refused:\n{key}"
    );

    // Nothing refused reached the store.
    let files = s.tool("semlith_files", json!({}));
    assert!(
        !files.contains("credentials") && !files.contains("id_ed25519") && !files.contains(".env"),
        "a refused file is in the store:\n{files}"
    );
    assert!(
        files.contains("ownership.md"),
        "the corpus did not index:\n{files}"
    );
}

/// The developer's real model cache, so a test that moves `HOME` does not make
/// every run download 52 MB again.
fn model_cache() -> String {
    if let Ok(dir) = std::env::var("SEMLITH_MODEL_CACHE") {
        return dir;
    }
    format!(
        "{}/.cache/semlith/models",
        std::env::var("HOME").unwrap_or_default()
    )
}

// ---------------------------------------------------------------- T05

/// The handshake used to be behind the embedding model.
///
/// `semlith mcp` called `fleet.warm()` before `mcp::serve` read its first line,
/// so a client's `initialize` waited on an ONNX session — or, on a machine that
/// had never fetched the weights, on a download. A client whose startup timeout
/// expired in that window saw no server and no reason for it, which is the
/// absence 0.21.0 exists to remove.
///
/// Asserted by ordering rather than by a stopwatch: the model is made
/// *impossible* to load — airgapped, with a model cache that is empty — so a
/// handshake that still answers is a handshake that never waited for one. On
/// 0.20.2 this produced zero responses and an exit; the failure is unambiguous
/// either way.
///
/// Needs no download, so it runs on every machine and in CI, which is the point.
#[test]
fn the_handshake_is_answered_without_the_embedding_model() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store").join(".semlith");
    let corpus = dir.path().join("corpus");
    fs::create_dir_all(&corpus).unwrap();
    // An empty corpus makes a real store with no vectors in it, which is the
    // one way to get a store without first loading the model this test is
    // proving the server does not need.
    let made = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(&store)
        .arg("index")
        .arg(&corpus)
        .env("SEMLITH_HOME", dir.path().join("home"))
        .output()
        .unwrap();
    assert!(made.status.success(), "could not build the fixture store");

    let mut child = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(&store)
        .arg("mcp")
        .env("SEMLITH_HOME", dir.path().join("home"))
        .env("SEMLITH_AIRGAP", "1")
        .env("SEMLITH_MODEL_CACHE", dir.path().join("empty-cache"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2024-11-05", "capabilities": {},
                        "clientInfo": { "name": "t", "version": "1" } },
        })
    )
    .unwrap();
    writeln!(
        stdin,
        "{}",
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} })
    )
    .unwrap();
    drop(stdin);

    let done = child.wait_with_output().unwrap();
    let answers: Vec<Value> = String::from_utf8_lossy(&done.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("each line is one JSON-RPC response"))
        .collect();

    assert_eq!(
        answers.len(),
        2,
        "the handshake waited on a model it cannot load: {}",
        String::from_utf8_lossy(&done.stderr),
    );
    assert_eq!(answers[0]["result"]["serverInfo"]["name"], "semlith");
    assert_eq!(
        answers[1]["result"]["tools"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        tools().len(),
        "the tool list shrank when it was answered without the model",
    );

    // The reason is on stderr, where a stdio client captures it, rather than
    // nowhere — a server that quietly cannot search is the same defect wearing
    // a different hat.
    let said = String::from_utf8_lossy(&done.stderr);
    assert!(
        said.contains("could not load the embedding model"),
        "nothing said why searches will fail: {said}",
    );
}

/// A machine with nothing indexed is a server, not an exit code.
///
/// `read_fleet` ended the process with "no semlith store covers …" before a
/// byte of protocol was written, so a fresh install wired into a client was a
/// client that reported no server at all. The tools are listed; the first call
/// that needs a corpus is where the message belongs.
#[test]
fn a_machine_with_no_store_still_answers_the_handshake() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("mcp")
        .env("SEMLITH_HOME", dir.path().join("home"))
        .current_dir(dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} })
    )
    .unwrap();
    writeln!(
        stdin,
        "{}",
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} })
    )
    .unwrap();
    drop(stdin);

    let done = child.wait_with_output().unwrap();
    let answers: Vec<Value> = String::from_utf8_lossy(&done.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("each line is one JSON-RPC response"))
        .collect();
    assert_eq!(
        answers.len(),
        2,
        "a store-less machine got no server: {}",
        String::from_utf8_lossy(&done.stderr),
    );
    assert_eq!(
        answers[1]["result"]["tools"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        tools().len(),
        "a store-less server listed a different tool surface",
    );
}

/// A client that never calls `server/discover` — which is every client on a
/// 2025 revision, and most of them — must still be told what this server is
/// for. Until 0.24.0 the sentence reached only the clients that needed it
/// least.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_handshake_carries_the_same_instructions_discover_sends() {
    let corpus = corpus("rust", &[("ownership.md", RUST)]);
    let store = store_for(&corpus);
    let mut server = Server::open(&[store.path()]);

    let hello = server.call(
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {} }),
    );
    let said = hello["result"]["instructions"]
        .as_str()
        .unwrap_or_else(|| panic!("initialize carries no instructions: {hello}"));

    let found = server.call("server/discover", modern(json!({})));
    let discovered = found["result"]["instructions"]
        .as_str()
        .expect("discover carries instructions");

    assert_eq!(
        said, discovered,
        "the two handshakes describe the same server differently"
    );
    assert!(
        said.contains("semlith_brief"),
        "the instructions name no call: {said}"
    );
}
