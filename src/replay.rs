//! What an agent did after semlith answered.
//!
//! The ledger knows what was retrieved and what it cost. It cannot know
//! whether the answer was enough — that is in the agent's own transcript, in
//! what it reached for next. A whole-file read after a retrieval is a refund:
//! semlith was asked and the agent read the file anyway. A grep is a miss. An
//! edit is the answer having sufficed.
//!
//! **This reads files semlith does not own, so it is off unless turned on.**
//! `home::Settings::session_replay` gates every entry point here, the Privacy
//! page is where it is turned on, and nothing read here is ever sent
//! anywhere — the same rule the ledger itself keeps.
//!
//! Claude Code only, in 0.26.0. Its transcripts are the ones on the reference
//! machine, and a parser written against a format nobody here can run is not
//! evidence that the feature works. Another client is a new `Reader`, not a
//! branch in this one.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// The client whose transcripts this release reads.
pub const CLIENT: &str = "claude-code";

/// Most transcripts to read in one pass, newest first.
///
/// A machine that has run agents for months holds thousands, and the panel is
/// about what happened lately. The count of what was skipped is reported.
pub const FILES: usize = 40;

/// Most bytes to read from one transcript.
///
/// A long session's transcript runs to tens of megabytes of tool output, and
/// the tail is the part that matters. Past this the file's head is skipped.
pub const BYTES: u64 = 8 * 1024 * 1024;

/// What the agent did after an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It read a whole file anyway. semlith was asked and did not save the read.
    Refund,
    /// It grepped. semlith answered and the answer was not the one it wanted.
    Miss,
    /// It edited. The answer was enough to act on.
    Sufficed,
    /// It did something else, or nothing before the transcript ended.
    Unknown,
}

impl Outcome {
    pub fn word(self) -> &'static str {
        match self {
            Outcome::Refund => "read the whole file",
            Outcome::Miss => "grepped anyway",
            Outcome::Sufficed => "edited",
            Outcome::Unknown => "nothing recorded",
        }
    }
}

/// One transcript, counted.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Session {
    /// The transcript's own id, which is its file name. Not semlith's session
    /// id: the two are minted by different processes and joining them would
    /// be a guess.
    pub id: String,
    /// The project directory the transcript sits under, as the client names it.
    pub project: String,
    /// Unix seconds of the file's last write.
    pub last: i64,
    /// semlith tool calls in this transcript.
    pub answers: usize,
    pub refund: usize,
    pub miss: usize,
    pub sufficed: usize,
    pub unknown: usize,
}

/// Everything the replay pass found.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Replay {
    pub client: String,
    pub sessions: Vec<Session>,
    /// Transcripts left unread at [`FILES`], so a short list is not read as
    /// the whole history.
    pub skipped: usize,
    /// Where the transcripts were read from, so a reader can check.
    pub from: String,
}

/// How many tool calls after an answer still count as "what it did next".
///
/// Three, because an agent commonly reads its own previous output or checks a
/// path in between. Past that it has moved on to something else and calling
/// the edit a consequence of the retrieval would be a story rather than a
/// reading.
const WINDOW: usize = 3;

/// Classify what followed one semlith call, from the tool names after it.
///
/// A pure function over names, so the rule is testable without a transcript:
/// the first of the next few calls that means something decides, and the
/// order of precedence is the order they are checked in.
pub fn outcome_of(following: &[String]) -> Outcome {
    for name in following.iter().take(WINDOW) {
        match kind_of(name) {
            Some(outcome) => return outcome,
            None => continue,
        }
    }
    Outcome::Unknown
}

/// What one tool name means, or nothing when it means nothing here.
fn kind_of(name: &str) -> Option<Outcome> {
    // A semlith call is not a consequence of the previous semlith call; it is
    // the next question.
    if is_semlith(name) {
        return Some(Outcome::Unknown);
    }
    match name {
        "Read" | "NotebookRead" => Some(Outcome::Refund),
        "Grep" | "Glob" => Some(Outcome::Miss),
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => Some(Outcome::Sufficed),
        _ => None,
    }
}

/// Whether a tool name is one of semlith's, under any client's prefixing.
///
/// Claude Code advertises an MCP tool as `mcp__<server>__<tool>`, and the
/// server name is whatever the user called it in their config — so the test
/// is the tool name, not the whole string.
pub fn is_semlith(name: &str) -> bool {
    name.rsplit("__")
        .next()
        .is_some_and(|tail| tail.starts_with("semlith_"))
}

/// Where this client keeps its transcripts.
///
/// Through `home::user_home`, which is the one place in `src/` that reads the
/// environment for a home directory.
pub fn transcripts_dir() -> Result<PathBuf> {
    Ok(crate::home::user_home()?.join(".claude").join("projects"))
}

/// Read the transcripts and count what followed each semlith answer.
///
/// The caller is responsible for the toggle: this function reads files the
/// moment it is called.
pub fn read(dir: &Path, limit: usize) -> Result<Replay> {
    let mut found: Vec<(i64, PathBuf, String)> = Vec::new();
    let Ok(projects) = std::fs::read_dir(dir) else {
        return Ok(Replay {
            client: CLIENT.to_string(),
            from: crate::plain(&dir.display().to_string()),
            ..Default::default()
        });
    };
    for project in projects.flatten() {
        let Ok(entries) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let modified = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let label = project
                .file_name()
                .to_string_lossy()
                .trim_start_matches('-')
                .to_string();
            found.push((modified, path, label));
        }
    }
    found.sort_by_key(|(at, _, _)| std::cmp::Reverse(*at));
    let skipped = found.len().saturating_sub(limit);
    found.truncate(limit);

    let mut sessions = Vec::new();
    for (last, path, project) in found {
        let Some(session) = one(&path, &project, last) else {
            continue;
        };
        if session.answers > 0 {
            sessions.push(session);
        }
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.last));
    Ok(Replay {
        client: CLIENT.to_string(),
        sessions,
        skipped,
        from: crate::plain(&dir.display().to_string()),
    })
}

/// One transcript's counts, or nothing when it cannot be read.
fn one(path: &Path, project: &str, last: i64) -> Option<Session> {
    let text = read_tail(path, BYTES)?;
    let mut calls: Vec<String> = Vec::new();
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            // A partial line at the head of a tail read, or a format this
            // build does not know. Skipped rather than guessed at.
            continue;
        };
        let content = event
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array());
        for part in content.into_iter().flatten() {
            if part.get("type").and_then(|t| t.as_str()) != Some("tool_use") {
                continue;
            }
            if let Some(name) = part.get("name").and_then(|n| n.as_str()) {
                calls.push(name.to_string());
            }
        }
    }

    let mut session = Session {
        id: path.file_stem()?.to_string_lossy().to_string(),
        project: project.to_string(),
        last,
        ..Default::default()
    };
    for (i, name) in calls.iter().enumerate() {
        if !is_semlith(name) {
            continue;
        }
        session.answers += 1;
        match outcome_of(&calls[i + 1..]) {
            Outcome::Refund => session.refund += 1,
            Outcome::Miss => session.miss += 1,
            Outcome::Sufficed => session.sufficed += 1,
            Outcome::Unknown => session.unknown += 1,
        }
    }
    Some(session)
}

/// The last `bytes` of a file, as text, starting at the next line break.
fn read_tail(path: &Path, bytes: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len > bytes {
        file.seek(SeekFrom::Start(len - bytes)).ok()?;
    }
    let mut raw = Vec::new();
    file.read_to_end(&mut raw).ok()?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    if len > bytes {
        // The first line is half a line.
        return Some(
            text.split_once('\n')
                .map(|(_, rest)| rest)
                .unwrap_or_default()
                .to_string(),
        );
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_tool_name_is_semlith_whatever_the_server_was_called() {
        assert!(is_semlith("mcp__semlith__semlith_search"));
        assert!(is_semlith("mcp__my-index__semlith_impact"));
        assert!(is_semlith("semlith_trace"));
        assert!(!is_semlith("Read"));
        assert!(!is_semlith("mcp__other__search"));
    }

    #[test]
    fn what_followed_decides_and_the_first_meaningful_call_wins() {
        assert_eq!(outcome_of(&names(&["Read"])), Outcome::Refund);
        assert_eq!(outcome_of(&names(&["Grep", "Edit"])), Outcome::Miss);
        assert_eq!(
            outcome_of(&names(&["TodoWrite", "Edit"])),
            Outcome::Sufficed
        );
        // A second semlith call is the next question, not a consequence.
        assert_eq!(
            outcome_of(&names(&["mcp__semlith__semlith_read", "Edit"])),
            Outcome::Unknown
        );
        // Nothing within the window is nothing, not a guess.
        assert_eq!(
            outcome_of(&names(&["TodoWrite", "TodoWrite", "TodoWrite", "Edit"])),
            Outcome::Unknown
        );
        assert_eq!(outcome_of(&[]), Outcome::Unknown);
    }
}
