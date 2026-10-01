//! Reading — and, when asked, writing — the configuration files agent clients
//! own.
//!
//! `src/setup.rs:669` states the rule this module bends: a tool that edits a
//! file it does not own is a tool that eventually corrupts one. It holds for
//! every default install. `semlith setup --register-all` is the user overriding
//! it for their own machine, per client, with the paths in front of them, and
//! everything here exists to make that override safe rather than convenient:
//!
//! - every path is listed before any file is written,
//! - every file is copied to `<name>.semlith-backup` before its first write,
//! - a file that does not parse is refused and left exactly as it was,
//! - a merge preserves every key it did not come to change, and
//! - a second run over the same file produces the same bytes as the first.
//!
//! Reading needs none of that and is always allowed. `semlith doctor` and the
//! portal's setup status both ask these files whether a client has semlith
//! registered and at what scope, because asking every client's CLI instead
//! would be a process per client on a route the portal calls on every load.
//!
//! ## Formats
//!
//! Codex's `config.toml` — which the ChatGPT desktop app reads as well — is
//! TOML, and semlith appends its `[mcp_servers.semlith]` table to it when the
//! file has none: a table at the end of a TOML file stands on its own, so
//! nothing already there is rewritten. Every other documented path is JSON,
//! and `serde_json` is already in the tree, so each is merged properly: parsed, the semlith entry set, re-serialized, every
//! sibling key intact. A file that strict JSON cannot parse — Zed allows
//! comments and trailing commas — is refused and left exactly as it was, and
//! its stanza is printed instead. The YAML arm below writes a new file whole
//! and never rewrites an existing one: a hand-rolled YAML rewriter is exactly
//! the thing that eventually corrupts somebody's configuration.

use crate::clients::{Client, Stanza};
use crate::home;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// The suffix a backup carries. Beside the original rather than in a temporary
/// directory, so a user who wants to undo the write can see it next to what it
/// undoes.
pub(crate) const BACKUP: &str = ".semlith-backup";

/// What semlith would do to one file, decided before anything is written.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Plan {
    pub client: String,
    pub path: PathBuf,
    /// Flattened, so the JSON is `"action": "merge"` and, for a refusal,
    /// `"action": "refuse", "reason": "…"`. Without this the tagged enum
    /// nests as `"action": {"action": "merge"}` and the page's comparison
    /// against a string is quietly always false.
    #[serde(flatten)]
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum Action {
    /// The file does not exist; it will be created from the documented stanza.
    Create,
    /// The file exists and parses; the semlith entry will be set in it.
    Merge,
    /// The file already says exactly this. Nothing will be written.
    AlreadyDone,
    /// The file cannot be written safely, and why.
    Refuse { reason: String },
}

impl std::fmt::Display for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match &self.action {
            Action::Create => "create".to_string(),
            Action::Merge => "merge into".to_string(),
            Action::AlreadyDone => "already registered in".to_string(),
            Action::Refuse { reason } => format!("skip ({reason})"),
        };
        write!(f, "{}: {what} {}", self.client, self.path.display())
    }
}

/// Every file semlith would write for these clients, and what it would do to
/// each.
///
/// Read-only. This is what `--register-all` shows the user before it asks, and
/// what the portal's Agents control lists.
pub fn plan(clients: &[&Client]) -> Vec<Plan> {
    let mut out = Vec::new();
    for client in clients {
        for stanza in client.config_files() {
            let Some(path) = resolve(stanza) else {
                continue;
            };
            if !client_present(&path) {
                continue;
            }
            let action = decide(stanza, &path);
            out.push(Plan {
                client: client.name.clone(),
                path,
                action,
            });
        }
    }
    out
}

/// Apply a plan. Returns the paths that changed, which is not every path in the
/// plan: one already holding the entry is left alone and reported as such.
pub fn apply(plans: &[Plan], stanzas: &[(&str, &Stanza)]) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for plan in plans {
        let Some(stanza) = stanzas
            .iter()
            .find(|(client, stanza)| {
                *client == plan.client && resolve(stanza).as_deref() == Some(&plan.path)
            })
            .map(|(_, stanza)| *stanza)
        else {
            continue;
        };
        match &plan.action {
            Action::AlreadyDone => {}
            Action::Refuse { .. } => {}
            Action::Create | Action::Merge => {
                write_one(stanza, &plan.path)?;
                written.push(plan.path.clone());
            }
        }
    }
    Ok(written)
}

/// The clients whose own configuration file names semlith.
///
/// A file read, never a process. Absence of a file and absence of an entry are
/// the same answer here — the client is not registered — because the question
/// the caller asks is whether it would find semlith, not why it would not.
pub fn registered_clients() -> Vec<String> {
    crate::clients::clients()
        .iter()
        .filter(|client| {
            client.config_files().any(|stanza| {
                resolve(stanza)
                    .and_then(|path| std::fs::read_to_string(path).ok())
                    .is_some_and(|text| names_semlith(&text))
            })
        })
        .map(|client| client.name.clone())
        .collect()
}

/// Whether a configuration file has a semlith server in it.
///
/// Deliberately a text test rather than a schema one. These are nineteen
/// different schemas across four products' ideas of where a server list goes,
/// and the question is only whether the name is there; a per-client schema
/// walker would be nineteen things to keep right for an answer this coarse.
/// The name is distinctive enough that a false positive means somebody wrote
/// "semlith" in their own config, which is the answer anyway.
fn names_semlith(text: &str) -> bool {
    text.contains("\"semlith\"")
        || text.contains("semlith:")
        || text.contains("[mcp_servers.semlith")
}

/// The absolute path a `path=` attribute names on this machine, or `None` when
/// the attribute is for another platform.
///
/// `~` is the user's home through `home::user_home`, which is the only function
/// in `src/` that reads the environment for it. `%APPDATA%` is expanded on
/// Windows and means the path is a Windows one, so it resolves to `None`
/// anywhere else.
pub fn resolve(stanza: &Stanza) -> Option<PathBuf> {
    let raw = stanza.path.as_deref()?;
    if !stanza.applies_here() {
        return None;
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        // A client whose whole directory an environment variable moves.
        if let Some(moved) = stanza
            .root
            .as_deref()
            .and_then(std::env::var_os)
            .filter(|v| !v.is_empty())
            && let Some((_, below)) = rest.split_once('/')
        {
            return Some(PathBuf::from(moved).join(below));
        }
        return home::user_home().ok().map(|home| home.join(rest));
    }
    for var in ["APPDATA", "LOCALAPPDATA"] {
        if let Some(rest) = raw.strip_prefix(&format!("%{var}%\\")) {
            if !cfg!(windows) {
                return None;
            }
            return std::env::var_os(var).map(|base| PathBuf::from(base).join(rest));
        }
    }
    Some(PathBuf::from(raw))
}

/// The directory a client makes for itself, above a file of its own: `~/.x`,
/// `~/.config/x`, `~/Documents/x`, `~/Library/Application Support/x`, or
/// `AppData\Roaming\x`. `None` for a path outside the home.
pub fn client_home(path: &Path) -> Option<PathBuf> {
    let home = home::user_home().ok()?;
    let rest = path.strip_prefix(&home).ok()?;
    let parts: Vec<_> = rest.components().collect();
    let depth = match parts.first()?.as_os_str().to_str()? {
        ".config" | "Documents" => 2,
        // A Store (MSIX) app's own directory is one level further down:
        // `AppData\Local\Packages\<app>`.
        "AppData" if parts.get(2).and_then(|p| p.as_os_str().to_str()) == Some("Packages") => 4,
        "Library" | "AppData" => 3,
        _ => 1,
    };
    // The file itself is never its own client's home.
    if parts.len() <= depth {
        return None;
    }
    Some(parts[..depth].iter().fold(home, |at, part| at.join(part)))
}

/// Whether the client that owns `path` is on this machine: its own directory
/// exists. Setup writes nothing, and links nothing, for a client that is not —
/// a skill link used to create a client's directory on a machine that never
/// had the client.
///
/// A path outside the home — one a `root=` variable moved — counts its own
/// directory, since the variable names where the client lives.
pub fn client_present(path: &Path) -> bool {
    match client_home(path) {
        Some(dir) => dir.is_dir(),
        None => path.parent().is_some_and(Path::is_dir),
    }
}

/// What would happen to this file, without touching it.
fn decide(stanza: &Stanza, path: &Path) -> Action {
    let exists = path.exists();
    if !exists {
        return Action::Create;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return Action::Refuse {
            reason: "the file cannot be read".into(),
        };
    };
    match stanza.format.as_str() {
        "json" => match merged_json(stanza, &text) {
            Ok(next) => {
                if next == text {
                    Action::AlreadyDone
                } else {
                    Action::Merge
                }
            }
            Err(e) => Action::Refuse {
                reason: e.to_string(),
            },
        },
        // TOML takes a table appended at the end as it stands, so semlith adds
        // its own and rewrites nothing else. Codex's `config.toml`, which the
        // ChatGPT desktop app reads too, is the one TOML file here.
        "toml" if names_semlith(&text) => Action::AlreadyDone,
        "toml" => Action::Merge,
        // A YAML file that is already there is one semlith will not rewrite;
        // see this module's header for why. The entry is printed instead.
        "yaml" if names_semlith(&text) => Action::AlreadyDone,
        "yaml" => Action::Refuse {
            reason: "semlith does not rewrite an existing YAML configuration; \
                     paste the block printed below"
                .into(),
        },
        other => Action::Refuse {
            reason: format!("semlith does not write {other} configuration files"),
        },
    }
}

fn write_one(stanza: &Stanza, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let existing = std::fs::read_to_string(path).ok();
    let next = match (stanza.format.as_str(), &existing) {
        ("json", Some(text)) => merged_json(stanza, text)?,
        ("json", None) => {
            // Round-tripped rather than copied, so a documented stanza that is
            // not valid JSON fails here rather than in the client.
            let value: serde_json::Value = serde_json::from_str(&stanza.text)
                .with_context(|| "the documented stanza is not valid JSON".to_string())?;
            serialize(&value)
        }
        ("yaml", None) => stanza.text.clone(),
        ("toml", None) => format!("{}\n", stanza.text.trim_end()),
        ("toml", Some(text)) => {
            let separator = match text.as_str() {
                "" => "",
                t if t.ends_with("\n\n") => "",
                t if t.ends_with('\n') => "\n",
                _ => "\n\n",
            };
            format!("{text}{separator}{}\n", stanza.text.trim_end())
        }
        (format, _) => bail!("semlith does not write {format} configuration files"),
    };

    if existing.as_deref() == Some(next.as_str()) {
        return Ok(());
    }
    if existing.is_some() {
        back_up(path)?;
    }

    // Through a neighbouring temporary file. A half-written `settings.json` is
    // a client that will not start, and this runs across a dozen of them.
    let temp = path.with_extension(format!(
        "{}.semlith-tmp",
        path.extension().and_then(|e| e.to_str()).unwrap_or("")
    ));
    std::fs::write(&temp, &next).with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// Take semlith's entry out of every user-level file of this client that
/// holds one (0.35.0's per-client Unregister). Returns the files changed.
///
/// The same care as a write: the file is backed up beside itself first, a
/// file that does not parse is refused and left as it was, every other key
/// stays, and the replacement goes through a neighbouring temporary file.
/// YAML is refused, for the reason this module never rewrites it.
pub fn unregister_files(client: &Client) -> Result<Vec<PathBuf>> {
    let mut changed = Vec::new();
    for stanza in client.config_files() {
        let Some(path) = resolve(stanza) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !names_semlith(&text) {
            continue;
        }
        let next = match stanza.format.as_str() {
            "json" => {
                let mut base: serde_json::Value =
                    serde_json::from_str(&text).with_context(|| {
                        format!("{} is not valid JSON; nothing was changed", path.display())
                    })?;
                let overlay: serde_json::Value = serde_json::from_str(&stanza.text)
                    .context("the documented stanza is not valid JSON")?;
                remove_entry(&mut base, &overlay);
                serialize(&base)
            }
            "toml" => without_toml_table(&text),
            other => bail!(
                "semlith does not rewrite {other} configuration; remove its entry from {} by hand",
                path.display()
            ),
        };
        if next == text {
            continue;
        }
        back_up(&path)?;
        let temp = path.with_extension(format!(
            "{}.semlith-tmp",
            path.extension().and_then(|e| e.to_str()).unwrap_or("")
        ));
        std::fs::write(&temp, &next).with_context(|| format!("writing {}", temp.display()))?;
        std::fs::rename(&temp, &path).with_context(|| format!("replacing {}", path.display()))?;
        changed.push(path);
    }
    Ok(changed)
}

/// Remove from `base` the `semlith` key the documented stanza puts there, at
/// the same path (`mcpServers.semlith`, `mcp.semlith`, …), and nothing else.
fn remove_entry(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    let (serde_json::Value::Object(base), serde_json::Value::Object(overlay)) = (base, overlay)
    else {
        return;
    };
    for (key, value) in overlay {
        if key == "semlith" {
            base.remove(key);
        } else if let Some(inner) = base.get_mut(key) {
            remove_entry(inner, value);
        }
    }
}

/// `text` without its `[mcp_servers.semlith…]` tables, every other byte kept.
fn without_toml_table(text: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            skipping = trimmed.starts_with("[mcp_servers.semlith");
        }
        if !skipping {
            out.push_str(line);
        }
    }
    out
}

/// Copy a file to `<name>.semlith-backup` before its first write.
///
/// Once. A second `--register-all` would otherwise overwrite the backup with
/// the already-registered file, which is the one state a backup is useless in.
pub fn back_up(path: &Path) -> Result<()> {
    let mut backup = path.as_os_str().to_os_string();
    backup.push(BACKUP);
    let backup = PathBuf::from(backup);
    if backup.exists() {
        return Ok(());
    }
    std::fs::copy(path, &backup).with_context(|| format!("backing up to {}", backup.display()))?;
    Ok(())
}

/// `existing` with the documented stanza's keys set into it.
///
/// A recursive merge of objects rather than a replace: the stanza names the
/// path to the semlith entry — `mcpServers.semlith`, `mcp.semlith`,
/// `context_servers.semlith` — and everything alongside it at every level is
/// kept. That is what makes the write idempotent and what keeps somebody's
/// other twelve servers.
fn merged_json(stanza: &Stanza, existing: &str) -> Result<String> {
    let mut base: serde_json::Value = if existing.trim().is_empty() {
        serde_json::Value::Object(Default::default())
    } else {
        serde_json::from_str(existing).context("the file is not valid JSON")?
    };
    let overlay: serde_json::Value =
        serde_json::from_str(&stanza.text).context("the documented stanza is not valid JSON")?;
    merge(&mut base, &overlay);
    Ok(serialize(&base))
}

fn merge(base: &mut serde_json::Value, overlay: &serde_json::Value) {
    match (base, overlay) {
        (serde_json::Value::Object(base), serde_json::Value::Object(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(key) {
                    Some(existing) => merge(existing, value),
                    None => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, overlay) => *base = overlay.clone(),
    }
}

/// Two-space JSON with a trailing newline — what every one of these files
/// already looks like, and what makes a second run byte-identical to the first.
fn serialize(value: &serde_json::Value) -> String {
    let mut out = Vec::new();
    let indent = b"  ";
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent);
    let mut ser = serde_json::Serializer::with_formatter(&mut out, formatter);
    serde::Serialize::serialize(value, &mut ser).expect("a parsed value re-serializes");
    let mut text = String::from_utf8(out).expect("serde_json emits UTF-8");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::Scope;

    /// Unregistering takes semlith's table out and keeps every other byte.
    #[test]
    fn a_toml_unregister_removes_only_semlith() {
        let text = "model = \"x\"\n\n[mcp_servers.other]\ncommand = \"o\"\n\n\
                    [mcp_servers.semlith]\ncommand = \"semlith\"\nargs = [\"mcp\"]\n\n\
                    [mcp_servers.semlith.env]\nA = \"1\"\n\n[profile]\nname = \"p\"\n";
        let out = without_toml_table(text);
        assert!(!out.contains("semlith"), "{out}");
        assert!(out.contains("[mcp_servers.other]") && out.contains("[profile]"));
        assert!(out.starts_with("model = \"x\"\n"));
    }

    #[test]
    fn a_json_unregister_removes_only_the_documented_key() {
        let mut base: serde_json::Value = serde_json::from_str(
            r#"{"mcpServers": {"semlith": {"command": "semlith"}, "other": {"command": "o"}}, "theme": "dark"}"#,
        )
        .unwrap();
        let overlay = serde_json::json!({ "mcpServers": { "semlith": { "command": "semlith" } } });
        remove_entry(&mut base, &overlay);
        assert_eq!(
            base,
            serde_json::json!({ "mcpServers": { "other": { "command": "o" } }, "theme": "dark" })
        );
    }

    #[test]
    fn a_toml_file_gains_the_table_once_and_keeps_every_other_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let toml = stanza(
            "toml",
            "[mcp_servers.semlith]\ncommand = \"/b/semlith\"\nargs = [\"mcp\"]",
            "~/.codex/config.toml",
        );
        let theirs = "model = \"x\"\n\n[mcp_servers.other]\ncommand = \"o\"\n";
        std::fs::write(&path, theirs).unwrap();
        assert_eq!(decide(&toml, &path), Action::Merge);
        write_one(&toml, &path).unwrap();
        let once = std::fs::read_to_string(&path).unwrap();
        assert!(once.starts_with(theirs), "{once}");
        assert!(
            once.ends_with("[mcp_servers.semlith]\ncommand = \"/b/semlith\"\nargs = [\"mcp\"]\n"),
            "{once}"
        );
        assert_eq!(decide(&toml, &path), Action::AlreadyDone);
        std::fs::remove_file(&path).unwrap();
        write_one(&toml, &path).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .starts_with("[mcp_servers.semlith]")
        );
    }

    /// Windows: every client registered by a file gets that system's file —
    /// Zed and Claude Desktop under `%APPDATA%`, the rest under the home —
    /// and one not installed gets nothing.
    #[cfg(windows)]
    #[test]
    fn register_all_writes_each_clients_windows_file() {
        let dir = tempfile::tempdir().unwrap();
        let (home, appdata, local) = (
            dir.path().join("home"),
            dir.path().join("home").join("AppData").join("Roaming"),
            dir.path().join("home").join("AppData").join("Local"),
        );
        for made in [
            appdata.join("Zed"),
            appdata.join("Claude"),
            home.join(".cursor"),
            home.join(".cline").join("data").join("settings"),
            home.join(".config").join("opencode"),
        ] {
            std::fs::create_dir_all(made).unwrap();
        }
        std::fs::create_dir_all(&local).unwrap();
        let vars = [
            ("HOME", home.as_os_str()),
            ("USERPROFILE", home.as_os_str()),
            ("APPDATA", appdata.as_os_str()),
            ("LOCALAPPDATA", local.as_os_str()),
        ];
        let written = crate::home::with_env_vars(&vars, || {
            let file_clients: Vec<&Client> = crate::clients::clients()
                .iter()
                .filter(|c| {
                    ["Zed", "Claude Desktop", "Cursor", "Cline", "OpenCode"]
                        .contains(&c.name.as_str())
                })
                .collect();
            let plans = plan(&file_clients);
            let stanzas: Vec<(&str, &Stanza)> = file_clients
                .iter()
                .flat_map(|c| c.config_files().map(move |s| (c.name.as_str(), s)))
                .collect();
            apply(&plans, &stanzas).unwrap()
        });
        for expected in [
            appdata.join("Zed").join("settings.json"),
            appdata.join("Claude").join("claude_desktop_config.json"),
            home.join(".cursor").join("mcp.json"),
            home.join(".cline")
                .join("data")
                .join("settings")
                .join("cline_mcp_settings.json"),
            home.join(".config").join("opencode").join("opencode.json"),
        ] {
            assert!(
                written.contains(&expected),
                "{} not written: {written:?}",
                expected.display()
            );
        }
        // The Store build of Claude Desktop is not installed here.
        assert!(!local.join("Packages").exists());
    }

    #[test]
    fn an_os_list_applies_on_each_system_it_names() {
        let mut both = stanza("json", "{}", "~/.config/zed/settings.json");
        both.os = Some("macos,linux".into());
        assert_eq!(both.applies_here(), !cfg!(windows));
        both.os = Some("windows".into());
        assert_eq!(both.applies_here(), cfg!(windows));
    }

    fn stanza(format: &str, text: &str, path: &str) -> Stanza {
        Stanza {
            format: format.to_string(),
            text: text.to_string(),
            register: false,
            unregister: false,
            hook: false,
            skills: false,
            rules: false,
            path: Some(path.to_string()),
            os: None,
            root: None,
            scope: Scope::Global,
        }
    }

    const ENTRY: &str = r#"{"mcpServers":{"semlith":{"command":"semlith","args":["mcp"]}}}"#;

    #[test]
    fn a_merge_keeps_every_key_it_did_not_come_to_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(
            &path,
            r#"{"theme":"dark","mcpServers":{"other":{"command":"other"}}}"#,
        )
        .unwrap();

        let s = stanza("json", ENTRY, "unused");
        write_one(&s, &path).unwrap();

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after["theme"], "dark", "an unrelated key was lost");
        assert_eq!(
            after["mcpServers"]["other"]["command"], "other",
            "another server was lost"
        );
        assert_eq!(after["mcpServers"]["semlith"]["command"], "semlith");
        assert_eq!(after["mcpServers"]["semlith"]["args"][0], "mcp");
    }

    #[test]
    fn a_second_write_is_byte_identical_and_leaves_one_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        std::fs::write(&path, r#"{"mcpServers":{"other":{"command":"other"}}}"#).unwrap();

        let s = stanza("json", ENTRY, "unused");
        write_one(&s, &path).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        let backup = std::fs::read_to_string(format!("{}{BACKUP}", path.display())).unwrap();

        write_one(&s, &path).unwrap();
        let second = std::fs::read_to_string(&path).unwrap();
        assert_eq!(first, second, "a second write changed the file");
        assert_eq!(
            backup,
            std::fs::read_to_string(format!("{}{BACKUP}", path.display())).unwrap(),
            "the second write overwrote the backup with the already-registered file"
        );
        assert_eq!(decide(&s, &path), Action::AlreadyDone);
    }

    #[test]
    fn a_file_already_holding_the_entry_is_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        let s = stanza("json", ENTRY, "unused");
        write_one(&s, &path).unwrap();
        assert_eq!(decide(&s, &path), Action::AlreadyDone);
        assert!(
            !PathBuf::from(format!("{}{BACKUP}", path.display())).exists(),
            "a created file was backed up, which there was nothing to back up"
        );
    }

    #[test]
    fn a_malformed_file_is_refused_and_left_exactly_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json");
        let before = "{ this is not json";
        std::fs::write(&path, before).unwrap();

        let s = stanza("json", ENTRY, "unused");
        match decide(&s, &path) {
            Action::Refuse { reason } => assert!(
                reason.contains("not valid JSON"),
                "the refusal does not say why: {reason}"
            ),
            other => panic!("a malformed file was not refused: {other:?}"),
        }
        assert!(write_one(&s, &path).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "a file that could not be parsed was written anyway"
        );
    }

    #[test]
    fn an_existing_yaml_configuration_is_refused_rather_than_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        let before = "extensions:\n  other:\n    enabled: true\n";
        std::fs::write(&path, before).unwrap();

        let s = stanza(
            "yaml",
            "extensions:\n  semlith:\n    cmd: semlith\n",
            "unused",
        );
        assert!(matches!(decide(&s, &path), Action::Refuse { .. }));
        assert!(write_one(&s, &path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn a_yaml_file_that_is_not_there_yet_is_written_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("semlith.yaml");
        let body = "name: semlith\ncommand: semlith\nargs:\n  - mcp\n";
        let s = stanza("yaml", body, "unused");
        assert_eq!(decide(&s, &path), Action::Create);
        write_one(&s, &path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
    }

    /// A path for another platform resolves nowhere, so `--register-all` on
    /// Linux does not plan a write into a Windows `%APPDATA%` path that would
    /// land in the working directory.
    #[test]
    fn a_path_for_another_platform_resolves_to_nothing() {
        let mut s = stanza("json", ENTRY, "%APPDATA%\\Claude\\config.json");
        s.os = Some("windows".into());
        if !cfg!(windows) {
            assert_eq!(resolve(&s), None);
        }
        let mut s = stanza("json", ENTRY, "~/Library/Application Support/x.json");
        s.os = Some("macos".into());
        if !cfg!(target_os = "macos") {
            assert_eq!(resolve(&s), None);
        }
    }
}
