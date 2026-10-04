//! Semlith Cloud, from this side of the wire.
//!
//! Everything here is built against `infra/docs/cloud-api.md` and is reached
//! only after `semlith cloud login`: with no `~/.semlith/cloud.json` nothing
//! in this module opens a socket, starts a thread or reads anything but the
//! file's absence. The rules that keep that true:
//!
//! - **A token goes to its own host and nowhere else.** A request is built from
//!   a credential [`Entry`] and a *path*; the URL is that entry's host plus the
//!   path, so no caller can hand a token to a URL a response named. An MCP URL
//!   the host reports is used only when it is on the same origin.
//! - **No redirect is followed.** A redirect is the one way a request could be
//!   carried somewhere its caller did not name, Authorization header and all.
//! - **A token is never printed.** [`Entry`] has no `Debug` that shows it,
//!   errors carry the host's sentence and never the request, and the portal is
//!   given the 13-character prefix the cloud itself shows.
//! - **Airgap refuses every call**, before a connection is opened, as it does
//!   for `add`, `upgrade` and the price table.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where `semlith cloud login` signs in when `--host` names nowhere else.
pub const DEFAULT_HOST: &str = "https://cloud.semlith.com";

/// Every org token starts with this.
pub const TOKEN_PREFIX: &str = "sml_live_";

/// How much of a token the cloud and this binary show: `sml_live_7f3c`.
pub const SHOWN_CHARS: usize = 13;

/// The most a reply may be. Reports are the largest thing the host sends.
const MAX_REPLY: u64 = 64 << 20;

/// How long an ordinary call may take, end to end.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// `~/.semlith/cloud.json`: the machine id and one entry per host and org.
pub fn credentials_path() -> Result<PathBuf> {
    Ok(crate::home::home_or_error()?.join("cloud.json"))
}

/// Whether this machine has ever signed in. One `stat`, and the whole of what
/// a binary nobody signed in with does about the cloud.
pub fn signed_in() -> bool {
    credentials_path().is_ok_and(|p| p.exists())
}

/// What `cloud.json` holds.
#[derive(Default, Serialize, Deserialize)]
pub struct Credentials {
    /// A random id made at the first sign-in. Only its hash leaves the
    /// machine, as the ledger's `machine`.
    #[serde(default)]
    pub machine: String,
    #[serde(default)]
    pub entries: Vec<Entry>,
}

/// One org token, bound to the host that minted it.
#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    /// `https://cloud.semlith.com`, no trailing slash.
    pub host: String,
    pub org: String,
    pub token: String,
    /// The plan the host last said, for the portal's header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// Unix seconds this entry was saved.
    #[serde(default)]
    pub added: u64,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("host", &self.host)
            .field("org", &self.org)
            .field("token", &self.prefix())
            .finish()
    }
}

impl Entry {
    /// The part of the token anything may show.
    pub fn prefix(&self) -> String {
        self.token.chars().take(SHOWN_CHARS).collect()
    }

    /// The host without its scheme, for a sentence.
    pub fn host_name(&self) -> &str {
        host_name(&self.host)
    }
}

/// `cloud.semlith.com` out of `https://cloud.semlith.com`.
pub fn host_name(host: &str) -> &str {
    host.split_once("://").map_or(host, |(_, rest)| rest)
}

impl Credentials {
    /// The file, or `None` when this machine never signed in.
    pub fn load() -> Result<Option<Self>> {
        let path = credentials_path()?;
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        // A token file anyone else can read is narrowed on sight, as the
        // registry and the store directories are.
        crate::home::tighten_file(&path);
        serde_json::from_str(&text).map(Some).with_context(|| {
            format!(
                "{} is not readable; move it aside and sign in again",
                path.display()
            )
        })
    }

    /// Write it owner-only, through a temporary file and a rename so a
    /// process killed mid-write leaves the previous file whole.
    ///
    /// On Windows there is no mode: the home sits in the user's profile, whose
    /// default ACL grants that user alone, which is the same answer the agent
    /// key relies on (`home::check_key_mode`).
    pub fn save(&self) -> Result<()> {
        let path = credentials_path()?;
        let dir = path.parent().unwrap_or(Path::new("."));
        crate::home::secure_dir(dir)?;
        let temp = path.with_extension(crate::home::unique_temp_suffix());
        let body = serde_json::to_string_pretty(self)? + "\n";
        crate::home::write_private(&temp, body.as_bytes())
            .with_context(|| format!("writing {}", temp.display()))?;
        if let Err(e) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            return Err(e).with_context(|| format!("writing {}", path.display()));
        }
        Ok(())
    }

    /// Replace the entry for this host and org, or add it.
    pub fn put(&mut self, entry: Entry) {
        self.entries
            .retain(|e| !(e.host == entry.host && e.org == entry.org));
        self.entries.push(entry);
        self.entries
            .sort_by(|a, b| (&a.org, &a.host).cmp(&(&b.org, &b.host)));
        if self.machine.is_empty() {
            self.machine = random_hex(16);
        }
    }

    /// The entry a command means: the one for `org` (on `host`, when named),
    /// or the only one when neither narrows it.
    pub fn find(&self, org: Option<&str>, host: Option<&str>) -> Result<&Entry> {
        let host = host.map(normal_host).transpose()?;
        let matching: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|e| org.is_none_or(|o| e.org == o))
            .filter(|e| host.as_ref().is_none_or(|h| &e.host == h))
            .collect();
        match matching.as_slice() {
            [one] => Ok(one),
            [] => match org {
                Some(org) => bail!(
                    "this machine is not signed in to {org}. Run `semlith cloud login {org}`."
                ),
                None => bail!(
                    "this machine is not signed in to Semlith Cloud. Run `semlith cloud login <org>`."
                ),
            },
            many => bail!(
                "this machine is signed in to {}; name one: {}",
                many.len(),
                many.iter()
                    .map(|e| format!("{} at {}", e.org, e.host_name()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// The hash of the machine id the ledger names this machine by.
    pub fn machine_id(&self) -> String {
        let hash = blake3::hash(self.machine.as_bytes()).to_hex();
        format!("m_{}", &hash[..16])
    }
}

/// The entry for `org`, loaded fresh.
pub fn entry_for(org: Option<&str>, host: Option<&str>) -> Result<Entry> {
    let Some(creds) = Credentials::load()? else {
        bail!("this machine is not signed in to Semlith Cloud. Run `semlith cloud login <org>`.");
    };
    creds.find(org, host).cloned()
}

fn random_hex(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    getrandom::fill(&mut raw).expect("the OS random source");
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A host as entries keep it: scheme, name and port, no trailing slash.
///
/// https only, except loopback, which is what a compose stack on this machine
/// and the tests' stub host are: a token sent in the clear across a network is
/// a token anyone on the path can read.
pub fn normal_host(raw: &str) -> Result<String> {
    let host = raw.trim().trim_end_matches('/');
    let Some((scheme, rest)) = host.split_once("://") else {
        bail!("{raw} is not a URL; a host looks like https://cloud.semlith.com");
    };
    if rest.is_empty() || rest.contains(['/', '?', '#', '@']) {
        bail!("{raw} is not a host; give it without a path, like https://cloud.semlith.com");
    }
    match scheme {
        "https" => {}
        "http" if loopback(rest) => {}
        _ => bail!("{raw} is not https; semlith sends a token to an https host only"),
    }
    Ok(format!("{scheme}://{rest}"))
}

fn loopback(authority: &str) -> bool {
    let name = authority
        .rsplit_once(':')
        .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
        .map_or(authority, |(name, _)| name);
    matches!(name, "localhost" | "127.0.0.1" | "[::1]")
}

/// `semlith/<version> (<os>; <arch>)`.
pub fn user_agent() -> String {
    format!(
        "semlith/{} ({}; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Why a call did not get the answer it asked for.
#[derive(Debug, Clone)]
pub enum Failure {
    /// Airgap is on; no connection was opened.
    Offline,
    /// The host could not be reached.
    Unreachable { host: String, why: String },
    /// The host answered with its error shape.
    Refused {
        status: u16,
        code: String,
        message: String,
        plan: Option<String>,
        min_version: Option<String>,
    },
}

impl Failure {
    /// `unreachable` or `refused`, as the portal's pill reads.
    pub fn kind(&self) -> &'static str {
        match self {
            Failure::Refused { .. } => "refused",
            _ => "unreachable",
        }
    }

    pub fn code(&self) -> Option<&str> {
        match self {
            Failure::Refused { code, .. } => Some(code),
            _ => None,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            Failure::Refused { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Offline => write!(
                f,
                "airgap is on (SEMLITH_AIRGAP, --airgap or the Privacy page), so semlith will not reach Semlith Cloud"
            ),
            Failure::Unreachable { host, why } => {
                write!(f, "{} could not be reached: {why}", host_name(host))
            }
            Failure::Refused {
                status,
                message,
                plan,
                min_version,
                ..
            } => {
                // The host's sentence as it is: `message` is written to be
                // printed. The two codes with a next step get it named.
                write!(f, "{}", message.trim())?;
                if *status == 426 {
                    match min_version {
                        Some(v) => write!(
                            f,
                            " This semlith is {}; the cloud needs {v} or newer. Run `semlith upgrade`.",
                            env!("CARGO_PKG_VERSION")
                        )?,
                        None => write!(f, " Run `semlith upgrade`.")?,
                    }
                }
                if *status == 402
                    && let Some(plan) = plan
                {
                    write!(f, " It needs the {} plan.", title(plan))?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for Failure {}

fn title(plan: &str) -> String {
    let mut chars = plan.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A reply the host gave with a success status.
pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
    /// `Semlith-Cloud-Version`, which `status` shows.
    pub version: Option<String>,
}

impl Reply {
    pub fn json(&self) -> Result<Value, Failure> {
        serde_json::from_slice(&self.body).map_err(|e| Failure::Refused {
            status: self.status,
            code: "bad_reply".into(),
            message: format!("The cloud answered with something that is not JSON ({e})."),
            plan: None,
            min_version: None,
        })
    }
}

/// What to send, beyond the method and path.
#[derive(Default)]
pub struct Send<'a> {
    /// A JSON body.
    pub json: Option<&'a Value>,
    /// Raw bytes with their content type and encoding (the push upload).
    pub bytes: Option<(&'a str, Option<&'a str>, Vec<u8>)>,
    /// Extra headers: the forwarded client and session.
    pub headers: Vec<(&'static str, String)>,
    pub timeout: Option<Duration>,
}

/// One call to `host` + `path`, with `token` as the bearer when there is one.
///
/// The only function here that opens a connection. `host` must already be a
/// [`normal_host`]; `path` starts with `/` and may carry a query string.
pub fn call(
    host: &str,
    token: Option<&str>,
    method: &str,
    path: &str,
    send: Send<'_>,
) -> Result<Reply, Failure> {
    if crate::embed::airgap() {
        return Err(Failure::Offline);
    }
    let url = format!("{host}{path}");
    crate::add::note_outbound("cloud", &url);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(send.timeout.unwrap_or(CALL_TIMEOUT)))
        .user_agent(user_agent())
        .build()
        .new_agent();
    let unreachable = |e: ureq::Error| Failure::Unreachable {
        host: host.to_string(),
        why: e.to_string(),
    };
    let auth = token.map(|t| format!("Bearer {t}"));
    macro_rules! headed {
        ($req:expr) => {{
            let mut req = $req;
            if !send
                .headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("accept"))
            {
                req = req.header("Accept", "application/json");
            }
            if let Some(auth) = &auth {
                req = req.header("Authorization", auth);
            }
            for (k, v) in &send.headers {
                req = req.header(*k, v);
            }
            req
        }};
    }
    let response = match method {
        "GET" => headed!(agent.get(&url)).call(),
        "DELETE" => headed!(agent.delete(&url)).call(),
        _ => {
            let builder = match method {
                "PUT" => agent.put(&url),
                _ => agent.post(&url),
            };
            let req = headed!(builder);
            if let Some(body) = send.json {
                req.header("Content-Type", "application/json")
                    .send(serde_json::to_vec(body).unwrap_or_default())
            } else if let Some((kind, encoding, bytes)) = send.bytes {
                let mut req = req.header("Content-Type", kind);
                if let Some(encoding) = encoding {
                    req = req.header("Content-Encoding", encoding);
                }
                req.send(bytes)
            } else {
                req.send_empty()
            }
        }
    };
    let mut response = response.map_err(unreachable)?;
    let status = response.status().as_u16();
    let version = response
        .headers()
        .get("semlith-cloud-version")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let mut body = Vec::new();
    use std::io::Read;
    response
        .body_mut()
        .as_reader()
        .take(MAX_REPLY)
        .read_to_end(&mut body)
        .map_err(|e| Failure::Unreachable {
            host: host.to_string(),
            why: format!("the reply broke off: {e}"),
        })?;
    if status >= 400 {
        return Err(refusal(status, &body));
    }
    Ok(Reply {
        status,
        body,
        version,
    })
}

/// The host's one error shape, or a sentence for a reply that is not it.
fn refusal(status: u16, body: &[u8]) -> Failure {
    let parsed: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let error = &parsed["error"];
    let text = |k: &str| error.get(k).and_then(Value::as_str).map(str::to_string);
    Failure::Refused {
        status,
        code: text("code").unwrap_or_else(|| format!("http_{status}")),
        message: text("message").unwrap_or_else(|| match status {
            401 => "The cloud did not accept this machine's token. Sign in again with `semlith cloud login`.".into(),
            404 => "The cloud has no such org or store, or this token cannot reach it.".into(),
            _ => format!("The cloud answered {status}."),
        }),
        plan: text("plan"),
        min_version: text("min_version"),
    }
}

/// An authenticated call for `entry`: its host, its token.
pub fn call_as(entry: &Entry, method: &str, path: &str, send: Send<'_>) -> Result<Reply, Failure> {
    call(&entry.host, Some(&entry.token), method, path, send)
}

/// The org's path segment, escaped. Slugs are lowercase words already; this
/// is so a hand-typed one cannot add a path segment of its own.
pub fn seg(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// This machine's name, for the approval page. `hostname` is on every OS
/// semlith ships for; a machine without it is "this machine".
fn machine_name() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "this machine".to_string())
}

/// Open `url` in the browser, when a person is at a terminal to see it.
fn open_browser(url: &str) {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return;
    }
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    let _ = command
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// `semlith cloud login`: the device authorization grant, or `--token`
/// checked with `/v1/whoami`. Saves the entry and returns it.
pub fn login(
    org: Option<&str>,
    host: Option<&str>,
    token: Option<&str>,
    say: &mut dyn FnMut(&str),
) -> Result<Entry> {
    let host = normal_host(host.unwrap_or(DEFAULT_HOST))?;
    let (org, token, plan) = match token {
        Some(token) => {
            let token = token.trim();
            if !token.starts_with(TOKEN_PREFIX) {
                bail!(
                    "that is not a Semlith Cloud org token; one starts with {TOKEN_PREFIX} and is made on the Agents page"
                );
            }
            let who = call(&host, Some(token), "GET", "/v1/whoami", Send::default())?.json()?;
            let slug = who["org"]["slug"].as_str().unwrap_or_default().to_string();
            if slug.is_empty() {
                bail!(
                    "{} did not say which org this token belongs to",
                    host_name(&host)
                );
            }
            if let Some(org) = org
                && org != slug
            {
                bail!("that token belongs to {slug}, not {org}; nothing was saved");
            }
            let plan = who["org"]["plan"].as_str().map(str::to_string);
            (slug, token.to_string(), plan)
        }
        None => {
            let (slug, token) = device_flow(&host, org, say)?;
            (slug, token, None)
        }
    };
    let entry = Entry {
        host,
        org,
        token,
        plan,
        added: unix_now(),
    };
    let mut creds = Credentials::load()?.unwrap_or_default();
    creds.put(entry.clone());
    creds.save()?;
    Ok(entry)
}

/// RFC 8628's shape: ask for a code, show it, poll until the person acts.
fn device_flow(
    host: &str,
    org: Option<&str>,
    say: &mut dyn FnMut(&str),
) -> Result<(String, String)> {
    let asked = call(
        host,
        None,
        "POST",
        "/v1/cli/authorize",
        Send {
            json: Some(&json!({
                "client": format!("semlith/{}", env!("CARGO_PKG_VERSION")),
                "host_name": machine_name(),
            })),
            ..Send::default()
        },
    )?
    .json()?;
    let device = asked["device_code"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let code = asked["user_code"].as_str().unwrap_or_default();
    let page = asked["verification_uri"].as_str().unwrap_or_default();
    let complete = asked["verification_uri_complete"].as_str().unwrap_or(page);
    if device.is_empty() || code.is_empty() {
        bail!("{} did not hand out a device code", host_name(host));
    }
    let mut interval = asked["interval"].as_u64().unwrap_or(5);
    let expires = asked["expires_in"].as_u64().unwrap_or(600);
    say(&format!(
        "Open {page} and enter the code {code}{}.",
        org.map(|o| format!(", then approve {o}"))
            .unwrap_or_default()
    ));
    open_browser(complete);
    let deadline = std::time::Instant::now() + Duration::from_secs(expires);
    loop {
        std::thread::sleep(Duration::from_secs(interval));
        if std::time::Instant::now() > deadline {
            bail!(
                "the code {code} expired before it was approved; run `semlith cloud login` again"
            );
        }
        let polled = call(
            host,
            None,
            "POST",
            "/v1/cli/token",
            Send {
                json: Some(&json!({ "device_code": device })),
                ..Send::default()
            },
        );
        match polled {
            Ok(reply) => {
                let got = reply.json()?;
                let token = got["token"].as_str().unwrap_or_default().to_string();
                let slug = got["org"].as_str().unwrap_or_default().to_string();
                if !token.starts_with(TOKEN_PREFIX) || slug.is_empty() {
                    bail!("{} approved the code but sent no token", host_name(host));
                }
                // Bound to the host this flow ran against, whatever `host`
                // the reply names: that is the host that minted it.
                if let Some(org) = org
                    && org != slug
                {
                    say(&format!(
                        "Approved for {slug}, not {org}; saved for {slug}."
                    ));
                }
                return Ok((slug, token));
            }
            Err(Failure::Refused { code, .. }) if code == "authorization_pending" => {}
            Err(Failure::Refused { code, .. }) if code == "slow_down" => interval += 5,
            Err(Failure::Refused { code, .. }) if code == "access_denied" => {
                bail!("the sign-in was denied in the browser; nothing was saved")
            }
            Err(Failure::Refused { status: 410, .. }) => {
                bail!(
                    "the code {code} expired before it was approved; run `semlith cloud login` again"
                )
            }
            Err(other) => return Err(other.into()),
        }
    }
}

/// `semlith cloud logout`: forget a token here. The token still exists on
/// the host until somebody revokes it there.
pub fn logout(org: Option<&str>, host: Option<&str>) -> Result<Entry> {
    let mut creds = Credentials::load()?.unwrap_or_default();
    let gone = creds.find(org, host)?.clone();
    creds
        .entries
        .retain(|e| !(e.host == gone.host && e.org == gone.org));
    if creds.entries.is_empty() {
        // Nobody is signed in any more, so the file goes and with it every
        // reason this binary had to reach the cloud.
        let path = credentials_path()?;
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    } else {
        creds.save()?;
    }
    Ok(gone)
}

/// `GET /v1/orgs/<org>/status`, and the cloud's version from its header.
pub fn status(entry: &Entry) -> Result<(Value, Option<String>), Failure> {
    let reply = call_as(
        entry,
        "GET",
        &format!("/v1/orgs/{}/status", seg(&entry.org)),
        Send {
            timeout: Some(FORWARD_TIMEOUT),
            ..Send::default()
        },
    )?;
    Ok((reply.json()?, reply.version))
}

/// Seconds behind, as a person reads it.
pub fn lag(seconds: Option<i64>) -> String {
    match seconds {
        None => "no lag known".to_string(),
        Some(s) if s < 120 => format!("{s} s behind"),
        Some(s) if s < 7200 => format!("{} min behind", s / 60),
        Some(s) => format!("{} h behind", s / 3600),
    }
}

/// The three cards `semlith cloud status` prints.
pub fn render_status(entry: &Entry, status: &Value, version: Option<&str>) -> String {
    let org = &status["org"];
    let s = |v: &Value| v.as_str().unwrap_or("—").to_string();
    let mut out = String::new();
    let seats = match (org["seats_used"].as_i64(), org["seats"].as_i64()) {
        (Some(used), Some(of)) => format!(" · {used} of {of} seats"),
        (None, Some(of)) => format!(" · {of} seats"),
        _ => String::new(),
    };
    let trial = match org["trial_ends_at"].as_str() {
        Some(at) if org["trial"].as_bool() == Some(true) => {
            format!(" · trial ends {}", &at[..at.len().min(10)])
        }
        _ => String::new(),
    };
    out.push_str(&format!(
        "{} · {} · {} ({}){seats}{trial}\n",
        s(&org["slug"]),
        s(&org["name"]),
        s(&org["plan"]),
        s(&org["status"]),
    ));
    out.push_str(&format!("host     {}\n", entry.host));
    out.push_str(&format!("token    {}…\n", entry.prefix()));
    if let Some(app) = org["app_url"].as_str() {
        out.push_str(&format!("app      {app}\n"));
    }
    if let Some(mcp) = status["mcp_url"].as_str() {
        out.push_str(&format!("mcp      {mcp}\n"));
    }
    if let Some(v) = version {
        out.push_str(&format!("cloud    {v}\n"));
    }
    let stores = status["stores"].as_array().cloned().unwrap_or_default();
    out.push_str(&format!("stores   {}\n", stores.len()));
    for store in &stores {
        let name = s(&store["name"]);
        out.push_str(&format!(
            "  {name}  {} · {} files · {} chunks",
            s(&store["state"]),
            store["files"].as_i64().unwrap_or(0),
            store["chunks"].as_i64().unwrap_or(0),
        ));
        if let Some(mcp) = status["mcp_url"].as_str() {
            out.push_str(&format!(" · reach {mcp}?store={name}"));
        }
        out.push('\n');
        for source in store["sources"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "    {}  {}  {}  {}  {}\n",
                s(&source["label"]),
                s(&source["kind"]),
                s(&source["revision"]),
                lag(source["behind_seconds"].as_i64()),
                s(&source["state"]),
            ));
        }
    }
    let usage = &status["usage"];
    if usage.is_object() {
        out.push_str(&format!(
            "usage    {} · {} of {} stored · {} of {} index minutes · queue {} running, {} waiting\n",
            s(&usage["month"]),
            crate::human_bytes(usage["store_bytes"].as_i64().unwrap_or(0)),
            crate::human_bytes(usage["cap_bytes"].as_i64().unwrap_or(0)),
            usage["index_minutes"].as_i64().unwrap_or(0),
            usage["cap_minutes"].as_i64().unwrap_or(0),
            usage["queue"]["running"].as_i64().unwrap_or(0),
            usage["queue"]["waiting"].as_i64().unwrap_or(0),
        ));
    }
    out.trim_end().to_string()
}

/// The MCP URL to keep for an org: the host's own when it is on the same
/// origin, else the documented path on the entry's host. A URL a reply named
/// on another origin is never one a token is sent to.
pub fn mcp_url_for(entry: &Entry, reported: Option<&str>) -> String {
    match reported {
        Some(url) if url.starts_with(&format!("{}/", entry.host)) => url.to_string(),
        _ => format!("{}/{}/mcp", entry.host, seg(&entry.org)),
    }
}

/// `semlith cloud connect`: one remote store per store the status lists (or
/// per name in `only`), named `<org>/<store>`. Returns the names written.
pub fn connect(entry: &Entry, only: &[String]) -> Result<Vec<String>> {
    let (status, _) = status(entry)?;
    let listed: Vec<String> = status["stores"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["name"].as_str().map(str::to_string))
        .collect();
    for name in only {
        if !listed.contains(name) {
            bail!(
                "{} has no store called {name} that this token can reach; it can reach: {}",
                entry.org,
                if listed.is_empty() {
                    "none".to_string()
                } else {
                    listed.join(", ")
                }
            );
        }
    }
    let mcp_url = mcp_url_for(entry, status["mcp_url"].as_str());
    let mut registry = crate::home::Registry::load()?;
    let mut written = Vec::new();
    for store in listed
        .iter()
        .filter(|s| only.is_empty() || only.contains(s))
    {
        let name = format!("{}/{store}", entry.org);
        registry.remote.insert(
            name.clone(),
            crate::home::Remote {
                host: entry.host.clone(),
                org: entry.org.clone(),
                store: store.clone(),
                mcp_url: mcp_url.clone(),
            },
        );
        written.push(name);
    }
    registry.save()?;
    Ok(written)
}

/// `semlith cloud disconnect`: take exactly that org's remote stores out of
/// the registry. Returns the names removed.
pub fn disconnect(org: &str) -> Result<Vec<String>> {
    let mut registry = crate::home::Registry::load()?;
    let gone: Vec<String> = registry
        .remote
        .iter()
        .filter(|(_, r)| r.org == org)
        .map(|(name, _)| name.clone())
        .collect();
    if gone.is_empty() {
        return Ok(gone);
    }
    for name in &gone {
        registry.remote.remove(name);
    }
    registry.save()?;
    Ok(gone)
}

/// Every remote store, by local name. Empty — and no more than one file read
/// — for anybody who never connected one.
pub fn remote_stores() -> Vec<Named> {
    crate::home::Registry::load()
        .map(|r| r.remote.into_iter().collect())
        .unwrap_or_default()
}

/// A remote store with the local name it is known by, `<org>/<store>`.
pub type Named = (String, crate::home::Remote);

/// `remote · acme`, the badge a remote store carries everywhere it is listed.
pub fn badge(remote: &crate::home::Remote) -> String {
    format!("remote · {}", remote.org)
}

/// The rows the Stores page and every picker list beside the local stores.
pub fn remote_rows() -> Vec<Value> {
    remote_stores()
        .into_iter()
        .map(|(name, r)| {
            json!({
                "name": name,
                "org": r.org,
                "store": r.store,
                "host": r.host,
                "host_name": host_name(&r.host),
                "mcp_url": r.mcp_url,
                "badge": badge(&r),
            })
        })
        .collect()
}

/// How long a forwarded call may hold up the local answer it rides beside.
const FORWARD_TIMEOUT: Duration = Duration::from_secs(15);

/// Who a forwarded call is for, so the org's ledger names the agent and not
/// this daemon (`Semlith-Client`, `Semlith-Session`).
pub struct Who<'a> {
    pub client: &'a str,
    pub session: &'a str,
}

impl Who<'_> {
    fn headers(&self) -> Vec<(&'static str, String)> {
        vec![
            ("Semlith-Client", header_safe(self.client)),
            ("Semlith-Session", header_safe(self.session)),
        ]
    }
}

/// A header value cannot carry a line break or anything outside printable
/// ASCII; a client name that does is sent with those characters dropped.
fn header_safe(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(128)
        .collect()
}

/// One hit from `POST /v1/orgs/<org>/search`: the binary's own hit shape, plus
/// where it came from.
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteHit {
    pub score: f32,
    /// The store's name in the cloud.
    #[serde(default)]
    pub store: String,
    pub path: String,
    #[serde(default)]
    pub start_line: u32,
    #[serde(default)]
    pub end_line: u32,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub lists: Vec<String>,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub symbol_kind: Option<String>,
    #[serde(default = "yes")]
    pub fresh: bool,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub behind_seconds: Option<i64>,
    /// `<org>/<store>`, the name this machine knows it by.
    #[serde(skip)]
    pub name: String,
    #[serde(skip)]
    pub org: String,
}

fn yes() -> bool {
    true
}

impl RemoteHit {
    /// What a remote row says beside its store: the badge, the revision it
    /// was indexed at and how far behind its source that is.
    pub fn provenance(&self) -> String {
        let mut out = format!("remote · {}", self.org);
        if let Some(rev) = &self.revision {
            out.push_str(&format!(" · {rev}"));
        }
        if self.behind_seconds.is_some() {
            out.push_str(&format!(" · {}", lag(self.behind_seconds)));
        }
        out
    }

    /// As a local hit, so one renderer draws both. The provenance rides in
    /// the store label, which is what a locate row groups by.
    pub fn to_hit(&self) -> crate::Hit {
        crate::Hit {
            score: self.score,
            path: self.path.clone(),
            start_line: self.start_line,
            end_line: self.end_line,
            text: self.text.clone(),
            store: Some(format!("{} ({})", self.name, self.provenance())),
            lists: self
                .lists
                .iter()
                .filter_map(|l| match l.as_str() {
                    "vector" => Some("vector"),
                    "keyword" => Some("keyword"),
                    "image" => Some("image"),
                    "definition" => Some("definition"),
                    "graph" => Some("graph"),
                    _ => None,
                })
                .collect(),
            image: None,
            fresh: self.fresh,
            symbol: self.symbol.clone(),
            symbol_kind: self.symbol_kind.clone(),
            symbol_line: None,
            copies: Vec::new(),
        }
    }

    /// As a row of `/api/search`, with the remote fields beside the usual ones.
    pub fn to_json(&self, query: &str) -> Value {
        json!({
            "line": crate::mcp::line_for(&self.text, query),
            "score": self.score,
            "path": self.path,
            "start_line": self.start_line,
            "end_line": self.end_line,
            // Always the text, whatever the format: the page opens a remote
            // row from it rather than asking the host a second time.
            "text": self.text,
            "store": self.name,
            "lists": self.lists,
            "image": null,
            "fresh": self.fresh,
            "symbol": self.symbol,
            "symbol_kind": self.symbol_kind,
            "remote": self.org,
            "badge": format!("remote · {}", self.org),
            "source": self.source,
            "revision": self.revision,
            "behind_seconds": self.behind_seconds,
        })
    }
}

/// What a search across remote stores brought back, and what it could not.
#[derive(Default)]
pub struct Fetched {
    pub hits: Vec<RemoteHit>,
    /// One phrase per group of stores skipped, naming them and why.
    pub skipped: Vec<String>,
    /// The stores behind those phrases, so a count can say "skipped" for
    /// them rather than 0.
    pub skipped_names: Vec<String>,
}

/// The search a remote store is asked: the same arguments the local one got.
pub struct Query<'a> {
    pub query: &'a str,
    pub k: usize,
    pub path: &'a [String],
    pub ext: &'a [String],
    pub lang: &'a [String],
    pub prefer: Option<&'a str>,
}

/// `remote stores skipped: …`, or nothing when nothing was.
pub fn skipped_line(skipped: &[String]) -> Option<String> {
    (!skipped.is_empty()).then(|| format!("remote stores skipped: {}", skipped.join("; ")))
}

/// The remote stores in `targets`, grouped by the credential that reaches
/// them: one request per org rather than per store.
fn by_org(targets: &[Named]) -> Vec<(Result<Entry, String>, Vec<&Named>)> {
    let creds = Credentials::load().ok().flatten();
    let mut groups: Vec<((String, String), Vec<&Named>)> = Vec::new();
    for target in targets {
        let key = (target.1.host.clone(), target.1.org.clone());
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(target),
            None => groups.push((key, vec![target])),
        }
    }
    groups
        .into_iter()
        .map(|((host, org), list)| {
            let entry = creds
                .as_ref()
                .and_then(|c| c.entries.iter().find(|e| e.host == host && e.org == org))
                .cloned()
                .ok_or_else(|| format!("not signed in to {org}; run `semlith cloud login {org}`"));
            (entry, list)
        })
        .collect()
}

fn names(list: &[&Named]) -> String {
    list.iter()
        .map(|(n, _)| n.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn nonempty(v: &[String]) -> Value {
    if v.is_empty() { Value::Null } else { json!(v) }
}

/// Search `targets` through `POST /v1/orgs/<org>/search`. Never fails: a
/// store that could not be asked is in `skipped`, and the local answer it
/// rides beside stays whole.
pub fn search(targets: &[Named], q: &Query<'_>, who: &Who<'_>) -> Fetched {
    let mut out = Fetched::default();
    for (entry, list) in by_org(targets) {
        let entry = match entry {
            Ok(e) => e,
            Err(why) => {
                out.skipped.push(format!("{} ({why})", names(&list)));
                out.skipped_names
                    .extend(list.iter().map(|(n, _)| n.clone()));
                continue;
            }
        };
        let body = json!({
            "query": q.query,
            "k": q.k,
            "stores": list.iter().map(|(_, r)| r.store.as_str()).collect::<Vec<_>>(),
            "path": nonempty(q.path),
            "ext": nonempty(q.ext),
            "lang": nonempty(q.lang),
            "prefer": q.prefer,
            "exact": false,
        });
        let reply = call_as(
            &entry,
            "POST",
            &format!("/v1/orgs/{}/search", seg(&entry.org)),
            Send {
                json: Some(&body),
                headers: who.headers(),
                timeout: Some(FORWARD_TIMEOUT),
                ..Send::default()
            },
        )
        .and_then(|r| r.json());
        match reply {
            Ok(answer) => {
                for raw in answer["hits"].as_array().into_iter().flatten() {
                    let Ok(mut hit) = serde_json::from_value::<RemoteHit>(raw.clone()) else {
                        continue;
                    };
                    // A hit for a store this machine did not ask about is not
                    // one to show; a missing store name is the only one asked.
                    let Some((name, _)) = list.iter().find(|(_, r)| {
                        r.store == hit.store || (hit.store.is_empty() && list.len() == 1)
                    }) else {
                        continue;
                    };
                    hit.name = name.clone();
                    hit.org = entry.org.clone();
                    out.hits.push(hit);
                }
            }
            Err(failure) => {
                out.skipped.push(format!("{} ({failure})", names(&list)));
                out.skipped_names
                    .extend(list.iter().map(|(n, _)| n.clone()));
            }
        }
    }
    out
}

/// Local and remote hits as one list, by score, cut to `k`.
///
/// Both sides are the same core's fused scores (cloud-api.md, "Search, for
/// merging"), which is what makes one order meaningful. Local hits are
/// labelled with their store when the fleet left them unlabelled, so a
/// merged list never mixes named and unnamed rows.
pub fn merge(
    local: Vec<crate::Hit>,
    local_label: Option<&str>,
    remote: &[RemoteHit],
    k: usize,
) -> Vec<crate::Hit> {
    let mut all: Vec<crate::Hit> = local
        .into_iter()
        .map(|mut h| {
            if h.store.is_none() {
                h.store = local_label.map(str::to_string);
            }
            h
        })
        .collect();
    all.extend(remote.iter().map(RemoteHit::to_hit));
    all.sort_by(|a, b| b.score.total_cmp(&a.score));
    all.truncate(k);
    all
}

/// [`merge`] for an answer in JSON: local rows as they are, remote hits as
/// [`RemoteHit::to_json`] rows with their remote fields, by score, cut to `k`.
pub fn merge_rows(local: Vec<Value>, remote: &[RemoteHit], query: &str, k: usize) -> Vec<Value> {
    let mut all = local;
    all.extend(remote.iter().map(|h| h.to_json(query)));
    let score = |v: &Value| v["score"].as_f64().unwrap_or(0.0);
    all.sort_by(|a, b| score(b).total_cmp(&score(a)));
    all.truncate(k);
    all
}

/// Forward one tool call to a remote store's MCP endpoint and return its
/// text, or why it could not be asked.
pub fn forward_tool(
    name: &str,
    remote: &crate::home::Remote,
    tool: &str,
    args: &Value,
    who: &Who<'_>,
) -> Result<String, String> {
    let creds = Credentials::load().ok().flatten();
    let Some(entry) = creds.as_ref().and_then(|c| {
        c.entries
            .iter()
            .find(|e| e.host == remote.host && e.org == remote.org)
    }) else {
        return Err(format!(
            "{name} (not signed in to {}; run `semlith cloud login {}`)",
            remote.org, remote.org
        ));
    };
    // The kept URL's path, or the documented one: never another origin.
    let base = mcp_url_for(entry, Some(&remote.mcp_url));
    let path = base[entry.host.len()..].to_string();
    let mut arguments = args.clone();
    if let Some(map) = arguments.as_object_mut() {
        map.insert("store".into(), json!(remote.store));
    }
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments },
    });
    let mut headers = who.headers();
    headers.push(("Accept", "application/json, text/event-stream".into()));
    headers.push(("MCP-Protocol-Version", "2025-06-18".into()));
    let reply = call_as(
        entry,
        "POST",
        &format!("{path}?store={}", seg(&remote.store)),
        Send {
            json: Some(&body),
            headers,
            timeout: Some(FORWARD_TIMEOUT),
            ..Send::default()
        },
    )
    .map_err(|f| format!("{name} ({f})"))?;
    let answer = rpc_answer(&reply.body)
        .ok_or_else(|| format!("{name} (the cloud's answer was not an MCP reply)"))?;
    if let Some(message) = answer["error"]["message"].as_str() {
        return Err(format!("{name} (refused: {message})"));
    }
    let text: Vec<&str> = answer["result"]["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["text"].as_str())
        .collect();
    Ok(text.join("\n"))
}

/// A JSON-RPC reply out of a body that is either JSON or an event stream.
fn rpc_answer(body: &[u8]) -> Option<Value> {
    if let Ok(v) = serde_json::from_slice::<Value>(body) {
        return Some(v);
    }
    String::from_utf8_lossy(body)
        .lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
        .rfind(|v| v.get("result").is_some() || v.get("error").is_some())
}

/// The largest file a push sends. The cloud refuses anything bigger again.
pub const PUSH_FILE_CAP: u64 = 1 << 20;

/// The most raw bytes one upload request carries; the contract's ceiling is
/// 100 MB per request, and tar headers are counted against what is left.
const UPLOAD_BATCH: u64 = 96 << 20;

/// How long one upload request may take.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// How often `--wait` asks how the job is doing.
const JOB_POLL: Duration = Duration::from_secs(2);

/// What a push did.
#[derive(Debug, Default, Serialize)]
pub struct PushReport {
    /// Files in the manifest: everything the source holds after this push.
    pub files: usize,
    /// Files whose bytes were sent, because the source did not hold them.
    pub sent: usize,
    pub bytes_sent: u64,
    /// Paths the source drops because they are no longer in the tree.
    pub removed: usize,
    /// What the binary's own rules kept back, with why.
    pub refused: Vec<(String, String)>,
    pub push: String,
    pub job: Option<i64>,
    pub position: Option<i64>,
    pub estimate_minutes: Option<i64>,
    /// The job's last state, when `--wait` followed it.
    pub state: Option<String>,
}

/// `org/store` into its halves.
pub fn split_target(target: &str) -> Result<(&str, &str)> {
    match target.split_once('/') {
        Some((org, store)) if !org.is_empty() && !store.is_empty() && !store.contains('/') => {
            Ok((org, store))
        }
        _ => bail!("name the store as <org>/<store>, for example acme/platform"),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `semlith cloud push <org>/<store> <dir>`: the manifest after the binary's
/// own refusals, the files the store's upload source does not already hold
/// in tar.gz requests of at most 100 MB, the commit, and with `wait` the job
/// followed to its end. Without `prune` the cloud refuses a push that would
/// remove files the source holds, naming them.
pub fn push(
    target: &str,
    dir: &Path,
    wait: bool,
    prune: bool,
    say: &mut dyn FnMut(&str),
) -> Result<PushReport> {
    let (org, store) = split_target(target)?;
    let entry = entry_for(Some(org), None)?;
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    let root = crate::canonical(dir);
    let (files, refused) = crate::push_files(&root, PUSH_FILE_CAP);
    let rel = |p: &Path| -> String {
        p.strip_prefix(&root)
            .unwrap_or(p)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    };
    let mut report = PushReport {
        refused: refused
            .iter()
            .map(|(p, why)| (crate::plain(&rel(p)), why.clone()))
            .collect(),
        ..PushReport::default()
    };
    let mut manifest = Vec::with_capacity(files.len());
    let mut by_path: std::collections::HashMap<String, (PathBuf, String)> = Default::default();
    for path in &files {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let name = rel(path);
        let hash = sha256_hex(&bytes);
        manifest.push(json!({ "path": name, "sha256": hash, "bytes": bytes.len() }));
        by_path.insert(name, (path.clone(), hash));
    }
    report.files = manifest.len();
    say(&format!(
        "{} file{} to {target} from {}{}",
        manifest.len(),
        if manifest.len() == 1 { "" } else { "s" },
        root.display(),
        if refused.is_empty() {
            String::new()
        } else {
            format!(" · {} kept back by semlith's own rules", refused.len())
        }
    ));
    let mut body = json!({ "files": manifest, "prune": prune });
    if let Some(commit) = git_head(&root) {
        body["commit"] = json!(commit);
    }
    let opened = call_as(
        &entry,
        "POST",
        &format!("/v1/orgs/{}/stores/{}/pushes", seg(org), seg(store)),
        Send {
            json: Some(&body),
            ..Send::default()
        },
    )?
    .json()?;
    report.push = opened["push"].as_str().unwrap_or_default().to_string();
    if report.push.is_empty() {
        bail!("{} opened no push", entry.host_name());
    }
    report.removed = opened["remove"].as_array().map_or(0, Vec::len);
    report.estimate_minutes = opened["estimate_minutes"].as_i64();
    let need: Vec<&str> = opened["need"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|p| by_path.contains_key(*p))
        .collect();

    // Only what the source does not hold, in batches under the ceiling.
    let need_total = need.len();
    let mut batch: Vec<(String, Vec<u8>)> = Vec::new();
    let mut batch_bytes = 0u64;
    let mut send_batch =
        |batch: &mut Vec<(String, Vec<u8>)>, report: &mut PushReport| -> Result<()> {
            if batch.is_empty() {
                return Ok(());
            }
            let raw: u64 = batch.iter().map(|(_, b)| b.len() as u64).sum();
            let archive = tar_gz(batch)?;
            call_as(
                &entry,
                "PUT",
                &format!("/v1/orgs/{}/pushes/{}/files", seg(org), seg(&report.push)),
                Send {
                    bytes: Some(("application/x-tar", Some("gzip"), archive)),
                    timeout: Some(UPLOAD_TIMEOUT),
                    ..Send::default()
                },
            )?;
            report.sent += batch.len();
            report.bytes_sent += raw;
            say(&format!(
                "sent {} of {} changed files ({})",
                report.sent,
                need_total,
                crate::human_bytes(report.bytes_sent as i64)
            ));
            batch.clear();
            Ok(())
        };
    for name in &need {
        let (path, hash) = &by_path[*name];
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        // The file changed between the manifest and now: send the bytes the
        // manifest named or nothing, since the cloud refuses a mismatch.
        if &sha256_hex(&bytes) != hash {
            bail!(
                "{} changed while it was being pushed; push again",
                path.display()
            );
        }
        let cost = bytes.len() as u64 + 1024;
        if batch_bytes + cost > UPLOAD_BATCH {
            send_batch(&mut batch, &mut report)?;
            batch_bytes = 0;
        }
        batch_bytes += cost;
        batch.push((name.to_string(), bytes));
    }
    send_batch(&mut batch, &mut report)?;
    if need.is_empty() {
        say("nothing changed since the last push; no file was sent");
    }

    let committed = call_as(
        &entry,
        "POST",
        &format!("/v1/orgs/{}/pushes/{}/commit", seg(org), seg(&report.push)),
        Send::default(),
    )?
    .json()?;
    report.job = committed["job"].as_i64();
    report.position = committed["position"].as_i64();
    if let Some(job) = report.job {
        say(&format!(
            "job {job} queued{}",
            report
                .position
                .map(|p| format!(", {p} ahead of it"))
                .unwrap_or_default()
        ));
        if wait {
            let mut last = String::new();
            loop {
                let state = call_as(
                    &entry,
                    "GET",
                    &format!("/v1/orgs/{}/jobs/{job}", seg(org)),
                    Send::default(),
                )?
                .json()?;
                let now = state["state"].as_str().unwrap_or("unknown").to_string();
                if now != last {
                    say(&format!(
                        "job {job}: {now}{}",
                        state["chunks"]
                            .as_i64()
                            .map(|c| format!(" · {c} chunks"))
                            .unwrap_or_default()
                    ));
                    last = now.clone();
                }
                if matches!(now.as_str(), "done" | "stopped" | "error") {
                    if now == "error"
                        && let Some(outcome) = state["outcome"].as_str()
                    {
                        say(outcome);
                    }
                    report.state = Some(now);
                    break;
                }
                std::thread::sleep(JOB_POLL);
            }
        }
    }
    Ok(report)
}

/// The commit a working tree is at, when it is a git checkout.
fn git_head(dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let head = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && head.len() >= 7).then_some(head)
}

/// A gzipped ustar archive of `files`. A path longer than the header's 100
/// bytes travels in a PAX `path` record, which every tar reader honours.
fn tar_gz(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    for (name, bytes) in files {
        if name.len() > 100 {
            let record = pax_record("path", name);
            gz.write_all(&tar_header("PaxHeader", record.len() as u64, b'x'))?;
            gz.write_all(&padded(record.as_bytes()))?;
        }
        gz.write_all(&tar_header(name, bytes.len() as u64, b'0'))?;
        gz.write_all(&padded(bytes))?;
    }
    gz.write_all(&[0u8; 1024])?;
    Ok(gz.finish()?)
}

/// `"<len> key=value\n"`, where the length counts itself.
fn pax_record(key: &str, value: &str) -> String {
    let body = format!(" {key}={value}\n");
    let mut len = body.len() + 1;
    while format!("{len}{body}").len() != len {
        len += 1;
    }
    format!("{len}{body}")
}

fn padded(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out.resize(bytes.len().div_ceil(512) * 512, 0);
    out
}

fn tar_header(name: &str, size: u64, kind: u8) -> [u8; 512] {
    let mut h = [0u8; 512];
    let name = name.as_bytes();
    let cut = name.len().min(100);
    h[..cut].copy_from_slice(&name[..cut]);
    let octal = |h: &mut [u8; 512], at: usize, width: usize, value: u64| {
        let text = format!("{value:0w$o}", w = width - 1);
        h[at..at + width - 1].copy_from_slice(text.as_bytes());
    };
    octal(&mut h, 100, 8, 0o644);
    octal(&mut h, 108, 8, 0);
    octal(&mut h, 116, 8, 0);
    octal(&mut h, 124, 12, size);
    octal(&mut h, 136, 12, 0);
    h[148..156].copy_from_slice(b"        ");
    h[156] = kind;
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    let sum: u32 = h.iter().map(|b| *b as u32).sum();
    let text = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(text.as_bytes());
    h
}

/// The most rows one ledger request carries.
pub const SYNC_BATCH: usize = 1000;

/// One request a minute per machine, at most.
pub const SYNC_EVERY: Duration = Duration::from_secs(60);

/// `POST /v1/orgs/<org>/ledger`'s body: what was retrieved, never the text.
///
/// `bytes` is null: the local ledger counts what an answer cost in tokens
/// (`excerpt_tokens`, sent as `tokens`), and a byte count made up from it
/// would be a number nobody measured.
pub fn sync_body(machine: &str, store: &str, rows: &[crate::store::SyncRow]) -> Value {
    json!({
        "machine": machine,
        "store": store,
        "rows": rows.iter().map(|r| json!({
            "local_id": r.id,
            "at": crate::clock::utc_rfc3339(r.at),
            "client": r.client,
            "session": r.session,
            "tool": r.tool,
            "bytes": null,
            "tokens": r.excerpt_tokens,
        })).collect::<Vec<_>>(),
    })
}

/// Send one batch of `rows` for `store`. The caller marks them sent only when
/// this succeeds, and a resend after a failure is safe: the cloud keys rows
/// on (org, machine, local_id).
pub fn send_rows(
    entry: &Entry,
    machine: &str,
    store: &str,
    rows: &[crate::store::SyncRow],
) -> Result<Value, Failure> {
    call_as(
        entry,
        "POST",
        &format!("/v1/orgs/{}/ledger", seg(&entry.org)),
        Send {
            json: Some(&sync_body(machine, store, rows)),
            timeout: Some(FORWARD_TIMEOUT),
            ..Send::default()
        },
    )?
    .json()
}

/// One store's next batch, start to finish: read, send, mark. `Ok(0)` when
/// there was nothing to send, in which case no request was made.
pub fn sync_store(
    entry: &Entry,
    machine: &str,
    store: &str,
    db: &rusqlite::Connection,
    since: i64,
) -> Result<usize> {
    let rows = crate::store::unsynced(db, since, SYNC_BATCH)?;
    if rows.is_empty() {
        return Ok(0);
    }
    send_rows(entry, machine, store, &rows)?;
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    crate::store::mark_synced(db, &ids, unix_now() as i64)?;
    Ok(rows.len())
}

/// What the sync thread last did, for the Ledger page's line.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncState {
    pub last_sent: Option<u64>,
    pub last_store: Option<String>,
    pub last_rows: usize,
    pub last_error: Option<String>,
}

static SYNC: std::sync::Mutex<Option<SyncState>> = std::sync::Mutex::new(None);
static SYNCING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn note_sync(change: impl FnOnce(&mut SyncState)) {
    let mut state = SYNC.lock().unwrap_or_else(|e| e.into_inner());
    change(state.get_or_insert_with(SyncState::default));
}

/// Start the daemon's sync thread, once. Called at start only when this
/// machine has signed in, and by the switch that turns sync on, so a daemon
/// nobody signed in to never runs it.
///
/// Each minute it sends at most one batch, from the first store with rows
/// waiting: one request a minute per machine is the contract's ceiling. The
/// fleet's lock is held to read and to mark, never across the request.
pub fn spawn_sync(state: std::sync::Arc<crate::daemon::State>) {
    use std::sync::atomic::Ordering;
    if SYNCING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        while !crate::watch::STOP.load(Ordering::Relaxed) {
            let started = std::time::Instant::now();
            while started.elapsed() < SYNC_EVERY {
                if crate::watch::STOP.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            sync_pass(&state);
        }
    });
}

/// One minute's work: the first store with rows waiting sends one batch.
fn sync_pass(state: &crate::daemon::State) {
    let Ok(Some(creds)) = Credentials::load() else {
        return;
    };
    let Ok(registry) = crate::home::Registry::load() else {
        return;
    };
    for (name, entry) in &registry.stores {
        let Some(sync) = &entry.settings.cloud_sync else {
            continue;
        };
        let Some(cred) = creds.entries.iter().find(|e| e.org == sync.org) else {
            note_sync(|s| s.last_error = Some(format!("not signed in to {}", sync.org)));
            continue;
        };
        if state.open_fleet().is_err() {
            return;
        }
        let rows = {
            let fleet = state.fleet.lock().unwrap_or_else(|e| e.into_inner());
            let Some(store) = fleet
                .as_ref()
                .and_then(|f| f.each().find(|(l, _)| l == name).map(|(_, s)| s))
            else {
                continue;
            };
            crate::store::unsynced(store.db(), sync.since, SYNC_BATCH).unwrap_or_default()
        };
        if rows.is_empty() {
            continue;
        }
        match send_rows(cred, &creds.machine_id(), name, &rows) {
            Ok(_) => {
                let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
                let fleet = state.fleet.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(store) = fleet
                    .as_ref()
                    .and_then(|f| f.each().find(|(l, _)| l == name).map(|(_, s)| s))
                {
                    let _ = crate::store::mark_synced(store.db(), &ids, unix_now() as i64);
                }
                note_sync(|s| {
                    s.last_sent = Some(unix_now());
                    s.last_store = Some(name.clone());
                    s.last_rows = rows.len();
                    s.last_error = None;
                });
            }
            Err(e) => note_sync(|s| s.last_error = Some(e.to_string())),
        }
        // One request a minute, whatever it answered.
        return;
    }
}

/// `semlith cloud sync <store> on|off`: record the switch. Returns what is
/// now recorded.
pub fn set_sync(
    store: &str,
    on: bool,
    org: Option<&str>,
) -> Result<Option<crate::home::CloudSync>> {
    crate::home::refuse_remote(store)?;
    let sync = if on {
        let entry = entry_for(org, None)?;
        Some(crate::home::CloudSync {
            org: entry.org,
            since: unix_now() as i64,
        })
    } else {
        None
    };
    crate::home::update_store_settings(store, move |s| match (&s.cloud_sync, sync) {
        // Turning on what is already on, for the same org, keeps its `since`.
        (Some(old), Some(new)) if old.org == new.org => {}
        (_, sync) => s.cloud_sync = sync,
    })
    .map(|s| s.cloud_sync)
}

/// Which local stores sync, of how many, and what the thread last did.
pub fn sync_view() -> Value {
    let registry = crate::home::Registry::load().unwrap_or_default();
    let on: Vec<Value> = registry
        .stores
        .iter()
        .filter_map(|(name, e)| {
            e.settings
                .cloud_sync
                .as_ref()
                .map(|s| json!({ "store": name, "org": s.org, "since": s.since }))
        })
        .collect();
    let last = SYNC
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default();
    json!({
        "on": on,
        "stores": registry.stores.len(),
        "last_sent": last.last_sent,
        "last_store": last.last_store,
        "last_rows": last.last_rows,
        "last_error": last.last_error,
    })
}

/// The formats `GET /v1/orgs/<org>/reports/<kind>` answers in.
pub const REPORT_FORMATS: [&str; 5] = ["md", "csv", "json", "html", "pdf"];

/// What a cloud report is asked for.
pub struct ReportAsk<'a> {
    pub kind: &'a str,
    pub format: &'a str,
    pub window: Option<&'a str>,
    pub model: Option<&'a str>,
    pub stores: &'a [String],
}

/// `semlith cloud report`: the cloud's report file, as its bytes.
pub fn report(entry: &Entry, ask: &ReportAsk<'_>) -> Result<Vec<u8>> {
    if !REPORT_FORMATS.contains(&ask.format) {
        bail!(
            "{} is not a report format; one of {}",
            ask.format,
            REPORT_FORMATS.join(", ")
        );
    }
    let mut query = format!("format={}", seg(ask.format));
    if let Some(window) = ask.window {
        query.push_str(&format!("&window={}", seg(window)));
    }
    if let Some(model) = ask.model {
        query.push_str(&format!("&model={}", seg(model)));
    }
    if !ask.stores.is_empty() {
        let list: Vec<String> = ask.stores.iter().map(|s| seg(s)).collect();
        query.push_str(&format!("&stores={}", list.join(",")));
    }
    let reply = call_as(
        entry,
        "GET",
        &format!(
            "/v1/orgs/{}/reports/{}?{query}",
            seg(&entry.org),
            seg(ask.kind)
        ),
        Send {
            headers: vec![("Accept", "*/*".into())],
            timeout: Some(Duration::from_secs(120)),
            ..Send::default()
        },
    )?;
    Ok(reply.body)
}

/// `semlith cloud replay <session>`: one transcript the member picked, sent
/// to the org's ledger. Returns how many answers the cloud accepted.
pub fn replay(entry: &Entry, session: &crate::replay::Session) -> Result<i64> {
    let items: Vec<Value> = session
        .recent
        .iter()
        .map(|a| {
            json!({
                "at": a.at,
                "tool": a.tool,
                "query": a.query,
                // The contract's word for "nothing recorded".
                "outcome": if a.outcome == "unknown" { "none" } else { a.outcome },
            })
        })
        .collect();
    let sent = call_as(
        entry,
        "POST",
        &format!("/v1/orgs/{}/replay", seg(&entry.org)),
        Send {
            json: Some(&json!({
                "session": session.id,
                "client": crate::replay::CLIENT,
                "items": items,
            })),
            ..Send::default()
        },
    );
    match sent {
        Ok(reply) => Ok(reply.json()?["accepted"].as_i64().unwrap_or(0)),
        // The cloud's own sentence says the same thing; one is enough.
        Err(Failure::Refused { code, .. }) if code == "replay_off" => bail!(
            "Session replay is off for {}: an admin turns it on in the cloud app under \
             Ledger › Session replay. Nothing was kept.",
            entry.org
        ),
        Err(other) => Err(other.into()),
    }
}

/// The transcripts this machine may read, or why it may not. The Privacy
/// page's Session replay switch governs every reader, this one included.
pub fn replay_sessions() -> Result<(PathBuf, crate::replay::Replay)> {
    if !crate::home::Settings::load().replay_on() {
        bail!(
            "Session replay is off on this machine (the Privacy page's switch), so semlith \
             reads no transcript; turn it on to pick one to send"
        );
    }
    let dir = crate::replay::transcripts_dir()?;
    let found = crate::replay::read(&dir, crate::replay::FILES)?;
    Ok((dir, found))
}

/// Settings › Cloud's live half: each signed-in org's status, and whether its
/// host answered (`connected`), could not be reached (`unreachable`) or
/// refused (`refused`), with the sentence that says why. Asked only when the
/// section is opened, and only for a machine that signed in.
pub fn status_view() -> Value {
    let creds = Credentials::load().ok().flatten();
    let orgs: Vec<Value> = creds
        .iter()
        .flat_map(|c| c.entries.iter())
        .map(|e| match status(e) {
            Ok((status, version)) => json!({
                "org": e.org, "host": e.host, "reach": "connected",
                "why": format!("{} answered just now", e.host_name()),
                "version": version, "status": status,
            }),
            Err(f) => json!({
                "org": e.org, "host": e.host, "reach": f.kind(),
                "why": f.to_string(), "code": f.code(),
            }),
        })
        .collect();
    json!({ "orgs": orgs })
}

/// `semlith doctor`'s cloud line: signed in or not, and for each org whether
/// its host answers `/v1/whoami`. Not signed in asks nothing.
pub fn doctor() -> Vec<Value> {
    let Some(creds) = Credentials::load().ok().flatten() else {
        return Vec::new();
    };
    creds
        .entries
        .iter()
        .map(|e| {
            let asked = call_as(
                e,
                "GET",
                "/v1/whoami",
                Send {
                    timeout: Some(FORWARD_TIMEOUT),
                    ..Send::default()
                },
            );
            match asked {
                Ok(reply) => json!({
                    "org": e.org, "host": e.host, "reach": "connected",
                    "why": format!("{} answered with this token", e.host_name()),
                    "version": reply.version,
                }),
                Err(f) => {
                    json!({ "org": e.org, "host": e.host, "reach": f.kind(), "why": f.to_string() })
                }
            }
        })
        .collect()
}

/// What the portal's Cloud section draws without asking the host anything:
/// who this machine is signed in as, by prefix. Never the token.
pub fn local_view() -> Value {
    let creds = Credentials::load().ok().flatten();
    let orgs: Vec<Value> = creds
        .iter()
        .flat_map(|c| c.entries.iter())
        .map(|e| {
            json!({
                "org": e.org,
                "host": e.host,
                "host_name": e.host_name(),
                "plan": e.plan,
                "prefix": e.prefix(),
            })
        })
        .collect();
    json!({
        "signed_in": !orgs.is_empty(),
        "orgs": orgs,
        "remote": remote_rows(),
        "sync": sync_view(),
    })
}

#[cfg(test)]
#[path = "../tests/common/stub.rs"]
mod stub;

#[cfg(test)]
mod tests {
    use super::*;

    fn local(score: f32, path: &str) -> crate::Hit {
        RemoteHit {
            name: "x".into(),
            org: "x".into(),
            ..remote(score, path)
        }
        .to_hit()
    }

    fn remote(score: f32, path: &str) -> RemoteHit {
        serde_json::from_value(json!({
            "score": score, "store": "platform", "path": path, "start_line": 1, "end_line": 2,
            "text": "fn order_total() { a long line of code that costs tokens to show }",
            "revision": "a41c9e2", "behind_seconds": 38
        }))
        .map(|mut h: RemoteHit| {
            h.name = "acme/platform".into();
            h.org = "acme".into();
            h
        })
        .unwrap()
    }

    #[test]
    fn a_merge_orders_by_score_cuts_to_k_and_labels_both_sides() {
        let mut mine = local(0.03, "notes/a.md");
        mine.store = None;
        let merged = merge(
            vec![mine, local(0.01, "notes/b.md")],
            Some("notes"),
            &[remote(0.02, "r/c.rs"), remote(0.05, "r/d.rs")],
            3,
        );
        let order: Vec<(&str, f32)> = merged.iter().map(|h| (h.path.as_str(), h.score)).collect();
        assert_eq!(
            order,
            [("r/d.rs", 0.05), ("notes/a.md", 0.03), ("r/c.rs", 0.02)]
        );
        assert_eq!(merged[1].store.as_deref(), Some("notes"));
        assert_eq!(
            merged[0].store.as_deref(),
            Some("acme/platform (remote · acme · a41c9e2 · 38 s behind)")
        );
    }

    /// The budget cuts a merged list as it cuts a local one: the same
    /// renderer, after the merge.
    #[test]
    fn the_token_budget_applies_to_the_merged_list() {
        let many: Vec<RemoteHit> = (0..40)
            .map(|i| remote(1.0 - i as f32 / 100.0, &format!("r/{i}.rs")))
            .collect();
        let merged = merge(Vec::new(), None, &many, 40);
        let reply = crate::mcp::search_reply(
            &crate::fleet::Fleet::empty(),
            &merged,
            "order total",
            crate::Prefer::default(),
            200,
        );
        let kept = reply.kept.iter().filter(|k| **k).count();
        assert!(kept < 40, "{kept} of 40 kept under 200 tokens");
        assert!(
            reply.text.contains(&format!("truncated: {kept} of 40")),
            "{}",
            reply.text
        );
    }

    /// The archive a push sends, read back the way a tar reader reads it.
    #[test]
    fn the_upload_is_a_tar_a_reader_can_unpack() {
        use std::io::Read;
        let long = format!("{}/deep.md", "d".repeat(120));
        let files = vec![
            ("docs/runbook.md".to_string(), b"hello".to_vec()),
            (long.clone(), b"long".to_vec()),
        ];
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(&tar_gz(&files).unwrap()[..])
            .read_to_end(&mut raw)
            .unwrap();
        let mut at = 0;
        let mut found = Vec::new();
        let mut pax_path: Option<String> = None;
        while at + 512 <= raw.len() && raw[at] != 0 {
            let h = &raw[at..at + 512];
            let stored: u32 = h
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    if (148..156).contains(&i) {
                        32
                    } else {
                        *b as u32
                    }
                })
                .sum();
            let said = u32::from_str_radix(std::str::from_utf8(&h[148..154]).unwrap(), 8).unwrap();
            assert_eq!(stored, said, "header checksum");
            let size = u64::from_str_radix(std::str::from_utf8(&h[124..135]).unwrap(), 8).unwrap()
                as usize;
            let name = String::from_utf8_lossy(&h[..100])
                .trim_end_matches('\0')
                .to_string();
            let body = &raw[at + 512..at + 512 + size];
            if h[156] == b'x' {
                let text = String::from_utf8_lossy(body);
                pax_path = text
                    .split_once("path=")
                    .map(|(_, p)| p.trim_end().to_string());
                assert_eq!(
                    text.split_once(' ').unwrap().0.parse::<usize>().unwrap(),
                    text.len()
                );
            } else {
                found.push((pax_path.take().unwrap_or(name), body.to_vec()));
            }
            at += 512 + size.div_ceil(512) * 512;
        }
        assert_eq!(found, files);
    }

    /// A store database holding `n` ledger rows, each with query text that
    /// must never leave the machine.
    fn ledger(n: usize) -> (tempfile::TempDir, rusqlite::Connection) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::store::open(&dir.path().join("store.db")).unwrap();
        let _w = crate::store::Writing::begin(&db).unwrap();
        for i in 0..n {
            db.execute(
                "INSERT INTO retrievals (at, client, query, hits, micros, excerpt_tokens,
                 whole_file_tokens, prev, hash, session, tool)
                 VALUES (?1, 'claude-code', ?2, 3, 10, 120, 900, '', '', 's-1', 'semlith_search')",
                rusqlite::params![1_000 + i as i64, format!("PRIVATE QUERY TEXT {i}")],
            )
            .unwrap();
        }
        drop(_w);
        (dir, db)
    }

    fn entry_at(url: &str) -> Entry {
        Entry {
            host: url.to_string(),
            org: "acme".into(),
            token: format!("{TOKEN_PREFIX}test"),
            plan: None,
            added: 0,
        }
    }

    /// Batches of at most 1,000, nothing sent twice once accepted, a resend
    /// after a failure carrying the same local ids, and no query text in any
    /// body.
    #[test]
    fn ledger_sync_batches_resends_idempotently_and_never_sends_text() {
        let fail_first = std::sync::atomic::AtomicBool::new(true);
        let host = stub::Stub::start(move |seen| {
            assert_eq!(seen.path, "/v1/orgs/acme/ledger");
            if fail_first.swap(false, std::sync::atomic::Ordering::SeqCst) {
                stub::error(503, "unavailable", "Postgres is away.")
            } else {
                stub::json(200, json!({ "accepted": 1, "duplicates": 0 }))
            }
        });
        let (_dir, db) = ledger(1_500);
        let entry = entry_at(&host.url);

        // Failed: nothing marked, so the same rows go again.
        assert!(sync_store(&entry, "m_1", "notes", &db, 0).is_err());
        assert_eq!(crate::store::unsynced_count(&db, 0).unwrap(), 1_500);
        assert_eq!(
            sync_store(&entry, "m_1", "notes", &db, 0).unwrap(),
            SYNC_BATCH
        );
        assert_eq!(sync_store(&entry, "m_1", "notes", &db, 0).unwrap(), 500);
        assert_eq!(sync_store(&entry, "m_1", "notes", &db, 0).unwrap(), 0);

        let seen = host.seen();
        assert_eq!(seen.len(), 3, "an empty batch is no request");
        let ids = |i: usize| -> Vec<i64> {
            seen[i].json()["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["local_id"].as_i64().unwrap())
                .collect()
        };
        assert_eq!(ids(0), ids(1), "the resend carries the same rows");
        assert_eq!(ids(1).len(), 1_000);
        assert_eq!(ids(2).len(), 500);
        assert_eq!(seen[0].json()["rows"][0]["at"], "1970-01-01T00:16:40Z");
        for s in &seen {
            assert!(
                !s.text().contains("PRIVATE QUERY TEXT"),
                "query text left the machine"
            );
            assert!(
                !s.text().contains("query"),
                "a query field left the machine"
            );
            let body = s.json();
            assert_eq!(body["machine"], "m_1");
            assert_eq!(body["store"], "notes");
            assert_eq!(body["rows"][0]["tool"], "semlith_search");
        }
    }

    /// Turning sync on sends what is retrieved from then on, not the history.
    #[test]
    fn ledger_sync_starts_from_when_it_was_turned_on() {
        let (_dir, db) = ledger(10);
        assert_eq!(crate::store::unsynced(&db, 1_005, 1_000).unwrap().len(), 5);
    }

    #[test]
    fn a_host_is_https_or_loopback() {
        assert_eq!(
            normal_host("https://cloud.semlith.com/").unwrap(),
            "https://cloud.semlith.com"
        );
        assert_eq!(
            normal_host("http://127.0.0.1:8080").unwrap(),
            "http://127.0.0.1:8080"
        );
        assert_eq!(
            normal_host("http://localhost:3000").unwrap(),
            "http://localhost:3000"
        );
        assert!(normal_host("http://cloud.semlith.com").is_err());
        assert!(normal_host("https://cloud.semlith.com/acme").is_err());
        assert!(normal_host("https://user@cloud.semlith.com").is_err());
        assert!(normal_host("cloud.semlith.com").is_err());
    }

    #[test]
    fn an_entry_never_prints_its_token() {
        let entry = Entry {
            host: "https://cloud.semlith.com".into(),
            org: "acme".into(),
            token: format!("{TOKEN_PREFIX}7f3cSECRETSECRETSECRET"),
            plan: None,
            added: 0,
        };
        let shown = format!("{entry:?}");
        assert!(!shown.contains("SECRET"), "{shown}");
        assert!(shown.contains("sml_live_7f3c"), "{shown}");
    }

    #[test]
    fn a_refusal_prints_the_hosts_sentence_and_the_next_step() {
        let old = refusal(
            426,
            br#"{"error":{"code":"client_too_old","message":"This semlith is too old.","min_version":"0.37.0"}}"#,
        );
        assert!(old.to_string().contains("Run `semlith upgrade`."), "{old}");
        let plan = refusal(
            402,
            br#"{"error":{"code":"plan_required","message":"Pushing needs a plan.","plan":"pro"}}"#,
        );
        assert_eq!(
            plan.to_string(),
            "Pushing needs a plan. It needs the Pro plan."
        );
        assert_eq!(plan.kind(), "refused");
    }
}
