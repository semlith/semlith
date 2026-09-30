//! The model, tokens and cost behind each ledger row, from the AI client's own
//! session log.
//!
//! MCP carries none of the three: a tool call arrives with its arguments and
//! nothing about the model that decided to make it. Most clients write a log
//! of every model request to disk, with the tool calls each one issued and the
//! tokens it spent, so the answer is on this machine already — in another
//! program's files.
//!
//! **This reads files semlith does not own, so it is off unless turned on**,
//! by `home::Settings::ledger_usage` (the Privacy page, or `semlith ledger
//! --usage on`). Logs are opened read-only, and from each call only the model
//! id and the numbers are kept: never a prompt, a reply or a file's text.
//!
//! Each client is a reader in [`readers`] that turns its log into [`Call`]s —
//! a semlith tool call, when it was made, and the request that made it. The
//! matching of calls to ledger rows is one function for every client,
//! [`assign`], so a reader only has to parse.

use crate::prices::{self, Tokens};
use crate::store::{Filled, Unfilled};
use anyhow::Result;

mod readers;

/// How far back a ledger row is still looked for. Clients prune their own
/// logs, and a row older than this whose log is gone would be read for on
/// every visit to the Ledger page.
pub const LOOKBACK: i64 = 30 * 86_400;

/// How long after a call a missing match is still "not yet". Clients write a
/// request's usage when the reply finishes, after semlith has already answered
/// and written its row; past this, the row is marked as not found so it is not
/// looked for again.
pub const GRACE: i64 = 15 * 60;

/// The furthest apart, in seconds, a log's call and a ledger row may be and
/// still be one call. semlith writes its row as it answers, and a client
/// stamps the call when the model emitted it; a slow tool or a queued call
/// puts minutes between them, and a clock never does.
pub const WINDOW: i64 = 300;

/// One semlith tool call, as a client's log records it.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    /// Unix seconds.
    pub at: i64,
    /// The tool without its prefixes: `search`, `brief`, `read`.
    pub tool: String,
    /// The call's arguments as the log holds them, for telling two calls a
    /// second apart by what they asked.
    pub args: String,
    pub found: Found,
}

/// The model request that made a call.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    pub model: Option<String>,
    /// Who served it, when the log says: OpenCode's `providerID`, Codex's
    /// `model_provider`.
    pub provider: Option<String>,
    pub tokens: Option<Tokens>,
    /// The client's own figure, when it records one.
    pub cost: Option<f64>,
    /// The log file this came from.
    pub source: String,
}

/// What a reader can say about a client.
#[derive(Debug)]
pub enum Log {
    /// The semlith calls in the logs it read.
    Calls(Vec<Call>),
    /// The client keeps nothing this could read, and why — said on every row
    /// rather than guessed at.
    NotRecorded(String),
}

/// The short tool name in a log's tool name, or `None` for a tool that is not
/// semlith's: `mcp__semlith__semlith_search`, `semlith.semlith_search` and
/// `semlith_search` are all `search`.
pub fn semlith_tool(name: &str) -> Option<String> {
    let at = name.rfind("semlith_")?;
    if at > 0 && name.as_bytes()[at - 1].is_ascii_alphanumeric() {
        return None;
    }
    let tool = &name[at + "semlith_".len()..];
    (!tool.is_empty()).then(|| tool.to_string())
}

/// Whether `args`, a log's record of a call's arguments, carries `query`.
fn asked(args: &str, query: &str) -> bool {
    if query.is_empty() {
        return false;
    }
    if args.contains(query) {
        return true;
    }
    // Logs hold arguments as JSON, where a quote or a newline in the query is
    // escaped.
    let escaped = serde_json::to_string(query).unwrap_or_default();
    escaped.len() > 2 && args.contains(&escaped[1..escaped.len() - 1])
}

/// Match rows to calls and say what each row gets.
///
/// For each row, oldest first: the calls to the same tool within [`WINDOW`]
/// that nothing has taken yet, those whose arguments carry the row's query
/// first, the nearest in time among them. A row with no call is left alone
/// until it is older than [`GRACE`], then marked so it is not looked for again.
///
/// `taken` holds the calls rows have already claimed, across clients: a row
/// that names only its MCP library is matched against every client's log, and
/// must not take a call a row naming its app already has.
pub fn assign(
    rows: &[Unfilled],
    calls: &[Call],
    client: &str,
    now: i64,
    table: &prices::Table,
    taken: &mut std::collections::HashSet<String>,
) -> Vec<(Unfilled, Filled)> {
    let key = |c: &Call| {
        format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            c.found.source, c.tool, c.at, c.args
        )
    };
    let mut out = Vec::new();
    for row in rows {
        let best = calls
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.tool == row.tool && (c.at - row.at).abs() <= WINDOW && !taken.contains(&key(c))
            })
            .min_by_key(|(_, c)| (!asked(&c.args, &row.query), (c.at - row.at).abs()))
            .map(|(i, _)| i);
        match best {
            Some(i) => {
                taken.insert(key(&calls[i]));
                out.push((row.clone(), priced(&calls[i].found, table)));
            }
            None if now - row.at > GRACE => out.push((
                row.clone(),
                Filled {
                    usage_source: if crate::clientid::generic(client) {
                        format!("no matching call in any client's log (the row names {client:?}, not an app)")
                    } else {
                        format!("no matching call in {client}'s log")
                    },
                    ..Default::default()
                },
            )),
            None => {}
        }
    }
    out
}

/// A found request as the row stores it: the client's own cost where it
/// keeps one, else the tokens at the table's price, else no cost.
pub fn priced(found: &Found, table: &prices::Table) -> Filled {
    let (cost_usd, cost_source) = match (found.cost, &found.model, found.tokens) {
        (Some(cost), _, _) => (Some(cost), Some("client".to_string())),
        (None, Some(model), Some(tokens)) => {
            match prices::lookup(table, found.provider.as_deref(), model) {
                Some((_, price)) => (Some(prices::cost(price, &tokens)), Some(table.label())),
                None => (None, None),
            }
        }
        _ => (None, None),
    };
    Filled {
        model: found.model.clone(),
        tokens: found.tokens,
        cost_usd,
        cost_source,
        usage_source: found.source.clone(),
    }
}

/// Every place the readers would look on this machine, per client, for the
/// Privacy page: what turning the setting on lets semlith open.
pub fn log_paths() -> Vec<serde_json::Value> {
    let home = crate::home::user_home()
        .map(|h| crate::plain(&h.display().to_string()))
        .unwrap_or_default();
    // `~/.claude/projects` rather than the whole home, which is the same on
    // every row and pushes the part that differs off the edge of the card.
    let short = |p: &std::path::PathBuf| {
        let full = crate::plain(&p.display().to_string());
        match full.strip_prefix(&home) {
            Some(rest) if !home.is_empty() => format!("~{rest}"),
            _ => full,
        }
    };
    readers::paths()
        .into_iter()
        .map(|(client, paths)| {
            serde_json::json!({
                "client": client,
                "paths": paths.iter().map(short).collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// Whether the ledger may read client logs at all.
pub fn enabled() -> bool {
    crate::home::Settings::load().ledger_usage.unwrap_or(false)
}

/// Fill in what can be filled for one store's unfilled rows. Returns how many
/// rows were written.
///
/// Does nothing — opens no log — unless [`enabled`]. Each client's logs are
/// read once per call, and only files changed since the oldest row it has to
/// place.
pub fn enrich(db: &rusqlite::Connection, now: i64) -> Result<usize> {
    if !enabled() {
        return Ok(0);
    }
    let rows = crate::store::unfilled(db, now - LOOKBACK)?;
    if rows.is_empty() {
        return Ok(0);
    }
    let table = prices::table();
    let mut by_client: std::collections::BTreeMap<String, Vec<Unfilled>> = Default::default();
    for row in rows {
        by_client.entry(row.client.clone()).or_default().push(row);
    }
    // Rows that name their app first, so a row naming only its MCP library
    // takes what they leave.
    let mut clients: Vec<(String, Vec<Unfilled>)> = by_client.into_iter().collect();
    clients.sort_by_key(|(client, _)| crate::clientid::generic(client));
    let mut taken = std::collections::HashSet::new();
    let mut written = 0;
    for (client, rows) in clients {
        let since = rows.iter().map(|r| r.at).min().unwrap_or(now) - WINDOW;
        let filled = match readers::read(&client, since) {
            Ok(Log::Calls(calls)) => assign(&rows, &calls, &client, now, &table, &mut taken),
            Ok(Log::NotRecorded(why)) => rows
                .into_iter()
                .map(|row| {
                    let filled = Filled {
                        usage_source: why.clone(),
                        ..Default::default()
                    };
                    (row, filled)
                })
                .collect(),
            // A log that would not read is this client's problem for now,
            // not the ledger's: the rows stay unfilled and are tried again.
            Err(_) => continue,
        };
        for (row, filled) in &filled {
            crate::store::fill_usage(db, row, filled)?;
        }
        written += filled.len();
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, at: i64, tool: &str, query: &str) -> Unfilled {
        Unfilled {
            id,
            at,
            client: "Claude Code".into(),
            session: String::new(),
            tool: tool.into(),
            query: query.into(),
            query_id: format!("q{id}"),
        }
    }

    fn call(at: i64, tool: &str, args: &str, model: &str) -> Call {
        Call {
            at,
            tool: tool.into(),
            args: args.into(),
            found: Found {
                model: Some(model.into()),
                tokens: Some(Tokens {
                    input: 10,
                    output: 20,
                    cache_read: 1_000,
                    cache_write: 0,
                    reasoning: 0,
                    cache_write_1h: 0,
                }),
                source: "log.jsonl".into(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn tool_names_are_read_under_every_prefix() {
        assert_eq!(
            semlith_tool("mcp__semlith__semlith_search").as_deref(),
            Some("search")
        );
        assert_eq!(
            semlith_tool("semlith.semlith_read").as_deref(),
            Some("read")
        );
        assert_eq!(semlith_tool("semlith_brief").as_deref(), Some("brief"));
        assert_eq!(semlith_tool("mysemlith_search"), None);
        assert_eq!(semlith_tool("Read"), None);
    }

    #[test]
    fn rows_match_the_call_that_asked_nearest_in_time() {
        let table = prices::built_in();
        let rows = [
            row(1, 1_000, "search", "where is x"),
            row(2, 1_001, "search", "who calls \"y\""),
        ];
        let calls = [
            call(
                999,
                "search",
                r#"{"query":"who calls \"y\""}"#,
                "claude-opus-4-5",
            ),
            call(
                998,
                "search",
                r#"{"query":"where is x"}"#,
                "claude-sonnet-4-5",
            ),
            call(
                1_000,
                "read",
                r#"{"target":"where is x"}"#,
                "claude-haiku-4-5",
            ),
        ];
        let got = assign(
            &rows,
            &calls,
            "Claude Code",
            1_010,
            &table,
            &mut Default::default(),
        );
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].1.model.as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(got[1].1.model.as_deref(), Some("claude-opus-4-5"));
        assert!(
            got[0]
                .1
                .cost_source
                .as_deref()
                .unwrap()
                .starts_with("models.dev ")
        );
        assert!(got[0].1.cost_usd.unwrap() > 0.0);
    }

    #[test]
    fn a_call_one_client_took_is_not_taken_again() {
        let table = prices::built_in();
        let calls = [call(1_000, "search", r#"{"query":"q"}"#, "claude-opus-4-5")];
        let mut taken = Default::default();
        let first = assign(
            &[row(1, 1_000, "search", "q")],
            &calls,
            "Claude Code",
            1_010,
            &table,
            &mut taken,
        );
        assert_eq!(first.len(), 1);
        let second = assign(
            &[row(2, 1_000, "search", "q")],
            &calls,
            "mcp",
            1_010,
            &table,
            &mut taken,
        );
        assert!(second.is_empty(), "{second:?}");
    }

    #[test]
    fn a_row_waits_for_its_log_then_is_marked() {
        let table = prices::built_in();
        let rows = [row(1, 1_000, "search", "q")];
        assert!(
            assign(
                &rows,
                &[],
                "Zed",
                1_000 + GRACE,
                &table,
                &mut Default::default()
            )
            .is_empty()
        );
        let late = assign(
            &rows,
            &[],
            "Zed",
            1_001 + GRACE,
            &table,
            &mut Default::default(),
        );
        assert_eq!(late[0].1.usage_source, "no matching call in Zed's log");
        assert_eq!(late[0].1.model, None);
        // Outside the window is not a match, however well the query fits.
        let far = [call(1_000 + WINDOW + 1, "search", r#"{"query":"q"}"#, "m")];
        assert_eq!(
            assign(&rows, &far, "Zed", 1_000, &table, &mut Default::default()).len(),
            0
        );
    }

    #[test]
    fn the_clients_own_cost_wins_and_an_unknown_model_has_none() {
        let table = prices::built_in();
        let mut found = call(0, "search", "", "claude-opus-4-5").found;
        found.cost = Some(0.0123);
        let filled = priced(&found, &table);
        assert_eq!(
            (filled.cost_usd, filled.cost_source.as_deref()),
            (Some(0.0123), Some("client"))
        );
        found.cost = None;
        found.model = Some("a-model-nobody-prices".into());
        let filled = priced(&found, &table);
        assert_eq!((filled.cost_usd, filled.cost_source), (None, None));
        assert!(filled.tokens.is_some());
    }
}
