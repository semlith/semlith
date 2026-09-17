//! What this machine's semlith install looks like, and what can be repaired.
//!
//! Two things live here, and they live together on purpose. The Privacy page
//! reads the machine and says which of its rules hold; `semlith doctor` reads
//! the machine and says whether the agent clients on it can reach semlith at
//! all. Both used to be reports with no write path, so a user who saw a failing
//! row was told what was wrong and left to work out the command themselves.
//!
//! From 0.18.0 a failing rule carries the command that repairs it, and where
//! the daemon can apply that command safely it can apply it. Safely is a
//! property of the repair rather than a judgement made per button:
//!
//! - it **narrows** access and never widens it,
//! - it is **idempotent**, so a second application changes nothing,
//! - it touches only a path semlith owns — the store home, a registered store,
//!   the model cache, `daemon.json`, the agent key — and
//! - it is confirmed by **re-running the rule's own check**, so a row turns
//!   green because the daemon looked again, not because a click succeeded.
//!
//! A rule that fails for a reason no process can repair says so and offers
//! nothing. `private addresses` is the case that fixes the boundary in place:
//! it fails because `SEMLITH_ADD_ALLOW_PRIVATE` is set in the environment the
//! daemon inherited, and no process can unset a variable in its parent's
//! environment. A button there could only apologise after the click.
//!
//! One engine, two surfaces, per the portal-parity rule: what the Privacy
//! page's button calls is what `semlith doctor --fix` calls.

use crate::home;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// A repair the daemon may apply to a path it owns.
///
/// One variant so far. Both repairs that qualify today are the same operation —
/// remove the group and other bits from something semlith wrote — and a second
/// variant added before there is a second kind of repair would be an
/// abstraction with one implementation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Repair {
    /// Set `path` to `to`, which must be narrower than what is there.
    Chmod { path: PathBuf, to: u32 },
}

impl Repair {
    /// The command a user would type instead of pressing the button.
    ///
    /// Shown on every failing row, including the ones with no button, because
    /// the manual step is the half that works everywhere.
    pub fn manual(&self) -> String {
        match self {
            Repair::Chmod { path, to } => format!("chmod {to:o} {}", shell_quote(path)),
        }
    }
}

/// One rule, as measured on this machine.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Finding {
    /// The rule's id, matching what the Privacy page has shown since 0.14.0.
    pub id: &'static str,
    /// Whether the machine is as the rule describes.
    pub ok: bool,
    /// What was measured, in one line.
    pub check: String,
    /// Whether this rule is a reading this platform can take at all.
    ///
    /// `false` for the two mode rules on Windows, which has no file mode. The
    /// distinction matters because `ok` alone cannot carry it: a rule that was
    /// never measured is not a rule that passed, and rendering it green is a
    /// tick the platform has not earned. A row that is not applicable is
    /// neither green nor red, offers nothing, and says why.
    pub applicable: bool,
    /// What a user would run to repair it, or `None` when nothing would.
    pub manual: Option<String>,
    /// What the daemon would apply, or `None` when no repair qualifies.
    ///
    /// `manual` without `repair` is the honest shape for a rule a person can
    /// fix and a process cannot.
    pub repair: Option<Repair>,
}

/// What an applied repair changed.
///
/// Both states, so the user can undo it by hand: `0700, was 0755`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Applied {
    pub path: PathBuf,
    pub was: String,
    pub now: String,
    /// The rule's own check, re-run after the write.
    pub rechecked_ok: bool,
}

impl std::fmt::Display for Applied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}, was {}", self.path.display(), self.now, self.was)
    }
}

/// The mode bits that must be clear on anything semlith wrote.
const OWNER_ONLY: u32 = 0o077;

/// Whether this platform has a file mode at all.
///
/// Windows does not, so the two rules that are readings of one are reported as
/// not applicable there rather than as passing. See [`Finding::applicable`].
const MODES: bool = cfg!(unix);

/// The four Privacy rules that are readings of this machine rather than
/// statements about the code.
///
/// The other six are enforced in `http::answer` before any handler runs and
/// report what the code does, so they can neither fail nor be repaired. They
/// stay in `routes::rules`, where they are written beside the rule text they
/// belong to; this function owns only the ones with a measurement behind them,
/// because those are the ones a repair could apply to.
///
/// `stores` is every store the caller has open. The CLI passes the registered
/// set and the daemon passes its own, which is why it is a parameter rather
/// than read here: a repair must apply to the stores the surface is reporting
/// on, not to a different list assembled underneath it.
pub fn privacy_findings(stores: &[(String, PathBuf)]) -> Vec<Finding> {
    let mut out = Vec::new();

    // `private addresses` first, because it is the one with no repair and the
    // shape of the rest is easier to read against it.
    let allow_private = std::env::var_os(crate::add::ALLOW_PRIVATE_ENV).is_some();
    out.push(Finding {
        id: "private addresses",
        ok: !allow_private,
        check: if allow_private {
            format!(
                "{} is set, so private addresses are allowed",
                crate::add::ALLOW_PRIVATE_ENV
            )
        } else {
            "on".to_string()
        },
        // Named as an unset rather than as a command with a path, because the
        // variable is in the environment the daemon was given and the shell
        // that has to unset it is the one the user will start the daemon from
        // next.
        manual: allow_private.then(|| {
            format!(
                "unset {} and restart `semlith start`",
                crate::add::ALLOW_PRIVATE_ENV
            )
        }),
        repair: None,
        applicable: true,
    });

    let cache = crate::model_cache_dir().unwrap_or_default();
    let cache_ok = crate::embed::check_cache_dir(&cache);
    out.push(Finding {
        id: "model cache",
        ok: cache_ok.is_ok(),
        check: match &cache_ok {
            Ok(()) => format!("{} is yours alone", cache.display()),
            Err(e) => e.to_string(),
        },
        // A cache another account owns is not semlith's to change, and a
        // `chmod` on it would fail after the click. The row says which case it
        // is; only the user's own cache gets either half.
        manual: match (&cache_ok, owned_by_us(&cache)) {
            (Err(_), true) => Some(
                Repair::Chmod {
                    path: cache.clone(),
                    to: 0o700,
                }
                .manual(),
            ),
            _ => None,
        },
        repair: match (&cache_ok, owned_by_us(&cache)) {
            (Err(_), true) => Some(Repair::Chmod {
                path: cache.clone(),
                to: 0o700,
            }),
            _ => None,
        },
        applicable: true,
    });

    // The home and every open store, because the rule is about all of them and
    // a row that named only the first would go green with the rest still loose.
    let home_dir = home::home_or_error();
    let mut loose: Vec<(String, PathBuf, u32)> = Vec::new();
    if let Ok(dir) = &home_dir
        && let Some(mode) = home::loose_mode(dir)
    {
        loose.push((dir.display().to_string(), dir.clone(), mode));
    }
    for (name, dir) in stores {
        if let Some(mode) = home::loose_mode(dir) {
            loose.push((name.clone(), dir.clone(), mode));
        }
    }
    out.push(Finding {
        id: "directory modes",
        ok: home_dir.is_ok() && loose.is_empty(),
        check: match (&home_dir, loose.is_empty()) {
            (Err(e), _) => e.to_string(),
            (Ok(dir), true) if !MODES => {
                format!("{} — this platform has no file mode to read", dir.display())
            }
            (Ok(dir), true) => format!("{} and every open store are 0700", dir.display()),
            (Ok(_), false) => loose
                .iter()
                .map(|(name, _, mode)| format!("{name} is {mode:o}"))
                .collect::<Vec<_>>()
                .join(", "),
        },
        manual: (!loose.is_empty()).then(|| {
            loose
                .iter()
                .map(|(_, dir, _)| {
                    Repair::Chmod {
                        path: dir.clone(),
                        to: 0o700,
                    }
                    .manual()
                })
                .collect::<Vec<_>>()
                .join(" && ")
        }),
        // The first loose directory. Pressing the button again repairs the
        // next one and the row names what is left, rather than one click
        // walking a list the user cannot see the end of.
        repair: loose.first().map(|(_, dir, _)| Repair::Chmod {
            path: dir.clone(),
            to: 0o700,
        }),
        applicable: MODES,
    });

    let key_path = home::agent_key_path().unwrap_or_default();
    let key_mode = mode_of(&key_path);
    let key_ok = key_mode.is_none_or(|mode| mode & OWNER_ONLY == 0);
    out.push(Finding {
        id: "agent key",
        ok: key_ok,
        // The two arms said the same sentence until 0.18.0, so a key at 0644
        // rendered exactly like one at 0600 and only the colour differed. The
        // check is what a reader quotes; it has to carry the finding.
        check: match key_mode {
            Some(mode) if mode & OWNER_ONLY == 0 => {
                format!("{} is {mode:o}, owner only", key_path.display())
            }
            Some(mode) => format!(
                "{} is {mode:o}, readable by more than you",
                key_path.display()
            ),
            None if !MODES => format!(
                "{} — this platform has no file mode to read",
                key_path.display()
            ),
            None => format!("{} has no mode to read", key_path.display()),
        },
        manual: (!key_ok).then(|| {
            Repair::Chmod {
                path: key_path.clone(),
                to: 0o600,
            }
            .manual()
        }),
        repair: (!key_ok).then(|| Repair::Chmod {
            path: key_path.clone(),
            to: 0o600,
        }),
        applicable: MODES,
    });

    out
}

/// Apply one repair and re-run the check behind it.
///
/// The re-check is the point. A `chmod` that returns success says the syscall
/// was accepted, not that the rule now holds — a directory under a mount that
/// ignores modes accepts it and changes nothing. What the caller gets back is a
/// reading taken after the write, so a row that goes green went green because
/// the daemon looked again.
pub fn apply(repair: &Repair, stores: &[(String, PathBuf)]) -> Result<Applied> {
    let Repair::Chmod { path, to } = repair;
    let was = mode_of(path).map_or_else(|| "no mode".to_string(), |mode| format!("{mode:o}"));

    // Refused rather than attempted. Every repair this engine offers removes
    // bits; one that added them would be this page recommending the thing it
    // exists to warn about, and the guard is here rather than at the call site
    // so a future repair cannot route around it.
    if let Some(current) = mode_of(path)
        && to & !current != 0
    {
        anyhow::bail!(
            "{} is {current:o} and the repair would set {to:o}, which grants access rather than                  removing it. Refused.",
            path.display()
        );
    }
    if !owned_by_us(path) {
        anyhow::bail!(
            "{} belongs to another account. semlith repairs only what it owns.",
            path.display()
        );
    }

    set_mode(path, *to)?;
    // The one place a privacy rule's reading changes, so the one place the
    // portal's privacy counter moves. `apply_all` is this function in a loop.
    crate::daemon::changes::bump(crate::daemon::changes::Domain::Privacy);

    let now = mode_of(path).map_or_else(|| "no mode".to_string(), |mode| format!("{mode:o}"));
    let rechecked_ok = privacy_findings(stores)
        .iter()
        .find(|finding| finding.repair.as_ref().is_some_and(|next| next == repair))
        .is_none_or(|finding| finding.ok);

    Ok(Applied {
        path: path.clone(),
        was,
        now,
        rechecked_ok,
    })
}

/// Every repair that qualifies, applied. What `semlith doctor --fix` and the
/// portal's Fix all both call.
pub fn apply_all(stores: &[(String, PathBuf)]) -> Vec<Result<Applied>> {
    let mut out = Vec::new();
    // Re-read between repairs rather than collecting the list once: repairing
    // the store home can clear the `directory modes` row on its own, and a list
    // taken up front would then apply a repair to a path already narrowed.
    while let Some(repair) = privacy_findings(stores)
        .into_iter()
        .filter(|finding| !finding.ok)
        .find_map(|finding| finding.repair)
    {
        let applied = apply(&repair, stores);
        let failed = applied.is_err();
        out.push(applied);
        // A repair that did not take would otherwise be offered again for ever.
        if failed {
            break;
        }
        if out.len() > 64 {
            break;
        }
    }
    out
}

/// This file's mode, or `None` on a platform that has none.
///
/// Windows reads `None` for every path, which is why the mode rules there are
/// neither green nor fixable: there is no reading to fail and no repair that
/// would mean anything.
fn mode_of(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o777)
            .ok()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use anyhow::Context;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .with_context(|| format!("setting {} to {mode:o}", path.display()))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        anyhow::bail!("this platform has no file mode to set")
    }
}

/// Whether this process's user owns `path`.
///
/// `true` on a platform with no uid: there, ownership is not what the repairs
/// turn on, and the `chmod` they would apply is refused earlier for having no
/// meaning.
fn owned_by_us(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: reads this process's own uid and cannot fail.
        let me = unsafe { libc::getuid() };
        std::fs::metadata(path).is_ok_and(|m| m.uid() == me)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

/// A path as it can be pasted into a shell.
///
/// Single quotes, because the manual command is shown to be copied and a store
/// name with a space in it would otherwise produce a command that repairs the
/// wrong path or none.
fn shell_quote(path: &Path) -> String {
    let text = path.display().to_string();
    if text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._-/~".contains(c))
    {
        return text;
    }
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unix only: the repair is a `chmod`, and Windows has no mode to set.
    /// `privacy_findings` reports those two rules as not applicable there,
    /// which `the_mode_rules_are_not_applicable_where_there_is_no_mode` covers.
    #[cfg(unix)]
    #[test]
    fn a_repair_that_would_widen_access_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        std::fs::write(&path, "sml_test").unwrap();
        set_mode(&path, 0o600).unwrap();

        let widen = Repair::Chmod {
            path: path.clone(),
            to: 0o644,
        };
        let refused = apply(&widen, &[]).unwrap_err().to_string();
        assert!(
            refused.contains("grants access"),
            "a widening repair was not refused: {refused}"
        );
        assert_eq!(mode_of(&path), Some(0o600), "the file was changed anyway");
    }

    #[cfg(unix)]
    #[test]
    fn a_repair_is_idempotent_and_reports_both_states() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key");
        std::fs::write(&path, "sml_test").unwrap();
        set_mode(&path, 0o644).unwrap();

        let narrow = Repair::Chmod {
            path: path.clone(),
            to: 0o600,
        };
        let first = apply(&narrow, &[]).unwrap();
        assert_eq!(first.was, "644");
        assert_eq!(first.now, "600");
        assert_eq!(mode_of(&path), Some(0o600));

        let second = apply(&narrow, &[]).unwrap();
        assert_eq!(second.was, "600", "the second run saw a different state");
        assert_eq!(second.now, "600", "the second run changed something");
    }

    /// The manual step is offered wherever a repair is, and the two describe
    /// the same change — otherwise a user who types the command and a user who
    /// presses the button end up with different machines.
    #[test]
    fn every_repair_has_the_manual_command_that_matches_it() {
        for finding in privacy_findings(&[]) {
            match (&finding.repair, &finding.manual) {
                (Some(repair), Some(manual)) => assert!(
                    manual.contains(&repair.manual()),
                    "{}'s manual step does not contain its repair: {manual}",
                    finding.id
                ),
                (Some(_), None) => {
                    panic!("{} offers a repair with no manual step", finding.id)
                }
                _ => {}
            }
        }
    }

    /// The rule that cannot be repaired by any process says so, and says it in
    /// the only way that helps: the manual step without a button.
    #[test]
    fn private_addresses_has_no_repair() {
        let finding = privacy_findings(&[])
            .into_iter()
            .find(|f| f.id == "private addresses")
            .expect("the rule is reported");
        assert!(
            finding.repair.is_none(),
            "a rule a daemon cannot repair was given a button"
        );
    }

    /// A rule that was never measured is not a rule that passed.
    ///
    /// Windows has no file mode, so the two rules that are readings of one are
    /// reported as not applicable there. Rendering them green would be a tick
    /// the platform has not earned, and offering a button would be offering a
    /// `chmod` that cannot mean anything.
    #[test]
    fn the_mode_rules_are_not_applicable_where_there_is_no_mode() {
        for finding in privacy_findings(&[]) {
            let is_a_mode_rule = matches!(finding.id, "directory modes" | "agent key");
            assert_eq!(
                finding.applicable,
                !is_a_mode_rule || MODES,
                "{} reports applicable = {} on a platform where MODES is {MODES}",
                finding.id,
                finding.applicable
            );
            if !finding.applicable {
                assert!(
                    finding.repair.is_none() && finding.manual.is_none(),
                    "{} offers a repair for something it did not measure",
                    finding.id
                );
                assert!(
                    finding.check.contains("no file mode"),
                    "{} does not say why it was not measured: {}",
                    finding.id,
                    finding.check
                );
            }
        }
    }

    /// A shell command shown to be copied has to survive a path with a space.
    #[test]
    fn a_manual_command_quotes_a_path_that_needs_it() {
        let repair = Repair::Chmod {
            path: PathBuf::from("/tmp/two words/.semlith"),
            to: 0o700,
        };
        assert_eq!(repair.manual(), "chmod 700 '/tmp/two words/.semlith'");
    }
}

// ------------------------------------------------------------------ clients

/// One client, as this machine has it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClientReport {
    pub name: String,
    /// The program that would register it, or `None` for a client with no
    /// registration CLI.
    pub command: Option<String>,
    /// Whether that program is on `PATH`. "Not installed" and "installed and
    /// not registered" are different answers and are never collapsed.
    pub present: bool,
    /// Whether semlith is registered for every project.
    pub registered: bool,
    /// `user`, `project`, or `None` when nothing was found.
    pub scope: Option<&'static str>,
    /// Every user-level configuration file this client has, and what is in it.
    pub files: Vec<FileReport>,
    /// The command that repairs this row, when one would.
    pub repair: Option<String>,
    /// Why semlith cannot register this client, for the three that it cannot.
    pub note: Option<String>,
    /// Whether this row is something wrong with the machine, as opposed to
    /// something semlith could do if asked.
    ///
    /// The distinction is what makes the exit code usable in a script. A client
    /// that is not installed is not a fault — most people have two or three of
    /// the twenty-seven. Nor is a file-only client the user has never opted into
    /// writing: `--register-all` is an offer, and a report that went red because
    /// the user had not taken it would be red on almost every machine, which is
    /// the same as not reporting at all. A fault is a client that is on this
    /// machine and cannot see semlith.
    pub fault: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FileReport {
    pub path: PathBuf,
    pub exists: bool,
    /// `false` for a file that is there and does not parse — which is a fault
    /// worth naming, because `--register-all` will refuse it.
    pub parses: bool,
    pub names_semlith: bool,
}

/// What every documented client looks like on this machine.
///
/// Reads files; runs nothing. Asking sixteen client CLIs whether they know
/// about semlith would be sixteen processes, and the portal calls this on every
/// load — `src/setup.rs` already carries the scar from doing that with one.
pub fn clients_report() -> Vec<ClientReport> {
    let project_scoped = claude_project_entries();
    crate::clients::clients()
        .iter()
        .map(|client| {
            let command = client
                .register_command()
                .and_then(|c| c.split_whitespace().next().map(str::to_string));
            let present = command.as_deref().is_some_and(on_path);

            let files: Vec<FileReport> = client
                .config_files()
                .filter_map(|stanza| crate::clientfile::resolve(stanza).map(|p| (stanza, p)))
                .map(|(stanza, path)| {
                    let text = std::fs::read_to_string(&path).ok();
                    FileReport {
                        exists: text.is_some(),
                        parses: match (&text, stanza.format.as_str()) {
                            (None, _) => true,
                            (Some(text), "json") => {
                                serde_json::from_str::<serde_json::Value>(text).is_ok()
                            }
                            _ => true,
                        },
                        names_semlith: text.as_deref().is_some_and(|t| t.contains("semlith")),
                        path,
                    }
                })
                .collect();

            let in_a_file = files.iter().any(|f| f.names_semlith);
            // Claude Code is the one client whose project-scope entries are
            // visible from outside it, and it is the client the defect was
            // found on: `~/.claude.json` held semlith under
            // `projects."…".mcpServers`, so every other project opened with no
            // semlith tools and nothing said why.
            let only_project = client.name == "Claude Code" && !project_scoped.is_empty();
            let registered = in_a_file || (client.name == "Claude Code" && claude_user_entry());

            let note = if crate::clients::UNREGISTERABLE.contains(&client.name.as_str()) {
                Some(client.note.clone())
            } else {
                None
            };
            // "On this machine" for a client with a CLI means the CLI is on
            // `PATH`. For a file-only client it means its configuration
            // directory already exists — the user has that client and has
            // configured something in it — because there is nothing else to
            // ask.
            let in_use = present
                || files
                    .iter()
                    .any(|f| f.exists || f.path.parent().is_some_and(Path::exists));
            let fault = note.is_none() && in_use && !registered;

            // A client that is not on this machine gets no command. It used to
            // get one — Kilo Code read "not installed" and carried `semlith
            // setup --register-all` beside it, and the Agents page's dry run
            // then proposed writing a configuration file for a client semlith
            // had just said was not there. Most people have two or three of
            // the twenty-seven; a report that offered a remedy for the other
            // twenty-four is a report nobody reads twice.
            let repair = if note.is_some() || !in_use {
                None
            } else if only_project && !registered {
                Some(format!(
                    "semlith setup  # removes {} project-scope registration(s) and registers at user scope",
                    project_scoped.len()
                ))
            } else if !registered && client.needs_a_file_written() {
                // Two commands for what reads as one state, so the cell says
                // why they differ: this client is registered by writing its
                // configuration file, which the plain `semlith setup` of the
                // row above it does not do.
                Some(format!(
                    "semlith setup --register-all  # {} is registered by writing its config file, which `semlith setup` alone does not do",
                    client.name
                ))
            } else if !registered && present {
                Some("semlith setup".to_string())
            } else {
                None
            };

            ClientReport {
                name: client.name.clone(),
                command,
                present,
                registered,
                fault,
                scope: match (registered, only_project) {
                    (true, _) => Some("user"),
                    (false, true) => Some("project"),
                    (false, false) => None,
                },
                files,
                repair,
                note,
            }
        })
        .collect()
}

/// Claude Code's per-project semlith registrations, which are the bug.
///
/// Read out of `~/.claude.json`'s `projects` map. Specific to one client on
/// purpose: it is the only client whose project-scope entries live in a
/// user-level file where anything else can see them, and it is the client this
/// release was found on.
fn claude_project_entries() -> Vec<String> {
    let Some(config) = claude_config() else {
        return Vec::new();
    };
    config
        .get("projects")
        .and_then(|p| p.as_object())
        .map(|projects| {
            projects
                .iter()
                .filter(|(_, value)| {
                    value
                        .get("mcpServers")
                        .and_then(|s| s.get("semlith"))
                        .is_some()
                })
                .map(|(path, _)| path.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn claude_user_entry() -> bool {
    claude_config().is_some_and(|config| {
        config
            .get("mcpServers")
            .and_then(|s| s.get("semlith"))
            .is_some()
    })
}

fn claude_config() -> Option<serde_json::Value> {
    let path = home::user_home().ok()?.join(".claude.json");
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Whether a program name resolves on `PATH`.
///
/// A walk rather than a spawn. The caller asks about sixteen of these on a
/// route the portal loads every time, and sixteen processes there is the
/// twenty-second hang `semlith setup` already learned about with one.
fn on_path(program: &str) -> bool {
    if program.contains('/') || program.contains('\\') {
        return Path::new(program).exists();
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(program);
        candidate.is_file()
            || std::env::var_os("PATHEXT")
                .map(|exts| {
                    std::env::split_paths(&exts)
                        .any(|ext| dir.join(format!("{program}{}", ext.display())).is_file())
                })
                .unwrap_or(false)
    })
}
