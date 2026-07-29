//! A stdio MCP server, so an agent can query the store as a tool call.
//!
//! Deliberately hand-rolled: MCP over stdio is newline-delimited JSON-RPC 2.0,
//! and the three methods a tools-only server needs fit in one file with no
//! extra dependency.
//!
//! Everything written to stdout is protocol. Diagnostics go to stderr.

use crate::Semlith;
use crate::filter::Filter;
use anyhow::Result;
use serde_json::{Value, json};
use std::io::{BufRead, Write};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// Read requests from `input` until EOF, answering on `output`.
pub fn serve(store: &mut Semlith, input: impl BufRead, mut output: impl Write) -> Result<()> {
    store.quiet = true;

    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                respond(&mut output, &json!(null), Err((-32700, e.to_string())))?;
                continue;
            }
        };

        // Notifications carry no id and must not be answered.
        let Some(id) = req.get("id").cloned() else {
            continue;
        };
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(json!({}));

        let result = dispatch(store, method, &params);
        respond(&mut output, &id, result)?;
    }
    Ok(())
}

fn dispatch(store: &mut Semlith, method: &str, params: &Value) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION);
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "semlith", "version": env!("CARGO_PKG_VERSION") },
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => call_tool(store, params),
        other => Err((-32601, format!("unknown method: {other}"))),
    }
}

fn tools() -> Value {
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
                    }
                },
                "required": ["query"]
            }
        },
        {
            "name": "semlith_stats",
            "description":
                "Report what the local semlith store currently contains: file count, chunk \
                 count, indexed bytes, and the embedding model. Use this to check whether a \
                 corpus is indexed before searching it.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

fn call_tool(store: &mut Semlith, params: &Value) -> Result<Value, (i64, String)> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    let body = match name {
        "semlith_search" => {
            let Some(query) = args.get("query").and_then(Value::as_str) else {
                return Err((-32602, "missing required argument: query".into()));
            };
            let k = args.get("k").and_then(Value::as_u64).unwrap_or(8) as usize;

            let filter = match Filter::new(
                &strings(&args, "path"),
                &strings(&args, "ext"),
                &strings(&args, "lang"),
            ) {
                Ok(f) => f,
                // An unknown language is the agent's mistake to correct, so it
                // goes back in-band with the list rather than as a protocol error.
                Err(e) => return Ok(tool_error(&e.to_string())),
            };

            // Told apart because an agent that scoped to the wrong subsystem
            // should widen the filter, not conclude the corpus is empty.
            if !filter.is_empty() && store.matching_files(&filter).unwrap_or(0) == 0 {
                "No indexed file matches that path/ext/lang filter. Try again without it."
                    .to_string()
            } else {
                match store.search_filtered(query, k.clamp(1, 50), &filter) {
                    Ok(hits) if hits.is_empty() => "No matches in the semlith store.".to_string(),
                    Ok(hits) => render(&hits),
                    // Tool failures are reported in-band so the agent can react,
                    // rather than as a protocol-level error.
                    Err(e) => return Ok(tool_error(&format!("search failed: {e}"))),
                }
            }
        }
        "semlith_stats" => match store.stats() {
            Ok((files, chunks, bytes)) => format!(
                "{files} files, {chunks} chunks, {} indexed, model {} ({} dim)",
                crate::human_bytes(bytes),
                store.model(),
                store.dim(),
            ),
            Err(e) => return Ok(tool_error(&format!("stats failed: {e}"))),
        },
        other => return Err((-32602, format!("unknown tool: {other}"))),
    };

    Ok(json!({ "content": [{ "type": "text", "text": body }] }))
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
        out.push_str(&format!(
            "[{}] {}:{}-{} (score {:.3})\n{}\n\n",
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

fn respond(
    out: &mut impl Write,
    id: &Value,
    result: Result<Value, (i64, String)>,
) -> std::io::Result<()> {
    let msg = match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err((code, message)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        }
    };
    writeln!(out, "{msg}")?;
    out.flush()
}
