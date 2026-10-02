//! The 0.35.0 backend the portal's v6 pages are drawn from: store creation
//! and settings, the ledger switch, review decisions, run history, airgap and
//! the Agents page's tool table.
//!
//! None of these indexes anything, so none needs a model. Every daemon here
//! runs under a scratch `SEMLITH_HOME` and `HOME`; nothing touches a real
//! login service, client configuration or registry.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct Daemon {
    child: Child,
    port: u16,
    token: String,
}

impl Daemon {
    /// Airgapped: a daemon with a store open warms the embedding model at
    /// start, and in a scratch home that is a 52 MB download the tests here
    /// neither need nor should make. The airgap test starts its own.
    fn start(home: &Path) -> Self {
        Self::start_with(home, true)
    }

    fn start_with(home: &Path, airgapped: bool) -> Self {
        std::fs::create_dir_all(home).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_semlith"));
        command
            .args(["start", "--port", "0"])
            .env("SEMLITH_HOME", home)
            .env("HOME", home)
            // The login-item seam: anything service.rs would install lands
            // here, never in the real LaunchAgents or systemd user directory.
            .env("SEMLITH_SERVICE_DIR", home.join("service"))
            .env_remove("SEMLITH_STORE")
            .env_remove("SEMLITH_PORT")
            .env_remove("SEMLITH_LEDGER")
            .env_remove("SEMLITH_AIRGAP");
        if airgapped {
            command.env("SEMLITH_AIRGAP", "1");
        }
        let mut child = command
            .current_dir(home)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("semlith start runs");
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut line)
            .expect("the daemon prints its URL");
        let rest = line
            .trim()
            .strip_prefix("http://127.0.0.1:")
            .expect("a loopback URL");
        let (port, token) = rest.split_once("/?token=").expect("a token in the URL");
        Self {
            child,
            port: port.parse().unwrap(),
            token: token.to_string(),
        }
    }

    fn send(&self, method: &str, path: &str, body: &str) -> (u16, Value) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("the daemon listens");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            self.token,
            body.len(),
        );
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let status = text
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
        // Chunked or not, the routes here answer one JSON object.
        let start = body.find('{').unwrap_or(0);
        let end = body.rfind('}').map(|e| e + 1).unwrap_or(body.len());
        let value = serde_json::from_str(&body[start..end]).unwrap_or(Value::Null);
        (status, value)
    }

    /// One MCP tool call over `/mcp`, with the agent key, as an agent makes it.
    fn call(&self, home: &Path, tool: &str, args: Value) -> Value {
        let key = std::fs::read_to_string(home.join("agent.key")).unwrap();
        let body = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": tool, "arguments": args },
        })
        .to_string();
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\n\
             Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            key.trim(),
            body.len(),
        );
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
        let start = body.find('{').unwrap_or(0);
        let end = body.rfind('}').map(|e| e + 1).unwrap_or(body.len());
        serde_json::from_str(&body[start..end]).unwrap_or(Value::Null)
    }

    fn get(&self, path: &str) -> Value {
        let (status, body) = self.send("GET", path, "");
        assert_eq!(status, 200, "GET {path}: {body}");
        body
    }

    fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.send("POST", path, &body.to_string())
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The ledger's switches are process-wide, so the tests here that flip them
/// in this process run one at a time.
static LEDGER: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn home() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    (dir, home)
}

// ------------------------------------------------------------------ A3

/// Paused, a retrieval writes no row; resumed, the next row chains to the
/// last real one and the chain still verifies.
#[test]
fn a_paused_ledger_writes_nothing_and_a_resume_chains_on() {
    let _one = LEDGER.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let store = semlith::Semlith::open(dir.path().join("store"), None).unwrap();
    let row = |path: &str| semlith::store::Acceptance {
        path: path.to_string(),
        class: "content".to_string(),
        mode: "redacted".to_string(),
        fingerprints: Vec::new(),
        confidence: Some(90),
        at: 0,
        source: "portal".to_string(),
    };
    let count = || semlith::store::ledger_totals(store.db()).unwrap().0;

    semlith::ledger::acceptance(store.db(), &row("/a"), "accept").unwrap();
    assert_eq!(count(), 1);

    semlith::ledger::set_paused(true);
    assert_eq!(
        semlith::ledger::recording_state(),
        json!({ "on": false, "reason": "paused" })
    );
    semlith::ledger::acceptance(store.db(), &row("/b"), "accept").unwrap();
    assert_eq!(count(), 1, "a paused ledger wrote a row");

    semlith::ledger::set_paused(false);
    semlith::ledger::acceptance(store.db(), &row("/c"), "accept").unwrap();
    assert_eq!(count(), 2);
    assert_eq!(semlith::store::ledger_break(store.db()).unwrap(), None);
}

/// The route pauses, the state says why, and it survives a restart.
#[test]
fn the_recording_switch_is_a_route_and_outlives_the_daemon() {
    let (_dir, home) = home();
    {
        let daemon = Daemon::start(&home);
        assert_eq!(
            daemon.get("/api/about")["recording"],
            json!({ "on": true, "reason": null })
        );
        let (status, body) = daemon.post("/api/ledger/recording", json!({ "on": false }));
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            body["recording"],
            json!({ "on": false, "reason": "paused" })
        );
        assert_eq!(daemon.get("/api/about")["ledger"], json!(false));
    }
    let daemon = Daemon::start(&home);
    assert_eq!(
        daemon.get("/api/about")["recording"],
        json!({ "on": false, "reason": "paused" }),
        "the pause did not survive a restart"
    );
    let (_, body) = daemon.post("/api/ledger/recording", json!({ "on": true }));
    assert_eq!(body["recording"], json!({ "on": true, "reason": null }));
    assert_eq!(
        daemon.get("/api/ledger")["recording"],
        json!({ "on": true, "reason": null })
    );
}

// ------------------------------------------------------------------ A5

/// The switch airgaps this daemon at once, refuses before a socket is
/// opened, counts nothing for a refused connection, and outlives a restart.
#[test]
fn the_airgap_switch_refuses_before_any_connection_and_is_kept() {
    let (_dir, home) = home();
    {
        let daemon = Daemon::start_with(&home, false);
        let privacy = daemon.get("/api/privacy");
        assert_eq!(privacy["airgap"], json!({ "on": false, "reason": null }));
        assert_eq!(privacy["outbound"]["count"], json!(0));
        assert!(privacy["outbound"]["since"].as_u64().is_some());

        let (status, body) = daemon.post("/api/airgap", json!({ "on": true }));
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["airgap"], json!({ "on": true, "reason": "runtime" }));

        let (status, body) = daemon.post("/api/upgrade", json!({ "action": "check" }));
        assert_ne!(status, 200, "an airgapped check answered: {body}");
        assert!(
            body["error"].as_str().unwrap_or("").contains("airgap"),
            "the refusal does not say why: {body}"
        );
        assert_eq!(daemon.get("/api/privacy")["outbound"]["count"], json!(0));
    }
    let daemon = Daemon::start_with(&home, false);
    assert_eq!(
        daemon.get("/api/privacy")["airgap"],
        json!({ "on": true, "reason": "runtime" })
    );
    let (_, body) = daemon.post("/api/airgap", json!({ "on": false }));
    assert_eq!(body["airgap"], json!({ "on": false, "reason": null }));
}

// ------------------------------------------------------------------ A1, A2

fn row<'a>(stores: &'a Value, name: &str) -> Option<&'a Value> {
    stores["stores"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
}

/// A named empty store is made, served at once with nothing in it, and a
/// bad or taken name is refused in the page's own words.
#[test]
fn a_store_is_created_empty_and_served_at_once() {
    let (_dir, home) = home();
    let daemon = Daemon::start(&home);

    let (status, body) = daemon.post(
        "/api/store/create",
        json!({ "name": "research-notes", "kind": "docs" }),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["name"], "research-notes");
    assert_eq!(body["kind"], "docs");
    assert!(
        body["dir"].as_str().unwrap().ends_with("research-notes"),
        "{body}"
    );

    let stores = daemon.get("/api/stores");
    let made = row(&stores, "research-notes").expect("the new store is not served");
    assert_eq!(made["files"], json!(0));
    assert_eq!(made["kind"], "docs");
    assert_eq!(made["lean"], "either");
    assert_eq!(made["watch"], json!(true));
    assert_eq!(made["record"], json!(true));
    assert_eq!(made["gitignore"], json!(true));
    assert!(made.get("unopened").is_none(), "{made}");

    let (status, body) = daemon.post("/api/store/create", json!({ "name": "Research Notes" }));
    assert_eq!(status, 400);
    assert_eq!(
        body["error"],
        "Use lowercase letters, digits and dashes — for example research-notes."
    );
    let (status, body) = daemon.post("/api/store/create", json!({ "name": "research-notes" }));
    assert_eq!(status, 400);
    assert_eq!(
        body["error"],
        "“research-notes” is already a store on this machine. Pick another name."
    );
}

/// Settings land in the registry, show on the row, survive a restart, and a
/// rename is picked up by an agent's next call without a restart.
#[test]
fn store_settings_persist_and_a_rename_resolves_for_agents() {
    let (_dir, home) = home();
    {
        let daemon = Daemon::start(&home);
        daemon.post("/api/store/create", json!({ "name": "alpha" }));

        let (status, body) = daemon.post(
            "/api/store/settings",
            json!({ "store": "alpha", "lean": "code", "watch": false, "record": false,
                    "gitignore": false, "kind": "code" }),
        );
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            body,
            json!({ "name": "alpha", "kind": "code", "lean": "code", "watch": false,
                    "record": false, "gitignore": false })
        );
        let (status, body) = daemon.post(
            "/api/store/settings",
            json!({ "store": "alpha", "lean": "prose" }),
        );
        assert_eq!(status, 400, "{body}");
    }

    let daemon = Daemon::start(&home);
    let stores = daemon.get("/api/stores");
    let alpha = row(&stores, "alpha").expect("alpha is served");
    assert_eq!(alpha["lean"], "code");
    assert_eq!(alpha["watch"], json!(false));
    assert_eq!(alpha["record"], json!(false));
    assert_eq!(alpha["gitignore"], json!(false));

    let found = daemon.call(&home, "semlith_files", json!({ "store": "alpha" }));
    assert_ne!(found["result"]["isError"], json!(true), "{found}");

    let (status, body) = daemon.post(
        "/api/store/settings",
        json!({ "store": "alpha", "rename": "beta" }),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["name"], "beta");
    assert_eq!(body["lean"], "code", "a rename lost the settings");

    let stores = daemon.get("/api/stores");
    assert!(
        row(&stores, "alpha").is_none(),
        "the old name is still served"
    );
    assert!(row(&stores, "beta").is_some(), "the new name is not served");
    assert!(home.join("stores").join("beta").join("store.db").exists());
    assert!(!home.join("stores").join("alpha").exists());

    let renamed = daemon.call(&home, "semlith_files", json!({ "store": "beta" }));
    assert_ne!(renamed["result"]["isError"], json!(true), "{renamed}");
    let old = daemon.call(&home, "semlith_files", json!({ "store": "alpha" }));
    assert!(
        old["result"]["isError"] == json!(true) || old.get("error").is_some(),
        "the old name still resolves: {old}"
    );
}

/// A store whose settings say `record: false` gets no retrieval rows, and
/// the others still do.
#[test]
fn a_store_switched_off_records_nothing() {
    let _one = LEDGER.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store");
    drop(semlith::Semlith::open(&path, None).unwrap());
    let fleet = semlith::fleet::Fleet::open(std::slice::from_ref(&path)).unwrap();
    let who = semlith::ledger::Who {
        client: "test",
        session: "s",
        version: "",
    };
    let rows = || {
        let s = semlith::Semlith::open_existing(&path).unwrap();
        semlith::store::ledger_totals(s.db()).unwrap().0
    };
    let ask = || {
        semlith::ledger::reply(&fleet, &who, "search", "q", "", &[], Duration::ZERO);
    };
    ask();
    assert_eq!(rows(), 1);
    semlith::ledger::set_unrecorded(vec![path.clone()]);
    ask();
    assert_eq!(rows(), 1, "a store switched off was recorded into");
    semlith::ledger::set_unrecorded(Vec::new());
    ask();
    assert_eq!(rows(), 2);
}

/// The Respect .gitignore switch: a file `.gitignore` names is in the scan
/// only when the switch is off, and the choice is kept on the store.
#[test]
fn the_gitignore_switch_decides_what_the_scan_walks() {
    let (_dir, home) = home();
    let corpus = home.join("notes");
    std::fs::create_dir_all(&corpus).unwrap();
    std::fs::write(
        corpus.join("kept.md"),
        "# kept\n\nwords about kept things\n",
    )
    .unwrap();
    std::fs::write(
        corpus.join("ignored.md"),
        "# ignored\n\nwords about others\n",
    )
    .unwrap();
    std::fs::write(corpus.join(".gitignore"), "ignored.md\n").unwrap();

    let daemon = Daemon::start(&home);
    daemon.post("/api/store/create", json!({ "name": "notes" }));
    let scan = |gitignore: bool| {
        let (status, body) = daemon.post(
            "/api/index",
            json!({ "store": "notes", "path": [corpus.display().to_string()],
                    "scan_only": true, "gitignore": gitignore }),
        );
        assert_eq!(status, 200, "{body}");
        body["runs"][0]["plan"]["embed"].as_u64().unwrap()
    };
    assert_eq!(scan(true), 1, "the .gitignore'd file was walked");
    assert_eq!(scan(false), 2, "switching .gitignore off left the file out");
    let stores = daemon.get("/api/stores");
    assert_eq!(row(&stores, "notes").unwrap()["gitignore"], json!(false));
}

/// Add sources: a folder outside a store's roots is refused by the boundary
/// unless the portal says it is being added, and then it becomes a root.
#[test]
fn add_sources_makes_a_folder_a_root_and_the_boundary_holds_without_it() {
    let (_dir, home) = home();
    let first = home.join("first");
    let second = home.join("second");
    for (dir, file) in [(&first, "a.md"), (&second, "b.md")] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(file), "# words\n\nsome words to index\n").unwrap();
    }
    let daemon = Daemon::start(&home);
    daemon.post("/api/store/create", json!({ "name": "notes" }));
    let index = |path: &PathBuf, add: bool| {
        daemon.post(
            "/api/index",
            json!({ "store": "notes", "path": [path.display().to_string()],
                    "review": "always", "add_roots": add }),
        )
    };
    let (status, body) = index(&first, false);
    assert_eq!(status, 200, "an empty store takes its first folder: {body}");

    let (status, body) = index(&second, false);
    assert_eq!(
        status, 403,
        "a folder outside the roots passed the boundary: {body}"
    );
    assert!(body.to_string().contains("outside the boundary"), "{body}");

    let (status, body) = index(&second, true);
    assert_ne!(status, 403, "Add sources was refused: {body}");
    let stores = daemon.get("/api/stores");
    let roots = row(&stores, "notes").unwrap()["roots"].to_string();
    assert!(
        roots.contains("second"),
        "the added folder is not one of the store's roots: {roots}"
    );
}

// ------------------------------------------------------------------ A7, A8

/// The wizard's review: the scan's items carry the same risk fields the
/// not-indexed table does, a held scan's files can be decided before any
/// pass ran, decisions are bulk and undoable, and a credential is never in.
#[test]
fn review_items_are_scored_and_decided_in_bulk_before_indexing() {
    let (_dir, home) = home();
    let corpus = home.join("work");
    std::fs::create_dir_all(&corpus).unwrap();
    let key = semlith::keyscan::forge(3);
    std::fs::write(corpus.join("a.rs"), format!("let token = \"{key}\";\n")).unwrap();
    std::fs::write(
        corpus.join("b.rs"),
        format!("let other = \"{}\";\n", semlith::keyscan::forge(3)),
    )
    .unwrap();
    std::fs::write(corpus.join("ok.md"), "# fine\n\nnothing here\n").unwrap();
    std::fs::write(corpus.join("id_rsa"), "not really a key\n").unwrap();

    let daemon = Daemon::start(&home);
    daemon.post("/api/store/create", json!({ "name": "work" }));
    let plan = |daemon: &Daemon| {
        let (status, body) = daemon.post(
            "/api/index",
            json!({ "store": "work", "path": [corpus.display().to_string()], "scan_only": true }),
        );
        assert_eq!(status, 200, "{body}");
        body["runs"][0]["plan"].clone()
    };
    let first = plan(&daemon);
    let review = first["review"].as_array().unwrap();
    assert_eq!(review.len(), 2, "{first}");
    for item in review {
        assert_eq!(item["band"], "high", "{item}");
        assert_eq!(item["suggest"], "out");
        assert!(item["why"].as_str().is_some_and(|w| !w.is_empty()));
        assert!(!item.to_string().contains(&key), "a value reached the plan");
    }
    assert!(
        first["not_indexed_paths"]["content"]
            .as_array()
            .is_some_and(|p| p.len() == 2),
        "{first}"
    );

    let a = corpus.join("a.rs").display().to_string();
    let b = corpus.join("b.rs").display().to_string();
    let (status, body) = daemon.post(
        "/api/refused/decide",
        json!({ "store": "work", "files": [a, b], "decision": "out" }),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["failed"], json!(0), "{body}");
    assert_eq!(
        plan(&daemon)["review"].as_array().unwrap().len(),
        0,
        "kept out is not offered again"
    );

    let decided = daemon.get("/api/decisions?store=work");
    let rows = decided["stores"][0]["rows"].as_array().unwrap();
    let kept: Vec<&Value> = rows.iter().filter(|r| r["outcome"] == "kept out").collect();
    assert_eq!(kept.len(), 2, "{decided}");
    assert!(
        kept.iter()
            .all(|r| r["by"] == "you" && r["can_undo"] == json!(true))
    );
    assert_eq!(
        daemon.get("/api/refused?decisions=1&store=work"),
        decided,
        "the two spellings of the decisions route disagree"
    );

    let (_, body) = daemon.post(
        "/api/refused/decide",
        json!({ "store": "work", "files": [a], "decision": "reset" }),
    );
    assert_eq!(body["failed"], json!(0), "{body}");
    assert_eq!(
        plan(&daemon)["review"].as_array().unwrap().len(),
        1,
        "reset did not undo"
    );

    let rsa = corpus.join("id_rsa").display().to_string();
    let (_, body) = daemon.post(
        "/api/refused/decide",
        json!({ "store": "work", "files": [rsa, b], "decision": "in" }),
    );
    let results = body["results"].as_array().unwrap();
    assert_eq!(
        results[0]["ok"],
        json!(false),
        "a credential was let in: {body}"
    );
    assert!(
        results[0]["error"].as_str().unwrap().contains("id_rsa"),
        "{body}"
    );
}

// ------------------------------------------------------------------ A9, A10

/// Picked files are re-indexed as one run named for them, and the finished
/// run is in the history after a restart. Airgapped, the run cannot load the
/// model and fails — which is still a finished run the history must keep.
#[test]
fn picked_files_reindex_as_one_run_and_its_record_survives_a_restart() {
    let (_dir, home) = home();
    let corpus = home.join("proj");
    std::fs::create_dir_all(corpus.join("src")).unwrap();
    std::fs::write(corpus.join("src/a.rs"), "fn a() {}\n").unwrap();
    {
        let daemon = Daemon::start(&home);
        daemon.post("/api/store/create", json!({ "name": "proj" }));
        // A root first, as adding a folder would record it.
        let _ = daemon.post(
            "/api/index",
            json!({ "store": "proj", "path": [corpus.display().to_string()], "scan_only": true }),
        );
        let (status, body) = daemon.post(
            "/api/index",
            json!({ "store": "proj", "files": [corpus.join("src/a.rs").display().to_string(), "nope.rs"] }),
        );
        assert_eq!(status, 400, "a missing file was queued: {body}");
        assert!(
            body["error"].as_str().unwrap().contains("nope.rs"),
            "{body}"
        );

        let (status, body) = daemon.post(
            "/api/index",
            json!({ "store": "proj", "files": [corpus.join("src/a.rs").display().to_string()] }),
        );
        assert_eq!(status, 200, "{body}");
        let run = body["runs"][0]["run"].as_u64().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let runs = daemon.get("/api/index/runs");
            let card = runs["runs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == run)
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(card["kind"], "files", "{runs}");
            if matches!(card["status"].as_str(), Some("done" | "failed" | "stopped")) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the run never finished: {runs}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let daemon = Daemon::start(&home);
    let runs = daemon.get("/api/index/runs");
    let history = runs["history"].as_array().unwrap();
    let row = history
        .iter()
        .find(|r| r["kind"] == "Re-index 1 files")
        .unwrap_or_else(|| panic!("no history row: {runs}"));
    assert_eq!(row["store"], "proj");
    assert!(row["result"].is_string() && row["log"].is_array(), "{row}");
}

// ------------------------------------------------------------------ ledger repair

fn row_of(n: i64) -> semlith::store::NewRetrieval<'static> {
    semlith::store::NewRetrieval {
        client_version: "",
        client: "test",
        session: "s",
        tool: "search",
        query: "q",
        hits: 1,
        micros: n,
        excerpt_tokens: 10,
        whole_file_tokens: 100,
        stale_hits: 0,
        tokenizer: "chars4",
        query_id: "",
    }
}

/// A broken chain is named by row, re-anchored by a note row appended onto
/// its head, and verifies after; no recorded row changes and no total moves.
#[test]
fn a_broken_ledger_is_named_repaired_by_a_note_and_its_totals_do_not_move() {
    let (_dir, home) = home();
    {
        let daemon = Daemon::start(&home);
        daemon.post("/api/store/create", json!({ "name": "audit" }));
    }
    let dir = home.join("stores").join("audit");
    let snapshot = |db: &rusqlite::Connection| -> Vec<String> {
        let mut stmt = db
            .prepare("SELECT id, at, client, query, hits, micros, prev, hash FROM retrievals WHERE id <= 3 ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| {
            Ok(format!(
                "{}|{}|{}|{}|{}|{}|{}|{}",
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
    };
    let (before, totals) = {
        let store = semlith::Semlith::open(&dir, None).unwrap();
        for n in 1..=3 {
            semlith::store::record_retrieval(store.db(), &row_of(n)).unwrap();
        }
        semlith::store::read_only(store.db(), false).unwrap();
        store
            .db()
            .execute("UPDATE retrievals SET query = 'edited' WHERE id = 2", [])
            .unwrap();
        (
            snapshot(store.db()),
            semlith::store::ledger_totals(store.db()).unwrap(),
        )
    };

    let daemon = Daemon::start(&home);
    let ledger = daemon.get("/api/ledger");
    assert_eq!(ledger["intact"], json!(false), "{ledger}");
    assert_eq!(ledger["break"]["store"], "audit");
    assert_eq!(ledger["break"]["row"], json!(2));
    assert!(ledger["break"]["at"].is_i64());

    let (status, body) = daemon.post("/api/ledger/verify", json!({ "repair": false }));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["stores"][0]["intact"], json!(false));
    assert_eq!(body["stores"][0]["break_row"], json!(2));

    let (_, body) = daemon.post("/api/ledger/verify", json!({ "repair": true }));
    assert_eq!(body["stores"][0]["intact"], json!(true), "{body}");
    assert_eq!(body["stores"][0]["repaired"], json!([2]));
    assert_eq!(
        body["stores"][0]["rows"],
        json!(4),
        "one note row was appended"
    );
    assert_eq!(daemon.get("/api/ledger")["intact"], json!(true));
    drop(daemon);

    let store = semlith::Semlith::open_existing(&dir).unwrap();
    assert_eq!(snapshot(store.db()), before, "a recorded row changed");
    assert_eq!(semlith::store::ledger_totals(store.db()).unwrap(), totals);
    assert_eq!(semlith::store::ledger_break(store.db()).unwrap(), None);
}

// ------------------------------------------------------------------ A6

/// Start at login from the page installs and removes the service through
/// the seam — never the real service manager — and About says which.
#[test]
fn start_at_login_installs_and_removes_through_the_seam() {
    let (_dir, home) = home();
    let daemon = Daemon::start(&home);
    let login = daemon.get("/api/about")["login"].clone();
    assert_eq!(login["installed"], json!(false), "{login}");
    assert!(login["mechanism"].is_string());

    let (status, body) = daemon.post("/api/login-item", json!({ "on": true }));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["login"]["installed"], json!(true), "{body}");
    let file = home.join("service").join("com.semlith.daemon.service");
    assert!(file.is_file(), "the seam was not written");
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("start --port"),
        "a daemon on a non-default port must keep its port at the next login"
    );
    assert_eq!(daemon.get("/api/about")["login"]["installed"], json!(true));

    let (_, body) = daemon.post("/api/login-item", json!({ "on": false }));
    assert_eq!(body["login"]["installed"], json!(false), "{body}");
    assert!(!file.exists());
}

// ------------------------------------------------------------------ A11, A12

/// One client is registered and unregistered by its file, backed up first,
/// with every other server in that file kept; the Agents page says which
/// clients are registered and what each tool's answer typically costs.
#[test]
fn one_client_registers_and_unregisters_and_tools_say_their_size() {
    let (_dir, home) = home();
    let cursor = home.join(".cursor");
    std::fs::create_dir_all(&cursor).unwrap();
    let file = cursor.join("mcp.json");
    std::fs::write(&file, r#"{"mcpServers": {"other": {"command": "o"}}}"#).unwrap();

    let daemon = Daemon::start(&home);
    let agents = daemon.get("/api/agents");
    let tools = agents["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 16, "{agents}");
    for tool in tools {
        assert!(tool["typical_tokens"].as_i64().unwrap() > 0, "{tool}");
        assert_eq!(tool["typical_source"], "estimate");
        assert!(tool["answers"].as_str().is_some_and(|a| !a.is_empty()));
    }
    let row = |agents: &Value| {
        agents["clients"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "cursor")
            .cloned()
            .expect("cursor is listed with an id")
    };
    assert_eq!(row(&agents)["registered"], json!(false));

    let (status, body) = daemon.post(
        "/api/agents/register",
        json!({ "clients": ["cursor"], "action": "register" }),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["results"][0]["ok"], json!(true), "{body}");
    let written = std::fs::read_to_string(&file).unwrap();
    assert!(
        written.contains("\"semlith\"") && written.contains("\"other\""),
        "{written}"
    );
    assert_eq!(row(&daemon.get("/api/agents"))["registered"], json!(true));

    let (_, body) = daemon.post(
        "/api/agents/register",
        json!({ "clients": ["cursor"], "action": "unregister" }),
    );
    assert_eq!(body["results"][0]["ok"], json!(true), "{body}");
    let left = std::fs::read_to_string(&file).unwrap();
    assert!(
        !left.contains("semlith") && left.contains("\"other\""),
        "{left}"
    );
    assert!(
        cursor.join("mcp.json.semlith-backup").exists(),
        "no backup was made"
    );
    assert_eq!(row(&daemon.get("/api/agents"))["registered"], json!(false));

    let (status, body) = daemon.post(
        "/api/agents/register",
        json!({ "clients": ["nobody"], "action": "unregister" }),
    );
    assert_eq!(status, 200);
    assert_eq!(body["results"][0]["ok"], json!(false), "{body}");
}

/// The not-indexed table's rows carry the same scored fields.
#[test]
fn not_indexed_rows_carry_their_risk() {
    let (_dir, home) = home();
    {
        let daemon = Daemon::start(&home);
        daemon.post("/api/store/create", json!({ "name": "rows" }));
    }
    let dir = home.join("stores").join("rows");
    let text = format!("token = {}\n", semlith::keyscan::forge(3));
    {
        let store = semlith::Semlith::open(&dir, None).unwrap();
        semlith::store::read_only(store.db(), false).unwrap();
        semlith::store::refuse(
            store.db(),
            "/somewhere/a.rs",
            semlith::store::class::CONTENT,
            "holds what looks like a token",
            &semlith::keyscan::scan("/somewhere/a.rs", &text),
            1,
            1,
        )
        .unwrap();
    }
    let daemon = Daemon::start(&home);
    let refused = daemon.get("/api/refused");
    let row = &refused["stores"][0]["rows"][0];
    assert_eq!(row["band"], "high", "{refused}");
    assert_eq!(row["tone"], "red");
    assert_eq!(row["kind"], "Access token");
    assert!(row["evidence"].as_str().unwrap().contains("line 1"));
}
