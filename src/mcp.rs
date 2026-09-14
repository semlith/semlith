//! A stdio MCP server, so an agent can query and maintain the store as tool
//! calls.
//!
//! Deliberately hand-rolled: MCP over stdio is newline-delimited JSON-RPC 2.0,
//! and what a tools-only server needs fits in one file with no extra dependency.
//!
//! The server speaks two eras of the protocol at once. Every client shipping
//! today opens with an `initialize` handshake and then sends bare requests;
//! revision `2026-07-28` deleted that handshake, made `server/discover`
//! mandatory, and moved the protocol version onto every individual request.
//! Rather than choose an era per connection, each message says which era it
//! belongs to: a request carrying `_meta.io.modelcontextprotocol/protocolVersion`
//! is modern, anything else is legacy. A legacy answer is byte for byte what
//! this server has always sent.
//!
//! Everything written to stdout is protocol. Diagnostics go to stderr.

use crate::filter::Filter;
use crate::fleet::Fleet;
use anyhow::Result;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;

/// The MCP revisions this server implements, newest first.
///
/// Every entry is a promise, so every entry has a session in `tests/mcp.rs`
/// proving it. `2025-03-26` is deliberately absent: it is the one revision that
/// required JSON-RPC batching, and this loop reads one message per line.
pub const SUPPORTED: [&str; 4] = ["2026-07-28", "2025-11-25", "2025-06-18", "2024-11-05"];

/// The revision that carries its version per request instead of shaking hands.
const MODERN: &str = "2026-07-28";

/// The newest revision that still has an `initialize` handshake, and so the
/// right answer when a client asks for one we do not implement.
const LEGACY_NEWEST: &str = "2025-11-25";

/// What a client that names no version at all gets. The field is required, so
/// a client omitting it is old rather than new, and the oldest revision is the
/// one every client can read.
const LEGACY_OLDEST: &str = "2024-11-05";

const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

/// `UnsupportedProtocolVersionError`, from the error range 2026-07-28 reserved
/// for the specification itself.
const UNSUPPORTED_VERSION: i64 = -32022;

/// Freshness hint for the cacheable lists. Neither the tool list nor the
/// server's identity changes while the process lives.
const TTL_MS: u64 = 3_600_000;

/// How many paths `semlith_files` returns before it starts counting instead.
const FILE_LIMIT: usize = 200;

/// How long `semlith_index` works before handing back what it has.
///
/// Clients cut a tool call off at a timeout — Codex documents 60 seconds — and
/// a call killed by the client is indistinguishable from a hung server. This
/// sits under the shortest of those, and the tool says what it did not reach.
const INDEX_BUDGET: Duration = Duration::from_secs(45);

/// `(code, message, data)`. `data` carries the version list on the one error
/// the protocol defines a shape for.
type Fail = (i64, String, Option<Value>);

/// Where a write goes when this process is not the one holding the lock.
///
/// `semlith mcp` in 0.8.0 wrote through its own `Fleet`, which is why an agent
/// and a watcher could not both be pointed at one store. When a daemon is
/// running it is the writer, and the two write tools are handed to it instead;
/// everything else still reads through the `Fleet`, because a reader is allowed
/// to exist alongside the writer and always was.
pub trait Writer: Send + Sync {
    /// `semlith_index`, returning the text the tool reports.
    fn index(&self, store: Option<&str>, paths: &[PathBuf]) -> Result<String, String>;
    /// `semlith_forget`, returning the text the tool reports.
    fn forget(&self, store: Option<&str>, path: &str) -> Result<String, String>;
    /// `semlith_add`, returning the text the tool reports.
    fn add(&self, store: Option<&str>, url: &str) -> Result<String, String>;
}

/// Answer one JSON-RPC request. `None` for a notification, which must not be
/// answered at all.
///
/// The whole protocol lives behind this one function, so a request that arrives
/// over stdio and the same request forwarded over loopback are answered by the
/// same code — which is what makes "every supported revision works through the
/// proxy" true by construction rather than by a second implementation agreeing.
pub fn answer(
    stores: &mut Fleet,
    writer: Option<&dyn Writer>,
    request: &Value,
    session: &mut Session,
) -> Option<Value> {
    let id = request.get("id").cloned()?;
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(json!({}));
    Some(reply(
        &id,
        dispatch(stores, writer, method, &params, session),
    ))
}

/// Who is on the other end of this connection, and which conversation it is.
///
/// The ledger is the reason this exists. Before 0.15.0 a retrieval by an agent
/// was recorded as nothing at all, and the savings figure the product is built
/// on counted only the portal's own search box. A row now says which client
/// asked and which conversation it belonged to, and both of those are facts
/// the protocol already carries — the client names itself in `initialize`, and
/// the transport already has a session.
#[derive(Debug, Clone)]
pub struct Session {
    /// The client's own name for itself, from `clientInfo.name`.
    ///
    /// `mcp` until the handshake says otherwise, which covers a client that
    /// sends no `clientInfo` and one that starts calling tools without
    /// initializing at all.
    pub client: String,
    pub id: String,
}

impl Session {
    pub fn new(id: impl Into<String>) -> Self {
        Session {
            client: "mcp".to_string(),
            id: id.into(),
        }
    }

    fn who(&self) -> crate::ledger::Who<'_> {
        crate::ledger::Who {
            client: &self.client,
            session: &self.id,
        }
    }
}

/// Read requests from `input` until EOF, answering on `output`.
pub fn serve(stores: &mut Fleet, input: impl BufRead, mut output: impl Write) -> Result<()> {
    stores.quiet = true;
    // One connection is one conversation, so the id is made once here and every
    // row this client writes carries it.
    let mut session = Session::new(format!("stdio-{}", std::process::id()));

    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                respond(
                    &mut output,
                    &json!(null),
                    Err((-32700, e.to_string(), None)),
                )?;
                continue;
            }
        };

        // Notifications carry no id and must not be answered.
        let Some(id) = req.get("id").cloned() else {
            continue;
        };
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(json!({}));

        let result = dispatch(stores, None, method, &params, &mut session);
        respond(&mut output, &id, result)?;
    }
    Ok(())
}

/// The revision a modern request declares, if it is one.
fn declared_version(params: &Value) -> Option<&str> {
    params.get("_meta")?.get(META_VERSION)?.as_str()
}

fn server_info() -> Value {
    json!({ "name": "semlith", "version": env!("CARGO_PKG_VERSION") })
}

/// Add what every modern result carries. Legacy results are left exactly as
/// they were, because an agent already connected must see no change.
fn modernize(mut result: Value, cacheable: Option<&str>) -> Value {
    result["resultType"] = json!("complete");
    result["_meta"] = json!({ META_SERVER_INFO: server_info() });
    if let Some(scope) = cacheable {
        result["ttlMs"] = json!(TTL_MS);
        result["cacheScope"] = json!(scope);
    }
    result
}

fn dispatch(
    stores: &mut Fleet,
    writer: Option<&dyn Writer>,
    method: &str,
    params: &Value,
    session: &mut Session,
) -> Result<Value, Fail> {
    let declared = declared_version(params);

    // Checked before the method runs, so a client on a revision we do not
    // implement is told once, plainly, rather than served something that only
    // resembles what it asked for.
    if let Some(version) = declared
        && !SUPPORTED.contains(&version)
    {
        return Err((
            UNSUPPORTED_VERSION,
            "Unsupported protocol version".into(),
            Some(json!({ "supported": SUPPORTED, "requested": version })),
        ));
    }
    let modern = declared == Some(MODERN);

    match method {
        // Mandatory in the modern era, and the probe a dual-era client opens
        // with on stdio: there is no HTTP status code here to fall back on, so
        // this answer is how a client tells a 2026-07-28 server from a 2025 one.
        "server/discover" => Ok(modernize(
            json!({
                "supportedVersions": SUPPORTED,
                "capabilities": { "tools": {} },
                "instructions":
                    "Search and maintain the local semlith stores this server was opened on. \
                     Call semlith_stats first to learn the store names the other tools accept.",
            }),
            Some("public"),
        )),

        "initialize" => {
            // The client names itself here or nowhere. `claude-code`, `cursor`,
            // `codex` — the ledger records exactly this string rather than a
            // display name, so what the table says matches what the client
            // calls itself in its own configuration.
            if let Some(name) = params
                .get("clientInfo")
                .and_then(|c| c.get("name"))
                .and_then(Value::as_str)
                .filter(|n| !n.trim().is_empty())
            {
                session.client = name.to_string();
            }
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = negotiate(asked);
            // "The agent sees no tools" is otherwise undiagnosable from the
            // client's side, and stderr is the one channel a stdio client
            // captures for exactly this.
            match asked {
                Some(a) if a != version => {
                    eprintln!("semlith: client asked for MCP {a}; answering {version}")
                }
                _ => eprintln!("semlith: MCP {version}"),
            }
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": server_info(),
            }))
        }

        // Removed by 2026-07-28, still sent by legacy clients. Answering it
        // costs one line; refusing it costs somebody a connection.
        "ping" => Ok(json!({})),

        "tools/list" => {
            let listed = json!({ "tools": tools(stores) });
            // Private, not public: the descriptions name the stores this
            // process was opened on, so the answer is this server's, not one
            // an intermediary may hand to another client.
            Ok(if modern {
                modernize(listed, Some("private"))
            } else {
                listed
            })
        }

        "tools/call" => {
            let called = call_tool(stores, writer, params, session)?;
            Ok(if modern {
                modernize(called, None)
            } else {
                called
            })
        }

        other => Err((-32601, format!("unknown method: {other}"), None)),
    }
}

/// The revision to answer a handshake with.
///
/// A server that echoes what it was asked claims every revision that exists,
/// including the ones that deleted the handshake it is answering. What the
/// protocol asks for instead is the truth: the same version when we implement
/// it, and otherwise the newest one we do.
fn negotiate(requested: Option<&str>) -> &'static str {
    match requested {
        Some(v) => SUPPORTED
            .iter()
            .find(|s| **s == v && **s != MODERN)
            .copied()
            .unwrap_or(LEGACY_NEWEST),
        None => LEGACY_OLDEST,
    }
}

fn tools(stores: &Fleet) -> Value {
    // An agent cannot narrow to a store whose name it has never seen, so the
    // open stores are part of the tool description rather than something to
    // discover by trial.
    tool_defs(&stores.labels().join(", "))
}

/// Every tool name this server serves.
///
/// The portal's Agents page reads this rather than repeating the list. Two
/// hand-written copies of a tool surface in two files is precisely how a tool
/// ends up served by the server and invisible in the portal, which the parity
/// rule exists to prevent — so there is only one copy.
pub fn tool_names() -> Vec<String> {
    tool_list().into_iter().map(|(name, _)| name).collect()
}

/// Every tool, with the one-line purpose its definition advertises.
///
/// The Agents page lists these. Reading the description out of the definition
/// rather than writing a second one beside it is what stops a tool being
/// served with one description and documented with another — the title an
/// annotation carries is the sentence a client shows a user, so it is the one
/// the portal shows too.
pub fn tool_list() -> Vec<(String, String)> {
    tool_defs("")
        .as_array()
        .map(|defs| {
            defs.iter()
                .filter_map(|tool| {
                    let name = tool["name"].as_str()?.to_string();
                    let about = tool["annotations"]["title"]
                        .as_str()
                        .or_else(|| tool["description"].as_str())
                        .unwrap_or_default()
                        .to_string();
                    Some((name, about))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// How many bytes `tools/list` is, for the harness that gates on it.
///
/// Exposed rather than recomputed in the test, so the number the gate reads is
/// the number the server sends.
pub fn tool_list_bytes() -> usize {
    serde_json::to_string(&tool_defs("default"))
        .map(|json| json.len())
        .unwrap_or(0)
}

/// How many bytes a locate reply for these hits would be.
///
/// The harness measures what an agent is actually handed, which is this and
/// not the `Hit` list behind it.
pub fn locate_bytes(hits: &[crate::Hit], query: &str) -> usize {
    locate(hits, query, DEFAULT_LOCATE_TOKENS).len()
}

fn tool_defs(open: &str) -> Value {
    let store_arg = format!("Open: {open}.");
    let write_store_arg = format!("Open: {open}. Required when several are.");

    // One short sentence per tool, and nothing at all on an argument whose
    // name already says what it is.
    //
    // 0.14.0's tool list was 8 955 bytes, about 2 200 tokens, and every agent
    // paid it once per session before asking anything. Most of it was advice —
    // when to prefer this tool over a grep, what a filter does to a result —
    // which a model either already knows or will not follow from a schema.
    // There are no `title` annotations either: a title that restates the
    // description is a second copy of it, and the portal's Agents page falls
    // back to the description for exactly this reason. The behavioural hints
    // stay, because a client acts on those.
    //
    // What is left is what a caller cannot guess: the defaults, the one filter
    // that can hide an answer, and the four words that say an edge may be
    // wrong. `the_tool_list_stays_small` keeps the rest from growing back a
    // paragraph at a time.
    json!([
        {
            "name": "semlith_search",
            "description": "Semantic + keyword search. Returns where; format excerpt adds the text.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "k": { "type": "integer", "description": "Default 8.", "minimum": 1, "maximum": 50 },
                    "format": { "type": "string", "enum": ["locate", "excerpt"], "description": "Default locate." },
                    "max_tokens": { "type": "integer", "description": "Default 1500.", "minimum": 200 },
                    "path": { "type": "array", "items": { "type": "string" }, "description": "Globs; a wrong guess hides the answer." },
                    "ext": { "type": "array", "items": { "type": "string" } },
                    "lang": { "type": "array", "items": { "type": "string" }, "description": "See semlith_languages." },
                    "prefer": { "type": "string", "enum": ["code", "docs", "any"], "description": "Lift code or prose. Default any." },
                    "store": { "type": "array", "items": { "type": "string" }, "description": store_arg }
                },
                "required": ["query"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "semlith_stats",
            "description": "What each open store holds: files, chunks, bytes, model, ledger totals.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "semlith_languages",
            "description": "The languages lang accepts, and which carry edges.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "semlith_files",
            "description": "List indexed files: \"not indexed\" is not \"not discussed\".",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "array", "items": { "type": "string" } },
                    "ext": { "type": "array", "items": { "type": "string" } },
                    "lang": { "type": "array", "items": { "type": "string" } },
                    "store": { "type": "array", "items": { "type": "string" } },
                    "limit": { "type": "integer" }
                }
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "semlith_index",
            "description": "Index paths. Only what changed is re-embedded.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "array", "items": { "type": "string" } },
                    "store": { "type": "string", "description": write_store_arg }
                },
                "required": ["path"]
            },
        },
        {
            "name": "semlith_add",
            "description": "Fetch one https URL into the store. No crawling. Refused under --airgap.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string" },
                    "store": { "type": "string", "description": write_store_arg }
                },
                "required": ["url"]
            },
            "annotations": { "openWorldHint": true }
        },
        {
            "name": "semlith_forget",
            "description": "Drop one file from a store. The file on disk is untouched.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "store": { "type": "string" }
                },
                "required": ["path"]
            },
            "annotations": { "destructiveHint": true }
        },
        {
            "name": "semlith_symbol",
            "description": "Where a symbol is defined, from the syntax tree rather than matched text.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "k": { "type": "integer", "description": "Default 20." },
                    "store": { "type": "string" }
                },
                "required": ["name"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "semlith_neighbors",
            "description": "What calls a symbol and what it calls. Edges are extracted, resolved, inferred or ambiguous; only the first two are certain.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "kind": { "type": "array", "items": { "type": "string" } },
                    "all": { "type": "boolean", "description": "Expand collapsed rows; list targets this store lacks." },
                    "store": { "type": "string" }
                },
                "required": ["name"]
            },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "semlith_path",
            "description": "The shortest chain between two symbols, or a statement that there is none. An ambiguous name is not crossed unless you ask.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from": { "type": "string" },
                    "to": { "type": "string" },
                    "depth": { "type": "integer", "description": "Default 6." },
                    "all_edges": { "type": "boolean", "description": "Cross them; the answer is then a hypothesis." },
                    "strict": { "type": "boolean" },
                    "store": { "type": "string" }
                },
                "required": ["from", "to"]
            },
            "annotations": { "readOnlyHint": true }
        }
    ])
}

fn call_tool(
    stores: &mut Fleet,
    writer: Option<&dyn Writer>,
    params: &Value,
    session: &Session,
) -> Result<Value, Fail> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let started = std::time::Instant::now();

    let body = match name {
        "semlith_search" => {
            let Some(query) = args.get("query").and_then(Value::as_str) else {
                return Err((-32602, "missing required argument: query".into(), None));
            };
            let k = args.get("k").and_then(Value::as_u64).unwrap_or(8) as usize;

            let filter = match filter_of(&args) {
                Ok(f) => f,
                // An unknown language is the agent's mistake to correct, so it
                // goes back in-band with the list rather than as a protocol error.
                Err(e) => return Ok(tool_error(&e)),
            };

            // A store name the agent guessed is the agent's mistake to correct,
            // so it comes back in-band with the names that exist.
            let only = strings(&args, "store");

            // Told apart because an agent that scoped to the wrong subsystem
            // should widen the filter, not conclude the corpus is empty. A
            // failure here is a broken store, not an empty selection, and must
            // not be reported as one.
            let selected = if filter.is_empty() {
                1
            } else {
                match stores.matching_files(&filter) {
                    Ok(n) => n,
                    Err(e) => return Ok(tool_error(&format!("search failed: {e}"))),
                }
            };

            if selected == 0 {
                "No indexed file matches that path/ext/lang filter. Try again without it."
                    .to_string()
            } else {
                // Locate by default from 0.15.0. `format: "excerpt"` is the
                // opt-back, and is what the CLI still does.
                let excerpts = args
                    .get("format")
                    .and_then(Value::as_str)
                    .is_some_and(|f| f.eq_ignore_ascii_case("excerpt"));
                let max_tokens = args
                    .get("max_tokens")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize)
                    .unwrap_or(DEFAULT_LOCATE_TOKENS);
                let prefer = match args.get("prefer").and_then(Value::as_str) {
                    Some(raw) => match crate::Prefer::parse(raw) {
                        Ok(p) => p,
                        Err(e) => return Ok(tool_error(&e.to_string())),
                    },
                    None => crate::Prefer::default(),
                };
                match stores.search_preferring(Some(&only), query, k.clamp(1, 50), &filter, prefer)
                {
                    Ok(hits) if hits.is_empty() => "No matches in the semlith store.".to_string(),
                    Ok(hits) if excerpts => {
                        format!("{}\n{}", reading(query, prefer), render(&hits))
                    }
                    Ok(hits) => format!(
                        "{}\n{}",
                        reading(query, prefer),
                        locate(&hits, query, max_tokens)
                    ),
                    // Tool failures are reported in-band so the agent can react,
                    // rather than as a protocol-level error.
                    Err(e) => return Ok(tool_error(&e.to_string())),
                }
            }
        }
        "semlith_stats" => {
            let many = stores.len() > 1;
            let mut lines = Vec::new();
            for (label, store) in stores.each() {
                let (files, chunks, bytes) = match store.stats() {
                    Ok(s) => s,
                    Err(e) => return Ok(tool_error(&format!("stats failed: {e}"))),
                };
                let body = format!(
                    "{files} files, {chunks} chunks, {} indexed, model {} ({} dim)",
                    crate::human_bytes(bytes),
                    store.model(),
                    store.dim(),
                );
                // One store answers exactly as it did before stores could be
                // combined; a name in front of it would only cost tokens.
                lines.push(if many {
                    format!("{label}: {body}")
                } else {
                    body
                });
            }
            lines.join("\n")
        }
        "semlith_files" => {
            let filter = match filter_of(&args) {
                Ok(f) => f,
                Err(e) => return Ok(tool_error(&e)),
            };
            let only = strings(&args, "store");
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .map(|n| n as usize)
                .unwrap_or(FILE_LIMIT)
                .max(1);

            match stores.paths_in(Some(&only), &filter, limit) {
                Ok((paths, _)) if paths.is_empty() => {
                    "No file in the semlith store matches that.".to_string()
                }
                Ok((paths, left_out)) => {
                    let mut out = paths.join("\n");
                    // A truncated list that does not say so is how an agent
                    // decides a file it cannot see was never indexed.
                    if left_out > 0 {
                        out.push_str(&format!(
                            "\n… and {left_out} more. Narrow with path/ext/lang, or raise limit."
                        ));
                    }
                    out
                }
                Err(e) => return Ok(tool_error(&e.to_string())),
            }
        }
        "semlith_index" => {
            let roots: Vec<PathBuf> = strings(&args, "path").iter().map(PathBuf::from).collect();
            if roots.is_empty() {
                return Err((-32602, "missing required argument: path".into(), None));
            }
            if let Some(writer) = writer {
                let named = strings(&args, "store");
                return Ok(
                    match writer.index(named.first().map(String::as_str), &roots) {
                        Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
                        Err(e) => tool_error(&e),
                    },
                );
            }

            let store = match stores.writable(&strings(&args, "store")) {
                Ok(s) => s,
                Err(e) => return Ok(tool_error(&e.to_string())),
            };

            // The key that opens this tool lives in a config file on disk, so
            // what it can reach is what a copied config file can reach. An
            // agent may index this store's own roots and the home directory,
            // and never a credential by name. `semlith index` on the command
            // line is not held to the boundary — the person typing it is the
            // owner of the machine.
            store.boundary = crate::Boundary::within(crate::home::index_roots(store.dir()));

            match store.index_paths_within(&roots, index_budget(), |_, _| {}) {
                // A store another process is writing is a conflict to report,
                // not an error to fail the call with: the agent can wait, or
                // write somewhere else.
                Err(e) => return Ok(tool_error(&e.to_string())),
                Ok(report) => {
                    let mut out = format!(
                        "{} indexed, {} unchanged, {} skipped, {} removed ({} chunks)",
                        report.indexed,
                        report.unchanged,
                        report.skipped,
                        report.removed,
                        report.chunks,
                    );
                    if report.remaining > 0 {
                        out.push_str(&format!(
                            "\nStopped at the time limit with {} paths remaining. \
                             Call semlith_index again with the same arguments to continue; \
                             nothing already indexed is redone.",
                            report.remaining
                        ));
                    }
                    // Named, one per line, with the rule that refused each. A
                    // refusal reported as a count is one an agent retries.
                    for (path, why) in &report.refused {
                        out.push_str(&format!("\nrefused: {path} — {why}"));
                    }
                    out
                }
            }
        }
        "semlith_add" => {
            // A bare string is what the schema asks for; an agent sends a
            // one-element array often enough that refusing it teaches nothing.
            let urls = strings(&args, "url");
            let url = match urls.as_slice() {
                [one] => one.clone(),
                [] => return Err((-32602, "missing required argument: url".into(), None)),
                many => {
                    return Ok(tool_error(&format!(
                        "semlith_add fetches one URL, not {}: {}",
                        many.len(),
                        many.join(", ")
                    )));
                }
            };
            let named = strings(&args, "store");

            if let Some(writer) = writer {
                return Ok(match writer.add(named.first().map(String::as_str), &url) {
                    Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
                    Err(e) => tool_error(&e),
                });
            }

            let store = match stores.writable(&named) {
                Ok(s) => s,
                Err(e) => return Ok(tool_error(&e.to_string())),
            };

            // A refused fetch is the agent's answer, not a protocol failure:
            // the URL was http, or too large, or a type with no reader, and the
            // agent can act on any of those.
            let fetched = match crate::add::fetch(&url, store.dir()) {
                Ok(fetched) => fetched,
                Err(e) => return Ok(tool_error(&format!("{e:#}"))),
            };
            match store.index_paths_within(
                std::slice::from_ref(&fetched.path),
                index_budget(),
                |_, _| {},
            ) {
                Err(e) => return Ok(tool_error(&e.to_string())),
                Ok(report) => format!(
                    "Fetched {} into {} and indexed {} chunks.",
                    fetched.url,
                    fetched.path.display(),
                    report.chunks
                ),
            }
        }
        "semlith_forget" => {
            // A bare string is what the schema asks for, but an agent sends a
            // one-element array often enough that refusing it teaches nothing.
            let paths = strings(&args, "path");
            let path = match paths.as_slice() {
                [one] => one.clone(),
                [] => return Err((-32602, "missing required argument: path".into(), None)),
                many => {
                    return Ok(tool_error(&format!(
                        "semlith_forget removes one file, not {}: {}",
                        many.len(),
                        many.join(", ")
                    )));
                }
            };
            if let Some(writer) = writer {
                let named = strings(&args, "store");
                return Ok(
                    match writer.forget(named.first().map(String::as_str), &path) {
                        Ok(text) => json!({ "content": [{ "type": "text", "text": text }] }),
                        Err(e) => tool_error(&e),
                    },
                );
            }

            let store = match stores.writable(&strings(&args, "store")) {
                Ok(s) => s,
                Err(e) => return Ok(tool_error(&e.to_string())),
            };
            match store.forget(std::path::Path::new(&path)) {
                Ok((0, 0)) => format!("{path} was not indexed; nothing removed."),
                Ok((0, images)) => format!("Removed {images} image vector(s) for {path}."),
                Ok((chunks, 0)) => format!("Removed {chunks} chunks for {path}."),
                Ok((chunks, images)) => {
                    format!("Removed {chunks} chunks and {images} image vector(s) for {path}.")
                }
                Err(e) => return Ok(tool_error(&e.to_string())),
            }
        }
        "semlith_symbol" => {
            let Some(name) = args.get("name").and_then(Value::as_str) else {
                return Err((-32602, "missing required argument: name".into(), None));
            };
            let k = args.get("k").and_then(Value::as_u64).unwrap_or(20) as usize;
            let only = strings(&args, "store");
            match stores.symbols_in(Some(&only), name, k.clamp(1, 200)) {
                Ok(found) if found.is_empty() => empty_graph(stores, name),
                Ok(found) => found
                    .iter()
                    .map(|s| {
                        format!(
                            "{} ({}) {}{}:{}-{}",
                            s.name,
                            s.kind,
                            label_of(&s.store),
                            s.path,
                            s.start_line,
                            s.end_line
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                Err(e) => return Ok(tool_error(&e.to_string())),
            }
        }
        "semlith_languages" => {
            // The list the `lang` filter accepts, which is why it is no longer
            // spelled out in that argument's description: it is a fact about
            // this build, and a tool can state it once instead of every agent
            // reading it once per session.
            let with_edges: Vec<&str> = crate::graph::LANGUAGES.to_vec();
            let names: Vec<String> = crate::filter::LANGUAGES
                .iter()
                .map(|entry| {
                    if with_edges.contains(&entry.name) {
                        format!("{} (graph)", entry.name)
                    } else {
                        entry.name.to_string()
                    }
                })
                .collect();
            format!(
                "{} languages. Those marked (graph) also carry symbols and edges, so \
                 semlith_symbol, semlith_neighbors and semlith_path work on them.\n{}",
                names.len(),
                names.join(", "),
            )
        }
        "semlith_neighbors" => {
            let Some(name) = args.get("name").and_then(Value::as_str) else {
                return Err((-32602, "missing required argument: name".into(), None));
            };
            let kinds = strings(&args, "kind");
            if let Some(bad) = kinds
                .iter()
                .find(|k| !crate::graph::KINDS.contains(&k.as_str()))
            {
                return Ok(tool_error(&format!(
                    "unknown edge kind {bad:?}; the kinds are {}",
                    crate::graph::KINDS.join(", ")
                )));
            }
            let only = strings(&args, "store");
            let all = args.get("all").and_then(Value::as_bool).unwrap_or(false);
            match stores.neighbours_in(Some(&only), name, &kinds, all) {
                // Only when there is genuinely nothing. A symbol whose every
                // target lies outside the corpus has something to say, and
                // saying "the graph does not have this" about it is the exact
                // confusion the hidden count exists to end.
                Ok(n)
                    if n.callers.is_empty()
                        && n.callees.is_empty()
                        && n.hidden == 0
                        && n.unresolved.is_empty() =>
                {
                    empty_graph(stores, name)
                }
                Ok(n) => {
                    let mut body = format!(
                        "callers of {name} ({}):\n{}\n\ncallees of {name} ({}):\n{}",
                        n.callers.len(),
                        render_ends(&n.callers),
                        n.callees.len(),
                        render_ends(&n.callees),
                    );
                    if n.hidden > 0 {
                        body.push_str(&format!(
                            "\n\n{} target{} outside this store, not listed. Ask with all: true.",
                            n.hidden,
                            if n.hidden == 1 { "" } else { "s" },
                        ));
                    }
                    if !n.unresolved.is_empty() {
                        let outside = n
                            .unresolved
                            .iter()
                            .map(|u| format!("  {} via {}", u.name, u.kind))
                            .collect::<Vec<_>>()
                            .join("\n");
                        body.push_str(&format!("\n\noutside this store:\n{outside}"));
                    }
                    body
                }
                Err(e) => return Ok(tool_error(&e.to_string())),
            }
        }
        "semlith_path" => {
            let (Some(from), Some(to)) = (
                args.get("from").and_then(Value::as_str),
                args.get("to").and_then(Value::as_str),
            ) else {
                return Err((-32602, "missing required arguments: from, to".into(), None));
            };
            let depth = args.get("depth").and_then(Value::as_u64).unwrap_or(6) as u32;
            let only = strings(&args, "store");
            // `strict` is the default and the argument is the way to say so
            // out loud; `all_edges` is what turns it off. A client that sends
            // both is asking for the refusal it named explicitly.
            let strict = args.get("strict").and_then(Value::as_bool).unwrap_or(false);
            let all_edges = args
                .get("all_edges")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                && !strict;
            let depth = depth.clamp(1, 20);
            match stores.path_in(Some(&only), from, to, depth, all_edges) {
                Ok(Some(chain)) if chain.steps.is_empty() => format!("{from} is {to}."),
                Ok(Some(chain)) => chain.render("", "", &crate::graph::verbatim),
                Ok(None) => crate::graph::not_connected(from, to, depth, all_edges),
                Err(e) => return Ok(tool_error(&e.to_string())),
            }
        }
        other => return Err((-32602, format!("unknown tool: {other}"), None)),
    };

    record(stores, session, name, &args, &body, started.elapsed());
    Ok(json!({ "content": [{ "type": "text", "text": body }] }))
}

/// Write this tool call into the ledger, if it was a retrieval.
///
/// Every read an agent makes is recorded here from 0.15.0. Until then the only
/// caller was the portal's own search box, which meant the savings figure the
/// product is built on counted the one user who was not the point.
///
/// Indexing, adding and forgetting are writes, not retrievals, and are not
/// recorded: the ledger answers "what did an agent read instead of reading
/// files", and those three answer a different question. `semlith_stats` and
/// `semlith_languages` are not retrievals either — they are questions about
/// the tool rather than about the corpus.
///
/// Failures here are swallowed exactly as they were in `routes.rs`: a store
/// that cannot be written to must not turn a successful answer into an error
/// the agent sees.
fn record(
    stores: &Fleet,
    session: &Session,
    tool: &str,
    args: &Value,
    body: &str,
    elapsed: std::time::Duration,
) {
    let who = session.who();
    match tool {
        "semlith_search" => {
            // The hits are gone by now — `render` and `locate` consume them —
            // so the row is built from the reply, which is what the agent was
            // actually handed and therefore what it actually cost.
            let Some(query) = args.get("query").and_then(Value::as_str) else {
                return;
            };
            crate::ledger::reply(stores, &who, "search", query, body, elapsed);
        }
        "semlith_neighbors" | "semlith_path" | "semlith_symbol" => {
            // `path` is asked about two names and the first is the one the
            // question is about, which is the one a grep would have started
            // from.
            let Some(subject) = args
                .get("name")
                .or_else(|| args.get("from"))
                .and_then(Value::as_str)
            else {
                return;
            };
            let short = tool.trim_start_matches("semlith_");
            let found = !body.contains("not connected") && !body.contains("nothing");
            crate::ledger::graph(stores, &who, short, subject, body, found, elapsed);
        }
        _ => {}
    }
}

/// What to say when the graph has nothing for a name.
///
/// An empty graph and an absent symbol read identically to an agent, and only
/// one of them means "this corpus has not been indexed since the graph
/// existed". Telling them apart is the difference between the agent running
/// `semlith_index` and concluding the symbol is not there.
fn empty_graph(stores: &Fleet, name: &str) -> String {
    let symbols: i64 = stores
        .each()
        .map(|(_, store)| {
            crate::store::graph_stats(store.db())
                .map(|(s, _)| s)
                .unwrap_or(0)
        })
        .sum();
    if symbols == 0 {
        "This store has no symbols yet: its graph is built as files are indexed. Call \
         semlith_index on the corpus, then ask again."
            .to_string()
    } else {
        format!(
            "No symbol named {name} in the graph. Only Rust, TypeScript, Python, Go, Java \
             and C carry symbols; semlith_search still finds text in everything else."
        )
    }
}

fn label_of(store: &Option<String>) -> String {
    match store {
        Some(label) => format!("[{label}] "),
        None => String::new(),
    }
}

fn render_ends(ends: &[crate::store::EdgeEnd]) -> String {
    if ends.is_empty() {
        return "  none".to_string();
    }
    ends.iter()
        .map(|e| {
            // A collapsed row stands for several definitions and must not name
            // one of them: an agent reading `get  src/a.rs:12` will quote that
            // file, and it is one of four the call could have meant.
            if e.confidence == crate::graph::AMBIGUOUS {
                return format!(
                    "  {} via {} ({}) · {} definitions{}",
                    e.symbol.name,
                    e.kind,
                    e.confidence,
                    e.definitions,
                    crate::graph::call_site(e, &crate::graph::verbatim)
                );
            }
            format!(
                "  {} via {} ({})  {}{}:{}{}",
                e.symbol.name,
                e.kind,
                e.confidence,
                label_of(&e.symbol.store),
                e.symbol.path,
                e.symbol.start_line,
                crate::graph::call_site(e, &crate::graph::verbatim)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// How long an index tool call may work for.
///
/// Overridable because clients disagree about how long a tool call may take,
/// and the person who knows which client this is, is the one who wrote its
/// configuration — not the model on the other end of the tool.
fn index_budget() -> Duration {
    match std::env::var("SEMLITH_MCP_INDEX_BUDGET") {
        Ok(v) => match v.trim().parse::<u64>() {
            Ok(seconds) => Duration::from_secs(seconds),
            Err(_) => INDEX_BUDGET,
        },
        Err(_) => INDEX_BUDGET,
    }
}

/// The path/ext/lang narrowing three tools share.
fn filter_of(args: &Value) -> Result<Filter, String> {
    Filter::new(
        &strings(args, "path"),
        &strings(args, "ext"),
        &strings(args, "lang"),
    )
    .map_err(|e| e.to_string())
}

/// One filter argument as strings. A bare string is accepted alongside an
/// array, because that is what an agent produces about half the time.
fn strings(args: &Value, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

fn tool_error(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

/// Compact, agent-readable: a locator line then the excerpt. Cheap to parse,
/// cheap in tokens, and the locator is enough to go read the real file.
/// The default reply shape from 0.15.0: where the answers are, not the answers
/// themselves.
///
/// The study measured a warm `semlith_search` reply at 4.9-7.2 KB at k=8
/// against 100-300 bytes for a grep the agent could have run. Most of that is
/// chunk text the agent did not ask for and often does not read: it asked
/// where something is, and was handed the something.
///
/// A locate row is where it is, what it is called, how it was found, and one
/// line of it. An agent that wants the text asks for `format: excerpt`, or
/// reads the file at the line it was just given — which is the same read it
/// would have done anyway, and now it is one read instead of eight.
///
/// Rows are grouped by file because that is how the answer is used: eight hits
/// in one file are one file to open.
fn locate(hits: &[crate::Hit], query: &str, max_tokens: usize) -> String {
    if hits.is_empty() {
        return String::new();
    }
    let budget = max_tokens.max(MIN_LOCATE_TOKENS);
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() > 2)
        .map(|t| t.to_lowercase())
        .collect();

    // Grouped, but in the order the ranking put the files in: the best hit's
    // file is the first thing read.
    let mut files: Vec<(String, Vec<&crate::Hit>)> = Vec::new();
    for hit in hits {
        let label = match &hit.store {
            Some(store) => format!("{store} {}", hit.path),
            None => hit.path.clone(),
        };
        match files.iter_mut().find(|(name, _)| *name == label) {
            Some((_, group)) => group.push(hit),
            None => files.push((label, vec![hit])),
        }
    }

    // Rendered whole, then cut from the bottom — the ranking's own order — so
    // what is dropped is what was worth least.
    let mut blocks: Vec<(usize, String)> = Vec::new();
    for (label, group) in &files {
        let mut block = format!("{label}\n");
        for hit in group {
            block.push_str(&locate_row(hit, &terms));
        }
        blocks.push((group.len(), block));
    }

    let total = hits.len();
    let mut out = String::new();
    let mut shown = 0usize;
    for (count, block) in blocks {
        if !out.is_empty() && tokens(&out) + tokens(&block) > budget {
            break;
        }
        out.push_str(&block);
        shown += count;
    }
    if shown < total {
        out.push_str(&format!("truncated: {shown} of {total}\n"));
    }
    out.trim_end().to_string()
}

/// How the query was read, in one short line above the hits.
///
/// An agent cannot correct a shape that was read wrongly unless it is told
/// which one was read, and `prefer` is the correction — so the line names both
/// and costs about eight tokens.
fn reading(query: &str, prefer: crate::Prefer) -> String {
    let shape = crate::shape_of(query);
    match prefer {
        crate::Prefer::Any => format!("{} · {}", shape.as_str(), shape.weighting()),
        chosen => format!(
            "{} · {} · prefer {}",
            shape.as_str(),
            shape.weighting(),
            chosen.as_str()
        ),
    }
}

/// One line of `where`, and one line of `what`.
fn locate_row(hit: &crate::Hit, terms: &[String]) -> String {
    if let Some(px) = hit.image {
        return format!("  image {}x{} px\n", px.width, px.height);
    }
    let mut marks = Vec::new();
    if !hit.lists.is_empty() {
        // A hit the graph reached says how well supported the edge was. A hit
        // the query matched needs no such qualifier.
        match (&hit.provenance, hit.lists.as_slice()) {
            (Some(tier), ["graph"]) => marks.push(format!("graph({tier})")),
            _ => marks.push(hit.lists.join("+")),
        }
    }
    if !hit.fresh {
        marks.push("stale".to_string());
    }
    let named = match (&hit.symbol, &hit.symbol_kind) {
        (Some(name), Some(kind)) => format!(" \u{00b7} {name} {kind}"),
        (Some(name), None) => format!(" \u{00b7} {name}"),
        _ => String::new(),
    };
    let marked = if marks.is_empty() {
        String::new()
    } else {
        format!(" \u{00b7} {}", marks.join(" \u{00b7} "))
    };
    format!(
        "  {}-{}{named}{marked}\n    {}\n",
        hit.start_line,
        hit.end_line,
        best_line(&hit.text, terms),
    )
}

/// The one line of a chunk most worth showing: the one with the most of the
/// query's words in it.
///
/// Falling back to the first line with anything on it, which is what a reader
/// skimming the file would see first anyway.
fn best_line(text: &str, terms: &[String]) -> String {
    const WIDTH: usize = 96;
    let scored = text
        .lines()
        .map(|line| {
            let lowered = line.to_lowercase();
            let hits = terms.iter().filter(|t| lowered.contains(*t)).count();
            (hits, line.trim())
        })
        .filter(|(_, line)| !line.is_empty())
        .max_by_key(|(hits, _)| *hits);
    let line = match scored {
        Some((0, _)) | None => text.lines().map(str::trim).find(|l| !l.is_empty()),
        Some((_, line)) => Some(line),
    }
    .unwrap_or_default();
    if line.chars().count() <= WIDTH {
        return line.to_string();
    }
    let cut: String = line.chars().take(WIDTH).collect();
    format!("{cut}\u{2026}")
}

/// The floor under `max_tokens`.
///
/// A budget small enough to return nothing is a budget that turns a search
/// into a silent failure, so it is refused rather than honoured.
const MIN_LOCATE_TOKENS: usize = 200;

/// The default budget for a locate reply.
const DEFAULT_LOCATE_TOKENS: usize = 1_500;

/// How many tokens a string is.
///
/// Four characters per token, the estimate 0.12.0's ledger has always used,
/// and it is labelled as an estimate wherever it is recorded.
fn tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

fn render(hits: &[crate::Hit]) -> String {
    let mut out = String::new();
    for (i, h) in hits.iter().enumerate() {
        // The store, when there is more than one, goes in front of the path:
        // one short word, and without it an excerpt from the client is
        // indistinguishable from one from the service.
        let from = match &h.store {
            Some(label) => format!("{label} "),
            None => String::new(),
        };
        // Which lists found it. A hit that only the graph reached is a
        // neighbour of a match rather than a match, and an agent that cannot
        // tell the two apart will quote it as though the query found it.
        let via = if h.lists.is_empty() {
            String::new()
        } else {
            format!(" via {}", h.lists.join("+"))
        };
        // An image has no excerpt to quote: it is named, sized, and left for
        // the agent to open. Saying "an image" outright matters more here than
        // anywhere else — an agent handed a path with no text under it would
        // otherwise read the silence as an empty file.
        if let Some(px) = h.image {
            out.push_str(&format!(
                "[{}] {from}{} — image, {}x{} px (score {:.3}{via})\n\n",
                i + 1,
                h.path,
                px.width,
                px.height,
                h.score,
            ));
            continue;
        }
        out.push_str(&format!(
            "[{}] {from}{}:{}-{} (score {:.3}{via})\n{}\n\n",
            i + 1,
            h.path,
            h.start_line,
            h.end_line,
            h.score,
            h.text.trim_end()
        ));
    }
    out.trim_end().to_string()
}

fn respond(out: &mut impl Write, id: &Value, result: Result<Value, Fail>) -> std::io::Result<()> {
    writeln!(out, "{}", reply(id, result))?;
    out.flush()
}

/// One JSON-RPC response object.
fn reply(id: &Value, result: Result<Value, Fail>) -> Value {
    match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err((code, message, data)) => {
            let mut error = json!({ "code": code, "message": message });
            if let Some(data) = data {
                error["data"] = data;
            }
            json!({ "jsonrpc": "2.0", "id": id, "error": error })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defect this release exists to fix: answering a handshake with
    /// whatever it was sent claims every revision that will ever exist,
    /// including the ones that deleted handshakes.
    #[test]
    fn a_revision_we_do_not_implement_is_never_echoed() {
        assert_eq!(negotiate(Some("1900-01-01")), LEGACY_NEWEST);
        assert_eq!(negotiate(Some("2029-11-05")), LEGACY_NEWEST);
        // The modern revision has no handshake, so a client asking for it over
        // `initialize` is asking for something that does not exist there.
        assert_eq!(negotiate(Some(MODERN)), LEGACY_NEWEST);
    }

    #[test]
    fn a_revision_we_do_implement_is_answered_with_itself() {
        for revision in ["2025-11-25", "2025-06-18", "2024-11-05"] {
            assert_eq!(negotiate(Some(revision)), revision);
        }
        // No version named at all: the field is required, so this client is
        // old rather than new.
        assert_eq!(negotiate(None), LEGACY_OLDEST);
    }

    /// A modern result is told apart from a legacy one by the message, not by
    /// the connection, so the marker has to be on the result itself.
    #[test]
    fn only_modern_requests_declare_a_version() {
        let modern = json!({ "_meta": { META_VERSION: MODERN } });
        assert_eq!(declared_version(&modern), Some(MODERN));
        assert_eq!(declared_version(&json!({ "name": "semlith_stats" })), None);
    }

    fn hit(path: &str, start: u32, end: u32, text: &str) -> crate::Hit {
        crate::Hit {
            score: 0.5,
            path: path.to_string(),
            start_line: start,
            end_line: end,
            text: text.to_string(),
            store: None,
            lists: vec!["vector", "keyword"],
            image: None,
            fresh: true,
            symbol: Some("edges_out".to_string()),
            symbol_kind: Some("fn".to_string()),
            provenance: None,
        }
    }

    /// The cost half of the release, as a unit.
    ///
    /// The study measured a warm reply at 4.9-7.2 KB at k=8 against 100-300
    /// bytes for the grep an agent could have run instead. A locate row is an
    /// address, not the thing at the address, and this is the budget that says
    /// so in a number rather than in a paragraph.
    #[test]
    fn a_locate_row_is_an_address_rather_than_its_contents() {
        let hits: Vec<crate::Hit> = (0..8)
            .map(|i| {
                hit(
                    &format!("src/file{i}.rs"),
                    100 + i * 10,
                    120 + i * 10,
                    "fn edges_out(db: &Connection, name: &str) -> Result<Vec<EdgeEnd>> {\n\
                         let filter = kind_predicate(kinds, \"e.kind\");\n\
                         // A long chunk of source, of the kind 0.14.0 sent whole.\n",
                )
            })
            .collect();

        let reply = locate(&hits, "edges_out", DEFAULT_LOCATE_TOKENS);
        let per_hit = reply.len() / hits.len();
        assert!(
            per_hit < 200,
            "a locate row costs {per_hit} bytes; the whole reply was:\n{reply}"
        );
        assert!(reply.contains("edges_out fn"), "{reply}");
        assert!(reply.contains("100-120"), "{reply}");
    }

    /// The budget is honoured, and a reply that was cut says so rather than
    /// looking like a complete answer that found less.
    #[test]
    fn a_small_budget_cuts_the_reply_and_says_it_did() {
        // Enough files for the floor to bite. A bigger *chunk* would not do
        // it: the budget measures the reply, and a locate row is one line of a
        // chunk however long the chunk is — which is the whole point of the
        // format, and is worth pinning here.
        let hits: Vec<crate::Hit> = (0..40)
            .map(|i| hit(&format!("src/file{i}.rs"), 10, 40, "fn a() { helper(); }\n"))
            .collect();

        let full = locate(&hits, "helper", 10_000);
        assert!(!full.contains("truncated:"), "nothing was cut: {full}");

        let cut = locate(&hits, "helper", MIN_LOCATE_TOKENS);
        assert!(cut.contains("truncated:"), "{cut}");
        assert!(cut.len() < full.len(), "the budget did not cut anything");
        assert!(
            cut.contains("of 40"),
            "the reader is told what the total was: {cut}"
        );
    }

    /// A budget small enough to return nothing would turn a search into a
    /// silent failure, so the first file always comes back.
    #[test]
    fn an_impossible_budget_still_returns_the_best_file() {
        let hits = vec![hit("src/store.rs", 948, 970, "fn edges_out() {}\n")];
        let reply = locate(&hits, "edges_out", 1);
        assert!(reply.contains("src/store.rs"), "{reply}");
    }

    /// A stale hit says so. An agent quoting a chunk from a file that has moved
    /// under it is the same class of defect as a wrong path, and just as quiet.
    #[test]
    fn a_stale_hit_is_marked_and_a_graph_reached_one_carries_its_tier() {
        let mut stale = hit("src/lib.rs", 10, 20, "fn a() {}\n");
        stale.fresh = false;
        let mut reached = hit("src/graph.rs", 30, 40, "fn b() {}\n");
        reached.lists = vec!["graph"];
        reached.provenance = Some(crate::graph::RESOLVED.to_string());

        let reply = locate(&[stale, reached], "a", DEFAULT_LOCATE_TOKENS);
        assert!(reply.contains("stale"), "{reply}");
        assert!(reply.contains("graph(resolved)"), "{reply}");
    }

    /// The tool list is the first thing every agent pays for, once per session,
    /// before it has asked anything.
    ///
    /// 0.14.0's was 8 955 bytes — about 2 200 tokens of schema an agent reads
    /// and mostly cannot act on. The budget here is the release's, and it is a
    /// test rather than a note because a one-sentence description is the kind
    /// of thing that grows back a paragraph at a time.
    #[test]
    fn the_tool_list_stays_small() {
        let size = serde_json::to_string(&tool_defs("default")).unwrap().len();
        let each: Vec<String> = tool_defs("default")
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                format!(
                    "{}={}",
                    t["name"].as_str().unwrap(),
                    serde_json::to_string(t).unwrap().len()
                )
            })
            .collect();
        assert!(
            size < 4_000,
            "tools/list is {size} bytes: {}",
            each.join(" ")
        );
    }
}
