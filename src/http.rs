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

/// How long a whole request has to arrive once its connection is accepted.
///
/// The read timeout above catches a socket that goes quiet; this catches one
/// that never does — a byte a second is not a stalled connection and would
/// hold a worker for as long as the sender felt like. Ten seconds is far more
/// than any request over loopback needs.
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);

/// How many connections may be in hand at once.
///
/// Eight workers, a waiting room that can be holding refusals for up to four
/// seconds, and streams that have left the pool: the cap is on all of them
/// together, because what it is protecting against is memory and file
/// descriptors rather than worker time. Past it the answer is 503 and the
/// connection closes, which is what a local client should see from a daemon
/// that is already at its limit.
const MAX_CONNECTIONS: usize = 32;

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

/// A refused connection and the moment it may be told so.
struct Late {
    stream: TcpStream,
    due: std::time::Instant,
    /// Held for as long as the refusal is, so a connection waiting in the
    /// waiting room still counts against [`MAX_CONNECTIONS`].
    _in_flight: InFlight,
}

/// One connection's place in the count, given up when it is dropped.
///
/// A guard rather than a pair of increments, because this connection can leave
/// by several doors — answered on a worker, held in the waiting room, or
/// streaming on a thread of its own — and a count that is only right on the
/// paths somebody remembered is a leak waiting for the one they did not.
struct InFlight(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The agent key, and the one it replaced.
///
/// `previous` is kept valid until this process exits so that a session in
/// flight when somebody rotated finishes rather than failing mid-answer. It is
/// not persisted: a restart is where the old key stops being accepted, which
/// is also the moment every client had to be told about the new one anyway.
#[derive(Default)]
struct Agent {
    current: String,
    /// The key this one replaced, and when it stops being accepted.
    previous: Option<(String, std::time::Instant)>,
}

/// How long a rotated agent key keeps working.
///
/// It used to be "until this process exits", which on a daemon somebody leaves
/// running is not a grace period but a second live credential — a key rotated
/// because it leaked stayed valid for as long as the machine was up. Fifteen
/// minutes is long enough for a tool call in flight and a client that has to be
/// restarted, and short enough to be a window rather than a state.
const KEY_GRACE: Duration = Duration::from_secs(15 * 60);

pub struct Server {
    listener: TcpListener,
    port: u16,
    /// Behind a lock so the Privacy page's Rotate button can replace it while
    /// the server is running.
    token: Arc<Mutex<String>>,
    agent: Arc<Mutex<Agent>>,
    /// Whether `/mcp` answers. Closing it drops the route, not the daemon.
    mcp_open: Arc<AtomicBool>,
    /// What a refused request costs right now.
    penalty: Arc<Mutex<Penalty>>,
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
            penalty: Arc::new(Mutex::new(Penalty::default())),
        })
    }

    /// Install the persisted agent key. Until this is called `/mcp` accepts
    /// the session token alone, because an empty key matches nothing.
    pub fn set_agent_key(&self, key: &str) {
        self.agent.lock().unwrap_or_else(|e| e.into_inner()).current = key.to_string();
    }

    pub fn agent_key(&self) -> String {
        self.agent
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .current
            .clone()
    }

    /// Replace the agent key.
    ///
    /// `now` drops the previous key immediately; otherwise it stays valid
    /// until this process exits, so a client mid-session finishes its work and
    /// only then needs the new stanza.
    pub fn rotate_agent(&self, key: &str, now: bool) {
        let mut agent = self.agent.lock().unwrap_or_else(|e| e.into_inner());
        let was = std::mem::replace(&mut agent.current, key.to_string());
        agent.previous = if now || was.is_empty() {
            None
        } else {
            Some((was, std::time::Instant::now() + KEY_GRACE))
        };
    }

    /// How long the previous key keeps working, if one still does.
    ///
    /// `None` once it has expired or was dropped immediately, which is what the
    /// Privacy page shows as a countdown rather than as "until this exits".
    pub fn key_grace(&self) -> Option<Duration> {
        let agent = self.agent.lock().unwrap_or_else(|e| e.into_inner());
        agent
            .previous
            .as_ref()
            .map(|(_, until)| until.saturating_duration_since(std::time::Instant::now()))
            .filter(|left| !left.is_zero())
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
        self.token.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Invalidate the current token and return the new one.
    pub fn rotate(&self) -> String {
        let fresh = new_token();
        *self.token.lock().unwrap_or_else(|e| e.into_inner()) = fresh.clone();
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
        let (tx, rx) = mpsc::channel::<(TcpStream, InFlight)>();
        let rx = Arc::new(Mutex::new(rx));
        let refused = Arc::new(refused);
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // Refused requests are answered by a thread of their own. Holding a
        // worker for the delay would mean eight wrong tokens at once could stop
        // the portal answering anything, which would make the throttle a denial
        // of service with extra steps.
        let (late_tx, late_rx) = mpsc::channel::<Late>();
        let waiting_room = std::thread::spawn(move || {
            while let Ok(late) = late_rx.recv() {
                let now = std::time::Instant::now();
                if late.due > now {
                    std::thread::sleep(late.due - now);
                }
                let mut stream = late.stream;
                let _ = write_response(&mut stream, Response::new(401, "text/plain", Vec::new()));
            }
        });

        let port = self.port;
        // One worker, built the same way whether it is one of the first eight
        // or the replacement for one that died. Every lock it takes is
        // recovered rather than expected: a thread that panicked while holding
        // one left the data behind it intact — `Response` and `Auth` are values,
        // not half-written state — and refusing to look at it afterwards turns
        // one panic into a dead server.
        let spawn_worker = {
            let rx = Arc::clone(&rx);
            let late_tx = late_tx.clone();
            let penalty = Arc::clone(&self.penalty);
            let handler = Arc::clone(&handler);
            let token = Arc::clone(&self.token);
            let agent = Arc::clone(&self.agent);
            let mcp_open = Arc::clone(&self.mcp_open);
            let refused = Arc::clone(&refused);
            move || {
                let rx = Arc::clone(&rx);
                let late_tx = late_tx.clone();
                let penalty = Arc::clone(&penalty);
                let handler = Arc::clone(&handler);
                let token = Arc::clone(&token);
                let agent = Arc::clone(&agent);
                let mcp_open = Arc::clone(&mcp_open);
                let refused = Arc::clone(&refused);
                std::thread::spawn(move || {
                    loop {
                        // The receiver is behind a mutex only for the recv; a
                        // worker that took a connection does not hold up the
                        // next.
                        let next = {
                            let guard = rx.lock().unwrap_or_else(|e| e.into_inner());
                            guard.recv()
                        };
                        let Ok((stream, in_flight)) = next else {
                            return;
                        };
                        let hold = |stream, in_flight| {
                            let delay = penalty.lock().unwrap_or_else(|e| e.into_inner()).charge();
                            let _ = late_tx.send(Late {
                                stream,
                                due: std::time::Instant::now() + delay,
                                _in_flight: in_flight,
                            });
                        };
                        let auth = {
                            let agent = agent.lock().unwrap_or_else(|e| e.into_inner());
                            Auth {
                                token: token.lock().unwrap_or_else(|e| e.into_inner()).clone(),
                                agent: agent.current.clone(),
                                previous: agent.previous.clone(),
                                mcp_open: mcp_open.load(Ordering::Relaxed),
                                port,
                            }
                        };
                        if let Some(class) = answer(stream, in_flight, &handler, &auth, &hold) {
                            refused(class);
                        }
                    }
                })
            }
        };

        let mut workers: Vec<_> = (0..WORKERS).map(|_| spawn_worker()).collect();

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
                Ok((mut stream, _)) => {
                    // The waker's own connection arrives here too; it carries
                    // no request, so `answer` reads nothing and closes it.
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
                    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

                    // Counted here rather than on a worker: a connection that
                    // is still queued is still a connection this process is
                    // holding open, and the flood that matters never reaches a
                    // worker at all.
                    let taken = live.fetch_add(1, Ordering::Relaxed);
                    let in_flight = InFlight(Arc::clone(&live));
                    if taken >= MAX_CONNECTIONS {
                        let _ = write_response(
                            &mut stream,
                            Response::new(503, "text/plain", Vec::new()),
                        );
                        drop(in_flight);
                        continue;
                    }
                    if tx.send((stream, in_flight)).is_err() {
                        break;
                    }
                    // A worker that ended for any reason is replaced here, so
                    // the pool is eight threads for as long as the daemon runs
                    // rather than eight minus however many requests have gone
                    // wrong since it started. Checked on accept because that is
                    // the only moment something is known to be happening.
                    for worker in &mut workers {
                        if worker.is_finished() {
                            *worker = spawn_worker();
                        }
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
        // `spawn_worker` captured a sender of its own so it could hand one to
        // each worker it makes, and it outlives the workers — so dropping the
        // one below is not enough on its own. Both have to go before the
        // waiting room's `recv` can end, and a shutdown that waits on a channel
        // nothing will ever close is a daemon that does not exit when it is
        // asked to.
        drop(spawn_worker);
        // Dropped after the workers have been joined, so the waiting room
        // answers whatever it still owes before it ends. A shutdown that closed
        // it first would look like a dropped connection to whoever was being
        // held.
        drop(late_tx);
        let _ = waiting_room.join();
        Ok(())
    }
}

/// The credentials in force for one request.
struct Auth {
    token: String,
    agent: String,
    previous: Option<(String, std::time::Instant)>,
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
        self.previous.as_ref().is_some_and(|(previous, until)| {
            !previous.is_empty() && *until > std::time::Instant::now() && same(bearer, previous)
        })
    }
}

/// Answer one connection. Returns why it was refused, if it was.
///
/// `hold` takes a connection whose credential was wrong and answers it later,
/// off this thread. Everything else is answered here and now: a malformed
/// request, a foreign `Host` and a cross-origin write are rule violations rather
/// than guesses at a secret, and delaying them would only slow down the person
/// who made a mistake.
fn answer(
    mut stream: TcpStream,
    in_flight: InFlight,
    handler: &Handler,
    auth: &Auth,
    hold: &impl Fn(TcpStream, InFlight),
) -> Option<Refusal> {
    let started = std::time::Instant::now();
    let mut reader = match stream.try_clone().map(BufReader::new) {
        Ok(reader) => reader,
        Err(_) => return Some(Refusal::Malformed),
    };

    // The head first, and only the head. Everything that can refuse this
    // request is decided from it, so a request that is going to be refused
    // never has its body read into memory — which is what keeps a thousand
    // cross-origin attempts from costing a thousand bodies' worth of it.
    let mut request = match read_head(&mut reader) {
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
        drain_body(&mut reader, &request, started);
        let _ = write_response(&mut stream, Response::new(400, "text/plain", Vec::new()));
        return Some(Refusal::ForeignHost);
    }

    let want = auth.token.as_str();
    let for_mcp = request.path == MCP_PATH;

    // A closed endpoint is not a route. Answered before the credential is
    // looked at, so a client that was told to stop learns the same thing
    // whether or not it still holds a key.
    if for_mcp && !auth.mcp_open {
        drain_body(&mut reader, &request, started);
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
            drain_body(&mut reader, &request, started);
            let _ = write_response(&mut stream, Response::new(403, "text/plain", Vec::new()));
            return Some(Refusal::CrossOrigin);
        }
        if !json_body(request.header("content-type")) {
            drain_body(&mut reader, &request, started);
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
            // Empty body, and not yet: a prober learns that something refused
            // it, nothing else, and not quickly.
            drain_body(&mut reader, &request, started);
            hold(stream, in_flight);
            return Some(Refusal::Unauthorized);
        }
    }

    // Allowed, so now the body — and now the clock matters, because from here
    // the connection is one this server intends to do work for.
    if read_body(&mut reader, &mut request).is_err() {
        let _ = write_response(&mut stream, Response::new(400, "text/plain", Vec::new()));
        return Some(Refusal::Malformed);
    }
    if started.elapsed() > REQUEST_DEADLINE {
        // A request dribbled out one byte at a time holds a worker for as long
        // as the sender likes. Ten seconds is far more than a loopback request
        // needs and far less than a client can hold.
        let _ = write_response(&mut stream, Response::new(408, "text/plain", Vec::new()));
        return Some(Refusal::Malformed);
    }

    let request = if head_only {
        Request {
            method: "GET".to_string(),
            ..request
        }
    } else {
        request
    };

    // A panicking route answers 500 and the daemon keeps serving. Before
    // 0.14.0 it took the worker with it permanently and poisoned whatever locks
    // it held, so one malformed request cost an eighth of the server until a
    // restart — and eight of them cost all of it.
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(&request)));
    let mut response = match caught {
        Ok(response) => response,
        Err(_) => {
            eprintln!(
                "semlith: {} {} panicked; answered 500 and kept serving",
                request.method, request.path
            );
            Response::new(500, "text/plain", Vec::new())
        }
    };
    if head_only {
        response.body = Body::Bytes(Vec::new());
    }

    // A streamed response leaves the pool. An index run holds its connection
    // for as long as the run takes — minutes on a large corpus — and doing that
    // on a worker means the portal is answering on seven threads while it runs,
    // or on none at all once somebody starts eight.
    if matches!(response.body, Body::Stream(_)) {
        std::thread::spawn(move || {
            let _in_flight = in_flight;
            let _ = write_response(&mut stream, response);
        });
        return None;
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
    let loopback = host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1";
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

/// The request line and the headers, with no body read yet.
///
/// Split from the body on purpose. Every rule that can refuse a request is
/// decided from what is here, so a refusal costs the head and nothing more.
fn read_head(reader: &mut BufReader<TcpStream>) -> Result<Option<Request>> {
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

    Ok(Some(Request {
        method,
        path: percent_decode(raw_path),
        query: parse_query(raw_query),
        headers,
        body: Vec::new(),
    }))
}

/// Read and throw away a declared body, so a refused peer can read its refusal.
///
/// A server that answers and closes while the client is still writing gives
/// that client a reset connection rather than the status it was sent: the
/// response is in the socket, and the reset takes it with it. So a refusal
/// drains what the client said it was sending — through a small buffer, never
/// into one the size of the body, which is the whole point of refusing before
/// the body is read. Bounded by the same cap a real body has and by the
/// request deadline, so draining cannot be the hold it prevents.
fn drain_body(reader: &mut BufReader<TcpStream>, request: &Request, started: std::time::Instant) {
    let Some(len) = request
        .headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
    else {
        return;
    };
    let mut left = len.min(MAX_BODY);
    let mut sink = [0u8; 8 * 1024];
    while left > 0 && started.elapsed() <= REQUEST_DEADLINE {
        let want = left.min(sink.len());
        match reader.read(&mut sink[..want]) {
            Ok(0) | Err(_) => return,
            Ok(n) => left -= n,
        }
    }
}

/// The body, once the request has earned one.
fn read_body(reader: &mut BufReader<TcpStream>, request: &mut Request) -> Result<()> {
    let Some(len) = request
        .headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
    else {
        return Ok(());
    };
    anyhow::ensure!(len <= MAX_BODY, "request body too large");
    request.body.resize(len, 0);
    reader.read_exact(&mut request.body)?;
    Ok(())
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
/// From the OS random source, the same construction `home::new_agent_key` uses.
/// It was a blake3 hash of the clock, this process's id and the address of a
/// fresh allocation until 0.14.0 — none of which is a secret. A clock is
/// readable by anything on the machine, a pid is in `/proc`, and an allocator
/// address under a defeated ASLR is a small space; a hash of three guessable
/// things is a guessable thing, however long its output is.
fn new_token() -> String {
    let mut bytes = [0u8; 32];
    let filled = getrandom::fill(&mut bytes).map(|()| bytes);
    token_from(filled)
}

/// The hex encoding, and the one place the random source failing is decided.
///
/// Split out so a test can hand it a failure: there is no way to make the OS
/// refuse, and a token minted from a fallback nobody noticed is exactly the
/// thing this function exists to prevent.
fn token_from(filled: Result<[u8; 32], getrandom::Error>) -> String {
    let bytes = filled
        .expect("the OS random source refused; semlith will not mint a session token without it");
    let mut out = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// What a refused request costs, and what a run of them costs.
///
/// A wrong token is answered late. The delay is what makes guessing a 256-bit
/// token over a loopback socket pointless rather than merely impractical, and
/// it grows with how many refusals this daemon has just answered, so a run of
/// guesses gets slower while one mistyped URL costs a quarter of a second.
///
/// The window lives here and resets with the daemon, which is the honest
/// lifetime: the token it guards resets then too.
#[derive(Default)]
struct Penalty {
    /// When each refusal in the last minute was answered.
    recent: std::collections::VecDeque<std::time::Instant>,
}

/// What one refusal costs before any escalation.
const REFUSAL_DELAY: Duration = Duration::from_millis(250);

/// The longest a refusal is ever held.
const MAX_REFUSAL_DELAY: Duration = Duration::from_secs(4);

/// How many refusals a minute may hold before each further block doubles it.
const REFUSALS_PER_STEP: usize = 20;

impl Penalty {
    /// Record a refusal and say how long to hold it.
    fn charge(&mut self) -> Duration {
        let now = std::time::Instant::now();
        while self
            .recent
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
        {
            self.recent.pop_front();
        }
        self.recent.push_back(now);
        // Every further twenty in the window doubles it: 250 ms, then 500, then
        // one second, two, and four.
        let steps = (self.recent.len().saturating_sub(1)) / REFUSALS_PER_STEP;
        let delay = REFUSAL_DELAY * 2u32.saturating_pow(steps.min(8) as u32);
        delay.min(MAX_REFUSAL_DELAY)
    }
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
        assert!(same_origin(
            &write(&[("Sec-Fetch-Site", "same-origin")]),
            7365
        ));
        assert!(!same_origin(
            &write(&[("Sec-Fetch-Site", "same-site")]),
            7365
        ));
        assert!(!same_origin(
            &write(&[("Sec-Fetch-Site", "cross-site")]),
            7365
        ));
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
        assert!(same_origin(
            &write(&[("Origin", "http://localhost:7365")]),
            7365
        ));
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
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "not lowercase hex: {a}"
        );
    }

    /// The OS random source is the only source. A token minted from a fallback
    /// nobody noticed is the finding this replaced, so the failure is a panic
    /// naming what refused rather than a weaker token.
    #[test]
    fn a_token_is_never_minted_without_the_os_random_source() {
        let refused = std::panic::catch_unwind(|| token_from(Err(getrandom::Error::UNSUPPORTED)))
            .expect_err("a failing random source minted a token anyway");
        let message = refused
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| refused.downcast_ref::<&str>().copied())
            .unwrap_or("");
        assert!(
            message.contains("the OS random source refused"),
            "the panic does not say what refused: {message}"
        );

        // The same bytes always encode the same way, so the hex half is not
        // where the entropy is.
        assert_eq!(token_from(Ok([0u8; 32])), "0".repeat(64));
        assert_eq!(&token_from(Ok([0xab; 32]))[..4], "abab");
    }

    /// A mistyped URL costs a quarter of a second; a run of guesses costs more
    /// each time, up to four seconds, and the window is a minute long.
    #[test]
    fn a_run_of_refusals_gets_slower() {
        let mut penalty = Penalty::default();
        assert_eq!(penalty.charge(), REFUSAL_DELAY, "the first refusal");
        for _ in 1..REFUSALS_PER_STEP {
            assert_eq!(penalty.charge(), REFUSAL_DELAY);
        }
        // The twenty-first in the window is where it starts doubling.
        assert_eq!(penalty.charge(), REFUSAL_DELAY * 2);
        for _ in 0..REFUSALS_PER_STEP {
            penalty.charge();
        }
        assert_eq!(penalty.charge(), REFUSAL_DELAY * 4);

        for _ in 0..200 {
            penalty.charge();
        }
        assert_eq!(
            penalty.charge(),
            MAX_REFUSAL_DELAY,
            "the delay is capped, because holding a connection forever is the \
             other kind of denial of service"
        );

        // A refusal a minute ago is not part of this run.
        let old = std::time::Instant::now() - Duration::from_secs(61);
        penalty.recent = std::iter::repeat_n(old, 500).collect();
        assert_eq!(penalty.charge(), REFUSAL_DELAY);
    }
}
