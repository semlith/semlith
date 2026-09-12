//! `semlith mcp` forwarding to a running `semlith start`.
//!
//! Without this the daemon would reintroduce the very conflict it exists to
//! remove. The daemon holds every store's write lock for its life, so an
//! agent's `semlith_index` against a store a portal is open on would be refused
//! for as long as the portal stayed open — which is worse than 0.8.0, not
//! better.
//!
//! So when a daemon is running for a store this process would have opened,
//! `semlith mcp` stops being a server and becomes a pipe: each JSON-RPC line
//! goes to the daemon's `/api/mcp` route and the daemon's answer comes back
//! unchanged. The daemon runs the same [`crate::mcp::answer`] this process
//! would have run, so every supported protocol revision behaves identically
//! through the proxy — there is no second implementation to drift.
//!
//! When no daemon is running, or one is recorded but does not answer, nothing
//! here happens and `semlith mcp` behaves exactly as 0.8.0 did.

use crate::daemon::Discovery;
use anyhow::{Context, Result, bail};
use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a forwarded call may take. Generous, because the far side may be
/// embedding a corpus on this call's behalf; the MCP index budget bounds that
/// at 45 seconds, and this leaves room around it.
const CALL_TIMEOUT: Duration = Duration::from_secs(90);

/// How long the liveness probe waits. Short: a stale discovery file pointing at
/// a dead port must not make every `semlith mcp` start slowly.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// A daemon this process should forward to.
pub struct Upstream {
    pub port: u16,
    token: String,
    /// The store directory whose discovery file named it, for the stderr line.
    pub via: PathBuf,
}

/// Find a live daemon among the stores this process was going to open.
///
/// Returns `None` for every failure — no discovery file, an unreadable one, a
/// stale one pointing at a dead port — because every one of them means the same
/// thing to the caller: serve the stores directly, as before.
pub fn find(dirs: &[PathBuf]) -> Option<Upstream> {
    for dir in dirs {
        let Some(discovery) = Discovery::read(dir) else {
            continue;
        };
        let upstream = Upstream {
            port: discovery.port,
            token: discovery.token,
            via: dir.clone(),
        };
        if upstream.alive() {
            return Some(upstream);
        }
        // A file left by a daemon that was killed. Saying so is worth a line:
        // the alternative is a user wondering why the portal's Agents view
        // never shows their client.
        eprintln!(
            "semlith: {} names a daemon on port {} that is not answering; \
             opening the store directly",
            dir.join(crate::daemon::DISCOVERY_FILE).display(),
            upstream.port,
        );
    }
    None
}

impl Upstream {
    fn alive(&self) -> bool {
        self.request("GET", "/api/about", None, PROBE_TIMEOUT)
            .is_ok_and(|body| body.contains("\"version\""))
    }

    /// Forward one JSON-RPC request and return the daemon's answer.
    pub fn call(&self, request: &str) -> Result<String> {
        self.request("POST", "/api/mcp", Some(request), CALL_TIMEOUT)
    }

    /// One HTTP request over loopback, hand-written for the same reason the
    /// server is: this is the whole of the client semlith needs.
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        timeout: Duration,
    ) -> Result<String> {
        let address = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, self.port));
        let mut stream = TcpStream::connect_timeout(&address, timeout)
            .with_context(|| format!("connecting to the daemon on 127.0.0.1:{}", self.port))?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;

        let body = body.unwrap_or("");
        let head = format!(
            "{method} {path} HTTP/1.1\r\n\
             Host: 127.0.0.1:{}\r\n\
             Cookie: {}={}\r\n\
             Semlith-Proxy: {}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\r\n",
            self.port,
            crate::http::TOKEN_COOKIE,
            self.token,
            std::process::id(),
            body.len(),
        );
        stream.write_all(head.as_bytes())?;
        stream.write_all(body.as_bytes())?;
        stream.flush()?;

        let mut raw = Vec::new();
        stream.read_to_end(&mut raw)?;
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (headers, payload) = text
            .split_once("\r\n\r\n")
            .context("the daemon sent no complete response")?;

        let status = headers
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .unwrap_or("");
        if status != "200" {
            bail!("the daemon answered {status}");
        }
        Ok(payload.to_string())
    }
}

/// Be the pipe: every line in, the daemon's answer out.
///
/// stdout stays protocol and stderr stays diagnostics, exactly as the in-process
/// server does. A call that fails after the daemon was found alive is answered
/// with a JSON-RPC error naming it rather than by silently closing, because a
/// client that loses its server mid-conversation reports a hung tool.
pub fn serve(upstream: &Upstream, input: impl BufRead, mut output: impl Write) -> Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        // Notifications carry no id and must not be answered — including not
        // being answered with an error if the forward fails.
        let id = serde_json::from_str::<serde_json::Value>(&line)
            .ok()
            .and_then(|v| v.get("id").cloned());

        match upstream.call(&line) {
            Ok(answer) => {
                let answer = answer.trim();
                if !answer.is_empty() {
                    writeln!(output, "{answer}")?;
                    output.flush()?;
                }
            }
            Err(e) => {
                let Some(id) = id else { continue };
                let error = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {
                        "code": -32603,
                        "message": format!(
                            "the semlith daemon on 127.0.0.1:{} stopped answering: {e}",
                            upstream.port
                        ),
                    },
                });
                writeln!(output, "{error}")?;
                output.flush()?;
            }
        }
    }
    Ok(())
}

/// Which store directory a proxy should look in first.
///
/// Only the directories this process would have opened anyway: a daemon on some
/// other store is not this server's business, and forwarding to it would answer
/// questions about a corpus the client never asked for.
pub fn candidates(dirs: &[PathBuf]) -> Vec<PathBuf> {
    dirs.iter()
        .filter(|d| d.join("store.db").exists())
        .cloned()
        .collect()
}

/// The pid a forwarding process announces, so the Agents view can count them.
pub fn proxy_pid(header: Option<&str>) -> Option<u32> {
    header?.trim().parse().ok()
}

/// Whether this directory currently has a daemon recorded against it.
pub fn daemon_recorded(dir: &Path) -> bool {
    Discovery::read(dir).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_announces_a_parseable_pid() {
        assert_eq!(proxy_pid(Some("4213")), Some(4213));
        assert_eq!(proxy_pid(Some(" 4213 ")), Some(4213));
        assert_eq!(proxy_pid(Some("not-a-pid")), None);
        assert_eq!(proxy_pid(None), None);
    }

    /// A directory that is not a store cannot be holding a daemon worth
    /// forwarding to, and probing it would cost a connect per start.
    #[test]
    fn only_real_store_directories_are_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("a");
        let empty = dir.path().join("b");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::write(store.join("store.db"), b"").unwrap();

        assert_eq!(
            candidates(&[store.clone(), empty]),
            vec![store],
            "a directory with no store.db is not a candidate"
        );
    }
}
