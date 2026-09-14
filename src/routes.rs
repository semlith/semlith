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

/// The largest page the Files route will serve. A corpus is tens of thousands
/// of files and a browser table is not; the portal asks for 8 to 50 and the
/// cap is what stops a hand-written query asking for all of them.
const FILE_PAGE: i64 = 500;

/// How deep the Files route will page.
///
/// The cost of an offset is paid before the page is cut: every open store is
/// asked for `offset + limit` rows in the requested order and the merge picks
/// the page out of the union, so the offset is what is allocated rather than
/// what is returned.
const FILE_OFFSET_MAX: i64 = 10_000;

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

        // A route that panics, so the containment in `http::answer` is proved
        // against a running daemon rather than against a unit test's handler.
        // `debug_assertions` is off in a release build, so the binary a user
        // installs does not have it: this is a test fixture that happens to
        // live beside the routes it tests, not a hidden endpoint.
        #[cfg(debug_assertions)]
        (true, _, "/api/panic") => panic!("the route that exists to panic, panicking"),

        (true, _, "/api/stores") => stores(state),
        (true, _, "/api/files") => files(state, request),
        (true, _, "/api/search") => search(state, request),
        (true, _, "/api/models") => models(),
        (true, _, "/api/languages") => languages(),
        (true, _, "/api/dirs") => dirs(request),
        (true, _, "/api/privacy") => privacy(state),
        (true, _, "/api/about") => about(state),
        (true, _, "/api/agents") => agents(state),
        (true, _, "/api/symbol") => symbol(state, request),
        (true, _, "/api/neighbors") => neighbors(state, request),
        (true, _, "/api/path") => shortest_path(state, request),
        (true, _, "/api/graph") => graph(state, request),
        (true, _, "/api/ledger") => ledger(state),
        (true, _, "/api/image") => image_file(state, request),
        (true, _, "/api/setup") => setup(),

        (_, true, "/api/index") => index(state, request),
        (_, true, "/api/add") => add(state, request),
        (_, true, "/api/forget") => forget(state, request),
        (_, true, "/api/adopt") => adopt(state, request),
        (_, true, "/api/trust") => trust(state, request),
        (_, true, "/api/agents/reveal") => reveal(state),
        (_, true, "/api/rotate") => rotate(state),
        (_, true, "/api/mcp") => mcp(state, request),
        // MCP over HTTP, on the path a client's configuration names. The
        // credential is checked in `http`, which accepts the agent key here
        // and nowhere else.
        (_, true, crate::http::MCP_PATH) => mcp(state, request),
        (_, true, "/api/endpoint") => endpoint(state, request),
        (_, true, "/api/key") => key(state, request),
        (_, true, "/api/setup") => fix_setup(request),
        (_, true, "/api/root") => root(state, request),
        (_, true, "/api/store/delete") => delete_store(state, request),
        (_, true, "/api/index/control") => index_control(state, request),
        (_, true, "/api/upgrade") => upgrade(request),

        // A route that exists on another verb is worth telling apart from one
        // that does not exist at all: the first is a bug in the page, the
        // second is a typed or stale URL. Only the write routes are listed,
        // because a GET of one of them is the case that actually happens — a
        // browser replaying a URL — and a path that is on no list at all, like
        // a route a later release removed, is a 404 rather than an invitation
        // to try another verb.
        (
            _,
            _,
            "/api/index" | "/api/add" | "/api/forget" | "/api/adopt" | "/api/rotate" | "/api/mcp"
            | "/api/upgrade",
        ) => Response::error(405, "wrong method for this route"),
        _ => Response::error(404, "no such route"),
    }
}

// ---------------------------------------------------------------- reads

/// Every open store, what it holds, and whether it is being kept current.
fn stores(state: &Arc<State>) -> Response {
    // Opened here rather than at startup: it is None until the first read, and
    // None again after a store joins, so every read route has to be able to
    // put it back. Cheap when it is already open.
    if let Err(e) = state.open_fleet() {
        return Response::error(500, &e.to_string());
    }
    let mut fleet = state.fleet.lock().unwrap_or_else(|e| e.into_inner());
    let registry = home::Registry::load().unwrap_or_default();
    let mut out = Vec::new();

    for handle in state.stores() {
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
                    // What the store spans, which `stats` cannot answer: the
                    // Stores page states lines of code and how many file types
                    // and readers they run to, and a page that counts its own
                    // rows to get there is describing the page.
                    store::file_facets(s.db(), &[]).unwrap_or_default(),
                )
            });

        let (files, chunks, bytes, model, dim, vectors, shards, facets) = match stats {
            Some((Ok((f, c, b)), model, dim, len, shards, facets)) => {
                (f, c, b, model, dim, len, shards, facets)
            }
            _ => (0, 0, 0, String::new(), 0, 0, None, store::Facets::default()),
        };

        // The reader is a property of the code rather than a column, so it is
        // derived from the extensions the store actually holds rather than
        // stored a second time beside them.
        let mut readers: Vec<&'static str> = facets
            .extensions
            .iter()
            .map(|ext| chunk::reader_of(Path::new(&format!("x.{ext}"))))
            .collect();
        readers.sort_unstable();
        readers.dedup();

        out.push(json!({
            "name": handle.name,
            "dir": handle.dir.display().to_string(),
            // A store made before 0.14.0 is readable by everyone on the machine
            // until it is opened by this binary, and one somebody chmod'ed is
            // readable until the next open too. Reported rather than silently
            // fixed, so the row says what happened.
            "loose_mode": home::loose_mode(&handle.dir).map(|m| format!("{m:o}")),
            // A store this daemon was pointed at explicitly — `semlith start
            // /some/path` — is open without having been trusted, which is
            // correct for an explicit instruction and worth saying, because the
            // next `semlith mcp` in that directory will refuse it.
            "trusted": registry.trusts(&handle.dir),
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
            "lines": facets.lines,
            "formats": facets.extensions.len(),
            "readers": readers.len(),
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

    // Before anything is opened. An offset of four billion asks every open
    // store for four billion rows in sorted order and merges them, which is a
    // whole machine's memory for a page nobody is reading. The portal pages in
    // fifteens; ten thousand is deeper than any table it draws, and a refusal
    // past it is a clearer answer than a clamp that silently shows page one.
    let offset = request
        .query("offset")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    if !(0..=FILE_OFFSET_MAX).contains(&offset) {
        return Response::error(
            400,
            &format!("offset must be between 0 and {FILE_OFFSET_MAX}"),
        );
    }

    if let Err(e) = state.open_fleet() {
        return Response::error(500, &e.to_string());
    }
    let mut fleet = state.fleet.lock().unwrap_or_else(|e| e.into_inner());
    let Some(fleet) = fleet.as_mut() else {
        return Response::json(&json!({ "files": [], "total": 0 }));
    };

    let sort = request
        .query("sort")
        .and_then(store::FileSort::parse)
        .unwrap_or(store::FileSort::Path);
    let desc = request.query("dir") == Some("desc");
    let limit = request
        .query("limit")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(15)
        .clamp(1, FILE_PAGE);

    // Each store is asked for the first `offset + limit` rows in the requested
    // order and the merge picks the page out of the union. Asking each store
    // for the page directly would be wrong the moment two stores are open: the
    // tenth row overall is not the tenth row of either of them.
    let take = offset.saturating_add(limit);
    let mut merged: Vec<(String, store::FileRow)> = Vec::new();
    let mut total = 0i64;
    let mut extensions: Vec<String> = Vec::new();
    let mut stores = 0usize;
    for (label, opened) in fleet.each() {
        if !only.is_empty() && !only.iter().any(|n| n == label) {
            continue;
        }
        stores += 1;
        match store::file_count(opened.db(), filter.groups()) {
            Ok(count) => total += count,
            Err(e) => return Response::error(500, &e.to_string()),
        }
        match store::file_facets(opened.db(), filter.groups()) {
            Ok(facets) => extensions.extend(facets.extensions),
            Err(e) => return Response::error(500, &e.to_string()),
        }
        let listed = match store::file_rows(opened.db(), filter.groups(), sort, desc, take) {
            Ok(r) => r,
            Err(e) => return Response::error(500, &e.to_string()),
        };
        merged.extend(listed.into_iter().map(|row| (label.to_string(), row)));
    }

    merged.sort_by(|(_, a), (_, b)| {
        let order = match sort {
            store::FileSort::Path => a.path.cmp(&b.path),
            store::FileSort::Bytes => a.bytes.cmp(&b.bytes),
            store::FileSort::Chunks => a.chunks.cmp(&b.chunks),
            store::FileSort::Lines => a.lines.cmp(&b.lines),
            store::FileSort::Indexed => a.indexed_at.cmp(&b.indexed_at),
        };
        let order = if desc { order.reverse() } else { order };
        order.then_with(|| a.path.cmp(&b.path))
    });

    let rows: Vec<Value> = merged
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .map(|(label, row)| {
            let as_path = Path::new(&row.path);
            json!({
                "store": label,
                "path": row.path,
                "ext": as_path.extension().and_then(|e| e.to_str()).unwrap_or(""),
                "lang": language_of(as_path),
                "reader": chunk::reader_of(as_path),
                "bytes": row.bytes,
                "chunks": row.chunks,
                "lines": row.lines,
                "indexed_at": row.indexed_at,
            })
        })
        .collect();

    extensions.sort();
    extensions.dedup();

    Response::json(&json!({
        "files": rows,
        "total": total,
        "offset": offset,
        "limit": limit,
        "sort": request.query("sort").unwrap_or("path"),
        "dir": if desc { "desc" } else { "asc" },
        // The header states what the whole listing spans, not what the page
        // happens to hold: "9 formats" read off fifteen rows is a number about
        // the table rather than about the corpus.
        "formats": extensions.len(),
        "stores": stores,
    }))
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

    // Before anything is opened. An offset of four billion asks every open
    // store for four billion rows in sorted order and merges them, which is a
    // whole machine's memory for a page nobody is reading. The portal pages in
    // fifteens; ten thousand is deeper than any table it draws, and a refusal
    // past it is a clearer answer than a clamp that silently shows page one.
    let offset = request
        .query("offset")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);
    if !(0..=FILE_OFFSET_MAX).contains(&offset) {
        return Response::error(
            400,
            &format!("offset must be between 0 and {FILE_OFFSET_MAX}"),
        );
    }

    if let Err(e) = state.open_fleet() {
        return Response::error(500, &e.to_string());
    }
    let mut fleet = state.fleet.lock().unwrap_or_else(|e| e.into_inner());
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
    // `format=locate` leaves the text out, for a caller paying for it. The
    // portal asks for it on the list and fetches the body when a row is
    // opened, which is the two-stage shape its Search page is built around.
    let locate = request
        .query("format")
        .is_some_and(|f| f.eq_ignore_ascii_case("locate"));
    let out: Vec<Value> = hits
        .iter()
        .map(|h| {
            json!({
                "score": h.score,
                "path": h.path,
                "start_line": h.start_line,
                "end_line": h.end_line,
                "text": if locate { String::new() } else { h.text.clone() },
                "store": h.store.clone().or_else(|| single.then(|| label.clone()).flatten()),
                "lists": h.lists,
                "image": h.image.map(|px| json!({ "width": px.width, "height": px.height })),
                "fresh": h.fresh,
                "symbol": h.symbol,
                "symbol_kind": h.symbol_kind,
                "provenance": h.provenance,
            })
        })
        .collect();

    if state.ledger {
        record(fleet, "portal", query, &hits, elapsed);
    }

    Response::json(&json!({
        "hits": out,
        "selected": selected,
        "chunks": fleet.chunks(),
        "micros": elapsed.as_micros() as u64,
    }))
}

/// Write one retrieval into every store that answered.
///
/// Tokens are estimated at four characters each. That is a rough rule and it is
/// said to be one on the page: what matters for the ratio is that both sides
/// are measured the same way, not that either is exact.
fn record(
    fleet: &crate::fleet::Fleet,
    client: &str,
    query: &str,
    hits: &[crate::Hit],
    elapsed: std::time::Duration,
) {
    const CHARS_PER_TOKEN: i64 = 4;
    let excerpt: i64 = hits.iter().map(|h| h.text.len() as i64).sum::<i64>() / CHARS_PER_TOKEN;

    for (label, store) in fleet.each() {
        // Only the stores this answer actually came from, so a search across
        // three stores does not write three identical rows.
        let mine: Vec<&crate::Hit> = hits
            .iter()
            .filter(|h| h.store.as_deref().is_none_or(|s| s == label))
            .collect();
        if mine.is_empty() {
            continue;
        }
        // What reading those files whole would have cost, which is the honest
        // denominator: the ratio is measured against a real alternative.
        let paths: std::collections::BTreeSet<&str> =
            mine.iter().map(|h| h.path.as_str()).collect();
        let whole: i64 = paths
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len() as i64 / CHARS_PER_TOKEN)
            .sum();
        let _ = store::record_retrieval(
            store.db(),
            client,
            query,
            mine.len() as i64,
            elapsed.as_micros() as i64,
            excerpt,
            whole,
        );
    }
}

/// The free half of the ledger: whether it is recording, and the totals.
fn ledger(state: &Arc<State>) -> Response {
    let empty = json!({
        "recording": state.ledger,
        "queries": 0,
        "clients": 0,
        "excerpt_tokens": 0,
        "whole_file_tokens": 0,
        "ratio": null,
        "intact": true,
    });
    let recording = state.ledger;
    with_fleet(state, empty, move |fleet| {
        let (mut queries, mut clients, mut excerpt, mut whole) = (0, 0, 0, 0);
        let mut intact = true;
        for (_, store) in fleet.each() {
            let (q, c, e, w) = store::ledger_totals(store.db())?;
            queries += q;
            clients = clients.max(c);
            excerpt += e;
            whole += w;
            intact = intact && store::ledger_break(store.db())?.is_none();
        }
        let ratio = (excerpt > 0).then(|| whole as f64 / excerpt as f64);
        Ok(json!({
            "recording": recording,
            "queries": queries,
            "clients": clients,
            "excerpt_tokens": excerpt,
            "whole_file_tokens": whole,
            "ratio": ratio,
            "intact": intact,
        }))
    })
}

fn models() -> Response {
    let mut out = vec![
        json!({
            "name": embed::GRANITE_NAME,
            "dim": 384,
            "description": "default. IBM Granite R2 small, int8, English",
        }),
        // Not a choice, so it is listed apart from the models a store can be
        // built with: both halves of it are loaded together, on the first
        // image a store indexes, and nothing selects them.
        json!({
            "name": crate::image::MODEL_NAME,
            "dim": crate::image::DIM,
            "description": "images. CLIP ViT-B/32, vision and text, fixed",
            "code": "Qdrant/clip-ViT-B-32-vision",
        }),
    ];
    for info in TextEmbedding::list_supported_models() {
        out.push(json!({
            "name": info.model.to_string(),
            "dim": info.dim,
            "description": info.description,
            "code": info.model_code,
        }));
    }

    // Size is reported for a model this machine has actually downloaded, and
    // left blank for one it has not. fastembed's catalogue carries no size, and
    // a number copied from a model card is a claim about somebody else's file.
    let cached = cached_model_sizes();
    for model in &mut out {
        let code = model.get("code").and_then(Value::as_str).unwrap_or("");
        let key = code.rsplit('/').next().unwrap_or(code).to_ascii_lowercase();
        if let Some(bytes) = cached.get(&key) {
            model["bytes"] = json!(bytes);
        }
    }

    Response::json(&json!({ "models": out }))
}

/// How many bytes each model in the cache occupies, by its directory name.
fn cached_model_sizes() -> std::collections::HashMap<String, u64> {
    let mut out = std::collections::HashMap::new();
    let Ok(entries) = std::fs::read_dir(crate::model_cache_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let total = walk_bytes(&entry.path());
        if total > 0 {
            // fastembed names a cache directory `models--<org>--<model>`.
            let key = name.rsplit("--").next().unwrap_or(&name).to_string();
            *out.entry(key).or_insert(0) += total;
        }
    }
    out
}

fn walk_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => walk_bytes(&entry.path()),
            Ok(_) => entry.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

/// One indexed image, served to the Search page's preview.
///
/// Bounded to files the store has actually indexed as images: the path arrives
/// from the page, and a route that read whatever path it was handed would be a
/// file-read primitive behind a loopback port. The `images` table is the
/// allowlist, so the only files this can serve are ones the user pointed
/// semlith at.
/// The largest image this route will read into memory.
///
/// A preview in a search result. The indexer's own cap is larger, and a file
/// past this one is a file the page should not be trying to draw anyway.
const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;

fn image_file(state: &Arc<State>, request: &Request) -> Response {
    let Some(want) = request.query("path") else {
        return Response::error(400, "missing path");
    };
    // Resolved before it is looked up, and the resolved path is what is looked
    // up. The store records canonical paths, so a name that now points
    // somewhere else — an indexed `diagram.png` replaced by a symlink to
    // `/etc/shadow` — resolves to a path the store has never heard of and is a
    // 404 rather than a read. Checking the name and then opening it is the
    // gap: the two are not the same file if anything moved in between.
    let path = crate::canonical(Path::new(want));
    let looked_up = path.to_string_lossy().into_owned();

    if let Err(e) = state.open_fleet() {
        return Response::error(500, &e.to_string());
    }
    let mut fleet = state.fleet.lock().unwrap_or_else(|e| e.into_inner());
    let Some(fleet) = fleet.as_mut() else {
        return Response::error(404, "no such image");
    };

    let indexed = fleet.each().any(|(_, store)| {
        store
            .db()
            .query_row(
                "SELECT 1 FROM images i JOIN files f ON f.id = i.file_id WHERE f.path = ?1",
                rusqlite::params![&looked_up],
                |_| Ok(()),
            )
            .is_ok()
    });
    if !indexed {
        return Response::error(404, "no such image");
    }

    // Opened once, and everything decided from the open handle: what is read is
    // what was opened, whatever the name points at by the time it is read.
    let Ok(file) = std::fs::File::open(&path) else {
        // Indexed once and gone since: the row is real and the file is not.
        return Response::error(404, "the file is no longer on disk");
    };
    let Ok(meta) = file.metadata() else {
        return Response::error(404, "the file is no longer on disk");
    };
    if !meta.is_file() {
        return Response::error(404, "no such image");
    }
    if meta.len() > MAX_IMAGE_BYTES {
        return Response::error(413, "the image is larger than the portal will draw");
    }
    use std::io::Read;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    if file.take(MAX_IMAGE_BYTES).read_to_end(&mut bytes).is_err() {
        return Response::error(404, "the file could not be read");
    }

    let path = path.as_path();
    let kind = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => return Response::error(404, "no such image"),
    };
    Response::new(200, kind, bytes)
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
    // containment check rather than after it — and through `std::fs` directly
    // rather than through the helper, which returns the path unchanged when it
    // cannot resolve one. A path that does not resolve is a path this route
    // cannot say anything about, so it is refused rather than checked as
    // written.
    let Ok(resolved) = std::fs::canonicalize(&asked) else {
        return Response::error(400, "that path cannot be resolved");
    };
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
        "token_header": crate::http::TOKEN_HEADER,
        // Enough of the token to recognise the one this browser holds, and
        // not enough to be one. The full value reaches the page once, in the
        // printed URL, and once more in the body of the rotate response — it is
        // in no other response, and it is in no cookie at all.
        "token_preview": preview(&state.server.token()),
        // How long a rotated agent key still works, if one still does. Shown
        // as a countdown rather than as "until this daemon exits", which on a
        // machine somebody leaves running is not a grace period at all.
        "key_grace_seconds": state.server.key_grace().map(|left| left.as_secs()),
        "host_allowed": ["localhost", "127.0.0.1", "::1"],
        "csp": "default-src 'self'",
        "cors": false,
        "store_home": home::home().display().to_string(),
        // One row per rule this release added, each with what the daemon found
        // when it looked — not a list of claims. A page that states a policy is
        // a page; a page that states a policy and the reading behind it is
        // something a reader can disagree with.
        "rules": rules(state),
    }))
}

/// Every rule 0.14.0 added, and the daemon's own check of it.
///
/// `ok` is what was measured, `check` is what was measured *about*, so a row
/// that fails says which thing on this machine is not as the rule describes.
/// Rules that hold by construction — the ones enforced in `http::answer` before
/// any handler runs — report what the code does rather than a reading, and say
/// so in `check`.
fn rules(state: &Arc<State>) -> Value {
    let home_dir = home::home();
    let cache = crate::model_cache_dir();
    let key_path = home::agent_key_path();
    let registry = home::Registry::load().unwrap_or_default();

    let store_modes: Vec<String> = state
        .stores()
        .iter()
        .filter_map(|s| home::loose_mode(&s.dir).map(|m| format!("{} is {m:o}", s.name)))
        .collect();

    // Annotated: on Windows both arms of this are `None`, and `None` on its own
    // tells the compiler nothing — the comparisons below then have two `PartialEq`
    // impls to choose between, one of them serde_json's. A Windows-only type
    // error, which is exactly the kind the matrix exists to find.
    let key_mode: Option<u32> = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&key_path)
                .map(|m| m.permissions().mode() & 0o777)
                .ok()
        }
        #[cfg(not(unix))]
        {
            None
        }
    };

    json!([
        {
            "id": "header-borne token",
            "rule": format!(
                "The session token travels in a {} header, never in a cookie. Every port                  on localhost is the same site, so a cookie would reach a page served by                  anything else on this machine.",
                crate::http::TOKEN_HEADER
            ),
            "check": "no route sets a cookie and none reads one",
            "ok": true,
        },
        {
            "id": "same-origin writes",
            "rule": "Every request that is not a GET carries Sec-Fetch-Site: same-origin —                      or, from a client that sends no fetch metadata, an Origin that matches                      or none at all — and a JSON content type. Both are checked before the                      token, so a cross-origin page cannot tell a right guess from a wrong                      one.",
            "check": "enforced in http::answer before any route runs",
            "ok": true,
        },
        {
            "id": "store trust",
            "rule": "A store outside the store home is opened only after semlith trust                      has recorded it. A .semlith directory can arrive inside a repository.",
            "check": if registry.trusted.is_empty() {
                "no store outside the home is trusted".to_string()
            } else {
                format!("{} trusted outside the home", registry.trusted.len())
            },
            "ok": true,
        },
        {
            "id": "index boundary",
            "rule": "An agent holding the key indexes only under this store's registered                      roots or the home directory. The command line is not held to this —                      the person typing it owns the machine.",
            "check": "enforced per path in Semlith::index_set",
            "ok": true,
        },
        {
            "id": "deny-list",
            "rule": "No path under a credential directory, and no file named like a                      credential, is indexed by an agent or the portal — with the                      hidden-file rule applied to an explicitly named path too.",
            "check": format!(
                "{} directories and {} name patterns",
                crate::filter::DENIED_DIRS.len(),
                crate::filter::DENIED_NAMES.len()
            ),
            "ok": true,
        },
        {
            "id": "private addresses",
            "rule": "semlith add resolves every hop and refuses an address that is not                      on the public internet: loopback, RFC 1918, link-local,                      carrier-grade NAT, unique local.",
            "check": if std::env::var_os(crate::add::ALLOW_PRIVATE_ENV).is_some() {
                format!("{} is set, so private addresses are allowed", crate::add::ALLOW_PRIVATE_ENV)
            } else {
                "on".to_string()
            },
            "ok": std::env::var_os(crate::add::ALLOW_PRIVATE_ENV).is_none(),
        },
        {
            "id": "pinned models",
            "rule": "Every model file is fetched at a pinned commit and verified against a                      digest recorded in the source and in docs/models.md. The weights are                      what computes every vector in every store.",
            "check": format!("granite at {}", &crate::embed::GRANITE_REVISION[..12]),
            "ok": true,
        },
        {
            "id": "model cache",
            "rule": "Weights are not loaded from a cache another account owns or can write                      to.",
            "check": match crate::embed::check_cache_dir(&cache) {
                Ok(()) => format!("{} is yours alone", cache.display()),
                Err(e) => e.to_string(),
            },
            "ok": crate::embed::check_cache_dir(&cache).is_ok(),
        },
        {
            "id": "directory modes",
            "rule": "The store home, every store, the model cache and daemon.json are                      readable by their owner and nobody else.",
            "check": if store_modes.is_empty() {
                match home::loose_mode(&home_dir) {
                    Some(mode) => format!("{} is {mode:o}", home_dir.display()),
                    None => format!("{} and every open store are 0700", home_dir.display()),
                }
            } else {
                store_modes.join(", ")
            },
            "ok": store_modes.is_empty() && home::loose_mode(&home_dir).is_none(),
        },
        {
            "id": "agent key",
            "rule": "The key is in one file, readable by you alone. No client                      configuration carries it and no command line shows it: every stanza                      names ${SEMLITH_AGENT_KEY}.",
            "check": match key_mode {
                Some(mode) if mode & 0o077 == 0 => format!("{} is {mode:o}", key_path.display()),
                Some(mode) => format!("{} is {mode:o}", key_path.display()),
                None => format!("{} has no mode to read", key_path.display()),
            },
            "ok": key_mode.is_none_or(|mode| mode & 0o077 == 0),
        },
    ])
}

/// The first sixteen characters of a secret, and an ellipsis.
fn preview(secret: &str) -> String {
    let head: String = secret.chars().take(16).collect();
    if head.len() < secret.len() {
        format!("{head}…")
    } else {
        head
    }
}

fn about(state: &Arc<State>) -> Response {
    let binary = std::env::current_exe().unwrap_or_default();
    Response::json(&json!({
        "version": env!("CARGO_PKG_VERSION"),
        "format_version": store::FORMAT_VERSION,
        "binary": binary.display().to_string(),
        // Measured rather than stated: the size of the file this process was
        // started from.
        "binary_bytes": std::fs::metadata(&binary).map(|m| m.len()).unwrap_or(0),
        "target": format!("{} · {}", std::env::consts::ARCH, std::env::consts::OS),
        "bind": format!("127.0.0.1:{}", state.server.port()),
        "revisions": crate::mcp::SUPPORTED,
        "port": state.server.port(),
        "pid": std::process::id(),
        "uptime": daemon::uptime(state),
        "store_home": home::home().display().to_string(),
        "model_cache": crate::model_cache_dir().display().to_string(),
        "models": TextEmbedding::list_supported_models().len() + 1,
        "languages": LANGUAGES.len(),
        // Which of those languages carry graph edges. The rest are searchable
        // exactly as before and simply have no symbols, which is a different
        // thing from being unsupported.
        "graph_languages": crate::graph::LANGUAGES,
        "edge_kinds": crate::graph::KINDS,
        "stores": state.stores().len(),
    }))
}

/// MCP status, and the stanza for every client the README documents.
///
/// Deliberately without `setup::status()`, which used to be embedded here.
/// That function shells out to `claude mcp list` and waits for it, so carrying
/// it made the Agents page block on a subprocess for a payload the page never
/// read — it asks `/api/setup` separately, and renders that panel when the
/// answer arrives rather than holding the whole page for it.
/// The agent key itself, once, because somebody pressed Reveal.
///
/// A POST rather than a GET: it is not something a page should receive for
/// merely being open, and the same-origin and JSON rules every write carries
/// apply to it. The key is on this machine already, in a file this user owns —
/// what this route changes is that reading it is a deliberate act rather than
/// part of rendering a page.
fn reveal(state: &Arc<State>) -> Response {
    Response::json(&json!({ "key": state.server.agent_key() }))
}

fn agents(state: &Arc<State>) -> Response {
    let connections = state.clients();
    let key = state.server.agent_key();
    Response::json(&json!({
        "forwarding": state.proxy_count() > 0,
        "connected": connections.len(),
        // Every client heard from recently, whichever transport it arrived on.
        "connections": connections,
        // Read from the MCP server's own definitions rather than repeated
        // here: a second copy is how a tool ends up served and invisible, and
        // a second description is how it ends up documented as something else.
        "tools": crate::mcp::tool_list()
            .into_iter()
            .map(|(name, about)| json!({ "name": name, "about": about }))
            .collect::<Vec<_>>(),
        "revisions": crate::mcp::SUPPORTED,
        "clients": crate::clients::clients(),
        "endpoint": {
            "url": format!("http://127.0.0.1:{}{}", state.server.port(), crate::http::MCP_PATH),
            "open": state.server.mcp_open(),
        },
        // Not the key. This route is read on every visit to the Agents page,
        // so the credential that opens the MCP endpoint used to be in a
        // response the page had done nothing to ask for. The page shows the
        // variable form, which is what a client stanza should carry anyway, and
        // `POST /api/agents/reveal` hands over the real value once, when
        // somebody presses the button.
        "key_env": crate::setup::KEY_ENV,
        // Not even a preview. A preview of an agent key still begins `sml_`,
        // and this route is read on every visit to the page — the point is that
        // nothing about the credential arrives unasked.
        "key_set": home::is_agent_key(&key),
        "key_path": home::agent_key_path().display().to_string(),
        "install": {
            "sh": crate::setup::INSTALL_SH,
            "ps1": crate::setup::INSTALL_PS1,
        },
    }))
}

/// The same per-step answers `semlith setup` computes, from the same function,
/// so the page and the terminal cannot disagree about what is set up. It is a
/// read: nothing here installs anything.
fn setup() -> Response {
    Response::json(&json!(crate::setup::status()))
}

/// Perform one setup step the page can see is not done.
///
/// A panel that reports "not on PATH" and then tells the reader to go and run a
/// command is a panel that found the problem and declined to fix it. Only the
/// steps that are safe without a prompt are here: PATH edits a shell rc file
/// this tool owns a marked block in, and nothing else is offered.
fn fix_setup(request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    match body.get("step").and_then(Value::as_str) {
        Some("path") => match crate::setup::run_path_step() {
            Ok(step) => Response::json(&json!({ "step": step, "status": crate::setup::status() })),
            Err(e) => Response::error(500, &e.to_string()),
        },
        Some(other) => Response::error(400, &format!("{other} is not a step this route runs")),
        None => Response::error(400, "missing step"),
    }
}

/// Point a registered store at another root.
///
/// For a corpus that moved: the registry still names the old directory and the
/// portal shows the root as missing, so the fix is offered where the problem is
/// visible. It rewrites the registry and nothing else — the store's vectors,
/// chunks and graph are untouched, and the watcher picks the new root up on the
/// next daemon start.
fn root(state: &Arc<State>, request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let (Some(store), Some(path)) = (
        body.get("store").and_then(Value::as_str),
        body.get("root").and_then(Value::as_str),
    ) else {
        return Response::error(400, "missing store or root");
    };
    match home::repoint(store, Path::new(path)) {
        Ok(()) => Response::json(&json!({
            "store": store,
            "root": path,
            "message": format!(
                "{store} now covers {path}. It is watched from the next start of \
                 the daemon; nothing was re-embedded."
            ),
            "restart": state.stores().iter().any(|s| s.name == store),
        })),
        Err(e) => Response::error(409, &e.to_string()),
    }
}

// ----------------------------------------------------------------- graph

/// Run a read against the open fleet, or answer with `empty` when no store is
/// open yet. Every graph route has the same three lines in front of it.
fn with_fleet(
    state: &Arc<State>,
    empty: Value,
    read: impl FnOnce(&crate::fleet::Fleet) -> anyhow::Result<Value>,
) -> Response {
    if let Err(e) = state.open_fleet() {
        return Response::error(500, &e.to_string());
    }
    let fleet = state.fleet.lock().unwrap_or_else(|e| e.into_inner());
    let Some(fleet) = fleet.as_ref() else {
        return Response::json(&empty);
    };
    match read(fleet) {
        Ok(value) => Response::json(&value),
        Err(e) => Response::error(500, &e.to_string()),
    }
}

fn symbol(state: &Arc<State>, request: &Request) -> Response {
    let Some(name) = request.query("name").filter(|n| !n.trim().is_empty()) else {
        return Response::error(400, "missing name");
    };
    let name = name.to_string();
    let k = request
        .query("k")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(20)
        .clamp(1, 200);
    let only = request.query_all("store");
    with_fleet(state, json!({ "symbols": [] }), move |fleet| {
        let only = (!only.is_empty()).then_some(only);
        let found = fleet.symbols_in(only.as_deref(), &name, k)?;
        Ok(json!({ "symbols": found }))
    })
}

fn neighbors(state: &Arc<State>, request: &Request) -> Response {
    let Some(name) = request.query("name").filter(|n| !n.trim().is_empty()) else {
        return Response::error(400, "missing name");
    };
    let name = name.to_string();
    let kinds = request.query_all("kind");
    if let Some(bad) = kinds
        .iter()
        .find(|k| !crate::graph::KINDS.contains(&k.as_str()))
    {
        return Response::error(400, &format!("unknown edge kind {bad:?}"));
    }
    let only = request.query_all("store");
    let all = request
        .query("all")
        .is_some_and(|v| v == "1" || v == "true");
    let empty = json!({ "callers": [], "callees": [] });
    with_fleet(state, empty, move |fleet| {
        let only = (!only.is_empty()).then_some(only);
        Ok(json!(fleet.neighbours_in(
            only.as_deref(),
            &name,
            &kinds,
            all
        )?))
    })
}

fn shortest_path(state: &Arc<State>, request: &Request) -> Response {
    let (Some(from), Some(to)) = (request.query("from"), request.query("to")) else {
        return Response::error(400, "missing from or to");
    };
    let (from, to) = (from.to_string(), to.to_string());
    let depth = request
        .query("depth")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(6)
        .clamp(1, 20);
    let only = request.query_all("store");
    // Resolved edges only unless the caller asks otherwise, the same default
    // the CLI and the MCP tool take. The portal's Graph page sends
    // `all_edges=1` behind its own control and labels what comes back.
    let all_edges = request
        .query("all_edges")
        .is_some_and(|v| v == "1" || v == "true");
    with_fleet(state, json!({ "path": null }), move |fleet| {
        let only = (!only.is_empty()).then_some(only);
        let chain = fleet.path_in(only.as_deref(), &from, &to, depth, all_edges)?;
        // `path` keeps its name and its shape — an array of steps or null —
        // so a 0.14.0 reader of this endpoint still finds what it looks for.
        // The counts arrive beside it rather than inside it.
        Ok(json!({
            "path": chain.as_ref().map(|c| &c.steps),
            "summary": chain.as_ref().map(|c| &c.summary),
            "all_edges": all_edges,
        }))
    })
}

/// The nodes and edges the Graph page draws.
///
/// Scoped to a store, to a directory, or to one symbol and its neighbourhood —
/// never the whole corpus, because a force layout over a monorepo is neither
/// drawable nor readable. The payload is a documented shape rather than
/// whatever the renderer happened to want: `{ nodes, edges, total, shown }`,
/// so another tool can draw the same graph and the renderer can be replaced.
fn graph(state: &Arc<State>, request: &Request) -> Response {
    let focus = request.query("name").map(str::to_string);
    let prefix = request.query("path").map(str::to_string);
    let limit = request
        .query("limit")
        .and_then(|v| v.parse::<usize>().ok())
        // Deliberately small. A force layout is readable at dozens of nodes
        // and a hairball at hundreds, and the scope box is how someone asks
        // for a different part of the graph rather than more of it at once.
        .unwrap_or(70)
        .clamp(1, crate::graph::MAX_NODES);
    let only = request.query_all("store");
    let empty = json!({ "nodes": [], "edges": [], "total": 0, "shown": 0 });

    with_fleet(state, empty, move |fleet| {
        let only = (!only.is_empty()).then_some(only);
        let chosen: Vec<(&str, &crate::Semlith)> = match &only {
            Some(names) => fleet
                .each()
                .filter(|(label, _)| names.iter().any(|n| n == label))
                .collect(),
            None => fleet.each().collect(),
        };
        let many = fleet.len() > 1;
        crate::graph::scoped(&chosen, focus.as_deref(), prefix.as_deref(), limit, many)
    })
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
    let named = body.get("store").and_then(Value::as_str);
    let store = match state.writable(named) {
        Ok(s) => s,
        // Nothing to write to, and no store named: this is the first run. The
        // daemon was started on a machine with nothing indexed, so the store
        // this path belongs in does not exist yet — make it, exactly as
        // `semlith index` would, and serve it without a restart. Before this,
        // the portal's first-run screen invited a developer to index a folder
        // and then answered the button with "no store is open".
        Err(e) if named.is_none() && state.stores().is_empty() => {
            match first_store(state, &paths[0]) {
                Ok(store) => store,
                Err(made) => return Response::error(409, &format!("{e}: {made:#}")),
            }
        }
        Err(e) => return Response::error(409, &e.to_string()),
    };

    match state.index(&store, paths) {
        Ok(progress) => stream(progress),
        Err(e) => Response::error(409, &e.to_string()),
    }
}

/// Pause, resume or stop the index run a store is working on.
///
/// Stopping undoes what the run embedded, so the store is as it was before it
/// started — a half-indexed corpus is worse than none, because nothing says
/// which half it is.
fn index_control(state: &Arc<State>, request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let store = match state.writable(body.get("store").and_then(Value::as_str)) {
        Ok(s) => s,
        Err(e) => return Response::error(409, &e.to_string()),
    };
    match body.get("action").and_then(Value::as_str) {
        Some("pause") => store
            .paused
            .store(true, std::sync::atomic::Ordering::Relaxed),
        Some("resume") => store
            .paused
            .store(false, std::sync::atomic::Ordering::Relaxed),
        Some("stop") => {
            // The queue first: a job that has not started is answered from
            // here, immediately, because there is nothing of it to undo.
            store.cancel_queued();
            store
                .cancelled
                .store(true, std::sync::atomic::Ordering::Relaxed);
            // Released, so a paused run reaches the check that stops it.
            store
                .paused
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }
        _ => return Response::error(400, "action must be \"pause\", \"resume\" or \"stop\""),
    }
    Response::json(&json!({
        "store": store.name,
        "paused": store.paused.load(std::sync::atomic::Ordering::Relaxed),
        "stopping": store.cancelled.load(std::sync::atomic::Ordering::Relaxed),
    }))
}

/// Create the store `path` belongs in and open it in this daemon.
///
/// The resolution is `home`'s, not a second copy of it, so the portal puts the
/// store exactly where `semlith index <path>` would have — same name, same
/// directory under the store home, same registry entry — and the two ways in
/// cannot disagree about where a corpus lives.
fn first_store(state: &Arc<State>, path: &Path) -> Result<Arc<Store>, anyhow::Error> {
    let choice = home::resolve(&[], path, None)?;
    let dir = choice.one()?;

    // Created before it is opened: `Semlith::open` is what lays the store down,
    // and the daemon can only take a lock on something that exists. The handle
    // is dropped immediately so the watcher thread can take the lock itself.
    {
        let store = crate::Semlith::open(&dir, None)?;
        let model = store.model().to_string();
        home::record(&choice, std::slice::from_ref(&path.to_path_buf()), &model)?;
    }

    state.open_store(&dir)
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
        Ok(s) => s,
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
    // One path or many. The Files page selects rows and forgets the set, and
    // a set of one is the same statement as before.
    let paths: Vec<String> = match (
        body.get("path").and_then(Value::as_str),
        body.get("paths").and_then(Value::as_array),
    ) {
        (Some(one), _) => vec![one.to_string()],
        (None, Some(many)) => many
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        (None, None) => return Response::error(400, "no path given"),
    };
    if paths.is_empty() {
        return Response::error(400, "no path given");
    }
    let store = match state.writable(body.get("store").and_then(Value::as_str)) {
        Ok(s) => s,
        Err(e) => return Response::error(409, &e.to_string()),
    };

    // Not streamed: forgetting a file is one statement, and a client that has
    // to parse a stream to learn a number is a client doing extra work. A
    // batch is those statements one after another, because the writer is one
    // thread and running them together would not make it two.
    let single = paths.len() == 1;
    let mut forgot = 0_i64;
    let mut images = 0_i64;
    let mut missing: Vec<String> = Vec::new();
    for path in &paths {
        let progress = match state.forget(&store, PathBuf::from(path)) {
            Ok(p) => p,
            Err(e) => return Response::error(409, &e.to_string()),
        };
        let value = match progress.recv() {
            Ok(v) => v,
            Err(_) => return Response::error(500, "the writer stopped before answering"),
        };
        // One path keeps the answer it has always had, so the MCP tool and
        // every existing caller read the same shape.
        if single {
            return Response::json(&value);
        }
        let chunks = value.get("forgot").and_then(Value::as_i64).unwrap_or(0);
        if chunks == 0 && value.get("images").and_then(Value::as_i64).unwrap_or(0) == 0 {
            missing.push(path.clone());
        }
        forgot += chunks;
        images += value.get("images").and_then(Value::as_i64).unwrap_or(0);
    }
    let kept = paths.len() - missing.len();
    Response::json(&json!({
        "files": kept,
        "asked": paths.len(),
        "forgot": forgot,
        "images": images,
        "not_indexed": missing,
        "message": format!(
            "{kept} file{} forgotten, {forgot} chunk{} removed.",
            if kept == 1 { "" } else { "s" },
            if forgot == 1 { "" } else { "s" },
        ),
    }))
}

/// Delete a store: everything semlith derived from a corpus, and the registry
/// entry naming it.
///
/// The files that were indexed are not touched, which is the line the portal
/// says out loud before it asks for confirmation.
fn delete_store(state: &Arc<State>, request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let Some(name) = body.get("store").and_then(Value::as_str) else {
        return Response::error(400, "no store given");
    };
    match state.delete_store(name) {
        Ok(dir) => Response::json(&json!({
            "store": name,
            "deleted": dir.display().to_string(),
            "message": format!(
                "{name} is gone: its vectors, chunks, graph and ledger were deleted and the \
                 registry no longer lists it. The files it indexed are untouched."
            ),
        })),
        Err(e) => Response::error(409, &e.to_string()),
    }
}

/// The welcome screen's "Adopt an existing .semlith", running the same code
/// path the CLI `adopt` runs.
/// Say that a store directory outside the home may be opened.
///
/// The portal's half of `semlith trust`. It records a path and moves nothing,
/// which is the difference from `/api/adopt` beside it: a developer who wants
/// their `.semlith` to stay with its corpus should not have to move it to keep
/// using it.
fn trust(state: &Arc<State>, request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let Some(dir) = body.get("path").and_then(Value::as_str) else {
        return Response::error(400, "no path given");
    };
    let mut registry = match home::Registry::load() {
        Ok(r) => r,
        Err(e) => return Response::error(500, &e.to_string()),
    };
    match registry.trust(Path::new(dir)) {
        Ok(dir) => Response::json(&json!({
            "dir": dir.display().to_string(),
            "trusted": registry.trusted.iter().map(|d| d.display().to_string()).collect::<Vec<_>>(),
            // The daemon opened its stores at startup, so one trusted now joins
            // on the next start — the same answer `/api/adopt` gives, for the
            // same reason.
            "restart_required": !state.stores().iter().any(|s| s.dir == dir),
        })),
        Err(e) => Response::error(400, &e.to_string()),
    }
}

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
    if state
        .stores()
        .iter()
        .any(|s| s.dir == crate::canonical(&dir))
    {
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
    let proxy = crate::proxy::proxy_pid(request.header("semlith-proxy"));
    if let Some(pid) = proxy {
        state.saw_proxy(pid);
    }
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };

    // Who is asking. A client is told from another by its session: the id this
    // daemon hands out at `initialize` over HTTP, or the proxy's pid for a
    // forwarding `semlith mcp`. One that echoes neither is one unnamed client
    // rather than a new one per request.
    let transport = match proxy {
        Some(_) => "stdio proxy",
        None if request.path == crate::http::MCP_PATH => "http /mcp",
        None => "portal",
    };
    let method = body.get("method").and_then(Value::as_str).unwrap_or("");
    let params = body.get("params");
    let name = params
        .and_then(|p| p.get("clientInfo"))
        .and_then(|c| c.get("name"))
        .and_then(Value::as_str);
    let revision = params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str);
    // A session id arrives from a client and goes back out in a response
    // header, so what a client may send is exactly what `new_session` produces:
    // sixteen hex characters. Anything else — a header injection, a control
    // character, a kilobyte of text — is replaced with a fresh id rather than
    // reflected, and the client is told the new one in the `initialize`
    // response the same way it would be told a first one.
    let session = request
        .header("mcp-session-id")
        .filter(|id| is_session_id(id))
        .map(str::to_string)
        .or_else(|| proxy.map(|pid| pid.to_string()))
        .unwrap_or_else(|| {
            if method == "initialize" {
                new_session()
            } else {
                String::from("anonymous")
            }
        });
    state.note_client(&session, transport, name, revision, method == "tools/call");
    // A method that is about the server rather than about a corpus is answered
    // whether or not anything is indexed: an agent connecting to a fresh
    // install should be told which tools exist, not that the daemon is broken.
    let corpus_free = matches!(method, "initialize" | "tools/list" | "ping")
        || method.starts_with("notifications/");
    if let Err(e) = state.open_mcp_fleet()
        && !corpus_free
    {
        return Response::error(409, &e.to_string());
    }

    let writer = daemon::Writer(Arc::clone(state));
    let mut fleet = state.mcp_fleet.lock().unwrap_or_else(|e| e.into_inner());
    let mut nothing = crate::fleet::Fleet::empty();
    let fleet = match fleet.as_mut() {
        Some(fleet) => fleet,
        None if corpus_free => &mut nothing,
        None => return Response::error(409, "this daemon has no store open"),
    };

    let response = match crate::mcp::answer(fleet, Some(&writer), &body) {
        Some(value) => Response::json(&value),
        // A notification. Answered with an empty 200 rather than an empty JSON
        // object, so the proxy writes nothing to a client that expects nothing.
        None => Response::new(200, "application/json; charset=utf-8", Vec::new()),
    };
    // The session id is handed back at `initialize`, which is where the MCP
    // HTTP transport says a server may assign one. A client that echoes it is
    // counted as itself on every later call; one that does not still works.
    if method == "initialize" {
        return response.header("Mcp-Session-Id", session);
    }
    response
}

/// The shape [`new_session`] produces, and the only shape accepted from a
/// client.
fn is_session_id(value: &str) -> bool {
    value.len() == 16 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A session id, which identifies a client and guards nothing.
fn new_session() -> String {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).expect("the OS random source");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Rotate the agent key, or take up one another process has just written.
///
/// The previous key stays valid until this daemon exits unless `now` is set,
/// so a client mid-session finishes its work rather than failing on the call
/// it happened to be making. It is not persisted: a restart is where the old
/// key stops, which is the same moment every client had to be told anyway.
fn key(state: &Arc<State>, request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let now = body.get("now").and_then(Value::as_bool).unwrap_or(false);
    // Read before it is replaced: it is what identifies the stanzas to rewrite.
    let previous = crate::home::agent_key().unwrap_or_default();
    let fresh = match body.get("key").and_then(Value::as_str) {
        // A key another process has already written to disk.
        Some(key) if crate::home::is_agent_key(key) => key.to_string(),
        Some(_) => return Response::error(400, "that is not an agent key"),
        None => match crate::home::rotate_agent_key() {
            Ok(key) => key,
            Err(e) => return Response::error(500, &e.to_string()),
        },
    };
    state.server.rotate_agent(&fresh, now);
    // Every client whose configuration file already carried the old key is
    // carried forward with it. A rotation that leaves twelve files
    // authenticating with a refused key is a rotation that breaks the machine
    // it was run on.
    let updated: Vec<String> = crate::setup::recarry_key(&previous, &fresh)
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    Response::json(&json!({
        "key": fresh,
        "previous_valid": !now,
        "updated": updated,
        "stanzas": crate::clients::http_stanzas(&fresh),
    }))
}

/// Start or stop the MCP endpoint while the daemon runs.
///
/// Closing it drops the route and nothing else: the stores stay open, the
/// watcher keeps running and the portal keeps working.
fn endpoint(state: &Arc<State>, request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let Some(open) = body.get("open").and_then(Value::as_bool) else {
        return Response::error(400, "missing open");
    };
    state.server.set_mcp_open(open);
    Response::json(&json!({
        "open": open,
        "url": format!("http://127.0.0.1:{}{}", state.server.port(), crate::http::MCP_PATH),
    }))
}

fn rotate(state: &Arc<State>) -> Response {
    let fresh = state.rotate();
    // Returned once, to the page that asked and holds the old one. It keeps
    // working because it puts this value in the header from here on; every
    // other holder of the old token stops at the next request.
    Response::json(&json!({ "token": fresh, "url": daemon::url(state) }))
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
fn named(state: &Arc<State>, name: &str) -> Option<Arc<Store>> {
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
