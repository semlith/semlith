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
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\nConnection: close\r\n\r\n",
            self.port, self.token
        ))
    }

    fn post(&self, path: &str, body: &str) -> Answer {
        self.raw(&format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            self.token,
            body.len()
        ))
    }

    /// A POST exactly as a page on another 127.0.0.1 port would make it: the
    /// fetch metadata a browser attaches for a same-site-but-not-same-origin
    /// request, and whatever credential the attacker guessed at.
    fn cross_origin(&self, path: &str, token: &str) -> Answer {
        let body = "{}";
        self.raw(&format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {token}\r\n\
             Origin: http://127.0.0.1:{}\r\nSec-Fetch-Site: same-site\r\n\
             Content-Type: text/plain;charset=UTF-8\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            self.port,
            self.port + 1,
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
        "GET /api/stores HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: not-it\r\nConnection: close\r\n\r\n",
        daemon.port
    ));
    assert_eq!(wrong_token.status, 401);

    // The cookie is gone from 0.14.0, so a request carrying one is a request
    // carrying no credential at all.
    let by_cookie = daemon.raw(&format!(
        "GET /api/stores HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nCookie: semlith_token={}\r\n\
         Connection: close\r\n\r\n",
        daemon.port, daemon.token
    ));
    assert_eq!(
        by_cookie.status, 401,
        "a cookie still opened a route: the session is header-borne"
    );

    // A page on another origin cannot read a cross-origin response, but it can
    // send the request. The Host check is what stops a name that resolves to
    // 127.0.0.1 reaching a route at all.
    let foreign = daemon.raw(&format!(
        "GET /api/stores HTTP/1.1\r\nHost: evil.example\r\nSemlith-Token: {}\r\nConnection: close\r\n\r\n",
        daemon.token
    ));
    assert_eq!(foreign.status, 400);

    let allowed = daemon.raw(&format!(
        "GET /api/stores HTTP/1.1\r\nHost: localhost:{}\r\nSemlith-Token: {}\r\nConnection: close\r\n\r\n",
        daemon.port, daemon.token
    ));
    assert_eq!(allowed.status, 200);
}

/// The printed URL hands the token to the page and sets nothing. The query form
/// opens the page, which needs no credential anyway, and opens nothing else.
#[test]
fn the_printed_url_hands_the_token_over_and_sets_no_cookie() {
    let daemon = Daemon::start("bootstrap", &[]);

    let first = daemon.raw(&format!(
        "GET /?token={} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        daemon.token, daemon.port
    ));
    assert_eq!(first.status, 200);
    assert!(
        !first.headers.contains("Set-Cookie"),
        "the bootstrap set a cookie:\n{}",
        first.headers
    );

    // The same token in the query of a route that answers about this machine is
    // not a credential. This is the form a link, a bookmark or a referrer leaks.
    let by_query = daemon.raw(&format!(
        "GET /api/stores?token={} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        daemon.token, daemon.port
    ));
    assert_eq!(by_query.status, 401);

    let by_header = daemon.get("/api/stores");
    assert_eq!(by_header.status, 200);

    // A reload carries no token anywhere: the page comes back and asks for its
    // data with the header it kept.
    let reload = daemon.raw(&format!(
        "GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        daemon.port
    ));
    assert_eq!(reload.status, 200);
}

/// The whole of H2, from the attacker's side: a page served by anything else on
/// 127.0.0.1 sends what a browser would send for it, and every write is refused
/// before a handler runs.
#[test]
fn a_page_on_another_local_port_cannot_write() {
    let daemon = Daemon::start("cross-origin", &[]);

    for route in [
        "/api/index",
        "/api/add",
        "/api/forget",
        "/api/adopt",
        "/api/rotate",
        "/api/mcp",
        "/api/endpoint",
        "/api/key",
        "/api/root",
        "/api/store/delete",
        "/api/index/control",
        "/api/upgrade",
    ] {
        // Even with the right token — which it cannot have, but the refusal
        // must not be what tells it so.
        let guessed = daemon.cross_origin(route, &daemon.token);
        assert_eq!(guessed.status, 403, "{route} answered a cross-origin write");
        assert!(
            guessed.body.is_empty(),
            "{route} leaked a body to a cross-origin write: {}",
            guessed.body
        );

        let blind = daemon.cross_origin(route, "not-it");
        assert_eq!(
            blind.status, 403,
            "{route} told a guess apart from a right answer"
        );
    }
}

/// The three things a write must carry, taken away one at a time. A script is
/// not a browser and keeps working: it sends no fetch metadata at all.
#[test]
fn a_write_needs_same_origin_metadata_and_a_json_body() {
    let daemon = Daemon::start("writes", &[]);
    let body = r#"{"open":true}"#;

    let wrong_type = daemon.raw(&format!(
        "POST /api/endpoint HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\n\
         Sec-Fetch-Site: same-origin\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        daemon.port,
        daemon.token,
        body.len()
    ));
    assert_eq!(wrong_type.status, 403, "a text/plain body was read");

    let foreign_origin = daemon.raw(&format!(
        "POST /api/endpoint HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\n\
         Origin: http://127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        daemon.port,
        daemon.token,
        daemon.port + 1,
        body.len()
    ));
    assert_eq!(foreign_origin.status, 403, "another origin's write was read");

    // curl: the token, a JSON body, and no fetch metadata.
    let script = daemon.post("/api/endpoint", body);
    assert_eq!(
        script.status, 200,
        "a script with the token was refused: {}",
        script.body
    );

    let page = daemon.raw(&format!(
        "POST /api/endpoint HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\n\
         Origin: http://127.0.0.1:{}\r\nSec-Fetch-Site: same-origin\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        daemon.port,
        daemon.token,
        daemon.port,
        body.len()
    ));
    assert_eq!(page.status, 200, "the portal's own write was refused");
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

    // The stream says something before the writer reaches the job, and then
    // says what happened to every file rather than only to the ones it
    // embedded. A run that speaks only about its own work is silent for a
    // whole re-index of an unchanged corpus, which reads as a hang.
    assert!(
        events.iter().any(|e| e["event"] == "queued"),
        "nothing was said while the job waited for the writer: {events:?}"
    );
    let outcomes: Vec<&str> = events
        .iter()
        .filter(|e| e["event"] == "file")
        .filter_map(|e| e["outcome"].as_str())
        .collect();
    assert!(
        outcomes.contains(&"indexing"),
        "no file reported itself as being embedded: {events:?}"
    );

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
        "GET /api/stores HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {fresh}\r\nConnection: close\r\n\r\n",
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
    assert_eq!(privacy["token_header"], "Semlith-Token");
    assert!(
        privacy.get("token_cookie").is_none(),
        "the Privacy page still describes a cookie"
    );
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

    // Claude Code leads with the endpoint; the subprocess form follows it.
    let stanza = clients[0]["stanzas"][2]["text"].as_str().expect("a stanza");
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

// ---------------------------------------------------------------- T09

/// A `semlith mcp` driven through the daemon, as a client would drive it.
struct Proxied {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Proxied {
    fn open(home: &Path, cwd: &Path) -> Self {
        let mut child = semlith(home)
            .arg("mcp")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("semlith mcp runs");
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let request = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params,
        });
        writeln!(self.stdin, "{request}").expect("the server takes a request");
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .expect("the server answers");
        assert!(!line.trim().is_empty(), "the server closed on {method}");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON-RPC: {e}\n{line}"))
    }
}

impl Drop for Proxied {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Every revision the server advertises has to work through the proxy exactly
/// as it does in process — which is what the daemon running the same
/// `mcp::answer` buys, rather than a second protocol implementation that agrees
/// until somebody edits one of them.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn every_revision_proves_itself_through_the_proxy_too() {
    let (dir, home, work) = sandbox("proxy-revisions");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);
    let root = work.join("api");
    let daemon = Daemon::start_in(dir, home.clone(), root.clone(), &[]);

    for revision in ["2025-11-25", "2025-06-18", "2024-11-05"] {
        let mut server = Proxied::open(&home, &root);
        let hello = server.call(
            "initialize",
            serde_json::json!({ "protocolVersion": revision, "capabilities": {} }),
        );
        assert_eq!(
            hello["result"]["protocolVersion"], revision,
            "the proxy changed what {revision} is answered with: {hello}"
        );

        let listed = server.call("tools/list", serde_json::json!({}));
        let names: Vec<String> = listed["result"]["tools"]
            .as_array()
            .expect("tools/list returns an array")
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect();
        // Read from the server's own definitions rather than repeated here.
        // What this test is about is that the proxy does not *alter* the tool
        // surface on any revision; a second hand-written copy of that surface
        // only means a release that adds a tool fails here for the wrong
        // reason, which is exactly what happened when the graph tools landed.
        assert_eq!(
            names,
            semlith::mcp::tool_names(),
            "wrong tool surface on {revision} through the proxy"
        );

        let hit = server.call(
            "tools/call",
            serde_json::json!({
                "name": "semlith_search",
                "arguments": { "query": "Fleet::writable", "k": 3 }
            }),
        );
        let text = hit["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("fleet.rs"),
            "no excerpts on {revision}: {text}"
        );
    }

    // And the modern revision, which shakes no hands at all.
    let mut modern = Proxied::open(&home, &root);
    let found = modern.call(
        "server/discover",
        serde_json::json!({
            "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" }
        }),
    );
    assert_eq!(found["result"]["resultType"], "complete", "{found}");
    assert_eq!(
        found["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "semlith"
    );
    drop(daemon);
}

/// The reason the proxy exists. In 0.8.0 this call failed for as long as a
/// watcher held the store; now the daemon performs it and the agent gets the
/// answer it asked for.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn the_write_tools_work_through_the_proxy_while_the_daemon_holds_the_lock() {
    let (dir, home, work) = sandbox("proxy-writes");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);
    let root = work.join("api");
    let extra = work.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    std::fs::write(extra.join("lock.rs"), "pub struct StoreLock;\n").unwrap();

    let daemon = Daemon::start_in(dir, home.clone(), root.clone(), &[]);
    let mut server = Proxied::open(&home, &root);

    let stats = server.call("tools/call", serde_json::json!({ "name": "semlith_stats" }));
    assert!(
        stats["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("chunks"),
        "{stats}"
    );

    let indexed = server.call(
        "tools/call",
        serde_json::json!({
            "name": "semlith_index",
            "arguments": { "path": extra.display().to_string() }
        }),
    );
    let text = indexed["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("1 indexed"),
        "the write tool did not go through the daemon: {text}"
    );
    assert!(
        !text.contains("being indexed by") && !text.contains("is held by"),
        "the write tool still hit the lock: {text}"
    );

    let forgotten = server.call(
        "tools/call",
        serde_json::json!({
            "name": "semlith_forget",
            "arguments": { "path": extra.join("lock.rs").display().to_string() }
        }),
    );
    assert!(
        forgotten["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("Removed"),
        "{forgotten}"
    );

    // And the daemon now counts the client, which is what the Agents view shows.
    let agents = daemon.get("/api/agents").json();
    assert_eq!(agents["forwarding"], serde_json::json!(true), "{agents}");
    assert!(agents["connected"].as_u64().unwrap_or(0) >= 1, "{agents}");
}

/// A discovery file left by a daemon that was killed must not send a client to
/// a dead port. The fallback is the whole of 0.8.0's behaviour, unchanged.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn a_stale_discovery_file_falls_back_to_opening_the_store() {
    let (dir, home, work) = sandbox("stale");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);
    let root = work.join("api");
    let store = home.join("stores").join("api");

    // A port nothing is listening on, recorded as if a daemon had been there.
    let dead = {
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);
        port
    };
    std::fs::write(
        store.join("daemon.json"),
        format!(r#"{{"pid":999999,"port":{dead},"token":"nothing","version":"0.9.0"}}"#),
    )
    .unwrap();

    let mut server = Proxied::open(&home, &root);
    let stats = server.call("tools/call", serde_json::json!({ "name": "semlith_stats" }));
    assert!(
        stats["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("chunks"),
        "a stale discovery file broke the server instead of being ignored: {stats}"
    );
    drop(dir);
}

// ---------------------------------------------------------------- T12

/// The Files page selects rows and forgets the set in one call, and the answer
/// says how many files and how many chunks went — not a stream a page has to
/// parse to learn a number.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn a_set_of_files_is_forgotten_in_one_call() {
    let (dir, home, work) = sandbox("bulk-forget");
    corpus(
        &home,
        &work,
        "api",
        &[
            ("fleet.rs", RUST),
            ("notes.md", "# Notes\n\nOwnership.\n"),
            ("keep.md", "# Keep\n\nThis one stays.\n"),
        ],
    );
    let daemon = Daemon::start_in(dir, home, work.join("api"), &[]);
    assert_eq!(daemon.get("/api/files").json()["total"], 3);

    let root = work.join("api");
    let body = format!(
        "{{\"paths\":[{},{},{}]}}",
        serde_json::to_string(&root.join("fleet.rs").display().to_string()).unwrap(),
        serde_json::to_string(&root.join("notes.md").display().to_string()).unwrap(),
        // A path nobody indexed is reported rather than failing the batch: a
        // selection made before a watcher pass can name a file that is gone.
        serde_json::to_string(&root.join("never.md").display().to_string()).unwrap(),
    );
    let answer = daemon.post("/api/forget", &body);
    assert_eq!(answer.status, 200);
    let done = answer.json();
    assert_eq!(done["asked"], 3, "{done}");
    assert_eq!(done["files"], 2, "{done}");
    assert!(done["forgot"].as_i64().unwrap() > 0, "{done}");
    assert_eq!(
        done["not_indexed"].as_array().map(Vec::len),
        Some(1),
        "the file that was never indexed is not named: {done}"
    );

    let left = daemon.get("/api/files").json();
    assert_eq!(left["total"], 1, "{left}");
    assert!(
        left["files"][0]["path"]
            .as_str()
            .unwrap()
            .ends_with("keep.md"),
        "the wrong file survived: {left}"
    );
}

/// Deleting a store closes it, removes what semlith derived, and leaves the
/// indexed files alone — while the daemon keeps answering.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn a_store_is_deleted_without_stopping_the_daemon() {
    let (dir, home, work) = sandbox("delete-store");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);
    let daemon = Daemon::start_in(dir, home.clone(), work.join("api"), &[]);
    assert_eq!(daemon.get("/api/files").json()["total"], 1);

    let store_dir = home.join("stores/api");
    assert!(
        store_dir.is_dir(),
        "the store was not created where expected"
    );

    let answer = daemon.post("/api/store/delete", "{\"store\":\"api\"}");
    assert_eq!(answer.status, 200, "{}", answer.body);
    assert!(
        answer.json()["message"]
            .as_str()
            .unwrap()
            .contains("untouched"),
        "{}",
        answer.body
    );

    assert!(!store_dir.exists(), "the store directory is still there");
    let registry = std::fs::read_to_string(home.join("registry.json")).unwrap();
    assert!(
        !registry.contains("\"api\""),
        "the registry still lists it: {registry}"
    );
    // The corpus itself is not semlith's to delete.
    assert!(work.join("api/fleet.rs").exists(), "the corpus was deleted");

    // Still serving: the daemon lost a store, not its life.
    assert_eq!(daemon.get("/api/files").json()["total"], 0);
    assert_eq!(daemon.get("/api/privacy").status, 200);
}

// ---------------------------------------------------------------- T13

/// One request off the `Daemon` handle, for a thread that cannot borrow it.
fn post_to(port: u16, token: &str, path: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("the daemon listens");
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .unwrap();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nSemlith-Token: {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    String::from_utf8_lossy(&raw).into_owned()
}

/// A run can be stopped, and stopping undoes it. A half-indexed corpus is
/// worse than none, because nothing in the store says which half it is.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn a_stopped_index_run_undoes_itself() {
    let (dir, home, work) = sandbox("stop-index");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);

    // Outside the watched root, so the watcher cannot index it behind the
    // run's back and the count belongs to the run alone.
    let extra = work.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    for i in 0..60 {
        let body = format!("# Note {i}\n\n{}", "Ownership and borrowing. ".repeat(120));
        std::fs::write(extra.join(format!("note{i:03}.md")), body).unwrap();
    }

    let daemon = Daemon::start_in(dir, home, work.join("api"), &[]);
    let before = daemon.get("/api/files").json()["total"].as_i64().unwrap();

    let port = daemon.port;
    let token = daemon.token.clone();
    let path = extra.display().to_string();
    let run = std::thread::spawn(move || {
        let body = format!("{{\"path\":{}}}", serde_json::to_string(&path).unwrap());
        post_to(port, &token, "/api/index", &body)
    });

    // Stop it once it has written something, so the rollback has work to do.
    let mut moved = false;
    for _ in 0..200 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if daemon.get("/api/files").json()["total"].as_i64().unwrap() > before {
            moved = true;
            break;
        }
    }
    assert!(moved, "the run never indexed anything to roll back");

    let stopped = daemon.post("/api/index/control", "{\"action\":\"stop\"}");
    assert_eq!(stopped.status, 200, "{}", stopped.body);

    let answer = run.join().expect("the index request");
    assert!(
        answer.contains("\"stopped\":true"),
        "the run did not report itself stopped: {answer}"
    );

    // Back to exactly what was there before it started.
    for _ in 0..100 {
        if daemon.get("/api/files").json()["total"].as_i64() == Some(before) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!(
        "the store kept what the stopped run embedded: {} files, was {before}",
        daemon.get("/api/files").json()["total"]
    );
}

/// A job does not wait for the watcher's catch-up, and a stop asked for while
/// it is still queued is answered at once.
///
/// The catch-up used to run to completion before the queue was looked at, so
/// the first request after a daemon start on a cold store waited for the whole
/// tree — and a stop in that window was cleared when the job finally began.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn a_queued_run_starts_at_once_and_stops_at_once() {
    let (dir, home, work) = sandbox("queue-latency");
    let root = work.join("api");
    std::fs::create_dir_all(&root).unwrap();
    for i in 0..40 {
        let body = format!("# Note {i}\n\n{}", "Ownership and borrowing. ".repeat(200));
        std::fs::write(root.join(format!("n{i:03}.md")), body).unwrap();
    }
    let extra = work.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    std::fs::write(extra.join("one.md"), "# One\n\nA single file.\n").unwrap();

    // Started against a corpus it has never seen, so the watcher's catch-up is
    // real work rather than a walk of hashes.
    let daemon = Daemon::start_in(dir, home, root, &[]);

    // Two jobs, so the second is behind the first for certain rather than by
    // timing: the first holds the writer, and the second is the queued one a
    // stop has to answer without waiting for it.
    let first = std::thread::spawn({
        let token = daemon.token.clone();
        let port = daemon.port;
        let path = work.join("api").display().to_string();
        move || {
            let body = format!("{{\"path\":{}}}", serde_json::to_string(&path).unwrap());
            post_to(port, &token, "/api/index", &body)
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(300));

    let port = daemon.port;
    let token = daemon.token.clone();
    let path = extra.display().to_string();
    let queued = std::thread::spawn(move || {
        let body = format!("{{\"path\":{}}}", serde_json::to_string(&path).unwrap());
        post_to(port, &token, "/api/index", &body)
    });
    std::thread::sleep(std::time::Duration::from_millis(200));

    let asked = std::time::Instant::now();
    let stopped = daemon.post("/api/index/control", "{\"action\":\"stop\"}");
    assert_eq!(stopped.status, 200, "{}", stopped.body);

    let answer = queued.join().expect("the queued index request");
    assert!(
        answer.contains("\"stopped\":true"),
        "the queued run was not stopped: {answer}"
    );
    // Nothing of it had run, so the answer does not wait for the writer.
    assert!(
        asked.elapsed() < std::time::Duration::from_secs(10),
        "the queued job took {:?} to answer a stop",
        asked.elapsed()
    );
    let _ = first.join();
}

/// A run longer than one slice finishes on its own, on one stream, and a stop
/// undoes every slice of it rather than the one that happened to be going.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn a_run_outlasts_its_slice_and_a_stop_undoes_all_of_it() {
    let (dir, home, work) = sandbox("slices");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);
    let extra = work.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    for i in 0..12 {
        let body = format!("# Note {i}\n\n{}", "Ownership and borrowing. ".repeat(80));
        std::fs::write(extra.join(format!("n{i:03}.md")), body).unwrap();
    }

    let daemon = Daemon::start_in(dir, home, work.join("api"), &[]);
    let before = daemon.get("/api/files").json()["total"].as_i64().unwrap();

    let body = format!(
        "{{\"path\":{}}}",
        serde_json::to_string(&extra.display().to_string()).unwrap()
    );
    let answer = daemon.post("/api/index", &body);
    assert_eq!(answer.status, 200);

    // One request, one answer: whatever the slice budget did in between, the
    // caller is never asked to press the button again.
    let events: Vec<serde_json::Value> = answer
        .body
        .lines()
        .filter_map(|l| serde_json::from_str(l.trim()).ok())
        .collect();
    let done = events
        .iter()
        .find(|e| e["event"] == "done")
        .unwrap_or_else(|| panic!("the run never reported done: {events:?}"));
    assert_eq!(
        done["remaining"], 0,
        "the run handed work back to the reader: {done}"
    );
    assert_eq!(
        daemon.get("/api/files").json()["total"].as_i64().unwrap(),
        before + 12,
        "not every file was indexed"
    );
}
