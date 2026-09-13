//! A synchronous HTTP/1.1 server, just large enough to serve the portal.
//!
//! Nothing here is a general-purpose web server and nothing here should become
//! one. It answers requests from one browser on one machine over the loopback
//! interface, which is why it is a few hundred lines of `std::net` rather than
//! a dependency: every HTTP crate that does chunked responses and a thread pool
//! brings a tree semlith would then be shipping to every user of a local search
//! tool, and the parts of HTTP this needs are the parts that fit on a page.
//!
//! Three rules are enforced here, before any handler sees a request, because
//! they are the product's privacy claim and a claim enforced in one place is
//! one that can be read in one place:
//!
//! - **Loopback only.** [`Server::bind`] binds `127.0.0.1` and there is no flag
//!   to change it.
//! - **A per-run token, carried in a request header.** Generated at start,
//!   printed once in the URL, handed to the page from that URL, and sent back
//!   as `Semlith-Token` on every API request. A request without it gets 401 and
//!   an empty body — not a login page, not an error object, nothing that tells a
//!   prober what is here.
//!
//!   It was a cookie until 0.14.0, and a cookie is the wrong container for it:
//!   `localhost` is one site however many ports are listening on it, so
//!   `SameSite=Strict` never separated this server from a page served by
//!   anything else on 127.0.0.1. A header the browser will not attach
//!   cross-origin does separate them. The cost is that a browser attaches no
//!   header to a stylesheet, a font, a favicon or an `<img src>`, so the page
//!   and its own static assets are served without a credential — they are the
//!   same bytes in every copy of the binary — and everything that answers about
//!   this machine is not.
//!
//! - **A write has to come from this origin.** Every request that is not a GET
//!   or a HEAD must carry `Sec-Fetch-Site: same-origin`, or — for a client that
//!   sends no fetch metadata at all, which is every client that is not a
//!   browser — an `Origin` matching this server or no `Origin`. It must also
//!   carry a JSON content type, because a body a browser can send with no
//!   preflight is a body this server will not read. Both are checked before the
//!   token, so a cross-origin page learns nothing from the difference between a
//!   right and a wrong guess.
//! - **A persisted agent key, and only for `/mcp`.** From 0.13.0 the daemon
//!   also answers MCP over HTTP, and that endpoint needs a credential a client
//!   can keep in a configuration file — which the session token can never be,
//!   because it changes every run. The two have deliberately separate
//!   lifetimes and separate reach: the agent key opens `/mcp` and nothing else,
//!   so a key in a config file can never reach the rotate, adopt or upgrade
//!   routes, and rotating the session token does not disconnect an agent.
//! - **A `Host` header that is localhost.** A page on another origin cannot
//!   read a cross-origin response, but it can *send* the request, and a DNS
//!   name that resolves to 127.0.0.1 would otherwise reach this server with the
//!   browser's cookies attached. Only `localhost` and `127.0.0.1`, with an
//!   optional port, are answered.
//!
//! Every response also carries a Content-Security-Policy that allows only
//! `'self'`, and no CORS header is ever emitted — there is no origin this
//! server wants to be readable from.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// The port `semlith start` listens on unless told otherwise.
///
/// 7365 spells SEML on a phone keypad, is registered to nothing in the IANA
/// list, sits below every OS ephemeral range, and is nowhere near the ports
/// local development already uses — 3000, 4200, 5000, 5173, 8000, 8080, 8888,
/// 9000, and the usual database and model-server ports. A stable port is what
/// lets the URL be a bookmark.
pub const DEFAULT_PORT: u16 = 7365;

/// Overrides [`DEFAULT_PORT`].
pub const PORT_ENV: &str = "SEMLITH_PORT";

/// The header the session token travels in.
///
/// A custom header is the point rather than a detail: a browser will not attach
/// one to a request a page on another origin made, and it cannot set one at all
/// on an `<img>`, a form or a stylesheet. That is the whole separation a cookie
/// could not give, because every port on `localhost` is the same site.
pub const TOKEN_HEADER: &str = "Semlith-Token";

/// [`TOKEN_HEADER`] as the map holds it. Header names are lowercased on the way
/// in so a lookup never depends on a client's casing.
const TOKEN_HEADER_KEY: &str = "semlith-token";

/// The one path the agent key opens.
pub const MCP_PATH: &str = "/mcp";

/// Threads answering requests.
///
/// A browser opens up to six connections to one origin, and this server closes
/// each connection after its response, so eight is enough for one portal with
/// a long-running index stream on one of them and nothing queueing behind it.
const WORKERS: usize = 8;

/// The largest request line and header block accepted. A browser's is under
/// 2 KiB; anything near this is not a browser.
const MAX_HEADERS: usize = 16 * 1024;

/// The largest request body accepted. The only bodies this server takes are
/// small JSON objects naming paths.
const MAX_BODY: usize = 1024 * 1024;

/// How long a connection may sit idle mid-request before it is dropped, so a
/// half-open socket cannot hold a worker forever.
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Set when the accept loop should be woken so it can notice `stop`.
static WAKE: AtomicBool = AtomicBool::new(false);

/// Why a request was refused, for the one-line-per-class count the daemon logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Refusal {
    /// No token, or the wrong one.
    Unauthorized,
    /// A `Host` header that is not loopback.
    ForeignHost,
    /// Malformed, oversized, or a method this server does not answer.
    Malformed,
    /// A write that did not come from this origin.
    CrossOrigin,
    /// A write whose body was not offered as JSON.
    BadContentType,
    /// An index or a fetch for a path outside the store's boundary.
    OutsideBoundary,
    /// An index for a path the deny-list names.
    DeniedPath,
    /// A fetch for an address that is not on the public internet.
    PrivateAddress,
}

impl Refusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Refusal::Unauthorized => "unauthorized",
            Refusal::ForeignHost => "foreign host",
            Refusal::Malformed => "malformed",
            Refusal::CrossOrigin => "cross-origin",
            Refusal::BadContentType => "bad-content-type",
            Refusal::OutsideBoundary => "outside-boundary",
            Refusal::DeniedPath => "denied-path",
            Refusal::PrivateAddress => "private-address",
        }
    }
}

/// One parsed request.
pub struct Request {
    pub method: String,
    /// Path with the query string removed, percent-decoded.
    pub path: String,
    pub query: BTreeMap<String, String>,
    /// Header names lowercased, so a lookup never depends on a client's casing.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn query(&self, key: &str) -> Option<&str> {
        self.query.get(key).map(String::as_str)
    }

    /// Every value a repeated query key has, for `?lang=rust&lang=go`.
    ///
    /// The map holds them joined by `\u{1}`, which cannot appear in a
    /// percent-decoded query value that a browser produced.
    pub fn query_all(&self, key: &str) -> Vec<String> {
        match self.query.get(key) {
            Some(v) => v.split('\u{1}').map(str::to_string).collect(),
            None => Vec::new(),
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// The body as JSON, and the only place a body is read.
    ///
    /// The content type is checked here as well as in [`answer`], so a route
    /// that grew a second body reader would still be refusing the one thing a
    /// cross-origin form can send without a preflight.
    pub fn json(&self) -> Result<serde_json::Value> {
        anyhow::ensure!(
            json_body(self.header("content-type")),
            "this route reads JSON; send Content-Type: application/json"
        );
        Ok(serde_json::from_slice(&self.body)?)
    }
}

/// Whether a `Content-Type` offers a JSON body.
///
/// `application/json` and nothing else. The three types a browser will post
/// from a form without asking anybody's permission first — `text/plain`,
/// `application/x-www-form-urlencoded`, `multipart/form-data` — are exactly the
/// ones this refuses.
fn json_body(content_type: Option<&str>) -> bool {
    content_type
        .map(|value| value.split(';').next().unwrap_or("").trim())
        .is_some_and(|media| media.eq_ignore_ascii_case("application/json"))
}

/// What a handler writes a streaming body through.
///
/// Each [`Chunks::send`] is one HTTP chunk, flushed immediately, which is what
/// makes an index run's progress visible while it is still running rather than
/// all at once when it ends.
pub struct Chunks<'a> {
    out: &'a mut dyn Write,
}

impl Chunks<'_> {
    pub fn send(&mut self, line: &str) -> std::io::Result<()> {
        write!(self.out, "{:x}\r\n", line.len())?;
        self.out.write_all(line.as_bytes())?;
        self.out.write_all(b"\r\n")?;
        self.out.flush()
    }
}

type Streamer = Box<dyn FnOnce(&mut Chunks) -> std::io::Result<()> + Send>;

enum Body {
    Bytes(Vec<u8>),
    /// Sent with `Transfer-Encoding: chunked`, for a response whose length is
    /// not known when it starts.
    Stream(Streamer),
}

pub struct Response {
    status: u16,
    content_type: &'static str,
    extra: Vec<(String, String)>,
    body: Body,
}

impl Response {
    pub fn new(status: u16, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type,
            extra: Vec::new(),
            body: Body::Bytes(body),
        }
    }

    pub fn json(value: &serde_json::Value) -> Self {
        Self::new(
            200,
            "application/json; charset=utf-8",
            serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec()),
        )
    }

    /// A refusal a handler decided on: still JSON, because the portal shows it.
    pub fn error(status: u16, message: &str) -> Self {
        Self::new(
            status,
            "application/json; charset=utf-8",
            serde_json::to_vec(&serde_json::json!({ "error": message }))
                .unwrap_or_else(|_| b"{}".to_vec()),
        )
    }

    pub fn asset(content_type: &'static str, bytes: &'static [u8]) -> Self {
        Self::new(200, content_type, bytes.to_vec())
    }

    /// A response whose body is produced as it goes.
    pub fn stream(f: impl FnOnce(&mut Chunks) -> std::io::Result<()> + Send + 'static) -> Self {
        Self {
            status: 200,
            content_type: "application/x-ndjson; charset=utf-8",
            extra: Vec::new(),
            body: Body::Stream(Box::new(f)),
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.extra.push((name.to_string(), value.into()));
        self
    }

}

/// A handler: everything the daemon serves, behind one function.
pub type Handler = Arc<dyn Fn(&Request) -> Response + Send + Sync>;

/// The agent key, and the one it replaced.
///
/// `previous` is kept valid until this process exits so that a session in
/// flight when somebody rotated finishes rather than failing mid-answer. It is
/// not persisted: a restart is where the old key stops being accepted, which
/// is also the moment every client had to be told about the new one anyway.
#[derive(Default)]
struct Agent {
    current: String,
    previous: Option<String>,
}

pub struct Server {
    listener: TcpListener,
    port: u16,
    /// Behind a lock so the Privacy page's Rotate button can replace it while
    /// the server is running.
    token: Arc<Mutex<String>>,
    agent: Arc<Mutex<Agent>>,
    /// Whether `/mcp` answers. Closing it drops the route, not the daemon.
    mcp_open: Arc<AtomicBool>,
}

impl Server {
    /// Bind the loopback interface, or explain what is already on the port.
    ///
    /// Never falls back to another port. A URL that moves is not a bookmark,
    /// and from 0.11.0 it is also a client's MCP endpoint — so a taken port is
    /// an error naming the flag, not a silent reassignment.
    pub fn bind(port: u16) -> Result<Self> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let listener = TcpListener::bind(addr).with_context(|| {
            format!(
                "port {port} is already in use — stop whatever is listening on it, \
                 or choose another with --port or {PORT_ENV}"
            )
        })?;
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        Ok(Self {
            listener,
            port,
            token: Arc::new(Mutex::new(new_token())),
            agent: Arc::new(Mutex::new(Agent::default())),
            mcp_open: Arc::new(AtomicBool::new(true)),
        })
    }

    /// Install the persisted agent key. Until this is called `/mcp` accepts
    /// the session token alone, because an empty key matches nothing.
    pub fn set_agent_key(&self, key: &str) {
        self.agent.lock().expect("the agent lock").current = key.to_string();
    }

    pub fn agent_key(&self) -> String {
        self.agent.lock().expect("the agent lock").current.clone()
    }

    /// Replace the agent key.
    ///
    /// `now` drops the previous key immediately; otherwise it stays valid
    /// until this process exits, so a client mid-session finishes its work and
    /// only then needs the new stanza.
    pub fn rotate_agent(&self, key: &str, now: bool) {
        let mut agent = self.agent.lock().expect("the agent lock");
        let was = std::mem::replace(&mut agent.current, key.to_string());
        agent.previous = if now || was.is_empty() {
            None
        } else {
            Some(was)
        };
    }

    pub fn mcp_open(&self) -> bool {
        self.mcp_open.load(Ordering::Relaxed)
    }

    pub fn set_mcp_open(&self, open: bool) {
        self.mcp_open.store(open, Ordering::Relaxed);
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn token(&self) -> String {
        self.token.lock().expect("the token lock").clone()
    }

    /// Invalidate the current token and return the new one.
    pub fn rotate(&self) -> String {
        let fresh = new_token();
        *self.token.lock().expect("the token lock") = fresh.clone();
        fresh
    }

    /// The URL to print. The token appears here and nowhere else.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/?token={}", self.port, self.token())
    }

    /// Serve until `stop` is set, calling `refused` once per refused request.
    ///
    /// `stop` is `'static` because a thread has to watch it: `accept` blocks,
    /// and something has to notice the flag and wake it. Polling `accept`
    /// instead would put the poll interval on the front of every request —
    /// this server closes each connection after answering, so that is every
    /// request, not every session.
    pub fn serve(
        &self,
        handler: Handler,
        stop: &'static AtomicBool,
        refused: impl Fn(Refusal) + Send + Sync + 'static,
    ) -> Result<()> {
        let (tx, rx) = mpsc::channel::<TcpStream>();
        let rx = Arc::new(Mutex::new(rx));
        let refused = Arc::new(refused);

        let mut workers = Vec::with_capacity(WORKERS);
        let port = self.port;
        for _ in 0..WORKERS {
            let rx = Arc::clone(&rx);
            let handler = Arc::clone(&handler);
            let token = Arc::clone(&self.token);
            let agent = Arc::clone(&self.agent);
            let mcp_open = Arc::clone(&self.mcp_open);
            let refused = Arc::clone(&refused);
            workers.push(std::thread::spawn(move || {
                loop {
                    // The receiver is behind a mutex only for the recv; a
                    // worker that took a connection does not hold up the next.
                    let next = {
                        let guard = rx.lock().expect("the queue lock");
                        guard.recv()
                    };
                    let Ok(stream) = next else { return };
                    let auth = {
                        let agent = agent.lock().expect("the agent lock");
                        Auth {
                            token: token.lock().expect("the token lock").clone(),
                            agent: agent.current.clone(),
                            previous: agent.previous.clone(),
                            mcp_open: mcp_open.load(Ordering::Relaxed),
                            port,
                        }
                    };
                    if let Some(class) = answer(stream, &handler, &auth) {
                        refused(class);
                    }
                }
            }));
        }

        // The waker: four wakeups a second while idle, nothing per request.
        // `WAKE` covers the case where the loop leaves for its own reasons, so
        // this thread never outlives the server that started it.
        let waker = {
            let port = self.port;
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) && !WAKE.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(250));
                }
                // One connection that goes nowhere, purely to return `accept`.
                let _ = TcpStream::connect(("127.0.0.1", port));
            })
        };

        while !stop.load(Ordering::Relaxed) {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    // The waker's own connection arrives here too; it carries
                    // no request, so `answer` reads nothing and closes it.
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
                    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                    if tx.send(stream).is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    WAKE.store(true, Ordering::Relaxed);
                    let _ = waker.join();
                    return Err(e).context("accepting a connection");
                }
            }
        }

        WAKE.store(true, Ordering::Relaxed);
        let _ = waker.join();

        // Dropping the sender ends every worker's `recv`, so shutdown finishes
        // the requests in flight and starts no more.
        drop(tx);
        for worker in workers {
            let _ = worker.join();
        }
        Ok(())
    }
}

/// The credentials in force for one request.
struct Auth {
    token: String,
    agent: String,
    previous: Option<String>,
    mcp_open: bool,
    /// The port this server answers on, so an `Origin` header can be compared
    /// against the origin it actually names.
    port: u16,
}

impl Auth {
    /// Whether a bearer credential opens the MCP endpoint.
    ///
    /// An empty configured key matches nothing: `same` compares lengths first,
    /// and an empty bearer against an empty key would otherwise be a match.
    fn is_agent(&self, bearer: &str) -> bool {
        if bearer.is_empty() {
            return false;
        }
        if !self.agent.is_empty() && same(bearer, &self.agent) {
            return true;
        }
        self.previous
            .as_deref()
            .is_some_and(|previous| !previous.is_empty() && same(bearer, previous))
    }
}

/// Answer one connection. Returns why it was refused, if it was.
fn answer(mut stream: TcpStream, handler: &Handler, auth: &Auth) -> Option<Refusal> {
    let request = match read_request(&mut stream) {
        Ok(Some(r)) => r,
        Ok(None) => return None,
        Err(_) => {
            let _ = write_response(&mut stream, Response::new(400, "text/plain", Vec::new()));
            return Some(Refusal::Malformed);
        }
    };

    // Host first: a request from a foreign origin is refused before the token
    // is even compared, so a wrong guess and a right one cost the same.
    if !loopback_host(request.header("host")) {
        let _ = write_response(&mut stream, Response::new(400, "text/plain", Vec::new()));
        return Some(Refusal::ForeignHost);
    }

    let want = auth.token.as_str();
    let for_mcp = request.path == MCP_PATH;

    // A closed endpoint is not a route. Answered before the credential is
    // looked at, so a client that was told to stop learns the same thing
    // whether or not it still holds a key.
    if for_mcp && !auth.mcp_open {
        let _ = write_response(&mut stream, Response::error(404, "no such route"));
        return None;
    }

    // HEAD is GET without the body. Answered by running the handler and
    // dropping what it produced, so a probe sees the real status and the real
    // headers rather than the 404 an unhandled method would give it.
    let head_only = request.method == "HEAD";
    let reading = head_only || request.method == "GET";

    // A write proves where it came from before anything else looks at it, so a
    // page on another local port gets the same answer whether its token guess
    // was right or wrong — and gets it without a handler having run.
    if !reading {
        if !same_origin(&request, auth.port) {
            let _ = write_response(&mut stream, Response::new(403, "text/plain", Vec::new()));
            return Some(Refusal::CrossOrigin);
        }
        if !json_body(request.header("content-type")) {
            let _ = write_response(&mut stream, Response::new(403, "text/plain", Vec::new()));
            return Some(Refusal::BadContentType);
        }
    }

    // The page and the bytes it loads carry no credential, because a browser
    // attaches no header to a stylesheet, a font or a favicon. They are the
    // same bytes in every copy of this binary; everything that answers about
    // this machine is below and needs the token.
    if !public(&request, reading) {
        // The agent key opens `/mcp` and nothing else. This is the whole reason
        // there are two credentials: a key that a client keeps in a file on disk
        // must not be able to rotate a token, adopt a store or start an upgrade.
        let by_agent = for_mcp
            && request
                .header("authorization")
                .and_then(|value| value.strip_prefix("Bearer "))
                .is_some_and(|bearer| auth.is_agent(bearer.trim()));
        let by_header = request
            .header(TOKEN_HEADER_KEY)
            .is_some_and(|t| same(t.trim(), want));

        if !by_header && !by_agent {
            // Empty body on purpose: a prober learns that something refused it
            // and nothing else.
            let _ = write_response(&mut stream, Response::new(401, "text/plain", Vec::new()));
            return Some(Refusal::Unauthorized);
        }
    }

    let request = if head_only {
        Request {
            method: "GET".to_string(),
            ..request
        }
    } else {
        request
    };

    let mut response = handler(&request);
    if head_only {
        response.body = Body::Bytes(Vec::new());
    }
    let _ = write_response(&mut stream, response);
    None
}

/// Whether this request may be answered without a credential.
///
/// The page itself and the files it loads, and nothing else. The token in the
/// printed URL's query is never read here: it is handed to the page, which
/// sends it back in a header, so a reload with no token in the address bar
/// still gets the page and a request for data still does not.
fn public(request: &Request, reading: bool) -> bool {
    reading && (request.path == "/" || crate::portal::asset(&request.path).is_some())
}

/// Whether a write came from this server's own origin.
///
/// `Sec-Fetch-Site` is the browser's own statement and cannot be set by script,
/// so when it is present it decides — and `same-site` is a refusal, because
/// every other port on `localhost` is same-site and none of them is this
/// server. A client that sends no fetch metadata is not a browser; it is judged
/// on `Origin` if it sent one and on the token alone if it did not.
fn same_origin(request: &Request, port: u16) -> bool {
    if let Some(site) = request.header("sec-fetch-site") {
        return site.trim().eq_ignore_ascii_case("same-origin");
    }
    match request.header("origin") {
        Some(origin) => own_origin(origin.trim(), port),
        None => true,
    }
}

/// Whether an `Origin` names this server: loopback, over http, on this port.
fn own_origin(origin: &str, port: u16) -> bool {
    let Some(authority) = origin.strip_prefix("http://") else {
        return false;
    };
    let Some((host, stated)) = authority.rsplit_once(':') else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let loopback =
        host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1";
    loopback && stated.parse::<u16>() == Ok(port)
}

/// Whether `Host` is this machine talking to itself.
///
/// A missing header is refused: HTTP/1.1 requires one, and the clients that
/// omit it are not browsers.
fn loopback_host(host: Option<&str>) -> bool {
    let Some(host) = host else { return false };
    let name = host.rsplit_once(':').map_or(host, |(n, _)| n);
    let name = name.trim_start_matches('[').trim_end_matches(']');
    name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1" || name == "::1"
}

/// Compare in time that does not depend on where the first wrong byte is.
///
/// The token is a secret and this is the check that guards it. The difference
/// is unmeasurable across a loopback socket, but a constant-time compare costs
/// nothing and removes the question.
fn same(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn read_request(stream: &mut TcpStream) -> Result<Option<Request>> {
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut head = String::new();
    let mut read = 0;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            // The peer closed before sending anything: a browser probing a
            // connection it then did not use. Not a refusal, not an error.
            return Ok(head.is_empty().then_some(None).flatten());
        }
        read += n;
        anyhow::ensure!(read <= MAX_HEADERS, "request headers too large");
        if line == "\r\n" || line == "\n" {
            break;
        }
        head.push_str(&line);
    }

    let mut lines = head.lines();
    let start = lines.next().context("empty request")?;
    let mut parts = start.split_whitespace();
    let method = parts.next().context("no method")?.to_string();
    let target = parts.next().context("no target")?.to_string();

    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let (raw_path, raw_query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target.as_str(), ""),
    };

    let mut body = Vec::new();
    if let Some(len) = headers.get("content-length").and_then(|v| v.parse().ok()) {
        let len: usize = len;
        anyhow::ensure!(len <= MAX_BODY, "request body too large");
        body.resize(len, 0);
        reader.read_exact(&mut body)?;
    }

    Ok(Some(Request {
        method,
        path: percent_decode(raw_path),
        query: parse_query(raw_query),
        headers,
        body,
    }))
}

/// `a=1&b=2` to a map, with a repeated key's values joined by `\u{1}`.
fn parse_query(raw: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for pair in raw.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decode(&key.replace('+', " "));
        let value = percent_decode(&value.replace('+', " "));
        out.entry(key)
            .and_modify(|existing| {
                existing.push('\u{1}');
                existing.push_str(&value);
            })
            .or_insert(value);
    }
    out
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn write_response(stream: &mut TcpStream, response: Response) -> std::io::Result<()> {
    let Response {
        status,
        content_type,
        extra,
        body,
    } = response;

    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    head.push_str(&format!("Content-Type: {content_type}\r\n"));
    // Everything the page loads is served by this server, nothing may frame
    // it, and no form may post anywhere else. There is no origin this server
    // wants to be readable from, so there is no CORS header here or anywhere.
    head.push_str(
        "Content-Security-Policy: default-src 'self'; \
         img-src 'self' data:; \
         style-src 'self'; \
         script-src 'self'; \
         font-src 'self'; \
         connect-src 'self'; \
         form-action 'none'; \
         frame-ancestors 'none'; \
         base-uri 'none'\r\n",
    );
    head.push_str("X-Content-Type-Options: nosniff\r\n");
    head.push_str("Referrer-Policy: no-referrer\r\n");
    // The token lives in the first URL. Nothing here may be stored by a proxy
    // that does not exist, but a browser's back-forward cache holding a
    // rotated token is a real thing to avoid.
    head.push_str("Cache-Control: no-store\r\n");
    head.push_str("Connection: close\r\n");
    for (name, value) in &extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }

    match body {
        Body::Bytes(bytes) => {
            head.push_str(&format!("Content-Length: {}\r\n\r\n", bytes.len()));
            stream.write_all(head.as_bytes())?;
            stream.write_all(&bytes)?;
            stream.flush()
        }
        Body::Stream(f) => {
            head.push_str("Transfer-Encoding: chunked\r\n\r\n");
            stream.write_all(head.as_bytes())?;
            stream.flush()?;
            let result = {
                let mut chunks = Chunks { out: stream };
                f(&mut chunks)
            };
            // The terminating chunk goes out whether or not the producer
            // failed: a client reading a stream that simply stops has no way
            // to tell a finished run from a dropped connection.
            stream.write_all(b"0\r\n\r\n")?;
            stream.flush()?;
            result
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

/// A 256-bit token, hex encoded.
///
/// The bytes come from blake3 over the machine's clock, this process's id and
/// the address of a fresh heap allocation — which is already a dependency, is
/// not a predictable sequence, and needs no RNG crate. The token guards a
/// loopback socket for one process's lifetime; it is not a long-lived secret.
fn new_token() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let entropy = Box::new(0u8);
    let address = &*entropy as *const u8 as usize;
    let mut hasher = blake3::Hasher::new();
    hasher.update(&now.to_le_bytes());
    hasher.update(&std::process::id().to_le_bytes());
    hasher.update(&address.to_le_bytes());
    hasher.update(&std::time::Instant::now().elapsed().as_nanos().to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Host check is what stops a page on another origin reaching this
    /// server with the browser's cookies attached, so the names it accepts are
    /// worth stating exactly.
    #[test]
    fn only_loopback_hosts_are_answered() {
        assert!(loopback_host(Some("localhost")));
        assert!(loopback_host(Some("localhost:7365")));
        assert!(loopback_host(Some("127.0.0.1:7365")));
        assert!(loopback_host(Some("LOCALHOST:7365")));
        assert!(loopback_host(Some("[::1]:7365")));

        assert!(!loopback_host(Some("evil.example")));
        assert!(!loopback_host(Some("evil.example:7365")));
        // A name that merely ends in localhost is a different host.
        assert!(!loopback_host(Some("notlocalhost")));
        assert!(!loopback_host(Some("localhost.evil.example")));
        // 127.0.0.1 by another spelling is still a foreign Host header: the
        // check is on the name the browser sent, not on where it resolves.
        assert!(!loopback_host(Some("127.1")));
        assert!(!loopback_host(None));
    }

    fn write(headers: &[(&str, &str)]) -> Request {
        Request {
            method: "POST".into(),
            path: "/api/index".into(),
            query: BTreeMap::new(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
                .collect(),
            body: Vec::new(),
        }
    }

    /// The rule that replaces the cookie. A page on another port of the same
    /// host is `same-site`, which is why `same-site` has to be a refusal: it is
    /// exactly the case the cookie could not tell apart from this server.
    #[test]
    fn only_a_same_origin_write_is_accepted() {
        assert!(same_origin(&write(&[("Sec-Fetch-Site", "same-origin")]), 7365));
        assert!(!same_origin(&write(&[("Sec-Fetch-Site", "same-site")]), 7365));
        assert!(!same_origin(&write(&[("Sec-Fetch-Site", "cross-site")]), 7365));
        assert!(!same_origin(&write(&[("Sec-Fetch-Site", "none")]), 7365));

        // A browser sends both; the fetch metadata decides, so an Origin a page
        // could have lied about never rescues a cross-site write.
        assert!(!same_origin(
            &write(&[
                ("Sec-Fetch-Site", "same-site"),
                ("Origin", "http://127.0.0.1:7365"),
            ]),
            7365
        ));

        // A client that is not a browser sends neither, and is judged on the
        // token alone — `curl` has to keep working.
        assert!(same_origin(&write(&[]), 7365));
        assert!(same_origin(&write(&[("Origin", "http://localhost:7365")]), 7365));
        assert!(!same_origin(
            &write(&[("Origin", "http://127.0.0.1:9999")]),
            7365
        ));
        assert!(!same_origin(&write(&[("Origin", "null")]), 7365));
        assert!(!same_origin(
            &write(&[("Origin", "https://127.0.0.1:7365")]),
            7365
        ));
    }

    /// The three types a form can post with no preflight are the three this
    /// refuses, which is what makes a cross-origin form useless against a route.
    #[test]
    fn only_a_json_body_is_read() {
        assert!(json_body(Some("application/json")));
        assert!(json_body(Some("application/json; charset=utf-8")));
        assert!(json_body(Some("Application/JSON")));

        assert!(!json_body(Some("text/plain")));
        assert!(!json_body(Some("application/x-www-form-urlencoded")));
        assert!(!json_body(Some("multipart/form-data; boundary=x")));
        assert!(!json_body(Some("application/json-patch+json")));
        assert!(!json_body(None));
    }

    /// The page and its own bytes are public; anything that answers about this
    /// machine is not, however it is asked for.
    #[test]
    fn only_the_page_and_its_assets_are_public() {
        let get = |path: &str| Request {
            method: "GET".into(),
            path: path.into(),
            query: BTreeMap::new(),
            headers: BTreeMap::new(),
            body: Vec::new(),
        };
        assert!(public(&get("/"), true));
        assert!(public(&get("/app.js"), true));
        assert!(public(&get("/style.css"), true));

        assert!(!public(&get("/api/stores"), true));
        assert!(!public(&get("/api/image"), true));
        assert!(!public(&get(MCP_PATH), true));
        // A write is never public, whatever it names.
        assert!(!public(&get("/"), false));
    }

    #[test]
    fn tokens_compare_by_value_and_by_length() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
        assert!(!same("", "a"));
    }

    /// `?lang=rust&lang=go` is how the portal sends a repeated filter, and a
    /// map that kept only the last one would silently narrow the search.
    #[test]
    fn a_repeated_query_key_keeps_every_value() {
        let q = parse_query("lang=rust&lang=go&k=5");
        let request = Request {
            method: "GET".into(),
            path: "/".into(),
            query: q,
            headers: BTreeMap::new(),
            body: Vec::new(),
        };
        assert_eq!(request.query_all("lang"), vec!["rust", "go"]);
        assert_eq!(request.query("k"), Some("5"));
        assert!(request.query_all("ext").is_empty());
    }

    #[test]
    fn percent_and_plus_both_decode() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("%2Fsrc%2Flib.rs"), "/src/lib.rs");
        // A stray percent is left alone rather than eating the next bytes.
        assert_eq!(percent_decode("100%"), "100%");
        let q = parse_query("query=Fleet%3A%3Awritable+opens");
        assert_eq!(q.get("query").unwrap(), "Fleet::writable opens");
    }

    /// Two tokens from one process must differ, or a rotate would hand back
    /// the token it was asked to invalidate.
    #[test]
    fn every_token_is_new() {
        let a = new_token();
        let b = new_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 64, "256 bits, hex encoded");
    }
}
