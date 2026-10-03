//! The Semlith Cloud client, end to end against a stub host.
//!
//! The real cloud is not here and is not needed: `common/stub.rs` answers the
//! routes of `infra/docs/cloud-api.md` with canned JSON and records every
//! request, so these tests assert what left the binary as well as what it did
//! with the answer. Every run has its own HOME and store home; none touches
//! the owner's `~/.semlith`, and none needs a model.

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
                    "usage": { "month": "2026-10", "store_bytes": 1000, "cap_bytes": 8000, "index_minutes": 2, "cap_minutes": 100, "queue": { "running": 0, "waiting": 0 } }
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
        use std::io::BufRead;
        let mut child = Command::new(env!("CARGO_BIN_EXE_semlith"))
            .args(["start", "--port", "0"])
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
