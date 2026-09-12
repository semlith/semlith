//! The daemon's routes: thin adapters over what the CLI and the MCP tools
//! already do.
//!
//! Nothing here decides anything. A search route builds the same [`Filter`] the
//! `--path`/`--ext`/`--lang` flags build and calls the same
//! [`Fleet::search_in`] the `semlith_search` tool calls; an index route puts a
//! job on the store's queue and streams what the writer reports. That is the
//! point: two implementations of "what does this filter select" is how a
//! portal ends up disagreeing with the terminal about what is indexed.
//!
//! Every route is reachable only after [`crate::http`] has checked the token
//! and the `Host` header, so nothing below re-checks either.

use crate::daemon::{self, State, Store};
use crate::filter::{Filter, LANGUAGES};
use crate::http::{Handler, Request, Response};
use crate::{chunk, embed, home, store};
use fastembed::TextEmbedding;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// How many files one Files request returns before it starts counting instead.
/// A corpus is tens of thousands of files and a browser table is not.
const FILE_PAGE: usize = 500;

pub fn handler(state: Arc<State>) -> Handler {
    Arc::new(move |request| route(&state, request))
}

fn route(state: &Arc<State>, request: &Request) -> Response {
    let path = request.path.as_str();
    let get = request.method == "GET";
    let post = request.method == "POST";

    if get && let Some((kind, bytes)) = crate::portal::asset(path) {
        return Response::asset(kind, bytes);
    }

    match (get, post, path) {
        (true, _, "/") => crate::portal::page(),

        (true, _, "/api/stores") => stores(state),
        (true, _, "/api/files") => files(state, request),
        (true, _, "/api/search") => search(state, request),
        (true, _, "/api/models") => models(),
        (true, _, "/api/languages") => languages(),
        (true, _, "/api/dirs") => dirs(request),
        (true, _, "/api/privacy") => privacy(state),
        (true, _, "/api/about") => about(state),
        (true, _, "/api/agents") => agents(state),
        (true, _, "/api/setup") => setup(),

        (_, true, "/api/index") => index(state, request),
        (_, true, "/api/add") => add(state, request),
        (_, true, "/api/forget") => forget(state, request),
        (_, true, "/api/adopt") => adopt(state, request),
        (_, true, "/api/rotate") => rotate(state),
        (_, true, "/api/mcp") => mcp(state, request),
        (_, true, "/api/upgrade") => upgrade(request),

        // A method that exists on another verb is worth telling apart from a
        // route that does not exist: one is a bug in the page, the other is a
        // typed URL.
        (_, _, p) if p.starts_with("/api/") => Response::error(405, "wrong method for this route"),
        _ => Response::error(404, "no such route"),
    }
}

// ---------------------------------------------------------------- reads

/// Every open store, what it holds, and whether it is being kept current.
fn stores(state: &Arc<State>) -> Response {
    let mut fleet = state.fleet.lock().expect("the fleet lock");
    let mut out = Vec::new();

    for handle in &state.stores {
        let stats = fleet
            .as_mut()
            .and_then(|f| {
                f.each()
                    .find(|(_, s)| s.dir() == handle.dir)
                    .map(|(_, s)| s)
            })
            .map(|s| {
                (
                    s.stats(),
                    s.model().to_string(),
                    s.dim(),
                    s.len(),
                    s.shards(),
                )
            });

        let (files, chunks, bytes, model, dim, vectors, shards) = match stats {
            Some((Ok((f, c, b)), model, dim, len, shards)) => (f, c, b, model, dim, len, shards),
            _ => (0, 0, 0, String::new(), 0, 0, None),
        };

        out.push(json!({
            "name": handle.name,
            "dir": handle.dir.display().to_string(),
            // Told apart so the portal can show a root that is not there as a
            // problem rather than silently listing one fewer.
            "roots": handle.roots.iter().map(|r| json!({
                "path": r.display().to_string(),
                "present": r.exists(),
            })).collect::<Vec<_>>(),
            "files": files,
            "chunks": chunks,
            "bytes": bytes,
            "model": model,
            "dim": dim,
            "vectors": vectors,
            "shards": shards.map(|(n, max)| json!({ "count": n, "resident": max })),
            "watching": handle.watching.load(Ordering::Relaxed),
            "queue": handle.queue_depth(),
            "last_write": handle.last_write.load(Ordering::Relaxed),
            "events": handle.events(),
        }));
    }

    Response::json(&json!({ "stores": out }))
}

/// The indexed files, with the filters the CLI has and the reader that parsed
/// each one.
fn files(state: &Arc<State>, request: &Request) -> Response {
    let filter = match filter_of(request) {
        Ok(f) => f,
        Err(e) => return Response::error(400, &e),
    };
    let only = request.query_all("store");

    let mut fleet = state.fleet.lock().expect("the fleet lock");
    let Some(fleet) = fleet.as_mut() else {
        return Response::json(&json!({ "files": [], "total": 0 }));
    };

    let mut rows = Vec::new();
    let mut total = 0usize;
    for (label, opened) in fleet.each() {
        if !only.is_empty() && !only.iter().any(|n| n == label) {
            continue;
        }
        let listed = match store::file_rows(opened.db(), filter.groups()) {
            Ok(r) => r,
            Err(e) => return Response::error(500, &e.to_string()),
        };
        total += listed.len();
        for (path, bytes, chunks, lines) in listed {
            if rows.len() >= FILE_PAGE {
                continue;
            }
            let as_path = Path::new(&path);
            rows.push(json!({
                "store": label,
                "path": path,
                "ext": as_path.extension().and_then(|e| e.to_str()).unwrap_or(""),
                "lang": language_of(as_path),
                "reader": chunk::reader_of(as_path),
                "bytes": bytes,
                "chunks": chunks,
                "lines": lines,
            }));
        }
    }

    // A truncated list that does not say it is truncated is how somebody
    // concludes a file was never indexed.
    Response::json(&json!({ "files": rows, "total": total, "page": FILE_PAGE }))
}

/// The same fused search the CLI and `semlith_search` run.
fn search(state: &Arc<State>, request: &Request) -> Response {
    let Some(query) = request.query("query").filter(|q| !q.trim().is_empty()) else {
        return Response::error(400, "missing query");
    };
    let k = request
        .query("k")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(8)
        .clamp(1, 100);
    let filter = match filter_of(request) {
        Ok(f) => f,
        Err(e) => return Response::error(400, &e),
    };
    let only = request.query_all("store");

    let mut fleet = state.fleet.lock().expect("the fleet lock");
    let Some(fleet) = fleet.as_mut() else {
        return Response::json(&json!({ "hits": [], "selected": 0 }));
    };

    // Asked before the search, exactly as the CLI asks it: a filter that
    // selects nothing is a different answer from a corpus that does not
    // discuss the query, and only one of them is the user's typo.
    let selected = if filter.is_empty() {
        None
    } else {
        match fleet.matching_files(&filter) {
            Ok(n) => Some(n),
            Err(e) => return Response::error(500, &e.to_string()),
        }
    };
    if selected == Some(0) {
        return Response::json(&json!({ "hits": [], "selected": 0, "chunks": fleet.chunks() }));
    }

    let started = std::time::Instant::now();
    let only = (!only.is_empty()).then_some(only);
    let hits = match fleet.search_in(only.as_deref(), query, k, &filter) {
        Ok(h) => h,
        Err(e) => return Response::error(500, &e.to_string()),
    };
    let elapsed = started.elapsed();

    // The `Hit` shape `--json` already prints, plus the store name — which for
    // a single-store fleet the CLI leaves out and the portal always wants,
    // because the portal always has the possibility of more than one.
    let single = fleet.len() == 1;
    let label = fleet.labels().first().map(|s| s.to_string());
    let out: Vec<Value> = hits
        .iter()
        .map(|h| {
            json!({
                "score": h.score,
                "path": h.path,
                "start_line": h.start_line,
                "end_line": h.end_line,
                "text": h.text,
                "store": h.store.clone().or_else(|| single.then(|| label.clone()).flatten()),
            })
        })
        .collect();

    Response::json(&json!({
        "hits": out,
        "selected": selected,
        "chunks": fleet.chunks(),
        "micros": elapsed.as_micros() as u64,
    }))
}

fn models() -> Response {
    let mut out = vec![json!({
        "name": embed::GRANITE_NAME,
        "dim": 384,
        "description": "default. IBM Granite R2 small, int8, English",
    })];
    for info in TextEmbedding::list_supported_models() {
        out.push(json!({
            "name": info.model.to_string(),
            "dim": info.dim,
            "description": info.description,
        }));
    }
    Response::json(&json!({ "models": out }))
}

fn languages() -> Response {
    let out: Vec<Value> = LANGUAGES
        .iter()
        .map(|entry| {
            json!({
                "name": entry.name,
                "extensions": entry.extensions,
                "filenames": entry.filenames,
            })
        })
        .collect();
    Response::json(&json!({ "languages": out }))
}

/// Children of a directory, for the Index view's picker.
///
/// Confined to the user's home and never following a symlink out of it. The
/// browser is asking a local server to list a filesystem, and the answer to
/// "which directories may it list" has to be a rule rather than a hope.
fn dirs(request: &Request) -> Response {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let home = crate::canonical(&home);

    let asked = request
        .query("path")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.clone());
    // Canonicalized first, so `..` and a symlink are both resolved before the
    // containment check rather than after it.
    let resolved = crate::canonical(&asked);
    if !resolved.starts_with(&home) {
        return Response::error(400, "outside the home directory");
    }

    let mut entries = Vec::new();
    let listing = match std::fs::read_dir(&resolved) {
        Ok(l) => l,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    for entry in listing.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        entries.push(json!({
            "name": name,
            "path": entry.path().display().to_string(),
            "dir": is_dir,
        }));
    }
    entries.sort_by(|a, b| {
        let (ad, bd) = (a["dir"].as_bool(), b["dir"].as_bool());
        bd.cmp(&ad).then_with(|| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        })
    });

    Response::json(&json!({
        "path": resolved.display().to_string(),
        "home": home.display().to_string(),
        "parent": resolved
            .parent()
            .filter(|p| p.starts_with(&home))
            .map(|p| p.display().to_string()),
        "entries": entries,
    }))
}

/// What the Privacy page checks: where this process listens, whether it may
/// fetch anything at all, and where the one download it can make is cached.
fn privacy(state: &Arc<State>) -> Response {
    let cache = crate::model_cache_dir();
    Response::json(&json!({
        "bind": format!("127.0.0.1:{}", state.server.port()),
        "bind_is_fixed": true,
        "airgap": state.airgap,
        "model_cache": cache.display().to_string(),
        "model_cached": cache.exists()
            && std::fs::read_dir(&cache).map(|mut d| d.next().is_some()).unwrap_or(false),
        "token_cookie": crate::http::TOKEN_COOKIE,
        "host_allowed": ["localhost", "127.0.0.1", "::1"],
        "csp": "default-src 'self'",
        "cors": false,
        "store_home": home::home().display().to_string(),
    }))
}

fn about(state: &Arc<State>) -> Response {
    Response::json(&json!({
        "version": env!("CARGO_PKG_VERSION"),
        "binary": std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        "port": state.server.port(),
        "pid": std::process::id(),
        "uptime": daemon::uptime(state),
        "store_home": home::home().display().to_string(),
        "model_cache": crate::model_cache_dir().display().to_string(),
        "models": TextEmbedding::list_supported_models().len() + 1,
        "languages": LANGUAGES.len(),
        "stores": state.stores.len(),
    }))
}

/// MCP status, and the stanza for every client the README documents.
fn agents(state: &Arc<State>) -> Response {
    Response::json(&json!({
        "forwarding": state.proxy_count() > 0,
        "connected": state.proxy_count(),
        "tools": [
            "semlith_search",
            "semlith_stats",
            "semlith_files",
            "semlith_index",
            "semlith_add",
            "semlith_forget",
        ],
        "revisions": crate::mcp::SUPPORTED,
        "clients": crate::clients::clients(),
        "install": {
            "sh": crate::setup::INSTALL_SH,
            "ps1": crate::setup::INSTALL_PS1,
        },
        "setup": crate::setup::status(),
    }))
}

/// The same per-step answers `semlith setup` computes, from the same function,
/// so the page and the terminal cannot disagree about what is set up. It is a
/// read: nothing here installs anything.
fn setup() -> Response {
    Response::json(&json!(crate::setup::status()))
}

// ---------------------------------------------------------------- writes

/// Index one or more paths, streaming progress as newline-delimited JSON.
fn index(state: &Arc<State>, request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let paths: Vec<PathBuf> = strings(&body, "path")
        .into_iter()
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        return Response::error(400, "no path given");
    }
    let store = match state.writable(body.get("store").and_then(Value::as_str)) {
        Ok(s) => Arc::clone(s),
        Err(e) => return Response::error(409, &e.to_string()),
    };

    match state.index(&store, paths) {
        Ok(progress) => stream(progress),
        Err(e) => Response::error(409, &e.to_string()),
    }
}

/// Fetch one URL into the store and index what landed, streaming the same
/// progress the folder picker streams.
///
/// The fetch runs here rather than inside the write queue. It is network work,
/// and holding a store's queue open for the length of a download would stall
/// every other write behind it — while the indexing of what landed, which is
/// the part the one-writer rule is about, still goes through the queue.
fn add(state: &Arc<State>, request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let Some(url) = body
        .get("url")
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())
    else {
        return Response::error(400, "no url given");
    };
    let store = match state.writable(body.get("store").and_then(Value::as_str)) {
        Ok(s) => Arc::clone(s),
        Err(e) => return Response::error(409, &e.to_string()),
    };

    // A refused fetch is an answer the page shows, not a 500. Every one of them
    // names something the person can act on: the URL was http, or too large, or
    // a type nothing here can read.
    let fetched = match crate::add::fetch(url, &store.dir) {
        Ok(fetched) => fetched,
        Err(e) => return Response::error(400, &format!("{e:#}")),
    };

    match state.index(&store, vec![fetched.path.clone()]) {
        Ok(progress) => stream(progress),
        Err(e) => Response::error(409, &e.to_string()),
    }
}

/// Check for a newer release, or install one. Both only on a click — the
/// daemon never looks on its own, which is the rule in AGENTS.md and the reason
/// this is a POST with an explicit action rather than something the page can
/// trigger by being open.
///
/// An applied upgrade replaces the binary this daemon is running from. The
/// rename-old, rename-new sequence leaves the running process on the old inode,
/// so the answer says to restart rather than pretending the swap took effect.
fn upgrade(request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    match body.get("action").and_then(Value::as_str) {
        Some("check") => match crate::upgrade::check() {
            Ok(found) => Response::json(&json!(found)),
            Err(e) => Response::error(502, &e.to_string()),
        },
        Some("apply") => match crate::upgrade::apply(
            body.get("version")
                .and_then(Value::as_str)
                .map(String::from),
        ) {
            Ok(()) => Response::json(&json!({
                "installed": true,
                "restart": "semlith start is still running the old binary — restart it",
            })),
            Err(e) => Response::error(409, &e.to_string()),
        },
        _ => Response::error(400, "action must be \"check\" or \"apply\""),
    }
}

fn forget(state: &Arc<State>, request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let Some(path) = body.get("path").and_then(Value::as_str) else {
        return Response::error(400, "no path given");
    };
    let store = match state.writable(body.get("store").and_then(Value::as_str)) {
        Ok(s) => Arc::clone(s),
        Err(e) => return Response::error(409, &e.to_string()),
    };

    // Not streamed: forgetting a file is one statement, and a client that has
    // to parse a stream to learn a number is a client doing extra work.
    let progress = match state.forget(&store, PathBuf::from(path)) {
        Ok(p) => p,
        Err(e) => return Response::error(409, &e.to_string()),
    };
    match progress.recv() {
        Ok(value) => Response::json(&value),
        Err(_) => Response::error(500, "the writer stopped before answering"),
    }
}

/// The welcome screen's "Adopt an existing .semlith", running the same code
/// path the CLI `adopt` runs.
fn adopt(state: &Arc<State>, request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let Some(dir) = body.get("path").and_then(Value::as_str) else {
        return Response::error(400, "no path given");
    };
    let dir = PathBuf::from(dir);
    // A store this daemon holds the lock on cannot be moved out from under
    // itself, and the error for that should say so rather than be an IO error
    // halfway through a rename.
    if state.stores.iter().any(|s| s.dir == crate::canonical(&dir)) {
        return Response::error(409, "this daemon is already using that store");
    }
    match home::adopt(&dir, None, None) {
        Ok((name, target)) => Response::json(&json!({
            "name": name,
            "dir": target.display().to_string(),
            // The daemon opened its stores at startup and holds their locks,
            // so a store adopted now joins on the next start. Said plainly
            // rather than left for the user to notice it is not in the list.
            "restart_required": true,
        })),
        Err(e) => Response::error(400, &e.to_string()),
    }
}

/// One forwarded JSON-RPC request, answered by the same code the stdio server
/// runs — which is what makes every supported protocol revision behave the
/// same through the proxy as it does in process.
fn mcp(state: &Arc<State>, request: &Request) -> Response {
    if let Some(pid) = crate::proxy::proxy_pid(request.header("semlith-proxy")) {
        state.saw_proxy(pid);
    }
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    if let Err(e) = state.open_mcp_fleet() {
        return Response::error(409, &e.to_string());
    }

    let writer = daemon::Writer(Arc::clone(state));
    let mut fleet = state.mcp_fleet.lock().expect("the mcp fleet lock");
    let Some(fleet) = fleet.as_mut() else {
        return Response::error(409, "this daemon has no store open");
    };

    match crate::mcp::answer(fleet, Some(&writer), &body) {
        Some(value) => Response::json(&value),
        // A notification. Answered with an empty 200 rather than an empty JSON
        // object, so the proxy writes nothing to a client that expects nothing.
        None => Response::new(200, "application/json; charset=utf-8", Vec::new()),
    }
}

fn rotate(state: &Arc<State>) -> Response {
    let fresh = state.rotate();
    // Returned once, and set as the cookie in the same response, so the page
    // that asked keeps working and the old token stops.
    Response::json(&json!({ "token": fresh, "url": daemon::url(state) })).with_token(&fresh)
}

// ---------------------------------------------------------------- helpers

/// Turn a queue receiver into a chunked NDJSON body.
fn stream(progress: std::sync::mpsc::Receiver<Value>) -> Response {
    Response::stream(move |chunks| {
        for event in progress {
            let mut line = event.to_string();
            line.push('\n');
            chunks.send(&line)?;
        }
        Ok(())
    })
}

/// The same `Filter` the `--path`/`--ext`/`--lang` flags build.
fn filter_of(request: &Request) -> Result<Filter, String> {
    Filter::new(
        &request.query_all("path"),
        &request.query_all("ext"),
        &request.query_all("lang"),
    )
    .map_err(|e| e.to_string())
}

/// A JSON field that is a string or an array of strings, as the MCP tools
/// already accept, because an agent and a page make the same mistake.
fn strings(value: &Value, key: &str) -> Vec<String> {
    match value.get(key) {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// Which `--lang` name covers this file, if any.
///
/// The extension answers for almost every file, and the filename answers for
/// the ones that have no extension. Both are read out of the one [`LANGUAGES`]
/// table the filter uses, so the Files view can never disagree with what
/// `--lang` would actually have matched.
fn language_of(path: &Path) -> &'static str {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    LANGUAGES
        .iter()
        .find(|entry| {
            (!ext.is_empty() && entry.extensions.contains(&ext.as_str()))
                || entry
                    .filenames
                    .iter()
                    .any(|pattern| matches(pattern, &name))
        })
        .map(|entry| entry.name)
        .unwrap_or("")
}

/// The one glob shape [`crate::filter::Language::filenames`] uses: a literal
/// name, optionally ending in `*`.
///
/// Written out rather than reached for through SQLite because this runs per
/// file in a listing of five hundred, and because the patterns are ours rather
/// than a user's — a full glob engine here would be capability nobody can call.
fn matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == pattern,
    }
}

/// The one place a route needs to know a [`Store`] by name for a test.
#[allow(dead_code)]
fn named<'a>(state: &'a Arc<State>, name: &str) -> Option<&'a Arc<Store>> {
    state.store(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_labelled_with_the_reader_that_would_parse_it() {
        assert_eq!(chunk::reader_of(Path::new("a/b.docx")), "word");
        assert_eq!(chunk::reader_of(Path::new("a/b.ipynb")), "notebook");
        assert_eq!(chunk::reader_of(Path::new("a/b.rs")), "text");
        assert_eq!(chunk::reader_of(Path::new("a/b.PDF")), "pdf");
    }

    #[test]
    fn a_file_is_labelled_with_the_language_lang_would_select() {
        assert_eq!(language_of(Path::new("src/lib.rs")), "rust");
        assert_eq!(language_of(Path::new("a/b.MD")), "markdown");
        assert_eq!(language_of(Path::new("a/b.unknown")), "");
        assert_eq!(language_of(Path::new("LICENSE")), "");
    }

    /// The MCP tools take a bare string where the schema says array, because
    /// agents send both. A page does too, so one reader serves them.
    #[test]
    fn a_path_is_taken_as_a_string_or_an_array() {
        assert_eq!(strings(&json!({ "path": "a" }), "path"), vec!["a"]);
        assert_eq!(
            strings(&json!({ "path": ["a", "b"] }), "path"),
            vec!["a", "b"]
        );
        assert!(strings(&json!({}), "path").is_empty());
    }
}
