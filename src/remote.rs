//! The `remote` embedding lane's channel, both ends.
//!
//! `semlith worker --listen` runs on a confidential GPU machine. Each
//! connection is TLS 1.3 under a key the worker made when it started and never
//! wrote down. The client sends a token and a fresh nonce; the worker answers
//! with attestation evidence bound to `sha256(nonce || its certificate)`, so a
//! report can neither be replayed nor lifted onto another key. Only after the
//! client has checked that evidence (`crate::attest`) does a token id leave
//! the machine. From then on the connection carries exactly the frames a local
//! lane's `__embed-worker` speaks on its stdin and stdout, and the worker hands
//! them to one of its own: nothing new to embed, nothing written to disk.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// The name the worker's certificate carries. Nothing checks it: the key is
/// trusted because the attestation names it, not because of a name.
const SERVER_NAME: &str = "semlith-worker";

/// The environment variable holding the token a client must present.
pub const TOKEN_ENV: &str = "SEMLITH_WORKER_TOKEN";

/// How long a connection may take to say hello and to be attested.
const HELLO_DEADLINE: Duration = Duration::from_secs(120);

/// A frame is a batch of ids or of vectors; 64 MB is far past either, as on
/// the local lanes' pipes.
const FRAME_CAP: usize = 64 << 20;

/// The client's first frame.
#[derive(serde::Serialize, serde::Deserialize)]
struct Hello {
    v: u32,
    token: String,
    /// Hex, 32 random bytes.
    nonce: String,
}

/// What the worker says before any batch: its evidence, or why not.
#[derive(serde::Serialize, serde::Deserialize, Default, Debug, Clone)]
pub struct Evidence {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The worker's binary version.
    #[serde(default)]
    pub version: String,
    /// The lane the worker embeds on.
    #[serde(default)]
    pub lane: String,
    /// The CPU TEE's evidence (a Google Cloud Attestation token on a
    /// Confidential VM), empty when the worker was started without one.
    #[serde(default)]
    pub cpu: String,
    /// The GPU's evidence (NVIDIA's attestation result), likewise.
    #[serde(default)]
    pub gpu: String,
}

/// The binding both ends compute: what the evidence must carry as its nonce.
pub fn binding(nonce: &[u8], certificate: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(nonce);
    hash.update(certificate);
    hex(&hash.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

fn same(a: &str, b: &str) -> bool {
    // Compared as digests, so the time taken says nothing about the token.
    Sha256::digest(a.as_bytes()) == Sha256::digest(b.as_bytes())
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

// ------------------------------------------------------------------ the pump

/// Drives one TLS connection on a thread of its own: raw bytes from
/// `outgoing` are written in order, and whatever arrives is cut into frames
/// for `incoming`. One thread owns the connection, so a large answer coming
/// in never waits behind a large batch going out. The connection closes when
/// `outgoing` is dropped, and `incoming` ends with the reason when the peer
/// goes.
fn pump(
    mut conn: rustls::Connection,
    mut sock: TcpStream,
    outgoing: mpsc::Receiver<Vec<u8>>,
    incoming: mpsc::Sender<std::result::Result<Vec<u8>, String>>,
) {
    let fail = |incoming: &mpsc::Sender<_>, why: String| {
        let _ = incoming.send(Err(why));
    };
    if let Err(e) = sock.set_nonblocking(true) {
        return fail(&incoming, format!("the connection: {e}"));
    }
    // rustls holds at most 64 KiB of plaintext by default, and a batch of
    // vectors is larger; what is in flight is bounded by the lane's depth.
    conn.set_buffer_limit(None);
    let mut received: Vec<u8> = Vec::new();
    let mut closing = false;
    let mut chunk = vec![0u8; 256 << 10];
    loop {
        let mut moved = false;
        while !closing {
            match outgoing.try_recv() {
                Ok(bytes) => {
                    if let Err(e) = conn.writer().write_all(&bytes) {
                        return fail(&incoming, format!("the connection: {e}"));
                    }
                    moved = true;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    conn.send_close_notify();
                    closing = true;
                }
            }
        }
        while conn.wants_write() {
            match conn.write_tls(&mut sock) {
                Ok(0) => return fail(&incoming, "the connection closed".into()),
                Ok(_) => moved = true,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => return fail(&incoming, format!("the connection: {e}")),
            }
        }
        if closing && !conn.wants_write() {
            return;
        }
        match conn.read_tls(&mut sock) {
            Ok(0) => return fail(&incoming, "the peer closed the connection".into()),
            Ok(_) => {
                moved = true;
                if let Err(e) = conn.process_new_packets() {
                    // Tell the peer why before going.
                    let _ = conn.write_tls(&mut sock);
                    return fail(&incoming, format!("the TLS session: {e}"));
                }
                loop {
                    match conn.reader().read(&mut chunk) {
                        Ok(0) => return fail(&incoming, "the peer closed the connection".into()),
                        Ok(n) => received.extend_from_slice(&chunk[..n]),
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(e) => return fail(&incoming, format!("the connection: {e}")),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return fail(&incoming, format!("the connection: {e}")),
        }
        // Whole frames out of what has arrived.
        let mut at = 0;
        while received.len() - at >= 4 {
            let len =
                u32::from_le_bytes(received[at..at + 4].try_into().expect("four bytes")) as usize;
            if len > FRAME_CAP {
                return fail(&incoming, format!("a frame of {len} bytes"));
            }
            if received.len() - at - 4 < len {
                break;
            }
            let frame = received[at + 4..at + 4 + len].to_vec();
            at += 4 + len;
            if incoming.send(Ok(frame)).is_err() {
                closing = true;
                conn.send_close_notify();
            }
        }
        received.drain(..at);
        if !moved {
            // ponytail: a 1 ms nap when idle instead of poll(2); a batch is
            // tens of milliseconds of device time, so the nap is noise.
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// The write half handed to whoever sends frames: each write is queued for
/// the pump in order, so `accel::write_frame` works on it unchanged.
pub struct Sender(mpsc::Sender<Vec<u8>>);

impl Write for Sender {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .send(buf.to_vec())
            .map_err(|_| std::io::Error::other("the connection has closed"))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// Finish the handshake while the socket still blocks, bounded.
fn handshake(conn: &mut rustls::Connection, sock: &mut TcpStream) -> Result<()> {
    sock.set_read_timeout(Some(HELLO_DEADLINE))?;
    sock.set_write_timeout(Some(HELLO_DEADLINE))?;
    while conn.is_handshaking() {
        conn.complete_io(sock).context("the TLS handshake")?;
    }
    sock.set_read_timeout(None)?;
    sock.set_write_timeout(None)?;
    Ok(())
}

/// A started pump: the frames that arrive, and the half that sends.
pub struct Channel {
    pub send: Sender,
    pub frames: mpsc::Receiver<std::result::Result<Vec<u8>, String>>,
}

fn start_pump(conn: rustls::Connection, sock: TcpStream) -> Channel {
    let (out_tx, out_rx) = mpsc::channel();
    let (in_tx, in_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("semlith-remote-pump".into())
        .spawn(move || pump(conn, sock, out_rx, in_tx))
        .expect("a thread for the connection");
    Channel {
        send: Sender(out_tx),
        frames: in_rx,
    }
}

// ---------------------------------------------------------------- the client

/// Accepts whatever certificate the worker shows and remembers it. The
/// handshake's signatures are still checked, so the worker proved it holds
/// the certificate's key; whether that key is trusted is decided afterwards,
/// by the attestation that names it.
#[derive(Debug)]
struct Remember {
    seen: std::sync::Mutex<Option<Vec<u8>>>,
    algorithms: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl rustls::client::danger::ServerCertVerifier for Remember {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        *self.seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(end_entity.to_vec());
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 is not offered".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

/// A connection to a worker that has said hello: its evidence, the binding
/// that evidence must carry, and the channel, on which nothing has been sent
/// but the hello. The caller checks the evidence before sending a batch.
pub struct Connected {
    pub evidence: Evidence,
    pub binding: String,
    pub channel: Channel,
}

/// Connect to a worker at `endpoint` (`host:port`), present `token`, and
/// collect its evidence for a fresh nonce.
pub fn connect(endpoint: &str, token: &str) -> Result<Connected> {
    let provider = provider();
    let remember = Arc::new(Remember {
        seen: std::sync::Mutex::new(None),
        algorithms: provider.signature_verification_algorithms,
    });
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("TLS 1.3")?
        .dangerous()
        .with_custom_certificate_verifier(remember.clone())
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(SERVER_NAME).expect("a valid name");
    let mut conn = rustls::Connection::Client(
        rustls::ClientConnection::new(Arc::new(config), name).context("a TLS session")?,
    );
    let mut sock = connect_tcp(endpoint)?;
    handshake(&mut conn, &mut sock).with_context(|| format!("the worker at {endpoint}"))?;
    let certificate = remember
        .seen
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .context("the worker showed no certificate")?;
    let mut nonce = [0u8; 32];
    getrandom::fill(&mut nonce).map_err(|e| anyhow::anyhow!("a random nonce: {e}"))?;
    let binding = binding(&nonce, &certificate);
    let mut channel = start_pump(conn, sock);
    let hello = serde_json::to_vec(&Hello {
        v: 1,
        token: token.to_string(),
        nonce: hex(&nonce),
    })?;
    channel.send.write_all(&frame(&hello))?;
    let answer = match channel.frames.recv_timeout(HELLO_DEADLINE) {
        Ok(Ok(frame)) => frame,
        Ok(Err(e)) => bail!("the worker at {endpoint} closed before answering: {e}"),
        Err(_) => bail!(
            "the worker at {endpoint} did not answer within {} s",
            HELLO_DEADLINE.as_secs()
        ),
    };
    let evidence: Evidence =
        serde_json::from_slice(&answer).context("the worker's answer to the hello")?;
    if !evidence.ok {
        bail!(
            "the worker at {endpoint} refused: {}",
            evidence.reason.as_deref().unwrap_or("no reason given")
        );
    }
    Ok(Connected {
        evidence,
        binding,
        channel,
    })
}

fn connect_tcp(endpoint: &str) -> Result<TcpStream> {
    use std::net::ToSocketAddrs;
    let mut last = None;
    for addr in endpoint
        .to_socket_addrs()
        .with_context(|| format!("resolving {endpoint}"))?
    {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(15)) {
            Ok(sock) => {
                sock.set_nodelay(true)?;
                return Ok(sock);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(anyhow::anyhow!(
        "connecting to {endpoint}: {}",
        last.map_or("no address".to_string(), |e| e.to_string())
    ))
}

// ---------------------------------------------------------------- the worker

/// How the worker is run.
pub struct Serve {
    pub listen: String,
    pub lane: String,
    /// Shell commands printing the CPU and GPU evidence for a binding, which
    /// replaces `{binding}` in them.
    pub attest_cpu: Option<String>,
    pub attest_gpu: Option<String>,
    pub max_connections: usize,
}

/// `semlith worker`: serve until killed.
pub fn serve(opts: Serve) -> Result<()> {
    let token = std::env::var(TOKEN_ENV)
        .ok()
        .filter(|t| t.len() >= 16)
        .with_context(|| format!("{TOKEN_ENV} must hold a token of at least 16 characters"))?;
    // Made now, held in memory, gone when the process goes.
    let made = rcgen::generate_simple_self_signed(vec![SERVER_NAME.to_string()])
        .context("making the worker's key")?;
    let certificate = made.cert.der().to_vec();
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(made.signing_key.serialize_der().into());
    let config = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .context("TLS 1.3")?
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(certificate.clone())],
            key,
        )
        .context("the worker's certificate")?;
    let config = Arc::new(config);
    let listener =
        TcpListener::bind(&opts.listen).with_context(|| format!("listening on {}", opts.listen))?;
    eprintln!(
        "semlith worker {} on {} (lane {}), key {}",
        env!("CARGO_PKG_VERSION"),
        listener.local_addr()?,
        opts.lane,
        &hex(&Sha256::digest(&certificate))[..16]
    );
    let opts = Arc::new(opts);
    let token = Arc::new(token);
    let certificate = Arc::new(certificate);
    let open = Arc::new(AtomicUsize::new(0));
    for sock in listener.incoming() {
        let Ok(sock) = sock else { continue };
        let peer = sock.peer_addr().map_or("unknown".into(), |a| a.to_string());
        if open.load(Ordering::SeqCst) >= opts.max_connections {
            eprintln!(
                "semlith worker: {peer} refused, {} connections open",
                opts.max_connections
            );
            continue;
        }
        open.fetch_add(1, Ordering::SeqCst);
        let (config, opts, token, certificate, open) = (
            config.clone(),
            opts.clone(),
            token.clone(),
            certificate.clone(),
            open.clone(),
        );
        std::thread::spawn(move || {
            let started = Instant::now();
            match connection(sock, config, &opts, &token, &certificate) {
                Ok((batches, rows)) => eprintln!(
                    "semlith worker: {peer} done after {:.0} s, {batches} batches, {rows} rows",
                    started.elapsed().as_secs_f64()
                ),
                Err(e) => eprintln!("semlith worker: {peer}: {e:#}"),
            }
            open.fetch_sub(1, Ordering::SeqCst);
        });
    }
    Ok(())
}

/// One client, from handshake to close: the counts of what it sent.
fn connection(
    mut sock: TcpStream,
    config: Arc<rustls::ServerConfig>,
    opts: &Serve,
    token: &str,
    certificate: &[u8],
) -> Result<(u64, u64)> {
    sock.set_nodelay(true)?;
    let mut conn =
        rustls::Connection::Server(rustls::ServerConnection::new(config).context("a TLS session")?);
    handshake(&mut conn, &mut sock)?;
    let mut channel = start_pump(conn, sock);
    let hello = match channel.frames.recv_timeout(HELLO_DEADLINE) {
        Ok(Ok(frame)) => frame,
        Ok(Err(e)) => bail!("closed before the hello: {e}"),
        Err(_) => bail!("no hello within {} s", HELLO_DEADLINE.as_secs()),
    };
    let refuse = |channel: &mut Channel, why: &str| -> Result<(u64, u64)> {
        let answer = Evidence {
            ok: false,
            reason: Some(why.to_string()),
            version: env!("CARGO_PKG_VERSION").into(),
            ..Default::default()
        };
        channel
            .send
            .write_all(&frame(&serde_json::to_vec(&answer)?))?;
        bail!("refused: {why}")
    };
    let Ok(hello) = serde_json::from_slice::<Hello>(&hello) else {
        return refuse(&mut channel, "the hello could not be read");
    };
    if hello.v != 1 {
        return refuse(&mut channel, "this worker speaks version 1 of the hello");
    }
    if !same(&hello.token, token) {
        return refuse(&mut channel, "the token was refused");
    }
    let Some(nonce) = unhex(&hello.nonce).filter(|n| n.len() == 32) else {
        return refuse(&mut channel, "the nonce must be 32 bytes in hex");
    };
    let bound = binding(&nonce, certificate);
    let run = |command: &Option<String>| -> Result<String> {
        let Some(command) = command else {
            return Ok(String::new());
        };
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(command.replace("{binding}", &bound))
            .stderr(std::process::Stdio::inherit())
            .output()
            .context("running the attestation command")?;
        if !out.status.success() {
            bail!("the attestation command exited with {}", out.status);
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let started = Instant::now();
    let (cpu, gpu) = match (run(&opts.attest_cpu), run(&opts.attest_gpu)) {
        (Ok(cpu), Ok(gpu)) => (cpu, gpu),
        (Err(e), _) | (_, Err(e)) => return refuse(&mut channel, &format!("{e:#}")),
    };
    eprintln!(
        "semlith worker: evidence in {} ms",
        started.elapsed().as_millis()
    );
    let answer = Evidence {
        ok: true,
        reason: None,
        version: env!("CARGO_PKG_VERSION").into(),
        lane: opts.lane.clone(),
        cpu,
        gpu,
    };
    channel
        .send
        .write_all(&frame(&serde_json::to_vec(&answer)?))?;

    // The lane's own worker, exactly as a local run starts it.
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .args(["__embed-worker", &opts.lane])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("starting the lane's worker")?;
    let mut stdin = child
        .stdin
        .take()
        .context("the lane's worker has no stdin")?;
    let mut stdout = child
        .stdout
        .take()
        .context("the lane's worker has no stdout")?;
    let Channel { mut send, frames } = channel;
    let back = std::thread::spawn(move || {
        while let Ok(body) = crate::accel::read_frame(&mut stdout) {
            if send.write_all(&frame(&body)).is_err() {
                return;
            }
        }
        // Dropping `send` closes the connection.
    });
    let (mut batches, mut rows) = (0u64, 0u64);
    while let Ok(Ok(body)) = frames.recv() {
        batches += 1;
        rows += body.get(..4).map_or(0, |b| {
            u64::from(u32::from_le_bytes(b.try_into().expect("four bytes")))
        });
        if crate::accel::write_frame(&mut stdin, &body).is_err() {
            break;
        }
    }
    drop(stdin);
    let _ = back.join();
    let _ = child.kill();
    let _ = child.wait();
    Ok((batches, rows))
}

// ------------------------------------------------------------ the lane's setup

/// The remote lane's settings, as `settings.json` holds them.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    /// The worker, as host:port.
    pub endpoint: Option<String>,
    /// The token the worker was started with.
    pub token: Option<String>,
    /// The attestation policy file (see `crate::attest::Policy`).
    pub policy: Option<std::path::PathBuf>,
}

/// Each setting from the environment, ahead of the saved one.
pub const ENDPOINT_ENV: &str = "SEMLITH_REMOTE_ENDPOINT";
pub const CLIENT_TOKEN_ENV: &str = "SEMLITH_REMOTE_TOKEN";
pub const POLICY_ENV: &str = "SEMLITH_REMOTE_POLICY";

/// What the lane needs, all present.
pub struct Config {
    pub endpoint: String,
    pub token: String,
    pub policy: std::path::PathBuf,
}

fn settings() -> Settings {
    let saved = crate::home::Settings::load().remote;
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    Settings {
        endpoint: env(ENDPOINT_ENV).or(saved.endpoint),
        token: env(CLIENT_TOKEN_ENV).or(saved.token),
        policy: env(POLICY_ENV).map(Into::into).or(saved.policy),
    }
}

pub fn config() -> Option<Config> {
    let s = settings();
    Some(Config {
        endpoint: s.endpoint?,
        token: s.token?,
        policy: s.policy?,
    })
}

/// Why the lane cannot start, when a setting is missing.
pub fn missing() -> Option<String> {
    let s = settings();
    let absent: Vec<&str> = [
        ("an endpoint", s.endpoint.is_none()),
        ("a token", s.token.is_none()),
        ("an attestation policy", s.policy.is_none()),
    ]
    .into_iter()
    .filter_map(|(what, gone)| gone.then_some(what))
    .collect();
    (!absent.is_empty()).then(|| {
        format!(
            "the remote lane has no {}: semlith accel on remote --endpoint host:port --token-file FILE --policy FILE",
            absent.join(", no ")
        )
    })
}

/// The last attestation the lane checked, for its row: when, and what it
/// found or why it refused.
static LAST: std::sync::Mutex<Option<serde_json::Value>> = std::sync::Mutex::new(None);

pub fn attestation() -> Option<serde_json::Value> {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Open the lane: connect, check the worker's attestation, and hand back the
/// channel only if it passed. Called on every start, so a reconnect is
/// attested afresh.
pub fn open() -> Result<(Channel, String)> {
    let config = config().with_context(|| missing().unwrap_or_default())?;
    let policy = crate::attest::Policy::load(&config.policy)?;
    let connected = connect(&config.endpoint, &config.token)?;
    let evidence = &connected.evidence;
    let at = now();
    match crate::attest::verify(
        &policy,
        &evidence.cpu,
        &evidence.gpu,
        &connected.binding,
        at,
    ) {
        Ok(verified) => {
            *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(serde_json::json!({
                "state": verified.state,
                "summary": verified.summary,
                "at": at,
                "worker": evidence.version,
                "lane": evidence.lane,
            }));
            Ok((connected.channel, evidence.lane.clone()))
        }
        Err(e) => {
            let reason = format!("{e:#}");
            *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(serde_json::json!({
                "state": "refused",
                "reason": reason,
                "at": at,
            }));
            // Dropping the channel closes the connection; nothing was sent
            // but the hello.
            bail!(
                "the worker at {} failed its attestation, so nothing was sent: {reason}",
                config.endpoint
            )
        }
    }
}

/// Save what `semlith accel on remote` was given; anything not given is kept.
pub fn save(
    endpoint: Option<String>,
    token_file: Option<&std::path::Path>,
    policy: Option<std::path::PathBuf>,
) -> Result<()> {
    let mut settings = crate::home::Settings::load();
    if let Some(endpoint) = endpoint {
        settings.remote.endpoint = Some(endpoint);
    }
    if let Some(path) = token_file {
        let token = std::fs::read_to_string(path)
            .with_context(|| format!("reading the token file {}", path.display()))?;
        settings.remote.token = Some(token.trim().to_string());
    }
    if let Some(path) = policy {
        // Checked now, so a typo is refused here rather than at the first run.
        crate::attest::Policy::load(&path)?;
        settings.remote.policy = Some(std::path::absolute(&path)?);
    }
    settings.save()
}
