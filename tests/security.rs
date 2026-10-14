//! One test per finding of the 2026-10-14 security audit (S01-S11), each
//! written to fail on the v0.39.0 binary before its fix landed.
//!
//! Every test drives the binary as a user or an agent would — the CLI, or a
//! scratch daemon on port 0 with its own `HOME` and `SEMLITH_HOME` — so the
//! same file proves the fix on this build and the defect on an old one:
//! `SEMLITH_TEST_BIN=<path to a v0.39.0 semlith>` runs it against that binary.
//! Fixtures are generated at run time; no hostile file is committed.
//!
//! Tests that index need the embedding model, so they are `#[ignore]`d like
//! the rest of the suites that do; they share the user's model cache rather
//! than downloading it into every scratch home.

use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

/// The binary under test: this build's, or the one `SEMLITH_TEST_BIN` names.
fn bin() -> PathBuf {
    std::env::var_os("SEMLITH_TEST_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_semlith")))
}

/// The user's model cache, so a scratch home does not download the model.
fn model_cache() -> PathBuf {
    std::env::var_os("SEMLITH_MODEL_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap())
                .join(".cache")
                .join("semlith")
                .join("models")
        })
}

/// A command against a scratch home.
fn semlith(home: &Path) -> Command {
    let mut cmd = Command::new(bin());
    cmd.env("HOME", home)
        .env("USERPROFILE", home)
        .env("SEMLITH_HOME", home.join(".semlith"))
        .env("SEMLITH_MODEL_CACHE", model_cache())
        .env_remove("SEMLITH_STORE")
        .env_remove("SEMLITH_PORT")
        .current_dir(home);
    cmd
}

fn run(home: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = semlith(home);
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("semlith runs")
}

struct Daemon {
    child: Child,
    port: u16,
    token: String,
    home: PathBuf,
}

impl Daemon {
    fn start(home: &Path, envs: &[(&str, &str)]) -> Self {
        std::fs::create_dir_all(home).unwrap();
        let mut cmd = semlith(home);
        cmd.args(["start", "--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("semlith start runs");
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut line)
            .expect("the daemon prints its URL");
        let rest = line
            .trim()
            .strip_prefix("http://127.0.0.1:")
            .unwrap_or_else(|| panic!("no loopback URL in {line:?}"));
        let (port, token) = rest.split_once("/?token=").expect("a token in the URL");
        Self {
            child,
            port: port.parse().unwrap(),
            token: token.to_string(),
            home: home.to_path_buf(),
        }
    }

    fn alive(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }

    fn key(&self) -> String {
        std::fs::read_to_string(self.home.join(".semlith").join("agent.key"))
            .expect("the daemon wrote an agent key")
            .trim()
            .to_string()
    }

    fn send(&self, method: &str, path: &str, headers: &str, body: &str) -> (u16, String) {
        let Ok(mut stream) = TcpStream::connect(("127.0.0.1", self.port)) else {
            return (0, String::new());
        };
        stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{headers}\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            body.len(),
        );
        if stream.write_all(request.as_bytes()).is_err() {
            return (0, String::new());
        }
        let mut raw = Vec::new();
        let _ = stream.read_to_end(&mut raw);
        let text = String::from_utf8_lossy(&raw).into_owned();
        let status = text
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body = text.split_once("\r\n\r\n").map_or("", |(_, b)| b).to_string();
        (status, body)
    }

    fn api(&self, path: &str) -> Value {
        let (status, body) = self.send("GET", path, &format!("Semlith-Token: {}\r\n", self.token), "");
        if status != 200 {
            return Value::Null;
        }
        serde_json::from_str(&body).unwrap_or(Value::Null)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Wait for `done` to hold, polling, up to `limit`. A condition, never a
/// fixed sleep: what is waited on is the state, not a guess at its timing.
fn until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    done()
}

// ---------------------------------------------------------------------- S01

/// The three shapes that abort tree-sitter's block grammars at depth 300, and
/// one ordinary file.
fn nesting_corpus(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let d = 300;
    std::fs::write(dir.join("quotes.md"), "> ".repeat(d) + "x\n").unwrap();
    let list: String = (0..d).map(|i| "  ".repeat(i) + "- a\n").collect();
    std::fs::write(dir.join("list.md"), list).unwrap();
    let seq: String = (0..d).map(|i| "  ".repeat(i) + "-\n").collect::<String>() + &"  ".repeat(d) + "- x\n";
    std::fs::write(dir.join("seq.yaml"), seq).unwrap();
    std::fs::write(dir.join("notes.md"), "# Notes\n\nAn ordinary file beside them.\n").unwrap();
}

const HOSTILE: [&str; 3] = ["quotes.md", "list.md", "seq.yaml"];

fn crashed_rows(refused: &Value) -> Vec<(String, String)> {
    refused["stores"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|s| s["rows"].as_array().cloned().unwrap_or_default())
        .filter(|r| r["class"] == "crashed")
        .map(|r| {
            (
                r["path"].as_str().unwrap_or_default().to_string(),
                r["rule"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

fn begins(home: &Path, name: &str) -> usize {
    let mut n = 0;
    for entry in walk(&home.join(".semlith")) {
        if entry.file_name().is_some_and(|f| f == "parse-journal") {
            let text = std::fs::read_to_string(&entry).unwrap_or_default();
            n += text
                .lines()
                .filter(|l| l.contains("\"op\":\"begin\"") && l.contains(name))
                .count();
        }
    }
    n
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// S01: the three files that aborted 0.39.0 leave the CLI and the daemon
/// running, each refused with a parser-crash reason, and neither a catch-up
/// nor a restart parses them again. The nesting cap is switched off (debug
/// builds honour the switch) so the real C abort reaches the parse worker.
#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn s01_a_file_that_aborts_a_parser_is_refused_once_and_never_loops() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let corpus = dir.path().join("corpus");
    std::fs::create_dir_all(&home).unwrap();
    nesting_corpus(&corpus);
    let cap_off = [("SEMLITH_NO_NESTING_CAP", "1")];

    let out = run(&home, &["index", corpus.to_str().unwrap()], &cap_off);
    assert!(
        out.status.success(),
        "semlith index did not survive the corpus: {:?}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let out = run(&home, &["refused", "--json"], &cap_off);
    let refused: Value = serde_json::from_slice(&out.stdout).expect("refused --json");
    let rows = crashed_rows(&refused);
    for name in HOSTILE {
        let row = rows.iter().find(|(p, _)| p.ends_with(name));
        let (_, reason) = row.unwrap_or_else(|| panic!("{name} is not refused as crashed: {rows:?}"));
        assert!(reason.starts_with("parser crashed"), "{name}: {reason}");
    }
    let before: Vec<usize> = HOSTILE.iter().map(|n| begins(&home, n)).collect();

    // Catch-up on start, then a restart: the daemon stays up, the refusals
    // stand, and the journal shows no second parse of any of them.
    for round in 0..2 {
        let mut daemon = Daemon::start(&home, &cap_off);
        let listed = until(Duration::from_secs(120), || {
            crashed_rows(&daemon.api("/api/refused")).len() >= HOSTILE.len()
        });
        assert!(daemon.alive(), "round {round}: the daemon died on catch-up");
        assert!(listed, "round {round}: the portal does not list the refusals");
        let (status, _) = daemon.send("GET", "/api/about", &format!("Semlith-Token: {}\r\n", daemon.token), "");
        assert_eq!(status, 200, "round {round}: the daemon does not answer");
        drop(daemon);
    }
    let after: Vec<usize> = HOSTILE.iter().map(|n| begins(&home, n)).collect();
    assert_eq!(before, after, "a refused file was handed to a parser again");

    // doctor names them.
    let out = run(&home, &["doctor"], &cap_off);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(said.contains("crashed a parser and are refused"), "{said}");
}

/// S01, the shipped default: the nesting cap gives the same files no graph
/// and indexes their text, so nothing crashes at all.
#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn s01_deep_nesting_is_indexed_as_text_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let corpus = dir.path().join("corpus");
    std::fs::create_dir_all(&home).unwrap();
    nesting_corpus(&corpus);
    let out = run(&home, &["index", corpus.to_str().unwrap()], &[]);
    assert!(out.status.success(), "{:?}", out.status);
    let out = run(&home, &["files"], &[]);
    let listed = String::from_utf8_lossy(&out.stdout);
    for name in HOSTILE {
        assert!(listed.contains(name), "{name} was not indexed:\n{listed}");
    }
}
