//! The steering hook: one line, at the moment an agent is about to look
//! something up the slow way in a folder semlith already indexes.
//!
//! The skill teaches an agent that the tools exist. This is what happens when
//! it forgets — a `PreToolUse` hook on the client's own `Bash`, `Read`, `Grep`
//! and `Glob` tools that names one concrete semlith call answering the same
//! question for a fraction of the tokens. From 0.30.0 it reads `Bash` too:
//! on 2026-09-25 agents looked things up with `grep`, `rg`, `sed -n` and `cat`
//! far more often than with the `Read` and `Grep` tools the first hook matched.
//!
//! Three modes. `soft`, the default, never blocks: a hook that refuses work an
//! agent is right to do is a hook a user removes on the first bad day. `gate`
//! refuses raw lookups until the session has made one semlith call, at most
//! twice, then nudges. `hard` always refuses `grep`, `rg` and `find` in an
//! indexed root. A `PostToolUse` entry on the semlith tools records that a
//! session used semlith, which is what `gate` waits for.
//!
//! Safe to run inside another process's tool call:
//!
//! * It decides from `registry.json` alone. No store is opened, no lock is
//!   taken, and a path no registered root covers costs one comparison.
//! * It never answers `allow`: that would approve a call the user's own
//!   permission rules were about to be asked about. The only decision it ever
//!   sends is `deny`, and only in `gate` or `hard`.
//! * Any failure — an unparsable command, a missing registry — is silence.

use crate::home::Registry;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long the ledger write may take before the hook gives up on it.
const LEDGER_TIMEOUT: Duration = Duration::from_millis(400);

/// Nudges a session gets before the hook goes quiet for it. Past three the
/// agent has read the line and decided; saying it again is noise.
const NUDGES: u32 = 3;

/// Refusals `gate` makes in one session before it only nudges.
const GATE_REFUSALS: u32 = 2;

/// How the hook steers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Nudge, never block. The default.
    Soft,
    /// Refuse raw lookups until the session has used semlith once.
    Gate,
    /// Always refuse `grep`, `rg` and `find` in an indexed root.
    Hard,
}

impl Mode {
    pub fn parse(raw: &str) -> anyhow::Result<Mode> {
        Ok(match raw {
            "soft" | "" => Mode::Soft,
            "gate" => Mode::Gate,
            "hard" => Mode::Hard,
            other => anyhow::bail!("the hook mode is soft, gate or hard, not {other:?}"),
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Soft => "soft",
            Mode::Gate => "gate",
            Mode::Hard => "hard",
        }
    }
}

/// What a client sends a hook. Only the fields semlith reads.
#[derive(Debug, Default, Deserialize)]
pub struct Event {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub hook_event_name: String,
    #[serde(default)]
    pub tool_name: String,
    #[serde(default)]
    pub cwd: Option<String>,
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
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// What the hook decided to do about one event.
#[derive(Debug, PartialEq)]
pub enum Decision {
    Quiet,
    /// Add one line naming the call that would have answered it.
    Nudge(String),
    /// Refuse the call, and say the same thing.
    Refuse(String),
}

/// What one session has done so far, as the hook remembers it.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, Deserialize)]
pub struct Session {
    /// The session has called a semlith tool.
    pub used: bool,
    pub nudges: u32,
    pub refusals: u32,
}

/// One raw lookup found in an event: what kind, the paths it touches, and
/// the concrete semlith call that answers it.
#[derive(Debug, PartialEq)]
struct Lookup {
    /// `grep`, `rg`, `find`, `fd`: a search, which `hard` refuses.
    search: bool,
    /// Paths named, relative to the working directory, or none for "here".
    paths: Vec<String>,
    call: String,
    /// A whole file read, which the ledger counts as a raw read.
    whole: Option<String>,
}

/// Decide what to say about one event, without touching anything.
///
/// `covers` is the only thing that reads the machine, and it is passed in so
/// the decision is testable without a registry.
pub fn decide(
    event: &Event,
    mode: Mode,
    session: &Session,
    covers: impl Fn(&Path) -> bool,
) -> (Decision, Option<PathBuf>) {
    let cwd = PathBuf::from(event.cwd.clone().unwrap_or_else(|| ".".into()));
    let Some(lookup) = lookup_of(event) else {
        return (Decision::Quiet, None);
    };
    let targets: Vec<PathBuf> = if lookup.paths.is_empty() {
        vec![cwd.clone()]
    } else {
        lookup.paths.iter().map(|p| cwd.join(p)).collect()
    };
    if !targets.iter().any(|t| covers(t)) {
        return (Decision::Quiet, None);
    }
    let said = format!(
        "semlith indexes this folder. Ask it instead: {}. It answers with file:line and the definition around each, for a fraction of the tokens.",
        lookup.call
    );
    let refund = lookup.whole.map(|p| cwd.join(p));
    let refuse = match mode {
        Mode::Soft => false,
        Mode::Gate => !session.used && session.refusals < GATE_REFUSALS,
        Mode::Hard => lookup.search,
    };
    if refuse {
        return (Decision::Refuse(said), refund);
    }
    if session.nudges >= NUDGES {
        return (Decision::Quiet, refund);
    }
    (Decision::Nudge(said), refund)
}

/// The raw lookup an event makes, if it makes one.
fn lookup_of(event: &Event) -> Option<Lookup> {
    let input = &event.tool_input;
    match event.tool_name.as_str() {
        "Read" => {
            let file = input.file_path.clone()?;
            let end = input.limit.map_or(80, |l| input.offset.unwrap_or(1) + l);
            let start = input.offset.unwrap_or(1).max(1);
            Some(Lookup {
                search: false,
                call: format!(
                    "semlith_read {{target: \"{file}:{start}-{end}\"}}, or semlith_brief {{question: \"<what you need from it>\"}}"
                ),
                whole: (input.offset.is_none() && input.limit.is_none()).then(|| file.clone()),
                paths: vec![file],
            })
        }
        "Grep" => {
            let pattern = input.pattern.clone().unwrap_or_default();
            Some(Lookup {
                search: true,
                call: search_call(&pattern),
                paths: input.path.clone().into_iter().collect(),
                whole: None,
            })
        }
        "Glob" => Some(Lookup {
            search: false,
            call: tree_call(input.path.as_deref()),
            paths: input.path.clone().into_iter().collect(),
            whole: None,
        }),
        "Bash" => bash_lookup(input.command.as_deref()?),
        _ => None,
    }
}

/// The first raw lookup in a shell command line.
///
/// Every segment of a compound command is read — `cd src && grep -rn x .` is a
/// grep — and a segment that searches nothing (`git log`, `cargo test`) is
/// passed over. A command the tokenizer cannot read is silence.
fn bash_lookup(command: &str) -> Option<Lookup> {
    for (piped, segment) in segments(command) {
        let words = split_words(segment.trim())?;
        let mut words: Vec<&str> = words.iter().map(String::as_str).collect();
        // Leading `sudo`, `time`, environment assignments.
        while let Some(first) = words.first() {
            if *first == "sudo"
                || *first == "time"
                || first.contains('=') && !first.starts_with('-')
            {
                words.remove(0);
            } else {
                break;
            }
        }
        let Some((&program, args)) = words.split_first() else {
            continue;
        };
        let program = program.rsplit('/').next().unwrap_or(program);
        let operands: Vec<String> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(|a| a.to_string())
            .collect();
        let flags = |f: &str| {
            args.iter()
                .any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains(f) || *a == f)
        };
        match program {
            "git" if args.first() == Some(&"grep") => {
                let rest: Vec<String> = args[1..]
                    .iter()
                    .filter(|a| !a.starts_with('-'))
                    .map(|a| a.to_string())
                    .collect();
                return Some(Lookup {
                    search: true,
                    call: search_call(rest.first().map(String::as_str).unwrap_or("")),
                    paths: rest.into_iter().skip(1).collect(),
                    whole: None,
                });
            }
            // A grep reading a pipe searches no file: `git status | grep x`.
            "grep" | "egrep" | "rg" | "ag" | "ack" if piped && operands.len() <= 1 => continue,
            "grep" | "egrep" | "rg" | "ag" | "ack" => {
                let pattern = operands.first().cloned().unwrap_or_default();
                return Some(Lookup {
                    search: true,
                    call: search_call(&pattern),
                    paths: operands.into_iter().skip(1).collect(),
                    whole: None,
                });
            }
            "find" | "fd" => {
                let listing = program == "fd"
                    || args
                        .iter()
                        .any(|a| matches!(*a, "-name" | "-iname" | "-type" | "-path"));
                if !listing {
                    continue;
                }
                let at = if program == "find" {
                    operands.first().cloned()
                } else {
                    operands.get(1).cloned()
                };
                return Some(Lookup {
                    search: true,
                    call: tree_call(at.as_deref()),
                    paths: at.into_iter().collect(),
                    whole: None,
                });
            }
            "tree" => {
                return Some(Lookup {
                    search: false,
                    call: tree_call(operands.first().map(String::as_str)),
                    paths: operands.into_iter().take(1).collect(),
                    whole: None,
                });
            }
            "ls" if flags("R") => {
                return Some(Lookup {
                    search: false,
                    call: tree_call(operands.first().map(String::as_str)),
                    paths: operands.into_iter().take(1).collect(),
                    whole: None,
                });
            }
            "cat" | "head" | "tail" | "less" | "bat" => {
                let Some(file) = operands.first().cloned() else {
                    continue;
                };
                return Some(Lookup {
                    search: false,
                    call: format!(
                        "semlith_read {{target: \"{file}:1-80\"}}, or semlith_brief {{question: \"<what you need from it>\"}}"
                    ),
                    whole: (program == "cat").then(|| file.clone()),
                    paths: vec![file],
                });
            }
            "sed" if flags("n") => {
                // `sed -n '10,40p' file`: the range is the first operand.
                let range = operands.first().cloned().unwrap_or_default();
                let file = operands.get(1).cloned()?;
                let lines = range.trim_end_matches('p').replace(',', "-");
                return Some(Lookup {
                    search: false,
                    call: format!("semlith_read {{target: \"{file}:{lines}\"}}"),
                    paths: vec![file],
                    whole: None,
                });
            }
            "awk" => {
                let file = operands.get(1).cloned()?;
                return Some(Lookup {
                    search: false,
                    call: format!("semlith_read {{target: \"{file}:1-80\"}}"),
                    paths: vec![file],
                    whole: None,
                });
            }
            _ => continue,
        }
    }
    None
}

/// The same sweep as `semlith_search {exact: true}`, which answers the lines
/// the grep would with the definition each sits in, and impact when the
/// pattern is a name, which is what a grep for a name usually wants to know.
fn search_call(pattern: &str) -> String {
    let clean = pattern.trim_matches(['\'', '"']);
    if clean.is_empty() {
        return "semlith_search {query: \"<what you are looking for>\"}".to_string();
    }
    let query = clean.replace('\\', "\\\\").replace('"', "\\\"");
    let bare = clean.replace("\\b", "");
    let bare = bare.trim_matches(['^', '$']);
    let name = bare
        .split(['|', '('])
        .next()
        .unwrap_or(bare)
        .trim_start_matches("fn ")
        .trim_start_matches("def ")
        .trim();
    let identifier = !name.is_empty()
        && !bare.contains('|')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == ':');
    let exact = format!("semlith_search {{query: \"{query}\", exact: true}}");
    if identifier {
        format!("{exact}, or semlith_impact {{name: \"{name}\"}} for every caller and call site")
    } else {
        exact
    }
}

fn tree_call(at: Option<&str>) -> String {
    match at
        .map(|a| a.trim_end_matches('/'))
        .filter(|a| !a.is_empty() && *a != ".")
    {
        Some(dir) => format!("semlith_files {{tree: true, path: [\"{dir}/**\"]}}"),
        None => "semlith_files {tree: true}".to_string(),
    }
}

/// A command line cut at `;`, `&`, `&&`, `||`, newlines and pipes outside
/// quotes, each piece marked when it reads a pipe.
///
/// Outside quotes only: `grep -E 'alpha|beta'` is one command, and cutting it
/// at the `|` left an unclosed quote that silenced the whole line.
fn segments(command: &str) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    let mut piece = String::new();
    let mut piped = false;
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => {
                quote = None;
                piece.push(c);
            }
            (Some(_), c) => piece.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                piece.push(c);
            }
            (None, '|') if chars.peek() != Some(&'|') => {
                out.push((piped, std::mem::take(&mut piece)));
                piped = true;
            }
            (None, '|' | ';' | '&' | '\n') => {
                if c == '|' {
                    chars.next();
                }
                out.push((piped, std::mem::take(&mut piece)));
                piped = false;
            }
            (None, c) => piece.push(c),
        }
    }
    out.push((piped, piece));
    out.retain(|(_, p)| !p.trim().is_empty());
    out
}

/// Shell words, honouring single and double quotes. `None` for an unclosed
/// quote, which the caller reads as silence.
fn split_words(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if !word.is_empty() || any {
                    words.push(std::mem::take(&mut word));
                    any = false;
                }
            }
            (None, c) => word.push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    if !word.is_empty() || any {
        words.push(word);
    }
    Some(words)
}

/// Whether any registered root covers `path`.
fn covered(path: &Path) -> bool {
    let Ok(registry) = Registry::load() else {
        return false;
    };
    let absolute = crate::canonical(path);
    registry.covering(&absolute).is_some()
}

/// The JSON a client expects back.
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

/// Read one event, answer it, and remember what the session did.
///
/// Every failure is silence and exit 0: this runs inside another program's
/// tool call.
pub fn run(input: &str, mode: Mode, client: &str) -> String {
    let Ok(event) = serde_json::from_str::<Event>(input) else {
        return String::new();
    };
    // `PostToolUse` on a semlith tool: the session has used semlith.
    if event.hook_event_name == "PostToolUse" {
        if event.tool_name.contains("semlith") {
            let mut state = load(&event.session_id);
            state.used = true;
            save(&event.session_id, &state);
        }
        return String::new();
    }
    let state = load(&event.session_id);
    let (decision, refund) = decide(&event, mode, &state, covered);
    let mut next = state.clone();
    match decision {
        Decision::Refuse(_) => next.refusals += 1,
        Decision::Nudge(_) => next.nudges += 1,
        Decision::Quiet => {}
    }
    if next != state {
        save(&event.session_id, &next);
    }
    if let Some(path) = refund
        && !matches!(decision, Decision::Quiet)
    {
        record(&path, client, &event.session_id, mode);
    }
    answer(&decision).unwrap_or_default()
}

/// Where per-session state lives: `~/.semlith/hook/`, pruned after a day.
fn state_dir() -> Option<PathBuf> {
    crate::home::home_or_error().ok().map(|h| h.join("hook"))
}

fn load(session: &str) -> Session {
    if session.is_empty() {
        return Session::default();
    }
    state_dir()
        .and_then(|d| std::fs::read(d.join(sanitised(session))).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save(session: &str, state: &Session) {
    let Some(dir) = state_dir() else { return };
    if session.is_empty() {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(bytes) = serde_json::to_vec(state) {
        let _ = std::fs::write(dir.join(sanitised(session)), bytes);
    }
    prune(&dir);
}

/// Sessions end; their state goes after a day.
///
// ponytail: a directory scan per state write. A session count large enough
// to matter would want an index.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let day = Duration::from_secs(24 * 60 * 60);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t.elapsed().unwrap_or_default() > day)
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

/// Ask a running daemon to record the raw read, with the mode that met it.
fn record(path: &Path, client: &str, session: &str, mode: Mode) {
    if !crate::ledger::enabled() {
        return;
    }
    let absolute = crate::canonical(path);
    let Some(store_dir) = store_dir_for(&absolute) else {
        return;
    };
    let Some(upstream) = crate::proxy::find(&[store_dir]) else {
        return;
    };
    let client = format!("{client} ({} hook)", mode.as_str());
    let _ = upstream.raw_read(
        &absolute.display().to_string(),
        &client,
        session,
        LEDGER_TIMEOUT,
    );
}

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
            "hook_event_name": "PreToolUse",
            "tool_name": tool,
            "cwd": "/proj",
            "tool_input": input,
        }))
        .unwrap()
    }

    fn bash(command: &str) -> Event {
        event("Bash", serde_json::json!({ "command": command }))
    }

    fn held(p: &Path) -> bool {
        p.starts_with("/proj")
    }

    #[test]
    fn a_grep_in_a_root_names_semlith_search_with_the_pattern() {
        let (d, _) = decide(
            &bash("grep -rn search_preferring src"),
            Mode::Soft,
            &Session::default(),
            held,
        );
        let Decision::Nudge(said) = d else {
            panic!("{d:?}")
        };
        assert!(
            said.contains("semlith_search {query: \"search_preferring\", exact: true}"),
            "{said}"
        );
        assert!(said.contains("semlith_impact"), "{said}");
    }

    #[test]
    fn a_regex_sweep_is_handed_over_whole_as_an_exact_search() {
        let (d, _) = decide(
            &bash(r"grep -rnE '\b(alpha|beta)\b' src/"),
            Mode::Soft,
            &Session::default(),
            held,
        );
        let Decision::Nudge(said) = d else {
            panic!("{d:?}")
        };
        assert!(
            said.contains(r#"semlith_search {query: "\\b(alpha|beta)\\b", exact: true}"#),
            "{said}"
        );
        assert!(!said.contains("semlith_impact"), "{said}");
    }

    #[test]
    fn a_compound_command_is_read_segment_by_segment() {
        let (d, _) = decide(
            &bash("cd src && rg -n 'fn index' ."),
            Mode::Soft,
            &Session::default(),
            held,
        );
        assert!(matches!(d, Decision::Nudge(_)), "{d:?}");
        let (d, _) = decide(
            &bash("git status | grep modified"),
            Mode::Soft,
            &Session::default(),
            held,
        );
        assert_eq!(d, Decision::Quiet, "a grep on a pipe searches no file");
    }

    #[test]
    fn git_and_cargo_commands_that_search_nothing_are_quiet() {
        for c in [
            "git log",
            "git diff HEAD",
            "cargo test",
            "ls src",
            "echo grep",
        ] {
            assert_eq!(
                decide(&bash(c), Mode::Soft, &Session::default(), held).0,
                Decision::Quiet,
                "{c}"
            );
        }
    }

    #[test]
    fn anything_outside_a_root_is_quiet() {
        let mut e = bash("grep -rn x .");
        e.cwd = Some("/elsewhere".into());
        assert_eq!(
            decide(&e, Mode::Hard, &Session::default(), held).0,
            Decision::Quiet
        );
    }

    #[test]
    fn gate_refuses_twice_then_nudges() {
        let e = bash("grep -rn x src");
        let mut s = Session::default();
        for _ in 0..2 {
            assert!(matches!(
                decide(&e, Mode::Gate, &s, held).0,
                Decision::Refuse(_)
            ));
            s.refusals += 1;
        }
        assert!(matches!(
            decide(&e, Mode::Gate, &s, held).0,
            Decision::Nudge(_)
        ));
    }

    #[test]
    fn after_a_semlith_call_gate_never_refuses() {
        let s = Session {
            used: true,
            ..Session::default()
        };
        let (d, _) = decide(&bash("grep -rn x src"), Mode::Gate, &s, held);
        assert!(!matches!(d, Decision::Refuse(_)), "{d:?}");
    }

    #[test]
    fn hard_refuses_search_commands_but_not_cat() {
        let s = Session {
            used: true,
            ..Session::default()
        };
        assert!(matches!(
            decide(&bash("rg foo"), Mode::Hard, &s, held).0,
            Decision::Refuse(_)
        ));
        assert!(matches!(
            decide(&bash("find . -name '*.rs'"), Mode::Hard, &s, held).0,
            Decision::Refuse(_)
        ));
        assert!(!matches!(
            decide(&bash("cat src/lib.rs"), Mode::Hard, &s, held).0,
            Decision::Refuse(_)
        ));
    }

    #[test]
    fn soft_never_refuses_and_goes_quiet_after_three() {
        let e = bash("grep -rn x src");
        let mut s = Session::default();
        for _ in 0..3 {
            assert!(matches!(
                decide(&e, Mode::Soft, &s, held).0,
                Decision::Nudge(_)
            ));
            s.nudges += 1;
        }
        assert_eq!(decide(&e, Mode::Soft, &s, held).0, Decision::Quiet);
    }

    #[test]
    fn reads_listings_and_sed_ranges_get_a_concrete_call() {
        let (d, refund) = decide(
            &event("Read", serde_json::json!({ "file_path": "/proj/src/a.rs" })),
            Mode::Soft,
            &Session::default(),
            held,
        );
        assert!(
            matches!(d, Decision::Nudge(ref s) if s.contains("semlith_read")),
            "{d:?}"
        );
        assert_eq!(refund, Some(PathBuf::from("/proj/src/a.rs")));
        // A bounded read is nudged too, and is not a whole-file refund.
        let (d, refund) = decide(
            &event(
                "Read",
                serde_json::json!({ "file_path": "/proj/src/a.rs", "offset": 10, "limit": 20 }),
            ),
            Mode::Soft,
            &Session::default(),
            held,
        );
        assert!(
            matches!(d, Decision::Nudge(ref s) if s.contains("a.rs:10-30")),
            "{d:?}"
        );
        assert_eq!(refund, None);
        let (d, _) = decide(
            &bash("sed -n '130,160p' src/lib.rs"),
            Mode::Soft,
            &Session::default(),
            held,
        );
        assert!(
            matches!(d, Decision::Nudge(ref s) if s.contains("src/lib.rs:130-160")),
            "{d:?}"
        );
        for listing in ["ls -R src", "tree src", "find src -type f"] {
            let (d, _) = decide(&bash(listing), Mode::Soft, &Session::default(), held);
            assert!(
                matches!(d, Decision::Nudge(ref s) if s.contains("tree: true")),
                "{listing}: {d:?}"
            );
        }
    }

    #[test]
    fn a_nudge_decides_nothing_and_a_refusal_only_denies() {
        let nudge = answer(&Decision::Nudge("x".into())).unwrap();
        assert!(!nudge.contains("permissionDecision"), "{nudge}");
        let refusal = answer(&Decision::Refuse("x".into())).unwrap();
        assert!(
            refusal.contains("\"permissionDecision\":\"deny\""),
            "{refusal}"
        );
        assert_eq!(answer(&Decision::Quiet), None);
    }

    #[test]
    fn an_unreadable_payload_or_command_is_silence() {
        assert_eq!(run("not json", Mode::Gate, "claude-code"), "");
        assert_eq!(run("{}", Mode::Gate, "claude-code"), "");
        assert_eq!(
            decide(
                &bash("grep 'unclosed"),
                Mode::Hard,
                &Session::default(),
                held
            )
            .0,
            Decision::Quiet
        );
    }

    /// The 50 ms budget, with the Bash parse added, over a thousand events.
    #[test]
    fn a_thousand_decisions_stay_inside_the_budget() {
        let e = bash("cd src && grep -rn 'fn search_preferring' . | head -20");
        let started = std::time::Instant::now();
        for _ in 0..1000 {
            let _ = decide(&e, Mode::Soft, &Session::default(), held);
        }
        let each = started.elapsed() / 1000;
        assert!(each < Duration::from_millis(50), "{each:?} a decision");
    }
}
