//! The three things `semlith setup` writes for an agent beyond its MCP
//! registration: the Agent Skill, the steering hook, and the rule block.
//!
//! Registering the server tells a client that semlith exists. None of these
//! does anything a registration does — they are what makes an agent *use* it.
//! The skill teaches; the hook catches the moment it forgets; the rule block is
//! for clients that read prose and have no skill directory.
//!
//! Every path here comes from an annotated fence in `docs/clients.md`, except
//! the cross-client skill directory below, which belongs to no client. Nothing
//! in this file is a path retyped into Rust.
//!
//! # What this writes, and what it leaves alone
//!
//! Every file semlith does not own is backed up beside itself before its first
//! write, and every write is reversible:
//!
//! * The skill is a link into a canonical copy under the store home. Removing
//!   the canonical directory removes all of them.
//! * The hook is one entry in a client's `PreToolUse` array. It is found again
//!   by the command it runs, so `setup --no-hooks` takes out exactly the entry
//!   semlith put there and leaves every other hook in place.
//! * The rule block sits between two markers. A second run replaces what is
//!   between them; nothing outside them is read or written.

use crate::clientfile;
use crate::clients::{self, Client, Stanza};
use crate::home;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The skill directory several clients read that belongs to none of them.
///
/// In code rather than in a fence because `docs/clients.md` is a file of client
/// facts, and this is a cross-client convention: it is not Claude Code's
/// directory or Qwen's, it is the one anybody's agent may look in.
const SHARED_SKILLS: &str = ".agents/skills";

/// The skill's name, which is its directory's name everywhere it is linked.
pub const SKILL_NAME: &str = "semlith";

/// What surrounds the rule block in a file semlith does not own.
const RULES_BEGIN: &str = "<!-- >>> semlith >>> -->";
const RULES_END: &str = "<!-- <<< semlith <<< -->";

/// Whether a thing semlith writes is there, absent, or there but out of date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// There, and saying what this version says.
    Present,
    /// Not there at all.
    Absent,
    /// There, but written by a different binary or an older block.
    Stale,
    /// semlith knows of no file to write for this client, so a person pastes
    /// the block instead.
    Paste,
}

/// The canonical copy every link points at.
pub fn canonical_skill_dir() -> Result<PathBuf> {
    Ok(home::home_or_error()?.join("skills").join(SKILL_NAME))
}

/// Every user-level skill directory to link into: the cross-client one, then
/// each one a client documents.
pub fn skill_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(home) = home::user_home() {
        out.push(home.join(SHARED_SKILLS));
    }
    for client in clients::clients() {
        for stanza in client.skill_dirs() {
            if let Some(path) = clientfile::resolve(stanza) {
                out.push(path);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Write the canonical skill and link it into every documented directory.
///
/// Idempotent, and it says so: the returned list holds only the directories
/// this run actually changed, so a second run returns nothing and `semlith
/// setup` reports it as already done rather than as work.
pub fn install_skill() -> Result<Vec<PathBuf>> {
    let canonical = canonical_skill_dir()?;
    home::secure_dir(&canonical).with_context(|| format!("creating {}", canonical.display()))?;
    let file = canonical.join("SKILL.md");
    if std::fs::read_to_string(&file).ok().as_deref() != Some(clients::SKILL) {
        std::fs::write(&file, clients::SKILL)
            .with_context(|| format!("writing {}", file.display()))?;
    }

    let mut linked = Vec::new();
    for dir in skill_dirs() {
        let at = dir.join(SKILL_NAME);
        if link_state(&at, &canonical) == State::Present {
            continue;
        }
        if std::fs::create_dir_all(&dir).is_err() {
            continue;
        }
        // A link that points elsewhere is replaced; a real directory somebody
        // else put there is not, because it is not semlith's to remove.
        if at.is_symlink() {
            let _ = std::fs::remove_file(&at);
        }
        if link(&canonical, &at).is_ok() {
            linked.push(at);
        }
    }
    Ok(linked)
}

/// Remove the canonical skill and every link into it.
///
/// Only what semlith installed. A symlink is removed when it points at the
/// canonical directory and left alone when it points anywhere else; a real
/// directory is removed only when it holds exactly the skill semlith wrote and
/// nothing besides. Anything else belongs to whoever put it there, and a
/// directory named `semlith` under someone's skills is not proof that semlith
/// made it — `remove_dir_all` on that reasoning is how an opt-out takes a
/// user's own work with it.
///
/// The paths it declined are returned, so a caller can say so rather than
/// reporting a clean removal it did not perform.
pub fn remove_skill() -> Result<Vec<PathBuf>> {
    let canonical = canonical_skill_dir()?;
    let mut declined = Vec::new();
    for dir in skill_dirs() {
        let at = dir.join(SKILL_NAME);
        if at == canonical {
            continue;
        }
        if at.is_symlink() {
            match std::fs::read_link(&at) {
                Ok(target) if target == canonical => {
                    let _ = std::fs::remove_file(&at);
                }
                _ if at.exists() => declined.push(at),
                // A link into a directory that is gone, pointing at us. Ours.
                _ => {
                    let _ = std::fs::remove_file(&at);
                }
            }
            continue;
        }
        if !at.is_dir() {
            continue;
        }
        if ours_alone(&at) {
            let _ = std::fs::remove_dir_all(&at);
        } else {
            declined.push(at);
        }
    }
    let _ = std::fs::remove_dir_all(&canonical);
    Ok(declined)
}

/// Whether this directory holds the skill semlith wrote, and nothing else.
///
/// The Windows install is a copy rather than a link, so removal has to be able
/// to recognise its own copy. One file, whose text is the skill this binary
/// carries: a directory somebody has added to is not semlith's to delete, and
/// neither is one whose `SKILL.md` is somebody else's.
fn ours_alone(at: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(at) else {
        return false;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    if names != ["SKILL.md"] {
        return false;
    }
    std::fs::read_to_string(at.join("SKILL.md")).ok().as_deref() == Some(clients::SKILL)
}

/// Where the skill is, per directory.
pub fn skill_state() -> Vec<(PathBuf, State)> {
    let canonical = match canonical_skill_dir() {
        Ok(dir) => dir,
        Err(_) => return Vec::new(),
    };
    skill_dirs()
        .into_iter()
        .map(|dir| {
            let at = dir.join(SKILL_NAME);
            (at.clone(), link_state(&at, &canonical))
        })
        .collect()
}

/// Whether `at` is this machine's link to `canonical`.
fn link_state(at: &Path, canonical: &Path) -> State {
    if at.is_symlink() {
        return match std::fs::read_link(at) {
            Ok(target) if target == canonical => State::Present,
            _ => State::Stale,
        };
    }
    if !at.exists() {
        return State::Absent;
    }
    // A copy: Windows, where a symlink needs a privilege an installer should
    // not ask for. It is current when its text is.
    match std::fs::read_to_string(at.join("SKILL.md")) {
        Ok(text) if text == clients::SKILL => State::Present,
        _ => State::Stale,
    }
}

/// A symlink where the platform has them, a copy where it does not.
fn link(canonical: &Path, at: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(canonical, at)
            .with_context(|| format!("linking {} to {}", at.display(), canonical.display()))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        // A directory symlink on Windows needs SeCreateSymbolicLinkPrivilege,
        // which a user running an installer usually does not have. A copy is
        // the honest fallback: `doctor` reports it stale when the text moves on
        // and `setup` rewrites it.
        //
        // Into an empty or absent directory only. On unix the symlink above
        // fails when something is already there, which is the behaviour that
        // keeps semlith out of a directory it did not create; a bare
        // `create_dir_all` here would instead drop a `SKILL.md` into whatever
        // somebody already had at that path.
        if at.exists() && !ours_alone(at) {
            anyhow::bail!("{} already exists and is not semlith's skill", at.display());
        }
        std::fs::create_dir_all(at)?;
        std::fs::copy(canonical.join("SKILL.md"), at.join("SKILL.md"))
            .with_context(|| format!("copying the skill to {}", at.display()))?;
        Ok(())
    }
}

/// Where Claude Code reads user-level subagents, and semlith's file there.
fn explorer_path() -> Option<PathBuf> {
    crate::home::user_home()
        .ok()
        .map(|h| h.join(".claude/agents/semlith-explorer.md"))
}

/// Whether the semlith-explorer agent is written and current.
pub fn explorer_installed() -> bool {
    explorer_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|text| text == clients::EXPLORER)
}

/// Write the semlith-explorer agent for Claude Code, when Claude Code is on
/// this machine (its `~/.claude` exists). `None` when there was nothing to do.
pub fn install_explorer() -> Result<Option<PathBuf>> {
    let Some(path) = explorer_path() else {
        return Ok(None);
    };
    let claude = path.parent().and_then(Path::parent);
    if !claude.is_some_and(Path::exists) || explorer_installed() {
        return Ok(None);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    if path.exists() {
        clientfile::back_up(&path)?;
    }
    std::fs::write(&path, clients::EXPLORER)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(Some(path))
}

/// Take the agent out again. Only a file that is semlith's: one naming
/// itself `semlith-explorer` in its frontmatter.
pub fn remove_explorer() -> Result<Option<PathBuf>> {
    let Some(path) = explorer_path() else {
        return Ok(None);
    };
    match std::fs::read_to_string(&path) {
        Ok(text) if text.contains("name: semlith-explorer") => {
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
            Ok(Some(path))
        }
        _ => Ok(None),
    }
}

/// Set `"alwaysLoad": true` on Claude Code's user-scope semlith entry (4.1).
///
/// `claude mcp add` has no flag for it, so the entry it wrote is upgraded in
/// place, backed up first; an entry that already carries it is left alone.
/// `None` when there is no user-scope entry to upgrade.
pub fn set_always_load() -> Result<Option<(PathBuf, bool)>> {
    let Some(path) = crate::home::user_home()
        .ok()
        .map(|h| h.join(".claude.json"))
    else {
        return Ok(None);
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let mut config: serde_json::Value =
        serde_json::from_str(&text).context("~/.claude.json is not valid JSON; left as it was")?;
    let Some(entry) = config
        .get_mut("mcpServers")
        .and_then(|s| s.get_mut("semlith"))
        .and_then(|e| e.as_object_mut())
    else {
        return Ok(None);
    };
    if entry.get("alwaysLoad").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(Some((path, false)));
    }
    entry.insert("alwaysLoad".into(), serde_json::Value::Bool(true));
    clientfile::back_up(&path)?;
    let mut next = serde_json::to_string_pretty(&config)?;
    if text.ends_with('\n') {
        next.push('\n');
    }
    std::fs::write(&path, next).with_context(|| format!("writing {}", path.display()))?;
    Ok(Some((path, true)))
}

/// Every client that documents a `PreToolUse` hook, with the file it goes in.
pub fn hook_clients() -> Vec<(&'static Client, &'static Stanza, PathBuf)> {
    clients::clients()
        .iter()
        .filter_map(|client| {
            let stanza = client.hook_stanza()?;
            let path = clientfile::resolve(stanza)?;
            Some((client, stanza, path))
        })
        .collect()
}

/// The hook events semlith writes an entry under.
const HOOK_EVENTS: [&str; 2] = ["PreToolUse", "PostToolUse"];

/// Merge the hook into every client that documents one.
///
/// A mode other than soft writes `semlith hook --mode <mode>` on the
/// `PreToolUse` entry, which is a different command and so a different
/// entry: a machine switching between them has one hook, not two.
pub fn install_hooks(mode: crate::hook::Mode) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for (_, stanza, path) in hook_clients() {
        let wanted = entry(stanza, mode)?;
        let existing = std::fs::read_to_string(&path).ok();
        let next = with_entry(existing.as_deref().unwrap_or(""), &wanted)?;
        if existing.as_deref() == Some(next.as_str()) {
            continue;
        }
        write_json(&path, &next, existing.is_some())?;
        written.push(path);
    }
    Ok(written)
}

/// Take semlith's hook back out, leaving every other hook in the file.
pub fn remove_hooks() -> Result<Vec<PathBuf>> {
    let mut changed = Vec::new();
    for (_, _, path) in hook_clients() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let next = without_entry(&text)?;
        if next == text {
            continue;
        }
        write_json(&path, &next, true)?;
        changed.push(path);
    }
    Ok(changed)
}

/// Where the hook is, per client, for one mode.
pub fn hook_state(mode: crate::hook::Mode) -> Vec<(String, PathBuf, State)> {
    hook_clients()
        .into_iter()
        .map(|(client, stanza, path)| {
            let state = match (entry(stanza, mode), std::fs::read_to_string(&path)) {
                (Ok(wanted), Ok(text)) => match with_entry(&text, &wanted) {
                    Ok(next) if next == text => State::Present,
                    // Present under a different command — the other strictness,
                    // or a path from a binary that has since moved.
                    _ if names_our_hook(&text) => State::Stale,
                    _ => State::Absent,
                },
                _ => State::Absent,
            };
            (client.name.clone(), path, state)
        })
        .collect()
}

/// The hook entries semlith owns, from the documented fence.
fn entry(stanza: &Stanza, mode: crate::hook::Mode) -> Result<serde_json::Value> {
    let mut block: serde_json::Value = serde_json::from_str(&stanza.text)
        .context("the documented hook stanza is not valid JSON")?;
    if mode != crate::hook::Mode::Soft {
        // The mode on the command the fence already names, rather than a
        // second command assembled here. Only the `PreToolUse` entry decides.
        if let Some(command) = block
            .pointer("/hooks/PreToolUse/0/hooks/0/command")
            .and_then(|v| v.as_str().map(str::to_string))
        {
            block["hooks"]["PreToolUse"][0]["hooks"][0]["command"] =
                serde_json::Value::String(format!("{command} --mode {}", mode.as_str()));
        }
    }
    Ok(block)
}

/// The mode a settings file's semlith hook runs in, if it has one.
pub fn installed_mode(text: &str) -> Option<crate::hook::Mode> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let list = value.pointer("/hooks/PreToolUse")?.as_array()?;
    let ours = list.iter().find(|item| is_ours(item))?;
    let command = ours["hooks"][0]["command"].as_str()?;
    Some(if command.contains("--mode hard") {
        crate::hook::Mode::Hard
    } else if command.contains("--mode gate") || command.contains("--strict") {
        crate::hook::Mode::Gate
    } else {
        crate::hook::Mode::Soft
    })
}

/// Whether a settings file already holds a semlith hook, whatever its flags.
///
/// Read out of the parsed `PreToolUse` array rather than by looking for two
/// words anywhere in the file. A settings file that mentions semlith for any
/// other reason -- a permission rule, a status line, an `additionalDirectories`
/// entry -- and the word "hook" for any other reason was reported as carrying a
/// stale semlith hook when it carried none at all, which sends a reader to
/// repair something that is not there.
fn names_our_hook(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .map(|v| {
            HOOK_EVENTS.iter().any(|event| {
                v.pointer(&format!("/hooks/{event}"))
                    .and_then(|list| list.as_array())
                    .is_some_and(|list| list.iter().any(is_ours))
            })
        })
        .unwrap_or(false)
}

/// `text` with semlith's entry present exactly once, and every other entry
/// untouched.
///
/// The array is the part that matters. A plain recursive merge would replace
/// `hooks.PreToolUse` wholesale and take somebody's other hooks with it, which
/// is the kind of thing a person finds out about a week later.
fn with_entry(text: &str, wanted: &serde_json::Value) -> Result<String> {
    let mut base: serde_json::Value = if text.trim().is_empty() {
        serde_json::Value::Object(Default::default())
    } else {
        serde_json::from_str(text).context("the file is not valid JSON")?
    };
    for event in HOOK_EVENTS {
        let Some(ours) = wanted["hooks"][event].get(0).cloned() else {
            continue;
        };
        let list = base
            .as_object_mut()
            .context("the file is not a JSON object")?
            .entry("hooks")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .context("`hooks` is not an object")?
            .entry(event)
            .or_insert_with(|| serde_json::Value::Array(Vec::new()))
            .as_array_mut()
            .with_context(|| format!("`hooks.{event}` is not an array"))?;
        list.retain(|item| !is_ours(item));
        list.push(ours);
    }
    Ok(serialize(&base))
}

/// `text` with semlith's entries taken out and nothing else changed.
fn without_entry(text: &str) -> Result<String> {
    let mut base: serde_json::Value =
        serde_json::from_str(text).context("the file is not valid JSON")?;
    for event in HOOK_EVENTS {
        if let Some(list) = base
            .pointer_mut(&format!("/hooks/{event}"))
            .and_then(|v| v.as_array_mut())
        {
            list.retain(|item| !is_ours(item));
            let empty = list.is_empty();
            // An empty array semlith created is noise in somebody's settings file.
            if empty && let Some(hooks) = base.pointer_mut("/hooks").and_then(|v| v.as_object_mut())
            {
                hooks.remove(event);
            }
        }
    }
    if base
        .pointer("/hooks")
        .and_then(|v| v.as_object())
        .is_some_and(|h| h.is_empty())
    {
        base.as_object_mut().map(|o| o.remove("hooks"));
    }
    Ok(serialize(&base))
}

/// Whether one `PreToolUse` entry is the one semlith wrote.
///
/// By the command it runs, not by its position: a user may have reordered the
/// array, and the entry above semlith's is not semlith's to remove.
fn is_ours(item: &serde_json::Value) -> bool {
    item["hooks"]
        .as_array()
        .map(|inner| {
            inner.iter().any(|h| {
                h["command"]
                    .as_str()
                    .map(|c| c.contains("semlith") && c.contains(" hook"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Every client whose rules file semlith knows, and every one it does not.
///
/// The second list is what `doctor` prints for a person to paste. A rules file
/// is prose somebody owns, and guessing its path would write the block into a
/// place nothing reads.
pub fn rules_state() -> Vec<(String, Option<PathBuf>, State)> {
    clients::clients()
        .iter()
        .filter_map(|client| {
            let stanza = client.rules_file()?;
            let path = clientfile::resolve(stanza)?;
            let state = match std::fs::read_to_string(&path) {
                Ok(text) if between(&text) == Some(block()) => State::Present,
                Ok(text) if text.contains(RULES_BEGIN) => State::Stale,
                _ => State::Absent,
            };
            Some((client.name.clone(), Some(path), state))
        })
        .collect()
}

/// The rule block as it is written into a file, markers and all.
fn block() -> String {
    clients::RULES.trim_end().to_string()
}

/// What sits between the markers, if both are there.
fn between(text: &str) -> Option<String> {
    let start = text.find(RULES_BEGIN)? + RULES_BEGIN.len();
    let end = text[start..].find(RULES_END)? + start;
    Some(text[start..end].trim().to_string())
}

/// Write the rule block into every rules file semlith knows a path for.
///
/// Only under `--register-all`: a rules file is prose a person wrote, and
/// appending to it unasked is a different thing from merging a server into a
/// server list.
pub fn install_rules() -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for (_, path, state) in rules_state() {
        let Some(path) = path else { continue };
        if state == State::Present {
            continue;
        }
        let existing = std::fs::read_to_string(&path).ok();
        let next = with_block(existing.as_deref().unwrap_or(""));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        if existing.is_some() {
            clientfile::back_up(&path)?;
        }
        std::fs::write(&path, &next).with_context(|| format!("writing {}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

/// `text` with the block between the markers, appended if it was not there.
fn with_block(text: &str) -> String {
    let marked = format!("{RULES_BEGIN}\n{}\n{RULES_END}\n", block());
    let Some(start) = text.find(RULES_BEGIN) else {
        let separator = if text.trim().is_empty() { "" } else { "\n" };
        return format!("{}{separator}{marked}", text.trim_end());
    };
    let Some(end) = text[start..]
        .find(RULES_END)
        .map(|i| start + i + RULES_END.len())
    else {
        return format!("{}\n{marked}", text.trim_end());
    };
    format!("{}{marked}{}", &text[..start], text[end..].trim_start())
}

/// Pretty JSON with a trailing newline, the way every other file semlith writes
/// is shaped.
fn serialize(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default() + "\n"
}

fn write_json(path: &Path, next: &str, existed: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    if existed {
        clientfile::back_up(path)?;
    }
    // Through a neighbouring temporary file: a half-written settings file is a
    // client that will not start.
    let temp = path.with_extension("json.semlith-tmp");
    std::fs::write(&temp, next).with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ours() -> serde_json::Value {
        json!({
            "hooks": { "PreToolUse": [{
                "matcher": "Read|Grep",
                "hooks": [{ "type": "command", "command": "/opt/semlith hook" }]
            }]}
        })
    }

    /// The property the whole file exists for: somebody else's hooks survive.
    #[test]
    fn writing_the_hook_keeps_every_other_hook_in_the_file() {
        let theirs = json!({
            "hooks": { "PreToolUse": [{
                "matcher": "Bash",
                "hooks": [{ "type": "command", "command": "/usr/local/bin/audit" }]
            }]},
            "model": "something they chose"
        })
        .to_string();

        let next = with_entry(&theirs, &ours()).unwrap();
        assert!(next.contains("/usr/local/bin/audit"), "{next}");
        assert!(next.contains("/opt/semlith hook"), "{next}");
        assert!(next.contains("something they chose"), "{next}");
    }

    /// Twice is once. A hook that accumulates is a hook that runs four times by
    /// Friday.
    #[test]
    fn writing_the_hook_twice_leaves_one_entry() {
        let once = with_entry("{}", &ours()).unwrap();
        let twice = with_entry(&once, &ours()).unwrap();
        assert_eq!(once, twice, "a second write changed the file");
        assert_eq!(twice.matches("/opt/semlith hook").count(), 1, "{twice}");
    }

    /// Removal is exact: semlith's entry goes and the rest of the file comes
    /// back byte for byte as it was.
    #[test]
    fn removing_the_hook_restores_what_was_there_before() {
        let theirs = serialize(&json!({
            "hooks": { "PreToolUse": [{
                "matcher": "Bash",
                "hooks": [{ "type": "command", "command": "/usr/local/bin/audit" }]
            }]}
        }));
        let with = with_entry(&theirs, &ours()).unwrap();
        assert_eq!(
            without_entry(&with).unwrap(),
            theirs,
            "removing semlith's hook did not restore the file"
        );
    }

    /// A file semlith created entirely is left without the empty scaffolding it
    /// put there.
    #[test]
    fn removing_the_only_hook_leaves_no_empty_array_behind() {
        let with = with_entry("{}", &ours()).unwrap();
        let without = without_entry(&with).unwrap();
        assert!(!without.contains("PreToolUse"), "{without}");
        assert!(!without.contains("hooks"), "{without}");
    }

    /// An opt-out must not take somebody's own work with it.
    ///
    /// A directory named `semlith` under a user's skills is not proof that
    /// semlith made it, and `remove_dir_all` on that reasoning is data loss on
    /// the one path a cautious user reaches for.
    #[test]
    fn removal_declines_a_directory_semlith_did_not_write() {
        let dir = tempfile::tempdir().unwrap();

        // Ours: exactly the skill this binary carries, and nothing else.
        let ours = dir.path().join("ours");
        std::fs::create_dir_all(&ours).unwrap();
        std::fs::write(ours.join("SKILL.md"), clients::SKILL).unwrap();
        assert!(ours_alone(&ours), "semlith's own copy was not recognised");

        // Theirs: the same name, their content.
        let theirs = dir.path().join("theirs");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(theirs.join("SKILL.md"), "# my own skill\n").unwrap();
        assert!(!ours_alone(&theirs), "somebody else's skill read as ours");

        // Ours, that they then added to. Theirs now.
        let shared = dir.path().join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("SKILL.md"), clients::SKILL).unwrap();
        std::fs::write(shared.join("notes.md"), "mine\n").unwrap();
        assert!(
            !ours_alone(&shared),
            "a directory somebody added to is not semlith's to delete"
        );
    }

    /// A settings file that mentions semlith and the word "hook" for unrelated
    /// reasons carries no semlith hook, and must not be reported as carrying a
    /// stale one: that sends a reader to repair something that is not there.
    #[test]
    fn a_file_that_merely_mentions_semlith_holds_no_hook() {
        let theirs = json!({
            "permissions": { "allow": ["Bash(semlith:*)"] },
            "statusLine": { "command": "my-hook-script" },
            "hooks": { "PostToolUse": [{ "matcher": "Edit", "hooks": [] }] }
        })
        .to_string();
        assert!(!names_our_hook(&theirs), "{theirs}");

        let with = with_entry(&theirs, &ours()).unwrap();
        assert!(names_our_hook(&with), "{with}");
        // And it left their PostToolUse alone on the way in.
        assert!(with.contains("PostToolUse"), "{with}");
    }

    /// The rule block replaces itself rather than stacking up.
    #[test]
    fn the_rule_block_is_written_once_and_replaced_in_place() {
        let theirs = "# My rules\n\nAlways use tabs.\n";
        let once = with_block(theirs);
        assert!(once.contains("Always use tabs."), "{once}");
        assert_eq!(once.matches(RULES_BEGIN).count(), 1, "{once}");

        let twice = with_block(&once);
        assert_eq!(twice, once, "a second write changed the file");
        assert_eq!(twice.matches(RULES_BEGIN).count(), 1, "{twice}");
        assert!(twice.contains("Always use tabs."), "{twice}");
    }

    /// Prose after the block is prose the person wrote, and it stays.
    #[test]
    fn the_rule_block_keeps_what_follows_it() {
        let text = format!("# Mine\n\n{RULES_BEGIN}\nold text\n{RULES_END}\n\nAnd mine again.\n");
        let next = with_block(&text);
        assert!(next.contains("And mine again."), "{next}");
        assert!(!next.contains("old text"), "{next}");
        assert!(next.starts_with("# Mine"), "{next}");
    }
}
