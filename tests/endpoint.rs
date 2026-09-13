//! The MCP endpoint over HTTP, and the two credentials in front of it.
//!
//! From 0.13.0 the daemon answers MCP at `/mcp` as well as over stdio, and the
//! credential that opens it is deliberately not the portal's session token:
//!
//! - The **session token** is per run, cookie-borne, and required by every
//!   `/api/*` route. Rotating it is the Privacy page's button.
//! - The **agent key** is persisted in `~/.semlith/agent.key`, arrives as an
//!   `Authorization: Bearer` header, and opens `/mcp` and nothing else.
//!
//! What is asserted here is the separation itself — that a key in a client's
//! configuration file cannot reach a route that rotates, adopts or upgrades,
//! and that rotating the token does not disconnect an agent. None of these
//! index anything, so none of them needs a model.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct Daemon {
    child: Child,
    port: u16,
    token: String,
    home: std::path::PathBuf,
}

impl Daemon {
    fn start(home: &std::path::Path, args: &[&str]) -> Self {
        std::fs::create_dir_all(home).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_semlith"))
            .arg("start")
            .arg("--port")
            .arg("0")
            .args(args)
            .env("SEMLITH_HOME", home)
            .env("HOME", home)
            .env_remove("SEMLITH_STORE")
            .env_remove("SEMLITH_PORT")
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
            home: home.to_path_buf(),
        }
    }

    fn key(&self) -> String {
        std::fs::read_to_string(self.home.join("agent.key"))
            .expect("the daemon wrote an agent key")
            .trim()
            .to_string()
    }

    /// One request, with whatever credential headers the caller wants.
    fn send(&self, method: &str, path: &str, headers: &str, body: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("the daemon listens");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{headers}\
             Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            body.len(),
        );
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().unwrap();

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let status = text
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (status, text)
    }

    fn with_key(&self, key: &str, body: &str) -> (u16, String) {
        self.send(
            "POST",
            "/mcp",
            &format!("Authorization: Bearer {key}\r\n"),
            body,
        )
    }

    fn with_token(&self, method: &str, path: &str, body: &str) -> (u16, String) {
        self.send(
            method,
            path,
            &format!("Cookie: semlith_token={}\r\n", self.token),
            body,
        )
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const LIST: &str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;

/// The whole point of two credentials: the one a client keeps in a file on
/// disk opens the endpoint and cannot reach anything else.
#[test]
fn the_agent_key_opens_mcp_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(&dir.path().join("home"), &[]);
    let key = daemon.key();
    assert!(key.starts_with("sml_"), "{key} is not shaped like a key");

    let (status, body) = daemon.with_key(&key, LIST);
    assert_eq!(status, 200, "the agent key did not open /mcp: {body}");
    assert!(body.contains("semlith_search"), "no tools listed: {body}");

    // The same key at an /api route is refused: a config file cannot rotate a
    // token, adopt a store or start an upgrade.
    for route in ["/api/stores", "/api/rotate", "/api/adopt", "/api/upgrade"] {
        let (status, _) = daemon.send(
            "POST",
            route,
            &format!("Authorization: Bearer {key}\r\n"),
            "{}",
        );
        assert_eq!(status, 401, "the agent key reached {route}");
    }

    // And no credential, or the wrong one, opens nothing.
    assert_eq!(daemon.send("POST", "/mcp", "", LIST).0, 401);
    assert_eq!(daemon.with_key("sml_not-the-key", LIST).0, 401);
}

/// A `Host` header that is not loopback is refused before the credential is
/// looked at, on `/mcp` exactly as on every portal route.
#[test]
fn a_foreign_host_is_refused_on_the_endpoint_too() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(&dir.path().join("home"), &[]);
    let key = daemon.key();

    let mut stream = TcpStream::connect(("127.0.0.1", daemon.port)).unwrap();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: evil.example\r\nAuthorization: Bearer {key}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{LIST}",
        LIST.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    assert!(
        String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 400"),
        "a foreign Host was not refused"
    );
}

/// The reason the key is persisted: a client configured once keeps working
/// across a restart, and across a rotation of the portal's session token.
#[test]
fn a_configured_client_survives_a_restart_and_a_token_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");

    let key = {
        let daemon = Daemon::start(&home, &[]);
        let key = daemon.key();
        assert_eq!(daemon.with_key(&key, LIST).0, 200);

        // Rotating the session token is the Privacy page's button. It must not
        // touch the endpoint.
        let (status, _) = daemon.with_token("POST", "/api/rotate", "{}");
        assert_eq!(status, 200, "the token did not rotate");
        assert_eq!(
            daemon.with_key(&key, LIST).0,
            200,
            "rotating the session token disconnected the agent"
        );
        key
    };

    // A second daemon on the same home: the key is read, not reminted.
    let daemon = Daemon::start(&home, &[]);
    assert_eq!(daemon.key(), key, "the key changed across a restart");
    assert_eq!(
        daemon.with_key(&key, LIST).0,
        200,
        "the client's stanza went stale across a restart"
    );
}

/// Rotation keeps the previous key valid until the daemon exits, so a session
/// already open finishes rather than failing on the call it was making.
#[test]
fn rotation_keeps_the_previous_key_until_the_daemon_exits() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let daemon = Daemon::start(&home, &[]);
    let first = daemon.key();

    let (status, body) = daemon.with_token("POST", "/api/key", "{}");
    assert_eq!(status, 200, "the key did not rotate: {body}");
    let second = std::fs::read_to_string(home.join("agent.key"))
        .unwrap()
        .trim()
        .to_string();
    assert_ne!(first, second, "rotation returned the same key");

    assert_eq!(daemon.with_key(&second, LIST).0, 200, "the new key failed");
    assert_eq!(
        daemon.with_key(&first, LIST).0,
        200,
        "the previous key was dropped before the daemon exited"
    );

    // `now` is the deliberate version: it drops the previous key at once.
    let (status, _) = daemon.with_token("POST", "/api/key", r#"{"now":true}"#);
    assert_eq!(status, 200);
    assert_eq!(
        daemon.with_key(&second, LIST).0,
        401,
        "--now did not drop the previous key"
    );
}

/// Closing the endpoint drops the route and nothing else.
#[test]
fn closing_the_endpoint_leaves_the_daemon_running() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(&dir.path().join("home"), &[]);
    let key = daemon.key();
    assert_eq!(daemon.with_key(&key, LIST).0, 200);

    let (status, _) = daemon.with_token("POST", "/api/endpoint", r#"{"open":false}"#);
    assert_eq!(status, 200);
    assert_eq!(
        daemon.with_key(&key, LIST).0,
        404,
        "a closed endpoint still answered"
    );
    assert_eq!(
        daemon.with_token("GET", "/api/stores", "").0,
        200,
        "closing the endpoint stopped the portal"
    );

    let (status, _) = daemon.with_token("POST", "/api/endpoint", r#"{"open":true}"#);
    assert_eq!(status, 200);
    assert_eq!(daemon.with_key(&key, LIST).0, 200, "it did not reopen");
}

/// `--no-mcp-http` starts with the endpoint closed, and says so.
#[test]
fn the_flag_starts_with_the_endpoint_closed() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(&dir.path().join("home"), &["--no-mcp-http"]);
    let key = daemon.key();
    assert_eq!(daemon.with_key(&key, LIST).0, 404);
    assert_eq!(daemon.with_token("GET", "/api/stores", "").0, 200);
}

/// The Agents page's list is the daemon's own: a client that has spoken to it
/// appears with the name it gave, its transport and its query count.
#[test]
fn a_connected_client_is_listed_with_what_it_said_about_itself() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(&dir.path().join("home"), &[]);
    let key = daemon.key();

    let hello = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"Test Client"}}}"#;
    let (status, response) = daemon.with_key(&key, hello);
    assert_eq!(status, 200, "initialize failed: {response}");
    let session = response
        .lines()
        .find_map(|l| l.strip_prefix("Mcp-Session-Id: "))
        .map(|s| s.trim().to_string())
        .expect("the server assigns a session id at initialize");

    let call = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"semlith_stats","arguments":{}}}"#;
    daemon.send(
        "POST",
        "/mcp",
        &format!("Authorization: Bearer {key}\r\nMcp-Session-Id: {session}\r\n"),
        call,
    );

    let (_, body) = daemon.with_token("GET", "/api/agents", "");
    assert!(
        body.contains("Test Client"),
        "the client is not listed: {body}"
    );
    assert!(
        body.contains("http /mcp"),
        "the transport is missing: {body}"
    );
    assert!(
        body.contains("\"queries\":1"),
        "the query was not counted: {body}"
    );
    assert!(
        body.contains("2025-06-18"),
        "the negotiated revision is missing: {body}"
    );
}
