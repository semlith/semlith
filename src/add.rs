//! Fetching one URL into a store.
//!
//! This is the only outbound connection semlith makes that is not the embedding
//! model download or `semlith upgrade`, and it exists under the rules issue #41
//! set for the portal: stated as behaviour, tested rather than asserted. The
//! whole of it is one request, started by a person or by an agent that person is
//! talking to, for exactly the URL they gave.
//!
//! What it does *not* do is the larger half of the design. It does not follow
//! links, read a sitemap, re-fetch on a schedule, send a credential, or run a
//! page's JavaScript. There is no timer and no background call. Under
//! `--airgap` it refuses before a socket is opened, so the guarantee that an
//! air-gapped machine can prove the process never reached the network survives
//! this release intact.
//!
//! Fetching and indexing are separate on purpose. `semlith start` is the sole
//! writer of every store it opened, so the daemon and the MCP tool fetch here
//! and then hand the path to the write queue, exactly as they do for a folder.

use anyhow::{Context, Result, bail};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::{embed, home};

/// The largest response body that will be read.
///
/// Read one byte past it, so a file that is exactly the cap is distinguishable
/// from one that runs past it. 32 MiB is far more than any page or paper, and
/// four times the 8 MiB a file may be and still be indexed at all.
pub const MAX_BYTES: u64 = 32 * 1024 * 1024;

/// How many redirects are followed before the chain is refused.
///
/// Five is more than any real document needs — a DOI to a publisher to a PDF is
/// three — and small enough that a redirect loop ends in five requests rather
/// than in however many the server felt like.
const MAX_REDIRECTS: usize = 5;

static RUNTIME_AIRGAP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The Privacy page's airgap switch, for this process.
///
/// A flag rather than `SEMLITH_AIRGAP`: the daemon has threads by the time a
/// person flips it, and setting the environment then is not sound. Every
/// outbound path asks `embed::airgap()`, which asks this too, so the switch
/// and the flag refuse the same things.
pub fn set_runtime_airgap(on: bool) {
    RUNTIME_AIRGAP.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn runtime_airgap() -> bool {
    RUNTIME_AIRGAP.load(std::sync::atomic::Ordering::Relaxed)
}

/// `{on, reason}` for the Privacy page: `flag` for `--airgap` (which sets
/// the variable before any thread starts, so the command line is what tells
/// the two apart), `env` for `SEMLITH_AIRGAP`, `runtime` for the switch.
/// The flag and the variable outrank the switch: turning it off cannot
/// override them.
pub fn airgap_state() -> serde_json::Value {
    let from_env = matches!(
        std::env::var(embed::AIRGAP_ENV).ok().as_deref(),
        Some("1" | "true" | "yes")
    );
    let reason = if from_env {
        Some(if std::env::args().any(|a| a == "--airgap") {
            "flag"
        } else {
            "env"
        })
    } else if runtime_airgap() {
        Some("runtime")
    } else {
        None
    };
    serde_json::json!({ "on": reason.is_some(), "reason": reason })
}

/// Every outbound connection this process has opened, counted in one place.
struct Outbound {
    count: u64,
    since: u64,
    /// `(at, what, host)`, newest last.
    recent: std::collections::VecDeque<(u64, String, String)>,
}

/// How many connections the Privacy page lists by name.
const OUTBOUND_RECENT: usize = 20;

static OUTBOUND: std::sync::Mutex<Option<Outbound>> = std::sync::Mutex::new(None);

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Start the count now, if nothing has: the daemon calls this at start so
/// `since` is when it began listening rather than its first fetch.
pub fn start_counting() {
    let mut out = OUTBOUND.lock().unwrap_or_else(|e| e.into_inner());
    out.get_or_insert_with(|| Outbound {
        count: 0,
        since: unix_now(),
        recent: Default::default(),
    });
}

/// Count one connection about to be opened. `what` says which feature
/// (`add`, `model`, `pack`, `upgrade`, `prices`); only the host of `url` is
/// kept, never its path or query, which can carry a token.
pub fn note_outbound(what: &str, url: &str) {
    let host = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h).to_string();
    start_counting();
    let mut out = OUTBOUND.lock().unwrap_or_else(|e| e.into_inner());
    let Some(out) = out.as_mut() else { return };
    out.count += 1;
    out.recent.push_back((unix_now(), what.to_string(), host));
    while out.recent.len() > OUTBOUND_RECENT {
        out.recent.pop_front();
    }
}

/// `{count, since, recent: [{at, what, host}]}`, newest first.
pub fn outbound() -> serde_json::Value {
    start_counting();
    let out = OUTBOUND.lock().unwrap_or_else(|e| e.into_inner());
    let Some(out) = out.as_ref() else {
        return serde_json::json!({ "count": 0, "since": null, "recent": [] });
    };
    serde_json::json!({
        "count": out.count,
        "since": out.since,
        "recent": out.recent.iter().rev().map(|(at, what, host)| serde_json::json!({
            "at": at, "what": what, "host": host,
        })).collect::<Vec<_>>(),
    })
}

/// The directory inside a store that fetched files land in.
///
/// Inside the store rather than in the user's corpus: `add` writing into a
/// repository someone is working in would put downloaded files under their
/// version control, and "semlith put a file in my repo" is not a thing a search
/// tool gets to do.
pub const DOWNLOADS: &str = "downloads";

/// Where a fetch landed.
#[derive(Debug, Clone)]
pub struct Fetched {
    /// The URL actually fetched, after a GitHub blob link was rewritten and
    /// after every redirect. Not always the URL that was asked for, which is
    /// why it is reported.
    pub url: String,
    pub path: PathBuf,
    pub bytes: usize,
}

/// Fetch `url` into `store_dir`'s downloads directory.
///
/// Returns where the file landed. Indexing it is the caller's job, because who
/// is allowed to write to the store depends on whether a daemon is running.
pub fn fetch(url: &str, store_dir: &Path) -> Result<Fetched> {
    // Before anything else, and before any socket: the whole point of the flag
    // is that the process can be shown never to have reached the network.
    if embed::airgap() {
        bail!(
            "`semlith add` fetches {url} over the network, which --airgap forbids; \
             download the file yourself and run `semlith index` on it"
        );
    }

    let url = rewrite(url);
    require_https(&url)?;

    let (final_url, content_type, body) = get(&url)?;
    let extension = extension_for(&content_type, &final_url, &body)?;
    let path = destination(store_dir, &final_url, &extension)?;

    let parent = path.parent().unwrap_or(store_dir);
    // The downloads directory sits inside the store, so it carries what the
    // store carries: the bytes of whatever was fetched.
    crate::home::secure_dir(parent)?;
    std::fs::write(&path, &body).with_context(|| format!("writing {}", path.display()))?;

    register_root(store_dir)?;

    Ok(Fetched {
        url: final_url,
        path,
        bytes: body.len(),
    })
}

/// The downloads directory of a store.
pub fn downloads_dir(store_dir: &Path) -> PathBuf {
    store_dir.join(DOWNLOADS)
}

/// A GitHub blob link rewritten to the file it displays.
///
/// `github.com/o/r/blob/main/src/lib.rs` is a page *about* a file: fetched, it
/// is a megabyte of application HTML with the source split across it one line
/// per element. The raw host serves the file itself, and it is the same URL
/// with two segments changed — so a developer pasting the link they were
/// looking at gets the file they meant rather than a page of markup.
fn rewrite(url: &str) -> String {
    let Some(rest) = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
    else {
        return url.to_string();
    };
    let parts: Vec<&str> = rest.splitn(4, '/').collect();
    match parts.as_slice() {
        [owner, repo, "blob", tail] => {
            format!("https://raw.githubusercontent.com/{owner}/{repo}/{tail}")
        }
        _ => url.to_string(),
    }
}

/// One plain-HTTP origin that may be fetched anyway, for tests.
///
/// `tests/add.rs` has to exercise the real fetch path — the redirect chain, the
/// size cap, the content-type check — against a server it controls, and a
/// fixture server that speaks TLS would mean shipping a certificate to test a
/// rule that has nothing to do with TLS. So one origin, named exactly, may be
/// plain HTTP; every other URL still has to be https, including every hop of a
/// redirect that starts at this one.
///
/// The same shape and the same reasoning as `SEMLITH_RELEASES_ORIGIN` in
/// `upgrade.rs`, and like it, deliberately not in `docs/compatibility.md`.
#[cfg(any(debug_assertions, feature = "dev-overrides"))]
const HTTP_ORIGIN_ENV: &str = "SEMLITH_ADD_ORIGIN";

/// HTTPS only, at every hop.
///
/// Plain HTTP is refused rather than upgraded: silently rewriting a URL a
/// person typed means the thing that was fetched is not the thing they asked
/// for, and for a tool whose claim is that its behaviour is checkable, that is
/// the wrong kind of helpful.
fn require_https(url: &str) -> Result<()> {
    // Credentials in a URL are refused rather than sent or stripped (audit
    // S22): `semlith add` never authenticates, and a user:pass kept in the
    // URL ended up in the stored path, where search results print it.
    let authority = url.split_once("://").map_or("", |(_, rest)| {
        rest.split(['/', '?', '#']).next().unwrap_or("")
    });
    if authority.contains('@') {
        bail!(
            "{} carries credentials; `semlith add` never sends any — download the \
             file yourself and run `semlith index` on it",
            url.split_once("://")
                .map_or(url, |(scheme, _)| scheme)
                .to_string()
                + "://…"
        );
    }
    if url.starts_with("https://") {
        return Ok(());
    }
    // A test seam, compiled out of the release binary (audit S20): a plain
    // http origin read from the environment is a downgrade a shell profile
    // could make.
    #[cfg(any(debug_assertions, feature = "dev-overrides"))]
    if let Some(origin) = std::env::var(HTTP_ORIGIN_ENV)
        .ok()
        .filter(|o| !o.is_empty())
        && url.starts_with(&origin)
    {
        return Ok(());
    }
    bail!("{url} is not https; `semlith add` fetches over https only")
}

/// Opt back in to fetching an address that is not on the public internet.
///
/// For a developer whose documentation is on an intranet host. Off by default,
/// because the default caller of this code is an agent holding a key from a
/// config file, and "fetch this URL" with a private address is how a tool that
/// runs on your machine becomes a way to read things only your machine can
/// reach — a metadata service, a router, a service bound to loopback.
pub const ALLOW_PRIVATE_ENV: &str = "SEMLITH_ADD_ALLOW_PRIVATE";

fn private_allowed() -> bool {
    std::env::var_os(ALLOW_PRIVATE_ENV).is_some_and(|v| v == "1" || v == "true")
}

/// Refuse a host that resolves to an address that is not on the public
/// internet.
///
/// Checked per hop rather than once, because a redirect is the whole trick: a
/// public host that answers 302 to `http://169.254.169.254/` is a public host
/// asking this process to read a cloud instance's credentials. Every address
/// the name resolves to has to pass, not just the first — a name that resolves
/// to a public address and a loopback one is a name that can be raced.
fn require_public(url: &str) -> Result<()> {
    if private_allowed() {
        return Ok(());
    }
    let Some(authority) = url
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or(""))
    else {
        bail!("{url} is not a URL semlith can resolve");
    };
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // `[::1]:8443`, `[::1]`, `host:8443` or `host`. An IPv6 literal is full of
    // colons, so its port is only what follows the closing bracket; before
    // 0.41.0 every IPv6 literal failed to parse here (audit S22).
    let (host, port) = match authority.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((h, tail)) => (h, tail.strip_prefix(':').unwrap_or("443")),
            None => bail!("{url} has an unclosed IPv6 address"),
        },
        None => match authority.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h, p),
            _ => (authority, "443"),
        },
    };
    let port: u16 = port.parse().unwrap_or(443);

    use std::net::ToSocketAddrs;
    let resolved: Vec<std::net::SocketAddr> = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("resolving {host}"))?
        .collect();
    if resolved.is_empty() {
        bail!("{host} resolves to nothing");
    }
    for address in resolved {
        if !is_public(&address.ip()) {
            bail!(
                "{host} resolves to {}, which is not on the public internet. \
                 `semlith add` fetches from the internet; set {ALLOW_PRIVATE_ENV}=1 \
                 if you meant to reach an address only this machine or this network \
                 can see.",
                address.ip()
            );
        }
    }
    Ok(())
}

/// Whether an address is one the public internet routes to.
///
/// Everything a cloud metadata service, a container network, a home router or
/// this machine itself sits on is refused. Written as a list of what is not
/// public rather than what is, because the not-public list is the one that is
/// closed.
fn is_public(ip: &std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                // Carrier-grade NAT, which a home router's own subnet often is.
                || (a == 100 && (64..128).contains(&b))
                // "This network", and the reserved block above the multicast
                // range that includes 255.255.255.255.
                || a == 0
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Unique local addresses, fc00::/7.
                || (v6.octets()[0] & 0xfe) == 0xfc
                // Link-local, fe80::/10.
                || (v6.octets()[0] == 0xfe && (v6.octets()[1] & 0xc0) == 0x80)
                // An IPv4 address wearing an IPv6 hat is judged as the IPv4
                // one: mapped (::ffff:0:0/96), compatible (::/96), NAT64
                // (64:ff9b::/96, 64:ff9b:1::/48) and 6to4 (2002::/16), each
                // of which a router or a translator turns back into it.
                || embedded_v4(v6).is_some_and(|v4| !is_public(&IpAddr::V4(v4)))
                // Teredo (2001::/32): the IPv4 server and client are in it
                // obfuscated, and nothing honest is fetched over it.
                || v6.segments()[..2] == [0x2001, 0])
        }
    }
}

/// The IPv4 address an IPv6 one carries, for the forms that carry one.
fn embedded_v4(v6: &std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let o = v6.octets();
    let s = v6.segments();
    let last = std::net::Ipv4Addr::new(o[12], o[13], o[14], o[15]);
    if let Some(v4) = v6.to_ipv4_mapped() {
        return Some(v4);
    }
    if s[..6] == [0; 6] && !v6.is_loopback() && !v6.is_unspecified() {
        return Some(last);
    }
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] || s[..3] == [0x64, 0xff9b, 1] {
        return Some(last);
    }
    if s[0] == 0x2002 {
        return Some(std::net::Ipv4Addr::new(o[2], o[3], o[4], o[5]));
    }
    None
}

/// The connection's own resolver, refusing what [`require_public`] refuses.
///
/// The check above resolves the name and the connection resolved it again,
/// so a name answering a public address first and a private one second
/// passed the check and reached the private one (audit S22). Here the
/// addresses the connection uses are the addresses checked; the certificate
/// is still checked against the name, so TLS is unchanged.
///
/// Wraps the resolver it checks rather than naming the system one, so a test
/// can stand in a name that answers public to the check and loopback after
/// (audit SK30, 2026-10-17, which read the code from before S22).
#[derive(Debug)]
struct PublicOnly<R>(R);

impl<R: ureq::unversioned::resolver::Resolver> ureq::unversioned::resolver::Resolver
    for PublicOnly<R>
{
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> std::result::Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let addrs = self.0.resolve(uri, config, timeout)?;
        if !private_allowed()
            && let Some(bad) = addrs.iter().find(|a| !is_public(&a.ip()))
        {
            return Err(ureq::Error::Io(std::io::Error::other(format!(
                "{} resolves to {}, which is not on the public internet",
                uri.host().unwrap_or("the host"),
                bad.ip()
            ))));
        }
        Ok(addrs)
    }
}

/// The agent `semlith add` fetches with, connecting only to what `resolver`
/// answers and [`PublicOnly`] passed.
fn agent(resolver: impl ureq::unversioned::resolver::Resolver) -> ureq::Agent {
    // Redirects are followed by [`get`] rather than by the agent, because the
    // check that matters is per hop: an https URL that redirects to http is
    // exactly the case an agent's own redirect following would hide.
    // Deadlines, as every outbound agent in semlith has: a server that accepts
    // the connection and then drips a byte a minute held this call, and before
    // 0.40.0 every other agent's call with it.
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(10)))
        .timeout_global(Some(std::time::Duration::from_secs(60)))
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .build();
    ureq::Agent::with_parts(
        config,
        ureq::unversioned::transport::DefaultConnector::default(),
        PublicOnly(resolver),
    )
}

/// Follow the chain to the document, returning where it ended, what the server
/// said it was, and its bytes.
fn get(url: &str) -> Result<(String, String, Vec<u8>)> {
    let agent = agent(ureq::unversioned::resolver::DefaultResolver::default());

    let mut current = url.to_string();
    require_public(&current)?;
    for _ in 0..=MAX_REDIRECTS {
        note_outbound("add", &current);
        let mut response = agent
            .get(&current)
            .call()
            .with_context(|| format!("fetching {current}"))?;

        let status = response.status().as_u16();
        if (300..400).contains(&status) {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.to_string())
                .with_context(|| format!("{current} redirected without saying where"))?;
            current = absolute(&current, &location);
            require_https(&current)?;
            require_public(&current)?;
            continue;
        }
        if status == 401 || status == 403 {
            bail!(
                "{current} needs credentials ({status}); `semlith add` never sends any — \
                 download the file yourself and run `semlith index` on it"
            );
        }
        if status >= 400 {
            bail!("{current} answered {status}");
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();

        // One byte past the cap, so a body that fills it exactly is
        // distinguishable from one that ran over.
        let mut body = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(MAX_BYTES + 1)
            .read_to_end(&mut body)
            .with_context(|| format!("reading {current}"))?;
        if body.len() as u64 > MAX_BYTES {
            bail!(
                "{current} is larger than the {} MiB `semlith add` will fetch",
                MAX_BYTES / 1024 / 1024
            );
        }
        if body.is_empty() {
            bail!("{current} returned nothing");
        }

        return Ok((current, content_type, body));
    }

    bail!("{url} redirected more than {MAX_REDIRECTS} times")
}

/// A `Location` resolved against the URL it came from.
///
/// Servers send all three forms — absolute, host-relative and path-relative —
/// and a redirect that is not resolved simply fails to be a URL.
fn absolute(from: &str, location: &str) -> String {
    if location.contains("://") {
        return location.to_string();
    }
    let after_scheme = from.split_once("://").map(|(_, rest)| rest).unwrap_or(from);
    let host_end = after_scheme.find('/').unwrap_or(after_scheme.len());
    let origin = &from[..from.len() - after_scheme.len() + host_end];

    if let Some(rooted) = location.strip_prefix('/') {
        return format!("{origin}/{rooted}");
    }
    let directory = from
        .rsplit_once('/')
        .map(|(head, _)| head)
        .unwrap_or(origin);
    format!("{directory}/{location}")
}

/// The extension to save the document under, which is what decides the reader
/// that will read it.
///
/// The declared type is cross-checked against the first bytes, because a server
/// that labels a PDF `text/html` would otherwise have its file handed to the
/// tag scanner and come out as nothing. A type no reader handles is refused
/// here rather than written and silently skipped at index time.
fn extension_for(content_type: &str, url: &str, body: &[u8]) -> Result<String> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    // The bytes win over the label. A PDF says so in its first five.
    if body.starts_with(b"%PDF-") {
        return Ok("pdf".to_string());
    }
    if mime == "application/pdf" {
        bail!("{url} is labelled a PDF but does not start with %PDF-");
    }

    match mime.as_str() {
        "text/html" | "application/xhtml+xml" => Ok("html".to_string()),
        // A text type semlith already has a reader for keeps the extension the
        // URL gave it, so a `.rs` file is indexed as Rust rather than as `.txt`
        // and `--lang rust` still finds it.
        _ if mime.starts_with("text/") || mime == "application/json" => {
            Ok(url_extension(url).unwrap_or_else(|| "txt".to_string()))
        }
        "" => bail!("{url} did not say what it is"),
        other => bail!(
            "{url} is {other}, which semlith has no reader for; \
             `semlith add` fetches pages, PDFs and text files"
        ),
    }
}

/// The extension the URL's own path carries, when it has one.
fn url_extension(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.rsplit('/').next()?;
    let ext = last.rsplit_once('.')?.1;
    looks_like_extension(ext).then(|| ext.to_ascii_lowercase())
}

/// Whether the tail after a dot is a file extension rather than part of the
/// name.
///
/// Letters and short. The digits rule is what keeps `2501.00001` — an arXiv
/// identifier, and the whole name of the paper — from being read as a file
/// called `2501`.
fn looks_like_extension(tail: &str) -> bool {
    !tail.is_empty()
        && tail.len() <= 8
        && tail.chars().all(|c| c.is_ascii_alphanumeric())
        && tail.chars().any(|c| c.is_ascii_alphabetic())
}

/// Where in the store the document is written.
///
/// Laid out by host so a store that has collected fifty pages is still
/// navigable, and so two sites' `index.html` are two files. The path is built
/// from the URL, which makes it attacker-influenced input to a filesystem
/// write: every segment is sanitised, and the result is checked to be inside
/// the downloads directory before anything is written.
fn destination(store_dir: &Path, url: &str, extension: &str) -> Result<PathBuf> {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let (host, path) = match after_scheme.split_once('/') {
        Some((host, path)) => (host, path),
        None => (after_scheme, ""),
    };
    let path = path.split(['?', '#']).next().unwrap_or("");

    // Decoded before it is split, not after. `..%2f..%2fetc` is one segment
    // until the escapes are undone, and a sanitiser that ran first would see a
    // single odd-looking name and pass it through whole.
    let path = crate::formats::unpercent(path);

    let root = downloads_dir(store_dir);
    let mut target = root.join(safe(host));

    let mut segments: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .map(safe)
        .collect();
    // A trailing slash means every segment was a directory, so the last of them
    // is not the document's name. Popping it would file `/docs/` as `docs.html`
    // and put it beside the directory it is the index of.
    let last = if path.ends_with('/') {
        None
    } else {
        segments.pop()
    };
    for segment in segments {
        target.push(segment);
    }

    let stem = match last {
        // A URL ending in `/` names a directory's index and has no filename of
        // its own.
        None => "index".to_string(),
        // The URL's own extension is dropped only when it is one. An arXiv id
        // ends in `.00001`, and cutting there would name the file `2501`.
        Some(name) => match name.rsplit_once('.') {
            Some((stem, tail)) if !stem.is_empty() && looks_like_extension(tail) => {
                stem.to_string()
            }
            _ => name,
        },
    };

    // The check that makes the sanitising provable rather than assumed. `safe`
    // already drops every separator and every `..`, so this can only fail if it
    // has a hole in it — which is exactly when it matters.
    let candidate = target.join(format!("{stem}.{extension}"));
    if !candidate.starts_with(&root) {
        bail!("{url} resolves outside the store's downloads directory");
    }

    Ok(unused(&target, &stem, extension))
}

/// One path segment with everything that is not a plain name removed.
///
/// Separators, `..`, `.`, drive letters, control characters and the characters
/// Windows refuses in a filename all go. What is left cannot leave the
/// directory it is joined to.
fn safe(segment: &str) -> String {
    let cleaned: String = segment
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim_matches('.').trim();
    if cleaned.is_empty() {
        return "file".to_string();
    }
    // Long enough for any real name, short enough that the whole path stays
    // inside the limits every filesystem has.
    cleaned.chars().take(80).collect()
}

/// The first name in `dir` that is not taken.
///
/// Fetching the same URL twice writes a second file rather than replacing the
/// first. Overwriting would be the surprising behaviour: the earlier copy is
/// already indexed, and a page that changed is the reason somebody fetched it
/// again.
fn unused(dir: &Path, stem: &str, extension: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{extension}"));
    if !first.exists() {
        return first;
    }
    for n in 2..1000 {
        let candidate = dir.join(format!("{stem}-{n}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

/// Record the downloads directory as a root of this store.
///
/// Without it `semlith start` would not watch the directory, and a store's own
/// downloads would be the one part of it that goes stale. A store that is not
/// in the registry — a `./.semlith` beside a corpus, or one named with
/// `--store` — has no entry to add to, which is not an error: the file is
/// indexed either way.
fn register_root(store_dir: &Path) -> Result<()> {
    let mut registry = home::Registry::load()?;
    let Some(name) = registry.name_of(store_dir).map(str::to_string) else {
        return Ok(());
    };
    let root = downloads_dir(store_dir);
    let Some(entry) = registry.stores.get_mut(&name) else {
        return Ok(());
    };
    if entry.roots.contains(&root) {
        return Ok(());
    }
    entry.roots.push(root);
    registry.save()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name that answers a public address the first time it is asked and
    /// this machine's loopback every time after: DNS rebinding.
    #[derive(Debug)]
    struct Rebinding {
        asked: std::sync::atomic::AtomicUsize,
        port: u16,
    }

    impl ureq::unversioned::resolver::Resolver for Rebinding {
        fn resolve(
            &self,
            _uri: &ureq::http::Uri,
            _config: &ureq::config::Config,
            _timeout: ureq::unversioned::transport::NextTimeout,
        ) -> std::result::Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error>
        {
            let first = self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
            let ip: std::net::IpAddr = if first {
                [93, 184, 215, 14]
            } else {
                [127, 0, 0, 1]
            }
            .into();
            let mut addrs = self.empty();
            addrs.push(std::net::SocketAddr::new(ip, self.port));
            Ok(addrs)
        }
    }

    /// Audit SK30 (2026-10-17): the address checked as public is the address
    /// connected to, so a rebinding name never reaches loopback. Closed by
    /// 0.41.0's S22 resolver; this pins it. The same name through an agent
    /// without [`PublicOnly`] does reach the listener, so the check below is
    /// not vacuous.
    #[test]
    fn sk30_a_rebinding_name_never_reaches_loopback() {
        use ureq::unversioned::resolver::Resolver;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let rebinding = || Rebinding {
            asked: Default::default(),
            port,
        };
        let uri: ureq::http::Uri = format!("https://rebind.test:{port}/doc.md")
            .parse()
            .unwrap();
        let config = ureq::Agent::config_builder().build();
        let timeout = ureq::unversioned::transport::NextTimeout {
            after: ureq::unversioned::transport::time::Duration::NotHappening,
            reason: ureq::Timeout::Resolve,
        };
        let connected = |listener: &std::net::TcpListener| {
            std::thread::sleep(std::time::Duration::from_millis(200));
            listener.accept().is_ok()
        };

        // Unguarded, the second answer is where the request goes.
        let plain = rebinding();
        let checked = plain.resolve(&uri, &config, timeout).unwrap();
        assert!(is_public(&checked[0].ip()));
        let unguarded = ureq::Agent::with_parts(
            ureq::Agent::config_builder()
                .timeout_global(Some(std::time::Duration::from_secs(2)))
                .build(),
            ureq::unversioned::transport::DefaultConnector::default(),
            plain,
        );
        let _ = unguarded.get(&uri.to_string()).call();
        assert!(connected(&listener), "the control never reached loopback");

        // Guarded: the connection's own answer is checked, and refused.
        let guarded = rebinding();
        let checked = guarded.resolve(&uri, &config, timeout).unwrap();
        assert!(is_public(&checked[0].ip()));
        let err = agent(guarded).get(&uri.to_string()).call().unwrap_err();
        assert!(
            err.to_string().contains("not on the public internet"),
            "{err}"
        );
        assert!(!connected(&listener), "a rebinding name reached loopback");
    }

    /// Audit S22: an IPv4 address inside an IPv6 one is judged as itself, an
    /// IPv6 literal parses, and credentials in a URL are refused.
    #[test]
    fn every_ipv6_spelling_of_a_private_address_is_refused() {
        for private in [
            "::ffff:127.0.0.1",
            "::127.0.0.1",
            "64:ff9b::a9fe:a9fe",
            "64:ff9b:1::a00:1",
            "2002:c0a8:0101::1",
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "fd00::1",
            "fe80::1",
            "::1",
        ] {
            let ip: std::net::IpAddr = private.parse().unwrap();
            assert!(!is_public(&ip), "{private} passed as public");
        }
        for public in [
            "2606:4700:4700::1111",
            "64:ff9b::808:808",
            "2002:0808:0808::1",
        ] {
            let ip: std::net::IpAddr = public.parse().unwrap();
            assert!(is_public(&ip), "{public} refused as private");
        }
        assert!(require_https("https://user:pw@example.com/x").is_err());
        assert!(require_https("https://example.com/a@b").is_ok());
    }

    #[test]
    fn an_ipv6_literal_is_resolved_not_misparsed() {
        if private_allowed() {
            return;
        }
        let err = require_public("https://[::1]:8443/x")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not on the public internet"), "{err}");
        let err = require_public("https://[::1]/x").unwrap_err().to_string();
        assert!(err.contains("not on the public internet"), "{err}");
    }

    #[test]
    fn a_github_blob_link_is_rewritten_to_the_file_it_shows() {
        assert_eq!(
            rewrite("https://github.com/semlith/semlith/blob/main/src/lib.rs"),
            "https://raw.githubusercontent.com/semlith/semlith/main/src/lib.rs"
        );
        // A tag or a commit is the same shape and is rewritten the same way.
        assert_eq!(
            rewrite("https://github.com/o/r/blob/v1.2.3/docs/a b.md"),
            "https://raw.githubusercontent.com/o/r/v1.2.3/docs/a b.md"
        );
        // Everything else on the host is left alone: an issue is a page, and a
        // page is what the HTML reader is for.
        for untouched in [
            "https://github.com/semlith/semlith/issues/41",
            "https://github.com/semlith/semlith",
            "https://example.com/blob/main/x.rs",
        ] {
            assert_eq!(rewrite(untouched), untouched);
        }
    }

    #[test]
    fn a_location_header_resolves_in_all_three_forms() {
        let from = "https://a.example/docs/guide.html";
        assert_eq!(absolute(from, "https://b.example/x"), "https://b.example/x");
        assert_eq!(absolute(from, "/x/y"), "https://a.example/x/y");
        assert_eq!(
            absolute(from, "other.html"),
            "https://a.example/docs/other.html"
        );
    }

    #[test]
    fn the_bytes_decide_the_reader_and_an_unknown_type_is_refused() {
        // Labelled HTML, actually a PDF. The label loses.
        assert_eq!(
            extension_for("text/html", "https://x.example/p", b"%PDF-1.7 ...").unwrap(),
            "pdf"
        );
        // Labelled a PDF and is not one, which is the case that would otherwise
        // hand a page of markup to the PDF reader.
        assert!(extension_for("application/pdf", "https://x.example/p", b"<html>").is_err());

        assert_eq!(
            extension_for("text/html; charset=utf-8", "https://x.example/p", b"<p>").unwrap(),
            "html"
        );
        // A text type keeps the extension the URL gave it, so `--lang rust`
        // still finds what was fetched.
        assert_eq!(
            extension_for("text/plain", "https://x.example/a/lib.rs", b"fn main() {}").unwrap(),
            "rs"
        );
        assert_eq!(
            extension_for("text/plain", "https://x.example/a/notes", b"hello").unwrap(),
            "txt"
        );

        for refused in ["image/png", "application/zip", "video/mp4", ""] {
            assert!(
                extension_for(refused, "https://x.example/p", b"\x89PNG").is_err(),
                "{refused:?} was accepted"
            );
        }
    }

    #[test]
    fn a_url_cannot_write_outside_the_downloads_directory() {
        let store = std::path::Path::new("/tmp/store");
        let root = downloads_dir(store);

        for hostile in [
            "https://x.example/../../../../etc/passwd",
            "https://x.example/..%2f..%2fetc/passwd",
            "https://../../etc/passwd",
            "https://x.example//etc/passwd",
            "https://x.example/a/../../../b",
            "https://x.example/%2e%2e/%2e%2e/etc/passwd",
        ] {
            let path = destination(store, hostile, "html").unwrap();
            assert!(
                path.starts_with(&root),
                "{hostile} escaped to {}",
                path.display()
            );
            assert!(
                !path.to_string_lossy().contains(".."),
                "{hostile} kept a .. in {}",
                path.display()
            );
        }
    }

    #[test]
    fn a_document_is_laid_out_by_host_and_keeps_its_name() {
        let store = std::path::Path::new("/tmp/store");
        let path = destination(store, "https://arxiv.org/pdf/2501.00001", "pdf").unwrap();
        assert!(
            path.ends_with("downloads/arxiv.org/pdf/2501.00001.pdf"),
            "{path:?}"
        );

        // A URL with no filename of its own names the directory's index.
        let path = destination(store, "https://example.com/docs/", "html").unwrap();
        assert!(
            path.ends_with("downloads/example.com/docs/index.html"),
            "{path:?}"
        );

        // The query string is not part of the name.
        let path = destination(store, "https://example.com/p?a=1&b=2", "html").unwrap();
        assert!(path.ends_with("downloads/example.com/p.html"), "{path:?}");
    }

    #[test]
    fn plain_http_is_refused_rather_than_upgraded() {
        assert!(require_https("http://example.com/x").is_err());
        assert!(require_https("ftp://example.com/x").is_err());
        assert!(require_https("file:///etc/passwd").is_err());
        assert!(require_https("https://example.com/x").is_ok());
    }
}
