//! Portal parity.
//!
//! From 0.9.0 a CLI command or an MCP tool is not finished until its portal
//! view exists. The rule is in AGENTS.md; this file is what makes it a gate
//! rather than a sentence, because a rule nothing enforces is a rule that
//! survives exactly until the release that is in a hurry.
//!
//! The subcommand list is read out of `--help` rather than out of a constant,
//! so adding a command and forgetting the portal fails here instead of shipping.
//! The tool list is read off a running daemon, for the same reason. The routes
//! the page calls are read out of `app.js`, so a page that calls a route the
//! daemon does not serve fails here rather than in a browser.
//!
//! 0.35.0 rebuilt the portal to design v6: nine pages in three groups, a page
//! per store, a Welcome screen and a store wizard. The old Files, Index,
//! Inside the index, Impact, Doctor, About and Cloud pages became tabs and
//! sections of the new ones; `MOVED` below is the list of where each went.
//!
//! ```sh
//! cargo test --test portal
//! ```

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const APP_JS: &str = include_str!("../src/portal/app.js");
const STYLE: &str = include_str!("../src/portal/style.css");
const INDEX_HTML: &str = include_str!("../src/portal/index.html");
const ROUTES_RS: &str = include_str!("../src/routes.rs");
const PORTAL_MD: &str = include_str!("../docs/portal.md");

/// Commands with no portal view, and why.
///
/// `start` is the daemon serving the portal — a view of itself is the portal.
/// `mcp` is a stdio protocol server; the Agents page documents it, but it has
/// no state of its own for a route to report.
///
/// `models` had a view until 0.27.0: a forty-eight-row table on the About page,
/// of which one row is a model this machine has actually fetched. Design v6
/// has no place for it either: Settings › About names the model the stores
/// use, which is the one row that was ever about this machine. The capability
/// is untouched — `semlith models` prints the full list and `/api/models` still
/// answers, for anything that reads it — so what is gone is a table, not a
/// thing a user could do. It is recorded in `docs/compatibility.md` as well as
/// here, because an exemption argued in one place is an exemption nobody
/// outside this file can find.
///
/// Until 0.35.0 `pattern` and `read` were here too. Both have a view now:
/// `pattern` is Search's fourth mode, and `read` is the "Read whole symbol"
/// button in Search's detail panel. Each is in `VIEWS` with the route its view
/// calls, so the route is probed rather than excused.
const NO_VIEW: [&str; 3] = ["start", "mcp", "models"];

/// Which route is a command's portal view. Adding a command means adding a
/// line here, which is the whole point: the compiler cannot notice a missing
/// view, and this list is the thing a reviewer looks at.
///
/// Every route named here is one `app.js` actually calls;
/// `every_parity_route_is_one_the_page_calls` checks that, so a row cannot
/// point at a route the page stopped using.
const VIEWS: &[(&str, &str)] = &[
    // The store wizard's scan and index, and a store's Re-index and Re-index
    // selected on its Files and Runs tabs.
    ("index", "/api/index"),
    // The wizard's Sources step, "Add a URL": one https request.
    ("add", "/api/add"),
    // The daemon *is* the watcher. Each store's watching flag and Home's
    // Watcher feed are read from the stores route; the store Settings tab's
    // "Watch for changes" switch posts to `/api/store/settings`, which
    // `every_route_the_page_calls_is_served` covers.
    ("watch", "/api/stores"),
    ("search", "/api/search"),
    // Search's Brief mode, the same assembly the tool returns.
    ("brief", "/api/brief"),
    // Search's Read whole symbol, in the detail panel of a hit.
    ("read", "/api/read"),
    // Search's Pattern mode, a tree-sitter query over one language.
    ("pattern", "/api/pattern"),
    // Stores › All stores, and every store's Overview tab.
    ("stats", "/api/stores"),
    // A store's Files tab. The cross-store Files page is gone (item 19 of the
    // 0.35.0 contract): Search's path filter covers it.
    ("files", "/api/files"),
    // A store's Files tab, Forget on selected rows, and Privacy's scan rows.
    ("forget", "/api/forget"),
    // Welcome's and Stores' "Adopt an existing .semlith".
    ("adopt", "/api/adopt"),
    // The store Settings tab's Trust, beside a store the daemon can see but
    // has not been told to open.
    ("trust", "/api/trust"),
    // Settings › About's language table, which the page fills from this route.
    ("languages", "/api/languages"),
    // `semlith setup` registers semlith with the agents on this machine. In
    // the portal that is Agents › Add a client and the wizard's Connect step,
    // both of which post here. The other steps have their own surfaces: Start
    // at login (Settings › Agent access, `/api/login-item`), the model download
    // (Welcome and the wizard). The file-manager helpers are the CLI's only
    // (`semlith setup --file-managers`): the owner dropped the portal card in
    // 0.35.0, since a drop onto the page already does what they offered.
    ("setup", "/api/agents/register"),
    // Settings › About: Check for updates, then Install. Both reach the
    // network, so neither is something a route answers to a GET that a
    // browser might replay.
    ("upgrade", "/api/upgrade"),
    // Graph › Explore's selected-node panel and Search's One hop around.
    ("symbol", "/api/symbol"),
    ("neighbors", "/api/neighbors"),
    // Graph › Path & evidence merges `path` and `trace` into one tab (v6):
    // the chain, its confidence per hop and the supporting lines are one
    // answer, and the page asks `/api/trace` for it. `/api/path` is still
    // served for anything that reads it; the page does not call it.
    ("path", "/api/trace"),
    ("trace", "/api/trace"),
    // Graph › Blast radius.
    ("impact", "/api/impact"),
    // Reports: the builder, its live preview and Save to disk.
    ("report", "/api/report"),
    // Reports' Schedules card. The card is the view and `/api/schedules` is
    // what fills it, so both surfaces read the one file the daemon owns.
    ("schedule", "/api/schedules"),
    // The Ledger page.
    ("ledger", "/api/ledger"),
    // Reports' savings card prices at a chosen model and has Update prices;
    // Settings › About has the Prices card with the same button.
    ("prices", "/api/prices"),
    // Settings › Agent access, the agent key's Rotate.
    ("key", "/api/key"),
    // `semlith drop` is Forget this store: the store Settings tab and the
    // Stores row menu, behind a confirmation.
    ("drop", "/api/store/delete"),
    // Stores' Compact N and the row menu's Compact, and the store Settings
    // tab's Compact.
    ("compact", "/api/store/compact"),
    // Privacy's Check what the stores hold: the same `Semlith::scan` behind
    // both, with a Forget button per row that posts to `/api/forget`, the
    // daemon's one eviction path.
    ("scan", "/api/privacy/scan"),
    // A store's Review tab reads the same `refusals` table for the files a
    // scan held back, and decides on them through `/api/refused/decide` —
    // singly or in bulk, by a person, never by an agent (0.35.0, item 19).
    ("refused", "/api/refused"),
    // `semlith doctor` is Agents › Health: the per-client report the command
    // prints, which `/api/agents` carries as its `doctor` block, beside the
    // machine checks from the privacy rules.
    ("doctor", "/api/agents"),
    // `semlith hook` is never typed by a person: a client runs it, and what a
    // person wants to see is whether it is installed, stale or absent for each
    // client — which is the hook column of Agents › Health, read from the
    // same report. The view of a hook is its state, not its output.
    ("hook", "/api/agents"),
    // Settings › Performance, "Where embedding runs": a switch per lane with
    // its device, state and share.
    ("accel", "/api/accel"),
    // Settings › Cloud: who this machine is signed in as, its remote stores,
    // ledger sync per store, and what leaves the machine.
    ("cloud", "/api/cloud"),
];

/// And the same for the MCP tool surface.
const TOOL_VIEWS: &[(&str, &str)] = &[
    ("semlith_search", "/api/search"),
    ("semlith_stats", "/api/stores"),
    // As above: the table is on Settings › About, filled from this route.
    ("semlith_languages", "/api/languages"),
    ("semlith_files", "/api/files"),
    // The Brief mode on the Search page, which is the same assembly this tool
    // returns — the parity rule applied in the release that added it.
    ("semlith_brief", "/api/brief"),
    // Until 0.35.0 these two pointed at the Agents page, which lists every
    // tool. Each has a view of its own now: Read whole symbol, and Search's
    // Pattern mode.
    ("semlith_read", "/api/read"),
    ("semlith_pattern", "/api/pattern"),
    ("semlith_index", "/api/index"),
    ("semlith_add", "/api/add"),
    ("semlith_forget", "/api/forget"),
    ("semlith_symbol", "/api/symbol"),
    ("semlith_neighbors", "/api/neighbors"),
    // Path & evidence, as for the command.
    ("semlith_path", "/api/trace"),
    ("semlith_impact", "/api/impact"),
    ("semlith_trace", "/api/trace"),
    ("semlith_report", "/api/report"),
];

/// Which verb a parity route answers on.
///
/// One list, because there were three of these and they had already drifted
/// apart from each other — a route added to one and forgotten in the next is a
/// test that passes by asking the wrong question and reports 405 as a missing
/// view. The routes the page calls carry their verb with them (see
/// `page_calls`), so this only has to cover the parity maps.
fn method_for(route: &str) -> &'static str {
    match route {
        "/api/index"
        | "/api/add"
        | "/api/forget"
        | "/api/adopt"
        | "/api/trust"
        | "/api/upgrade"
        | "/api/key"
        | "/api/endpoint"
        | "/api/store/delete"
        | "/api/store/compact"
        | "/api/agents/register" => "POST",
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
        let (_, body) = self.get(path);
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("{path} is not JSON: {e}\n{body}"))
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

/// A row in `NO_VIEW` for a command that no longer exists, or that also has a
/// row in `VIEWS`, is an exemption nobody is arguing any more.
#[test]
fn every_exemption_is_for_a_command_that_exists_and_has_no_row() {
    let commands = subcommands();
    for name in NO_VIEW {
        assert!(
            commands.iter().any(|c| c == name),
            "NO_VIEW excuses `semlith {name}`, which --help does not list"
        );
        assert!(
            !VIEWS.iter().any(|(command, _)| *command == name),
            "`semlith {name}` is in NO_VIEW and in VIEWS; it is one or the other"
        );
    }
}

/// The same rule for the MCP surface, which is the other half of what an agent
/// can do and therefore the other half of what the portal owes a view for.
#[test]
fn every_mcp_tool_has_a_portal_view() {
    let daemon = Daemon::start();
    let agents = daemon.json("/api/agents");
    // Each entry is `{ name, about }` from 0.13.0: Agents › Tools lists what
    // each tool is for beside its name, read from the tool's own definition.
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
/// The test above reads the Agents route, which derives its list from
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

/// Every `(verb, route)` the page calls, read out of `app.js`.
///
/// Not a JavaScript parser: every `/api/…` in a line that is not a comment is a
/// route the page calls, and it is a POST when the text before it is
/// `post("` or ``post(` ``, a GET otherwise — `api(…)` and the `SOURCES` table
/// both read. The query string is dropped. A change of calling style that this
/// misses shows up as the floor in `the_page_calls_are_read_out_of_app_js`
/// failing, not as a silent pass.
fn page_calls() -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    for line in APP_JS.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
            continue;
        }
        let mut rest = line;
        let mut consumed = 0;
        while let Some(at) = rest.find("/api/") {
            let before = &line[..consumed + at];
            let path: String = rest[at..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-'))
                .collect();
            let quoted = before.ends_with('"') || before.ends_with('`') || before.ends_with('\'');
            if quoted {
                let verb = if before.ends_with("post(\"") || before.ends_with("post(`") {
                    "POST"
                } else {
                    "GET"
                };
                let path = path.trim_end_matches('/').to_string();
                if !out.iter().any(|(v, p)| *v == verb && *p == path) {
                    out.push((verb, path));
                }
            }
            consumed += at + 5;
            rest = &line[consumed..];
        }
    }
    out
}

/// The extraction above finds what the page plainly calls, so a page that
/// changed how it calls its routes fails here before the served-route test
/// passes over an empty list.
#[test]
fn the_page_calls_are_read_out_of_app_js() {
    let calls = page_calls();
    for want in [
        ("GET", "/api/changes"),
        ("GET", "/api/stores"),
        ("GET", "/api/index/runs"),
        ("POST", "/api/index"),
        ("POST", "/api/refused/decide"),
        ("POST", "/api/store/settings"),
    ] {
        assert!(
            calls.iter().any(|(v, p)| *v == want.0 && p == want.1),
            "page_calls did not find {want:?} in app.js; found {calls:?}"
        );
    }
    assert!(
        calls.len() >= 50,
        "only {} routes read out of app.js: {calls:?}",
        calls.len()
    );
}

/// Routes the page calls that this test does not send a request to, and why.
/// Each is checked against the router's own source instead, so a route that is
/// missing still fails; what is skipped is only the request.
const NOT_PROBED: &[(&str, &str, &str)] = &[
    (
        "POST",
        "/api/rotate",
        "replaces the session token this test holds, so every later probe would answer 401 — \
         which is not 404, and so passes without asking anything",
    ),
    (
        "POST",
        "/api/doctor/gpu",
        "runs every lane's check, and the first may download a lane's components: a network \
         fetch in an offline suite",
    ),
    (
        "POST",
        "/api/login-item",
        "installs or removes the login service; launchd, systemd and schtasks are not \
         redirected by HOME, so a probe could reach the owner's real service",
    ),
];

/// Every route the page calls is served, on the verb the page calls it with.
///
/// The parity maps say which route is a command's view; this says the page and
/// the daemon agree about every route the page uses, which is the half a
/// rewrite of `app.js` can break without touching a command. A route that is
/// in `NOT_PROBED` must still have its arm in `src/routes.rs`.
#[test]
fn every_route_the_page_calls_is_served() {
    let daemon = Daemon::start();
    let mut missing = Vec::new();
    for (verb, route) in page_calls() {
        if NOT_PROBED.iter().any(|(v, r, _)| *v == verb && *r == route) {
            let arm = match verb {
                "POST" => [
                    format!("(_, true, \"{route}\")"),
                    format!("(_, _, \"{route}\") if get || post"),
                ],
                _ => [
                    format!("(true, _, \"{route}\")"),
                    format!("(_, _, \"{route}\") if get || post"),
                ],
            };
            if !arm.iter().any(|a| ROUTES_RS.contains(a.as_str())) {
                missing.push(format!("{verb} {route} (no arm in src/routes.rs)"));
            }
            continue;
        }
        let status = daemon.status(verb, &route);
        if status == 404 || status == 405 {
            missing.push(format!("{verb} {route} answered {status}"));
        }
    }
    assert!(
        missing.is_empty(),
        "app.js calls routes the daemon does not serve:\n  {}",
        missing.join("\n  ")
    );
}

/// Every route a parity row names is one the page actually calls — a view is
/// what a person can open, and a route nothing on the page fetches is not one.
#[test]
fn every_parity_route_is_one_the_page_calls() {
    let calls = page_calls();
    for (name, route) in VIEWS.iter().chain(TOOL_VIEWS.iter()) {
        assert!(
            calls.iter().any(|(_, p)| p == route),
            "{name} names {route} as its view, and app.js never calls it"
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
        let (_, body) = daemon.get(route);
        let parsed = body.replace("http://www.w3.org/2000/svg", "");
        assert!(
            !parsed.contains("http://") && !parsed.contains("https://"),
            "{route} names another origin, which the CSP forbids loading"
        );
    }
}

/// A sanity check on the maps above: every route they name is one the router
/// actually routes, not a path somebody hoped for.
#[test]
fn no_view_in_the_map_is_a_route_that_does_not_exist() {
    let daemon = Daemon::start();
    let mut seen: Vec<&str> = Vec::new();
    for (_, route) in VIEWS.iter().chain(TOOL_VIEWS.iter()) {
        if seen.contains(route) {
            continue;
        }
        seen.push(route);
        let method = method_for(route);
        assert_ne!(daemon.status(method, route), 404, "{route} is not served");
    }
}

/// What the page polls to read live, with the verb it answers on.
///
/// The parity map above pairs a *command* with its view, which is the rule
/// AGENTS.md states. These are not a command's view — they are what the page
/// polls — so they keep their own rows here, beside the general check above,
/// so that dropping one names itself.
const LIVE_ROUTES: &[(&str, &str)] = &[
    // What is indexing and how far it has got: the header's run pill, each
    // store's Runs tab and the wizard's Index step.
    ("GET", "/api/index/runs"),
    // One run's log after a cursor. Asked with no store on purpose: naming one
    // that does not exist is a 404 about the *store*, which is the right answer
    // and indistinguishable here from the route being missing.
    ("GET", "/api/index/log"),
    // The repositories under a folder, for the wizard's Keep together / One
    // store each and for `semlith index --projects`.
    ("GET", "/api/projects"),
    // The six change counters the whole live portal is driven by.
    ("GET", "/api/changes"),
    // Settings › Performance's limits.
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

/// The six domains `/api/changes` reports, which `DOMAIN_KEYS` in `app.js`
/// maps to what each one refetches.
const DOMAINS: [&str; 6] = ["stores", "runs", "clients", "ledger", "events", "privacy"];

/// `/api/changes` is the one clock, so it has to carry all six domains: a page
/// watching a domain this route does not report would never refetch it, and
/// would be the one stale panel on an otherwise live portal. The page's own
/// table has to name the same six, or a counter that moves refetches nothing.
#[test]
fn the_changes_route_reports_every_domain() {
    let daemon = Daemon::start();
    let changes = daemon.json("/api/changes");
    for domain in DOMAINS {
        assert!(
            changes
                .get(domain)
                .and_then(serde_json::Value::as_u64)
                .is_some(),
            "/api/changes does not report {domain}: {changes}"
        );
    }

    let start = APP_JS
        .find("const DOMAIN_KEYS = {")
        .expect("app.js maps domains to keys in DOMAIN_KEYS");
    let block = &APP_JS[start..];
    let block = &block[..block.find("\n};").expect("DOMAIN_KEYS is closed")];
    for domain in DOMAINS {
        assert!(
            block.contains(&format!("\n  {domain}: [")),
            "DOMAIN_KEYS in app.js does not say what {domain} refetches"
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
    std::fs::remove_dir_all(&root).unwrap_or_else(|e| panic!("removing {}: {e}", root.display()));
    assert_ne!(
        counter(),
        before,
        "the root at {} was deleted and the stores counter did not move",
        root.display()
    );
}

/// The two flags 0.20.0 added to `index` have a surface on the page, which is
/// what portal parity asks of a capability.
///
/// Until 0.35.0 both were on the Index page. Design v6 moves them into the
/// store wizard's Sources step: `--projects` is the repository discovery that
/// decides whether a folder holds several repositories, and `--each` is "One
/// store each", which sends the wizard's run with `store: "each"`. Asserted
/// against the page's own source, because a flag documented in `--help` and
/// absent from the portal is exactly the debt this repository says it does not
/// carry.
#[test]
fn the_index_flags_have_a_portal_surface() {
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
        APP_JS.contains("\"One store each\"") && APP_JS.contains("? \"each\""),
        "the wizard offers no One store each, or does not send it as `each`, so --each has \
         no portal view"
    );
    assert!(
        APP_JS.contains("/api/projects"),
        "the wizard never asks for projects, so --projects has no portal view"
    );
}

/// The Privacy page's rules section is the release's claims with the daemon's
/// own reading beside each. A row that only stated the rule would be a sentence
/// somebody wrote.
#[test]
fn the_privacy_route_carries_a_rule_per_control_with_its_own_check() {
    let daemon = Daemon::start();
    let privacy = daemon.json("/api/privacy");
    let rules = privacy["rules"]
        .as_array()
        .expect("the privacy route carries no rules");

    // Every control, by the name the page shows.
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

/// The folder picker — the wizard's Browse and Adopt existing — is confined to
/// the user's home. With no home it used to root at the filesystem — `/` on
/// unix, whichever volume the process started on under Windows — and then
/// refuse the user's own profile as outside it. A picker that cannot be
/// confined is refused instead (#71).
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
/// Until 0.35.0 the Graph page kept an allow-list, `EDGE_KINDS`, and dropped
/// any edge whose kind was not on it — which is how `contains` and `aliases`
/// were fetched and never painted for two releases while the rail went on
/// counting them. The v6 Explore tab has no allow-list: its filter removes an
/// edge only when the chip for that kind or confidence is switched off, and
/// keeps everything else, which it draws in the "other" ink. So the guarantee
/// is now two checks rather than one list compared with another: every kind a
/// chip filters is a kind the store stores (a chip for a kind that does not
/// exist filters nothing), and the filter falls through to keeping the edge.
#[test]
fn the_graph_page_draws_every_edge_kind_the_store_stores() {
    let stored = semlith::graph::KINDS;
    let mut filtered = Vec::new();
    for piece in APP_JS.split("e.kind === \"").skip(1) {
        let kind = &piece[..piece.find('"').expect("a closed kind literal")];
        filtered.push(kind);
        assert!(
            stored.contains(&kind),
            "the Graph page filters edges of kind {kind:?}, which graph::KINDS does not have"
        );
    }
    assert!(
        !filtered.is_empty(),
        "the Graph page filters no edge kind at all; the chips are gone or renamed"
    );
    assert!(
        !APP_JS.contains("const EDGE_KINDS"),
        "an EDGE_KINDS allow-list is back: a kind missing from it would be fetched and dropped"
    );

    let start = APP_JS
        .find("const keep = d.edges.filter(")
        .expect("the Explore tab filters d.edges into `keep`");
    let body = &APP_JS[start..];
    let body = &body[..body.find("\n    });").expect("the filter is closed")];
    assert!(
        body.trim_end().ends_with("return true;"),
        "the edge filter does not end by keeping the edge, so a kind with no chip may be dropped:\n{body}"
    );
}

/// Text the portal can render, with comments and interpolations removed.
///
/// Not a JavaScript parser. It tracks the three quote characters, backslash
/// escapes, `//` and `/* */`, regular-expression literals, and drops the
/// `${...}` spans inside a template literal, which is all that is needed to
/// decide whether a run of digits is a sentence a user reads or a note to
/// whoever edits the file next.
///
/// Regular expressions were added in 0.35.0: the v6 page strips quotes from a
/// pasted path with `/^['"]|['"]$/g`, and a scanner that read that quote as
/// the start of a string was out of step for the rest of the file — it read
/// code as text and text as code, so both the version check and the selling
/// check were asking about the wrong characters. A `/` starts a regex where a
/// value is expected: after an operator, an opening bracket, a comma, a colon
/// or at the start of a line, which is how the page writes every one of them.
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
            '/' if src[..i]
                .iter()
                .rev()
                .find(|c| !c.is_whitespace())
                .is_none_or(|c| "(,=:[!&|?{};+-*%<>~^".contains(*c)) =>
            {
                // A regex literal: skip to its closing `/`, honouring escapes
                // and character classes, then its flags.
                i += 1;
                let mut class = false;
                while i < src.len() && src[i] != '\n' {
                    match src[i] {
                        '\\' => i += 1,
                        '[' => class = true,
                        ']' => class = false,
                        '/' if !class => break,
                        _ => {}
                    }
                    i += 1;
                }
                i += 1;
                while i < src.len() && src[i].is_ascii_alphabetic() {
                    i += 1;
                }
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
/// README. The portal is the same surface with a larger audience.
///
/// The live values are unaffected: Settings › About's version and binary rows,
/// the sidebar's version and the update card are `${...}` interpolations of
/// what the server just returned, and this reads none of them. v6 shows one
/// version string, the running binary's (0.35.0, item 2).
#[test]
fn the_portal_names_no_release_in_anything_a_user_reads() {
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

/// The Agents route carries what Settings › Agent access and Agents › Health
/// read.
///
/// The service status is the Start at login card; the per-client report is
/// Agents › Health, Home's Agents card and the wizard's Connect step. A route
/// that quietly stops sending either leaves those blank with nothing failing
/// anywhere else.
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
        "the doctor report on /api/agents is empty; Agents › Health lists no client at all"
    );
    for client in clients {
        let name = client["name"]
            .as_str()
            .unwrap_or_else(|| panic!("a doctor entry with no name: {client}"));
        // `in_use` is what the page filters on — it shows the clients somebody
        // here has, not the full catalogue — so an entry missing it would
        // silently drop off the card.
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

/// The sidebar: nine entries in three groups, in this order (design v6).
///
/// Until 0.35.0 it was fourteen pages in four groups. v6 rebuilds it around
/// the jobs a user comes to do; the old pages are now tabs and sections of
/// these nine, listed in `MOVED`. The store page (`#/store/<name>`) sits under
/// Stores and lights its entry, so it has no row of its own here.
const SIDEBAR: &[(&str, &str, &str)] = &[
    ("Workspace", "home", "Home"),
    ("Workspace", "stores", "Stores"),
    ("Workspace", "search", "Search"),
    ("Workspace", "graph", "Graph"),
    ("Agents", "agents", "Agents"),
    ("Agents", "ledger", "Ledger"),
    ("Agents", "reports", "Reports"),
    ("Machine", "privacy", "Privacy"),
    ("Machine", "settings", "Settings"),
];

/// The `["Group", ["id", …]]` rows of `NAV`, flattened in source order.
fn nav_rows() -> Vec<(String, String)> {
    let start = APP_JS.find("const NAV = [").expect("app.js defines NAV");
    let body = &APP_JS[start + "const NAV = [".len()..];
    let body = &body[..body.find("\n];").expect("NAV is closed")];
    let mut rows = Vec::new();
    for line in body.lines() {
        let quoted: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
        if let Some((group, ids)) = quoted.split_first() {
            for id in ids {
                rows.push((group.to_string(), id.to_string()));
            }
        }
    }
    rows
}

/// `id: { title: "…", group: "…" }` from `PAGES`.
fn page_entry(id: &str) -> Option<(String, String)> {
    let start = APP_JS.find("const PAGES = {")?;
    let body = &APP_JS[start..];
    let body = &body[..body.find("\n};")?];
    let line = body
        .lines()
        .find(|l| l.trim_start().starts_with(&format!("{id}: {{")))?;
    let field = |name: &str| -> Option<String> {
        let at = line.find(&format!("{name}: \""))?;
        let rest = &line[at + name.len() + 3..];
        Some(rest[..rest.find('"')?].to_string())
    };
    Some((field("title")?, field("group")?))
}

#[test]
fn the_sidebar_is_the_nine_pages_in_three_groups_in_order() {
    let rows = nav_rows();
    let got: Vec<(&str, &str)> = rows
        .iter()
        .map(|(g, id)| (g.as_str(), id.as_str()))
        .collect();
    let want: Vec<(&str, &str)> = SIDEBAR.iter().map(|(g, id, _)| (*g, *id)).collect();
    assert_eq!(
        got, want,
        "the sidebar's groups, entries or order have moved"
    );

    // The title and group a page carries agree with where the rail draws it:
    // the breadcrumb and the tab title read `PAGES`, the rail reads `NAV`.
    for (group, id, title) in SIDEBAR {
        let (t, g) = page_entry(id).unwrap_or_else(|| panic!("PAGES has no entry for {id}"));
        assert_eq!(&t, title, "{id} is titled {t:?} in PAGES");
        assert_eq!(
            &g, group,
            "{id} is grouped under {g:?} in PAGES and {group:?} in NAV"
        );
    }

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
    assert_eq!(groups, vec!["Workspace", "Agents", "Machine"]);
}

#[test]
fn every_sidebar_entry_has_a_mark_and_something_to_render() {
    for (_, id, title) in SIDEBAR {
        assert!(
            APP_JS.contains(&format!("\n  {id}: \"M")),
            "{title} has no icon in `I`, so the rail would draw a gap"
        );
        assert!(
            APP_JS.contains(&format!("\nVIEWS.{id} = {{")),
            "{title} has no view, so the entry navigates to nothing"
        );
    }
    // The pages that are not in the rail: a store's own page, the Welcome
    // screen and the store wizard.
    assert!(
        APP_JS.contains("\nVIEWS.store = {"),
        "no store page behind #/store/<name>"
    );
    assert!(
        APP_JS.contains("route.page === \"welcome\"")
            && APP_JS.contains("function welcomeScreen()"),
        "no Welcome screen behind #/welcome"
    );
    assert!(
        APP_JS.contains("route.page === \"new\"") && APP_JS.contains("function wizardScreen("),
        "no store wizard behind #/new"
    );
}

/// Where each surface of a page 0.35.0 removed went, and what proves it is
/// there: a tab's `[id, label]`, a button's text, or the route its control
/// posts to.
///
/// Item 19 of the 0.35.0 contract: views v6 does not draw are folded into v6
/// slots, not dropped. Only the cross-store Files list is dropped, because
/// Search's path filter covers it. A row that fails here is a capability that
/// lost its surface in the rewrite.
const MOVED: &[(&str, &str, &str)] = &[
    ("Files", "a store's Files tab", r#"["files", "Files"]"#),
    ("Index", "a store's Runs tab", r#"["runs", "Runs""#),
    ("Index", "the store wizard", "function wizardScreen("),
    (
        "Inside the index",
        "Stores › Inside the index (#/stores/inside)",
        r#"["inside", "Inside the index"]"#,
    ),
    (
        "Impact",
        "Graph › Blast radius (#/graph/blast)",
        r#"["blast", "Blast radius"]"#,
    ),
    (
        "path and trace",
        "Graph › Path & evidence (#/graph/path)",
        r#"["path", "Path & evidence"]"#,
    ),
    (
        "Doctor",
        "Agents › Health (#/agents/health)",
        r#"["health", "Health""#,
    ),
    (
        "About",
        "Settings › About (#/settings/about)",
        r#"["about", "About""#,
    ),
    (
        "Cloud",
        "Settings › Cloud (#/settings/cloud)",
        r#"["cloud", "Cloud""#,
    ),
    (
        "pattern",
        "Search's Pattern mode",
        r#"["pattern", "Pattern""#,
    ),
    (
        "prices",
        "Reports' savings card and Settings › About",
        "\"Update prices\")",
    ),
    (
        "usage from client logs",
        "the Ledger's switch",
        "post(\"/api/ledger/usage\"",
    ),
    (
        "root re-point",
        "a store's Settings tab",
        "post(\"/api/root\"",
    ),
    ("trust", "a store's Settings tab", "post(\"/api/trust\""),
    (
        "Install update",
        "Settings › About",
        "`Install ${up.latest}`",
    ),
    (
        "retention",
        "Settings › Performance",
        "\"history_retention_days\"",
    ),
];

#[test]
fn every_surface_of_a_removed_page_has_a_place_in_v6() {
    for (old, new, needle) in MOVED {
        assert!(
            APP_JS.contains(needle),
            "{old} was to live on {new}, and app.js has no {needle:?}"
        );
    }
    // Prices are on two surfaces, so the button is drawn twice.
    assert!(
        APP_JS.matches("\"Update prices\")").count() >= 2,
        "Update prices is on Reports and Settings › About; app.js draws it fewer than twice"
    );
}

/// Bulk acceptance by a person (0.35.0, item 19).
///
/// Until 0.35.0 a refused file was accepted one at a time, by a person, never
/// by an agent. The owner reversed the first half: the store Review tab and
/// the wizard's Review step decide on a selection, and the route takes a list.
/// The second half stands — it is enforced by the route, not the page, and is
/// tested where the route is. What this checks is that every decision the page
/// sends is a list of files, so no surface is left on the one-file form.
#[test]
fn the_review_surfaces_decide_on_a_list_of_files() {
    let calls: Vec<&str> = APP_JS
        .split("post(\"/api/refused/decide\", {")
        .skip(1)
        .map(|rest| &rest[..rest.find('}').expect("a closed body")])
        .collect();
    assert!(
        calls.len() >= 2,
        "the Review tab and the wizard's Review step both decide; app.js posts {} decisions",
        calls.len()
    );
    for body in calls {
        assert!(
            body.contains("files"),
            "a decision is posted without a list of files: {{{body}}}"
        );
    }
    assert!(
        !APP_JS.contains("/api/refused/accept") && !APP_JS.contains("/api/refused/revoke"),
        "the page still calls the one-file accept or revoke route"
    );
}

/// Session replay is on by default (0.35.0, item 19).
///
/// It was off by default until 0.35.0; the owner reversed that. A fresh
/// settings file reads as on, and the Privacy page's switch shows what this
/// route says. A user who turned it off explicitly stays off — that half is a
/// settings-file property, tested beside `home::Settings`.
#[test]
fn session_replay_is_on_for_a_fresh_settings_file() {
    let daemon = Daemon::start();
    let replay = daemon.json("/api/ledger/replay");
    assert_eq!(
        replay["enabled"], true,
        "a fresh daemon reports session replay off: {replay}"
    );
}

/// The governing rule since the v4 design: the binary is free and complete, so
/// no paid surface survives anywhere in the portal.
///
/// Checked over what the page can render rather than over the source, because
/// a comment recording why a lock was removed is not a lock. The one word
/// allowed through is "key", and only as the agent key.
#[test]
fn nothing_the_portal_renders_offers_to_sell_anything() {
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
            // and the route behind Settings › About's update card. Neither is
            // an offer to sell.
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
            // ledger" on Settings › Cloud is a description of the hosted
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
    // allow-listing every sentence that mentions the credential — Settings ›
    // Agent access is about the agent key and most of its copy says "key".
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

/// Every tool the server advertises is on Agents › Tools with no marker saying
/// it costs anything, and the cost row is measured rather than stated.
#[test]
fn the_agents_page_marks_no_tool_as_paid_and_measures_the_list() {
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

/// Settings › Cloud describes the service and contacts nothing.
///
/// It was a page of its own until 0.35.0; v6 makes it a Settings section. The
/// guarantee is unchanged: no connected state until the cloud client (0.38.0),
/// and no command behind it.
/// 0.37.0 ships `semlith cloud`, so the Cloud section draws two states: not
/// signed in (what the cloud adds, the two commands, no price) and signed in
/// (each org's header, reachability, remote stores, ledger sync, what leaves
/// the machine). Both read `/api/cloud`; the live half reads
/// `/api/cloud/status`, which a machine that never signed in answers with
/// nothing and no request.
#[test]
fn the_cloud_section_draws_both_states() {
    for needle in [
        "function seCloud()",
        "function seCloudOff()",
        "cloud: \"/api/cloud\"",
        "/api/cloud/status",
        "pill(\"not connected\"",
        "semlith cloud login <org>",
        "semlith cloud connect <org>",
        "What leaves this machine",
        "Ledger sync",
        "\"Disconnect\"",
        "syncing ${on} of",
        "· cloud: ",
    ] {
        assert!(APP_JS.contains(needle), "app.js lost `{needle}`");
    }
    let off = &APP_JS[APP_JS.find("function seCloudOff()").unwrap()..];
    // `\n}` and not `\n}\n`: a Windows checkout ends lines with CRLF.
    let off = &off[..off.find("\n}").unwrap()];
    for price in ["$", "₹", "per month", "/mo", "price"] {
        assert!(
            !off.contains(price),
            "the not-connected state names a price: {price}"
        );
    }
    assert!(
        subcommands().iter().any(|name| name == "cloud"),
        "`semlith cloud` is missing"
    );
    let daemon = Daemon::start();
    let status = daemon.json("/api/cloud/status");
    assert_eq!(status["orgs"], serde_json::json!([]), "{status}");
    let local = daemon.json("/api/cloud");
    assert_eq!(local["signed_in"], false, "{local}");
}

/// No element is sized or coloured by a `style` attribute.
///
/// The portal is served under `style-src 'self'` with no `unsafe-inline`, so
/// a width written that way is blocked and the bar renders at its default
/// size — silently, which is how three bars shipped at full width in
/// development before the browser drive caught them. Dynamic sizes go
/// through the CSSOM (`node.style.width = …`), which the policy allows.
#[test]
fn nothing_the_portal_builds_carries_an_inline_style_attribute() {
    let mut offenders = Vec::new();
    for (i, line) in APP_JS.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        // `el(…, { style: … })` and `"style":` set the attribute through the
        // element builder, `setAttribute("style", …)` sets it directly, and a
        // `style=` inside a string is markup somebody is about to parse.
        if trimmed.contains("style:")
            || trimmed.contains("\"style\"")
            || trimmed.contains("setAttribute(\"style\"")
            || trimmed.contains("setAttribute('style'")
            || trimmed.contains("style=")
        {
            offenders.push(format!("app.js:{}: {}", i + 1, trimmed));
        }
    }
    for (i, line) in INDEX_HTML.lines().enumerate() {
        if line.contains("style=") || line.contains("<style") {
            offenders.push(format!("index.html:{}: {}", i + 1, line.trim()));
        }
    }
    assert!(
        offenders.is_empty(),
        "these would be dropped by the portal's own content-security policy:\n  {}",
        offenders.join("\n  ")
    );
}

/// `docs/portal.md` documents every page (AGENTS.md, "Portal parity"): each
/// sidebar page has a section, and so do the three screens outside the rail.
#[test]
fn the_portal_doc_has_a_section_for_every_page() {
    let headings: Vec<&str> = PORTAL_MD
        .lines()
        .filter_map(|l| l.strip_prefix("## "))
        .collect();
    for (_, _, title) in SIDEBAR {
        assert!(
            headings.contains(title),
            "docs/portal.md has no `## {title}` section"
        );
    }
    for title in ["Welcome", "The store wizard", "A store's page"] {
        assert!(
            headings.contains(&title),
            "docs/portal.md has no `## {title}` section"
        );
    }
}
