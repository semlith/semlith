//! One test per finding of the 2026-10-14 security audit, each written to fail
//! before its fix landed: S01-S11 on the v0.39.0 binary, S12-S25 on v0.40.0.
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
            PathBuf::from(
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .unwrap(),
            )
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

#[allow(dead_code)]
struct Daemon {
    child: Child,
    port: u16,
    token: String,
    home: PathBuf,
}

#[allow(dead_code)]
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
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
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
        let body = text
            .split_once("\r\n\r\n")
            .map_or("", |(_, b)| b)
            .to_string();
        (status, body)
    }

    fn api(&self, path: &str) -> Value {
        let (status, body) = self.send(
            "GET",
            path,
            &format!("Semlith-Token: {}\r\n", self.token),
            "",
        );
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

/// Every file under `dir`, recursively.
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

// One module per finding, each in its own file, so findings land without
// touching each other's tests.
#[path = "security/races.rs"]
mod races;
#[path = "security/s01.rs"]
mod s01;
#[path = "security/s02.rs"]
mod s02;
#[path = "security/s03.rs"]
mod s03;
#[path = "security/s04.rs"]
mod s04;
#[path = "security/s05.rs"]
mod s05;
#[path = "security/s06.rs"]
mod s06;
#[path = "security/s07.rs"]
mod s07;
#[path = "security/s08.rs"]
mod s08;
#[path = "security/s09.rs"]
mod s09;
#[path = "security/s10.rs"]
mod s10;
#[path = "security/s11.rs"]
mod s11;
#[path = "security/s12.rs"]
mod s12;
#[path = "security/s13.rs"]
mod s13;
#[path = "security/s16.rs"]
mod s16;
