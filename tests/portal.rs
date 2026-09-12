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
const NO_VIEW: [&str; 2] = ["start", "mcp"];

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
    ("stats", "/api/stores"),
    ("files", "/api/files"),
    ("forget", "/api/forget"),
    ("adopt", "/api/adopt"),
    ("models", "/api/models"),
    ("languages", "/api/languages"),
    ("setup", "/api/setup"),
    // A check and an install both reach the network, so neither is something a
    // route answers to a GET that a browser might replay.
    ("upgrade", "/api/upgrade"),
];

/// And the same for the MCP tool surface.
const TOOL_VIEWS: &[(&str, &str)] = &[
    ("semlith_search", "/api/search"),
    ("semlith_stats", "/api/stores"),
    ("semlith_files", "/api/files"),
    ("semlith_index", "/api/index"),
    ("semlith_add", "/api/add"),
    ("semlith_forget", "/api/forget"),
];

/// Which verb a route answers on.
///
/// One list, because there were three of these and they had already drifted
/// apart from each other — a route added to one and forgotten in the next is a
/// test that passes by asking the wrong question and reports 405 as a missing
/// view.
fn method_for(route: &str) -> &'static str {
    match route {
        "/api/index" | "/api/add" | "/api/forget" | "/api/adopt" | "/api/upgrade" => "POST",
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
        let dir = tempfile::Builder::new()
            .prefix("semlith-parity-")
            .tempdir()
            .expect("a temporary directory");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();

        let mut child = Command::new(env!("CARGO_BIN_EXE_semlith"))
            .arg("start")
            .arg("--port")
            .arg("0")
            .env("SEMLITH_HOME", &home)
            .env("HOME", &home)
            .env_remove("SEMLITH_STORE")
            .env_remove("SEMLITH_PORT")
            .current_dir(dir.path())
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
            _dir: dir,
        }
    }

    /// The status a route answers with. 404 is the only failing answer here:
    /// this test is about a route existing, not about what it returns for an
    /// empty daemon.
    fn status(&self, method: &str, path: &str) -> u16 {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("the daemon listens");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nCookie: semlith_token={}\r\n\
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
            "GET /api/agents HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nCookie: semlith_token={}\r\nConnection: close\r\n\r\n",
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
    let tools: Vec<String> = agents["tools"]
        .as_array()
        .expect("a tool list")
        .iter()
        .map(|t| t.as_str().unwrap().to_string())
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
            "GET {route} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nCookie: semlith_token={}\r\nConnection: close\r\n\r\n",
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
