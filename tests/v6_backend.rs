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
    fn start(home: &Path) -> Self {
        std::fs::create_dir_all(home).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_semlith"))
            .args(["start", "--port", "0"])
            .env("SEMLITH_HOME", home)
            .env("HOME", home)
            // The login-item seam: anything service.rs would install lands
            // here, never in the real LaunchAgents or systemd user directory.
            .env("SEMLITH_SERVICE_DIR", home.join("service"))
            .env_remove("SEMLITH_STORE")
            .env_remove("SEMLITH_PORT")
            .env_remove("SEMLITH_LEDGER")
            .env_remove("SEMLITH_AIRGAP")
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
        assert_eq!(body["recording"], json!({ "on": false, "reason": "paused" }));
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
        let daemon = Daemon::start(&home);
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
    let daemon = Daemon::start(&home);
    assert_eq!(
        daemon.get("/api/privacy")["airgap"],
        json!({ "on": true, "reason": "runtime" })
    );
    let (_, body) = daemon.post("/api/airgap", json!({ "on": false }));
    assert_eq!(body["airgap"], json!({ "on": false, "reason": null }));
}
