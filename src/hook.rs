//! The steering hook: one line, at the moment an agent is about to read a file
//! semlith already holds.
//!
//! The skill teaches an agent that the tools exist. This is what happens when
//! it forgets — a `PreToolUse` hook on the client's own read and grep tools
//! that names the semlith call which would answer the same question for a
//! fraction of the tokens. It never blocks by default, because a hook that
//! refuses work an agent is right to do is a hook a user removes on the first
//! bad day, and a removed hook steers nobody.
//!
//! Three properties make it safe to run inside another process's tool call:
//!
//! * It decides from `registry.json` alone. No store is opened, no lock is
//!   taken, and a file no registered root covers costs one path comparison.
//! * It never emits a `permissionDecision` unless `--strict` was asked for.
//!   Answering `allow` would auto-approve a read the user's own permission
//!   rules were about to be asked about, which is semlith deciding something it
//!   was not asked to decide.
//! * The ledger row goes through a running daemon, or not at all. Opening a
//!   store's SQLite from inside a client's tool call is how a read ends up
//!   waiting on an index run.

use crate::home::Registry;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long the ledger write may take before the hook gives up on it.
///
/// Short on purpose. The hook is on a 50 ms budget inside somebody else's tool
/// call, and a row that does not get written is a figure stated as a floor —
/// which is a far smaller cost than a read that hangs.
const LEDGER_TIMEOUT: Duration = Duration::from_millis(400);

/// What a client sends a `PreToolUse` hook.
///
/// Only the fields semlith reads. A client that adds more is unaffected, and a
/// client that omits one it should have sent produces no nudge rather than an
/// error inside its own tool call.
#[derive(Debug, Deserialize)]
pub struct Event {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub tool_name: String,
    #[serde(default)]
    pub tool_input: ToolInput,
}

#[derive(Debug, Default, Deserialize)]
pub struct ToolInput {
    #[serde(default)]
    pub file_path: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub pattern: Option<String>,
    /// A `Read` that names either of these is reading part of a file, not the
    /// whole of it, and is already doing the thing this hook exists to suggest.
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// What the hook decided to do about one event.
#[derive(Debug, PartialEq)]
pub enum Decision {
    /// Say nothing. A file no store holds, a bounded read, a tool this hook is
    /// not about.
    Quiet,
    /// Add one line naming the call that would have answered it.
    Nudge(String),
    /// Refuse this once, under `--strict`, and say the same thing.
    Refuse(String),
}

/// Which semlith call answers this read, and what it is about.
///
/// A path and the store that holds it, decided from the registry. `None` means
/// there is nothing to say.
fn covered(path: &Path) -> Option<(String, PathBuf)> {
    let registry = Registry::load().ok()?;
    let absolute = crate::canonical(path);
    let (name, _) = registry.covering(&absolute)?;
    Some((name.to_string(), absolute))
}

/// The line an agent reads.
///
/// One sentence, naming a call it can make now. Two would be a paragraph in the
/// middle of somebody's tool call, and the thing being asked for is small.
fn line(target: &str, whole_file: bool) -> String {
    if whole_file {
        format!(
            "semlith indexes {target}. `semlith_brief` answers a question about it in one call \
             under a token budget, and `semlith_read` returns one span or one symbol — either \
             costs a fraction of this file read whole."
        )
    } else {
        format!(
            "semlith indexes {target}. `semlith_search` finds the spans this pattern is looking \
             for, ranked, without reading every file that matches it."
        )
    }
}

/// Decide what to say about one event, without touching anything.
///
/// Split from [`run`] so the decision is testable without a registry, a daemon
/// or a client: `covered` is the only thing that reads the machine, and it is
/// passed in.
pub fn decide(
    event: &Event,
    strict: bool,
    first_of_session: bool,
    covers: impl Fn(&Path) -> Option<(String, PathBuf)>,
) -> (Decision, Option<PathBuf>) {
    let (target, whole_file) = match event.tool_name.as_str() {
        // A bounded read is already the shape this hook would ask for.
        "Read" if event.tool_input.offset.is_some() || event.tool_input.limit.is_some() => {
            return (Decision::Quiet, None);
        }
        "Read" => match &event.tool_input.file_path {
            Some(p) => (PathBuf::from(p), true),
            None => return (Decision::Quiet, None),
        },
        // A grep with no path is the repository-wide one this is about; a grep
        // scoped to a directory is still a scan of every file under it.
        "Grep" => {
            let at = event.tool_input.path.clone().unwrap_or_else(|| ".".into());
            (PathBuf::from(at), false)
        }
        _ => return (Decision::Quiet, None),
    };

    let Some((_store, absolute)) = covers(&target) else {
        return (Decision::Quiet, None);
    };

    let said = line(&display(&absolute), whole_file);
    // Strict refuses once per session and then gets out of the way. A hook that
    // refuses every read turns into a hook that gets uninstalled, and the first
    // refusal is the one that is read anyway.
    let decision = if strict && first_of_session {
        Decision::Refuse(said)
    } else {
        Decision::Nudge(said)
    };
    // Only a whole-file read is a refund. A grep read no file whole, so
    // counting it would inflate the figure the release exists to defend.
    (decision, whole_file.then_some(absolute))
}

/// The path as a person reading the line would recognise it: relative to the
/// working directory when it is under it, absolute otherwise.
fn display(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// The JSON a client expects back.
///
/// `additionalContext` adds the line without deciding anything. A
/// `permissionDecision` of `allow` would approve a read the user's own rules
/// were about to be consulted about, so it is never sent — the only decision
/// this hook ever makes is `deny`, under `--strict`, and only once.
fn answer(decision: &Decision) -> Option<String> {
    let body = match decision {
        Decision::Quiet => return None,
        Decision::Nudge(said) => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "additionalContext": said,
            }
        }),
        Decision::Refuse(said) => serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": said,
            }
        }),
    };
    Some(body.to_string())
}

/// Read one event, answer it, and record the raw read if a daemon is up.
///
/// Every failure is silence and exit 0. This runs inside another program's tool
/// call, where an error on stderr is noise in somebody's terminal and a non-zero
/// exit is a broken client.
pub fn run(input: &str, strict: bool, client: &str) -> String {
    let Ok(event) = serde_json::from_str::<Event>(input) else {
        return String::new();
    };
    let first = strict && !seen(&event.session_id);
    let (decision, refund) = decide(&event, strict, first, covered);
    if let Decision::Refuse(_) = decision {
        mark(&event.session_id);
    }
    if let Some(path) = refund {
        record(&path, client, &event.session_id);
    }
    answer(&decision).unwrap_or_default()
}

/// Where a strict session's first refusal is remembered.
fn sessions_dir() -> Option<PathBuf> {
    crate::home::home_or_error()
        .ok()
        .map(|h| h.join("hook-sessions"))
}

/// Whether this session has already had its one refusal.
fn seen(session: &str) -> bool {
    match (sessions_dir(), session.is_empty()) {
        // A client that sends no session id gets the nudge rather than a
        // refusal on every read, which is the safer way to be wrong.
        (_, true) => true,
        (Some(dir), false) => dir.join(sanitised(session)).exists(),
        (None, _) => true,
    }
}

fn mark(session: &str) {
    let Some(dir) = sessions_dir() else { return };
    if session.is_empty() {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(sanitised(session)), b"");
    prune(&dir);
}

/// One marker per session, and sessions end.
///
// ponytail: a full directory scan per refusal, which happens at most once per
// session. A session count large enough to matter would want an index, and by
// then the markers belong in the store rather than in files.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let week = Duration::from_secs(7 * 24 * 60 * 60);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().unwrap_or_default() > week)
            .unwrap_or(false);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A session id is a client's string, and it becomes a filename here.
fn sanitised(session: &str) -> String {
    session
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(64)
        .collect()
}

/// Ask a running daemon to record the raw read.
///
/// Through the daemon because it is the process holding the store open. With no
/// daemon there is no row, and the Ledger page says the figure is a floor
/// rather than pretending the read did not happen.
fn record(path: &Path, client: &str, session: &str) {
    if !crate::ledger::enabled() {
        return;
    }
    let Some(store_dir) = store_dir_for(path) else {
        return;
    };
    let Some(upstream) = crate::proxy::find(&[store_dir]) else {
        return;
    };
    let _ = upstream.raw_read(&path.display().to_string(), client, session, LEDGER_TIMEOUT);
}

/// The store directory of the store whose roots cover `path`.
fn store_dir_for(path: &Path) -> Option<PathBuf> {
    let registry = Registry::load().ok()?;
    let (name, _) = registry.covering(path)?;
    Registry::dir_of(name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(tool: &str, input: serde_json::Value) -> Event {
        serde_json::from_value(serde_json::json!({
            "session_id": "s1",
            "tool_name": tool,
            "tool_input": input,
        }))
        .unwrap()
    }

    fn held(_: &Path) -> Option<(String, PathBuf)> {
        Some(("proj".into(), PathBuf::from("/proj/src/store.rs")))
    }

    fn unheld(_: &Path) -> Option<(String, PathBuf)> {
        None
    }

    #[test]
    fn a_whole_file_read_of_an_indexed_file_is_nudged_and_refunded() {
        let e = event(
            "Read",
            serde_json::json!({ "file_path": "/proj/src/store.rs" }),
        );
        let (decision, refund) = decide(&e, false, false, held);
        let Decision::Nudge(said) = decision else {
            panic!("a held file must be nudged: {decision:?}");
        };
        assert!(
            said.contains("semlith_brief"),
            "the line names no call: {said}"
        );
        assert_eq!(refund, Some(PathBuf::from("/proj/src/store.rs")));
    }

    /// The whole point of deciding from the registry: a file outside every
    /// registered root is somebody else's business.
    #[test]
    fn a_file_no_store_holds_is_silent() {
        let e = event(
            "Read",
            serde_json::json!({ "file_path": "/elsewhere/x.rs" }),
        );
        assert_eq!(decide(&e, false, false, unheld).0, Decision::Quiet);
    }

    /// A bounded read is already the shape the nudge would ask for, so nudging
    /// it would be telling an agent to do what it is doing.
    #[test]
    fn a_bounded_read_is_silent() {
        for bound in ["offset", "limit"] {
            let e = event(
                "Read",
                serde_json::json!({ "file_path": "/proj/src/store.rs", bound: 10 }),
            );
            assert_eq!(decide(&e, false, false, held).0, Decision::Quiet, "{bound}");
        }
    }

    /// A grep is worth a line and is not a refund: it read no file whole, and
    /// counting it would inflate the figure this release exists to defend.
    #[test]
    fn a_grep_is_nudged_but_never_refunded() {
        let e = event("Grep", serde_json::json!({ "pattern": "fn main" }));
        let (decision, refund) = decide(&e, false, false, held);
        assert!(matches!(decision, Decision::Nudge(_)), "{decision:?}");
        assert_eq!(refund, None, "a grep read no file whole");
    }

    /// A tool this hook is not about must cost one string comparison and say
    /// nothing at all.
    #[test]
    fn another_tool_is_silent() {
        let e = event("Bash", serde_json::json!({ "command": "ls" }));
        assert_eq!(decide(&e, false, false, held).0, Decision::Quiet);
    }

    /// Strict refuses the first qualifying read of a session and then reverts,
    /// so the message is read once rather than fought with all day.
    #[test]
    fn strict_refuses_once_and_then_nudges() {
        let e = event(
            "Read",
            serde_json::json!({ "file_path": "/proj/src/store.rs" }),
        );
        assert!(matches!(
            decide(&e, true, true, held).0,
            Decision::Refuse(_)
        ));
        assert!(matches!(
            decide(&e, true, false, held).0,
            Decision::Nudge(_)
        ));
    }

    /// `allow` would approve a read the user's own permission rules were about
    /// to be consulted about. The only decision this hook ever sends is `deny`.
    #[test]
    fn a_nudge_decides_nothing_and_a_refusal_only_denies() {
        let nudge = answer(&Decision::Nudge("x".into())).unwrap();
        assert!(
            !nudge.contains("permissionDecision"),
            "a nudge must not decide: {nudge}"
        );
        assert!(nudge.contains("additionalContext"), "{nudge}");

        let refusal = answer(&Decision::Refuse("x".into())).unwrap();
        assert!(
            refusal.contains("\"permissionDecision\":\"deny\""),
            "{refusal}"
        );

        assert_eq!(answer(&Decision::Quiet), None);
    }

    /// A payload semlith does not understand is not an error inside somebody
    /// else's tool call.
    #[test]
    fn an_unreadable_payload_is_silence() {
        assert_eq!(run("not json", false, "claude-code"), "");
        assert_eq!(run("{}", false, "claude-code"), "");
    }
}
