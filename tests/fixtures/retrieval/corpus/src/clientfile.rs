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
//! registered and at what scope, because asking sixteen client CLIs instead
//! would be sixteen processes on a route the portal calls on every load.
//!
//! ## Formats
//!
//! Nineteen of the twenty-one documented paths are JSON, and `serde_json` is
//! already in the tree, so those are merged properly: parsed, the semlith entry
//! set, re-serialized, every sibling key intact.
//!
//! The two that are not are YAML — Goose's `config.yaml` and Continue's
//! per-server `semlith.yaml` — and semlith adds no YAML dependency for them.
//! Continue needs none: its path is one file per server, so there is nothing to
//! merge into and the file is written whole. Goose's is a shared file, so
//! semlith writes it only when it does not exist yet; where one is already
//! there, the block and the path are printed and the file is not touched. A
//! hand-rolled YAML rewriter is exactly the thing that eventually corrupts
//! somebody's configuration, and this release is not the place to find that out.

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
    match stanza.os.as_deref() {
        Some("windows") if !cfg!(windows) => return None,
        Some("macos") if !cfg!(target_os = "macos") => return None,
        Some("linux") if !cfg!(target_os = "linux") => return None,
        _ => {}
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home::user_home().ok().map(|home| home.join(rest));
    }
    if let Some(rest) = raw.strip_prefix("%APPDATA%\\") {
        if !cfg!(windows) {
            return None;
        }
        return std::env::var_os("APPDATA").map(|base| PathBuf::from(base).join(rest));
    }
    Some(PathBuf::from(raw))
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

/// Copy a file to `<name>.semlith-backup` before its first write.
///
/// Once. A second `--register-all` would otherwise overwrite the backup with
/// the already-registered file, which is the one state a backup is useless in.
fn back_up(path: &Path) -> Result<()> {
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
/// path to the semlith entry — `mcpServers.semlith`, `mcp.servers.semlith`,
/// `amp.mcpServers.semlith` — and everything alongside it at every level is
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

    fn stanza(format: &str, text: &str, path: &str) -> Stanza {
        Stanza {
            format: format.to_string(),
            text: text.to_string(),
            register: false,
            unregister: false,
            path: Some(path.to_string()),
            os: None,
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
