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
