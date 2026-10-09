//! The Semlith Cloud client, end to end against a stub host.
//!
//! The real cloud is not here and is not needed: `common/stub.rs` answers the
//! routes of the Semlith Cloud API with canned JSON and records every
//! request, so these tests assert what left the binary as well as what it did
//! with the answer. Every run has its own HOME and store home; none touches
//! the developer's `~/.semlith`, and none needs a model.

#[path = "common/stub.rs"]
mod stub;

use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Output};
use stub::{Seen, Stub};

const TOKEN: &str = "sml_live_7f3cAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const OTHER: &str = "sml_live_9b2eBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

fn semlith(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(args)
        .env("HOME", home)
        .env("SEMLITH_HOME", home.join(".semlith"))
        .env_remove("SEMLITH_STORE")
        .env_remove("SEMLITH_AIRGAP")
        .current_dir(home)
        .output()
        .expect("semlith runs")
}

fn out(o: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn credentials(home: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(home.join(".semlith/cloud.json")).unwrap())
        .unwrap()
}

fn whoami(slug: &'static str) -> impl Fn(&Seen) -> stub::Answer + Send + Sync {
    move |seen: &Seen| match seen.path.as_str() {
        "/v1/whoami" => stub::json(
            200,
            json!({ "org": { "slug": slug, "plan": "team" }, "token": { "prefix": "sml_live_7f3c" } }),
        ),
        _ => stub::error(404, "not_found", "No such route."),
    }
}

/// A device flow whose token endpoint answers `polls` in order, then fails.
fn device(polls: Vec<stub::Answer>) -> Stub {
    let polls = std::sync::Mutex::new(polls.into_iter());
    Stub::start(move |seen| match seen.path.as_str() {
        "/v1/cli/authorize" => stub::json(
            200,
            json!({
                "device_code": "dev-1", "user_code": "WDJB-MJHT",
                "verification_uri": "https://cloud.example/cli",
                "verification_uri_complete": "https://cloud.example/cli?code=WDJB-MJHT",
                "interval": 0, "expires_in": 600,
            }),
        ),
        "/v1/cli/token" => polls
            .lock()
            .unwrap()
            .next()
            .unwrap_or_else(|| stub::error(500, "test", "polled once too often")),
        _ => stub::error(404, "not_found", "No such route."),
    })
}

fn approved() -> stub::Answer {
    stub::json(
        200,
        json!({ "token": TOKEN, "org": "acme", "host": "https://elsewhere.example" }),
    )
}

#[test]
fn device_login_polls_until_approved_and_saves_owner_only() {
    let home = tempfile::tempdir().unwrap();
    let host = device(vec![
        stub::error(400, "authorization_pending", "Waiting."),
        stub::error(400, "authorization_pending", "Waiting."),
        approved(),
    ]);
    let o = semlith(
        home.path(),
        &["cloud", "login", "acme", "--host", &host.url],
    );
    assert!(o.status.success(), "{}", out(&o));
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("WDJB-MJHT"),
        "{}",
        out(&o)
    );
    // Success once: three polls, and nothing after the one that answered.
    assert_eq!(host.to("/v1/cli/token").len(), 3);
    let authorize = &host.to("/v1/cli/authorize")[0];
    assert!(
        authorize.json()["client"]
            .as_str()
            .unwrap()
            .starts_with("semlith/")
    );
    assert!(authorize.json()["host_name"].is_string());
    for seen in host.seen() {
        let ua = seen.header("user-agent").unwrap_or_default();
        assert!(ua.starts_with("semlith/") && ua.contains("; "), "{ua}");
        // No token exists yet, so none may have been sent.
        assert!(seen.header("authorization").is_none());
    }

    let saved = credentials(home.path());
    assert_eq!(saved["entries"][0]["org"], "acme");
    // Bound to the host the flow ran against, not the one the reply named.
    assert_eq!(saved["entries"][0]["host"], json!(host.url));
    assert!(
        !String::from_utf8_lossy(&o.stdout).contains(TOKEN),
        "the token was printed"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.path().join(".semlith/cloud.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn slow_down_adds_five_seconds_to_the_interval() {
    let home = tempfile::tempdir().unwrap();
    let host = device(vec![stub::error(400, "slow_down", "Too fast."), approved()]);
    let started = std::time::Instant::now();
    let o = semlith(home.path(), &["cloud", "login", "--host", &host.url]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(started.elapsed() >= std::time::Duration::from_secs(5));
    assert_eq!(host.to("/v1/cli/token").len(), 2);
}

#[test]
fn a_denied_or_expired_code_saves_nothing() {
    for (answer, says) in [
        (stub::error(400, "access_denied", "Denied."), "denied"),
        (stub::error(410, "expired", "Expired."), "expired"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let host = device(vec![answer]);
        let o = semlith(home.path(), &["cloud", "login", "--host", &host.url]);
        assert!(!o.status.success(), "{}", out(&o));
        assert!(
            String::from_utf8_lossy(&o.stderr).contains(says),
            "{}",
            out(&o)
        );
        assert!(!home.path().join(".semlith/cloud.json").exists());
        assert_eq!(host.to("/v1/cli/token").len(), 1);
    }
}

#[test]
fn a_pasted_token_is_checked_before_it_is_saved() {
    let home = tempfile::tempdir().unwrap();
    let host = Stub::start(whoami("acme"));

    let o = semlith(
        home.path(),
        &[
            "cloud", "login", "acme", "--host", &host.url, "--token", "nope",
        ],
    );
    assert!(!o.status.success());
    assert!(
        host.seen().is_empty(),
        "a token that is not one was sent somewhere"
    );

    let o = semlith(
        home.path(),
        &[
            "cloud", "login", "globex", "--host", &host.url, "--token", TOKEN,
        ],
    );
    assert!(!o.status.success(), "{}", out(&o));
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("belongs to acme"),
        "{}",
        out(&o)
    );
    assert!(!home.path().join(".semlith/cloud.json").exists());

    let o = semlith(
        home.path(),
        &[
            "cloud", "login", "acme", "--host", &host.url, "--token", TOKEN,
        ],
    );
    assert!(o.status.success(), "{}", out(&o));
    let whoami = host.to("/v1/whoami");
    assert_eq!(
        whoami.last().unwrap().header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert_eq!(credentials(home.path())["entries"][0]["plan"], "team");
}

/// Two hosts, two tokens: each token reaches only the host that issued it.
#[test]
fn a_token_is_sent_to_its_own_host_only() {
    let home = tempfile::tempdir().unwrap();
    let status = |slug: &'static str| {
        move |seen: &Seen| match seen.path.as_str() {
            "/v1/whoami" => stub::json(200, json!({ "org": { "slug": slug, "plan": "pro" } })),
            p if p.starts_with("/v1/orgs/") => stub::json(
                200,
                json!({ "org": { "slug": slug, "name": slug, "plan": "pro", "status": "active" },
                        "mcp_url": "https://cloud.example/x/mcp", "stores": [], "usage": {} }),
            ),
            _ => stub::error(404, "not_found", "No."),
        }
    };
    let a = Stub::start(status("acme"));
    let b = Stub::start(status("globex"));
    assert!(
        semlith(
            home.path(),
            &["cloud", "login", "--host", &a.url, "--token", TOKEN]
        )
        .status
        .success()
    );
    assert!(
        semlith(
            home.path(),
            &["cloud", "login", "--host", &b.url, "--token", OTHER]
        )
        .status
        .success()
    );

    let o = semlith(home.path(), &["cloud", "status", "acme"]);
    assert!(o.status.success(), "{}", out(&o));
    let o = semlith(home.path(), &["cloud", "status", "globex", "--json"]);
    assert!(o.status.success(), "{}", out(&o));
    let printed: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(printed["cloud_version"], "0.3.0");

    for seen in a.seen() {
        assert!(
            !seen
                .header("authorization")
                .unwrap_or_default()
                .contains(OTHER)
        );
    }
    for seen in b.seen() {
        assert!(
            !seen
                .header("authorization")
                .unwrap_or_default()
                .contains(TOKEN)
        );
    }
    assert_eq!(a.to("/v1/orgs/acme/status").len(), 1);
    assert_eq!(b.to("/v1/orgs/globex/status").len(), 1);
    assert!(b.to("/v1/orgs/acme").is_empty());
}

#[test]
fn the_hosts_sentence_is_printed_with_the_next_step() {
    let home = tempfile::tempdir().unwrap();
    let host = Stub::start(|seen: &Seen| match seen.path.as_str() {
        "/v1/whoami" => stub::json(200, json!({ "org": { "slug": "acme" } })),
        _ => stub::json(
            426,
            json!({ "error": { "code": "client_too_old", "message": "This client is too old.", "min_version": "9.0.0" } }),
        ),
    });
    assert!(
        semlith(
            home.path(),
            &["cloud", "login", "--host", &host.url, "--token", TOKEN]
        )
        .status
        .success()
    );
    let o = semlith(home.path(), &["cloud", "status"]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("This client is too old.") && err.contains("semlith upgrade"),
        "{err}"
    );
    assert!(!err.contains(TOKEN));
}

#[test]
fn logout_forgets_the_token_and_airgap_reaches_nothing() {
    let home = tempfile::tempdir().unwrap();
    let host = Stub::start(whoami("acme"));
    assert!(
        semlith(
            home.path(),
            &["cloud", "login", "--host", &host.url, "--token", TOKEN]
        )
        .status
        .success()
    );
    let before = host.seen().len();

    let o = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["cloud", "status"])
        .env("HOME", home.path())
        .env("SEMLITH_HOME", home.path().join(".semlith"))
        .env("SEMLITH_AIRGAP", "1")
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("airgap"),
        "{}",
        out(&o)
    );
    assert_eq!(host.seen().len(), before, "airgap still reached the host");

    let o = semlith(home.path(), &["cloud", "logout", "acme"]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(!home.path().join(".semlith/cloud.json").exists());
    let o = semlith(home.path(), &["cloud", "status"]);
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("not signed in"),
        "{}",
        out(&o)
    );
}

// ------------------------------------------------------------ remote stores

/// A host with two stores, `platform` and `docs`, whose status names an MCP
/// URL on `mcp_host` — another origin when a test wants one.
fn org_host(mcp_host: Option<String>) -> Stub {
    Stub::start(move |seen: &Seen| {
        let path = seen.path.split('?').next().unwrap_or_default();
        match path {
            "/v1/whoami" => stub::json(200, json!({ "org": { "slug": "acme", "plan": "team" } })),
            "/v1/orgs/acme/status" => stub::json(
                200,
                json!({
                    "org": { "slug": "acme", "name": "Acme", "plan": "team", "status": "active", "seats": 8, "seats_used": 6 },
                    "mcp_url": format!("{}/acme/mcp", mcp_host.clone().unwrap_or_else(|| "SELF".into())),
                    "stores": [
                        { "name": "platform", "state": "fresh", "files": 10, "chunks": 90,
                          "sources": [{ "label": "acme/api", "kind": "github", "revision": "a41c9e2", "behind_seconds": 38, "state": "fresh" }] },
                        { "name": "docs", "state": "fresh", "files": 3, "chunks": 9, "sources": [] }
                    ],
                    "usage": { "month": "2026-10", "store_bytes": 1000, "cap_bytes": 8000, "index_chunks": 12400, "cap_chunks": 100000, "queue": { "running": 0, "waiting": 0 } }
                }),
            ),
            _ => stub::error(404, "not_found", "No such route."),
        }
    })
}

fn signed_in(home: &Path, host: &Stub) {
    let o = semlith(
        home,
        &["cloud", "login", "--host", &host.url, "--token", TOKEN],
    );
    assert!(o.status.success(), "{}", out(&o));
}

fn registry(home: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(home.join(".semlith/registry.json")).unwrap())
        .unwrap()
}

/// What `Registry::save` writes for one local store, byte for byte.
const LOCAL_REGISTRY: &str = r#"{
  "stores": {
    "notes": {
      "roots": [
        "/nowhere/notes"
      ],
      "model": "granite",
      "created": 1,
      "kind": "both",
      "lean": "either",
      "watch": true,
      "record": true,
      "gitignore": true
    }
  },
  "trusted": []
}
"#;

#[test]
fn connect_then_disconnect_leaves_the_registry_byte_for_byte() {
    let home = tempfile::tempdir().unwrap();
    let host = org_host(None);
    signed_in(home.path(), &host);
    let path = home.path().join(".semlith/registry.json");
    std::fs::write(&path, LOCAL_REGISTRY).unwrap();

    let o = semlith(home.path(), &["cloud", "connect", "acme"]);
    assert!(o.status.success(), "{}", out(&o));
    let reg = registry(home.path());
    let platform = &reg["remote"]["acme/platform"];
    assert_eq!(platform["org"], "acme");
    assert_eq!(platform["store"], "platform");
    // The status named `SELF/acme/mcp`, which is not this host: the token's
    // own host is kept instead.
    assert_eq!(platform["mcp_url"], json!(format!("{}/acme/mcp", host.url)));
    assert!(reg["remote"]["acme/docs"].is_object());
    // Never a directory: not in `stores`, and nothing made under the home.
    assert!(reg["stores"].get("acme/platform").is_none());
    assert!(!home.path().join(".semlith/stores/acme-platform").exists());

    // Cloud 0.4.0's usage is in chunks, and the status line says so.
    let o = semlith(home.path(), &["cloud", "status", "acme"]);
    assert!(o.status.success(), "{}", out(&o));
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(text.contains("12,400 of 100,000 chunks indexed"), "{text}");

    let o = semlith(home.path(), &["cloud", "disconnect", "acme"]);
    assert!(o.status.success(), "{}", out(&o));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), LOCAL_REGISTRY);
}

#[test]
fn connect_takes_only_the_stores_named_and_refuses_one_it_cannot_reach() {
    let home = tempfile::tempdir().unwrap();
    let host = org_host(None);
    signed_in(home.path(), &host);
    let o = semlith(
        home.path(),
        &["cloud", "connect", "acme", "--store", "nope"],
    );
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("platform, docs"),
        "{}",
        out(&o)
    );
    let o = semlith(
        home.path(),
        &["cloud", "connect", "acme", "--store", "docs"],
    );
    assert!(o.status.success(), "{}", out(&o));
    let reg = registry(home.path());
    assert!(reg["remote"]["acme/docs"].is_object());
    assert!(reg["remote"].get("acme/platform").is_none());

    // Listed with the badge by the read commands, with no local store at all.
    for command in ["stats", "files"] {
        let o = semlith(home.path(), &[command]);
        assert!(o.status.success(), "{command}: {}", out(&o));
        assert!(
            String::from_utf8_lossy(&o.stdout).contains("acme/docs  remote · acme"),
            "{}",
            out(&o)
        );
    }
}

/// A daemon on a redirected home, as `tests/endpoint.rs` starts one.
struct Daemon {
    child: std::process::Child,
    port: u16,
    token: String,
    home: std::path::PathBuf,
}

impl Daemon {
    fn start(home: &Path) -> Self {
        Self::start_with(home, &[])
    }

    fn start_with(home: &Path, env: &[(&str, &std::ffi::OsStr)]) -> Self {
        use std::io::BufRead;
        let mut child = Command::new(env!("CARGO_BIN_EXE_semlith"))
            .args(["start", "--port", "0"])
            .envs(env.iter().copied())
            .env("SEMLITH_HOME", home.join(".semlith"))
            .env("HOME", home)
            .env_remove("SEMLITH_STORE")
            .env_remove("SEMLITH_PORT")
            .env_remove("SEMLITH_AIRGAP")
            .current_dir(home)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("semlith start runs");
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.as_mut().unwrap())
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

    fn send(&self, method: &str, path: &str, headers: &str, body: &str) -> (u16, String) {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .unwrap();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{headers}Content-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            body.len()
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
        let body = text
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default();
        (status, body)
    }

    fn api(&self, method: &str, path: &str, body: &str) -> (u16, String) {
        self.send(
            method,
            path,
            &format!("Semlith-Token: {}\r\n", self.token),
            body,
        )
    }

    fn json(&self, path: &str) -> Value {
        let (status, body) = self.api("GET", path, "");
        assert_eq!(status, 200, "{path}: {body}");
        serde_json::from_str(&body).unwrap_or_else(|_| panic!("{path}: {body}"))
    }

    fn tool(&self, name: &str, arguments: Value) -> Value {
        let key = std::fs::read_to_string(self.home.join(".semlith/agent.key")).unwrap();
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                           "params": { "name": name, "arguments": arguments } });
        let (status, text) = self.send(
            "POST",
            "/mcp",
            &format!(
                "Authorization: Bearer {}\r\nSemlith-Host: Claude Code\r\n",
                key.trim()
            ),
            &body.to_string(),
        );
        assert_eq!(status, 200, "{text}");
        serde_json::from_str(&text).unwrap()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn tool_text(answer: &Value) -> String {
    answer["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn every_write_path_refuses_a_remote_store_with_the_reason() {
    let home = tempfile::tempdir().unwrap();
    let host = org_host(None);
    signed_in(home.path(), &host);
    assert!(
        semlith(home.path(), &["cloud", "connect", "acme"])
            .status
            .success()
    );

    for args in [
        vec!["drop", "acme/platform", "--yes"],
        vec!["index", "--name", "acme/platform", "."],
    ] {
        let o = semlith(home.path(), &args);
        assert!(!o.status.success(), "{args:?}: {}", out(&o));
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("is a remote store"),
            "{args:?}: {}",
            out(&o)
        );
    }

    let daemon = Daemon::start(home.path());
    for (route, body) in [
        (
            "/api/index",
            json!({ "store": "acme/platform", "path": ["."] }),
        ),
        (
            "/api/add",
            json!({ "store": "acme/platform", "url": "https://example.com" }),
        ),
        (
            "/api/forget",
            json!({ "store": "acme/platform", "path": "a.md" }),
        ),
        ("/api/store/compact", json!({ "store": "acme/platform" })),
        (
            "/api/store/settings",
            json!({ "store": "acme/platform", "watch": false }),
        ),
        ("/api/store/delete", json!({ "store": "acme/platform" })),
    ] {
        let (status, text) = daemon.api("POST", route, &body.to_string());
        assert_eq!(status, 400, "{route}: {text}");
        assert!(text.contains("is a remote store"), "{route}: {text}");
    }
    for (tool, args) in [
        (
            "semlith_index",
            json!({ "store": "acme/platform", "paths": ["."] }),
        ),
        (
            "semlith_forget",
            json!({ "store": "acme/platform", "path": "a.md" }),
        ),
        (
            "semlith_add",
            json!({ "store": "acme/platform", "url": "https://example.com" }),
        ),
    ] {
        let answer = daemon.tool(tool, args);
        assert_eq!(answer["result"]["isError"], true, "{tool}: {answer}");
        assert!(
            tool_text(&answer).contains("is a remote store"),
            "{tool}: {answer}"
        );
    }

    // And the read surfaces list them, with the badge.
    let stores = daemon.json("/api/stores");
    assert_eq!(stores["remote"][0]["badge"], "remote · acme", "{stores}");
    let stats = tool_text(&daemon.tool("semlith_stats", json!({})));
    assert!(stats.contains("acme/platform: remote · acme"), "{stats}");
    // Writes refused, so the registry still holds no local store.
    assert!(
        registry(home.path())["stores"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}

// ------------------------------------------------------------ merged answers

/// An org host that also answers search and MCP.
fn answering_host() -> Stub {
    Stub::start(move |seen: &Seen| {
        let path = seen.path.split('?').next().unwrap_or_default();
        let hit = |score: f64, store: &str, file: &str, line: &str| {
            json!({ "score": score, "store": store, "path": file, "start_line": 3, "end_line": 9,
                    "text": line, "lists": ["vector", "keyword"], "symbol": "order_total",
                    "symbol_kind": "function", "fresh": true, "source": "acme/api",
                    "revision": "a41c9e2", "behind_seconds": 38 })
        };
        match path {
            "/v1/whoami" => stub::json(200, json!({ "org": { "slug": "acme", "plan": "team" } })),
            "/v1/orgs/acme/status" => stub::json(
                200,
                json!({ "org": { "slug": "acme" }, "mcp_url": "SELF",
                        "stores": [{ "name": "platform" }, { "name": "docs" }] }),
            ),
            "/v1/orgs/acme/search" => stub::json(
                200,
                json!({ "hits": [
                    hit(0.020, "platform", "api/src/low.rs", "fn low_total() {}"),
                    hit(0.031, "platform", "api/src/total.rs", "fn order_total() -> Money {"),
                    hit(0.025, "docs", "runbook.md", "The order total is computed in pricing."),
                    hit(0.040, "elsewhere", "not/asked.rs", "a store nobody asked about"),
                ] }),
            ),
            "/acme/mcp" => {
                let call = seen.json();
                let tool = call["params"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let store = call["params"]["arguments"]["store"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                stub::json(
                    200,
                    json!({ "jsonrpc": "2.0", "id": 1, "result": { "content": [
                        { "type": "text", "text": format!("{tool} answered by {store} @ a41c9e2") }
                    ] } }),
                )
            }
            _ => stub::error(404, "not_found", "No such route."),
        }
    })
}

fn connected(home: &Path) -> Stub {
    let host = answering_host();
    signed_in(home, &host);
    let o = semlith(home, &["cloud", "connect", "acme"]);
    assert!(o.status.success(), "{}", out(&o));
    host
}

#[test]
fn a_search_naming_remote_stores_is_one_list_by_score_with_provenance() {
    let home = tempfile::tempdir().unwrap();
    let host = connected(home.path());
    let daemon = Daemon::start(home.path());

    let answer = daemon.tool(
        "semlith_search",
        json!({ "query": "where is the order total computed", "k": 2, "store": ["acme/platform", "acme/docs"] }),
    );
    let text = tool_text(&answer);
    // k honoured, by score: total.rs (0.031) then runbook.md (0.025); low.rs
    // and the store nobody asked about are not shown.
    let total = text.find("api/src/total.rs").expect(&text);
    let runbook = text.find("runbook.md").expect(&text);
    assert!(total < runbook, "{text}");
    assert!(
        !text.contains("low.rs") && !text.contains("not/asked.rs"),
        "{text}"
    );
    assert!(
        text.contains("remote · acme · a41c9e2 · 38 s behind"),
        "{text}"
    );

    let asked = host.to("/v1/orgs/acme/search");
    assert_eq!(asked.len(), 1, "one request per org, not per store");
    let body = asked[0].json();
    let mut stores: Vec<&str> = body["stores"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    stores.sort();
    assert_eq!(stores, ["docs", "platform"]);
    assert_eq!(body["k"], 2);
    // The agent is named, not the daemon.
    // No initialize, so the host the proxy header named is the client.
    assert_eq!(asked[0].header("semlith-client"), Some("Claude Code"));
    assert!(asked[0].header("semlith-session").is_some());
    assert_eq!(
        asked[0].header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
}

#[test]
fn another_tool_naming_a_remote_store_is_forwarded_and_appended() {
    let home = tempfile::tempdir().unwrap();
    let host = connected(home.path());
    let daemon = Daemon::start(home.path());
    let answer = daemon.tool(
        "semlith_symbol",
        json!({ "name": "order_total", "store": "acme/platform" }),
    );
    let text = tool_text(&answer);
    assert!(text.contains("acme/platform (remote · acme)"), "{text}");
    assert!(
        text.contains("semlith_symbol answered by platform @ a41c9e2"),
        "{text}"
    );
    let mcp = host.to("/acme/mcp");
    assert_eq!(mcp.len(), 1);
    assert_eq!(mcp[0].path, "/acme/mcp?store=platform");
    assert!(mcp[0].header("semlith-session").is_some());
    // A tool naming no store stays local: nothing more reached the host.
    daemon.tool("semlith_symbol", json!({ "name": "order_total" }));
    assert_eq!(host.to("/acme/mcp").len(), 1);
}

#[test]
fn the_portal_search_merges_remote_rows_with_their_fields() {
    let home = tempfile::tempdir().unwrap();
    let _host = connected(home.path());
    let daemon = Daemon::start(home.path());
    let answer = daemon.json("/api/search?query=order%20total&k=2&store=acme/platform");
    let hits = answer["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 2, "{answer}");
    assert_eq!(hits[0]["path"], "api/src/total.rs");
    assert_eq!(hits[0]["store"], "acme/platform");
    assert_eq!(hits[0]["badge"], "remote · acme");
    assert_eq!(hits[0]["revision"], "a41c9e2");
    assert_eq!(hits[0]["behind_seconds"], 38);
    assert!(answer.get("remote_skipped").is_none());

    let symbol = daemon.json("/api/symbol?name=order_total&store=acme/platform");
    assert_eq!(symbol["remote"][0]["store"], "acme/platform", "{symbol}");
}

/// The host goes away: the answer still comes, with one line naming the
/// stores it could not ask and why.
#[test]
fn an_unreachable_host_costs_one_line_and_nothing_else() {
    let home = tempfile::tempdir().unwrap();
    let _host = connected(home.path());
    // Re-point both files at a port nothing listens on.
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    for file in ["cloud.json", "registry.json"] {
        let path = home.path().join(".semlith").join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace(&_host.url, &dead);
        std::fs::write(&path, text).unwrap();
    }
    let daemon = Daemon::start(home.path());
    let answer = daemon.tool("semlith_search", json!({ "query": "order total" }));
    let text = tool_text(&answer);
    assert_ne!(answer["result"]["isError"], true, "{answer}");
    assert!(
        text.contains("remote stores skipped: acme/docs, acme/platform ("),
        "{text}"
    );
    assert!(text.contains("could not be reached"), "{text}");
    assert_eq!(text.matches("remote stores skipped").count(), 1);

    let portal = daemon.json("/api/search?query=order%20total");
    assert!(
        portal["remote_skipped"]
            .as_str()
            .unwrap()
            .contains("could not be reached"),
        "{portal}"
    );
    assert!(portal["hits"].as_array().unwrap().is_empty());
}

/// Signed in with nothing connected: a search asks the host nothing.
#[test]
fn signed_in_but_not_connected_reaches_nothing() {
    let home = tempfile::tempdir().unwrap();
    let host = answering_host();
    signed_in(home.path(), &host);
    let before = host.seen().len();
    let daemon = Daemon::start(home.path());
    daemon.json("/api/stores");
    daemon.json("/api/search?query=order%20total");
    assert_eq!(host.seen().len(), before);
}

/// The model the developer's machine already has, read where it is, so the
/// ignored test below does not download 52 MB into a temporary home.
fn model_cache() -> std::path::PathBuf {
    std::env::var_os("SEMLITH_MODEL_CACHE")
        .map(Into::into)
        .unwrap_or_else(|| {
            Path::new(&std::env::var_os("HOME").unwrap()).join(".cache/semlith/models")
        })
}

/// One local store and one remote store, one list with both chips; then the
/// host goes away and the local answer is still whole.
#[test]
#[ignore = "indexes a local store, so it needs the embedding model"]
fn a_local_and_a_remote_store_answer_as_one_list() {
    let home = tempfile::tempdir().unwrap();
    let host = connected(home.path());
    let corpus = home.path().join("notes");
    std::fs::create_dir_all(&corpus).unwrap();
    std::fs::write(
        corpus.join("pricing.md"),
        "The order total is the sum of the line totals, then tax.",
    )
    .unwrap();
    let cache = model_cache();
    let o = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["index", "--name", "notes"])
        .arg(&corpus)
        .env("HOME", home.path())
        .env("SEMLITH_HOME", home.path().join(".semlith"))
        .env("SEMLITH_MODEL_CACHE", &cache)
        .env_remove("SEMLITH_STORE")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", out(&o));

    let daemon = Daemon::start_with(home.path(), &[("SEMLITH_MODEL_CACHE", cache.as_os_str())]);
    let answer = daemon.tool(
        "semlith_search",
        json!({ "query": "how is the order total computed", "k": 8 }),
    );
    let text = tool_text(&answer);
    assert!(text.contains("notes "), "the local store's chip: {text}");
    assert!(text.contains("pricing.md"), "{text}");
    assert!(
        text.contains("acme/platform (remote · acme"),
        "the remote chip: {text}"
    );
    assert!(!text.contains("remote stores skipped"), "{text}");
    assert_eq!(host.to("/v1/orgs/acme/search").len(), 1);

    let portal = daemon.json("/api/search?query=order%20total");
    let stores: Vec<&str> = portal["hits"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|h| h["store"].as_str())
        .collect();
    assert!(
        stores.contains(&"notes") && stores.contains(&"acme/platform"),
        "{portal}"
    );
    drop(daemon);

    // The host is gone: the local answer is unchanged but for the line.
    for file in ["cloud.json", "registry.json"] {
        let path = home.path().join(".semlith").join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace(&host.url, "http://127.0.0.1:9");
        std::fs::write(&path, text).unwrap();
    }
    let daemon = Daemon::start_with(home.path(), &[("SEMLITH_MODEL_CACHE", cache.as_os_str())]);
    let text = tool_text(&daemon.tool(
        "semlith_search",
        json!({ "query": "how is the order total computed", "k": 8 }),
    ));
    assert!(text.contains("pricing.md"), "{text}");
    assert!(
        text.contains("remote stores skipped: acme/docs, acme/platform ("),
        "{text}"
    );
}

// --------------------------------------------------------- command line

/// `semlith` with the developer's model cache, for a run that loads the model.
fn semlith_model(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(args)
        .env("HOME", home)
        .env("SEMLITH_HOME", home.join(".semlith"))
        .env("SEMLITH_MODEL_CACHE", model_cache())
        .env_remove("SEMLITH_STORE")
        .env_remove("SEMLITH_AIRGAP")
        .current_dir(home)
        .output()
        .expect("semlith runs")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Re-point the credentials and the registry at a port nothing listens on.
fn host_gone(home: &Path, host: &Stub) {
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    for file in ["cloud.json", "registry.json"] {
        let path = home.join(".semlith").join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace(&host.url, &dead);
        std::fs::write(&path, text).unwrap();
    }
}

/// Only remote stores, no local one: the command line answers from them,
/// labelled with where each hit came from, and loads no model to do it.
#[test]
fn the_cli_searches_remote_stores_with_no_local_one() {
    let home = tempfile::tempdir().unwrap();
    let host = connected(home.path());
    let o = semlith(home.path(), &["search", "-k", "2", "order total"]);
    assert!(o.status.success(), "{}", out(&o));
    let text = stdout(&o);
    let total = text.find("api/src/total.rs").expect(&text);
    let runbook = text.find("runbook.md").expect(&text);
    assert!(total < runbook, "by score: {text}");
    assert!(
        !text.contains("low.rs") && !text.contains("not/asked.rs"),
        "{text}"
    );
    assert!(
        text.contains("[acme/platform (remote · acme · a41c9e2 · 38 s behind)]"),
        "{text}"
    );
    let asked = host.to("/v1/orgs/acme/search");
    assert_eq!(asked.len(), 1, "one request per org");
    assert_eq!(asked[0].header("semlith-client"), Some("cli"));

    // `--json` rows carry the remote fields `/api/search` adds.
    let o = semlith(home.path(), &["search", "--json", "-k", "2", "order total"]);
    assert!(o.status.success(), "{}", out(&o));
    let rows: Value =
        serde_json::from_str(&stdout(&o)).unwrap_or_else(|e| panic!("{e}: {}", out(&o)));
    assert_eq!(rows[0]["path"], "api/src/total.rs", "{rows}");
    assert_eq!(rows[0]["store"], "acme/platform");
    assert_eq!(rows[0]["remote"], "acme");
    assert_eq!(rows[0]["badge"], "remote · acme");
    assert_eq!(rows[0]["revision"], "a41c9e2");
    assert_eq!(rows[0]["behind_seconds"], 38);
    assert_eq!(rows.as_array().unwrap().len(), 2);
}

/// `-s <org>/<store>` names that remote store and asks for it alone.
#[test]
fn the_cli_store_flag_names_a_remote_store() {
    let home = tempfile::tempdir().unwrap();
    let host = connected(home.path());
    let o = semlith(
        home.path(),
        &["search", "-s", "acme/platform", "order total"],
    );
    assert!(o.status.success(), "{}", out(&o));
    let text = stdout(&o);
    assert!(text.contains("api/src/total.rs"), "{text}");
    assert!(!text.contains("runbook.md"), "docs was not asked: {text}");
    let asked = host.to("/v1/orgs/acme/search");
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].json()["stores"], json!(["platform"]));

    // `stats` and `files` say what a remote store is rather than erroring.
    for command in ["stats", "files"] {
        let o = semlith(home.path(), &[command, "-s", "acme/platform"]);
        assert!(o.status.success(), "{command}: {}", out(&o));
        let text = stdout(&o);
        assert!(
            text.contains("acme/platform  remote · acme"),
            "{command}: {text}"
        );
        assert!(!text.contains("acme/docs"), "{command}: {text}");
    }
}

/// The host goes away: one line says which stores were skipped and why, and
/// the command still answers.
#[test]
fn the_cli_skips_an_unreachable_host_in_one_line() {
    let home = tempfile::tempdir().unwrap();
    let host = connected(home.path());
    host_gone(home.path(), &host);
    let o = semlith(home.path(), &["search", "order total"]);
    assert!(o.status.success(), "{}", out(&o));
    let err = stderr(&o);
    assert!(
        err.contains("remote stores skipped: acme/docs, acme/platform ("),
        "{err}"
    );
    assert!(err.contains("could not be reached"), "{err}");
    assert_eq!(err.matches("remote stores skipped").count(), 1, "{err}");
    assert!(err.contains("No match in the remote stores."), "{err}");
}

fn index_notes(home: &Path) {
    let corpus = home.join("notes");
    std::fs::create_dir_all(&corpus).unwrap();
    std::fs::write(
        corpus.join("pricing.md"),
        "The order total is the sum of the line totals, then tax.",
    )
    .unwrap();
    let o = semlith_model(
        home,
        &["index", "--name", "notes", corpus.to_str().unwrap()],
    );
    assert!(o.status.success(), "{}", out(&o));
}

/// A local and a remote store from the command line: one list by score
/// under one `k`; the host going away leaves the local answer whole; and a
/// machine with no remote store prints the same `--json` bytes it always did.
#[test]
#[ignore = "indexes a local store, so it needs the embedding model"]
fn the_cli_merges_local_and_remote_hits_by_score() {
    let home = tempfile::tempdir().unwrap();
    index_notes(home.path());
    let query = "how is the order total computed";
    let before = semlith_model(home.path(), &["search", "--json", query]);
    assert!(before.status.success(), "{}", out(&before));

    let host = connected(home.path());
    let o = semlith_model(home.path(), &["search", "--json", "-k", "3", query]);
    assert!(o.status.success(), "{}", out(&o));
    let rows: Value =
        serde_json::from_str(&stdout(&o)).unwrap_or_else(|e| panic!("{e}: {}", out(&o)));
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 3, "{rows:?}");
    let scores: Vec<f64> = rows.iter().map(|r| r["score"].as_f64().unwrap()).collect();
    assert!(
        scores.windows(2).all(|w| w[0] >= w[1]),
        "by score: {scores:?}"
    );
    assert!(
        rows.iter()
            .any(|r| r["path"].as_str().unwrap().ends_with("pricing.md")),
        "{rows:?}"
    );
    assert!(rows.iter().any(|r| r["remote"] == "acme"), "{rows:?}");

    let o = semlith_model(home.path(), &["search", "-k", "8", query]);
    assert!(o.status.success(), "{}", out(&o));
    let text = stdout(&o);
    assert!(text.contains("[notes] "), "the local store's label: {text}");
    assert!(text.contains("pricing.md"), "{text}");
    assert!(text.contains("[acme/platform (remote · acme"), "{text}");
    assert!(stderr(&o).contains("across 3 stores"), "{}", out(&o));

    // `-s` names a local and a remote store side by side.
    let notes = home.path().join(".semlith/stores/notes");
    let o = semlith_model(
        home.path(),
        &[
            "search",
            "-s",
            notes.to_str().unwrap(),
            "-s",
            "acme/docs",
            query,
        ],
    );
    assert!(o.status.success(), "{}", out(&o));
    let text = stdout(&o);
    assert!(
        text.contains("pricing.md") && text.contains("runbook.md"),
        "{text}"
    );
    assert!(!text.contains("api/src/total.rs"), "{text}");

    host_gone(home.path(), &host);
    let o = semlith_model(home.path(), &["search", query]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(stdout(&o).contains("pricing.md"), "{}", out(&o));
    assert!(
        stderr(&o).contains("remote stores skipped: acme/docs, acme/platform ("),
        "{}",
        out(&o)
    );

    let o = semlith(home.path(), &["cloud", "disconnect", "acme"]);
    assert!(o.status.success(), "{}", out(&o));
    let after = semlith_model(home.path(), &["search", "--json", query]);
    assert!(after.status.success(), "{}", out(&after));
    assert_eq!(
        stdout(&before),
        stdout(&after),
        "no remote store, same bytes"
    );
}

// ------------------------------------------------------------------- push

/// A host holding one upload source: it answers a manifest with the paths
/// whose hashes it does not hold, and holds what a commit committed.
fn upload_host() -> Stub {
    let held: std::sync::Mutex<std::collections::HashMap<String, String>> = Default::default();
    let pending: std::sync::Mutex<Vec<(String, String)>> = Default::default();
    Stub::start(move |seen: &Seen| {
        let path = seen.path.split('?').next().unwrap_or_default().to_string();
        match (seen.method.as_str(), path.as_str()) {
            (_, "/v1/whoami") => {
                stub::json(200, json!({ "org": { "slug": "acme", "plan": "pro" } }))
            }
            ("POST", "/v1/orgs/acme/stores/infra/pushes") => {
                let files = seen.json()["files"].as_array().cloned().unwrap_or_default();
                let held = held.lock().unwrap();
                let wanted: Vec<(String, String)> = files
                    .iter()
                    .map(|f| {
                        (
                            f["path"].as_str().unwrap().to_string(),
                            f["sha256"].as_str().unwrap().to_string(),
                        )
                    })
                    .collect();
                let need: Vec<&String> = wanted
                    .iter()
                    .filter(|(p, h)| held.get(p) != Some(h))
                    .map(|(p, _)| p)
                    .collect();
                let need = json!(need);
                *pending.lock().unwrap() = wanted;
                stub::json(
                    201,
                    json!({ "push": "p_1", "need": need, "remove": [], "estimate_chunks": 1400 }),
                )
            }
            ("PUT", "/v1/orgs/acme/pushes/p_1/files") => (204, vec![], vec![]),
            ("POST", "/v1/orgs/acme/pushes/p_1/commit") => {
                let mut held = held.lock().unwrap();
                for (p, h) in pending.lock().unwrap().drain(..) {
                    held.insert(p, h);
                }
                stub::json(202, json!({ "job": 48121, "position": 2 }))
            }
            ("GET", "/v1/orgs/acme/jobs/48121") => stub::json(
                200,
                json!({ "job": 48121, "kind": "push", "store": "infra", "state": "done", "position": 0, "chunks": 12 }),
            ),
            _ => stub::error(404, "not_found", "No such route."),
        }
    })
}

/// The names in a gzipped tar, in order.
fn tar_names(gz: &[u8]) -> Vec<String> {
    use std::io::Read;
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(gz)
        .read_to_end(&mut raw)
        .unwrap();
    let mut names = Vec::new();
    let mut at = 0;
    while at + 512 <= raw.len() && raw[at] != 0 {
        let h = &raw[at..at + 512];
        let size = usize::from_str_radix(std::str::from_utf8(&h[124..135]).unwrap(), 8).unwrap();
        names.push(
            String::from_utf8_lossy(&h[..100])
                .trim_end_matches('\0')
                .to_string(),
        );
        at += 512 + size.div_ceil(512) * 512;
    }
    names
}

#[test]
fn a_push_sends_only_what_changed_after_the_binarys_own_refusals() {
    let home = tempfile::tempdir().unwrap();
    let host = upload_host();
    signed_in(home.path(), &host);
    let tree = home.path().join("infra");
    let write = |rel: &str, bytes: &[u8]| {
        let p = tree.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    };
    write("README.md", b"# infra\nHow the boxes are built.\n");
    write("terraform/main.tf", b"resource \"null\" \"x\" {}\n");
    write(".gitignore", b"*.log\n");
    write("build.log", b"noise\n");
    write(".env", b"TOKEN=1\n");
    write("node_modules/left-pad/index.js", b"module.exports = 1;\n");
    write("keys.txt", b"aws_access_key_id = AKIAZ7Q4M2P9X3K8L5N6\n");
    write("big.bin", &vec![b'a'; 2 << 20]);

    let o = semlith(
        home.path(),
        &[
            "cloud",
            "push",
            "acme/infra",
            tree.to_str().unwrap(),
            "--wait",
            "--json",
        ],
    );
    assert!(o.status.success(), "{}", out(&o));
    let report: Value = serde_json::from_slice(&o.stdout).unwrap();

    let manifest = host.to("/v1/orgs/acme/stores/infra/pushes")[0].json();
    let mut listed: Vec<&str> = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    listed.sort();
    assert_eq!(listed, ["README.md", "terraform/main.tf"], "{}", out(&o));
    let readme = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "README.md")
        .unwrap();
    assert_eq!(readme["bytes"], 33);
    assert_eq!(readme["sha256"].as_str().unwrap().len(), 64);
    let refused: Vec<&str> = report["refused"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r[0].as_str().unwrap())
        .collect();
    assert!(
        refused.contains(&"keys.txt") && refused.contains(&"big.bin"),
        "{refused:?}"
    );

    let puts = host.to("/v1/orgs/acme/pushes/p_1/files");
    assert_eq!(puts.len(), 1);
    assert_eq!(puts[0].header("content-type"), Some("application/x-tar"));
    assert_eq!(puts[0].header("content-encoding"), Some("gzip"));
    let mut sent = tar_names(&puts[0].body);
    sent.sort();
    assert_eq!(sent, ["README.md", "terraform/main.tf"]);
    assert_eq!(report["job"], 48121);
    assert_eq!(report["position"], 2);
    assert_eq!(report["estimate_chunks"], 1400);
    assert_eq!(report["state"], "done");
    assert_eq!(host.to("/v1/orgs/acme/jobs/48121").len(), 1);

    // Nothing changed: a manifest, a commit, and no file at all.
    let o = semlith(
        home.path(),
        &[
            "cloud",
            "push",
            "acme/infra",
            tree.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(o.status.success(), "{}", out(&o));
    let second: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(second["sent"], 0);
    assert_eq!(
        host.to("/v1/orgs/acme/pushes/p_1/files").len(),
        1,
        "a second push sent files"
    );

    // One file changed: that file and nothing else.
    write("README.md", b"# infra\nChanged.\n");
    let o = semlith(
        home.path(),
        &["cloud", "push", "acme/infra", tree.to_str().unwrap()],
    );
    assert!(o.status.success(), "{}", out(&o));
    let puts = host.to("/v1/orgs/acme/pushes/p_1/files");
    assert_eq!(puts.len(), 2);
    assert_eq!(tar_names(&puts[1].body), ["README.md"]);
}

// ------------------------------------------------------------ ledger sync

#[test]
fn ledger_sync_is_a_per_store_switch_that_starts_off() {
    let home = tempfile::tempdir().unwrap();
    let host = org_host(None);
    signed_in(home.path(), &host);
    let path = home.path().join(".semlith/registry.json");
    std::fs::write(&path, LOCAL_REGISTRY).unwrap();
    assert!(
        semlith(home.path(), &["cloud", "connect", "acme"])
            .status
            .success()
    );

    let o = semlith(home.path(), &["cloud", "sync", "notes", "on"]);
    assert!(o.status.success(), "{}", out(&o));
    let sync = &registry(home.path())["stores"]["notes"]["cloud_sync"];
    assert_eq!(sync["org"], "acme");
    assert!(sync["since"].as_i64().unwrap() > 0);

    let o = semlith(home.path(), &["cloud", "sync", "acme/platform", "on"]);
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("is a remote store"),
        "{}",
        out(&o)
    );

    let o = semlith(home.path(), &["cloud", "sync", "notes", "off"]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(
        semlith(home.path(), &["cloud", "disconnect", "acme"])
            .status
            .success()
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), LOCAL_REGISTRY);
}

// ------------------------------------------------------- report and replay

#[test]
fn a_cloud_report_is_written_where_asked() {
    let home = tempfile::tempdir().unwrap();
    let host = Stub::start(|seen: &Seen| {
        if seen.path == "/v1/whoami" {
            return stub::json(200, json!({ "org": { "slug": "acme" } }));
        }
        (
            200,
            vec![("Content-Type".into(), "text/csv".into())],
            b"store,saved\nplatform,12\n".to_vec(),
        )
    });
    signed_in(home.path(), &host);
    let o = semlith(
        home.path(),
        &["cloud", "report", "acme", "savings", "--format", "docx"],
    );
    assert!(!o.status.success());
    assert_eq!(host.seen().len(), 1, "a bad format still reached the host");

    let file = home.path().join("savings.csv");
    let o = semlith(
        home.path(),
        &[
            "cloud",
            "report",
            "acme",
            "savings",
            "--format",
            "csv",
            "--window",
            "7d",
            "--stores",
            "platform,docs",
            "--out",
            file.to_str().unwrap(),
        ],
    );
    assert!(o.status.success(), "{}", out(&o));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "store,saved\nplatform,12\n"
    );
    assert_eq!(
        host.seen().last().unwrap().path,
        "/v1/orgs/acme/reports/savings?format=csv&window=7d&stores=platform,docs"
    );
}

fn transcript(home: &Path, id: &str) {
    let dir = home.join(".claude/projects/-work-api");
    std::fs::create_dir_all(&dir).unwrap();
    let call = |at: &str, name: &str, input: Value| {
        json!({ "timestamp": at, "message": { "content": [{ "type": "tool_use", "name": name, "input": input }] } })
            .to_string()
    };
    let lines = [
        call(
            "2026-10-01T09:14:03Z",
            "mcp__semlith__semlith_search",
            json!({ "query": "where is the order total" }),
        ),
        call(
            "2026-10-01T09:14:09Z",
            "Edit",
            json!({ "file_path": "src/total.rs" }),
        ),
    ];
    std::fs::write(dir.join(format!("{id}.jsonl")), lines.join("\n") + "\n").unwrap();
}

#[test]
fn replay_sends_only_the_session_picked() {
    let home = tempfile::tempdir().unwrap();
    let host = Stub::start(|seen: &Seen| match seen.path.as_str() {
        "/v1/whoami" => stub::json(200, json!({ "org": { "slug": "acme" } })),
        "/v1/orgs/acme/replay" => stub::json(200, json!({ "accepted": 1 })),
        _ => stub::error(404, "not_found", "No."),
    });
    signed_in(home.path(), &host);
    transcript(home.path(), "53fc2b7b-ec0");
    transcript(home.path(), "other-session");

    // No session named: a list to pick from, and nothing sent.
    let o = semlith(home.path(), &["cloud", "replay"]);
    assert!(o.status.success(), "{}", out(&o));
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("53fc2b7b-ec0"),
        "{}",
        out(&o)
    );
    assert!(host.to("/v1/orgs/acme/replay").is_empty());

    let o = semlith(home.path(), &["cloud", "replay", "53fc2b7b-ec0"]);
    assert!(o.status.success(), "{}", out(&o));
    let sent = host.to("/v1/orgs/acme/replay");
    assert_eq!(sent.len(), 1);
    let body = sent[0].json();
    assert_eq!(body["session"], "53fc2b7b-ec0");
    assert_eq!(body["client"], "claude-code");
    assert_eq!(body["items"][0]["tool"], "semlith_search");
    assert_eq!(body["items"][0]["query"], "where is the order total");
    assert_eq!(body["items"][0]["outcome"], "sufficed");
}

#[test]
fn replay_off_in_the_org_is_explained() {
    let home = tempfile::tempdir().unwrap();
    let host = Stub::start(|seen: &Seen| match seen.path.as_str() {
        "/v1/whoami" => stub::json(200, json!({ "org": { "slug": "acme" } })),
        _ => stub::error(409, "replay_off", "Replay is off."),
    });
    signed_in(home.path(), &host);
    transcript(home.path(), "s1");
    let o = semlith(home.path(), &["cloud", "replay", "s1"]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("Ledger › Session replay"), "{err}");
    assert_eq!(
        err.matches("Session replay is off").count(),
        1,
        "said once: {err}"
    );
}
