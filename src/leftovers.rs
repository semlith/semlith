//! A one-time removal of what earlier releases wrote into clients semlith no
//! longer supports.
//!
//! Earlier releases registered semlith in clients it no longer supports.
//! `docs/clients.md` is the registry every other path in `setup` comes from,
//! and it names only the supported ones, so these paths live here, in one
//! table, and nowhere else.
//!
//! Only what semlith wrote is removed: the `semlith` server entry (and only
//! when it launches or points at semlith), the rule block between semlith's
//! markers, and a skill link into semlith's own skill directory. Every other
//! byte of a file is kept exactly — which is why a JSON entry is cut out of the
//! text rather than the file being re-serialized, which would reformat
//! somebody's own settings. Each edited file is backed up beside itself first,
//! a file that does not parse is left alone and named, and a second run finds
//! nothing.

use crate::clientfile;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// What semlith may have left at one path.
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// A JSON file whose object at this key path holds a `semlith` entry.
    Json(&'static [&'static str]),
    /// Goose's `config.yaml`: a `semlith` entry under `extensions`.
    GooseYaml,
    /// A file that is semlith's own server definition and nothing else.
    WholeFile,
    /// A rules file holding semlith's marked block.
    Rules,
    /// A link, or on Windows a copy, of semlith's skill.
    Skill,
}

/// Every file an earlier `semlith setup` wrote for a client it no longer lists.
const LEFTOVERS: &[(&str, &str, Kind)] = &[
    (
        "Qwen Code",
        "~/.qwen/settings.json",
        Kind::Json(&["mcpServers"]),
    ),
    ("Qwen Code", "~/.qwen/QWEN.md", Kind::Rules),
    ("Qwen Code", "~/.qwen/skills/semlith", Kind::Skill),
    (
        "Amp",
        "~/.config/amp/settings.json",
        Kind::Json(&["amp.mcpServers"]),
    ),
    ("Amp", "~/.config/amp/AGENTS.md", Kind::Rules),
    ("Crush", "~/.config/crush/CRUSH.md", Kind::Rules),
    ("Droid", "~/.factory/mcp.json", Kind::Json(&["mcpServers"])),
    ("Droid", "~/.factory/AGENTS.md", Kind::Rules),
    ("Goose", "~/.config/goose/config.yaml", Kind::GooseYaml),
    ("Goose", "~/.config/goose/.goosehints", Kind::Rules),
    (
        "Amazon Q Developer CLI",
        "~/.aws/amazonq/mcp.json",
        Kind::Json(&["mcpServers"]),
    ),
    (
        "OpenClaw",
        "~/.openclaw/openclaw.json",
        Kind::Json(&["mcp", "servers"]),
    ),
    ("OpenClaw", "~/.openclaw/workspace/AGENTS.md", Kind::Rules),
    (
        "DeepSeek-TUI",
        "~/.deepseek/mcp.json",
        Kind::Json(&["mcpServers"]),
    ),
    (
        "Codewhale",
        "~/.codewhale/mcp.json",
        Kind::Json(&["mcpServers"]),
    ),
    ("Warp", "~/.warp/.mcp.json", Kind::Json(&["mcpServers"])),
    (
        "Windsurf",
        "~/.codeium/windsurf/mcp_config.json",
        Kind::Json(&["mcpServers"]),
    ),
    (
        "Windsurf",
        "~/.codeium/windsurf/memories/global_rules.md",
        Kind::Rules,
    ),
    (
        "Junie",
        "~/.junie/mcp/mcp.json",
        Kind::Json(&["mcpServers"]),
    ),
    ("Junie", "~/.junie/AGENTS.md", Kind::Rules),
    ("Roo Code", "~/.roo/rules/semlith.md", Kind::Rules),
    (
        "Kilo Code",
        "~/.config/kilo/kilo.json",
        Kind::Json(&["mcp"]),
    ),
    (
        "Continue",
        "~/.continue/mcpServers/semlith.yaml",
        Kind::WholeFile,
    ),
    (
        "Kiro",
        "~/.kiro/settings/mcp.json",
        Kind::Json(&["mcpServers"]),
    ),
    ("Kiro", "~/.kiro/steering/semlith.md", Kind::Rules),
    ("Kiro", "~/.kiro/skills/semlith", Kind::Skill),
    (
        "LM Studio",
        "~/.lmstudio/mcp.json",
        Kind::Json(&["mcpServers"]),
    ),
];

/// One thing this run did, or declined to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub client: &'static str,
    pub path: PathBuf,
    /// `None` when it was removed; the reason when it was left alone.
    pub left: Option<String>,
    pub what: &'static str,
}

impl std::fmt::Display for Removal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.left {
            None => write!(
                f,
                "{}: removed {} from {}",
                self.client,
                self.what,
                self.path.display()
            ),
            Some(why) => write!(
                f,
                "{}: left {} alone — {why}",
                self.client,
                self.path.display()
            ),
        }
    }
}

/// Remove semlith's own entries from every unsupported client on this machine.
///
/// Returns one line per file touched or declined; nothing when there was
/// nothing to do, which is every run after the first.
pub fn clean() -> Vec<Removal> {
    let Ok(home) = crate::home::user_home() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for &(client, raw, kind) in LEFTOVERS {
        let path = home.join(raw.trim_start_matches("~/"));
        let outcome = match kind {
            Kind::Skill => skill(&path),
            _ => file(&path, kind),
        };
        match outcome {
            Ok(Some(what)) => out.push(Removal {
                client,
                path,
                left: None,
                what,
            }),
            Ok(None) => {}
            Err(why) => out.push(Removal {
                client,
                path,
                left: Some(format!("{why:#}")),
                what: "",
            }),
        }
    }
    out
}

/// One file: what was removed from it, if anything.
fn file(path: &Path, kind: Kind) -> Result<Option<&'static str>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    let (next, what) = match kind {
        Kind::Json(at) => (without_json_entry(&text, at)?, "the semlith server"),
        Kind::GooseYaml => (without_goose_entry(&text), "the semlith extension"),
        Kind::WholeFile => (
            (text.contains("name: semlith")
                && text
                    .lines()
                    .filter(|l| l.contains("command:") || l.contains("url:"))
                    .any(points_at_semlith))
            .then(String::new),
            "semlith's server file",
        ),
        Kind::Rules => (without_rules(&text), "the semlith rule block"),
        Kind::Skill => unreachable!("links are not files"),
    };
    let Some(next) = next else {
        return Ok(None);
    };
    clientfile::back_up(path)?;
    if next.trim().is_empty() {
        std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    } else {
        std::fs::write(path, next).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(Some(what))
}

/// A skill link semlith made, removed; anything else at that path is not ours.
fn skill(at: &Path) -> Result<Option<&'static str>> {
    if at.is_symlink() {
        let target = std::fs::read_link(at)?;
        let ours = crate::agentfiles::canonical_skill_dir().is_ok_and(|c| c == target);
        if !ours {
            return Ok(None);
        }
        std::fs::remove_file(at).with_context(|| format!("removing {}", at.display()))?;
        return Ok(Some("the semlith skill link"));
    }
    if at.is_dir() && crate::agentfiles::ours_alone(at) {
        std::fs::remove_dir_all(at).with_context(|| format!("removing {}", at.display()))?;
        return Ok(Some("the semlith skill"));
    }
    Ok(None)
}

/// Whether an entry's body — never its name — launches or points at semlith:
/// the binary's path, or the daemon's endpoint. A server somebody else named
/// `semlith` is not ours to remove.
fn points_at_semlith(body: &str) -> bool {
    body.contains("semlith") || body.contains(":7365/mcp")
}

/// `text` without the `semlith` member of the object at `at`, byte for byte
/// otherwise. `None` when there is no such entry, or it is not semlith's.
fn without_json_entry(text: &str, at: &[&str]) -> Result<Option<String>> {
    let value: serde_json::Value =
        serde_json::from_str(text).context("not strict JSON, so semlith will not edit it")?;
    let mut container = &value;
    for key in at {
        match container.get(*key) {
            Some(next) => container = next,
            None => return Ok(None),
        }
    }
    let Some(entry) = container.get("semlith") else {
        return Ok(None);
    };
    if !points_at_semlith(&entry.to_string()) {
        return Ok(None);
    }
    let bytes = text.as_bytes();
    let mut object = skip_ws(bytes, 0);
    for key in at {
        let member = members(bytes, object)?
            .into_iter()
            .find(|m| m.key == *key)
            .context("the file changed while it was read")?;
        object = member.value;
    }
    let all = members(bytes, object)?;
    let Some(i) = all.iter().position(|m| m.key == "semlith") else {
        return Ok(None);
    };
    let (cut_from, cut_to) = if all.len() == 1 {
        // The only member: the object becomes empty, `{}`.
        (object + 1, all[0].end_of_ws_before_close)
    } else if i + 1 < all.len() {
        // Up to the next member, taking the comma and the gap with it.
        (all[i].start, all[i + 1].start)
    } else {
        // The last of several: from the end of the one before.
        (all[i - 1].end, all[i].end)
    };
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..cut_from]);
    out.push_str(&text[cut_to..]);
    // What is left must still be the same document, less one entry.
    serde_json::from_str::<serde_json::Value>(&out).context("removing the entry broke the file")?;
    Ok(Some(out))
}

struct Member {
    key: String,
    /// The key's opening quote.
    start: usize,
    /// Where the member's value begins.
    value: usize,
    /// Just past the member's value.
    end: usize,
    /// Just past the whitespace after the value, when this is the last member:
    /// where the object's `}` is.
    end_of_ws_before_close: usize,
}

/// The members of the object whose `{` is at `open`.
fn members(b: &[u8], open: usize) -> Result<Vec<Member>> {
    anyhow::ensure!(b.get(open) == Some(&b'{'), "expected an object");
    let mut out = Vec::new();
    let mut i = skip_ws(b, open + 1);
    if b.get(i) == Some(&b'}') {
        return Ok(out);
    }
    loop {
        let start = i;
        let key_end = scan_string(b, i)?;
        let key: String = serde_json::from_slice(&b[i..key_end])?;
        i = skip_ws(b, key_end);
        anyhow::ensure!(b.get(i) == Some(&b':'), "expected ':'");
        let value = skip_ws(b, i + 1);
        let end = scan_value(b, value)?;
        let after = skip_ws(b, end);
        out.push(Member {
            key,
            start,
            value,
            end,
            end_of_ws_before_close: after,
        });
        match b.get(after) {
            Some(b',') => i = skip_ws(b, after + 1),
            Some(b'}') => return Ok(out),
            _ => anyhow::bail!("expected ',' or '}}'"),
        }
    }
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Just past the string whose opening quote is at `i`.
fn scan_string(b: &[u8], i: usize) -> Result<usize> {
    anyhow::ensure!(b.get(i) == Some(&b'"'), "expected a string");
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return Ok(j + 1),
            _ => j += 1,
        }
    }
    anyhow::bail!("an unterminated string")
}

/// Just past the value starting at `i`.
fn scan_value(b: &[u8], i: usize) -> Result<usize> {
    match b.get(i) {
        Some(b'"') => scan_string(b, i),
        Some(b'{') | Some(b'[') => {
            let mut depth = 0usize;
            let mut j = i;
            while j < b.len() {
                match b[j] {
                    b'"' => {
                        j = scan_string(b, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            anyhow::bail!("an unterminated value")
        }
        Some(_) => {
            let mut j = i;
            while j < b.len() && !matches!(b[j], b',' | b'}' | b']') && !b[j].is_ascii_whitespace()
            {
                j += 1;
            }
            Ok(j)
        }
        None => anyhow::bail!("expected a value"),
    }
}

/// Goose's `config.yaml` without a `semlith:` block directly under
/// `extensions:`. Lines only; the rest of the file is not parsed or rewritten.
fn without_goose_entry(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let indent = |l: &str| l.len() - l.trim_start_matches(' ').len();
    let top = lines.iter().position(|l| l.trim_end() == "extensions:")?;
    let mut i = top + 1;
    let mut child = None;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        let n = indent(line);
        if n == 0 {
            return None;
        }
        let level = *child.get_or_insert(n);
        if n == level && line.trim_end().trim_start() == "semlith:" {
            let mut end = i + 1;
            let mut last_body = i + 1;
            while end < lines.len() {
                let l = lines[end];
                if l.trim().is_empty() {
                    end += 1;
                    continue;
                }
                if indent(l) <= level {
                    break;
                }
                end += 1;
                last_body = end;
            }
            let body: String = lines[i + 1..last_body].concat();
            if !points_at_semlith(&body) {
                return None;
            }
            let mut out: String = lines[..i].concat();
            out.push_str(&lines[last_body..].concat());
            // `extensions:` with nothing under it is null, which Goose reads as
            // an error; a file that held only ours goes entirely.
            let rest_has_children = lines[top + 1..i]
                .iter()
                .chain(&lines[last_body..])
                .take_while(|l| l.trim().is_empty() || indent(l) > 0)
                .any(|l| !l.trim().is_empty());
            if !rest_has_children {
                if out.trim() == "extensions:" {
                    return Some(String::new());
                }
                out = out.replacen("extensions:\n", "extensions: {}\n", 1);
            }
            return Some(out);
        }
        i += 1;
    }
    None
}

/// `text` without semlith's marked rule block, as `agentfiles::with_block`
/// wrote it: the markers through the newline after the closing one.
fn without_rules(text: &str) -> Option<String> {
    let (begin, end) = crate::agentfiles::RULES_MARKERS;
    let start = text.find(begin)?;
    let stop = text[start..].find(end)? + start + end.len();
    let stop = if text[stop..].starts_with('\n') {
        stop + 1
    } else {
        stop
    };
    Some(format!("{}{}", &text[..start], &text[stop..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_entry_goes_and_every_other_byte_stays() {
        let text = "{\n    \"theme\": \"dark\",\n    \"mcpServers\": {\n        \"other\": { \"command\": \"x\" },\n        \"semlith\": { \"command\": \"/Users/me/.semlith/bin/semlith\", \"args\": [\"mcp\"] }\n    }\n}\n";
        let out = without_json_entry(text, &["mcpServers"]).unwrap().unwrap();
        assert_eq!(
            out,
            "{\n    \"theme\": \"dark\",\n    \"mcpServers\": {\n        \"other\": { \"command\": \"x\" }\n    }\n}\n"
        );
        // First of several, and the only one.
        let first = "{\"mcpServers\": {\"semlith\": {\"command\": \"/x/semlith\"}, \"b\": {}}}";
        assert_eq!(
            without_json_entry(first, &["mcpServers"]).unwrap().unwrap(),
            "{\"mcpServers\": {\"b\": {}}}"
        );
        let only = "{\"mcp\": {\"servers\": {\n  \"semlith\": {\"url\": \"http://127.0.0.1:7365/mcp\"}\n}}}";
        assert_eq!(
            without_json_entry(only, &["mcp", "servers"])
                .unwrap()
                .unwrap(),
            "{\"mcp\": {\"servers\": {}}}"
        );
    }

    #[test]
    fn an_entry_that_is_not_semliths_or_a_file_that_is_not_json_is_left() {
        let theirs =
            "{\"mcpServers\": {\"semlith\": {\"command\": \"npx\", \"args\": [\"other\"]}}}";
        assert!(
            without_json_entry(theirs, &["mcpServers"])
                .unwrap()
                .is_none()
        );
        assert!(without_json_entry("{ // comment\n}", &["mcpServers"]).is_err());
        assert!(
            without_json_entry("{\"mcpServers\": {}}", &["mcpServers"])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_goose_extension_goes_and_its_siblings_stay() {
        let text = "GOOSE_MODEL: x\nextensions:\n  developer:\n    enabled: true\n  semlith:\n    type: stdio\n    cmd: \"/home/me/.semlith/bin/semlith\"\n    args:\n      - mcp\n    timeout: 300\nother: 1\n";
        assert_eq!(
            without_goose_entry(text).unwrap(),
            "GOOSE_MODEL: x\nextensions:\n  developer:\n    enabled: true\nother: 1\n"
        );
        let alone = "extensions:\n  semlith:\n    cmd: \"/x/semlith\"\n    args:\n      - mcp\n";
        assert_eq!(without_goose_entry(alone).unwrap(), "");
        assert!(without_goose_entry("extensions:\n  developer:\n    enabled: true\n").is_none());
    }

    #[test]
    fn a_rule_block_goes_and_the_prose_around_it_stays() {
        let (begin, end) = crate::agentfiles::RULES_MARKERS;
        let text = format!("# Mine\n\nKeep this.\n{begin}\nuse semlith\n{end}\n");
        assert_eq!(without_rules(&text).unwrap(), "# Mine\n\nKeep this.\n");
        let middle = format!("a\n{begin}\nx\n{end}\nb\n");
        assert_eq!(without_rules(&middle).unwrap(), "a\nb\n");
        assert!(without_rules("nothing of ours").is_none());
    }

    /// The acceptance run in miniature: every dropped client seeded with a
    /// semlith entry beside a foreign one, cleaned, and cleaned again.
    #[test]
    fn a_seeded_home_is_cleaned_once_and_the_second_run_finds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        let (begin, end) = crate::agentfiles::RULES_MARKERS;
        let mut expected = Vec::new();
        for &(_, raw, kind) in LEFTOVERS {
            let path = home.join(raw.trim_start_matches("~/"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            match kind {
                Kind::Json(at) => {
                    let mut inner = "{\"keep\": {\"command\": \"other\"}, \"semlith\": {\"command\": \"/b/semlith\", \"args\": [\"mcp\"]}}".to_string();
                    for key in at.iter().rev() {
                        inner = format!("{{\"{key}\": {inner}}}");
                    }
                    let text = inner.replacen('{', "{\"foreign\": 1, ", 1);
                    std::fs::write(&path, &text).unwrap();
                    expected.push((path, Some("keep")));
                }
                Kind::GooseYaml => {
                    std::fs::write(&path, "x: 1\nextensions:\n  keep:\n    enabled: true\n  semlith:\n    cmd: \"/b/semlith\"\n    args:\n      - mcp\n").unwrap();
                    expected.push((path, Some("keep:")));
                }
                Kind::WholeFile => {
                    std::fs::write(&path, "name: Semlith\nmcpServers:\n  - name: semlith\n    command: \"/b/semlith\"\n    args:\n      - mcp\n").unwrap();
                    expected.push((path, None));
                }
                Kind::Rules => {
                    std::fs::write(&path, format!("My rules.\n{begin}\nours\n{end}\n")).unwrap();
                    expected.push((path, Some("My rules.")));
                }
                Kind::Skill => {}
            }
        }
        let first = crate::home::with_env_var(crate::home::HOME_VAR, home.as_os_str(), clean);
        assert!(first.iter().all(|r| r.left.is_none()), "{first:?}");
        for (path, kept) in &expected {
            let backup = PathBuf::from(format!("{}{}", path.display(), clientfile::BACKUP));
            assert!(backup.exists(), "no backup beside {}", path.display());
            match kept {
                Some(kept) => {
                    let text = std::fs::read_to_string(path).unwrap();
                    assert!(text.contains(kept), "{}: {text}", path.display());
                    assert!(
                        !text.contains("/b/semlith") && !text.contains(begin),
                        "{text}"
                    );
                }
                None => assert!(!path.exists(), "{} survived", path.display()),
            }
        }
        let second = crate::home::with_env_var(crate::home::HOME_VAR, home.as_os_str(), clean);
        assert!(second.is_empty(), "{second:?}");
    }
}
