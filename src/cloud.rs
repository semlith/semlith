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
            let mut req = $req.header("Accept", "application/json");
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
        Send::default(),
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
pub fn remote_stores() -> Vec<(String, crate::home::Remote)> {
    crate::home::Registry::load()
        .map(|r| r.remote.into_iter().collect())
        .unwrap_or_default()
}

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
    json!({ "signed_in": !orgs.is_empty(), "orgs": orgs, "remote": remote_rows() })
}

#[cfg(test)]
#[path = "../tests/common/stub.rs"]
mod stub;

#[cfg(test)]
mod tests {
    use super::*;

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
