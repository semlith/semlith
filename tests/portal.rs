//! Portal parity.
//!
//! From 0.9.0 a CLI command or an MCP tool is not finished until its portal
//! view exists. The rule is in AGENTS.md; this file is what makes it a gate
//! rather than a sentence, because a rule nothing enforces is a rule that
//! survives exactly until the release that is in a hurry.
//!
//! The subcommand list is read out of `--help` rather than out of a constant,
//! so adding a command and forgetting the portal fails here instead of shipping.
//! The tool list is read off a running daemon, for the same reason.
//!
//! ```sh
//! cargo test --test portal
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Commands with no portal view, and why.
///
/// `start` is the daemon serving the portal — a view of itself is the portal.
/// `mcp` is a stdio protocol server; the Agents view documents it, but it has
/// no state of its own for a route to report.
///
/// `pattern` takes a tree-sitter query in S-expression syntax. Nobody writes
/// one of those from memory, so a page for it is a box you can only fill by
/// pasting from documentation — which makes it a worse manual, not a view. The
/// capability is real and stays on the CLI and over MCP, where the caller is an
/// agent that can write the query; the Agents page lists the tool and says what
/// it does.
///
/// `read` has a view, and it is the Search page. Reading one span is the second
/// stage of a search — the button is on the hit that raised the question, with
/// the span already in hand. A page of its own could only be started by
/// retyping a coordinate you got from Search, which is why there is no longer
/// one. `/api/read` is what that button calls.
///
/// `models` had a view until 0.27.0: a forty-eight-row table on the About page,
/// of which one row is a model this machine has actually fetched. The v4 design
/// has no place for it, and the page it sat on is seven facts and a language
/// table. The capability is untouched — `semlith models` prints the full list
/// and `/api/models` still answers, for anything that reads it — so what is
/// gone is a table, not a thing a user could do. This is the one knowing
/// exception to the portal-parity rule in this release, and it is recorded in
/// `docs/compatibility.md` as well as here, because an exemption argued in one
/// place is an exemption nobody outside this file can find.
const NO_VIEW: [&str; 5] = ["start", "mcp", "pattern", "read", "models"];

/// Which route is a command's portal view. Adding a command means adding a
/// line here, which is the whole point: the compiler cannot notice a missing
/// view, and this list is the thing a reviewer looks at.
const VIEWS: &[(&str, &str)] = &[
    ("index", "/api/index"),
    ("add", "/api/add"),
    // The daemon *is* the watcher, and the Stores view is where its event feed
    // and per-store "watching" flag are read from.
    ("watch", "/api/stores"),
    ("search", "/api/search"),
    ("brief", "/api/brief"),
    ("stats", "/api/stores"),
    ("files", "/api/files"),
    ("forget", "/api/forget"),
    ("adopt", "/api/adopt"),
    // `semlith trust` is the Stores page's "Trust this store", beside a store
    // the daemon can see but has not been told to open.
    ("trust", "/api/trust"),
    // `semlith languages` is the About page's language table, which the v3
    // design puts there rather than on a page of its own. `/api/languages` is
    // still what fills it; the route named here is the page's own, because a
    // parity row has to point at a view somebody can open.
    ("languages", "/api/about"),
    ("setup", "/api/setup"),
    // A check and an install both reach the network, so neither is something a
    // route answers to a GET that a browser might replay.
    ("upgrade", "/api/upgrade"),
    ("symbol", "/api/symbol"),
    ("neighbors", "/api/neighbors"),
    ("path", "/api/path"),
    ("impact", "/api/impact"),
    ("trace", "/api/trace"),
    ("report", "/api/report"),
    // `semlith schedule` is the Reports page's Schedules card. The card is the
    // view and `/api/schedules` is what fills it, so both surfaces read the one
    // file the daemon owns rather than each keeping a list.
    ("schedule", "/api/schedules"),
    ("ledger", "/api/ledger"),
    // `semlith key` is the Agents page's Rotate button, which posts here.
    ("key", "/api/key"),
    // `semlith drop` is the Stores page's Delete, behind its second click.
    ("drop", "/api/store/delete"),
    // `semlith scan` is the Privacy page's Scan section: the same
    // `Semlith::scan` behind both, with a Forget button per row that posts to
    // `/api/forget`, the daemon's one eviction path.
    ("scan", "/api/privacy/scan"),
    // `semlith doctor` is the Doctor page, which reads the same two functions
    // the command prints and posts its repairs to the same engine.
    ("doctor", "/api/doctor"),
    // `semlith hook` is never typed by a person: a client runs it, and what a
    // person wants to see is whether it is installed, stale or absent for each
    // client -- which is what `doctor` reports and what the Agents page draws
    // from the same source. The view of a hook is its state, not its output.
    ("hook", "/api/doctor"),
];

/// And the same for the MCP tool surface.
const TOOL_VIEWS: &[(&str, &str)] = &[
    ("semlith_search", "/api/search"),
    ("semlith_stats", "/api/stores"),
    // As above: the table moved to About, so that is where the view is.
    ("semlith_languages", "/api/about"),
    ("semlith_files", "/api/files"),
    // Both tools are agent-facing and neither has a page: `semlith_read` is the
    // Search page's second stage, and `semlith_pattern` is a query syntax no
    // one types into a browser. The Agents page is where a person sees that
    // they exist and what they are for, so that is the view named here.
    // The Brief view on the Search page, which is the same assembly this tool
    // returns -- the parity rule applied in the release that added it, not the
    // one after next.
    ("semlith_brief", "/api/brief"),
    ("semlith_read", "/api/agents"),
    ("semlith_pattern", "/api/agents"),
    ("semlith_index", "/api/index"),
    ("semlith_add", "/api/add"),
    ("semlith_forget", "/api/forget"),
    ("semlith_symbol", "/api/symbol"),
    ("semlith_neighbors", "/api/neighbors"),
    ("semlith_path", "/api/path"),
    ("semlith_impact", "/api/impact"),
    ("semlith_trace", "/api/trace"),
    ("semlith_report", "/api/report"),
];

/// Which verb a route answers on.
///
/// One list, because there were three of these and they had already drifted
/// apart from each other — a route added to one and forgotten in the next is a
/// test that passes by asking the wrong question and reports 405 as a missing
/// view.
fn method_for(route: &str) -> &'static str {
    match route {
        "/api/index" | "/api/add" | "/api/forget" | "/api/adopt" | "/api/trust"
        | "/api/upgrade" | "/api/key" | "/api/endpoint" | "/api/store/delete" => "POST",
        _ => "GET",
    }
}

struct Daemon {
    child: Child,
    port: u16,
    token: String,
    _dir: tempfile::TempDir,
}

impl Daemon {
    fn start() -> Self {
        Self::spawn(true)
    }

    /// The same daemon with every home variable removed from its environment.
    ///
    /// `SEMLITH_HOME` still points at a temporary directory, because a daemon
    /// with nowhere to keep its stores does not start at all; what is missing
    /// is the *user's* home, which is what the directory browser is confined
    /// to. On Windows that is the ordinary state of a fresh shell (#70, #71).
    fn start_without_a_home() -> Self {
        Self::spawn(false)
    }

    /// A daemon serving a store home somebody else has already filled.
    fn start_in(home: &std::path::Path) -> Self {
        Self::spawn_in(Some(home), true)
    }

    fn spawn(with_home: bool) -> Self {
        Self::spawn_in(None, with_home)
    }

    fn spawn_in(store_home: Option<&std::path::Path>, with_home: bool) -> Self {
        let dir = tempfile::Builder::new()
            .prefix("semlith-parity-")
            .tempdir()
            .expect("a temporary directory");
        let home = match store_home {
            Some(given) => given.to_path_buf(),
            None => dir.path().join("home"),
        };
        std::fs::create_dir_all(&home).unwrap();

        let mut command = Command::new(env!("CARGO_BIN_EXE_semlith"));
        command
            .arg("start")
            .arg("--port")
            .arg("0")
            .env("SEMLITH_HOME", &home)
            .env_remove("SEMLITH_STORE")
            .env_remove("SEMLITH_PORT")
            .current_dir(dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if with_home {
            command.env("HOME", &home);
        } else {
            command
                .env_remove("HOME")
                .env_remove("USERPROFILE")
                .env_remove("HOMEDRIVE")
                .env_remove("HOMEPATH");
        }
        let mut child = command.spawn().expect("semlith start runs");

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
            _dir: dir,
        }
    }

    /// The status a route answers with. 404 is the only failing answer here:
    /// this test is about a route existing, not about what it returns for an
    /// empty daemon.
    fn status(&self, method: &str, path: &str) -> u16 {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("the daemon listens");
        stream
            .set_read_timeout(Some(Duration::from_secs(90)))
            .unwrap();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\n\
             Content-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}",
            self.port, self.token
        );
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().unwrap();

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        String::from_utf8_lossy(&raw)
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    /// A GET route's status and raw body, for the routes whose failure is the
    /// point of the test.
    fn get(&self, path: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("the daemon listens");
        stream
            .set_read_timeout(Some(Duration::from_secs(90)))
            .unwrap();
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\n\
             Connection: close\r\n\r\n",
            self.port, self.token
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
        let body = text
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap_or_default();
        (status, body)
    }

    /// A GET route's body, parsed.
    fn json(&self, path: &str) -> serde_json::Value {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("the daemon listens");
        stream
            .set_read_timeout(Some(Duration::from_secs(90)))
            .unwrap();
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\n\
             Connection: close\r\n\r\n",
            self.port, self.token
        );
        stream.write_all(request.as_bytes()).unwrap();
        stream.flush().unwrap();

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let text = String::from_utf8_lossy(&raw).into_owned();
        let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
        serde_json::from_str(body).unwrap_or_else(|e| panic!("{path} is not JSON: {e}\n{text}"))
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Every subcommand `semlith --help` lists.
fn subcommands() -> Vec<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--help")
        .output()
        .expect("semlith --help runs");
    let help = String::from_utf8_lossy(&out.stdout).into_owned();

    let mut names = Vec::new();
    let mut in_commands = false;
    for line in help.lines() {
        if line.trim_end().ends_with("Commands:") {
            in_commands = true;
            continue;
        }
        if in_commands {
            if line.trim().is_empty() {
                continue;
            }
            // The options block ends the commands block.
            if !line.starts_with("  ") || line.trim_start().starts_with('-') {
                break;
            }
            if let Some(name) = line.split_whitespace().next() {
                names.push(name.to_string());
            }
        }
    }
    names.retain(|n| n != "help");
    assert!(
        names.len() >= 10,
        "only parsed {names:?} out of --help; the parser, not the CLI, is probably wrong"
    );
    names
}

/// The parity rule, as a gate. A command with no line in `VIEWS` and no reason
/// in `NO_VIEW` fails here — which is the release that added it noticing,
/// rather than the release after next.
#[test]
fn every_cli_command_has_a_portal_view() {
    let daemon = Daemon::start();

    for command in subcommands() {
        if NO_VIEW.contains(&command.as_str()) {
            continue;
        }
        let route = VIEWS
            .iter()
            .find(|(name, _)| *name == command)
            .map(|(_, route)| *route)
            .unwrap_or_else(|| {
                panic!(
                    "`semlith {command}` has no portal view. Add one and list it in \
                     tests/portal.rs, or add it to NO_VIEW with the reason."
                )
            });

        let method = method_for(route);
        let status = daemon.status(method, route);
        assert_ne!(
            status, 404,
            "`semlith {command}` claims the route {route}, which does not exist"
        );
        assert_ne!(
            status, 405,
            "{route} does not answer {method}, which is how {command} would use it"
        );
    }
}

/// The same rule for the MCP surface, which is the other half of what an agent
/// can do and therefore the other half of what the portal owes a view for.
#[test]
fn every_mcp_tool_has_a_portal_view() {
    let daemon = Daemon::start();

    let body = {
        let mut stream = TcpStream::connect(("127.0.0.1", daemon.port)).unwrap();
        let request = format!(
            "GET /api/agents HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\nConnection: close\r\n\r\n",
            daemon.port, daemon.token
        );
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        raw.split_once("\r\n\r\n")
            .map(|(_, b)| b.to_string())
            .unwrap()
    };
    let agents: serde_json::Value = serde_json::from_str(&body).expect("the agents route is JSON");
    // Each entry is `{ name, about }` from 0.13.0: the page lists what each
    // tool is for beside its name, read from the tool's own definition.
    let tools: Vec<String> = agents["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .map(|t| {
            let name = t["name"].as_str().expect("a tool name").to_string();
            assert!(
                !t["about"].as_str().unwrap_or_default().is_empty(),
                "the tool {name} is served with no description"
            );
            name
        })
        .collect();

    assert!(!tools.is_empty(), "the daemon reports no tools at all");
    for tool in tools {
        let route = TOOL_VIEWS
            .iter()
            .find(|(name, _)| *name == tool)
            .map(|(_, route)| *route)
            .unwrap_or_else(|| {
                panic!(
                    "the MCP tool {tool} has no portal view. Add one and list it in \
                     tests/portal.rs."
                )
            });
        let method = method_for(route);
        assert_ne!(
            daemon.status(method, route),
            404,
            "{tool} claims the route {route}, which does not exist"
        );
    }
}

/// Every tool the MCP server actually serves has a parity row.
///
/// The test above reads the Agents route, which now derives its list from
/// `mcp::tool_names()` rather than repeating it — so this asserts against the
/// server's own definitions directly, and a tool added to `mcp::tools` with no
/// route and no row fails here rather than shipping invisible.
#[test]
fn every_tool_the_server_defines_has_a_parity_row() {
    let served = semlith::mcp::tool_names();
    assert!(!served.is_empty(), "the server defines no tools at all");
    for tool in &served {
        assert!(
            TOOL_VIEWS.iter().any(|(name, _)| name == tool),
            "{tool} is served by mcp::tools but has no row in TOOL_VIEWS"
        );
    }
    for (name, _) in TOOL_VIEWS {
        assert!(
            served.iter().any(|t| t == name),
            "TOOL_VIEWS lists {name}, which the server does not serve"
        );
    }
}

/// Nothing the portal serves may reach for another origin, because the policy
/// the server sends would block it — on a machine with a network as well as on
/// one without. The unit test in `src/portal` checks the bytes; this checks
/// what actually comes down the wire.
#[test]
fn what_the_portal_serves_names_no_other_origin() {
    let daemon = Daemon::start();
    for route in ["/", "/style.css", "/app.js"] {
        let mut stream = TcpStream::connect(("127.0.0.1", daemon.port)).unwrap();
        let request = format!(
            "GET {route} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\nConnection: close\r\n\r\n",
            daemon.port, daemon.token
        );
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let body = raw.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
        let parsed = body.replace("http://www.w3.org/2000/svg", "");
        assert!(
            !parsed.contains("http://") && !parsed.contains("https://"),
            "{route} names another origin, which the CSP forbids loading"
        );
    }
}

/// A sanity check on the list above: every route it names is one the portal
/// module actually serves or the router actually routes, not a path somebody
/// hoped for.
#[test]
fn no_view_in_the_map_is_a_route_that_does_not_exist() {
    let daemon = Daemon::start();
    let routes: Vec<&str> = VIEWS
        .iter()
        .chain(TOOL_VIEWS.iter())
        .map(|(_, route)| *route)
        .collect();
    let mut seen: Vec<&str> = Vec::new();
    for route in routes {
        if seen.contains(&route) {
            continue;
        }
        seen.push(route);
        let method = method_for(route);
        assert_ne!(daemon.status(method, route), 404, "{route} is not served");
    }
}

/// Every route 0.20.0 added, with the verb it answers on.
///
/// The parity map above pairs a *command* with its view, which is the rule
/// AGENTS.md states. These four are not a command's view — they are what the
/// page polls to read live — so they need their own row here or nothing would
/// notice one of them being dropped.
const LIVE_ROUTES: &[(&str, &str)] = &[
    // What is indexing and how far it has got, for a page or a script.
    ("GET", "/api/index/runs"),
    // One run's log after a cursor. Asked with no store on purpose: naming one
    // that does not exist is a 404 about the *store*, which is the right answer
    // and indistinguishable here from the route being missing.
    ("GET", "/api/index/log"),
    // The repositories under a folder, for the Index page's checklist and for
    // `semlith index --projects`.
    ("GET", "/api/projects"),
    // The six change counters the whole live portal is driven by.
    ("GET", "/api/changes"),
    // The three machine settings.
    ("POST", "/api/index/settings"),
];

#[test]
fn every_route_the_live_portal_polls_is_served() {
    let daemon = Daemon::start();
    for (method, route) in LIVE_ROUTES {
        let status = daemon.status(method, route);
        assert_ne!(status, 404, "{route} is not served");
        assert_ne!(status, 405, "{route} does not answer on {method}");
    }
}

/// `/api/changes` is the one clock, so it has to carry all six domains: a page
/// watching a domain this route does not report would never refetch it, and
/// would be the one stale panel on an otherwise live portal.
#[test]
fn the_changes_route_reports_every_domain() {
    let daemon = Daemon::start();
    let changes = daemon.json("/api/changes");
    for domain in ["stores", "runs", "clients", "ledger", "events", "privacy"] {
        assert!(
            changes
                .get(domain)
                .and_then(serde_json::Value::as_u64)
                .is_some(),
            "/api/changes does not report {domain}: {changes}"
        );
    }
}

/// A root folder deleted under an open store moves the stores counter.
///
/// Deleting a folder writes nothing to any store, so before `notice_roots` the
/// counter stood still and an open Stores page drew the root as present until
/// something unrelated moved it — the badge for a gone corpus appeared only on
/// the next navigation.
#[test]
fn a_deleted_root_moves_the_stores_counter() {
    let corpus = tempfile::Builder::new()
        .prefix("semlith-gone-root-")
        .tempdir()
        .unwrap();
    std::fs::write(corpus.path().join("a.md"), "A root that is about to go.").unwrap();
    let home = tempfile::Builder::new()
        .prefix("semlith-gone-home-")
        .tempdir()
        .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["index", "--quiet", &corpus.path().display().to_string()])
        .env("SEMLITH_HOME", home.path())
        .current_dir(corpus.path())
        .output()
        .expect("semlith index runs");
    assert!(
        out.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let daemon = Daemon::start_in(home.path());
    let counter = || daemon.json("/api/changes")["stores"].as_u64().unwrap();
    // The first poll is the baseline, and a second one with nothing changed
    // must not move.
    let before = counter();
    assert_eq!(counter(), before, "an idle poll moved the stores counter");

    // Removed by hand rather than by the guard's drop, which swallows a
    // failure: a root the watcher kept open would read as a counter that did
    // not move rather than as a folder that was never deleted.
    let root = corpus.path().to_path_buf();
    std::fs::remove_dir_all(&root)
        .unwrap_or_else(|e| panic!("removing {}: {e}", root.display()));
    assert_ne!(
        counter(),
        before,
        "the root at {} was deleted and the stores counter did not move",
        root.display()
    );
}

/// The two flags 0.20.0 adds to `index` have a surface on the page, which is
/// what portal parity asks of a capability.
///
/// `--each` is the Index page's target control, and `--projects` is its
/// repository checklist. Asserted against the page's own source, because a
/// flag documented in `--help` and absent from the portal is exactly the debt
/// this repository says it does not carry.
#[test]
fn the_index_flags_have_a_portal_surface() {
    const APP_JS: &str = include_str!("../src/portal/app.js");
    let help = std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["index", "--help"])
        .output()
        .expect("semlith index --help");
    let help = String::from_utf8_lossy(&help.stdout);

    assert!(
        help.contains("--each"),
        "index --help does not offer --each"
    );
    assert!(
        help.contains("--projects"),
        "index --help does not offer --projects"
    );
    assert!(
        APP_JS.contains("each folder becomes its own store"),
        "the Index page offers no `each` target, so --each has no portal view"
    );
    assert!(
        APP_JS.contains("/api/projects"),
        "the Index page never asks for projects, so --projects has no portal view"
    );
}

/// The Privacy page's Rules section is the release's claims with the daemon's
/// own reading beside each. A row that only stated the rule would be a sentence
/// somebody wrote.
#[test]
fn the_privacy_route_carries_a_rule_per_control_with_its_own_check() {
    let daemon = Daemon::start();
    let privacy = daemon.json("/api/privacy");
    let rules = privacy["rules"]
        .as_array()
        .expect("the privacy route carries no rules");

    // Every control this release added, by the name the page shows.
    for want in [
        "header-borne token",
        "same-origin writes",
        "store trust",
        "index boundary",
        "deny-list",
        "private addresses",
        "pinned models",
        "model cache",
        "directory modes",
        "agent key",
    ] {
        let rule = rules
            .iter()
            .find(|r| r["id"] == want)
            .unwrap_or_else(|| panic!("no rule for {want}:\n{privacy}"));
        assert!(
            rule["rule"].as_str().is_some_and(|t| t.len() > 40),
            "{want} states no rule: {rule}"
        );
        assert!(
            rule["check"].as_str().is_some_and(|t| !t.is_empty()),
            "{want} has no check: {rule}"
        );
        assert!(rule["ok"].is_boolean(), "{want} has no verdict: {rule}");
    }

    // On a fresh daemon in a sandbox, every one of them holds — which is what
    // makes a row that does not hold worth looking at.
    for rule in rules {
        assert_eq!(
            rule["ok"], true,
            "{} does not hold on a fresh install: {}",
            rule["id"], rule["check"]
        );
    }
}

/// The Index page's folder picker is confined to the user's home. With no home
/// it used to root at the filesystem — `/` on unix, whichever volume the
/// process started on under Windows — and then refuse the user's own profile
/// as outside it. A picker that cannot be confined is refused instead (#71).
#[test]
fn the_directory_browser_names_the_home_and_refuses_when_there_is_none() {
    let daemon = Daemon::start();
    let listing = daemon.json("/api/dirs");
    let home = listing["home"].as_str().expect("a home in the answer");
    assert!(!home.is_empty(), "the browser reported no home: {listing}");
    assert_ne!(home, "/", "the browser rooted at the filesystem: {listing}");
    let (status, _) = daemon.get(&format!("/api/dirs?path={home}"));
    assert_eq!(status, 200, "the home itself was refused as outside itself");

    let homeless = Daemon::start_without_a_home();
    let (status, body) = homeless.get("/api/dirs");
    assert_eq!(
        status, 500,
        "a browser with no home answered {status}: {body}"
    );
    assert!(
        body.contains("HOME"),
        "the refusal should name the variables it looked for: {body}"
    );
}

/// `offset` was validated and then ignored, so a client paging through results
/// was handed page one every time (#78).
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_search_route_applies_the_offset_it_validates() {
    let corpus = tempfile::Builder::new()
        .prefix("semlith-offset-corpus-")
        .tempdir()
        .unwrap();
    for (name, body) in [
        (
            "alpha.md",
            "The queue drains messages into the worker pool.",
        ),
        (
            "beta.md",
            "The worker pool reports queue depth every second.",
        ),
        ("gamma.md", "Queue depth is what the pool scales on."),
    ] {
        std::fs::write(corpus.path().join(name), body).unwrap();
    }

    // Indexed before the daemon starts, because the daemon opens the stores it
    // finds at start and a store created after that is not one of them.
    let home = tempfile::Builder::new()
        .prefix("semlith-offset-home-")
        .tempdir()
        .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["index", "--quiet", &corpus.path().display().to_string()])
        .env("SEMLITH_HOME", home.path())
        .current_dir(corpus.path())
        .output()
        .expect("semlith index runs");
    assert!(
        out.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let daemon = Daemon::start_in(home.path());
    let two = daemon.json("/api/search?query=queue%20depth&k=2");
    let hits = two["hits"].as_array().expect("hits").clone();
    assert!(hits.len() >= 2, "a three-file corpus returned {two}");

    let paged = daemon.json("/api/search?query=queue%20depth&k=1&offset=1");
    assert_eq!(
        paged["offset"].as_i64(),
        Some(1),
        "the response does not echo the offset: {paged}"
    );
    assert_eq!(
        paged["hits"].as_array().map(Vec::len),
        Some(1),
        "k=1 returned more than one row: {paged}"
    );
    assert_eq!(
        paged["hits"][0]["path"].as_str().unwrap_or_default(),
        hits[1]["path"].as_str().unwrap_or_default(),
        "offset=1 did not skip the first hit: {paged}"
    );
}

/// The canvas draws every kind the store holds.
///
/// `EDGE_KINDS` in `app.js` is the filter the Graph page applies on every load,
/// so a kind missing from it is fetched from `/api/graph` and dropped before
/// painting — which `contains` and `aliases` were for two releases, while the
/// rail went on counting them. The two lists are in different languages and
/// cannot share a constant, so this is what keeps them from drifting again.
#[test]
fn the_graph_page_draws_every_edge_kind_the_store_stores() {
    const APP_JS: &str = include_str!("../src/portal/app.js");

    let line = APP_JS
        .lines()
        .find(|line| line.starts_with("const EDGE_KINDS = "))
        .expect("app.js declares EDGE_KINDS on one line");
    let mut drawn: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
    drawn.sort_unstable();

    let mut stored: Vec<&str> = semlith::graph::KINDS.to_vec();
    stored.sort_unstable();

    assert_eq!(
        drawn, stored,
        "the Graph page's edge kinds and graph::KINDS disagree; a kind in one \
         and not the other is either an edge nothing draws or a chip that \
         filters nothing"
    );
}

/// Text the portal can render, with comments and interpolations removed.
///
/// Not a JavaScript parser. It tracks the three quote characters, backslash
/// escapes, `//` and `/* */`, and drops the `${...}` spans inside a template
/// literal, which is all that is needed to decide whether a run of digits is a
/// sentence a user reads or a note to whoever edits the file next.
fn user_visible_strings(source: &str) -> Vec<String> {
    let src: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < src.len() {
        match src[i] {
            '/' if src.get(i + 1) == Some(&'/') => {
                while i < src.len() && src[i] != '\n' {
                    i += 1;
                }
            }
            '/' if src.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < src.len() && !(src[i] == '*' && src.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
            }
            quote @ ('"' | '\'' | '`') => {
                let mut literal = String::new();
                i += 1;
                while i < src.len() {
                    if src[i] == '\\' {
                        i += 2;
                        continue;
                    }
                    if quote == '`' && src[i] == '$' && src.get(i + 1) == Some(&'{') {
                        // A live value, not a sentence: skip to the matching brace.
                        let mut depth = 1;
                        i += 2;
                        while i < src.len() && depth > 0 {
                            match src[i] {
                                '{' => depth += 1,
                                '}' => depth -= 1,
                                _ => {}
                            }
                            i += 1;
                        }
                        continue;
                    }
                    if src[i] == quote {
                        i += 1;
                        break;
                    }
                    literal.push(src[i]);
                    i += 1;
                }
                out.push(literal);
            }
            _ => i += 1,
        }
    }
    out
}

/// The first `<digits>.<digits>.<digits>` in `text` that is not the start of a
/// dotted quad, so an address the portal prints is not read as a release.
fn semver_like(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let digits = |at: usize| chars.get(at).is_some_and(char::is_ascii_digit);
    let mut i = 0;
    while i < chars.len() {
        if !digits(i) || (i > 0 && (digits(i - 1) || chars[i - 1] == '.')) {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        let mut groups = 1;
        while digits(end) {
            end += 1;
        }
        while groups < 3 && chars.get(end) == Some(&'.') && digits(end + 1) {
            end += 1;
            while digits(end) {
                end += 1;
            }
            groups += 1;
        }
        let quad = chars.get(end) == Some(&'.') && digits(end + 1);
        if groups == 3 && !quad {
            return Some(chars[start..end].iter().collect());
        }
        i = end.max(start + 1);
    }
    None
}

/// No release named anywhere inside the running product.
///
/// The About page used to say that forty grammars arrived in a named release
/// and that the binary had grown by a measured number of MiB since another one
/// — three versions and four figures, hardcoded, on a page where no reader can
/// tell which build they describe. They were true of one build on one machine
/// and went stale without failing anything, which is the defect
/// `the_readme_carries_no_release_specific_content` exists to catch in the
/// README. The portal is the same surface with a larger audience and had no
/// such gate.
///
/// The live values are unaffected: About's version and binary rows and the
/// setup screen's installed version are `${...}` interpolations of what the
/// server just returned, and this reads none of them.
#[test]
fn the_portal_names_no_release_in_anything_a_user_reads() {
    const APP_JS: &str = include_str!("../src/portal/app.js");

    for literal in user_visible_strings(APP_JS) {
        if let Some(found) = semver_like(&literal) {
            panic!(
                "src/portal/app.js renders {found:?} in {literal:?}; a version written into a \
                 sentence describes one build and goes stale inside the product, where the \
                 reader cannot tell. State it from what the server returns, or do not state it"
            );
        }
    }
}

/// The Agents page's two newest cards have something to read.
///
/// From this release the endpoint can be installed as a login service, and the
/// page that claims one endpoint for every client is the page that owes a
/// reader the answer to whether this machine actually kept one across a reboot.
/// Both halves — the service status and the per-client report — arrive on
/// `/api/agents`, and a route that quietly stops sending either leaves the two
/// cards blank with nothing failing anywhere else.
#[test]
fn the_agents_route_reports_the_service_and_every_client() {
    let daemon = Daemon::start();
    let agents = daemon.json("/api/agents");

    let service = &agents["service"];
    assert!(
        service.is_object(),
        "/api/agents sends no service block: {agents}"
    );
    let status = &service["status"];
    assert!(
        status["installed"].is_boolean(),
        "service `installed` is not a boolean: {status}"
    );
    // Not a boolean on one platform and absent on the others: the card says
    // out loud where a login task restarts only a failure, and it can only say
    // that from a value it was given.
    assert!(
        status["restarts"].is_boolean(),
        "service `restarts` is not a boolean: {status}"
    );
    let mechanism = status["mechanism"]
        .as_str()
        .unwrap_or_else(|| panic!("no service mechanism: {status}"));
    assert!(
        ["launchd", "systemd", "schtasks", "none"].contains(&mechanism),
        "unknown service mechanism {mechanism:?}; the portal names the mechanism \
         in prose and would print nothing for one it does not know"
    );
    for key in ["definition", "log"] {
        assert!(
            status[key].is_string() || status[key].is_null(),
            "service status {key} is neither a path nor null: {status}"
        );
    }
    assert!(
        service["last_started"].is_u64() || service["last_started"].is_null(),
        "last_started is neither unix seconds nor null: {service}"
    );

    let clients = agents["doctor"]
        .as_array()
        .unwrap_or_else(|| panic!("/api/agents sends no doctor report: {agents}"));
    assert!(
        !clients.is_empty(),
        "the doctor report on /api/agents is empty; the Agents page lists no client at all"
    );
    for client in clients {
        let name = client["name"]
            .as_str()
            .unwrap_or_else(|| panic!("a doctor entry with no name: {client}"));
        // `in_use` is what the Agents page filters on — it shows the clients
        // somebody here has, not the full catalogue — so an entry missing it
        // would silently drop off the card.
        for key in ["present", "registered", "fault", "in_use", "disabled_here"] {
            assert!(
                client[key].is_boolean(),
                "{name}: {key} is not a boolean: {client}"
            );
        }
        for key in ["command", "repair", "note", "explain"] {
            assert!(
                client[key].is_string() || client[key].is_null(),
                "{name}: {key} is neither a string nor null: {client}"
            );
        }
        let scope = &client["scope"];
        assert!(
            scope.is_null() || matches!(scope.as_str(), Some("user" | "project")),
            "{name}: unexpected scope {scope}"
        );
        // The directories the override names. The page renders these and not
        // `disabled_here`, because "here" for a daemon a service manager
        // started is the root of the disk and nobody's working directory.
        let disabled_in = client["disabled_in"]
            .as_array()
            .unwrap_or_else(|| panic!("{name}: disabled_in is not an array: {client}"));
        for dir in disabled_in {
            assert!(
                dir.is_string(),
                "{name}: disabled_in holds {dir}, not a path"
            );
        }
        let files = client["files"]
            .as_array()
            .unwrap_or_else(|| panic!("{name}: files is not an array: {client}"));
        for file in files {
            assert!(
                file["path"].is_string(),
                "{name}: a file with no path: {file}"
            );
            for key in ["exists", "parses", "names_semlith"] {
                assert!(
                    file[key].is_boolean(),
                    "{name}: file {key} is not a boolean: {file}"
                );
            }
        }
    }
}

/// The sidebar: thirteen entries in four groups, in this order.
///
/// The entries are the v4 design's thirteen. The grouping is the v3 design's,
/// taken back in 0.26.1: v4 flattened v3's four groups into two of six and
/// seven, which reads as one long list with two headings in it. v3's fourth
/// group was `Account`, holding License and About; the binary is free and
/// there is no licence page, so the last group is the two pages that describe
/// the machine this is running on.
///
/// Fourteen, not thirteen: `Index` and `Inside the index` are two pages in the
/// design and were one here, so the indexing controls and the figures about
/// what was indexed were stacked on one page. Splitting them is what the
/// design draws, and the second half — the corpus — is the page nothing else
/// in this product can show.
///
/// Doctor is deliberately not in the design's own list.
/// It is a page this binary already serves, and `semlith doctor` would
/// otherwise be the one command with no view — which the parity test above
/// would fail anyway, from the other direction.
const SIDEBAR: &[(&str, &str)] = &[
    ("Workspace", "Stores"),
    ("Workspace", "Files"),
    ("Workspace", "Index"),
    ("Workspace", "Inside the index"),
    ("Explore", "Search"),
    ("Explore", "Graph"),
    ("Explore", "Impact"),
    ("Operate", "Retrieval ledger"),
    ("Operate", "Reports"),
    ("Operate", "Agents"),
    ("Operate", "Cloud"),
    ("Operate", "Privacy"),
    ("Machine", "Doctor"),
    ("Machine", "About"),
];

/// The `{ group: …, id: …, label: … }` rows of `VIEWS`, in source order.
fn sidebar_rows(source: &str) -> Vec<(String, String, String)> {
    let start = source
        .find("const VIEWS = [")
        .expect("app.js defines VIEWS");
    let body = &source[start..];
    let end = body.find("\n];").expect("VIEWS is closed");
    let mut rows = Vec::new();
    for line in body[..end].lines() {
        let field = |name: &str| -> Option<String> {
            let at = line.find(&format!("{name}: \""))?;
            let rest = &line[at + name.len() + 3..];
            Some(rest[..rest.find('"')?].to_string())
        };
        if let (Some(group), Some(id), Some(label)) = (field("group"), field("id"), field("label"))
        {
            rows.push((group, id, label));
        }
    }
    rows
}

#[test]
fn the_sidebar_is_the_thirteen_pages_in_four_groups_in_order() {
    const APP_JS: &str = include_str!("../src/portal/app.js");
    let rows = sidebar_rows(APP_JS);
    let got: Vec<(&str, &str)> = rows
        .iter()
        .map(|(group, _, label)| (group.as_str(), label.as_str()))
        .collect();
    assert_eq!(
        got,
        SIDEBAR.to_vec(),
        "the sidebar's groups, labels or order have moved"
    );
    // Each group contiguous: the rail draws a rule between groups, so an entry
    // in the wrong place splits a group into two.
    let mut groups: Vec<&str> = Vec::new();
    for (group, _) in &got {
        if groups.last() != Some(group) {
            assert!(
                !groups.contains(group),
                "{group} appears twice, so the rail would draw it as two groups"
            );
            groups.push(group);
        }
    }
    assert_eq!(groups, vec!["Workspace", "Explore", "Operate", "Machine"]);
}

#[test]
fn every_sidebar_entry_has_a_mark_and_something_to_render() {
    const APP_JS: &str = include_str!("../src/portal/app.js");
    for (_, id, label) in sidebar_rows(APP_JS) {
        assert!(
            APP_JS.contains(&format!("\n  {id}: \"M")),
            "{label} has no entry in NAV_ICONS, so the rail would draw a gap"
        );
        assert!(
            APP_JS.contains(&format!("\n  {id}: ")) && APP_JS.contains(&format!("{id}View")),
            "{label} has no view function, so the entry navigates to nothing"
        );
    }
}

/// The governing rule of the v4 design: the binary is free and complete, so
/// no paid surface survives anywhere in the portal.
///
/// Checked over what the page can render rather than over the source, because
/// a comment recording why a lock was removed is not a lock. The one word
/// allowed through is "key", and only as the agent key.
#[test]
fn nothing_the_portal_renders_offers_to_sell_anything() {
    const APP_JS: &str = include_str!("../src/portal/app.js");
    const STYLE: &str = include_str!("../src/portal/style.css");
    const FORBIDDEN: [&str; 8] = [
        "Pro",
        "Team",
        "Enterprise",
        "licence",
        "license",
        "unlock",
        "upgrade",
        "trial",
    ];
    for literal in user_visible_strings(APP_JS) {
        let lower = literal.to_lowercase();
        for word in FORBIDDEN {
            let needle = word.to_lowercase();
            if !lower.contains(&needle) {
                continue;
            }
            // `upgrade` is also what `semlith upgrade` does to the binary,
            // which is a command this portal has a page for and not an offer
            // to sell anything.
            // `upgrade` is also what `semlith upgrade` does to the binary,
            // and the route behind that page. Neither is an offer to sell.
            if needle == "upgrade"
                && (lower.contains("semlith upgrade")
                    || lower.contains("upgrading")
                    || lower.contains("up to date")
                    || lower.starts_with("/api/upgrade")
                    || lower.contains("this version")
                    || lower.contains("newer version")
                    || lower.contains("install"))
            {
                continue;
            }
            // "team" inside an ordinary word is not the word — and "a team
            // ledger" on the Cloud page is a description of the hosted
            // service, not a tier of this binary. The design's own copy says
            // it, and it is the one place allowed.
            if needle == "team"
                && (!lower
                    .split(|c: char| !c.is_alphanumeric())
                    .any(|w| w == "team")
                    || lower.contains("team ledger"))
            {
                continue;
            }
            if needle == "pro"
                && !lower
                    .split(|c: char| !c.is_alphanumeric())
                    .any(|w| w == "pro")
            {
                continue;
            }
            panic!("the portal renders {word:?} in {literal:?}");
        }
    }
    // The stylesheet cannot render words, but a class named for a lock is a
    // lock somebody is about to draw.
    for name in [".plan-", ".locked", ".pro-", ".paywall"] {
        assert!(!STYLE.contains(name), "the stylesheet still has {name}");
    }
    // And the one word the design lets through: `key`, as the agent key.
    //
    // Checked as the phrases that would mean a licence rather than by
    // allow-listing every sentence that mentions the credential — the portal
    // has a page about the agent key and most of its copy says "key".
    for literal in user_visible_strings(APP_JS) {
        let lower = literal.to_lowercase();
        for phrase in [
            "licence key",
            "license key",
            "product key",
            "activation key",
            "key activate",
            "activate your key",
            "enter your key",
            "buy a key",
            "purchase a key",
            "key required",
        ] {
            assert!(
                !lower.contains(phrase),
                "the portal renders a licence key: {literal:?}"
            );
        }
    }
}

/// Every tool the server advertises is on the Agents page with no marker
/// saying it costs anything, and the cost row is measured rather than stated.
#[test]
fn the_agents_page_marks_no_tool_as_paid_and_measures_the_list() {
    const APP_JS: &str = include_str!("../src/portal/app.js");
    assert!(
        !APP_JS.contains("· paid") && !APP_JS.contains("\"paid\""),
        "a tool is marked paid"
    );
    // The row states what this binary's own list costs, from the route,
    // rather than a number written down when it was last measured.
    assert!(
        APP_JS.contains("tool_list_bytes") && APP_JS.contains("What the tool list costs"),
        "the cost row is not read from the server"
    );
}

/// The Cloud page describes the service and contacts nothing.
#[test]
fn the_cloud_page_is_the_not_connected_state_and_has_no_client() {
    const APP_JS: &str = include_str!("../src/portal/app.js");
    assert!(
        APP_JS.contains("async function cloudView()"),
        "no Cloud page"
    );
    assert!(
        APP_JS.contains("Semlith Cloud is one hosted store"),
        "the Cloud page lost its lead copy"
    );
    // No connected state in this release: `cloudConnected` would be the flag
    // that draws one, and there is none.
    assert!(
        !APP_JS.contains("cloudConnected"),
        "a connected state exists"
    );
    // And no command behind it. The blocks on the page are text to copy.
    assert!(
        !subcommands().iter().any(|name| name == "cloud"),
        "`semlith cloud` exists, which this release says it does not"
    );
}

/// No bar is sized by a `style` attribute written into the markup.
///
/// The portal is served under `style-src 'self'` with no `unsafe-inline`, so
/// a width written that way is blocked and the bar renders at its default
/// size — silently, which is how three bars shipped at full width in
/// development before the browser drive caught them. Dynamic sizes go
/// through the CSSOM, as the tooltip's position has since 0.11.0.
#[test]
fn nothing_the_portal_builds_carries_an_inline_style_attribute() {
    const APP_JS: &str = include_str!("../src/portal/app.js");
    let mut offenders = Vec::new();
    for (i, line) in APP_JS.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        // `el(…, { style: … })` and `"style":` both set the attribute.
        if trimmed.contains("style:") || trimmed.contains("\"style\"") {
            offenders.push(format!("{}: {}", i + 1, trimmed));
        }
    }
    assert!(
        offenders.is_empty(),
        "these would be dropped by the portal's own content-security policy:\n  {}",
        offenders.join("\n  ")
    );
}
