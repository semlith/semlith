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

/// The most files a derived-column sort will order.
///
/// `FILE_OFFSET_MAX` bounds how deep the table can be paged; this bounds how
/// much has to be held to order it by a column the database does not hold.
const DERIVED_SORT_MAX: i64 = 20_000;

/// How many ledger rows the Retrieval ledger page is handed.
///
/// The same depth `semlith ledger --last 200` offers, which is what the
/// portal-parity rule asks for: the page shows the rows the CLI prints.
const LEDGER_ROWS: usize = 200;

/// How many sessions the ledger page's table reads at once.
///
/// It pages in the browser over what this returns rather than re-querying
/// per page: a session row is a few numbers, and a round trip per page of a
/// local table is a round trip for nothing.
const LEDGER_SESSIONS: usize = 500;

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

        (true, _, "/api/stores") => stores(state, request),
        (true, _, "/api/files") => files(state, request),
        (true, _, "/api/search") => search(state, request),
        (true, _, "/api/brief") => brief(state, request),
        (true, _, "/api/models") => models(),
        (true, _, "/api/languages") => languages(),
        (true, _, "/api/dirs") => dirs(request),
        (true, _, "/api/privacy") => privacy(state),
        (true, _, "/api/privacy/scan") => privacy_scan(state),
        (true, _, "/api/about") => about(state),
        (true, _, "/api/agents") => agents(state),
        (true, _, "/api/pattern") => pattern(state, request),
        (true, _, "/api/read") => read(state, request),
        (true, _, "/api/symbol") => symbol(state, request),
        (true, _, "/api/neighbors") => neighbors(state, request),
        (true, _, "/api/path") => shortest_path(state, request),
        (true, _, "/api/impact") => impact(state, request),
        (true, _, "/api/trace") => trace(state, request),
        (true, _, "/api/map") => map(state, request),
        (true, _, "/api/report") => report(state, request),
        (true, _, "/api/schedules") => schedules(state),
        // Both verbs: a GET reports whether the toggle is on and, when it
        // is, what the transcripts say; a POST is the toggle itself.
        (_, _, "/api/ledger/replay") if get || post => replay(request),
        (true, _, "/api/graph") => graph(state, request),
        (true, _, "/api/corpus") => corpus(state),
        (true, _, "/api/ledger") => ledger(state),
        (true, _, "/api/image") => image_file(state, request),
        (true, _, "/api/setup") => setup(),
        (true, _, "/api/doctor") => doctor(state),
        (true, _, "/api/index/runs") => index_runs(state),
        (true, _, "/api/index/log") => index_log(state, request),
        (true, _, "/api/projects") => projects(request),
        (true, _, "/api/changes") => changes(state),

        (_, true, "/api/index") => index(state, request),
        (_, true, "/api/add") => add(state, request),
        (_, true, "/api/forget") => forget(state, request),
        (_, true, "/api/adopt") => adopt(state, request),
        (_, true, "/api/trust") => trust(state, request),
        (_, true, "/api/ledger/raw-read") => raw_read(state, request),
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
        (_, true, "/api/privacy/fix") => privacy_fix(state, request),
        (_, true, "/api/agents/register") => register_clients(state, request),
        (_, true, "/api/root") => root(state, request),
        (_, true, "/api/store/delete") => delete_store(state, request),
        (_, true, "/api/index/control") => index_control(state, request),
        (_, true, "/api/index/settings") => index_settings(state, request),
        (true, _, "/api/accel") => accel_status(),
        (_, true, "/api/accel") => accel_change(request),
        (_, true, "/api/schedules") => schedule_write(state, request),
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
            "/api/index"
            | "/api/add"
            | "/api/forget"
            | "/api/adopt"
            | "/api/rotate"
            | "/api/mcp"
            | "/api/upgrade"
            | "/api/privacy/fix"
            | "/api/agents/register",
        ) => Response::error(405, "wrong method for this route"),
        _ => Response::error(404, "no such route"),
    }
}

/// Every schedule the daemon holds, exactly as the file holds them.
///
/// Read from disk rather than from anything the runner is caching, because the
/// file is the state: `semlith schedule add` is a different process writing the
/// same file, and a route answering from memory would show the page one list
/// while the terminal showed another.
fn schedules(_state: &Arc<State>) -> Response {
    match crate::schedule::Schedules::load() {
        Ok(file) => Response::json(&serde_json::json!({ "schedules": file.schedules })),
        // A schedules file this binary cannot read is a report that will not be
        // written, not a reason for the page to fail to draw.
        Err(e) => Response::error(500, &format!("{e:#}")),
    }
}

/// Add, remove, or turn one on and off.
///
/// One route and an `action` rather than three paths: every one of them is the
/// same read-modify-write of one small file, and splitting them would be three
/// places to remember to wake the timer from.
fn schedule_write(state: &Arc<State>, request: &Request) -> Response {
    let body: serde_json::Value = match serde_json::from_slice(&request.body) {
        Ok(value) => value,
        Err(e) => return Response::error(400, &format!("the body is not JSON: {e}")),
    };
    let action = body.get("action").and_then(|v| v.as_str()).unwrap_or("");

    let mut file = match crate::schedule::Schedules::load() {
        Ok(file) => file,
        Err(e) => return Response::error(500, &format!("{e:#}")),
    };

    let answer = match action {
        "add" => {
            let Some(kind) = body.get("kind").and_then(|v| v.as_str()) else {
                return Response::error(400, "missing kind");
            };
            let Some(dir) = body.get("dir").and_then(|v| v.as_str()) else {
                return Response::error(400, "missing dir");
            };
            let Some(every) = body.get("every_seconds").and_then(|v| v.as_u64()) else {
                return Response::error(400, "missing every_seconds");
            };
            let format = body
                .get("format")
                .and_then(|v| v.as_str())
                .unwrap_or("markdown");
            let model = body
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("Sonnet 5");
            let mut schedule = crate::schedule::Schedule::new(
                kind,
                format,
                model,
                every,
                std::path::Path::new(dir),
            );
            schedule.window = body
                .get("window")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            schedule.stores = body
                .get("stores")
                .and_then(|v| v.as_array())
                .map(|rows| {
                    rows.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            // Refused before anything is written, naming what would have
            // worked. Half a schedules file is worse than none.
            if let Err(e) = schedule.check() {
                return Response::error(400, &format!("{e:#}"));
            }
            match file.add(schedule) {
                Ok(id) => serde_json::json!({ "id": id }),
                Err(e) => return Response::error(400, &format!("{e:#}")),
            }
        }
        "remove" => {
            let Some(id) = body.get("id").and_then(|v| v.as_str()) else {
                return Response::error(400, "missing id");
            };
            if !file.remove(id) {
                return Response::error(404, &format!("no schedule {id:?}"));
            }
            serde_json::json!({ "removed": id })
        }
        "set" => {
            let Some(id) = body.get("id").and_then(|v| v.as_str()) else {
                return Response::error(400, "missing id");
            };
            let Some(enabled) = body.get("enabled").and_then(|v| v.as_bool()) else {
                return Response::error(400, "missing enabled");
            };
            let Some(schedule) = file.schedules.get_mut(id) else {
                return Response::error(404, &format!("no schedule {id:?}"));
            };
            schedule.enabled = enabled;
            serde_json::json!({ "id": id, "enabled": enabled })
        }
        other => {
            return Response::error(
                400,
                &format!("unknown action {other:?}; the actions are add, remove, set"),
            );
        }
    };

    if let Err(e) = file.save() {
        return Response::error(500, &format!("{e:#}"));
    }
    // The timer is asleep on a condition variable until the next due time. It
    // would find this within a minute anyway — that ceiling exists for the CLI,
    // which cannot reach this condvar — but a page that has just added a
    // schedule should not have to wait out a sleep to see its next run.
    state.schedules.wake();
    Response::json(&answer)
}

// ---------------------------------------------------------------- reads

/// Every open store, what it holds, and whether it is being kept current.
fn stores(state: &Arc<State>, request: &Request) -> Response {
    // Per-language coverage is a scan of every call edge with a lookup per
    // edge, and this route is polled by every page that shows a store count.
    // Only the page that draws the table asks for it: with it on every poll
    // the Stores page fell far enough behind that the browser drive caught a
    // row still describing a store whose directory had already gone.
    let want_coverage = request.query("coverage") == Some("1");
    // Before the fleet, because it may add to what the fleet has to cover. A
    // store the CLI wrote while this daemon was running is registered and not
    // open, and this is the read that notices — which is what makes `semlith
    // index ~/work/new-project` show up on this page without a restart.
    let unopened = state.reconcile();
    // Opened here rather than at startup: it is None until the first read, and
    // None again after a store joins, so every read route has to be able to
    // put it back. Cheap when it is already open.
    if let Err(e) = state.open_fleet() {
        return Response::error(500, &e.to_string());
    }
    let mut fleet = state.fleet.lock().unwrap_or_else(|e| e.into_inner());
    let registry = home::Registry::load().unwrap_or_default();
    // Probed once, before the rows are built. Selecting stores is what probes
    // them, so every store is asked for first and the answers are read off
    // afterwards: a store whose database cannot be read has no figures to show
    // and its row has to say why rather than show a convincing set of zeros.
    let failed = fleet
        .as_ref()
        .map(|f| {
            let _ = f.each().count();
            f.failed()
        })
        .unwrap_or_default();
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
                    // The store's own last write, not this daemon's memory of
                    // one. A store written yesterday has a last write today.
                    store::last_write(s.db()).ok().flatten(),
                    // One savings line per card, and never the number alone:
                    // its coverage and its tier travel with it, because a
                    // figure a reader cannot check is a figure they are being
                    // asked to take on trust.
                    store::ledger_savings(s.db()).ok(),
                    // What the graph covers, per language. The Index page's
                    // one table, and the same rows `semlith stats` prints: an
                    // absent edge and a file the parser gave up on are
                    // different problems and a store-wide percentage hides
                    // both.
                    if want_coverage {
                        store::coverage_by_language(s.db()).unwrap_or_default()
                    } else {
                        Vec::new()
                    },
                    // The two graph-health figures that belong to no
                    // language: what the graph points at and cannot find,
                    // and how many names it cannot tell apart. Same reads
                    // `semlith stats` prints under its coverage table, so
                    // the page and the terminal cannot disagree.
                    if want_coverage {
                        let (top, distinct) =
                            store::unresolved_targets(s.db(), 5).unwrap_or_default();
                        let several =
                            store::names_with_several_definitions(s.db()).unwrap_or_default();
                        let months = store::chunks_by_month(s.db()).unwrap_or_default();
                        Some(json!({
                            "unresolved_top": top
                                .iter()
                                .map(|(name, n)| json!({ "name": name, "edges": n }))
                                .collect::<Vec<_>>(),
                            "unresolved_names": distinct,
                            "several_definitions": several,
                            "months": months
                                .iter()
                                .map(|(month, n)| json!({ "month": month, "chunks": n }))
                                .collect::<Vec<_>>(),
                        }))
                    } else {
                        None
                    },
                )
            });

        #[allow(clippy::type_complexity)]
        let (
            files,
            chunks,
            bytes,
            model,
            dim,
            vectors,
            shards,
            facets,
            written,
            savings,
            coverage,
            health,
        ) = match stats {
            Some((
                Ok((f, c, b)),
                model,
                dim,
                len,
                shards,
                facets,
                written,
                savings,
                coverage,
                health,
            )) => (
                f, c, b, model, dim, len, shards, facets, written, savings, coverage, health,
            ),
            _ => (
                0,
                0,
                0,
                String::new(),
                0,
                0,
                None,
                store::Facets::default(),
                None,
                None,
                Vec::new(),
                None,
            ),
        };

        // The daemon's own counter still wins when it is newer, so a re-embed
        // that has landed in this session shows immediately rather than waiting
        // for the next read of the file table.
        let session = handle.last_write.load(Ordering::Relaxed) as i64;
        let last_write = written.unwrap_or(0).max(session);

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
            "dir": crate::plain(&handle.dir.display().to_string()),
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
                "path": crate::plain(&r.display().to_string()),
                "present": r.exists(),
            })).collect::<Vec<_>>(),
            "files": files,
            "chunks": chunks,
            "bytes": bytes,
            "model": model,
            "dim": dim,
            "vectors": vectors,
            "shards": shards.map(|(n, max)| json!({ "count": n, "resident": max })),
            "coverage": coverage.iter().map(|row| json!({
                "language": row.language,
                "files": row.files,
                "parser_failed": row.parser_failed,
                "definitions": row.definitions,
                "extracted": row.extracted,
                "resolved": row.resolved,
                "ambiguous": row.ambiguous,
                "unresolved": row.unresolved,
                "settled": row.settled_share(),
            })).collect::<Vec<_>>(),
            "health": health,
            "lines": facets.lines,
            "formats": facets.extensions.len(),
            "readers": readers.len(),
            "watching": handle.watching.load(Ordering::Relaxed),
            // Why the writer ended, beside `watching: false`, so a store that
            // stopped being kept current says what stopped it.
            "stopped_because": handle.stopped_because.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            "queue": handle.queue_depth(),
            "last_write": last_write,
            // Rows this store held for files outside its roots, dropped when
            // the daemon opened it. Shown for the session so the correction is
            // visible rather than silent.
            "pruned": handle.pruned.load(Ordering::Relaxed),
            "missing": false,
            // Open, registered, and unreadable: the figures on this row are
            // zeros because the database could not be read, not because the
            // store is empty. `failed` below carries the reason and the remedy.
            "unreadable": failed
                .iter()
                .any(|f| f.store == handle.name || Path::new(&f.path) == handle.dir),
            // Tokens saved, what they cover, and how they were counted. One
            // line, no chart: a chart of one number is decoration, and the
            // three qualifiers are what make the number defensible.
            "savings": savings.map(|s| json!({
                "net_tokens": s.net,
                "credited": s.credited,
                "total": s.total,
                "coverage": s.coverage(),
                "tier": s.tier(),
            })),
            "events": handle.events(),
        }));
    }

    // Registered and not served. Listed rather than dropped, because a store
    // the user can see in `semlith stats` and not on this page reads as the
    // portal having lost it. A directory another process is writing comes back
    // on the next read of this route, by itself.
    for store in unopened {
        out.push(json!({
            "name": store.name,
            "dir": crate::plain(&store.dir.display().to_string()),
            "unopened": store.why,
            // A registry entry for a directory that is not there. The portal
            // shows it as missing, naming the path that is absent, and offers
            // it nowhere a real store is offered — not in the Index dropdown,
            // not as a search chip, not as a graph chip.
            "missing": store.missing,
            "roots": [],
            "files": 0,
            "chunks": 0,
            "bytes": 0,
            "model": "",
            "dim": 0,
            "vectors": 0,
            "lines": 0,
            "formats": 0,
            "readers": 0,
            "watching": false,
            "queue": 0,
            "last_write": 0,
            "pruned": 0,
            "events": [],
        }));
    }

    let mut answer = json!({ "stores": out });
    if !failed.is_empty() {
        answer["failed"] = json!(failed);
    }
    Response::json(&answer)
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
        .unwrap_or(15);
    // Refused rather than clamped, the same contract `offset` above has. A
    // caller that asked for two thousand rows and silently got five hundred
    // has no way to tell that from a corpus with five hundred files in it.
    if !(1..=FILE_PAGE).contains(&limit) {
        return Response::error(400, &format!("limit must be between 1 and {FILE_PAGE}"));
    }

    // Each store is asked for the first `offset + limit` rows in the requested
    // order and the merge picks the page out of the union. Asking each store
    // for the page directly would be wrong the moment two stores are open: the
    // tenth row overall is not the tenth row of either of them.
    // Three of the seven columns are derived rather than stored, so ordering
    // by one of them needs every matching row and not one page of each
    // store's. Bounded, and refused past the bound rather than silently
    // ordering a prefix — a sort that quietly describes the first ten thousand
    // rows of a larger corpus is a sort nobody can trust.
    let mut merged: Vec<(String, store::FileRow)> = Vec::new();
    let mut total = 0i64;
    let mut extensions: Vec<String> = Vec::new();
    let mut stores = 0usize;
    // Through the fleet rather than filtered here: a store that cannot be read
    // is left out of the listing and reported beside it, and a listing scoped
    // to nothing but unreadable stores is refused rather than answered empty.
    let only = (!only.is_empty()).then_some(only);
    let chosen = match fleet.selected(only.as_deref()) {
        Ok(c) => c,
        Err(e) => return Response::error(500, &format!("{e:#}")),
    };
    // Counted first, because how many rows each store has to be asked for
    // depends on it: a derived sort needs all of them.
    for (_, opened) in &chosen {
        let opened = *opened;
        // Stores with a matching file, not stores that happen to be open. The
        // header read "0 files · 0 formats · 2 stores", where two of the three
        // numbers answered the filter and the third did not.
        match store::file_count(opened.db(), filter.groups()) {
            Ok(count) => {
                total += count;
                stores += usize::from(count > 0);
            }
            Err(e) => return Response::error(500, &e.to_string()),
        }
        match store::file_facets(opened.db(), filter.groups()) {
            Ok(facets) => extensions.extend(facets.extensions),
            Err(e) => return Response::error(500, &e.to_string()),
        }
    }
    if sort.derived() && total > DERIVED_SORT_MAX {
        return Response::error(
            400,
            &format!(
                "this column is derived rather than stored, so ordering by it needs every \
                 matching row; narrow the filter to {DERIVED_SORT_MAX} files or fewer"
            ),
        );
    }
    let take = if sort.derived() {
        total
    } else {
        offset.saturating_add(limit)
    };
    for (label, opened) in &chosen {
        let (label, opened) = (*label, *opened);
        let listed = match store::file_rows(opened.db(), filter.groups(), sort, desc, take) {
            Ok(r) => r,
            Err(e) => return Response::error(500, &e.to_string()),
        };
        merged.extend(listed.into_iter().map(|row| (label.to_string(), row)));
    }

    merged.sort_by(|(a_store, a), (b_store, b)| {
        let order = match sort {
            store::FileSort::Path => a.path.cmp(&b.path),
            store::FileSort::Bytes => a.bytes.cmp(&b.bytes),
            store::FileSort::Chunks => a.chunks.cmp(&b.chunks),
            store::FileSort::Lines => a.lines.cmp(&b.lines),
            store::FileSort::Indexed => a.indexed_at.cmp(&b.indexed_at),
            store::FileSort::Store => a_store.cmp(b_store),
            store::FileSort::Reader => {
                chunk::reader_of(Path::new(&a.path)).cmp(chunk::reader_of(Path::new(&b.path)))
            }
            store::FileSort::Lang => {
                language_of(Path::new(&a.path)).cmp(language_of(Path::new(&b.path)))
            }
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
                "path": crate::plain(&row.path),
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

    let mut out = json!({
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
    });
    failures_beside(fleet, &mut out);
    Response::json(&out)
}

/// The same fused search the CLI and `semlith_search` run.
/// One question, one answer, under a token budget -- what `semlith brief` and
/// `semlith_brief` return, for the page that shows a person the same thing.
///
/// Free, like every primitive under it. The assembly is the tool's, serialised
/// as the tool serialises it, so the page cannot drift into showing something
/// the agent surface does not return.
fn brief(state: &Arc<State>, request: &Request) -> Response {
    let Some(question) = request.query("question").filter(|q| !q.trim().is_empty()) else {
        return Response::error(400, "missing question");
    };
    let budget = request
        .query("budget")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(crate::brief::DEFAULT_BUDGET)
        .clamp(1, 200_000);
    let filter = match filter_of(request) {
        Ok(f) => f,
        Err(e) => return Response::error(400, &e),
    };
    let prefer = match request.query("prefer") {
        Some(raw) => match crate::Prefer::parse(raw) {
            Ok(p) => p,
            Err(e) => return Response::error(400, &e.to_string()),
        },
        None => crate::Prefer::default(),
    };
    let only: Vec<String> = request
        .query("store")
        .map(|s| s.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    if let Err(e) = state.open_fleet() {
        return Response::error(500, &e.to_string());
    }
    let mut fleet = state.fleet.lock().unwrap_or_else(|e| e.into_inner());
    let Some(fleet) = fleet.as_mut() else {
        return Response::json(&json!({ "spans": [], "symbols": [] }));
    };

    let started = std::time::Instant::now();
    let only = (!only.is_empty()).then_some(only);
    let brief = match crate::brief::brief(fleet, only.as_deref(), question, budget, &filter, prefer)
    {
        Ok(b) => b,
        Err(e) => return Response::error(500, &format!("{e:#}")),
    };
    let elapsed = started.elapsed();

    let body = match serde_json::to_value(&brief) {
        Ok(v) => v,
        Err(e) => return Response::error(500, &e.to_string()),
    };
    if state.ledger {
        // From the brief rather than from its rendering. `reply` recovers the
        // files an answer named by reading them back out of rendered text, and
        // what this route hands it is JSON -- so every brief from this page was
        // recorded with no hits and no saving, exactly as every brief from the
        // command line was. The MCP tool still goes through `reply` because
        // what it hands over really is the rendered text `paths_in` was written
        // for, and because a span whose text the budget dropped should be
        // counted at its locator rather than at its file.
        crate::ledger::brief(
            fleet,
            &crate::ledger::Who {
                client: "portal",
                session: "portal",
            },
            question,
            &brief,
            &body.to_string(),
            elapsed,
        );
    }
    let mut answer = json!({
        "brief": body,
        "chunks": fleet.chunks(),
        "micros": elapsed.as_micros() as u64,
    });
    failures_beside(fleet, &mut answer);
    Response::json(&answer)
}

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
            Err(e) => return Response::error(500, &format!("{e:#}")),
        }
    };
    if selected == Some(0) {
        return Response::json(&json!({ "hits": [], "selected": 0, "chunks": fleet.chunks() }));
    }

    let prefer = match request.query("prefer") {
        Some(raw) => match crate::Prefer::parse(raw) {
            Ok(p) => p,
            Err(e) => return Response::error(400, &e.to_string()),
        },
        None => crate::Prefer::default(),
    };

    let started = std::time::Instant::now();
    let only = (!only.is_empty()).then_some(only);
    // The offset is applied rather than validated and dropped. Ranking is over
    // the whole fused set, so a page is cut out of a deeper search: ask for
    // `offset + k` and take the page from `offset`, the way `/api/files`
    // already does. A caller that paged used to be handed page one every time
    // and had no way to tell (#78).
    let deep = (offset as usize)
        .saturating_add(k)
        .min(FILE_OFFSET_MAX as usize);
    let found = match fleet.search_preferring(only.as_deref(), query, deep, &filter, prefer) {
        Ok(h) => h,
        Err(e) => return Response::error(500, &format!("{e:#}")),
    };
    let elapsed = started.elapsed();
    let hits: Vec<_> = found.into_iter().skip(offset as usize).take(k).collect();

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
                "path": crate::plain(&h.path),
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
        // The portal is one client among several now, named the same way the
        // agents are, and recorded through the same path they use.
        crate::ledger::search(
            fleet,
            &crate::ledger::Who {
                client: "portal",
                session: "portal",
            },
            query,
            &hits,
            elapsed,
        );
    }

    // The page draws the shape hint from these two rather than re-deriving the
    // rule in JavaScript, so there is one classifier and it is the one that
    // ranked the answer.
    let shape = crate::shape_of(query);
    let mut answer = json!({
        "hits": out,
        "offset": offset,
        "selected": selected,
        "chunks": fleet.chunks(),
        "micros": elapsed.as_micros() as u64,
        "shape": shape,
        "shape_label": shape.as_str(),
        "weighting": shape.weighting(),
        "prefer": prefer,
    });
    failures_beside(fleet, &mut answer);
    Response::json(&answer)
}

/// The free half of the ledger: whether it is recording, and the totals.
/// Record one whole-file read an agent made without asking semlith.
///
/// The steering hook is the only caller. It runs inside a client's tool call
/// and must not open a store itself, so it posts the fact here and the daemon —
/// which already holds every store open — writes the row.
///
/// The row is written into the store whose roots cover the file, so Refunds is
/// per store the way every other ledger figure is, and a read of a file no open
/// store holds is recorded nowhere rather than against whichever store happened
/// to be first.
fn raw_read(state: &Arc<State>, request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let Some(path) = body.get("path").and_then(Value::as_str) else {
        return Response::error(400, "no path given");
    };
    let client = body
        .get("client")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let session = body.get("session").and_then(Value::as_str).unwrap_or("");

    // `--no-ledger` is a promise about this session, and it covers rows the
    // hook asks for exactly as it covers rows a search writes.
    if !state.ledger {
        return Response::json(&json!({ "recorded": false, "reason": "not recording" }));
    }
    with_fleet(state, json!({ "recorded": false }), move |fleet| {
        Ok(json!({ "recorded": crate::ledger::raw_read(fleet, client, session, path) }))
    })
}

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
        let (mut clients, mut excerpt, mut whole) = (0, 0, 0);
        let mut intact = true;
        // Unioned rather than summed: one search over six stores writes a row
        // in each that answered it, under one query id, and adding six stores'
        // own counts is what made this page report sixty-three queries for
        // about a dozen searches.
        let mut seen: std::collections::BTreeSet<String> = Default::default();
        for (label, store) in fleet.each() {
            let (_, c, e, w) = store::ledger_totals(store.db())?;
            seen.extend(store::ledger_keys(
                store.db(),
                label,
                store::LedgerScope::All,
            )?);
            clients = clients.max(c);
            excerpt += e;
            whole += w;
            intact = intact && store::ledger_break(store.db())?.is_none();
        }
        let queries = seen.len() as i64;
        // Summed across stores the same way the totals are, and reported with
        // the denominators that make them readable: a ratio on its own is a
        // marketing number, and the Ledger page is told not to draw one.
        let mut net = 0;
        let mut measured = true;
        let mut credited_keys: std::collections::BTreeSet<String> = Default::default();
        let mut estimated_keys: std::collections::BTreeSet<String> = Default::default();
        let mut by_client: std::collections::BTreeMap<String, i64> = Default::default();
        // The newest rows across every store, merged, for the table this page
        // owes the CLI's `semlith ledger --last 20`. Read here rather than
        // from a second route so the tiles and the rows cannot disagree.
        let mut rows: Vec<Value> = Vec::new();
        let mut sessions: Vec<Value> = Vec::new();
        let mut refunds = 0;
        let mut zero_hit = 0;
        let mut refunds_measured = false;
        for (label, store) in fleet.each() {
            let savings = store::ledger_savings(store.db())?;
            net += savings.net;
            measured = measured && savings.measured;
            let misses = store::ledger_misses(store.db())?;
            refunds += misses.refunds;
            zero_hit += misses.zero_hit;
            // One hooked store is enough to make the figure a measurement for
            // that store, and the page says which kind it is rather than
            // averaging two different things into one word.
            refunds_measured = refunds_measured || misses.measured;
            credited_keys.extend(store::ledger_keys(
                store.db(),
                label,
                store::LedgerScope::Credited,
            )?);
            estimated_keys.extend(store::ledger_keys(
                store.db(),
                label,
                store::LedgerScope::Estimated,
            )?);
            for (client, count) in store::ledger_clients(store.db())? {
                *by_client.entry(client).or_default() += count;
            }
            // One row per session per store, grouped by SQL rather than by
            // the browser: a page adding up six stores' rows is a second
            // opinion about a number the store can state.
            for session in store::ledger_sessions(store.db(), LEDGER_SESSIONS)? {
                sessions.push(json!({
                    "session": session.session,
                    "client": session.client,
                    "store": label,
                    "first": session.first,
                    "last": session.last,
                    "when": crate::clock::local_stamp(session.last),
                    "retrievals": session.retrievals,
                    "zero_hit": session.zero_hit,
                    "excerpt_tokens": session.excerpt_tokens,
                    "whole_file_tokens": session.whole_file_tokens,
                    "net_tokens": session.net,
                    "tier": session.tier(),
                }));
            }
            for row in store::retrievals(store.db(), LEDGER_ROWS)? {
                rows.push(json!({
                    "at": row.at,
                    // Local time with the offset, the same string `semlith
                    // ledger` prints. One dataset, one clock.
                    "when": crate::clock::local_stamp(row.at),
                    "store": label,
                    "client": row.client,
                    "query": row.query,
                    "hits": row.hits,
                    "ms": row.micros / 1000,
                    "excerpt_tokens": row.excerpt_tokens,
                    "whole_file_tokens": row.whole_file_tokens,
                    "query_id": row.query_id,
                }));
            }
        }
        rows.sort_by_key(|row| std::cmp::Reverse(row["at"].as_i64().unwrap_or(0)));
        rows.truncate(LEDGER_ROWS);
        sessions.sort_by_key(|row| std::cmp::Reverse(row["last"].as_i64().unwrap_or(0)));
        sessions.truncate(LEDGER_SESSIONS);
        let credited = credited_keys.len() as i64;
        let measured = measured && estimated_keys.is_empty();
        let coverage = if queries == 0 {
            0
        } else {
            credited * 100 / queries
        };
        let ratio = (excerpt > 0).then(|| whole as f64 / excerpt as f64);
        Ok(json!({
            "recording": recording,
            "queries": queries,
            "clients": clients,
            "excerpt_tokens": excerpt,
            "whole_file_tokens": whole,
            "ratio": ratio,
            "intact": intact,
            "net_tokens": net,
            "credited": credited,
            "coverage": coverage,
            "tier": if measured && credited > 0 { "measured" } else { "modelled" },
            // What semlith did not answer, in two figures rather than one.
            // A refund is an agent that did not reach for semlith; a zero hit
            // is semlith that did not reach the answer. They call for opposite
            // things, so they are never added together.
            "refunds": refunds,
            "refunds_measured": refunds_measured,
            "zero_hit": zero_hit,
            "by_client": by_client,
            "rows": rows,
            "sessions": sessions,
            // Rows written before this version were one per open store, so the
            // figures they contribute to may be over-counted. Said on the page
            // rather than corrected in place: the chain is not rewritten.
            "legacy_rows": rows.iter().any(|row| {
                row["query_id"].as_str().unwrap_or_default().is_empty()
            }),
        }))
    })
}

/// Notes fastembed's catalogue gets wrong about its own models.
///
/// These strings are shown in semlith's UI, beside a model the user is invited
/// to choose between, so they are semlith's statements whatever their origin.
/// Four of them described a different model than the row they sat on: two
/// called a base model large, two called an English-only model multilingual,
/// and one called itself the default when semlith's default is granite.
///
/// Keyed on the name the row shows. A model whose upstream note is right is
/// not listed, so this table stays the list of known errors rather than a
/// second catalogue that has to be kept in step.
const MODEL_NOTES: &[(&str, &str)] = &[
    (
        "BGEBaseENV15Q",
        "Quantized v1.5 release of the base English model",
    ),
    (
        "GTEBaseENV15",
        "Base English embedding model from Alibaba DAMO Academy",
    ),
    (
        "GTEBaseENV15Q",
        "Quantized base English embedding model from Alibaba DAMO Academy",
    ),
    (
        "BGESmallENV15",
        "Fast and small English model from BAAI. Not the one semlith builds a store with unless it is asked for",
    ),
];

fn models() -> Response {
    let mut out = vec![
        json!({
            "name": embed::GRANITE_NAME,
            "dim": 384,
            "description": "default. IBM Granite R2 small, int8, English",
            // The repository it is fetched from, which is also how its cache
            // directory is named — so the model semlith actually runs shows
            // its size like every other cached model rather than a dash.
            "code": embed::GRANITE_REPO,
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
        let name = info.model.to_string();
        let description = MODEL_NOTES
            .iter()
            .find(|(model, _)| *model == name)
            .map(|(_, note)| (*note).to_string())
            .unwrap_or_else(|| info.description.clone());
        out.push(json!({
            "name": name,
            "dim": info.dim,
            "description": description,
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
    let Ok(entries) = crate::model_cache_dir().and_then(|dir| Ok(std::fs::read_dir(dir)?)) else {
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
    // Never a volume root. Falling back to `/` — or, on Windows, to the drive
    // the process started on — turned a picker confined to the user's home into
    // one confined to the whole filesystem, and then refused the user's own
    // profile as outside it (#71).
    let home = match crate::home::user_home() {
        Ok(home) => crate::canonical(&home),
        Err(e) => return Response::error(500, &e.to_string()),
    };

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
        // Dot-entries are noise in a corpus picker, with one exception: a
        // `.semlith` directory is the thing "Adopt existing .semlith" adopts,
        // and hiding it meant nothing in the listing told an adoptable folder
        // from any other — while the feature named after it could not reach
        // it at all.
        if name.starts_with('.') && name != crate::home::LOCAL_DIR {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let path = entry.path();
        entries.push(json!({
            "name": name,
            "path": crate::plain(&path.display().to_string()),
            "dir": is_dir,
            // Whether pointing adopt at this folder would work: it either is a
            // store, or holds one at `.semlith`.
            "adoptable": is_dir && (is_store(&path) || is_store(&path.join(crate::home::LOCAL_DIR))),
        }));
    }
    entries.sort_by(|a, b| {
        let (ad, bd) = (a["dir"].as_bool(), b["dir"].as_bool());
        // Case-insensitively, because a picker that sinks every lowercase name
        // below every uppercase one is a picker where `semlith-drive-corpus`
        // is three screens under `Screen Studio Projects`.
        bd.cmp(&ad).then_with(|| {
            let (an, bn) = (
                a["name"].as_str().unwrap_or(""),
                b["name"].as_str().unwrap_or(""),
            );
            an.to_lowercase()
                .cmp(&bn.to_lowercase())
                .then_with(|| an.cmp(bn))
        })
    });

    Response::json(&json!({
        "path": crate::plain(&resolved.display().to_string()),
        "home": crate::plain(&home.display().to_string()),
        "parent": resolved
            .parent()
            .filter(|p| p.starts_with(&home))
            .map(|p| crate::plain(&p.display().to_string())),
        "entries": entries,
    }))
}

/// Whether a directory is a semlith store, by the one file that says so.
fn is_store(dir: &Path) -> bool {
    dir.join("store.db").exists()
}

/// What the Privacy page checks: where this process listens, whether it may
/// fetch anything at all, and where the one download it can make is cached.
fn privacy(state: &Arc<State>) -> Response {
    let cache = crate::model_cache_dir().unwrap_or_default();
    Response::json(&json!({
        "bind": format!("127.0.0.1:{}", state.server.port()),
        "bind_is_fixed": true,
        "airgap": state.airgap,
        "model_cache": crate::plain(&cache.display().to_string()),
        "model_cached": cache.exists()
            && std::fs::read_dir(&cache).map(|mut d| d.next().is_some()).unwrap_or(false),
        "token_header": crate::http::TOKEN_HEADER,
        // Enough of the token to recognise the one this browser holds, and
        // not enough to be one. The full value reaches the page once, in the
        // printed URL, and once more in the body of the rotate response — it is
        // in no other response, and it is in no cookie at all.
        "token_preview": preview(&state.server.token()),
        // How long a rotated agent key still works, if one still does. A
        // number of seconds rather than "until this daemon exits", which on a
        // machine somebody leaves running is not a grace period at all.
        "key_grace_seconds": state.server.key_grace().map(|left| left.as_secs()),
        "host_allowed": ["localhost", "127.0.0.1", "::1"],
        "csp": "default-src 'self'",
        "cors": false,
        "store_home": shown(home::home_or_error()),
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
    let registry = home::Registry::load().unwrap_or_default();

    // The four rules that are readings of this machine live in `doctor`, with
    // the repairs that apply to them, so the page and `semlith doctor --fix`
    // cannot disagree about what is wrong or about what would fix it. The six
    // below are statements about the code — enforced in `http::answer` before
    // any handler runs — so they carry no measurement, no manual step and no
    // button.
    let measured = crate::doctor::privacy_findings(&open_stores(state));
    let measured = |id: &str| {
        measured
            .iter()
            .find(|finding| finding.id == id)
            .expect("every measured rule is reported")
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
        row(
            "private addresses",
            "semlith add resolves every hop and refuses an address that is not                  on the public internet: loopback, RFC 1918, link-local,                  carrier-grade NAT, unique local.",
            measured("private addresses"),
        ),
        {
            "id": "pinned models",
            "rule": "Every model file is fetched at a pinned commit and verified against a                      digest recorded in the source and in docs/models.md. The weights are                      what computes every vector in every store.",
            "check": format!("granite at {}", &crate::embed::GRANITE_REVISION[..12]),
            "ok": true,
        },
        row(
            "model cache",
            "Weights are not loaded from a cache another account owns or can write                  to.",
            measured("model cache"),
        ),
        row(
            "directory modes",
            "The store home, every store, the model cache and daemon.json are                  readable by their owner and nobody else.",
            measured("directory modes"),
        ),
        row(
            "agent key",
            "The key is in one file, readable by you alone. No registration semlith                  writes carries it and no command line shows it: a client launches                  `semlith mcp`, which reads the key from that file itself.",
            measured("agent key"),
        ),
    ])
}

/// Every store the daemon has open, as `doctor` takes them.
///
/// The repairs apply to the stores this page is reporting on, so the list is
/// passed in rather than re-derived: a second assembly of it here is a second
/// answer to which stores are open.
fn open_stores(state: &Arc<State>) -> Vec<(String, std::path::PathBuf)> {
    state
        .stores()
        .iter()
        .map(|s| (s.name.clone(), s.dir.clone()))
        .collect()
}

/// One measured rule as the page renders it.
///
/// `manual` and `repair` are what 0.18.0 added: the command a user would type,
/// on every failing row, and the repair the daemon would apply where one
/// qualifies. A row with `manual` and no `repair` is a rule a person can fix
/// and a process cannot, which is a state the page has to be able to show.
fn row(id: &str, rule: &str, finding: &crate::doctor::Finding) -> Value {
    json!({
        "id": id,
        "rule": rule,
        "check": finding.check,
        "ok": finding.ok,
        "applicable": finding.applicable,
        "manual": finding.manual,
        "repair": finding.repair,
    })
}

/// `semlith doctor`, as a route.
///
/// The same two functions the CLI calls, so the page and the terminal cannot
/// disagree about whether a client is registered or a rule holds. The portal
/// parity rule is why this exists at all: `semlith doctor` is a command, so it
/// has a view in the release that adds it.
fn doctor(state: &Arc<State>) -> Response {
    Response::json(&json!({
        "clients": crate::doctor::clients_report(),
        "rules": crate::doctor::privacy_findings(&open_stores(state)),
        "unregisterable": crate::clients::UNREGISTERABLE,
    }))
}

/// Every file the open stores hold that semlith would refuse today.
///
/// The Privacy page's half of `semlith scan`, and the same function behind it:
/// `Semlith::scan` applies the deny-list to the name and the credential table
/// to the text the store is actually holding. Two implementations of "what
/// should not be here" would eventually disagree, and the one that mattered
/// would be whichever the user did not run.
///
/// Read-only. The Forget buttons beside the rows post to `/api/forget`, which
/// is the daemon's one eviction path — queued through the writer thread like
/// every other write, rather than a second one opened here.
fn privacy_scan(state: &Arc<State>) -> Response {
    if let Err(e) = state.open_fleet() {
        return Response::error(500, &e.to_string());
    }
    let mut fleet = state.fleet.lock().unwrap_or_else(|e| e.into_inner());
    let Some(fleet) = fleet.as_mut() else {
        return Response::json(&json!({ "findings": [] }));
    };
    let mut findings = Vec::new();
    for (label, opened) in fleet.each() {
        match opened.scan() {
            Ok(found) => {
                for finding in found {
                    findings.push(json!({
                        "store": label,
                        // Two spellings of one path, on purpose. `path` is
                        // what a person reads; `key` is what the store is
                        // holding it under, which on Windows carries the
                        // `\\?\` prefix — and a Forget is a lookup, so it has
                        // to be spelled the store's way or it finds nothing.
                        "path": crate::plain(&finding.path),
                        "key": finding.path,
                        "why": finding.why,
                    }));
                }
            }
            Err(e) => return Response::error(500, &format!("{e:#}")),
        }
    }
    let mut answer = json!({ "findings": findings });
    failures_beside(fleet, &mut answer);
    Response::json(&answer)
}

/// Apply one Privacy repair, or every one that qualifies.
///
/// Calls `doctor::apply`, which is what `semlith doctor --fix` calls. The
/// button does not have a repair of its own and cannot have one: a second
/// implementation here is a second answer to what "safe" means, on the page
/// whose whole subject is that question.
///
/// The response carries the rules re-read afterwards, so the page redraws from
/// a measurement rather than assuming the click worked.
fn privacy_fix(state: &Arc<State>, request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let stores = open_stores(state);
    let applied: Vec<Value> = match body.get("rule").and_then(Value::as_str) {
        Some(id) => {
            let Some(repair) = crate::doctor::privacy_findings(&stores)
                .into_iter()
                .find(|finding| finding.id == id)
                .and_then(|finding| finding.repair)
            else {
                return Response::error(400, &format!("{id} has no repair semlith can apply"));
            };
            match crate::doctor::apply(&repair, &stores) {
                Ok(a) => vec![json!(a)],
                Err(e) => return Response::error(400, &e.to_string()),
            }
        }
        None => crate::doctor::apply_all(&stores)
            .into_iter()
            .map(|r| match r {
                Ok(a) => json!(a),
                Err(e) => json!({ "error": e.to_string() }),
            })
            .collect(),
    };
    Response::json(&json!({
        "applied": applied,
        "rules": rules(state),
    }))
}

/// Write the configuration file of every client semlith cannot ask to register
/// itself — the portal's half of `semlith setup --register-all`.
///
/// Two calls, deliberately. Without a body it returns the plan: every path and
/// what would happen to it, which is what the page shows before it asks. With
/// `{"confirm": true}` it applies that plan. This is the one place the daemon
/// writes a file it does not own, and a user who has not seen the list has not
/// agreed to it.
fn register_clients(state: &Arc<State>, request: &Request) -> Response {
    let _ = state;
    let body = request.json().unwrap_or(json!({}));
    let writable: Vec<&crate::clients::Client> = crate::clients::clients()
        .iter()
        .filter(|client| client.needs_a_file_written())
        .collect();
    let plans = crate::clientfile::plan(&writable);

    if body.get("confirm").and_then(Value::as_bool) != Some(true) {
        return Response::json(&json!({ "plan": plans, "confirmed": false }));
    }

    let stanzas: Vec<(&str, &crate::clients::Stanza)> = writable
        .iter()
        .flat_map(|client| {
            client
                .config_files()
                .map(move |stanza| (client.name.as_str(), stanza))
        })
        .collect();
    match crate::clientfile::apply(&plans, &stanzas) {
        Ok(written) => Response::json(&json!({
            "plan": plans,
            "confirmed": true,
            "written": written,
            "clients": crate::doctor::clients_report(),
        })),
        Err(e) => Response::error(500, &e.to_string()),
    }
}

/// A path for a status field, or the reason there is not one.
///
/// Never a placeholder path: a Privacy page that prints `./.semlith` because
/// the home did not resolve is a page stating something untrue about where the
/// user's corpus is (#73).
fn shown(path: anyhow::Result<PathBuf>) -> String {
    match path {
        Ok(p) => crate::plain(&p.display().to_string()),
        Err(e) => format!("unresolved — {e}"),
    }
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
        "binary": crate::plain(&binary.display().to_string()),
        // Measured rather than stated: the size of the file this process was
        // started from.
        "binary_bytes": std::fs::metadata(&binary).map(|m| m.len()).unwrap_or(0),
        "target": format!("{} · {}", std::env::consts::ARCH, std::env::consts::OS),
        "bind": format!("127.0.0.1:{}", state.server.port()),
        "revisions": crate::mcp::SUPPORTED,
        // From the manifest rather than a string in this file, so the page and
        // the crates.io listing cannot come to disagree about the licence.
        "license": env!("CARGO_PKG_LICENSE"),
        // Whether this daemon is recording retrievals. The sidebar states it on
        // every page, the way the design's daemon card does, and asking
        // `/api/ledger` for one boolean would carry the whole ledger with it.
        "ledger": state.ledger,
        "port": state.server.port(),
        "pid": std::process::id(),
        "uptime": daemon::uptime(state),
        "store_home": shown(home::home_or_error()),
        "model_cache": shown(crate::model_cache_dir()),
        "models": TextEmbedding::list_supported_models().len() + 1,
        "languages": LANGUAGES.len(),
        // Which of those languages carry graph edges. The rest are searchable
        // exactly as before and simply have no symbols, which is a different
        // thing from being unsupported.
        "graph_languages": crate::graph::languages(),
        "edge_kinds": crate::graph::KINDS,
        "stores": state.stores().len(),
        // Background while idle, normal while embedding, and what the last
        // switch cost. Read by the About page and by the acceptance check that
        // times each transition.
        "priority": crate::priority::snapshot(),
        // Embedding sessions loaded now: one per writer that has embedded in
        // the last minute, and the one query session every reader shares.
        "sessions": {
            "writers": crate::writer_sessions(),
            "query": crate::fleet::query_sessions(),
        },
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

/// What the tool list costs a session, in tokens, and how that was counted.
///
/// Once per session, before the agent has asked anything: it is the standing
/// charge for having semlith connected at all, and a user should be able to
/// read it rather than capture traffic to discover it.
fn tool_list_tokens(state: &Arc<State>) -> (i64, &'static str) {
    let text = crate::mcp::tool_list()
        .into_iter()
        .map(|(name, about)| format!("{name} {about}"))
        .collect::<Vec<_>>()
        .join(" ");
    let guard = state.fleet.lock().ok();
    let counter = guard
        .as_ref()
        .and_then(|f| f.as_ref())
        .map(|f| f.counter())
        .unwrap_or(crate::ledger::Counter::Chars4);
    let count = counter.count(&text);
    // The whole payload, not only the prose: the schemas are what an agent is
    // sent. The prose is what a tokenizer can be run over honestly, so the
    // count is scaled by the payload's share of it rather than estimated twice.
    let bytes = crate::mcp::tool_list_bytes() as i64;
    let prose = text.len().max(1) as i64;
    (count * bytes / prose, counter.label())
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
        // What the tool list costs an agent, once per session, before it has
        // asked anything. A cost a user should be able to see rather than one
        // they would have to capture traffic to discover.
        "tool_list_bytes": crate::mcp::tool_list_bytes(),
        // And in the unit an agent is billed in. Measured with a store's own
        // tokenizer where one is loaded, and labelled with which counter said
        // so -- the same two tiers the ledger uses, because a token count
        // estimated at four characters each is not the same fact as one a
        // tokenizer produced, and the page says which it is showing.
        "tool_list_tokens": tool_list_tokens(state).0,
        "tool_list_tier": tool_list_tokens(state).1,
        "revisions": crate::mcp::SUPPORTED,
        "clients": crate::clients::clients(),
        // Whether semlith is there without being asked, and since when. The
        // page said "one endpoint, every client" while the endpoint existed
        // only as long as somebody held a terminal open for it.
        "service": {
            "status": crate::service::status(),
            "last_started": crate::service::last_started(),
        },
        // The same four-step report `semlith doctor` prints, from the same
        // function, so the page and the terminal cannot disagree about which
        // client can reach semlith.
        //
        // `disabled_here` is deliberately not surfaced: "here" for this process
        // is wherever a service manager started the daemon, which is nobody's
        // working directory. The page shows `disabled_in` — the directories the
        // override names — because that is the fact, and the reader is the one
        // who knows which of them they work in.
        "doctor": crate::doctor::clients_report(),
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
        "key_path": shown(home::agent_key_path()),
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
        Ok(mut value) => {
            failures_beside(fleet, &mut value);
            Response::json(&value)
        }
        // The whole chain: the outermost message of a store that cannot be read
        // is four words, and the store, the path and the remedy are all below
        // it.
        Err(e) => Response::error(500, &format!("{e:#}")),
    }
}

/// Say which stores an answer could not reach, beside the answer.
///
/// Additive by construction: the key is absent when every store answered, so a
/// reader written against 0.26 sees exactly the payload it saw then. Every
/// route that aggregates over several stores ends with this, which is why the
/// portal has one notice to draw rather than one per page.
fn failures_beside(fleet: &crate::fleet::Fleet, value: &mut Value) {
    let failed = fleet.failed();
    if failed.is_empty() {
        return;
    }
    if let Some(object) = value.as_object_mut() {
        object.insert("failed".into(), json!(failed));
    }
}

/// A tree-sitter structural pattern over the indexed files of one language.
fn pattern(state: &Arc<State>, request: &Request) -> Response {
    let Some(query) = request.query("query").filter(|q| !q.trim().is_empty()) else {
        return Response::error(400, "missing query");
    };
    let Some(lang) = request.query("lang").filter(|l| !l.trim().is_empty()) else {
        return Response::error(400, "missing lang");
    };
    let only = request.query_all("store");
    // The same two arguments the tool and the command take, so the page can
    // narrow a pattern and page past the cap exactly as an agent can.
    let filter = match filter_of(request) {
        Ok(f) => f,
        Err(e) => return Response::error(400, &e),
    };
    let offset = request
        .query("offset")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let empty = json!({ "language": lang, "matches": [], "files": 0, "truncated": false });
    with_fleet(state, empty, move |fleet| {
        let only = (!only.is_empty()).then_some(only);
        // A bad pattern or an unknown language is the caller's to correct, and
        // comes back as the parser's own words rather than an empty list.
        match fleet.pattern_in(only.as_deref(), lang, query, &filter, offset) {
            Ok(found) => Ok(serde_json::to_value(found)?),
            Err(e) => Ok(json!({
                "language": lang,
                "matches": [],
                "files": 0,
                "truncated": false,
                "error": e.to_string(),
            })),
        }
    })
}

/// One span, or the definitions to choose between. The Search page's second
/// stage: the list costs about 150 bytes a hit and this is what turns one of
/// them into the text.
fn read(state: &Arc<State>, request: &Request) -> Response {
    let Some(raw) = request.query("target").filter(|t| !t.trim().is_empty()) else {
        return Response::error(400, "missing target");
    };
    let target = crate::Target::parse(raw);
    let only = request.query_all("store");
    with_fleet(state, json!({ "span": null }), move |fleet| {
        let only = (!only.is_empty()).then_some(only);
        match fleet.read_in(only.as_deref(), &target, &crate::filter::Filter::default())? {
            None => Ok(json!({ "span": null, "definitions": [] })),
            Some(crate::Read::One(span)) => Ok(json!({ "span": span, "definitions": [] })),
            Some(crate::Read::Choose(rows)) => Ok(json!({ "span": null, "definitions": rows })),
        }
    })
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
        let found = fleet.evidence_in(
            only.as_deref(),
            &name,
            &crate::graph::dependency_kinds(),
            k,
            false,
        )?;
        // `symbols` stays where it was so the portal's existing symbol lookup
        // is unchanged; the rest of the block is beside it rather than in
        // place of it.
        Ok(json!({
            "symbols": found.definitions,
            "callers": found.callers,
            "callees": found.callees,
            "ego": found.ego,
        }))
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

/// Everything that reaches one symbol, for the Impact page.
///
/// The mirror of `/api/path`, and it takes the same two controls for the same
/// reason: a caller that crosses a name with several definitions is a guess,
/// so the default refuses and the page says so when it asks anyway.
fn impact(state: &Arc<State>, request: &Request) -> Response {
    let Some(name) = request.query("name") else {
        return Response::error(400, "missing name");
    };
    let name = name.to_string();
    let kinds = request.query_all("kind");
    let depth = request
        .query("depth")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(3)
        .clamp(1, 10);
    let only = request.query_all("store");
    let all_edges = request
        .query("all_edges")
        .is_some_and(|v| v == "1" || v == "true");
    with_fleet(state, json!({ "impact": null }), move |fleet| {
        let only = (!only.is_empty()).then_some(only);
        let impact = fleet.impact_in(only.as_deref(), &name, &kinds, depth, all_edges)?;
        Ok(json!({
            "impact": impact,
            "headline": impact.headline(),
            "limit": crate::graph::IMPACT_LIMIT,
        }))
    })
}

/// A chain as evidence, for the Trace panel.
///
/// Reads the chain `/api/path` would return and the source line behind each
/// hop, so the panel quotes the store rather than the file on disk — the same
/// rule `semlith read` follows and for the same reason.
fn trace(state: &Arc<State>, request: &Request) -> Response {
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
    let all_edges = request
        .query("all_edges")
        .is_some_and(|v| v == "1" || v == "true");
    with_fleet(state, json!({ "trace": null }), move |fleet| {
        let only = (!only.is_empty()).then_some(only);
        let trace = fleet.trace_in(only.as_deref(), &from, &to, depth, all_edges, &crate::plain)?;
        Ok(json!({
            "trace": trace,
            "evidence": trace.evidence(&crate::plain),
        }))
    })
}

/// The communities the Graph page's Map panel lists.
///
/// A list, never a picture: a force layout over a whole corpus is a hairball,
/// and what a reader wants from it is which subsystems exist and what joins
/// them, which reads better as rows.
fn map(state: &Arc<State>, request: &Request) -> Response {
    let only = request.query_all("store");
    let shown = request
        .query("shown")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(crate::graph::COMMUNITIES_SHOWN)
        .clamp(1, 50);
    with_fleet(state, json!({ "communities": [] }), move |fleet| {
        let only = (!only.is_empty()).then_some(only);
        let (communities, total, edges) = fleet.communities_in(only.as_deref(), shown)?;
        Ok(json!({
            "communities": communities,
            "shown": communities.len(),
            "total": total,
            "edges": edges,
        }))
    })
}

/// What the agent did after each answer, from this machine's own transcripts.
///
/// Reads nothing unless the Privacy page's toggle is on. A GET reports the
/// state and, when it is on, the counts; a POST with `{"on": true|false}`
/// sets it. The refusal is the answer rather than an error: a page asking
/// "is this on" must be able to hear "no" without a red box.
fn replay(request: &Request) -> Response {
    if request.method == "POST" {
        let body = match request.json() {
            Ok(b) => b,
            Err(e) => return Response::error(400, &e.to_string()),
        };
        let Some(on) = body.get("on").and_then(Value::as_bool) else {
            return Response::error(400, "missing on");
        };
        let mut saved = home::Settings::load();
        saved.session_replay = Some(on);
        if let Err(e) = saved.save() {
            return Response::error(500, &e.to_string());
        }
        return Response::json(&json!({ "enabled": on }));
    }

    let enabled = home::Settings::load().session_replay.unwrap_or(false);
    let dir = crate::replay::transcripts_dir().ok();
    if !enabled {
        return Response::json(&json!({
            "enabled": false,
            "client": crate::replay::CLIENT,
            "from": dir.map(|d| crate::plain(&d.display().to_string())),
            "sessions": [],
        }));
    }
    let Some(dir) = dir else {
        return Response::error(
            500,
            "this machine has no home directory to read transcripts from",
        );
    };
    match crate::replay::read(&dir, crate::replay::FILES) {
        Ok(found) => Response::json(&json!({
            "enabled": true,
            "client": found.client,
            "from": found.from,
            "skipped": found.skipped,
            "sessions": found.sessions,
        })),
        Err(e) => Response::error(500, &e.to_string()),
    }
}

/// What is inside the index, measured from the stores themselves.
///
/// The `Inside the index` page. Every figure here is counted from the store
/// when this is called — the page's own subtitle promises that, and a cached
/// number behind that sentence would be a lie the page tells about itself.
///
/// Answers per store and totalled, because the reader has several open and
/// the one question they ask first is how big the whole thing is. An
/// unreadable store is left out and named, as everywhere else that reads
/// across the fleet.
fn corpus(state: &Arc<State>) -> Response {
    if let Err(e) = state.open_fleet() {
        return Response::error(500, &format!("{e:#}"));
    }
    let mut fleet = state.fleet.lock().unwrap_or_else(|e| e.into_inner());
    let Some(fleet) = fleet.as_mut() else {
        return Response::json(&json!({ "stores": [] }));
    };
    let mut stores = Vec::new();
    for (label, opened) in fleet.each() {
        match crate::store::corpus(opened.db(), |path| {
            language_of(std::path::Path::new(path)).to_string()
        }) {
            Ok(measured) => {
                let mut row = serde_json::to_value(&measured).unwrap_or_else(|_| json!({}));
                if let Some(map) = row.as_object_mut() {
                    map.insert("store".into(), json!(label));
                }
                stores.push(row);
            }
            // One store that cannot be measured is not the whole page. It is
            // reported as itself, the way an unreadable store is.
            Err(e) => stores.push(json!({ "store": label, "error": format!("{e:#}") })),
        }
    }
    let mut answer = json!({ "stores": stores });
    failures_beside(fleet, &mut answer);
    Response::json(&answer)
}

/// One of the five reports, in one of the five formats, over a window and a
/// scope.
///
/// The same bytes `semlith report` writes, from the same generator: the
/// portal's export is not a second renderer, so a file downloaded from the
/// page and one written in a terminal are the same file.
///
/// `window` and `scope` are both optional and both omitted means exactly what
/// it meant in 0.26.x — every open store, and each report's own span. `scope`
/// is the store filter the route took as `store` and discarded; `store` is
/// still read, as its own alias, because a caller that was already sending it
/// meant it.
fn report(state: &Arc<State>, request: &Request) -> Response {
    let Some(kind) = request.query("kind") else {
        return Response::error(400, "missing kind");
    };
    let kind = kind.to_string();
    let format = request.query("format").unwrap_or("markdown").to_string();
    if !crate::report::ALL_FORMATS.contains(&format.as_str()) && format != "md" {
        return Response::error(
            400,
            &format!(
                "unknown format {format:?}; the formats are {}",
                crate::report::ALL_FORMATS.join(", ")
            ),
        );
    }
    let window = match crate::report::window_of(request.query("window")) {
        Ok(window) => window,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let model = request.query("model").unwrap_or("Sonnet 5").to_string();
    // Refused rather than defaulted. The savings report is a figure in money,
    // and a model nobody prices used to come back priced at Sonnet 5 under
    // Sonnet 5's name, so the caller could not tell it had been ignored.
    if crate::report::price_named(&model).is_none() {
        return Response::error(
            400,
            &format!(
                "no prices for model {model:?} — this binary prices {}",
                crate::report::price_names()
            ),
        );
    }
    let mut only = request.query_all("scope");
    only.extend(request.query_all("store"));

    // The builder's two content toggles. Absent is off, which is what every
    // 0.26.x caller sends.
    let flag = |name: &str| matches!(request.query(name), Some("1" | "true" | "on"));
    let options = crate::report::Options {
        excerpts: flag("excerpts"),
        redact: flag("redact"),
    };

    // A PDF is bytes and cannot ride inside the JSON envelope the other four
    // use, so this one format answers as the file itself. Same generator, same
    // blocks, same window and scope — only the wrapper differs.
    if format == crate::report::PDF {
        if let Err(e) = state.open_fleet() {
            return Response::error(500, &e.to_string());
        }
        let fleet = state.fleet.lock().unwrap_or_else(|e| e.into_inner());
        let Some(fleet) = fleet.as_ref() else {
            return Response::error(409, "no store is open to report on");
        };
        return match crate::report::generate_with(fleet, &kind, &model, window, &only, options)
            .and_then(|report| report.render_bytes(&format))
        {
            Ok(bytes) => Response::new(200, "application/pdf", bytes),
            Err(e) => Response::error(400, &e.to_string()),
        };
    }

    // The body is text of whichever format was asked for, wrapped in JSON so
    // one route answers every format and the page can show a report before
    // deciding to save it.
    with_fleet(state, json!({ "report": null }), move |fleet| {
        let report = crate::report::generate_with(fleet, &kind, &model, window, &only, options)?;
        Ok(json!({
            "kind": report.kind,
            "title": report.title,
            "generated": report.generated,
            "format": format,
            "text": report.render(&format)?,
            "report": report,
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
        // Through the fleet rather than filtered here, so a store that cannot
        // be read is left out of the drawing and reported beside it, and so a
        // request scoped to nothing but unreadable stores is refused.
        let chosen = fleet.selected(only.as_deref())?;
        let many = fleet.len() > 1;
        crate::graph::scoped(&chosen, focus.as_deref(), prefix.as_deref(), limit, many)
    })
}

// ---------------------------------------------------------------- writes

/// Index one or more paths, and answer with the runs that were started.
///
/// This route used to hold an HTTP worker for the whole of a run and stream it
/// as newline-delimited JSON, which made the tab that pressed the button the
/// only thing in the world that knew the run was happening. It now queues the
/// work and returns: the run lives in the daemon, and the page reads it from
/// `/api/index/runs` and `/api/index/log` like anything else. The stream is
/// kept where it is still the right shape — a forwarded `semlith_index`, whose
/// caller is blocking on the answer and never held a worker here.
///
/// Three targets, and no fourth:
///
/// - a named store, as before;
/// - `"each"`, one store per path, resolved exactly as `semlith index <path>`
///   resolves it;
/// - nothing, which means `each` for several paths and the single-store
///   behaviour of today for one.
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
    // Before a store is chosen or made. The route answered 200 for a path that
    // does not exist and left a store behind for it (#76).
    let (paths, unreadable) = crate::check_roots(&paths);
    if !unreadable.is_empty() {
        let named = unreadable
            .iter()
            .map(|(path, why)| format!("{}: {why}", path.display()))
            .collect::<Vec<_>>()
            .join("; ");
        return Response::error(400, &format!("cannot index {named}"));
    }

    let asked = body.get("store").and_then(Value::as_str);
    let each = match asked {
        Some("each") => true,
        Some(_) => false,
        // No store named means the path picks its own, through `home::resolve`
        // — exactly where `semlith index <path>` would have put it. One folder
        // used to go into whichever store happened to be writable, which is
        // both a choice nobody made and half of #120: a path landing in an
        // unrelated store is a path outside that store's roots, and the route
        // then recorded it as a root to make it fit. Several folders were
        // already three corpora for the same reason.
        None => true,
    };

    if each {
        let mut started = Vec::new();
        for path in &paths {
            // One store per path, through `home::resolve` rather than a second
            // copy of its rules — including `free_name`'s suffix for a second
            // `api` — so a store this page makes is byte-for-byte where the
            // command line would have put it.
            let store = match store_for(state, path) {
                Ok(store) => store,
                Err(e) => return Response::error(409, &format!("{}: {e:#}", path.display())),
            };
            match state.index(&store, vec![path.clone()]) {
                Ok((run, _progress)) => started.push(json!({
                    "run": run,
                    "store": store.name,
                    "path": crate::plain(&path.display().to_string()),
                })),
                // Named, and the ones already started are in the answer: a
                // folder that could not be queued should not take the two that
                // were with it.
                Err(e) => started.push(json!({
                    "store": store.name,
                    "path": crate::plain(&path.display().to_string()),
                    "error": e.to_string(),
                })),
            }
        }
        return Response::json(&json!({ "runs": started, "target": "each" }));
    }

    let store = match state.writable(asked) {
        Ok(s) => s,
        // Nothing to write to, and no store named: this is the first run. The
        // daemon was started on a machine with nothing indexed, so the store
        // this path belongs in does not exist yet — make it, exactly as
        // `semlith index` would, and serve it without a restart. Before this,
        // the portal's first-run screen invited a developer to index a folder
        // and then answered the button with "no store is open".
        Err(e) if asked.is_none() && state.stores().is_empty() => {
            match store_for(state, &paths[0]) {
                Ok(store) => store,
                Err(made) => return Response::error(409, &format!("{e}: {made:#}")),
            }
        }
        Err(e) => return Response::error(409, &e.to_string()),
    };

    // The boundary is read *before* the paths are adopted, which is the whole
    // of #120: adopting first made every posted path a root of the store, so
    // the check the run then applied could never refuse anything, and the
    // promise in docs/security.md and docs/compatibility.md was not kept
    // between 0.20.0 and 0.26.0. A path outside the store's roots, outside
    // the store's own directory and outside the home directory is refused
    // here, by name, with the rule that refused it.
    let roots = home::index_roots(&store.dir);
    let outside: Vec<String> = paths
        .iter()
        .filter(|path| !crate::filter::within_boundary(path, &roots))
        .map(|path| crate::plain(&path.display().to_string()))
        .collect();
    if !outside.is_empty() {
        return Response::error(
            403,
            &format!(
                "outside the boundary for store {}: {}. A path must be under one of that \
                 store's registered roots, or under the store's own directory when it is a \
                 `.semlith` beside its corpus. Add it as a root first, or post no store and \
                 let the path have one of its own.",
                store.name,
                outside.join(", ")
            ),
        );
    }

    // Only now. A folder inside the boundary that is added to an existing
    // store becomes one of its roots, so the watcher keeps it current and a
    // later index into it is allowed. Without this the folder is indexed once
    // and then silently stops being watched, which is the shape of a bug
    // nobody reports for a month.
    adopt_roots(&store, &paths);

    match state.index(&store, paths) {
        Ok((run, _progress)) => Response::json(&json!({
            "runs": [{ "run": run, "store": store.name }],
            "target": "store",
        })),
        Err(e) => Response::error(409, &e.to_string()),
    }
}

/// Every store's run, and the queue waiting behind them.
///
/// The standing answer to "is anything indexing and how far is it", from a
/// page or from a script, replacing a stream that only the process which
/// opened it could read. It holds a worker for one request and returns.
fn index_runs(state: &Arc<State>) -> Response {
    let admission = &state.admission;
    let mut runs: Vec<Value> = state
        .stores()
        .iter()
        .flat_map(|store| store.run_snapshots(admission.position_of(&store.name)))
        .collect();
    // Cards of runs whose stop deleted their store, after the live stores'.
    runs.extend(
        state
            .gone
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned(),
    );
    let queue: Vec<Value> = admission
        .waiting()
        .into_iter()
        .enumerate()
        .map(|(at, (run, store, paths))| {
            json!({
                "run": run,
                "store": store,
                "position": at + 1,
                "paths": paths.iter()
                    .map(|p| crate::plain(&p.display().to_string()))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    // Re-read here rather than cached at startup: one of the numbers behind
    // the derivation is how much memory is free *now*.
    let limits = daemon::Limits::in_force();
    Response::json(&json!({
        "runs": runs,
        "queue": queue,
        "running": admission.running(),
        "held": admission.held(),
        "limits": limits,
    }))
}

/// One run's log lines after a cursor.
///
/// A cursor rather than an offset, so two clients reading the same run through
/// their own cursors each see every line exactly once.
fn index_log(state: &Arc<State>, request: &Request) -> Response {
    let Some(name) = request.query("store") else {
        return Response::error(400, "missing store");
    };
    let Some(store) = state.store(name) else {
        return Response::error(404, &format!("no store called {name} is open"));
    };
    let after = request.query("after").and_then(|v| v.parse::<u64>().ok());
    // A store can have two runs, so a caller that knows which one it is
    // reading names it. Without a run, the live one answers — which is what a
    // caller that knows only a store name means.
    let run = request.query("run").and_then(|v| v.parse::<u64>().ok());
    let lines = store.log_after(run, after);
    let last = lines
        .last()
        .and_then(|line| line.get("seq"))
        .and_then(Value::as_u64)
        .or(after);
    Response::json(&json!({ "store": name, "run": run, "lines": lines, "cursor": last }))
}

/// The projects directly under a directory, for the Index page's checklist.
///
/// A git repository is the unit, because it is the unit a developer means by
/// "project" and the one thing on disk that says so. One level only: a
/// monorepo is one store, and its nested repositories are its own business.
/// Where nothing under the directory is a repository, its plain subfolders are
/// offered instead, flagged as such, because a folder of folders is still what
/// the person was pointing at.
///
/// Confined to the user's home exactly as `/api/dirs` is, and for the same
/// reason: this is a browser asking a local server to list a filesystem.
fn projects(request: &Request) -> Response {
    let home = match crate::home::user_home() {
        Ok(home) => crate::canonical(&home),
        Err(e) => return Response::error(500, &e.to_string()),
    };
    let asked = request
        .query("path")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.clone());
    let Ok(resolved) = std::fs::canonicalize(&asked) else {
        return Response::error(400, "that path cannot be resolved");
    };
    if !resolved.starts_with(&home) {
        return Response::error(400, "outside the home directory");
    }

    // The same discovery `semlith index --projects` runs, from the same
    // function, so the checklist and the terminal cannot disagree about what
    // is under a folder.
    let (found, repositories) = match home::projects_under(&resolved) {
        Ok(found) => found,
        Err(e) => return Response::error(400, &format!("{e:#}")),
    };
    let registry = home::Registry::load().unwrap_or_default();
    let rows: Vec<Value> = found
        .iter()
        .map(|path| {
            json!({
                "name": path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                "path": crate::plain(&path.display().to_string()),
                "repository": repositories,
                // Shown and unticked rather than hidden, so a user can see
                // what is already indexed from the same list they choose out
                // of — "nothing here" and "all of it is done" are different
                // answers.
                "indexed": registry.covering(path).map(|(name, _)| name.to_string()),
            })
        })
        .collect();

    Response::json(&json!({
        "path": crate::plain(&resolved.display().to_string()),
        "home": crate::plain(&home.display().to_string()),
        // Where "Up" goes, under the same containment rule `/api/dirs` has.
        // Without it this picker opened at the home directory and could not
        // leave it, so it could not be pointed at a monorepo anywhere else on
        // the machine — while the picker beside it navigated freely.
        "parent": resolved
            .parent()
            .filter(|p| p.starts_with(&home))
            .map(|p| crate::plain(&p.display().to_string())),
        // Repositories where there are any, plain subfolders where there are
        // none. One list either way, so the page has one thing to render.
        "projects": rows,
        // Everything under here that can be navigated into, which is not the
        // same list: a folder of folders offers its subfolders as projects,
        // and a folder of repositories offers none at all.
        "folders": folders(&resolved),
        "repositories": repositories,
    }))
}

/// The directories directly under `dir`, for a picker to drill into.
fn folders(dir: &Path) -> Vec<Value> {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Value> = listing
        .flatten()
        .filter(|entry| {
            entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
                && !entry.file_name().to_string_lossy().starts_with('.')
        })
        .map(|entry| {
            json!({
                "name": entry.file_name().to_string_lossy().into_owned(),
                "path": crate::plain(&entry.path().display().to_string()),
            })
        })
        .collect();
    out.sort_by_key(|row| row["name"].as_str().unwrap_or("").to_lowercase());
    out
}

/// Six integers saying which domains have been written.
///
/// The one clock in the page. Everything that reads live — the Stores rows,
/// the run cards, the Agents counts, the Ledger, the Privacy rules — polls
/// this and refetches only what moved, so a quiet daemon with a tab open costs
/// one small request a second and nothing else.
fn changes(state: &Arc<State>) -> Response {
    // A store another process just made is a change to the stores domain, and
    // nothing in *this* process bumped the counter for it. One `stat` here is
    // what closes the circle; `/api/stores` does the reconciling, once, when
    // the page comes to ask.
    daemon::changes::notice_registry();
    // The same for a root folder deleted or restored underneath a store.
    let stores = state.stores();
    daemon::changes::notice_roots(
        stores
            .iter()
            .flat_map(|store| store.roots.iter().map(PathBuf::as_path)),
    );
    let mut out = serde_json::Map::new();
    for domain in daemon::changes::DOMAINS {
        out.insert(
            domain.as_str().to_string(),
            json!(daemon::changes::read(domain)),
        );
    }
    Response::json(&Value::Object(out))
}

/// Save one or more of the three settings.
///
/// A field the user moved is saved and applies to the next run queued; a field
/// the environment sets is refused, because an explicit variable is an
/// instruction from whoever started the process and a page may not overrule it.
fn index_settings(state: &Arc<State>, request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let mut saved = home::Settings::load();
    let limits = daemon::Limits::in_force();
    // Clamped here and not only on the page. A cap the page knows about and
    // the route does not is a cap that any other caller walks past, and what
    // it guards is a machine that swaps rather than a preference.
    let read = |key: &str, ceiling: usize| {
        body.get(key)
            .and_then(Value::as_u64)
            .map(|n| (n.max(1) as usize).min(ceiling.max(1)))
    };

    if let Some(n) = read("runs_at_once", limits.runs_at_once.ceiling) {
        if limits.runs_at_once.source == daemon::Source::Environment {
            return Response::error(
                409,
                &format!(
                    "{} is set in this daemon's environment, so the page cannot change it",
                    daemon::PARALLEL_ENV
                ),
            );
        }
        saved.runs_at_once = Some(n);
    }
    if let Some(n) = read("embed_threads", limits.embed_threads.ceiling) {
        if limits.embed_threads.source == daemon::Source::Environment {
            return Response::error(
                409,
                &format!(
                    "{} is set in this daemon's environment, so the page cannot change it",
                    crate::embed::THREADS_ENV
                ),
            );
        }
        saved.embed_threads = Some(n);
    }
    if let Some(n) = read("index_memory_mb", limits.index_memory_mb.ceiling) {
        if limits.index_memory_mb.source == daemon::Source::Environment {
            return Response::error(
                409,
                &format!(
                    "{} is set in this daemon's environment, so the page cannot change it",
                    crate::index::INDEX_MEMORY_ENV
                ),
            );
        }
        saved.index_memory_mb = Some(n);
    }
    if let Err(e) = saved.save() {
        return Response::error(500, &format!("{e:#}"));
    }

    // Runs-at-once is the one of the three that means something to a queue
    // already waiting, so it takes effect now rather than at the next start:
    // raising it admits the head immediately.
    // All three take effect now. Threads reach every writer at its next
    // batch, the budget reaches the readers here and every writer at its next
    // tick, and runs-at-once admits or holds at once.
    let limits = daemon::Limits::in_force();
    limits.apply();
    for fleet in [&state.fleet, &state.mcp_fleet] {
        if let Some(fleet) = fleet.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            fleet.follow_budget();
        }
    }
    state.admission.set_limit(limits.runs_at_once.value);
    let applied = format!(
        "now running with {} run(s) at once, {} thread(s) each, and {} MiB of vectors per store",
        limits.runs_at_once.value, limits.embed_threads.value, limits.index_memory_mb.value
    );
    Response::json(&json!({ "limits": limits, "applied": applied }))
}

/// The accelerator lanes: each one's switch, state, device and share of the
/// rate, and what its components take on disk.
fn accel_status() -> Response {
    let mut body = crate::accel::snapshot();
    body["bytes"] = crate::accel::component_bytes();
    Response::json(&body)
}

/// Turn a lane on or off, or remove its components. The same functions
/// `semlith accel` calls, so the page and the terminal refuse the same things.
fn accel_change(request: &Request) -> Response {
    let body = match request.json() {
        Ok(b) => b,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    let Some(lane) = body.get("lane").and_then(Value::as_str) else {
        return Response::error(400, "name the lane: cpu, gpu or cuda");
    };
    let outcome = match body.get("action").and_then(Value::as_str) {
        Some("on") => crate::accel::set(lane, true),
        Some("off") => crate::accel::set(lane, false),
        Some("remove") => crate::accel::remove(lane).map(|bytes| {
            format!(
                "{lane}'s components removed, {} freed",
                crate::human_bytes(bytes as i64)
            )
        }),
        _ => return Response::error(400, "action must be \"on\", \"off\" or \"remove\""),
    };
    match outcome {
        Ok(said) => {
            let mut answer = crate::accel::snapshot();
            answer["said"] = json!(said);
            Response::json(&answer)
        }
        Err(e) => Response::error(409, &format!("{e:#}")),
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
    let run = body.get("run").and_then(Value::as_u64);
    let action = body.get("action").and_then(Value::as_str);
    // A card whose store its own stop deleted has no store to name. Removing
    // it, or clearing finished cards, is answered from the list it lives in.
    if matches!(action, Some("remove") | Some("clear")) {
        let dropped = state.gone_changed(|gone| {
            let before = gone.len();
            match (action, run) {
                (Some("remove"), Some(id)) => {
                    gone.retain(|card| card.get("id").and_then(Value::as_u64) != Some(id))
                }
                _ => gone.clear(),
            }
            before - gone.len()
        });
        if dropped > 0 && action == Some("remove") {
            return Response::json(&json!({ "removed": dropped }));
        }
    }
    let store = match state.writable(body.get("store").and_then(Value::as_str)) {
        Ok(s) => s,
        Err(e) => return Response::error(409, &e.to_string()),
    };
    let mut dequeued = 0;
    let mut removed = 0;
    let mut deleting = false;
    match action {
        // Answered with the state asked for, at once: the engine reaches it
        // at its next batch, and the card moves on when it does.
        Some("pause") => {
            store
                .paused
                .store(true, std::sync::atomic::Ordering::Relaxed);
            store.mark_pausing();
        }
        Some("resume") => {
            store
                .paused
                .store(false, std::sync::atomic::Ordering::Relaxed);
            store.mark_resuming();
        }
        Some("stop") => {
            // A run that has already finished is not one a stop can act on.
            // It used to be accepted: the confirm dialog promised to undo
            // everything the run had embedded, nothing visibly happened, and
            // the store was left carrying a cancellation that killed whatever
            // ran next. Refusing it here is the half of that fix the caller
            // can see.
            if !store.run_live() && state.admission.position_of(&store.name).is_none() {
                return Response::error(
                    409,
                    &format!(
                        "{} has no run to stop — the last one has already finished. \
                         Remove its card instead.",
                        store.name
                    ),
                );
            }
            // The two queues first, in front of the running job: neither has
            // embedded anything, so both are answered from here immediately
            // rather than when the writer eventually reaches them.
            dequeued += state.admission.dequeue(&store.name);
            // Every run taken off the store's queue was already admitted, so
            // its place among the runs that may go at once is given back here.
            // Without this a stop before the writer reached the run leaked a
            // slot, and three of them left the daemon admitting nothing at all.
            let cancelled = store.cancel_queued();
            dequeued += cancelled.len();
            for run in cancelled {
                state.admission.finish(run);
            }
            store
                .cancelled
                .store(true, std::sync::atomic::Ordering::Relaxed);
            // Released, so a paused run reaches the check that stops it.
            store
                .paused
                .store(false, std::sync::atomic::Ordering::Relaxed);
            store.mark_stopping();
            // "Also delete the store", ticked in the dialog. Only ever on the
            // user's explicit word: the page ticks it by default for a store
            // that held nothing before the run, and the route does what the
            // box says rather than guessing.
            if body.get("delete").and_then(Value::as_bool) == Some(true) {
                deleting = true;
                state.delete_after_stop(Arc::clone(&store));
            }
        }
        // Taking a folder out of the queue before it starts. Told apart from
        // `stop` because it costs nothing and undoes nothing: there is no run
        // to unwind, only a place in a line.
        Some("dequeue") => {
            dequeued += state.admission.dequeue(&store.name);
            if dequeued == 0 {
                return Response::error(
                    409,
                    &format!("{} has nothing waiting in the queue", store.name),
                );
            }
        }
        // Dismissing a card that has nothing left to say. Not a stop: there is
        // no run to unwind and nothing in the store changes, which is exactly
        // why the two must not share a button.
        Some("remove") => {
            let Some(id) = run else {
                return Response::error(400, "remove needs the run to remove");
            };
            if !store.remove_run(id) {
                return Response::error(
                    409,
                    &format!("run {id} is not a finished run of {}", store.name),
                );
            }
            removed += 1;
        }
        Some("clear") => removed += store.clear_finished_runs(),
        _ => {
            return Response::error(
                400,
                "action must be \"pause\", \"resume\", \"stop\", \"dequeue\", \"remove\" \
                 or \"clear\"",
            );
        }
    }
    Response::json(&json!({
        "store": store.name,
        "paused": store.paused.load(std::sync::atomic::Ordering::Relaxed),
        "stopping": store.cancelled.load(std::sync::atomic::Ordering::Relaxed),
        "state": match action {
            Some("pause") => "pausing",
            Some("resume") => "running",
            Some("stop") => "stopping",
            _ => "",
        },
        "deleting": deleting,
        "dequeued": dequeued,
        "removed": removed,
    }))
}

/// Record paths as roots of a store that already exists.
///
/// Best-effort and never fatal: a `--store` path and a `.semlith` beside its
/// corpus are deliberately not registered (`home::record` returns early for
/// both), and a run into one of those is still a perfectly good run.
fn adopt_roots(store: &Arc<Store>, paths: &[PathBuf]) {
    let registry = match home::Registry::load() {
        Ok(r) => r,
        Err(_) => return,
    };
    let Some(name) = registry.name_of(&store.dir) else {
        return;
    };
    let choice = home::Choice::Registered {
        name: name.to_string(),
        dir: store.dir.clone(),
    };
    // The model is the store's own; a root recorded against the wrong one
    // would be a store claiming to hold vectors it cannot compare.
    let model = crate::Semlith::open(&store.dir, None)
        .map(|opened| opened.model().to_string())
        .unwrap_or_default();
    let _ = home::record(&choice, paths, &model);
}

/// Create the store `path` belongs in and open it in this daemon.
///
/// The resolution is `home`'s, not a second copy of it, so the portal puts the
/// store exactly where `semlith index <path>` would have — same name, same
/// directory under the store home, same registry entry — and the two ways in
/// cannot disagree about where a corpus lives. That is what makes indexing one
/// folder from the page and again from the terminal produce one store rather
/// than two, and it is why `each` calls this per path instead of naming stores
/// itself.
fn store_for(state: &Arc<State>, path: &Path) -> Result<Arc<Store>, anyhow::Error> {
    use anyhow::Context as _;
    // One at a time. Two requests for the same new folder otherwise both
    // resolve it to the same new name, both create it, both rewrite the
    // registry through the one temporary file this process owns, and the
    // loser of the daemon's store lock answers 409 (#132).
    // ponytail: one lock for every store; per-store locks if creating stores
    // ever becomes frequent enough to queue behind.
    static CREATING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = CREATING.lock().unwrap_or_else(|e| e.into_inner());

    let choice = home::resolve(&[], path, None)
        .with_context(|| format!("choosing a store for {}", path.display()))?;
    let dir = choice.one()?;

    // A store this daemon already serves needs nothing made. Opening it again
    // here was a second connection doing schema work on a store the watcher
    // and a run may be writing, only to reread a model the registry already
    // has. It is the only SQLite call on this path, so it is the open whose
    // short read #132 reported.
    let canonical = crate::canonical(&dir);
    if let Some(open) = state.stores().into_iter().find(|s| s.dir == canonical) {
        return Ok(open);
    }

    // Created before it is opened: `Semlith::open` is what lays the store down,
    // and the daemon can only take a lock on something that exists. The handle
    // is dropped immediately so the watcher thread can take the lock itself.
    {
        let store = crate::Semlith::open(&dir, None)
            .with_context(|| format!("creating the store at {}", dir.display()))?;
        let model = store.model().to_string();
        home::record(&choice, std::slice::from_ref(&path.to_path_buf()), &model)
            .context("recording it in the registry")?;
    }

    // The run that indexes it is submitted by the caller a moment from now, so
    // the watcher holds its catch-up rather than racing that run for the
    // writer and leaving it reporting a corpus it did not index.
    state.open_store(&dir, true)
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

    // The same immediate answer `/api/index` gives, for the same reason: the
    // run is the store's and the page reads it from the snapshot. Only the
    // fetch is synchronous here, because its refusals are what the page shows.
    match state.index(&store, vec![fetched.path.clone()]) {
        Ok((run, _progress)) => Response::json(&json!({
            "runs": [{ "run": run, "store": store.name }],
            "target": "store",
            "fetched": crate::plain(&fetched.path.display().to_string()),
            "url": fetched.url,
        })),
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
        let chunks = value.get("forgot").and_then(Value::as_i64).unwrap_or(0);
        let pictures = value.get("images").and_then(Value::as_i64).unwrap_or(0);
        // One path keeps the answer it has always had, so the MCP tool and
        // every existing caller read the same shape — except that a path the
        // store never held is a 404 rather than a 200 saying zero. A client
        // that reads the status and not the body used to be told the file was
        // gone (#77).
        if single {
            if chunks == 0 && pictures == 0 {
                return Response::error(404, &format!("{path} is not indexed"));
            }
            return Response::json(&value);
        }
        if chunks == 0 && pictures == 0 {
            missing.push(path.clone());
        }
        forgot += chunks;
        images += pictures;
    }
    let kept = paths.len() - missing.len();
    let body = json!({
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
    });
    // A set where nothing was removed is not a success either, and the body
    // already names which paths were not indexed.
    if kept == 0 {
        return Response::new(404, "application/json", body.to_string().into_bytes());
    }
    Response::json(&body)
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
            "deleted": crate::plain(&dir.display().to_string()),
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
            "dir": crate::plain(&dir.display().to_string()),
            "trusted": registry.trusted.iter().map(|d| crate::plain(&d.display().to_string())).collect::<Vec<_>>(),
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
    // The folder that holds the store is as good an answer as the store
    // itself. The CLI takes the store directory — "usually ./.semlith" — and
    // the picker hid dot-directories, so the feature called "Adopt existing
    // .semlith" had no path through the portal that reached one: pointing it
    // at the folder containing the store failed with "no store.db in it".
    let dir = if !is_store(&dir) && is_store(&dir.join(crate::home::LOCAL_DIR)) {
        dir.join(crate::home::LOCAL_DIR)
    } else {
        dir
    };
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
            "dir": crate::plain(&target.display().to_string()),
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

    // The client named itself at `initialize` and never again, so the name the
    // ledger records comes from what was noted then. The transport's session id
    // is the conversation id, which is what it is for.
    let mut mcp_session = crate::mcp::Session::new(session.clone());
    if let Some(named) = state.client_name(&session, transport) {
        mcp_session.client = named;
    }
    let response = match crate::mcp::answer(fleet, Some(&writer), &body, &mut mcp_session) {
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
/// Unless `now` is set, the previous key stays valid for the grace window in
/// `http::KEY_GRACE` and no longer, so a client mid-session finishes its work
/// rather than failing on the call it happened to be making. It is not
/// persisted either, so a restart inside the window also ends it.
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
        .map(|p| crate::plain(&p.display().to_string()))
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

    /// Every path this file hands out is one a person can use.
    ///
    /// `std::fs::canonicalize` on Windows returns the verbatim `\\?\C:\...`
    /// form, which is what the store holds and what the long-path APIs need —
    /// and what no editor opens, no shell completes and nobody pastes back
    /// (#74). The rule is that a path is made plain where it is rendered, and
    /// every route in this file renders into JSON. The store roots were the
    /// one column that had been missed, which is the column that tells a user
    /// what a store indexes.
    #[test]
    fn every_path_this_file_hands_out_is_plain() {
        let source = include_str!("routes.rs");
        let raw: Vec<(usize, &str)> = source
            .lines()
            .enumerate()
            // Assembled rather than written, so this test's own predicate is
            // not a match for itself.
            .filter(|(_, line)| line.contains(&format!("display(){}", ".to_string()")))
            .filter(|(_, line)| !line.contains("crate::plain("))
            // This test's own prose.
            .filter(|(_, line)| !line.trim_start().starts_with("///"))
            .map(|(n, line)| (n + 1, line.trim()))
            .collect();
        assert!(
            raw.is_empty(),
            "these paths go out as JSON without being made plain, so on Windows they carry \
             the verbatim prefix: {raw:#?}"
        );
    }

    /// A correction for a model that no longer exists is a correction that has
    /// silently stopped applying, and the wrong note comes back.
    #[test]
    fn every_corrected_model_note_names_a_model_that_is_listed() {
        let listed: Vec<String> = TextEmbedding::list_supported_models()
            .into_iter()
            .map(|info| info.model.to_string())
            .collect();
        for (name, _) in MODEL_NOTES {
            assert!(
                listed.iter().any(|model| model == name),
                "{name} is corrected here and is not in fastembed's catalogue any more"
            );
        }
    }
}
