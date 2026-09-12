//! `semlith start`: the daemon, its loopback server, and the three rules that
//! guard it.
//!
//! Requests are made over a raw `TcpStream` rather than through an HTTP client,
//! for two reasons. The release adds no dependency, so there is no client to
//! reach for; and half of what is under test here is what the server does with
//! a request a well-behaved client would never send — a missing token, a
//! `Host` header naming somebody else's domain — which a client would not let
//! a test construct.
//!
//! The tests that only need a server run offline. The ones that need a store
//! with vectors in it embed, so they download a model on first run:
//!
//! ```sh
//! cargo test --test daemon -- --ignored
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A running daemon and the sandbox it owns.
struct Daemon {
    child: Child,
    port: u16,
    token: String,
    home: PathBuf,
    _dir: tempfile::TempDir,
}

/// One HTTP response, as far as these tests care about it.
struct Answer {
    status: u16,
    headers: String,
    body: String,
}

impl Answer {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON: {e}\n{}", self.body))
    }
}

fn real_model_cache() -> PathBuf {
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
        .env("SEMLITH_MODEL_CACHE", real_model_cache())
        .env_remove("SEMLITH_STORE")
        .env_remove("SEMLITH_AIRGAP")
        .env_remove("SEMLITH_PORT");
    command
}

/// A sandbox with a home and a working tree, and nothing of the developer's.
fn sandbox(tag: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix(&format!("semlith-daemon-{tag}-"))
        .tempdir()
        .expect("a temporary directory");
    let home = dir.path().join("home");
    let work = dir.path().join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    (dir, home, work)
}

impl Daemon {
    /// Start a daemon on an ephemeral port and wait for the URL it prints.
    ///
    /// Port 0 rather than 7365: several of these run at once, and a test that
    /// fails because another test had the port would be a test about nothing.
    /// The default port itself is asserted separately.
    fn start(tag: &str, extra: &[&str]) -> Self {
        let (dir, home, work) = sandbox(tag);
        Self::start_in(dir, home, work, extra)
    }

    fn start_in(dir: tempfile::TempDir, home: PathBuf, work: PathBuf, extra: &[&str]) -> Self {
        let mut child = semlith(&home)
            .arg("start")
            .arg("--port")
            .arg("0")
            .args(extra)
            .current_dir(&work)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("semlith start runs");

        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().expect("stdout is piped"))
            .read_line(&mut line)
            .expect("the daemon prints its URL");
        let url = line.trim().to_string();

        let rest = url
            .strip_prefix("http://127.0.0.1:")
            .unwrap_or_else(|| panic!("the URL does not bind loopback: {url}"));
        let (port, query) = rest.split_once("/?token=").unwrap_or_else(|| {
            panic!("the URL does not carry a token in the documented shape: {url}")
        });

        Self {
            child,
            port: port.parse().expect("a port"),
            token: query.to_string(),
            home,
            _dir: dir,
        }
    }

    /// Send a request verbatim, so a test can say exactly what goes on the wire.
    fn raw(&self, request: &str) -> Answer {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("the daemon listens");
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().unwrap();

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (headers, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
        let status = headers
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        Answer {
            status,
            headers: headers.to_string(),
            body: body.to_string(),
        }
    }

    fn get(&self, path: &str) -> Answer {
        self.raw(&format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nCookie: semlith_token={}\r\nConnection: close\r\n\r\n",
            self.port, self.token
        ))
    }

    fn post(&self, path: &str, body: &str) -> Answer {
        self.raw(&format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nCookie: semlith_token={}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            self.token,
            body.len()
        ))
    }

    fn store_dir(&self, name: &str) -> PathBuf {
        self.home.join("stores").join(name)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Index a corpus into the sandbox home before the daemon starts, so the
/// daemon has something to hold the lock on.
fn corpus(home: &Path, work: &Path, name: &str, files: &[(&str, &str)]) {
    let root = work.join(name);
    std::fs::create_dir_all(&root).unwrap();
    for (file, text) in files {
        std::fs::write(root.join(file), text).unwrap();
    }
    let out = semlith(home)
        .arg("index")
        .arg(".")
        .arg("--quiet")
        .current_dir(&root)
        .output()
        .expect("index runs");
    assert!(
        out.status.success(),
        "indexing {name} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

const RUST: &str = "\
/// The one store a write goes to.
///
/// One writer per store is the product's rule, and with several stores open
/// there is no \"the\" store, so an unnamed write among several is refused.
pub fn writable(&mut self, only: &[String]) -> Result<&mut Semlith> {
    todo!()
}
";

/// Poll until `check` passes, so a test never depends on a fixed sleep.
fn until(what: &str, limit: Duration, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(120));
    }
    panic!("timed out waiting for {what}");
}

// ---------------------------------------------------------------- T04

/// The three rules, in the order the server applies them. A prober learns that
/// something refused it and nothing else — no body, no hint about what is here.
#[test]
fn the_token_and_the_host_check_guard_every_route() {
    let daemon = Daemon::start("guards", &[]);

    let no_token = daemon.raw(&format!(
        "GET /api/stores HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        daemon.port
    ));
    assert_eq!(no_token.status, 401);
    assert!(
        no_token.body.is_empty(),
        "401 leaked a body: {}",
        no_token.body
    );

    let wrong_token = daemon.raw(&format!(
        "GET /api/stores HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nCookie: semlith_token=not-it\r\nConnection: close\r\n\r\n",
        daemon.port
    ));
    assert_eq!(wrong_token.status, 401);

    // A page on another origin cannot read a cross-origin response, but it can
    // send the request — with this browser's cookies attached. The Host check
    // is what stops that reaching a route at all.
    let foreign = daemon.raw(&format!(
        "GET /api/stores HTTP/1.1\r\nHost: evil.example\r\nCookie: semlith_token={}\r\nConnection: close\r\n\r\n",
        daemon.token
    ));
    assert_eq!(foreign.status, 400);

    let allowed = daemon.raw(&format!(
        "GET /api/stores HTTP/1.1\r\nHost: localhost:{}\r\nCookie: semlith_token={}\r\nConnection: close\r\n\r\n",
        daemon.port, daemon.token
    ));
    assert_eq!(allowed.status, 200);
}

/// The URL the daemon prints is the one request allowed to carry the token in
/// the query, because it is the request that turns it into a cookie.
#[test]
fn the_printed_url_sets_the_cookie_and_nothing_else_needs_to() {
    let daemon = Daemon::start("cookie", &[]);

    let first = daemon.raw(&format!(
        "GET /?token={} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        daemon.token, daemon.port
    ));
    assert_eq!(first.status, 200);
    assert!(
        first.headers.contains("Set-Cookie: semlith_token=")
            && first.headers.contains("SameSite=Strict")
            && first.headers.contains("HttpOnly"),
        "the first request did not hand over a strict cookie:\n{}",
        first.headers
    );
}

/// Every response carries a policy that allows only `'self'`, and no CORS
/// header exists anywhere: there is no origin this server wants to be
/// readable from.
#[test]
fn every_response_carries_the_policy_and_no_cors_header() {
    let daemon = Daemon::start("csp", &[]);
    for route in ["/", "/style.css", "/app.js", "/api/stores"] {
        let answer = daemon.get(route);
        assert_eq!(answer.status, 200, "{route} answered {}", answer.status);
        assert!(
            answer
                .headers
                .contains("Content-Security-Policy: default-src 'self'"),
            "{route} has no policy:\n{}",
            answer.headers
        );
        assert!(
            !answer
                .headers
                .to_ascii_lowercase()
                .contains("access-control-"),
            "{route} emits a CORS header:\n{}",
            answer.headers
        );
    }
}

/// The page and everything it loads come out of the binary, so they are served
/// with no network at all — which is the same thing as saying the portal works
/// with the cable unplugged.
#[test]
fn the_portal_and_its_assets_are_served_from_the_binary() {
    let daemon = Daemon::start("assets", &[]);

    let page = daemon.get("/");
    assert!(page.headers.contains("Content-Type: text/html"));
    assert!(page.body.contains("app.js") && page.body.contains("style.css"));

    for asset in [
        "/style.css",
        "/app.js",
        "/fonts/IBMPlexSans-Regular.woff2",
        "/fonts/IBMPlexMono-Regular.woff2",
        "/fonts/LICENSE.txt",
    ] {
        let answer = daemon.get(asset);
        assert_eq!(answer.status, 200, "{asset} was not served");
        assert!(!answer.body.is_empty(), "{asset} is empty");
    }

    // Nothing in what is served may point at another origin, or the policy
    // above would block it on a machine with a network and the page would be
    // broken only for the user who has none.
    let css = daemon.get("/style.css").body;
    assert!(!css.contains("http://") && !css.contains("https://"));
}

// ---------------------------------------------------------------- T05

/// The port is a bookmark, so it is fixed, documented and never reassigned
/// behind the user's back.
#[test]
fn the_default_port_is_the_documented_one() {
    assert_eq!(semlith::http::DEFAULT_PORT, 7365);
}

/// A daemon that quietly moved to another port would break the bookmark, the
/// README and — from 0.11.0 — a client's MCP endpoint. So it exits, naming the
/// port and the flag that changes it.
#[test]
fn a_taken_port_is_an_error_naming_the_flag() {
    let (dir, home, work) = sandbox("taken");
    let squatter = std::net::TcpListener::bind("127.0.0.1:0").expect("an ephemeral port");
    let port = squatter.local_addr().unwrap().port();

    let out = semlith(&home)
        .arg("start")
        .arg("--port")
        .arg(port.to_string())
        .current_dir(&work)
        .output()
        .expect("semlith start runs");

    assert!(!out.status.success(), "a taken port must not be ignored");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains(&port.to_string()) && said.contains("--port"),
        "the error names neither the port nor the flag: {said}"
    );
    drop(dir);
}

/// With nothing registered the daemon still starts, because the portal's
/// welcome screen is exactly what a developer with no store needs to see.
#[test]
fn a_daemon_with_no_store_still_serves_the_portal() {
    let daemon = Daemon::start("empty", &[]);
    let stores = daemon.get("/api/stores").json();
    assert_eq!(stores["stores"].as_array().map(Vec::len), Some(0));
    assert_eq!(daemon.get("/").status, 200);
}

/// The daemon is the writer for its whole life, so a second writer is refused
/// — which is the conflict the daemon exists to make impossible rather than
/// merely unlikely.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn a_second_writer_is_refused_while_the_daemon_holds_the_store() {
    let (dir, home, work) = sandbox("second-writer");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);
    let daemon = Daemon::start_in(dir, home.clone(), work.join("api"), &[]);

    let out = semlith(&home)
        .arg("index")
        .arg(".")
        .arg("--quiet")
        .current_dir(work.join("api"))
        .output()
        .expect("index runs");

    assert!(!out.status.success(), "a second writer was allowed in");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("semlith start") || said.contains("daemon"),
        "the refusal does not say a daemon holds the store: {said}"
    );
    drop(daemon);
}

/// Shutdown has an order, and every part of it is observable: the lock comes
/// back, and the discovery file goes away so nothing is pointed at a daemon
/// that has stopped answering.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn a_signal_releases_the_lock_and_removes_the_discovery_file() {
    let (dir, home, work) = sandbox("signal");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);
    let mut daemon = Daemon::start_in(dir, home.clone(), work.join("api"), &[]);

    let discovery = daemon.store_dir("api").join("daemon.json");
    until(
        "the discovery file to appear",
        Duration::from_secs(10),
        || discovery.exists(),
    );

    let pid = daemon.child.id();
    assert!(
        Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .expect("kill runs")
            .success()
    );
    let _ = daemon.child.wait();

    until("the discovery file to go", Duration::from_secs(20), || {
        !discovery.exists()
    });

    // The lock is the real proof: an index that succeeds is an index that got
    // the writer's lock back.
    let out = semlith(&home)
        .arg("index")
        .arg(".")
        .arg("--quiet")
        .current_dir(work.join("api"))
        .output()
        .expect("index runs");
    assert!(
        out.status.success(),
        "the lock was not released: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ---------------------------------------------------------------- T06

/// The portal is a view of the same corpus the terminal sees. If the two ever
/// answer one query differently, one of them is lying about what is indexed.
#[test]
#[ignore = "searches, so it downloads an embedding model on first run"]
fn the_portal_and_the_cli_land_on_the_same_chunk() {
    let (dir, home, work) = sandbox("search");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);
    let daemon = Daemon::start_in(dir, home.clone(), work.join("api"), &[]);

    let answer = daemon
        .get("/api/search?query=Fleet%3A%3Awritable&k=3")
        .json();
    let hits = answer["hits"].as_array().expect("hits").clone();
    assert!(!hits.is_empty(), "the portal found nothing: {answer}");

    let top = hits
        .iter()
        .take(3)
        .any(|h| h["text"].as_str().unwrap_or("").contains("pub fn writable"));
    assert!(top, "the defining chunk is not in the top three: {answer}");

    let out = semlith(&home)
        .arg("search")
        .arg("Fleet::writable")
        .arg("--k")
        .arg("3")
        .arg("--json")
        .current_dir(work.join("api"))
        .output()
        .expect("search runs");
    let cli: serde_json::Value = serde_json::from_slice(&out.stdout).expect("the CLI prints JSON");

    assert_eq!(
        cli[0]["path"], hits[0]["path"],
        "the CLI and the portal disagree about the top hit"
    );
    assert_eq!(cli[0]["start_line"], hits[0]["start_line"]);
}

/// The Files view answers with the filters the CLI has, and says which reader
/// turned each file into text — because "read as binary and skipped" and
/// "read and empty" look identical in a file list and are different problems.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn the_files_route_reports_the_reader_and_honours_the_filters() {
    let (dir, home, work) = sandbox("files");
    corpus(
        &home,
        &work,
        "api",
        &[("fleet.rs", RUST), ("notes.md", "# Notes\n\nOwnership.\n")],
    );
    let daemon = Daemon::start_in(dir, home, work.join("api"), &[]);

    let all = daemon.get("/api/files").json();
    assert_eq!(all["total"], 2);

    let rust = daemon.get("/api/files?lang=rust").json();
    let files = rust["files"].as_array().expect("files");
    assert_eq!(files.len(), 1, "the lang filter did not narrow: {rust}");
    assert!(files[0]["path"].as_str().unwrap().ends_with("fleet.rs"));
    assert_eq!(files[0]["reader"], "text");
    assert_eq!(files[0]["lang"], "rust");
    assert!(files[0]["chunks"].as_i64().unwrap() > 0);

    // A filter that selects nothing is a different answer from a corpus that
    // does not contain the thing, and the route must not conflate them.
    let none = daemon.get("/api/files?lang=haskell").json();
    assert_eq!(none["files"].as_array().map(Vec::len), Some(0));
}

// ---------------------------------------------------------------- T07

/// The index route streams, so a browser sees a run while it is running. A
/// response that arrived whole at the end would be a progress bar that only
/// ever shows 100%.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn indexing_from_the_portal_streams_progress_and_then_lists_the_files() {
    let (dir, home, work) = sandbox("index-route");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);
    let extra = work.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    std::fs::write(extra.join("lock.rs"), "pub struct StoreLock;\n").unwrap();

    let daemon = Daemon::start_in(dir, home, work.join("api"), &[]);

    let body = format!(
        "{{\"path\":{}}}",
        serde_json::to_string(&extra.display().to_string()).unwrap()
    );
    let answer = daemon.post("/api/index", &body);
    assert_eq!(answer.status, 200);
    assert!(
        answer.headers.contains("Transfer-Encoding: chunked"),
        "the index route did not stream:\n{}",
        answer.headers
    );

    let events: Vec<serde_json::Value> = answer
        .body
        .lines()
        .filter(|l| l.trim_start().starts_with('{'))
        .map(|l| serde_json::from_str(l.trim()).expect("each line is one JSON object"))
        .collect();
    assert!(
        events.iter().any(|e| e["event"] == "file"),
        "no progress event arrived before the run completed: {events:?}"
    );
    let done = events
        .iter()
        .find(|e| e["event"] == "done")
        .unwrap_or_else(|| panic!("the run never reported done: {events:?}"));
    assert_eq!(done["indexed"], 1);

    let files = daemon.get("/api/files").json();
    assert_eq!(files["total"], 2, "the new file is not listed: {files}");

    // And forgetting it takes it back out, of the view and of the search.
    let forget = daemon.post(
        "/api/forget",
        &format!(
            "{{\"path\":{}}}",
            serde_json::to_string(&extra.join("lock.rs").display().to_string()).unwrap()
        ),
    );
    assert_eq!(forget.status, 200);
    assert!(forget.json()["forgot"].as_i64().unwrap() > 0);
    assert_eq!(daemon.get("/api/files").json()["total"], 1);
}

// ---------------------------------------------------------------- T08

/// The watcher is the daemon's reason to exist: a save is searchable without
/// anyone running anything.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn a_save_changes_the_file_count_with_no_manual_action() {
    let (dir, home, work) = sandbox("watch");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);
    let root = work.join("api");
    let daemon = Daemon::start_in(dir, home, root.clone(), &[]);

    // Wait for the watcher to say it is watching, or the write below races the
    // catch-up pass and the test measures nothing.
    until("the watcher to be ready", Duration::from_secs(30), || {
        daemon.get("/api/stores").json()["stores"][0]["watching"] == serde_json::json!(true)
    });

    let before = daemon.get("/api/files").json()["files"][0]["lines"]
        .as_i64()
        .expect("a line count");

    std::fs::write(
        root.join("fleet.rs"),
        format!("{RUST}\n// A paragraph added while the daemon was running.\n"),
    )
    .unwrap();

    until(
        "the watcher to re-embed the save",
        Duration::from_secs(40),
        || {
            daemon.get("/api/files").json()["files"][0]["lines"]
                .as_i64()
                .is_some_and(|lines| lines > before)
        },
    );
}

// ---------------------------------------------------------------- T10

/// The Privacy page's Rotate button has to actually invalidate: a token that
/// still worked after rotating would make the button worse than nothing.
#[test]
fn rotating_the_token_invalidates_the_old_one_immediately() {
    let daemon = Daemon::start("rotate", &[]);

    let rotated = daemon.post("/api/rotate", "{}");
    assert_eq!(rotated.status, 200);
    let fresh = rotated.json()["token"]
        .as_str()
        .expect("a new token")
        .to_string();
    assert_ne!(fresh, daemon.token);

    // The old one, which this Daemon still holds, must now be refused.
    assert_eq!(daemon.get("/api/stores").status, 401);

    let with_new = daemon.raw(&format!(
        "GET /api/stores HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nCookie: semlith_token={fresh}\r\nConnection: close\r\n\r\n",
        daemon.port
    ));
    assert_eq!(with_new.status, 200);
}

/// An air-gapped machine's claim is that this process never reached the
/// network. With no model cached it has to refuse rather than try — and say
/// where to put the weights.
#[test]
fn airgap_refuses_to_fetch_and_names_the_cache() {
    let (dir, home, work) = sandbox("airgap");
    let empty = dir.path().join("no-models");
    std::fs::create_dir_all(&empty).unwrap();
    std::fs::write(work.join("a.md"), "Ownership.\n").unwrap();

    let out = semlith(&home)
        .env("SEMLITH_MODEL_CACHE", &empty)
        .arg("index")
        .arg(".")
        .arg("--airgap")
        .current_dir(&work)
        .output()
        .expect("index runs");

    assert!(!out.status.success(), "airgap let a download through");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains(empty.to_str().unwrap()),
        "the error does not name the cache path: {said}"
    );
}

/// The Privacy route is the page's source of facts, so the facts have to be
/// read off the running process rather than written into the page.
#[test]
fn the_privacy_route_reports_what_this_process_actually_does() {
    let daemon = Daemon::start("privacy", &["--airgap"]);
    let privacy = daemon.get("/api/privacy").json();

    assert_eq!(privacy["bind"], format!("127.0.0.1:{}", daemon.port));
    assert_eq!(privacy["cors"], serde_json::json!(false));
    assert_eq!(privacy["airgap"], serde_json::json!(true));
    assert_eq!(privacy["csp"], "default-src 'self'");
    assert_eq!(privacy["token_cookie"], "semlith_token");
}

// ---------------------------------------------------------------- T11

/// The Agents view's stanzas come from the README, which is the same text
/// `tests/clients.rs` executes. A stanza retyped anywhere else would be right
/// until somebody edited one of the two copies.
#[test]
fn the_agents_route_serves_the_readme_stanzas_verbatim() {
    let daemon = Daemon::start("agents", &[]);
    let agents = daemon.get("/api/agents").json();

    let clients = agents["clients"].as_array().expect("clients");
    assert!(clients.len() >= 12, "only {} clients", clients.len());
    assert_eq!(clients[0]["name"], "Claude Code");

    let stanza = clients[0]["stanzas"][0]["text"].as_str().expect("a stanza");
    assert_eq!(stanza.trim(), "claude mcp add semlith -- semlith mcp");

    let readme = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
        .expect("the README is beside the crate");
    assert!(
        readme.contains(stanza.trim()),
        "the route serves a stanza the README does not contain: {stanza}"
    );

    for client in clients {
        for stanza in client["stanzas"].as_array().expect("stanzas") {
            assert!(
                !stanza["text"].as_str().unwrap_or("").contains("--store"),
                "{} still carries a store path",
                client["name"]
            );
        }
    }
}
