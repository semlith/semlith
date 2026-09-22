//! One store whose database cannot be read must not blank every page that
//! reads across stores (#129).
//!
//! The damage is real: `store.db` is truncated the way a killed index run or an
//! unplugged drive leaves it, and the store is registered beside a healthy one.
//! What is asserted is the decision the routes now take — answer from every
//! store that can be read, and say which could not, with its path and its
//! remedy — and the one case that still fails, because there is no partial
//! answer to give: a request scoped to nothing but the damaged store.
//!
//! These index real text, so they embed and download a model on first run:
//!
//! ```sh
//! cargo test --test unreadable_store -- --ignored
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Two files with no vocabulary in common, so "which store answered" is
/// decidable from a hit's text alone.
const HEALTHY: &str = "\
/// Sourdough rises because a starter of flour and water ferments.
fn hydration(flour: f32, water: f32) -> f32 {
    water / flour
}

fn crumb() -> f32 {
    hydration(1000.0, 800.0)
}
";

const DAMAGED: &str = "\
/// The borrow checker proves that no value has two mutable aliases.
fn ownership() -> u8 {
    7
}
";

struct Daemon {
    child: Child,
    port: u16,
    token: String,
    _dir: tempfile::TempDir,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON: {e}\n{}", self.body))
    }
}

fn model_cache() -> PathBuf {
    if let Ok(dir) = std::env::var("SEMLITH_MODEL_CACHE") {
        return PathBuf::from(dir);
    }
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
        .join(".cache")
        .join("semlith")
        .join("models")
}

fn semlith(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_semlith"));
    command
        .env("SEMLITH_HOME", home)
        .env("HOME", home)
        .env("SEMLITH_MODEL_CACHE", model_cache())
        .env_remove("SEMLITH_STORE")
        .env_remove("SEMLITH_AIRGAP")
        .env_remove("SEMLITH_PORT");
    command
}

impl Daemon {
    fn start(home: &Path, work: &Path, dir: tempfile::TempDir) -> Self {
        let mut child = semlith(home)
            .arg("start")
            .arg("--port")
            .arg("0")
            .current_dir(work)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("semlith start runs");

        // The banner, not a sleep: the daemon prints its URL when it is
        // listening, and how long it takes to get there grows with the binary.
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().expect("stdout is piped"))
            .read_line(&mut line)
            .expect("the daemon prints its URL");
        let url = line.trim().to_string();
        let rest = url
            .strip_prefix("http://127.0.0.1:")
            .unwrap_or_else(|| panic!("the URL does not bind loopback: {url}"));
        let (port, token) = rest
            .split_once("/?token=")
            .unwrap_or_else(|| panic!("no token in the URL: {url}"));

        Self {
            child,
            port: port.parse().expect("a port"),
            token: token.to_string(),
            _dir: dir,
        }
    }

    fn get(&self, path: &str) -> Answer {
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\n\
             Connection: close\r\n\r\n",
            self.port, self.token
        );
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("the daemon listens");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().unwrap();

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (headers, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
        Answer {
            status: headers
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            body: body.to_string(),
        }
    }
}

/// Index `name` into the store home and hand back the store directory.
fn store(home: &Path, work: &Path, name: &str, source: &str) -> PathBuf {
    let corpus = work.join(name);
    std::fs::create_dir_all(&corpus).unwrap();
    std::fs::write(corpus.join("lib.rs"), source).unwrap();
    let out = semlith(home)
        .arg("index")
        .arg(&corpus)
        .arg("--quiet")
        .output()
        .expect("semlith index runs");
    assert!(
        out.status.success(),
        "indexing {name} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dir = home.join("stores").join(name);
    assert!(
        dir.join("store.db").is_file(),
        "no store at {}",
        dir.display()
    );
    dir
}

/// The failure entry for `name`, or a panic saying what the route did answer.
fn failure<'a>(answer: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    let failed = answer["failed"]
        .as_array()
        .unwrap_or_else(|| panic!("no `failed` list beside the answer: {answer}"));
    failed
        .iter()
        .find(|f| f["store"] == name)
        .unwrap_or_else(|| panic!("`failed` does not name {name}: {answer}"))
}

/// The whole of #129, over three routes that aggregate across stores.
#[test]
#[ignore = "indexes two stores, which downloads an embedding model on first run"]
fn a_store_that_cannot_be_read_is_reported_beside_the_stores_that_answered() {
    let dir = tempfile::Builder::new()
        .prefix("semlith-unreadable-")
        .tempdir()
        .unwrap();
    let home = dir.path().join("home");
    let work = dir.path().join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&work).unwrap();

    let _healthy = store(&home, &work, "healthy", HEALTHY);
    let broken = store(&home, &work, "damaged", DAMAGED);

    // The damage, and nothing else: the file stays where it was, with a
    // plausible prefix of itself, which is what a killed index run leaves.
    let db = broken.join("store.db");
    let held = std::fs::OpenOptions::new().write(true).open(&db).unwrap();
    held.set_len(512).unwrap();
    drop(held);
    for beside in ["store.db-wal", "store.db-shm"] {
        let _ = std::fs::remove_file(broken.join(beside));
    }

    let daemon = Daemon::start(&home, &work, dir);
    // The path the store is opened by, spelled the way it leaves the daemon:
    // canonical, and with the Windows verbatim prefix stripped as `plain` does.
    let path_of_damaged = std::fs::canonicalize(&broken)
        .unwrap()
        .display()
        .to_string()
        .trim_start_matches(r"\\?\")
        .to_string();

    // ------------------------------------------------------------ /api/graph
    let answer = daemon.get("/api/graph");
    assert_eq!(
        answer.status, 200,
        "one unreadable store must not blank the Graph page: {}",
        answer.body
    );
    let graph = answer.json();
    let failed = failure(&graph, "damaged");
    assert_eq!(failed["path"], path_of_damaged, "{graph}");
    assert!(
        failed["remedy"]
            .as_str()
            .is_some_and(|r| r.contains("semlith drop damaged")),
        "the remedy has to be runnable: {graph}"
    );
    assert!(
        failed["error"]
            .as_str()
            .is_some_and(|e| !e.trim().is_empty()),
        "the reason has to survive into the answer: {graph}"
    );
    assert!(
        graph["nodes"].as_array().is_some_and(|n| !n.is_empty()),
        "the healthy store still has to draw: {graph}"
    );

    // ------------------------------------------------------------ /api/files
    let answer = daemon.get("/api/files");
    assert_eq!(answer.status, 200, "{}", answer.body);
    let files = answer.json();
    assert_eq!(
        failure(&files, "damaged")["path"],
        path_of_damaged,
        "{files}"
    );
    let rows = files["files"].as_array().unwrap();
    assert!(
        !rows.is_empty() && rows.iter().all(|r| r["store"] == "healthy"),
        "the listing has to be the healthy store's files: {files}"
    );

    // ----------------------------------------------------------- /api/search
    let answer = daemon.get("/api/search?query=hydration");
    assert_eq!(answer.status, 200, "{}", answer.body);
    let search = answer.json();
    assert_eq!(
        failure(&search, "damaged")["path"],
        path_of_damaged,
        "{search}"
    );
    assert!(
        search["hits"].as_array().is_some_and(|h| !h.is_empty()),
        "the healthy store still has to answer: {search}"
    );

    // ----------------------------------------------------------- /api/stores
    let answer = daemon.get("/api/stores");
    assert_eq!(answer.status, 200, "{}", answer.body);
    let stores = answer.json();
    let rows = stores["stores"].as_array().unwrap();
    let damaged_row = rows
        .iter()
        .find(|s| s["name"] == "damaged")
        .unwrap_or_else(|| panic!("the damaged store is not listed: {stores}"));
    assert_eq!(damaged_row["unreadable"], true, "{stores}");
    let healthy_row = rows.iter().find(|s| s["name"] == "healthy").unwrap();
    assert_eq!(healthy_row["unreadable"], false, "{stores}");
    assert_eq!(failure(&stores, "damaged")["store"], "damaged", "{stores}");

    // ------------------------- scoped to the damaged store alone, which fails
    for route in [
        "/api/graph?store=damaged",
        "/api/files?store=damaged",
        "/api/search?query=hydration&store=damaged",
    ] {
        let answer = daemon.get(route);
        assert_eq!(
            answer.status, 500,
            "there is no partial answer to give for {route}: {}",
            answer.body
        );
        let said = answer.body.clone();
        assert!(
            said.contains("damaged"),
            "the refusal has to name the store — {route}: {said}"
        );
        assert!(
            // A backslash is escaped on its way into JSON and a forward slash
            // is not, so the same path is two strings across the two platforms.
            said.contains(&path_of_damaged)
                || said.contains(&path_of_damaged.replace('\\', "\\\\")),
            "the refusal has to name the path — {route}: {said}"
        );
        assert!(
            said.contains("semlith drop damaged"),
            "the refusal has to name the remedy — {route}: {said}"
        );
    }

    // And the healthy store, scoped, is untouched by any of it.
    let answer = daemon.get("/api/graph?store=healthy");
    assert_eq!(answer.status, 200, "{}", answer.body);
}

/// The issue's own repro: the store was fine when the daemon opened it and goes
/// unreadable underneath a running server.
///
/// The other test damages the store before the daemon starts, so the fleet
/// never opens it. This is the other half — the connection is already open and
/// the failure arrives on a read — and it is the half #129 was reported from,
/// because `?store=healthy` answered while the unscoped route did not.
#[test]
#[ignore = "indexes two stores, which downloads an embedding model on first run"]
fn a_store_that_goes_unreadable_under_a_running_daemon_is_dropped_from_the_answer() {
    let dir = tempfile::Builder::new()
        .prefix("semlith-unreadable-live-")
        .tempdir()
        .unwrap();
    let home = dir.path().join("home");
    let work = dir.path().join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&work).unwrap();

    store(&home, &work, "healthy", HEALTHY);
    let broken = store(&home, &work, "damaged", DAMAGED);

    let daemon = Daemon::start(&home, &work, dir);
    // Both stores answer first, so the fleet is open and holding a connection
    // to the file that is about to be truncated.
    let before = daemon.get("/api/files");
    assert_eq!(before.status, 200, "{}", before.body);
    assert!(
        before.json()["failed"].is_null(),
        "nothing has failed yet: {}",
        before.body
    );

    let held = std::fs::OpenOptions::new()
        .write(true)
        .open(broken.join("store.db"))
        .unwrap();
    held.set_len(512).unwrap();
    drop(held);

    let answer = daemon.get("/api/files");
    assert_eq!(
        answer.status, 200,
        "the listing must survive a store going unreadable: {}",
        answer.body
    );
    let files = answer.json();
    assert_eq!(failure(&files, "damaged")["store"], "damaged", "{files}");
    assert!(
        files["files"]
            .as_array()
            .is_some_and(|r| !r.is_empty() && r.iter().all(|f| f["store"] == "healthy")),
        "the healthy store still lists its files: {files}"
    );

    // Scoped to the damaged store there is still no partial answer to give.
    let answer = daemon.get("/api/files?store=damaged");
    assert_eq!(answer.status, 500, "{}", answer.body);
    assert!(
        answer.body.contains("semlith drop damaged"),
        "the refusal has to name the remedy: {}",
        answer.body
    );
    // Scoped to the healthy one it answers, exactly as the issue reported.
    assert_eq!(daemon.get("/api/files?store=healthy").status, 200);
}
