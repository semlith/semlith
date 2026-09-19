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

    out.push(on_path_finding());

    out
}

/// Every `semlith` on `PATH`, and which one a bare command reaches.
///
/// Reported here rather than beside the clients because it is a reading of the
/// machine and not of a client: it is the same answer for all twenty-seven, and
/// it is the shape `Finding` already has — an id, what was measured, and the
/// command a person would type. The Privacy page renders the rules it names by
/// id and so does not show this one, which is right: it is not a privacy rule
/// and it is not a claim about what the code does.
///
/// `ok` is `true` however many are found, and that is the decision in this
/// function rather than an oversight. `ok` is what `semlith doctor`'s exit code
/// is built from, and a machine with a second install left over from a
/// `cargo install` is not a machine with something wrong — it is a machine
/// worth telling. A rule that went red for it would go red on every developer's
/// machine, and an exit code that did that is one every script learns to
/// ignore.
fn on_path_finding() -> Finding {
    let found = semliths_on_path();
    let spelled = |semlith: &OnPath| format!("{} ({})", semlith.path.display(), semlith.version);
    Finding {
        id: "semlith on PATH",
        ok: true,
        check: match found.as_slice() {
            // Not a fault either. From 0.21.0 every registration semlith writes
            // names an absolute path, so a client reaches the binary whether or
            // not `PATH` has heard of it.
            [] => "no semlith on PATH".to_string(),
            [only] => spelled(only),
            [first, shadowed @ ..] => format!(
                "{} is what a bare `semlith` reaches; also {}",
                spelled(first),
                shadowed.iter().map(spelled).collect::<Vec<_>>().join(", ")
            ),
        },
        // An offer, printed. Not a prompt: `doctor` is read from scripts and a
        // question on stdin would hang the next hook that runs it. Not a
        // removal either — a second install is the user's, and semlith repairs
        // only what it wrote.
        manual: (found.len() > 1).then(|| {
            found[1..]
                .iter()
                .map(|semlith| format!("rm {}", shell_quote(&semlith.path)))
                .collect::<Vec<_>>()
                .join(" && ")
        }),
        repair: None,
        applicable: true,
    }
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
            "{} is {current:o} and the repair would set {to:o}, which grants access rather than removing it. Refused.",
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
    /// Directories where this client is registered for semlith and has been
    /// switched off for that directory alone.
    ///
    /// Empty for every client but Claude Code, which is the only one whose
    /// per-directory overrides live in a user-level file anything else can
    /// read. It is the first entry because it is the one that is invisible:
    /// `registered` is true, `scope` is `user`, the binary answers, and the
    /// session still opens with no semlith.
    pub disabled_in: Vec<String>,
    /// Whether the directory doctor is being run from is one of them.
    pub disabled_here: bool,
    /// The sentence that says why this row is red, when the reason is not
    /// readable from the row itself.
    pub explain: Option<String>,
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
    /// Whether this client is on this machine at all.
    ///
    /// Wider than `present`, which is only about a CLI on `PATH`: a client
    /// semlith registers by writing a file is here if its configuration
    /// directory is. The portal read the two as one, so a client with a
    /// configuration directory and no CLI was labelled "not installed" and
    /// handed a fix command in the next column — one row saying two different
    /// things, which is finding 3.8 from the other end.
    pub in_use: bool,
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

/// A registration that named a bare command, and what it was rewritten to.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Rewritten {
    pub client: String,
    pub path: PathBuf,
    /// The command that was there, which could not launch.
    pub was: String,
    /// The absolute path that replaced it.
    pub now: String,
}

/// Rewrite every registration semlith wrote that names a bare command.
///
/// Every machine registered before 0.21.0 is in this state. The entry says
/// `"command": "semlith"`, which launches only from a `PATH` that happens to
/// carry the binary — and the process that reads these files is very often not
/// a login shell: an editor started from a desktop icon, a launchd or systemd
/// agent, a desktop app. The client reports that the server exited and says
/// nothing about why, which reads as semlith being broken.
///
/// Repaired rather than reported, and not behind a flag. This is semlith's own
/// entry, written by semlith, that objectively cannot launch; a report telling
/// the user to run a second command to fix what the first command already
/// diagnosed is a report that leaves most machines broken. The three things
/// that make it safe to do unasked are the three `clientfile` already
/// guarantees: the merge preserves every key it did not come to change, the
/// file is copied to `<name>.semlith-backup` before its first write, and the
/// write goes through a neighbouring temporary file and a rename, so a config
/// is never left half-written.
///
/// A caller that must not write skips this function and calls
/// [`clients_report_read_only`] instead of [`clients_report`]. Nothing else in
/// either path touches a file.
pub fn repair_bare_commands() -> Vec<Rewritten> {
    // A build artifact does not get to rewrite anyone's client configuration.
    //
    // The repair points every entry at `current_exe()`, and during development
    // that is `target/debug/semlith` — so running `./target/debug/semlith
    // doctor` once would silently repoint a developer's real Cursor, Codex and
    // Copilot registrations at a binary inside a directory `cargo clean`
    // deletes. They would then have no semlith and a configuration file that
    // looks correct, which is the exact failure this release exists to remove,
    // manufactured by the command meant to detect it.
    if running_from_a_build_directory() {
        return Vec::new();
    }
    bare_entries()
        .into_iter()
        .filter(|(client, stanza, path, ..)| {
            crate::clientfile::apply(
                &[crate::clientfile::Plan {
                    client: client.name.clone(),
                    path: path.clone(),
                    action: crate::clientfile::Action::Merge,
                }],
                &[(client.name.as_str(), stanza)],
            )
            .is_ok()
        })
        .map(|(client, _, path, was, now)| Rewritten {
            client: client.name.clone(),
            path,
            was,
            now,
        })
        .collect()
}

/// Whether this binary is a build artifact rather than an installed semlith.
///
/// `target/debug` and `target/release` under a directory holding a
/// `Cargo.toml`. Narrow on purpose: a user who installed semlith into a
/// directory that happens to be called `target` is not who this is about, and
/// refusing to repair on their machine would be worse than the defect.
fn running_from_a_build_directory() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(profile) = exe.parent() else {
        return false;
    };
    let named = profile
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "debug" || n == "release");
    named
        && profile
            .parent()
            .is_some_and(|target| target.file_name().and_then(|n| n.to_str()) == Some("target"))
        && profile
            .parent()
            .and_then(Path::parent)
            .is_some_and(|root| root.join("Cargo.toml").is_file())
}

/// Every configuration file holding a semlith entry whose command cannot launch.
///
/// Only the JSON files, and only the ones a stanza gives a `path=` for: those
/// are the files `semlith setup --register-all` wrote, so they are the ones
/// semlith is repairing rather than editing. Claude Code's `~/.claude.json` is
/// deliberately not among them — it has no `path=` stanza because `claude mcp
/// add` owns it, and the repair there is to run that command again, which
/// `semlith setup` does.
///
/// "Cannot launch" is a command with no path separator in it. A command the
/// user pointed somewhere themselves — a wrapper script, a version manager's
/// shim — has one, and is left alone: it may well be doing something semlith
/// does not know about, and it at least names a place rather than a `PATH`.
#[allow(clippy::type_complexity)]
fn bare_entries() -> Vec<(
    &'static crate::clients::Client,
    &'static crate::clients::Stanza,
    PathBuf,
    String,
    String,
)> {
    let mut out = Vec::new();
    for client in crate::clients::clients() {
        for stanza in client.config_files() {
            if stanza.format != "json" {
                continue;
            }
            let Some(path) = crate::clientfile::resolve(stanza) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let (Ok(existing), Ok(documented)) = (
                serde_json::from_str::<serde_json::Value>(&text),
                serde_json::from_str::<serde_json::Value>(&stanza.text),
            ) else {
                continue;
            };
            // Where the command lives is read out of the documented stanza
            // rather than guessed, because the four products put it in four
            // places — `mcpServers.semlith`, `mcp.servers.semlith`,
            // `amp.mcpServers.semlith`, `servers.semlith` — and a walker that
            // went looking for any `"command"` anywhere in the file would find
            // somebody else's server and rewrite it.
            let Some(site) = command_site(&documented) else {
                continue;
            };
            let (Some(was), Some(now)) =
                (command_at(&existing, &site), command_at(&documented, &site))
            else {
                continue;
            };
            if was == now || was.contains('/') || was.contains('\\') {
                continue;
            }
            out.push((client, stanza, path, was, now));
        }
    }
    out
}

/// The chain of object keys from a stanza's root down to the entry that names a
/// command, or `None` for a stanza that names none — every HTTP one.
fn command_site(value: &serde_json::Value) -> Option<Vec<String>> {
    let object = value.as_object()?;
    if object.get("command").and_then(|c| c.as_str()).is_some() {
        return Some(Vec::new());
    }
    object.iter().find_map(|(key, child)| {
        let mut rest = command_site(child)?;
        rest.insert(0, key.clone());
        Some(rest)
    })
}

/// The command at that chain of keys, or `None` when the file does not go there.
fn command_at(value: &serde_json::Value, site: &[String]) -> Option<String> {
    let mut at = value;
    for key in site {
        at = at.get(key)?;
    }
    at.get("command")?.as_str().map(str::to_string)
}

/// What every documented client looks like on this machine, after repairing
/// every registration of semlith's own that cannot launch.
///
/// The repair is here rather than in the caller so that every surface gets it —
/// `semlith doctor`, `semlith setup` and the portal all arrive through this
/// function, and a machine whose entry cannot launch is a machine where the
/// answer any of the three would otherwise print is wrong.
pub fn clients_report() -> Vec<ClientReport> {
    let rewritten = repair_bare_commands();
    report(&rewritten)
}

/// [`clients_report`] without the write.
///
/// For a caller that has promised not to touch the machine. The row still names
/// a bare command it found; it says the entry would be rewritten rather than
/// that it was.
pub fn clients_report_read_only() -> Vec<ClientReport> {
    report(&[])
}

/// Reads files; runs nothing. Asking sixteen client CLIs whether they know
/// about semlith would be sixteen processes, and the portal calls this on every
/// load — `src/setup.rs` already carries the scar from doing that with one.
fn report(rewritten: &[Rewritten]) -> Vec<ClientReport> {
    // After the repair, so a file that was just rewritten is not also reported
    // as still needing it. What is left here either was never repairable or
    // could not be written.
    let still_bare = bare_entries();
    let project_scoped = claude_project_entries();
    let switched_off = claude_disabled_entries();
    let here = std::env::current_dir().ok();
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
            // `PATH`. For a file-only client it means its own configuration
            // file exists — not merely the directory above it.
            //
            // The parent directory used to count, and on an ordinary developer's
            // machine that is `~/.config` or `~/Library/Application Support`,
            // which exist because something else put them there. Twenty-seven
            // clients then produced thirteen "in use" and eleven faults, six of
            // them for software not installed at all — so `semlith doctor`
            // exited non-zero on a healthy machine and `--brief`, whose whole
            // job is to be a session-start hook, was red every time. A check
            // that is always red is the same as no check, which is the failure
            // this release exists to remove.
            let in_use = present || files.iter().any(|f| f.exists);
            // A client that is registered and switched off for the directory
            // it is being asked from is a fault, and the one this release was
            // found on. Nothing else reports it: `registered` is true, the
            // entry is at user scope, the binary answers `initialize` in under
            // a second — and the session opens with no semlith and no reason.
            let disabled_in: Vec<String> = if client.name == "Claude Code" {
                switched_off.clone()
            } else {
                Vec::new()
            };
            let disabled_here = here
                .as_deref()
                .is_some_and(|cwd| disabled_in.iter().any(|dir| same_place(dir, cwd)));
            let repaired = rewritten.iter().find(|r| r.client == client.name);
            // An entry still naming a bare command after the repair ran either
            // could not be written or was never offered the write. Either way
            // this client cannot launch semlith, which is the same fault as not
            // being registered at all and is reported as one.
            let bare = still_bare.iter().find(|(c, ..)| c.name == client.name);
            let fault =
                note.is_none() && in_use && (!registered || disabled_here || bare.is_some());

            // A client that is not on this machine gets no command. It used to
            // get one — Kilo Code read "not installed" and carried `semlith
            // setup --register-all` beside it, and the Agents page's dry run
            // then proposed writing a configuration file for a client semlith
            // had just said was not there. Most people have two or three of
            // the twenty-seven; a report that offered a remedy for the other
            // twenty-four is a report nobody reads twice.
            let repair = if note.is_some() || !in_use {
                None
            } else if disabled_here {
                // A command, not a paragraph. The paragraph — which file, which
                // key, which directory — is the `explain` field, because it is
                // what a person needs and `run:` is what a script copies.
                Some(
                    "claude mcp reset-project-choices  # or turn semlith back on from inside the client with /mcp"
                        .to_string(),
                )
            } else if let Some((_, _, path, ..)) = bare {
                // `semlith doctor` and not `--register-all`: the entry is
                // already there and the write that repairs it is the one this
                // command does by itself. A user reading this line is reading
                // it because the write did not happen — a read-only run, or a
                // file semlith could not replace.
                Some(format!(
                    "semlith doctor  # rewrites the semlith entry in {}",
                    path.display()
                ))
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
                in_use,
                explain: if disabled_here {
                    Some(format!(
                        "registered at user scope and switched off for this directory: {} has \
                         projects[\"{}\"].{DISABLED_KEY} containing \"semlith\". \
                         Every other directory sees the server; this one sees nothing, \
                         and nothing says so.",
                        claude_config_path()
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "~/.claude.json".to_string()),
                        here.as_deref().unwrap_or(Path::new(".")).display(),
                    ))
                } else if let Some(r) = repaired {
                    // Both commands, because the user is being told a file of
                    // theirs was changed without being asked. The old one is
                    // what they would put back if they disagreed.
                    Some(format!(
                        "repaired {}: the semlith entry named `{}`, which launches only from a \
                         PATH that carries it, and now names `{}`. The file it replaced is \
                         beside it as {}{}.",
                        r.path.display(),
                        r.was,
                        r.now,
                        r.path.display(),
                        crate::clientfile::BACKUP,
                    ))
                } else if let Some((_, _, path, was, now)) = bare {
                    Some(format!(
                        "{} names the semlith entry as `{was}`, which launches only from a PATH \
                         that carries it — and the process that reads this file is very often \
                         not a login shell. It has not been rewritten to `{now}`.",
                        path.display(),
                    ))
                } else {
                    None
                },
                disabled_in,
                disabled_here,
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

/// The key Claude Code switches a server off for one project with.
const DISABLED_KEY: &str = "disabledMcpjsonServers";

/// Claude Code projects where semlith is switched off for that project alone.
///
/// The defect of 2026-09-17 and 2026-09-18, and the reason this file reads
/// `~/.claude.json` twice. `claude_project_entries` finds semlith registered in
/// the wrong place; this finds it registered in the right one and turned off —
/// the same silence from the opposite direction, and the one no surface
/// anywhere reported. On the machine it was found on, `claude mcp list` run
/// from any other directory said `semlith: semlith mcp - OK Connected`, so
/// every check short of asking from the affected directory passed.
///
/// Both spellings are read. `disabledMcpjsonServers` is the one the client
/// writes today; `disabledMcpServers` is checked beside it so a rename does not
/// turn this back into a silent absence.
fn claude_disabled_entries() -> Vec<String> {
    let Some(config) = claude_config() else {
        return Vec::new();
    };
    let Some(projects) = config.get("projects").and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    projects
        .iter()
        .filter(|(_, value)| {
            [DISABLED_KEY, "disabledMcpServers"].iter().any(|key| {
                value
                    .get(key)
                    .and_then(|d| d.as_array())
                    .is_some_and(|off| off.iter().any(|name| name.as_str() == Some("semlith")))
            })
        })
        .map(|(path, _)| path.clone())
        .collect()
}

/// Whether two spellings name the same directory.
///
/// Canonicalised when both sides resolve, compared as written when they do not
/// — a configuration file may name a directory that has since been moved, and
/// that entry is still the one switching semlith off. `canonicalize` is what
/// makes this correct across a symlinked home; it is also what produces two
/// spellings of one place on Windows, which is why the comparison is on the
/// canonical form of *both* sides rather than one of each.
fn same_place(written: &str, actual: &Path) -> bool {
    same_file(Path::new(written), actual)
}

fn claude_config_path() -> Option<PathBuf> {
    Some(home::user_home().ok()?.join(".claude.json"))
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
    serde_json::from_str(&std::fs::read_to_string(claude_config_path()?).ok()?).ok()
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
    path_dirs()
        .iter()
        .any(|dir| candidate_in(dir, program).is_some())
}

/// Every place `program` resolves on `PATH`, in the order `PATH` would try them.
///
/// [`on_path`] above stops at the first, because for the sixteen client CLIs the
/// question is only whether there is one. This one keeps looking, and is asked
/// about `semlith` alone: the first is what a bare command reaches and the rest
/// are what it does not, which is a distinction no other program here needs.
///
/// A directory named twice in `PATH` yields its binary once. `PATH` repeating
/// itself is common — a shell profile sourced twice does it — and a report that
/// said "also /usr/local/bin/semlith" about the binary it had already named as
/// the winner would be describing a second install that is not there.
fn every_on_path(program: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for dir in path_dirs() {
        if let Some(found) = candidate_in(&dir, program)
            && !out.iter().any(|seen| same_file(seen, &found))
        {
            out.push(found);
        }
    }
    out
}

/// The directories on `PATH`, empty when there is no `PATH` at all.
fn path_dirs() -> Vec<PathBuf> {
    let paths = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&paths).collect()
}

/// Where `program` would be found in one `PATH` directory, if anywhere.
///
/// `PATHEXT` is Windows' list of the suffixes an unqualified name may carry, so
/// `semlith` there is `semlith.exe` on disk. Reading it here rather than
/// appending `.exe` in the caller keeps the one spelling of this rule in the one
/// place both callers go through.
fn candidate_in(dir: &Path, program: &str) -> Option<PathBuf> {
    let bare = dir.join(program);
    if bare.is_file() {
        return Some(bare);
    }
    let exts = std::env::var_os("PATHEXT")?;
    std::env::split_paths(&exts)
        .map(|ext| dir.join(format!("{program}{}", ext.display())))
        .find(|candidate| candidate.is_file())
}

/// Whether two paths name the same file, comparing what they resolve to.
///
/// Canonicalised on both sides rather than one: a `PATH` entry is very often a
/// symlink — `/usr/local/bin/semlith` pointing into a Homebrew cellar, `~/.local/bin`
/// into a version manager — and two spellings of one binary counted as two
/// installs is the false alarm this whole report exists to avoid raising.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// One `semlith` binary `PATH` resolves to.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OnPath {
    pub path: PathBuf,
    /// What it answered to `--version`, or why it did not answer.
    pub version: String,
}

/// Every `semlith` on `PATH`, with its version, first one first.
///
/// The machine this was found on had two: `~/.semlith/bin/semlith` at 0.20.1 in
/// front, and behind it a `~/.cargo/bin/semlith` at 0.14.0 left over from a
/// `cargo install` nobody remembered. Nothing on that machine said so. From
/// 0.21.0 every registration semlith writes names an absolute path, so this is
/// no longer what decides which binary a client launches — but anything that
/// spawns the bare name still resolves through here, and a version that old
/// speaks a protocol the caller does not.
///
/// One spawn per binary found, which is normally one and is normally none: the
/// running executable is asked nothing, because it knows its own version. This
/// is deliberately not on `clients_report`'s path, which the portal calls on
/// every page load and which already refuses to ask sixteen client CLIs
/// anything.
pub fn semliths_on_path() -> Vec<OnPath> {
    let running = std::env::current_exe().ok();
    every_on_path("semlith")
        .into_iter()
        .map(|path| OnPath {
            version: match &running {
                Some(running) if same_file(running, &path) => {
                    format!("semlith {}", env!("CARGO_PKG_VERSION"))
                }
                _ => version_of(&path),
            },
            path,
        })
        .collect()
}

/// What one binary says it is.
///
/// Anything it prints is taken as the answer, stderr included and the first line
/// only. This runs something the user installed, not something semlith wrote:
/// an old release, a shell wrapper, a stub from a package manager. A binary that
/// cannot be run at all is reported as that rather than skipped, because a
/// `semlith` on `PATH` that does not execute is exactly the state a user would
/// want named.
fn version_of(path: &Path) -> String {
    let Ok(out) = std::process::Command::new(path).arg("--version").output() else {
        return "did not run".to_string();
    };
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let said = if stdout.is_empty() {
        String::from_utf8_lossy(&out.stderr).trim().to_string()
    } else {
        stdout
    };
    said.lines().next().unwrap_or("said nothing").to_string()
}

// -------------------------------------------------------------------- proof

/// What actually happens when something tries to reach semlith.
///
/// Every other check in this file reads a file. A file can say semlith is
/// registered while nothing can launch it: that was true on the machine this
/// release was found on, twice, and each time every reading-based check passed.
/// This one starts a server and asks a client.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Proof {
    /// The command a registration names — the thing that has to launch.
    pub command: String,
    /// Whether that command, run with the environment a service manager gives
    /// a job rather than the one a login shell builds, answered `initialize`.
    pub launched: bool,
    /// How many tools it then listed. `None` when it never launched.
    pub tools: Option<usize>,
    /// What the client itself says, for the client that can be asked.
    pub client: Option<ClientVerdict>,
    /// The first step that failed, and the command that shows it. `None` when
    /// every step passed.
    pub failed: Option<String>,
}

/// The command Claude Code's own configuration names for semlith.
///
/// Read rather than assumed. Claude Code owns `~/.claude.json` through
/// `claude mcp add`, so semlith does not rewrite it — which means a machine
/// registered before 0.21.0 still has a bare `semlith` there, and that is
/// precisely the string worth trying to launch.
fn claude_registered_command() -> Option<String> {
    claude_config()?
        .get("mcpServers")?
        .get("semlith")?
        .get("command")?
        .as_str()
        .map(str::to_string)
}

/// A client's own answer about semlith, in the client's own words.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClientVerdict {
    pub name: &'static str,
    /// The line the client printed, trimmed.
    pub says: String,
    pub connected: bool,
}

/// Run the four steps: registered, launched, listed the tools, and — for the
/// one client that can be asked — connected.
///
/// Deliberately not called by `clients_report`, and so not by the portal, which
/// reads that on every page load. This spawns processes.
pub fn proof() -> Proof {
    // The command a client would actually run, not the one semlith would write
    // today. Those differ on every machine registered before 0.21.0, and the
    // difference is the whole question: `binary_path()` always launches,
    // because it is this process. A bare `semlith` in somebody's existing
    // configuration is what does not.
    let command =
        claude_registered_command().unwrap_or_else(|| crate::clients::binary_path().to_string());
    let (launched, tools) = launches(&command);
    let client = claude_verdict();

    let failed = if !launched {
        Some(format!(
            "`{command}` is what a client would run, and it does not launch from a plain PATH. Show it with: env -i PATH=/usr/bin:/bin:/usr/local/bin HOME=\"$HOME\" sh -c '{command} mcp'. Repair it with: semlith setup"
        ))
    } else if tools.is_none_or(|n| n == 0) {
        Some(format!(
            "the server launched and listed no tools. Show it with: \
             printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\",\"params\":{{}}}}' \
             | {command} mcp"
        ))
    } else {
        client.as_ref().filter(|v| !v.connected).map(|v| {
            format!(
                "{} launched semlith and did not connect to it: {}. \
                 Show it with: claude mcp get semlith",
                v.name, v.says,
            )
        })
    };

    Proof {
        command,
        launched,
        tools,
        client,
        failed,
    }
}

/// Start the registered command the way something other than a login shell
/// would, and see whether it answers.
///
/// The environment is cleared apart from `HOME`, and `PATH` is the one a
/// process gets when nobody has arranged otherwise. That is the whole point:
/// a bare `semlith` in a registration launches from a developer's interactive
/// shell and from nowhere else, and reading the configuration file cannot tell
/// the difference.
fn launches(command: &str) -> (bool, Option<usize>) {
    use std::io::Write;
    let mut child = match std::process::Command::new(command)
        .arg("mcp")
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/local/bin")
        .env("HOME", home::user_home().unwrap_or_default())
        .env("SEMLITH_AIRGAP", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return (false, None),
    };
    // Airgapped on purpose: this asks whether the server answers, not whether
    // it can embed, and a `doctor` that downloads a model is a `doctor` nobody
    // runs twice. 0.21.0 is what makes the handshake independent of the model,
    // so this is only possible from this release onwards.
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2024-11-05","capabilities":{{}},"clientInfo":{{"name":"semlith-doctor","version":"1"}}}}}}"#
        );
        let _ = writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{{}}}}"#
        );
    }
    drop(child.stdin.take());
    let Ok(done) = child.wait_with_output() else {
        return (false, None);
    };
    let mut launched = false;
    let mut tools = None;
    for line in String::from_utf8_lossy(&done.stdout).lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match value.get("id").and_then(serde_json::Value::as_u64) {
            Some(1) => launched = value.get("result").is_some(),
            Some(2) => {
                tools = value["result"]["tools"].as_array().map(Vec::len);
            }
            _ => {}
        }
    }
    (launched, tools)
}

/// Ask Claude Code, from a directory that is nobody's project, what it sees.
///
/// A fresh directory because the answer depends on it: a server registered at
/// user scope and switched off for one project reports connected from
/// everywhere else, which is how it stayed invisible. This asks from neutral
/// ground; `clients_report` asks about the directory the user is standing in.
///
/// One client, because it is the only one with a command that answers this
/// question in under two seconds. The rest are read from their files, and the
/// report says so rather than implying a pass.
fn claude_verdict() -> Option<ClientVerdict> {
    if !on_path("claude") {
        return None;
    }
    let neutral = std::env::temp_dir();
    let out = std::process::Command::new("claude")
        .args(["mcp", "get", "semlith"])
        .current_dir(neutral)
        .output()
        .ok()?;
    let said = String::from_utf8_lossy(&out.stdout);
    let status = said
        .lines()
        .find(|line| line.trim_start().starts_with("Status:"))
        .map(|line| line.trim().to_string())
        .unwrap_or_else(|| {
            said.lines()
                .next()
                .unwrap_or("no answer")
                .trim()
                .to_string()
        });
    Some(ClientVerdict {
        name: "Claude Code",
        connected: status.contains("Connected"),
        says: status,
    })
}
