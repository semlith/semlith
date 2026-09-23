//! POST /api/index, many at once, for one folder (#132).
//!
//! The browser drive saw this route answer 409 with a SQLite error out of
//! `store_for` under heavy load. Four requests posted together for one new
//! folder made it answer 409 in every run before the fix: always a registry
//! error, from two threads rewriting it through the one temporary file the
//! process owns, and in some runs SQLite's "duplicate column name" or
//! "database is locked", from two connections laying the same new store down
//! at once (`store::tests` holds that half on its own). A second process
//! running `semlith stats` against the same home stands in for the CLI beside
//! a daemon.
//!
//! Offline: no run embeds anything (the model cache is empty and the air gap
//! is closed), because the failure is in choosing and opening the store, which
//! happens before a run starts.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Daemon {
    child: Child,
    port: u16,
    token: String,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn semlith(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_semlith"));
    command
        .env("SEMLITH_HOME", home)
        .env("HOME", home)
        .env("SEMLITH_MODEL_CACHE", home.join("no-models"))
        .env("SEMLITH_AIRGAP", "1")
        .env_remove("SEMLITH_STORE")
        .env_remove("SEMLITH_PORT");
    command
}

impl Daemon {
    fn start(home: &Path, work: &Path) -> Self {
        let mut child = semlith(home)
            .args(["start", "--port", "0"])
            .current_dir(work)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("semlith start runs");
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().expect("stdout is piped"))
            .read_line(&mut line)
            .expect("the daemon prints its URL");
        let rest = line
            .trim()
            .strip_prefix("http://127.0.0.1:")
            .unwrap_or_else(|| panic!("not a loopback URL: {line}"))
            .to_string();
        let (port, token) = rest.split_once("/?token=").expect("a token in the URL");
        Self {
            port: port.parse().expect("a port"),
            token: token.to_string(),
            child,
        }
    }

    fn post(&self, path: &str, body: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("the daemon listens");
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .unwrap();
        write!(
            stream,
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            self.token,
            body.len()
        )
        .unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (status, body.to_string())
    }
}

#[test]
fn index_requests_posted_together_for_one_folder_all_start() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let work = dir.path().join("work");
    let corpus = work.join("small");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&corpus).unwrap();
    for i in 0..20 {
        std::fs::write(corpus.join(format!("f{i}.rs")), format!("fn f{i}() {{}}\n")).unwrap();
    }
    let daemon = Arc::new(Daemon::start(&home, &work));
    let body = format!(
        "{{\"path\":[{}],\"store\":\"each\"}}",
        serde_json::to_string(&corpus.display().to_string()).unwrap()
    );

    // Released together, so the first requests race to create the store; the
    // rest keep coming while the daemon serves it.
    let deadline = Instant::now() + Duration::from_secs(8);
    let start = Arc::new(std::sync::Barrier::new(4));
    let posters: Vec<_> = (0..4)
        .map(|_| {
            let daemon = Arc::clone(&daemon);
            let body = body.clone();
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                let mut refused = Vec::new();
                while Instant::now() < deadline {
                    let (status, answer) = daemon.post("/api/index", &body);
                    // A run that could not be queued is reported per path in
                    // a 200; only a store that could not be had is a 409.
                    if status != 200 {
                        refused.push(format!("{status} {answer}"));
                    }
                }
                refused
            })
        })
        .collect();
    let reader = {
        let home = home.clone();
        std::thread::spawn(move || {
            let mut failed = Vec::new();
            while Instant::now() < deadline {
                let out = semlith(&home).arg("stats").output().unwrap();
                let said = String::from_utf8_lossy(&out.stderr).into_owned();
                if said.contains("disk I/O") || said.contains("database is locked") {
                    failed.push(said);
                }
            }
            failed
        })
    };

    let mut failures: Vec<String> = posters
        .into_iter()
        .flat_map(|p| p.join().unwrap())
        .collect();
    failures.extend(reader.join().unwrap());
    failures.sort();
    failures.dedup();
    assert!(
        failures.is_empty(),
        "requests for one folder were refused:\n{}",
        failures.join("\n")
    );
}
