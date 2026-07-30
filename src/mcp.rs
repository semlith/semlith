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

/// Read requests from `input` until EOF, answering on `output`.
pub fn serve(stores: &mut Fleet, input: impl BufRead, mut output: impl Write) -> Result<()> {
    stores.quiet = true;

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

        let result = dispatch(stores, method, &params);
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

fn dispatch(stores: &mut Fleet, method: &str, params: &Value) -> Result<Value, Fail> {
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
            let called = call_tool(stores, params)?;
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
    let open = stores.labels().join(", ");
    let store_arg = format!(
        "Restrict the search to these stores by name. Open stores: {open}. \
         Omit to search all of them, which is usually right — narrow only when \
         you already know which corpus holds the answer."
    );
    let write_store_arg = format!(
        "Which store to write to, by name. Open stores: {open}. \
         Required when more than one is open, since a store takes one writer."
    );

    json!([
        {
            "name": "semlith_search",
            "description":
                "Semantic search over the local semlith store: docs, PDFs, code, and notes \
                 that have been indexed on this machine. Returns the most relevant excerpts \
                 with their file path and line range. Use this instead of reading whole files \
                 when you need to find where something is discussed or implemented. \
                 Optionally narrow the search to one part of the corpus with path, ext or lang.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What you are looking for. A question in plain English, or an exact identifier — both are searched."
                    },
                    "k": {
                        "type": "integer",
                        "description": "How many excerpts to return (default 8).",
                        "minimum": 1,
                        "maximum": 50
                    },
                    "path": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description":
                            "Restrict the search to files matching these globs, e.g. \"src/**\". \
                             A pattern that does not start with / matches anywhere in the tree. \
                             `*` crosses directory separators, so `src/*` reaches the whole \
                             subtree. Only filter when you already know which part of the \
                             corpus holds the answer — a wrong guess hides it entirely."
                    },
                    "ext": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Restrict the search to these file extensions, e.g. [\"rs\", \"toml\"]."
                    },
                    "lang": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description":
                            "Restrict the search to these languages, e.g. [\"rust\"]. Accepts: \
                             c, cpp, csharp, css, go, haskell, html, java, javascript, json, \
                             kotlin, lua, markdown, ocaml, php, python, ruby, rust, scala, \
                             shell, sql, swift, toml, typescript, yaml."
                    },
                    "store": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": store_arg
                    }
                },
                "required": ["query"]
            },
            "annotations": { "title": "Search the semlith store", "readOnlyHint": true }
        },
        {
            "name": "semlith_stats",
            "description":
                "Report what the local semlith stores currently contain: file count, chunk \
                 count, indexed bytes, and the embedding model, one line per store. Use this \
                 to check whether a corpus is indexed before searching it, and to learn the \
                 store names the other tools accept.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "title": "Describe the semlith stores", "readOnlyHint": true }
        },
        {
            "name": "semlith_files",
            "description":
                "List the files currently indexed, narrowed the same way as a search. Use it \
                 to tell \"this file is not indexed\" apart from \"the corpus does not discuss \
                 this\", which a search returning nothing cannot say.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Only list files matching these globs, e.g. \"src/**\"."
                    },
                    "ext": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Only list files with these extensions, e.g. [\"rs\"]."
                    },
                    "lang": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Only list files of these languages, e.g. [\"rust\"]."
                    },
                    "store": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": store_arg
                    },
                    "limit": {
                        "type": "integer",
                        "description": "How many paths to return before counting the rest (default 200).",
                        "minimum": 1
                    }
                }
            },
            "annotations": { "title": "List indexed files", "readOnlyHint": true }
        },
        {
            "name": "semlith_index",
            "description":
                "Index files and directories into a store that is already open, so their \
                 contents become searchable in this session. Only what has changed is \
                 re-embedded. Large corpora take longer than one tool call allows: the call \
                 returns what it managed and how much is left, and calling it again continues \
                 from there.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Files or directories to index. Directories are walked, honouring .gitignore."
                    },
                    "store": { "type": "string", "description": write_store_arg }
                },
                "required": ["path"]
            },
            "annotations": { "title": "Index files into the store", "readOnlyHint": false }
        },
        {
            "name": "semlith_forget",
            "description":
                "Remove one file from a store, so it stops appearing in searches. The file on \
                 disk is untouched; only what was indexed about it goes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "The file to remove from the index." },
                    "store": { "type": "string", "description": write_store_arg }
                },
                "required": ["path"]
            },
            "annotations": {
                "title": "Forget a file",
                "readOnlyHint": false,
                "destructiveHint": true
            }
        }
    ])
}

fn call_tool(stores: &mut Fleet, params: &Value) -> Result<Value, Fail> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

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
                match stores.search_in(Some(&only), query, k.clamp(1, 50), &filter) {
                    Ok(hits) if hits.is_empty() => "No matches in the semlith store.".to_string(),
                    Ok(hits) => render(&hits),
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
            let store = match stores.writable(&strings(&args, "store")) {
                Ok(s) => s,
                Err(e) => return Ok(tool_error(&e.to_string())),
            };

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
                    out
                }
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
            let store = match stores.writable(&strings(&args, "store")) {
                Ok(s) => s,
                Err(e) => return Ok(tool_error(&e.to_string())),
            };
            match store.forget(std::path::Path::new(&path)) {
                Ok(0) => format!("{path} was not indexed; nothing removed."),
                Ok(n) => format!("Removed {n} chunks for {path}."),
                Err(e) => return Ok(tool_error(&e.to_string())),
            }
        }
        other => return Err((-32602, format!("unknown tool: {other}"), None)),
    };

    Ok(json!({ "content": [{ "type": "text", "text": body }] }))
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
        out.push_str(&format!(
            "[{}] {from}{}:{}-{} (score {:.3})\n{}\n\n",
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
    let msg = match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err((code, message, data)) => {
            let mut error = json!({ "code": code, "message": message });
            if let Some(data) = data {
                error["data"] = data;
            }
            json!({ "jsonrpc": "2.0", "id": id, "error": error })
        }
    };
    writeln!(out, "{msg}")?;
    out.flush()
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
}
