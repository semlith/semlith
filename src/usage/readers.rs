//! One reader per supported client: where its log is, and how a semlith call
//! and the request that made it look inside it.
//!
//! The formats are each client's own and undocumented; every reader has a
//! synthetic fixture under `tests/fixtures/usage/` that pins what it expects.
//! Token figures are put into one convention on the way out (see
//! [`Tokens`]): some clients count cached input inside `input` and some count
//! it apart, and some count reasoning inside `output` and some apart.

use super::{Call, Found, Log, semlith_tool};
use crate::prices::Tokens;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// The semlith calls in `client`'s logs changed since `since`.
///
/// `client` is the name a ledger row carries — one of `docs/clients.md`'s, or
/// the handshake name an older row was written with.
pub fn read(client: &str, since: i64) -> Result<Log> {
    // A row written before 0.33.0 may carry the MCP library's name — `mcp`,
    // `rmcp` — instead of the app's. Its call is in one of the logs, so every
    // reader is asked, and the matcher's tool, time and arguments decide.
    if crate::clientid::generic(client) {
        let mut calls = Vec::new();
        for named in [
            "Claude Code",
            "OpenAI Codex",
            "ChatGPT desktop (the Codex app)",
            "OpenCode",
            "Gemini CLI",
            "GitHub Copilot CLI",
            "GitHub Copilot in VS Code",
            "Cline",
            "Claude Desktop",
            "IO CLI",
        ] {
            if let Ok(Log::Calls(found)) = read(named, since) {
                calls.extend(found);
            }
        }
        return Ok(Log::Calls(calls));
    }
    let home = crate::home::user_home()?;
    let calls = match kind(client) {
        Some(Kind::ClaudeCode) => claude_code(&[claude_dir(&home)], since)?,
        Some(Kind::Codex { desktop }) => codex(&codex_dir(&home), since, desktop)?,
        // The handshake name both apps sent before 0.33.0 told them apart.
        Some(Kind::EitherCodex) => {
            let mut calls = codex(&codex_dir(&home), since, false)?;
            calls.extend(codex(&codex_dir(&home), since, true)?);
            calls
        }
        Some(Kind::OpenCode) => opencode(&opencode_db(&home), since)?,
        Some(Kind::Gemini) => gemini(&home.join(".gemini").join("tmp"), since)?,
        Some(Kind::CopilotCli) => copilot_cli(&copilot_dir(&home), since)?,
        Some(Kind::VsCode) => {
            let logs = vscode_debug_logs(&vscode_user(&home), since);
            if logs.is_empty() && !any_vscode_debug_log(&vscode_user(&home)) {
                return Ok(Log::NotRecorded(
                    "VS Code keeps per-request usage only with chat.agentDebugLog.fileLogging.enabled on"
                        .into(),
                ));
            }
            vscode(&logs)?
        }
        Some(Kind::Cline) => cline(&cline_dir(&home), since)?,
        Some(Kind::ClaudeDesktop) => {
            let dir = claude_desktop_dir(&home);
            if !dir.exists() {
                return Ok(Log::NotRecorded(
                    "Claude Desktop keeps no token usage for chats on this machine".into(),
                ));
            }
            claude_code(&[dir], since)?
        }
        Some(Kind::IoCli) => io_cli(&io_db(&home), since)?,
        Some(Kind::Zed) => {
            return Ok(Log::NotRecorded(
                "Zed keeps a per-thread total, not the request behind each call".into(),
            ));
        }
        Some(Kind::Cursor) => {
            return Ok(Log::NotRecorded(
                "Cursor stores no token counts on this machine".into(),
            ));
        }
        None => {
            return Ok(Log::NotRecorded(format!(
                "{client} is not a client whose log semlith reads"
            )));
        }
    };
    Ok(Log::Calls(calls))
}

/// Where each client's reader looks, on this machine, for the Privacy page.
/// A client with no readable log is listed with none.
pub fn paths() -> Vec<(&'static str, Vec<PathBuf>)> {
    let Ok(home) = crate::home::user_home() else {
        return Vec::new();
    };
    let vscode = vscode_user(&home);
    vec![
        ("Claude Code", vec![claude_dir(&home)]),
        (
            "OpenAI Codex and the ChatGPT desktop app",
            vec![codex_dir(&home)],
        ),
        ("OpenCode", vec![opencode_db(&home)]),
        ("Gemini CLI", vec![home.join(".gemini").join("tmp")]),
        (
            "GitHub Copilot CLI",
            vec![
                copilot_dir(&home).join("session-state"),
                copilot_dir(&home).join("session-store.db"),
            ],
        ),
        (
            "GitHub Copilot in VS Code",
            vec![
                vscode.join("workspaceStorage"),
                vscode.join("globalStorage"),
            ],
        ),
        ("Cline", vec![cline_dir(&home)]),
        ("Claude Desktop", vec![claude_desktop_dir(&home)]),
        ("IO CLI", vec![io_db(&home)]),
        ("Zed", Vec::new()),
        ("Cursor", Vec::new()),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    ClaudeCode,
    Codex { desktop: bool },
    EitherCodex,
    OpenCode,
    Gemini,
    CopilotCli,
    VsCode,
    Cline,
    ClaudeDesktop,
    IoCli,
    Zed,
    Cursor,
}

fn kind(client: &str) -> Option<Kind> {
    Some(match client.to_ascii_lowercase().as_str() {
        "claude code" | "claude-code" => Kind::ClaudeCode,
        "openai codex" => Kind::Codex { desktop: false },
        "codex" | "codex-mcp-client" => Kind::EitherCodex,
        "chatgpt desktop (the codex app)" => Kind::Codex { desktop: true },
        "opencode" => Kind::OpenCode,
        "github copilot cli" => Kind::CopilotCli,
        "github copilot in vs code" | "visual studio code" | "vscode" => Kind::VsCode,
        "claude desktop" | "claude-ai" => Kind::ClaudeDesktop,
        "io cli" | "io" | "io-cli" => Kind::IoCli,
        "zed" => Kind::Zed,
        "cursor" | "cursor-vscode" => Kind::Cursor,
        l if l.starts_with("gemini") => Kind::Gemini,
        l if l.contains("cline") => Kind::Cline,
        _ => return None,
    })
}

// ------------------------------------------------------------------ where

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn claude_dir(home: &Path) -> PathBuf {
    env_dir("CLAUDE_CONFIG_DIR")
        .unwrap_or_else(|| home.join(".claude"))
        .join("projects")
}

fn codex_dir(home: &Path) -> PathBuf {
    env_dir("CODEX_HOME")
        .unwrap_or_else(|| home.join(".codex"))
        .join("sessions")
}

fn opencode_db(home: &Path) -> PathBuf {
    env_dir("XDG_DATA_HOME")
        .unwrap_or_else(|| home.join(".local").join("share"))
        .join("opencode")
        .join("opencode.db")
}

fn copilot_dir(home: &Path) -> PathBuf {
    env_dir("COPILOT_HOME")
        .or_else(|| env_dir("XDG_CONFIG_HOME").map(|d| d.join("copilot")))
        .unwrap_or_else(|| home.join(".copilot"))
}

fn cline_dir(home: &Path) -> PathBuf {
    env_dir("CLINE_DATA_DIR")
        .or_else(|| env_dir("CLINE_DIR").map(|d| d.join("data")))
        .unwrap_or_else(|| home.join(".cline").join("data"))
        .join("sessions")
}

fn io_db(home: &Path) -> PathBuf {
    env_dir("IO_CONFIG_HOME")
        .unwrap_or_else(|| home.join(".io-cli"))
        .join("runs.db")
}

/// The per-user application-data folder an Electron app writes under.
fn app_data(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else if cfg!(windows) {
        env_dir("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming"))
    } else {
        env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"))
    }
}

fn vscode_user(home: &Path) -> PathBuf {
    app_data(home).join("Code").join("User")
}

fn claude_desktop_dir(home: &Path) -> PathBuf {
    app_data(home)
        .join("Claude")
        .join("local-agent-mode-sessions")
}

// ---------------------------------------------------------------- helpers

/// Files under `root` for which `keep` holds, modified at or after `since`,
/// at most `depth` directories down. Symbolic links are not followed.
fn walk(root: &Path, depth: usize, since: i64, keep: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() && depth > 0 {
            out.extend(walk(&path, depth - 1, since, keep));
        } else if kind.is_file() && keep(&path) && modified(&path) >= since {
            out.push(path);
        }
    }
    out
}

fn modified(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

fn ends(path: &Path, suffix: &str) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(suffix))
}

/// Each line of a file that `wanted` lets through, parsed. A line that is not
/// JSON is skipped: a log being appended to can end mid-line.
fn lines(path: &Path, wanted: &mut dyn FnMut(&str) -> bool) -> Result<Vec<Value>> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut out = Vec::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if wanted(&line)
            && let Ok(value) = serde_json::from_str::<Value>(&line)
        {
            out.push(value);
        }
    }
    Ok(out)
}

/// A database another program writes, opened so this one never can.
fn open_ro(path: &Path) -> Result<rusqlite::Connection> {
    let db = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening {}", path.display()))?;
    db.busy_timeout(std::time::Duration::from_secs(2))?;
    Ok(db)
}

fn int(v: &Value, pointer: &str) -> i64 {
    v.pointer(pointer).and_then(Value::as_i64).unwrap_or(0)
}

fn text(v: &Value, pointer: &str) -> Option<String> {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Unix seconds from `2026-09-30T10:00:02.300Z`, `2026-09-30 10:00:02` or an
/// offset form. No zone means UTC, which is what every log here writes.
pub fn iso_secs(s: &str) -> Option<i64> {
    let s = s.trim();
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut offset = 0;
    if let Some(zone) = s.get(19..) {
        let zone = zone.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
        if let Some(sign) = zone.chars().next().filter(|c| *c == '+' || *c == '-') {
            let hh: i64 = zone.get(1..3)?.parse().ok()?;
            let mm: i64 = zone.get(zone.len().saturating_sub(2)..)?.parse().ok()?;
            offset = (hh * 60 + mm) * 60 * if sign == '+' { 1 } else { -1 };
        }
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3_600 + mi * 60 + se - offset)
}

/// Input that counted cache inside it, put into this module's terms.
fn tokens_incl_cache(
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
) -> Tokens {
    Tokens {
        input: (input - cache_read - cache_write).max(0),
        output,
        cache_read,
        cache_write,
        reasoning,
        cache_write_1h: 0,
    }
}

fn call(at: i64, tool: String, args: String, found: Found) -> Call {
    Call {
        at,
        tool,
        args,
        found,
    }
}

fn source(path: &Path, request: &str) -> String {
    format!("{}#{request}", crate::plain(&path.display().to_string()))
}

// ------------------------------------------------------------ Claude Code

/// `~/.claude/projects/<slug>/<session>.jsonl` and each session's subagents.
/// One response is several lines — one per content block — that repeat its
/// usage, the later ones with the larger output count, so a message's usage
/// is its last line's.
fn claude_code(roots: &[PathBuf], since: i64) -> Result<Vec<Call>> {
    let mut out = Vec::new();
    for root in roots {
        for path in walk(root, 8, since, &|p| ends(p, ".jsonl")) {
            out.extend(claude_code_file(&path)?);
        }
    }
    Ok(out)
}

fn claude_code_file(path: &Path) -> Result<Vec<Call>> {
    let mut pending: Vec<String> = Vec::new();
    let records = lines(path, &mut |line| {
        line.contains("semlith_") || pending.iter().any(|id| line.contains(id.as_str()))
    })?;
    // The last usage per message, then the calls with it.
    let mut usage: HashMap<String, (Option<String>, Tokens, String)> = HashMap::new();
    let mut calls: Vec<(i64, String, String, String)> = Vec::new();
    for r in &records {
        if r.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(id) = text(r, "/message/id") else {
            continue;
        };
        let u = r.pointer("/message/usage").cloned().unwrap_or_default();
        let tokens = Tokens {
            input: int(&u, "/input_tokens"),
            output: int(&u, "/output_tokens"),
            cache_read: int(&u, "/cache_read_input_tokens"),
            cache_write: int(&u, "/cache_creation_input_tokens"),
            reasoning: int(&u, "/output_tokens_details/thinking_tokens"),
            cache_write_1h: int(&u, "/cache_creation/ephemeral_1h_input_tokens"),
        };
        let request = text(r, "/requestId").unwrap_or_else(|| id.clone());
        let entry = usage.entry(id.clone()).or_insert((None, tokens, request));
        entry.0 = text(r, "/message/model").or(entry.0.take());
        if tokens.output >= entry.1.output {
            entry.1 = tokens;
        }
        let at = text(r, "/timestamp")
            .and_then(|t| iso_secs(&t))
            .unwrap_or(0);
        for block in r
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let Some(tool) = block
                .get("name")
                .and_then(Value::as_str)
                .and_then(semlith_tool)
            else {
                continue;
            };
            let args = block.get("input").map(Value::to_string).unwrap_or_default();
            calls.push((at, tool, args, id.clone()));
            if !pending.contains(&id) {
                pending.push(id.clone());
            }
        }
    }
    Ok(calls
        .into_iter()
        .filter_map(|(at, tool, args, id)| {
            let (model, tokens, request) = usage.get(&id)?.clone();
            Some(call(
                at,
                tool,
                args,
                Found {
                    model,
                    provider: Some("anthropic".into()),
                    tokens: Some(tokens),
                    cost: None,
                    source: source(path, &request),
                },
            ))
        })
        .collect())
}

// ------------------------------------------------------------------ Codex

/// `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`, shared by the Codex CLI
/// and the ChatGPT desktop app, told apart by the session's `originator`. A
/// call's usage is the first per-request record after it: `token_count`'s
/// `last_token_usage` (`total_token_usage` is the session's running sum), or
/// the app's `token_usage_record`. In the app's code mode an MCP call runs
/// inside an `exec` tool call, and is given that call's request.
fn codex(root: &Path, since: i64, desktop: bool) -> Result<Vec<Call>> {
    let mut out = Vec::new();
    for path in walk(root, 4, since, &|p| ends(p, ".jsonl")) {
        out.extend(codex_file(&path, desktop)?);
    }
    Ok(out)
}

fn codex_file(path: &Path, desktop: bool) -> Result<Vec<Call>> {
    let records = lines(path, &mut |line| {
        [
            "semlith",
            "\"token_count\"",
            "\"token_usage_record\"",
            "\"turn_context\"",
            "\"session_meta\"",
            "\"custom_tool_call\"",
        ]
        .iter()
        .any(|needle| line.contains(needle))
    })?;
    let mut out: Vec<Call> = Vec::new();
    let (mut model, mut provider) = (None::<String>, None::<String>);
    let mut waiting: Vec<usize> = Vec::new();
    let mut exec: Option<Option<(Tokens, usize)>> = None;
    let mut known: Vec<String> = Vec::new();
    for (n, r) in records.iter().enumerate() {
        let kind = r.get("type").and_then(Value::as_str).unwrap_or("");
        let p = r.get("payload").cloned().unwrap_or_default();
        let at = text(r, "/timestamp")
            .and_then(|t| iso_secs(&t))
            .unwrap_or(0);
        match (kind, p.get("type").and_then(Value::as_str)) {
            ("session_meta", _) => {
                let originator = text(&p, "/originator")
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if originator.contains("desktop") != desktop {
                    return Ok(Vec::new());
                }
                provider = text(&p, "/model_provider");
            }
            ("turn_context", _) => model = text(&p, "/model"),
            ("response_item", Some("function_call")) => {
                let name = text(&p, "/name").unwrap_or_default();
                let namespace = text(&p, "/namespace").unwrap_or_default();
                let Some(tool) = semlith_tool(&name) else {
                    continue;
                };
                if !namespace.is_empty()
                    && !namespace.contains("semlith")
                    && !name.contains("semlith_")
                {
                    continue;
                }
                known.extend(text(&p, "/call_id"));
                waiting.push(out.len());
                out.push(call(
                    at,
                    tool,
                    text(&p, "/arguments").unwrap_or_default(),
                    found(&model, &provider, None, path, ""),
                ));
            }
            ("response_item", Some("custom_tool_call"))
                if text(&p, "/name").as_deref() == Some("exec") =>
            {
                exec = Some(None);
            }
            ("event_msg", Some("item_completed")) => {
                let item = p.get("item").cloned().unwrap_or_default();
                if text(&item, "/type").as_deref() != Some("McpToolCall")
                    || text(&item, "/server").as_deref() != Some("semlith")
                    || known.iter().any(|k| Some(k) == text(&item, "/id").as_ref())
                {
                    continue;
                }
                let Some(tool) = text(&item, "/tool").as_deref().and_then(semlith_tool) else {
                    continue;
                };
                let started = p
                    .get("started_at_ms")
                    .and_then(Value::as_i64)
                    .map_or(at, |ms| ms / 1000);
                let args = item
                    .get("arguments")
                    .map(Value::to_string)
                    .unwrap_or_default();
                let mut c = call(
                    started,
                    tool,
                    args,
                    found(&model, &provider, None, path, ""),
                );
                match exec {
                    Some(Some((tokens, record))) => {
                        c.found = found(
                            &model,
                            &provider,
                            Some(tokens),
                            path,
                            &format!("usage{record}"),
                        );
                    }
                    _ => waiting.push(out.len()),
                }
                out.push(c);
            }
            ("event_msg", Some("token_count")) | ("token_usage_record", _) => {
                let u = if kind == "token_usage_record" {
                    p.get("usage").cloned()
                } else {
                    p.pointer("/info/last_token_usage").cloned()
                };
                let Some(u) = u else { continue };
                let tokens = tokens_incl_cache(
                    int(&u, "/input_tokens"),
                    int(&u, "/output_tokens"),
                    int(&u, "/cached_input_tokens"),
                    int(&u, "/cache_write_input_tokens"),
                    int(&u, "/reasoning_output_tokens"),
                );
                let request = format!("usage{n}");
                for i in waiting.drain(..) {
                    out[i].found = found(&model, &provider, Some(tokens), path, &request);
                }
                if exec == Some(None) {
                    exec = Some(Some((tokens, n)));
                }
            }
            _ => {}
        }
    }
    // A call whose request has not been written yet is not a match.
    Ok(out
        .into_iter()
        .filter(|c| c.found.tokens.is_some())
        .collect())
}

fn found(
    model: &Option<String>,
    provider: &Option<String>,
    tokens: Option<Tokens>,
    path: &Path,
    request: &str,
) -> Found {
    Found {
        model: model.clone(),
        provider: provider.clone(),
        tokens,
        cost: None,
        source: source(path, request),
    }
}

// --------------------------------------------------------------- OpenCode

/// `~/.local/share/opencode/opencode.db`: a `tool` part per call, and the
/// usage and cost on its message. Reasoning is counted apart from output.
fn opencode(db: &Path, since: i64) -> Result<Vec<Call>> {
    if !db.exists() {
        return Ok(Vec::new());
    }
    let conn = open_ro(db)?;
    let mut q = conn.prepare(
        "SELECT p.time_created, p.data, m.id, m.data FROM part p JOIN message m ON m.id = p.message_id
          WHERE p.time_created >= ?1 AND p.data LIKE '%semlith%'",
    )?;
    let rows = q.query_map([since * 1000], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (created, part, id, message) = row?;
        let (Ok(part), Ok(m)) = (
            serde_json::from_str::<Value>(&part),
            serde_json::from_str::<Value>(&message),
        ) else {
            continue;
        };
        if text(&part, "/type").as_deref() != Some("tool") {
            continue;
        }
        let Some(tool) = text(&part, "/tool").as_deref().and_then(semlith_tool) else {
            continue;
        };
        let reasoning = int(&m, "/tokens/reasoning");
        let at = part
            .pointer("/state/time/start")
            .and_then(Value::as_i64)
            .unwrap_or(created)
            / 1000;
        out.push(call(
            at,
            tool,
            part.pointer("/state/input")
                .map(Value::to_string)
                .unwrap_or_default(),
            Found {
                model: text(&m, "/modelID"),
                provider: text(&m, "/providerID"),
                tokens: Some(Tokens {
                    input: int(&m, "/tokens/input"),
                    output: int(&m, "/tokens/output") + reasoning,
                    cache_read: int(&m, "/tokens/cache/read"),
                    cache_write: int(&m, "/tokens/cache/write"),
                    reasoning,
                    cache_write_1h: 0,
                }),
                cost: m.get("cost").and_then(Value::as_f64),
                source: source(db, &id),
            },
        ));
    }
    Ok(out)
}

// ------------------------------------------------------------- Gemini CLI

/// `~/.gemini/tmp/<project>/chats/session-*.jsonl`, an append log in which a
/// message is written again once its tool calls are added: the last line with
/// an id is that message. `input` includes the cached tokens; thoughts are
/// counted apart from output.
fn gemini(root: &Path, since: i64) -> Result<Vec<Call>> {
    let mut out = Vec::new();
    for path in walk(root, 3, since, &|p| ends(p, ".jsonl")) {
        let mut latest: HashMap<String, Value> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for r in lines(&path, &mut |l| {
            l.contains("semlith_") && l.contains("\"toolCalls\"")
        })? {
            let Some(id) = text(&r, "/id") else { continue };
            if !latest.contains_key(&id) {
                order.push(id.clone());
            }
            latest.insert(id, r);
        }
        for id in order {
            let m = &latest[&id];
            let thoughts = int(m, "/tokens/thoughts");
            let tokens = tokens_incl_cache(
                int(m, "/tokens/input"),
                int(m, "/tokens/output") + thoughts,
                int(m, "/tokens/cached"),
                0,
                thoughts,
            );
            let at = text(m, "/timestamp")
                .and_then(|t| iso_secs(&t))
                .unwrap_or(0);
            for c in m
                .get("toolCalls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(tool) = text(c, "/name").as_deref().and_then(semlith_tool) else {
                    continue;
                };
                let when = text(c, "/timestamp")
                    .and_then(|t| iso_secs(&t))
                    .unwrap_or(at);
                out.push(call(
                    when,
                    tool,
                    c.get("args").map(Value::to_string).unwrap_or_default(),
                    Found {
                        model: text(m, "/model"),
                        provider: Some("google".into()),
                        tokens: Some(tokens),
                        cost: None,
                        source: source(&path, &id),
                    },
                ));
            }
        }
    }
    Ok(out)
}

// -------------------------------------------------------- GitHub Copilot CLI

/// The calls are in `session-state/<session>/events.jsonl`; the usage is in
/// `session-store.db`'s `assistant_usage_events`, which names no call, so a
/// message takes the usage row written for its session just before it.
fn copilot_cli(root: &Path, since: i64) -> Result<Vec<Call>> {
    let store = root.join("session-store.db");
    let db = if store.exists() {
        Some(open_ro(&store)?)
    } else {
        None
    };
    let mut out = Vec::new();
    for path in walk(&root.join("session-state"), 2, since, &|p| {
        ends(p, "events.jsonl")
    }) {
        let session = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        for r in lines(&path, &mut |l| {
            l.contains("semlith") && l.contains("\"assistant.message\"")
        })? {
            let at = text(&r, "/timestamp")
                .and_then(|t| iso_secs(&t))
                .unwrap_or(0);
            let usage = match &db {
                Some(db) => copilot_usage(db, &session, at)?,
                None => None,
            };
            let Some((request, model, tokens)) = usage else {
                continue;
            };
            for t in r
                .pointer("/data/toolRequests")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let name = text(t, "/mcpToolName")
                    .or_else(|| text(t, "/name"))
                    .unwrap_or_default();
                if text(t, "/mcpServerName").is_some_and(|s| s != "semlith") {
                    continue;
                }
                let Some(tool) = semlith_tool(&name) else {
                    continue;
                };
                out.push(call(
                    at,
                    tool,
                    t.get("arguments").map(Value::to_string).unwrap_or_default(),
                    Found {
                        model: Some(model.clone()),
                        provider: Some("github-copilot".into()),
                        tokens: Some(tokens),
                        cost: None,
                        source: source(&store, &request),
                    },
                ));
            }
        }
    }
    Ok(out)
}

fn copilot_usage(
    db: &rusqlite::Connection,
    session: &str,
    at: i64,
) -> Result<Option<(String, String, Tokens)>> {
    let mut q = db.prepare(
        "SELECT id, model, COALESCE(input_tokens, 0), COALESCE(output_tokens, 0),
                COALESCE(cache_read_tokens, 0), COALESCE(cache_write_tokens, 0),
                COALESCE(reasoning_tokens, 0), created_at
           FROM assistant_usage_events WHERE session_id = ?1",
    )?;
    let rows = q.query_map([session], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            [
                r.get::<_, i64>(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ],
            r.get::<_, String>(7)?,
        ))
    })?;
    let mut best: Option<(i64, (String, String, Tokens))> = None;
    for row in rows {
        let (id, model, [input, output, read, write, reasoning], created) = row?;
        let Some(when) = iso_secs(&created) else {
            continue;
        };
        let gap = at - when;
        if !(0..=super::WINDOW).contains(&gap) || best.as_ref().is_some_and(|(g, _)| *g <= gap) {
            continue;
        }
        best = Some((
            gap,
            (
                format!("usage{id}"),
                model,
                tokens_incl_cache(input, output, read, write, reasoning),
            ),
        ));
    }
    Ok(best.map(|(_, b)| b))
}

// -------------------------------------------------- GitHub Copilot in VS Code

/// VS Code's chat history keeps usage per turn, not per request. Per request
/// is only in the agent debug log, `…/GitHub.copilot-chat/debug-logs/<session>/
/// main.jsonl`, written when `chat.agentDebugLog.fileLogging.enabled` is on: a
/// call takes the last `llm_request` that finished before it started.
fn vscode_debug_logs(user: &Path, since: i64) -> Vec<PathBuf> {
    walk(user, 5, since, &|p| {
        ends(p, "main.jsonl")
            && p.to_string_lossy()
                .to_ascii_lowercase()
                .contains("copilot-chat")
            && p.to_string_lossy().contains("debug-logs")
    })
}

fn any_vscode_debug_log(user: &Path) -> bool {
    !vscode_debug_logs(user, 0).is_empty()
}

fn vscode(logs: &[PathBuf]) -> Result<Vec<Call>> {
    let mut out = Vec::new();
    for path in logs {
        let records = lines(path, &mut |l| {
            l.contains("\"llm_request\"") || l.contains("semlith_")
        })?;
        let mut last: Option<(i64, String, Option<String>, Tokens)> = None;
        for r in records {
            let ts = int(&r, "/ts");
            match text(&r, "/type").as_deref() {
                Some("llm_request") => {
                    let a = r.get("attrs").cloned().unwrap_or_default();
                    let tokens = tokens_incl_cache(
                        int(&a, "/inputTokens"),
                        int(&a, "/outputTokens"),
                        int(&a, "/cachedTokens"),
                        0,
                        0,
                    );
                    let request = text(&r, "/spanId").unwrap_or_default();
                    last = Some((ts + int(&r, "/dur"), request, text(&a, "/model"), tokens));
                }
                Some("tool_call") => {
                    let Some(tool) = text(&r, "/name").as_deref().and_then(semlith_tool) else {
                        continue;
                    };
                    let Some((_, request, model, tokens)) =
                        last.clone().filter(|(end, ..)| *end <= ts)
                    else {
                        continue;
                    };
                    out.push(call(
                        ts / 1000,
                        tool,
                        text(&r, "/attrs/args").unwrap_or_default(),
                        Found {
                            model,
                            provider: Some("github-copilot".into()),
                            tokens: Some(tokens),
                            cost: None,
                            source: source(path, &request),
                        },
                    ));
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

// ------------------------------------------------------------------ Cline

/// `~/.cline/data/sessions/<id>/<id>.messages.json`: each assistant message
/// with its tool calls, its model and its own metrics, cost included.
fn cline(root: &Path, since: i64) -> Result<Vec<Call>> {
    let mut out = Vec::new();
    for path in walk(root, 2, since, &|p| ends(p, ".messages.json")) {
        let Ok(doc) = serde_json::from_str::<Value>(&std::fs::read_to_string(&path)?) else {
            continue;
        };
        for m in doc
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if text(m, "/role").as_deref() != Some("assistant") {
                continue;
            }
            let provider = text(m, "/modelInfo/provider");
            let (input, read, write) = (
                int(m, "/metrics/inputTokens"),
                int(m, "/metrics/cacheReadTokens"),
                int(m, "/metrics/cacheWriteTokens"),
            );
            // Anthropic reports input without the cache; the others Cline
            // talks to report it with.
            let tokens = if provider.as_deref() == Some("anthropic") {
                Tokens {
                    input,
                    output: int(m, "/metrics/outputTokens"),
                    cache_read: read,
                    cache_write: write,
                    reasoning: 0,
                    cache_write_1h: 0,
                }
            } else {
                tokens_incl_cache(input, int(m, "/metrics/outputTokens"), read, write, 0)
            };
            for block in m
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if text(block, "/type").as_deref() != Some("tool_use") {
                    continue;
                }
                let Some(tool) = text(block, "/name").as_deref().and_then(semlith_tool) else {
                    continue;
                };
                out.push(call(
                    int(m, "/ts") / 1000,
                    tool,
                    block.get("input").map(Value::to_string).unwrap_or_default(),
                    Found {
                        model: text(m, "/modelInfo/id"),
                        provider: provider.clone(),
                        tokens: Some(tokens),
                        cost: m.pointer("/metrics/cost").and_then(Value::as_f64),
                        source: source(&path, &text(m, "/id").unwrap_or_default()),
                    },
                ));
            }
        }
    }
    Ok(out)
}

// ----------------------------------------------------------------- IO CLI

/// `~/.io-cli/runs.db`: a semlith call is an `mcp_events` row, and its
/// request is the successful `provider_calls` attempt of the same run and
/// step. `prompt_tokens` includes the cache.
fn io_cli(db: &Path, since: i64) -> Result<Vec<Call>> {
    if !db.exists() {
        return Ok(Vec::new());
    }
    let conn = open_ro(db)?;
    let mut q = conn.prepare(
        "SELECT me.tool, pc.id, pc.model, pc.provider, COALESCE(pc.prompt_tokens, 0),
                COALESCE(pc.completion_tokens, 0), COALESCE(pc.cache_read_tokens, 0),
                COALESCE(pc.cache_write_tokens, 0), COALESCE(pc.reasoning_tokens, 0),
                COALESCE((SELECT MIN(ta.started_at) FROM tool_attempts ta
                           WHERE ta.run_id = me.run_id AND ta.step = me.step AND ta.tool = me.tool), pc.at),
                COALESCE((SELECT st.calls FROM step_turns st
                           WHERE st.run_id = me.run_id AND st.step = me.step), '')
           FROM mcp_events me
           JOIN provider_calls pc ON pc.run_id = me.run_id AND pc.step = me.step AND pc.failure IS NULL
          WHERE me.kind = 'called' AND me.server = 'semlith'",
    )?;
    let rows = q.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            [
                r.get::<_, i64>(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
            ],
            r.get::<_, String>(9)?,
            r.get::<_, String>(10)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (tool, id, model, provider, [input, output, read, write, reasoning], at, args) = row?;
        let (Some(tool), Some(at)) = (semlith_tool(&tool), iso_secs(&at)) else {
            continue;
        };
        if at < since {
            continue;
        }
        out.push(call(
            at,
            tool,
            args,
            Found {
                model,
                provider,
                tokens: Some(tokens_incl_cache(input, output, read, write, reasoning)),
                cost: None,
                source: source(db, &format!("provider_call{id}")),
            },
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_AT: i64 = 1_790_762_400; // 2026-09-30T10:00:00Z

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/usage")
            .join(name)
    }

    /// A fixture laid out where the reader looks, in a scratch directory.
    fn placed(name: &str, at: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(at);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy(fixture(name), &path).unwrap();
        (dir, path)
    }

    fn database(sql: &str, at: &Path) {
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        let db = rusqlite::Connection::open(at).unwrap();
        db.execute_batch(&std::fs::read_to_string(fixture(sql)).unwrap())
            .unwrap();
    }

    fn one(calls: &[Call]) -> &Call {
        assert_eq!(calls.len(), 1, "{calls:#?}");
        &calls[0]
    }

    #[test]
    fn iso_times_read_in_every_shape_the_logs_write() {
        assert_eq!(iso_secs("2026-09-30T10:00:00.000Z"), Some(FIXTURE_AT));
        assert_eq!(iso_secs("2026-09-30 10:00:02"), Some(FIXTURE_AT + 2));
        assert_eq!(iso_secs("2026-09-30T15:30:00+05:30"), Some(FIXTURE_AT));
        assert_eq!(iso_secs("not a time"), None);
    }

    #[test]
    fn claude_code_takes_the_message_that_issued_the_call() {
        let (dir, _) = placed("claude-code.jsonl", "projects/-tmp-proj/s.jsonl");
        let calls = claude_code(&[dir.path().join("projects")], 0).unwrap();
        let c = one(&calls);
        assert_eq!((c.tool.as_str(), c.at), ("search", FIXTURE_AT + 2));
        assert!(c.args.contains("parser"));
        assert_eq!(c.found.model.as_deref(), Some("claude-opus-5"));
        assert_eq!(
            c.found.tokens,
            Some(Tokens {
                input: 3,
                output: 95,
                cache_read: 51_000,
                cache_write: 420,
                reasoning: 40,
                cache_write_1h: 420,
            })
        );
        assert!(c.found.source.ends_with("#req_fixture0001"));
    }

    #[test]
    fn codex_takes_the_first_per_request_count_after_the_call() {
        let (dir, _) = placed("codex.jsonl", "sessions/2026/09/30/rollout-a.jsonl");
        let calls = codex(&dir.path().join("sessions"), 0, false).unwrap();
        let c = one(&calls);
        assert_eq!((c.tool.as_str(), c.at), ("search", FIXTURE_AT + 3));
        assert_eq!(c.found.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(c.found.provider.as_deref(), Some("openai"));
        // 20 000 input of which 8 000 cached; not the cumulative 40 300.
        assert_eq!(
            c.found.tokens,
            Some(Tokens {
                input: 12_000,
                output: 150,
                cache_read: 8_000,
                cache_write: 0,
                reasoning: 100,
                cache_write_1h: 0,
            })
        );
        // The CLI's rollout is not the desktop app's.
        assert!(
            codex(&dir.path().join("sessions"), 0, true)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn the_desktop_apps_code_mode_call_takes_its_exec_request() {
        let (dir, _) = placed(
            "codex-desktop-codemode.jsonl",
            "sessions/2026/09/30/rollout-b.jsonl",
        );
        let calls = codex(&dir.path().join("sessions"), 0, true).unwrap();
        let c = one(&calls);
        assert_eq!(c.tool, "search");
        assert_eq!(
            c.found.tokens,
            Some(Tokens {
                input: 2_000,
                output: 110,
                cache_read: 37_000,
                cache_write: 0,
                reasoning: 30,
                cache_write_1h: 0,
            })
        );
        assert!(
            codex(&dir.path().join("sessions"), 0, false)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn opencode_reads_its_database_and_keeps_its_own_cost() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode/opencode.db");
        database("opencode.sql", &db);
        let calls = opencode(&db, 0).unwrap();
        let c = one(&calls);
        assert_eq!((c.tool.as_str(), c.at), ("search", FIXTURE_AT + 3));
        assert_eq!(
            c.found.model.as_deref(),
            Some("deepseek/deepseek-v4-flash-0731")
        );
        assert_eq!(c.found.provider.as_deref(), Some("openrouter"));
        assert_eq!(c.found.cost, Some(0.00069));
        assert_eq!(
            c.found.tokens,
            Some(Tokens {
                input: 33_697,
                output: 230,
                cache_read: 512,
                cache_write: 0,
                reasoning: 141,
                cache_write_1h: 0,
            })
        );
        assert!(opencode(&db, FIXTURE_AT + 3_600).unwrap().is_empty());
    }

    #[test]
    fn gemini_takes_the_last_copy_of_a_message() {
        let (dir, _) = placed("gemini-cli.jsonl", "tmp/proj/chats/session-1.jsonl");
        let calls = gemini(&dir.path().join("tmp"), 0).unwrap();
        let c = one(&calls);
        assert_eq!((c.tool.as_str(), c.at), ("search", FIXTURE_AT + 2));
        assert_eq!(
            c.found.tokens,
            Some(Tokens {
                input: 2_800,
                output: 52,
                cache_read: 16_000,
                cache_write: 0,
                reasoning: 12,
                cache_write_1h: 0,
            })
        );
    }

    #[test]
    fn copilot_cli_joins_the_usage_row_written_just_before() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".copilot");
        let events = root.join("session-state/00000000-0000-4000-8000-0000000cop01/events.jsonl");
        std::fs::create_dir_all(events.parent().unwrap()).unwrap();
        std::fs::copy(fixture("copilot-cli.events.jsonl"), &events).unwrap();
        database(
            "copilot-cli.session-store.sql",
            &root.join("session-store.db"),
        );
        let calls = copilot_cli(&root, 0).unwrap();
        let c = one(&calls);
        assert_eq!((c.tool.as_str(), c.at), ("search", FIXTURE_AT + 3));
        assert_eq!(c.found.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(
            c.found.tokens,
            Some(Tokens {
                input: 5_000,
                output: 120,
                cache_read: 30_000,
                cache_write: 0,
                reasoning: 50,
                cache_write_1h: 0,
            })
        );
    }

    #[test]
    fn vscode_takes_the_request_that_finished_before_the_call() {
        let (dir, _) = placed(
            "vscode-copilot.debug-log.main.jsonl",
            "User/workspaceStorage/h/GitHub.copilot-chat/debug-logs/s/main.jsonl",
        );
        let logs = vscode_debug_logs(&dir.path().join("User"), 0);
        let c = one(&vscode(&logs).unwrap()).clone();
        assert_eq!((c.tool.as_str(), c.at), ("search", FIXTURE_AT + 2));
        assert!(c.found.source.ends_with("#a1a1a1a1a1a1a1a1"));
        assert_eq!(
            c.found.tokens,
            Some(Tokens {
                input: 8_800,
                output: 60,
                cache_read: 20_000,
                cache_write: 0,
                reasoning: 0,
                cache_write_1h: 0,
            })
        );
    }

    #[test]
    fn cline_keeps_the_messages_own_metrics() {
        let (dir, _) = placed(
            "cline.messages.json",
            "sessions/1790762400000_abcde/1790762400000_abcde.messages.json",
        );
        let calls = cline(&dir.path().join("sessions"), 0).unwrap();
        let c = one(&calls);
        assert_eq!((c.tool.as_str(), c.at), ("search", FIXTURE_AT + 3));
        assert_eq!(c.found.cost, Some(0.00103));
        assert_eq!(c.found.tokens.unwrap().input, 23_800);
    }

    #[test]
    fn io_cli_joins_the_successful_attempt_of_the_step() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join(".io-cli/runs.db");
        database("io-cli.sql", &db);
        let calls = io_cli(&db, 0).unwrap();
        let c = one(&calls);
        assert_eq!((c.tool.as_str(), c.at), ("search", FIXTURE_AT + 2));
        assert!(c.args.contains("parser"));
        assert_eq!(
            c.found.tokens,
            Some(Tokens {
                input: 28_650,
                output: 122,
                cache_read: 0,
                cache_write: 0,
                reasoning: 44,
                cache_write_1h: 0,
            })
        );
    }

    #[test]
    fn every_supported_client_has_an_answer() {
        for client in [
            "Claude Code",
            "OpenAI Codex",
            "ChatGPT desktop (the Codex app)",
            "OpenCode",
            "Gemini CLI",
            "GitHub Copilot CLI",
            "GitHub Copilot in VS Code",
            "Zed",
            "Cline",
            "Cursor",
            "Claude Desktop",
            "IO CLI",
        ] {
            assert!(kind(client).is_some(), "{client} has no reader");
        }
        assert_eq!(kind("portal"), None);
    }
}
