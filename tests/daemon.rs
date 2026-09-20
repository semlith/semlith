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

    /// Ask for an index run and hand back the run id the daemon answers with.
    ///
    /// Until 0.20.0 this route held a worker open for the whole run and
    /// streamed it; it now queues the work and returns, because the run
    /// belongs to the daemon rather than to the request that started it. So a
    /// test asks for the run here and reads it back from `/api/index/log`,
    /// which is exactly what the page does.
    fn index_run(&self, path: &Path) -> u64 {
        let body = format!(
            "{{\"path\":{}}}",
            serde_json::to_string(&path.display().to_string()).unwrap()
        );
        let answer = self.post("/api/index", &body);
        assert_eq!(answer.status, 200, "{}", answer.body);
        let started = answer.json();
        started["runs"][0]["run"]
            .as_u64()
            .unwrap_or_else(|| panic!("the route did not name the run it started: {started}"))
    }

    /// Every line one run wrote, in the order it wrote them, up to and
    /// including its `done`.
    ///
    /// Read through the cursor rather than in one go, so what comes back is
    /// what a page polling the log would have been shown as the run went —
    /// a run that only spoke at the end would fail the `file` assertions the
    /// callers make on it.
    fn run_events(&self, store: &str, run: u64, limit: Duration) -> Vec<serde_json::Value> {
        let mut events: Vec<serde_json::Value> = Vec::new();
        let mut after: Option<u64> = None;
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            let mut path = format!("/api/index/log?store={store}&run={run}");
            if let Some(seq) = after {
                path.push_str(&format!("&after={seq}"));
            }
            for line in self.get(&path).json()["lines"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                after = line["seq"].as_u64().or(after);
                let done = line["event"] == "done";
                events.push(line);
                if done {
                    return events;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("the run never reported done: {events:?}");
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

    /// The daemon's resident memory in bytes, read from the OS rather than
    /// guessed at. `ps` is on every platform these tests run on.
    fn rss(&self) -> Option<u64> {
        let out = Command::new("ps")
            .args(["-o", "rss=", "-p", &self.child.id().to_string()])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<u64>()
            .ok()
            .map(|kib| kib * 1024)
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

/// How long to give a real index run — an embedding model over a few dozen
/// small files, on a machine that may be doing four other tests at the time.
/// A bound rather than a guess: every wait below is a poll that returns as
/// soon as the daemon says so.
const SLOW: Duration = Duration::from_secs(120);

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

/// A refused request must cost the daemon nothing that adds up. A thousand of
/// them are answered from the head alone — no body is ever read — so the
/// process is the size it was when it started.
#[test]
#[ignore = "makes a thousand requests and reads the process's memory, so it is slow"]
fn a_thousand_refusals_do_not_grow_the_daemon() {
    let daemon = Daemon::start("flood", &[]);

    let idle = daemon.rss().expect("the daemon's resident memory");

    // A megabyte of body on every one of them, declared and never sent: a
    // server that allocated before it refused would be holding a gigabyte.
    let body = "x".repeat(64 * 1024);
    for _ in 0..1000 {
        let answer = daemon.raw(&format!(
            "POST /api/index HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\n\
             Origin: http://127.0.0.1:{}\r\nSec-Fetch-Site: same-site\r\n\
             Content-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            daemon.port,
            daemon.token,
            daemon.port + 1,
            body.len()
        ));
        assert_eq!(answer.status, 403);
    }

    let after = daemon.rss().expect("the daemon's resident memory");
    let grew = after.saturating_sub(idle);
    assert!(
        grew < 10 * 1024 * 1024,
        "a thousand refusals grew the daemon by {} KiB, from {} to {}",
        grew / 1024,
        idle,
        after
    );
}

/// The Files route's offset is paid before the page is cut, so it is bounded
/// rather than clamped: a number past the end is a mistake worth saying so.
#[test]
fn the_files_route_refuses_an_offset_it_would_have_to_allocate() {
    let daemon = Daemon::start("offset", &[]);

    let far = daemon.get("/api/files?offset=4000000000");
    assert_eq!(far.status, 400);
    assert!(
        far.json()["error"]
            .as_str()
            .unwrap_or_default()
            .contains("10000"),
        "the refusal does not say what the limit is: {}",
        far.body
    );

    assert_eq!(daemon.get("/api/files?offset=-1").status, 400);
    assert_eq!(daemon.get("/api/files?offset=10000").status, 200);
    assert_eq!(daemon.get("/api/files?offset=10001").status, 400);
}

/// A panicking route used to cost a worker permanently and poison every lock
/// it held, so one malformed request took an eighth of the server and eight of
/// them took all of it. It now costs one 500 and nothing else.
///
/// Only in a debug build: the route that panics is compiled out of the binary
/// a user installs, so there is nothing to drive in a release run.
#[test]
#[cfg(debug_assertions)]
fn a_panicking_route_costs_one_request_and_not_the_daemon() {
    let daemon = Daemon::start("panic", &[]);

    // More panics than the pool has workers, so a pool that lost one per panic
    // would have none left.
    for attempt in 0..12 {
        let boom = daemon.get("/api/panic");
        assert_eq!(boom.status, 500, "panic {attempt} answered {}", boom.status);
        assert!(boom.body.is_empty(), "the 500 leaked a body: {}", boom.body);
    }

    // The same route, still answering. Then a route that reads the state the
    // panicking one was holding when it died.
    assert_eq!(daemon.get("/api/panic").status, 500);
    let about = daemon.get("/api/about");
    assert_eq!(
        about.status, 200,
        "the daemon stopped answering after a panic"
    );
    assert_eq!(about.json()["version"], env!("CARGO_PKG_VERSION"));

    let stores = daemon.get("/api/stores");
    assert_eq!(
        stores.status, 200,
        "the store locks did not survive the panics"
    );

    // And a write, which takes more of the daemon's state than a read does.
    let endpoint = daemon.post("/api/endpoint", r#"{"open":false}"#);
    assert_eq!(
        endpoint.status, 200,
        "a write after a panic: {}",
        endpoint.body
    );
}

/// Guessing costs time, and the cost grows with the run. The numbers come off
/// the test's own clock rather than out of the constants, because a delay that
/// is configured and never applied looks identical to one that works.
#[test]
#[ignore = "measures a real delay, so it takes the delay"]
fn a_run_of_wrong_tokens_gets_slower_and_a_single_one_does_not() {
    let daemon = Daemon::start("throttle", &[]);

    let one = Instant::now();
    let refused = daemon.raw(&format!(
        "GET /api/stores HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: not-it\r\nConnection: close\r\n\r\n",
        daemon.port
    ));
    let single = one.elapsed();
    assert_eq!(refused.status, 401);
    assert!(
        single >= Duration::from_millis(250),
        "one wrong token was answered in {single:?}, so nothing is being held"
    );
    assert!(
        single < Duration::from_millis(1200),
        "one mistyped URL cost {single:?}; a single mistake must not be \
         punished like a run of guesses"
    );

    // Sixty in a row: twenty at a quarter of a second, twenty at half, twenty
    // at a second. Thirty-five seconds is the floor that arithmetic gives, and
    // the first request above has already been charged to the same window.
    let run = Instant::now();
    for _ in 0..59 {
        let answer = daemon.raw(&format!(
            "GET /api/stores HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: not-it\r\n\
             Connection: close\r\n\r\n",
            daemon.port
        ));
        assert_eq!(answer.status, 401);
        assert!(answer.body.is_empty(), "a refusal leaked a body");
    }
    let elapsed = run.elapsed();
    assert!(
        elapsed >= Duration::from_secs(34),
        "sixty wrong tokens took {elapsed:?}; the delay is not escalating"
    );
}

/// The throttle must not become the denial of service it exists to prevent: a
/// connection being held costs the thread that holds it and nothing else.
#[test]
#[ignore = "measures a real delay, so it takes the delay"]
fn a_held_refusal_does_not_delay_anybody_else() {
    let daemon = Daemon::start("interleave", &[]);
    let port = daemon.port;

    // More wrong tokens at once than the server has workers. If the delay were
    // answered on a worker, these would hold every one of them.
    let floods: Vec<_> = (0..16)
        .map(|_| {
            std::thread::spawn(move || {
                let mut stream =
                    TcpStream::connect(("127.0.0.1", port)).expect("the daemon listens");
                stream
                    .set_read_timeout(Some(Duration::from_secs(30)))
                    .unwrap();
                let request = format!(
                    "GET /api/stores HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
                     Semlith-Token: not-it\r\nConnection: close\r\n\r\n"
                );
                stream.write_all(request.as_bytes()).unwrap();
                stream.flush().unwrap();
                let mut raw = Vec::new();
                let _ = stream.read_to_end(&mut raw);
            })
        })
        .collect();

    // Long enough for all sixteen to be parsed, refused and put on hold.
    std::thread::sleep(Duration::from_millis(120));

    let good = Instant::now();
    let answer = daemon.get("/api/stores");
    let waited = good.elapsed();
    assert_eq!(answer.status, 200);
    assert!(
        waited < Duration::from_millis(400),
        "a correct request waited {waited:?} behind sixteen held refusals"
    );

    for flood in floods {
        let _ = flood.join();
    }
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
    assert_eq!(
        foreign_origin.status, 403,
        "another origin's write was read"
    );

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

/// A run is reported as it happens, so a browser sees it while it is running.
/// A run that spoke only once it was over would be a progress bar that only
/// ever shows 100%.
///
/// Until 0.20.0 this route held an HTTP worker open for the whole run and
/// streamed it as newline-delimited JSON, so the tab that pressed the button
/// was the only thing in the world that knew the run was happening. The run
/// lives in the daemon now: the same closure that emitted those chunks writes
/// every event to the run's own log, `POST /api/index` answers with the run's
/// id at once, and `/api/index/log` is what the page draws from. What is
/// asserted here is unchanged — the events, in order, per file — because that
/// is the guarantee; the framing was only ever how it was delivered.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn indexing_from_the_portal_reports_progress_and_then_lists_the_files() {
    let (dir, home, work) = sandbox("index-route");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);
    // Not inside the watched root: the watcher would index it at startup, and
    // these tests are about a run they ask for. A folder the page hands to
    // `POST /api/index` becomes one of the store's roots, which is how a
    // folder joins an existing corpus from the page — the boundary an agent is
    // held to is asserted by `the_index_boundary_holds_for_an_agents_index`.
    let extra = home.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    std::fs::write(extra.join("lock.rs"), "pub struct StoreLock;\n").unwrap();

    let daemon = Daemon::start_in(dir, home, work.join("api"), &[]);

    let run = daemon.index_run(&extra);
    let events = daemon.run_events("api", run, Duration::from_secs(120));
    assert!(
        events.iter().any(|e| e["event"] == "file"),
        "no progress event arrived before the run completed: {events:?}"
    );
    let done = events
        .iter()
        .find(|e| e["event"] == "done")
        .unwrap_or_else(|| panic!("the run never reported done: {events:?}"));
    assert_eq!(done["indexed"], 1);

    // The run says something before the writer reaches the job, and then says
    // what happened to every file rather than only to the ones it
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

/// The Agents view's stanzas come from `docs/clients.md`, which is the same
/// text `tests/clients.rs` executes. A stanza retyped anywhere else would be
/// right until somebody edited one of the two copies.
#[test]
fn the_agents_route_serves_the_documented_stanzas_verbatim() {
    let daemon = Daemon::start("agents", &[]);
    let agents = daemon.get("/api/agents").json();

    let clients = agents["clients"].as_array().expect("clients");
    assert!(clients.len() >= 12, "only {} clients", clients.len());
    assert_eq!(clients[0]["name"], "Claude Code");

    // The registration, found by its role rather than by its position: the
    // list grew an `unregister` fence in 0.18.0, and a positional index is a
    // second thing to keep right every time the documentation gains a block.
    let stanza = clients[0]["stanzas"]
        .as_array()
        .expect("stanzas")
        .iter()
        .find(|stanza| stanza["register"] == true)
        .and_then(|stanza| stanza["text"].as_str())
        .expect("Claude Code carries a registration");
    let stanza = stanza.trim();
    assert!(
        stanza.starts_with("claude mcp add --scope user semlith -- ") && stanza.ends_with(" mcp"),
        "the registration the portal serves must carry the scope that means every project: {stanza}"
    );
    // And an absolute path, not the bare command. 0.21.0 changed this because
    // a bare `semlith` launches only from a PATH that happens to carry it, and
    // the PATH a login shell builds is not the one a service manager hands a
    // job — which is how a correctly registered semlith comes to be absent
    // with nothing saying so. Asserted as a property rather than as the exact
    // string, because the string is this machine's own binary path.
    let program = stanza
        .trim_start_matches("claude mcp add --scope user semlith -- ")
        .trim_end_matches(" mcp")
        .trim_matches('"');
    assert!(
        std::path::Path::new(program).is_absolute(),
        "the registration names `{program}`, which launches only from a PATH that carries it"
    );
    assert!(
        !program.starts_with(r"\\?\"),
        "the registration carries a verbatim path, which a client configuration cannot use: {program}"
    );

    let doc =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/clients.md"))
            .expect("docs/clients.md is beside the crate");
    // The document holds the placeholder; the route serves it with this
    // machine's own binary path substituted in. Putting the placeholder back is
    // what keeps this assertion about the thing it was written to protect —
    // that the route serves what the documentation says, and not a second
    // hand-written copy of it — now that one token of the stanza is per-machine.
    let documented = stanza.replace(program, "${SEMLITH_BIN}");
    assert!(
        doc.contains(&documented),
        "the route serves a stanza docs/clients.md does not contain: {documented}"
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
    // Inside the store's root, because that is what an agent may write: the
    // boundary is the store's registered roots, and the home directory is the
    // fallback only for a store that has none — so a folder beside the corpus
    // or under the home is refused, which
    // `the_index_boundary_holds_for_an_agents_index` asserts. Created before
    // the daemon starts, so the startup catch-up indexes it and no filesystem
    // event is in flight while the agent works: the forget below is then the
    // only thing that can have removed it, and the index after it the only
    // thing that can have put it back.
    let extra = root.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    std::fs::write(extra.join("lock.rs"), "pub struct StoreLock;\n").unwrap();

    let daemon = Daemon::start_in(dir, home.clone(), root.clone(), &[]);

    // Wait for the startup catch-up to have both files, or it lands between
    // the forget and the index below and the agent is told "1 unchanged"
    // about a file the watcher put back. The daemon prints its URL before the
    // catch-up has walked the tree, so starting is not the same as caught up.
    until("the catch-up to have indexed the corpus", SLOW, || {
        daemon.get("/api/files").json()["total"] == serde_json::json!(2)
    });

    let mut server = Proxied::open(&home, &root);

    let stats = server.call("tools/call", serde_json::json!({ "name": "semlith_stats" }));
    assert!(
        stats["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("chunks"),
        "{stats}"
    );

    // The write that failed for the whole of 0.8.0 while a watcher held the
    // store. Nothing here touches the file on disk, so the watcher has no
    // event to race the agent with.
    let forgotten = server.call(
        "tools/call",
        serde_json::json!({
            "name": "semlith_forget",
            "arguments": { "path": extra.join("lock.rs").display().to_string() }
        }),
    );
    let removed = forgotten["result"]["content"][0]["text"].as_str().unwrap();
    assert!(removed.starts_with("Removed"), "{forgotten}");
    assert!(
        !removed.contains("being indexed by") && !removed.contains("is held by"),
        "the write tool still hit the lock: {removed}"
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

/// A run can be stopped, and stopping undoes it. A half-indexed corpus is
/// worse than none, because nothing in the store says which half it is.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn a_stopped_index_run_undoes_itself() {
    let (dir, home, work) = sandbox("stop-index");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);

    // Outside the watched root, so the watcher cannot index it behind the
    // run's back and the count belongs to the run alone. Not inside the root
    // either: the watcher would index it at startup, and this test is about a
    // run it asks for.
    let extra = home.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    for i in 0..60 {
        let body = format!("# Note {i}\n\n{}", "Ownership and borrowing. ".repeat(120));
        std::fs::write(extra.join(format!("note{i:03}.md")), body).unwrap();
    }

    let daemon = Daemon::start_in(dir, home, work.join("api"), &[]);
    let before = daemon.get("/api/files").json()["total"].as_i64().unwrap();

    let run = daemon.index_run(&extra);

    // Stop it once it has written something, so the rollback has work to do.
    until("the run to embed something to roll back", SLOW, || {
        daemon.get("/api/files").json()["total"].as_i64().unwrap() > before
    });

    let stopped = daemon.post("/api/index/control", "{\"action\":\"stop\"}");
    assert_eq!(stopped.status, 200, "{}", stopped.body);

    // The run answers for itself, on its own record, rather than through the
    // request that started it: since 0.20.0 that request was answered the
    // moment the run was queued.
    let events = daemon.run_events("api", run, SLOW);
    let done = events
        .last()
        .filter(|e| e["event"] == "done")
        .unwrap_or_else(|| panic!("the run never reported done: {events:?}"));
    assert_eq!(
        done["stopped"],
        serde_json::json!(true),
        "the run did not report itself stopped: {done}"
    );

    // Back to exactly what was there before it started.
    until(
        &format!("the store to give back what the stopped run embedded, to {before} files"),
        SLOW,
        || daemon.get("/api/files").json()["total"].as_i64() == Some(before),
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
    // Not inside the watched root: the watcher would index it at startup, and
    // this test is about a run it asks for.
    let extra = home.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    std::fs::write(extra.join("one.md"), "# One\n\nA single file.\n").unwrap();

    // Started against a corpus it has never seen, so the watcher's catch-up is
    // real work rather than a walk of hashes.
    let daemon = Daemon::start_in(dir, home, root, &[]);

    // Two jobs, so the second is behind the first for certain rather than by
    // timing: the first holds the writer, and the second is the queued one a
    // stop has to answer without waiting for it.
    let first = daemon.index_run(&work.join("api"));
    let queued = daemon.index_run(&extra);
    assert_ne!(first, queued, "one submission became one run twice over");

    // Waiting its turn, on the store's queue or on the admission queue — which
    // of the two it is depends on how many runs this machine admits at once,
    // and the answer a stop owes it is the same either way.
    let is_queued = |run: &serde_json::Value| run["id"] == queued && run["status"] == "queued";
    until(
        "the second run to be waiting behind the first",
        SLOW,
        || {
            daemon.get("/api/index/runs").json()["runs"]
                .as_array()
                .is_some_and(|runs| runs.iter().any(is_queued))
        },
    );

    let asked = std::time::Instant::now();
    let stopped = daemon.post("/api/index/control", "{\"action\":\"stop\"}");
    assert_eq!(stopped.status, 200, "{}", stopped.body);

    // Nothing of it had run, so the answer does not wait for the writer to
    // finish the run in front of it.
    let events = daemon.run_events("api", queued, Duration::from_secs(10));
    let done = events
        .last()
        .filter(|e| e["event"] == "done")
        .unwrap_or_else(|| panic!("the queued run never reported done: {events:?}"));
    assert_eq!(
        done["stopped"],
        serde_json::json!(true),
        "the queued run was not stopped: {done}"
    );
    assert!(
        asked.elapsed() < std::time::Duration::from_secs(10),
        "the queued job took {:?} to answer a stop",
        asked.elapsed()
    );
}

/// A run longer than one slice finishes on its own, on one stream, and a stop
/// undoes every slice of it rather than the one that happened to be going.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn a_run_outlasts_its_slice_and_a_stop_undoes_all_of_it() {
    let (dir, home, work) = sandbox("slices");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);
    // Not inside the watched root: the watcher would index it at startup, and
    // this test is about a run it asks for.
    let extra = home.join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    for i in 0..12 {
        let body = format!("# Note {i}\n\n{}", "Ownership and borrowing. ".repeat(80));
        std::fs::write(extra.join(format!("n{i:03}.md")), body).unwrap();
    }

    let daemon = Daemon::start_in(dir, home, work.join("api"), &[]);
    let before = daemon.get("/api/files").json()["total"].as_i64().unwrap();

    let run = daemon.index_run(&extra);

    // One submission, one run: whatever the slice budget did in between, the
    // caller is never asked to press the button again, and every slice is
    // reported against the one run it belongs to.
    let events = daemon.run_events("api", run, SLOW);
    let dones: Vec<&serde_json::Value> = events.iter().filter(|e| e["event"] == "done").collect();
    assert_eq!(
        dones.len(),
        1,
        "a slice ended the run rather than continuing it: {events:?}"
    );
    let done = dones[0];
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

/// A session id arrives from a client and goes back out in a response header,
/// so what a client may send is exactly what the server produces.
#[test]
fn a_session_id_is_sixteen_hex_characters_or_it_is_replaced() {
    let daemon = Daemon::start("session", &[]);
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{}}}"#;

    let send = |id: &str| -> Answer {
        daemon.raw(&format!(
            "POST /api/mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\n\
             Mcp-Session-Id: {id}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{init}",
            daemon.port,
            daemon.token,
            init.len()
        ))
    };

    // A well-formed one is echoed back.
    let good = send("0123456789abcdef");
    assert_eq!(good.status, 200, "{}", good.body);
    assert!(
        good.headers.contains("Mcp-Session-Id: 0123456789abcdef"),
        "a valid session id was not kept:\n{}",
        good.headers
    );

    for bad in [
        "not-hex-at-all!!",
        "0123456789abcdefg",
        "short",
        "0123456789abcde\u{7f}",
    ] {
        let answer = send(bad);
        assert_eq!(answer.status, 200, "{}", answer.body);
        assert!(
            !answer.headers.contains(bad),
            "a malformed session id was reflected into a response header:\n{}",
            answer.headers
        );
        // Replaced rather than dropped: the client is told the id it will be
        // known by, the same way it would be told a first one.
        let line = answer
            .headers
            .lines()
            .find(|l| l.starts_with("Mcp-Session-Id: "))
            .unwrap_or_else(|| panic!("no session id was handed back:\n{}", answer.headers));
        let fresh = line.trim_start_matches("Mcp-Session-Id: ").trim();
        assert_eq!(
            fresh.len(),
            16,
            "the replacement is not a session id: {fresh}"
        );
        assert!(fresh.chars().all(|c| c.is_ascii_hexdigit()), "{fresh}");
    }
}

/// The other half of the index boundary: a forwarded `semlith_index` is held
/// to it inside the daemon, not only in the stdio MCP server. A path outside
/// the target store's roots is refused by name, with the rule that refused it.
///
/// The agent is the credential this boundary is for: the key lives in a config
/// file on disk, so what it can reach is what a copied config file can reach,
/// and `/mcp` is the only route that key opens. From 0.26.0 `POST /api/index`
/// is held to the same boundary whenever it names a store (#120); what it may
/// still do, and an agent may not, is index a folder that names no store at
/// all, which becomes its own corpus through `home::resolve` rather than
/// joining somebody else's. The two are asserted apart because they are two
/// different credentials, not because the rule is soft.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn the_index_boundary_holds_for_an_agents_index() {
    let (dir, home, work) = sandbox("agent-boundary");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);
    // A sibling of the corpus: not under the store's root.
    let outside = work.join("somebody-elses");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("notes.md"), "Ownership and borrowing.\n").unwrap();

    let root = work.join("api");
    let daemon = Daemon::start_in(dir, home.clone(), root.clone(), &[]);
    let mut server = Proxied::open(&home, &root);

    let asked = server.call(
        "tools/call",
        serde_json::json!({
            "name": "semlith_index",
            "arguments": { "path": outside.display().to_string() }
        }),
    );
    let text = asked["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("the tool did not answer with text: {asked}"));

    // Refused by name, with the rule, in what the agent is told.
    assert!(
        text.contains("refused") && text.contains("outside"),
        "the run did not report the refusal:\n{text}"
    );
    assert!(
        text.contains("somebody-elses"),
        "the refusal does not name the path:\n{text}"
    );
    assert!(
        text.starts_with("0 indexed"),
        "a refused path was indexed anyway:\n{text}"
    );

    // And nothing from it reached the store.
    let files = daemon.get("/api/files").json();
    let listed = serde_json::to_string(&files).unwrap();
    assert!(
        !listed.contains("notes.md"),
        "a refused file is in the store:\n{listed}"
    );
}

/// The daemon is the sole writer of every registered store, but it is not the
/// only process that writes one: `semlith index` beside a running daemon is
/// how a new project joins. Until 0.19.0 the store it created was invisible —
/// not on the Stores page, not answerable by name over MCP — until the daemon
/// was restarted.
///
/// A corpus of empty files, so the run registers a store without embedding
/// anything and this test needs no model.
#[test]
fn a_store_the_cli_wrote_appears_without_a_restart() {
    let (dir, home, work) = sandbox("reconcile");
    let served = work.join("api");
    std::fs::create_dir_all(&served).unwrap();
    std::fs::write(served.join("notes.md"), "").unwrap();

    let daemon = Daemon::start_in(dir, home, served, &[]);

    let joining = daemon.home.join("..").join("work").join("new-project");
    std::fs::create_dir_all(&joining).unwrap();
    std::fs::write(joining.join("readme.md"), "").unwrap();

    let before = daemon.get("/api/stores").json();
    assert!(
        !serde_json::to_string(&before)
            .unwrap()
            .contains("new-project"),
        "the store exists before it has been created"
    );

    let out = semlith(&daemon.home)
        .arg("index")
        .arg(&joining)
        .output()
        .expect("semlith index runs");
    assert!(
        out.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let after = daemon.get("/api/stores").json();
    let listed = serde_json::to_string(&after).unwrap();
    assert!(
        listed.contains("new-project"),
        "the daemon did not reconcile with the registry:\n{listed}"
    );
}

/// A registered directory another process holds the lock on is reported as
/// being written, never forced open: the daemon taking a second writer's lock
/// is the one thing the store's whole design is arranged to prevent.
#[test]
fn a_store_another_process_is_writing_is_listed_rather_than_opened() {
    let (dir, home, work) = sandbox("reconcile-locked");
    let served = work.join("api");
    std::fs::create_dir_all(&served).unwrap();
    std::fs::write(served.join("notes.md"), "").unwrap();

    let daemon = Daemon::start_in(dir, home, served, &[]);

    let joining = daemon.home.join("..").join("work").join("busy-project");
    std::fs::create_dir_all(&joining).unwrap();
    std::fs::write(joining.join("readme.md"), "").unwrap();
    let out = semlith(&daemon.home)
        .arg("index")
        .arg(&joining)
        .output()
        .expect("semlith index runs");
    assert!(out.status.success());

    // Held by this process, which is not the daemon — exactly the case a
    // second `semlith index` still running would present.
    let store_dir = daemon.home.join("stores").join("busy-project");
    let lock = semlith::lock::StoreLock::acquire(&store_dir).expect("the lock is free");

    let listed = serde_json::to_string(&daemon.get("/api/stores").json()).unwrap();
    assert!(
        listed.contains("busy-project"),
        "a locked store should still be listed:\n{listed}"
    );
    assert!(
        listed.contains("unopened"),
        "a locked store should say why it is not open:\n{listed}"
    );

    // Released, and the next read opens it by itself.
    drop(lock);
    let reopened = serde_json::to_string(&daemon.get("/api/stores").json()).unwrap();
    assert!(
        !reopened.contains("unopened"),
        "the store should have opened once the lock was released:\n{reopened}"
    );
}

/// The index route refuses a path outside the store's boundary, and refusing
/// it leaves the store's roots as they were.
///
/// Issue #120: the route recorded whatever was posted as a root *before* the
/// run applied the boundary, so the check could never refuse anything and the
/// promise in `docs/security.md` was not kept from 0.20.0 to 0.26.0.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn the_index_route_refuses_a_path_outside_the_boundary_and_keeps_the_roots() {
    let (dir, home, work) = sandbox("boundary");
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);
    // A sibling of the indexed corpus, outside the store's roots and outside
    // the sandbox home — the exact shape the issue describes.
    let outside = dir.path().join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.rs"), RUST).unwrap();

    let registry = home.join(".semlith").join("registry.json");
    let before = std::fs::read_to_string(&registry).unwrap_or_default();

    let daemon = Daemon::start_in(dir, home.clone(), work.join("api"), &[]);
    // Naming the store is the case the promise is about: this path is to go
    // into *that* corpus, and it is not under any of that corpus's roots.
    // Naming no store is the other half of the fix — the path picks its own
    // store through `home::resolve`, as `semlith index` does, and is asserted
    // below.
    let body = format!(
        "{{\"store\":\"api\",\"path\":{}}}",
        serde_json::to_string(&outside.display().to_string()).unwrap()
    );
    let answer = daemon.post("/api/index", &body);
    assert_eq!(
        answer.status, 403,
        "a path outside the boundary was accepted: {}",
        answer.body
    );
    assert!(
        answer.body.contains("outside the boundary"),
        "the refusal does not name the rule: {}",
        answer.body
    );
    assert!(
        answer.body.contains("elsewhere"),
        "the refusal does not name the path: {}",
        answer.body
    );

    let after = std::fs::read_to_string(&registry).unwrap_or_default();
    assert_eq!(
        before, after,
        "a refused path was recorded as a root anyway"
    );
    assert!(
        !after.contains("elsewhere"),
        "the refused path is in the registry: {after}"
    );

    // With no store named the same path is accepted, because it becomes its
    // own corpus rather than joining somebody else's. That is where `semlith
    // index <path>` would have put it, and it is why refusing the named case
    // costs the page nothing.
    let own = format!(
        "{{\"path\":{}}}",
        serde_json::to_string(&outside.display().to_string()).unwrap()
    );
    let accepted = daemon.post("/api/index", &own);
    assert_eq!(
        accepted.status, 200,
        "a folder with no store named must become its own store: {}",
        accepted.body
    );
    let started = accepted.json();
    let store = started["runs"][0]["store"].as_str().unwrap_or_default();
    assert_ne!(
        store, "api",
        "the folder joined an unrelated store: {started}"
    );
}

/// A forwarded `semlith_index` says how the skipped divide up, exactly as the
/// in-process server does.
///
/// Issue #121: the daemon's summary was built from counts alone, so an agent
/// whose call was forwarded was told "1 847 skipped" and nothing about why,
/// while the same call answered in process named every reason. One call in
/// two places may not say two different things about it.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn a_forwarded_index_names_the_reasons_it_skipped() {
    let (dir, home, work) = sandbox("skipped-reasons");
    let root = work.join("api");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("fleet.rs"), RUST).unwrap();
    // Bytes no reader claims: skipped, with a reason.
    std::fs::write(root.join("blob.bin"), [0u8, 159, 146, 150, 0, 1, 2, 3]).unwrap();
    corpus(&home, &work, "api", &[("fleet.rs", RUST)]);

    let daemon = Daemon::start_in(dir, home.clone(), root.clone(), &[]);
    let _ = &daemon;
    let mut server = Proxied::open(&home, &root);
    let asked = server.call(
        "tools/call",
        serde_json::json!({
            "name": "semlith_index",
            "arguments": { "path": root.display().to_string() }
        }),
    );
    let text = asked["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("the tool did not answer with text: {asked}"));

    assert!(text.contains("skipped"), "{text}");
    assert!(
        text.contains("binary"),
        "the forwarded summary gives a skipped count with no reason:\n{text}"
    );
    // The shape the in-process server writes: the reasons in a parenthesis
    // directly after the count.
    assert!(
        text.contains("skipped (") || text.contains("skipped ("),
        "the reasons are not where the in-process summary puts them:\n{text}"
    );
}
