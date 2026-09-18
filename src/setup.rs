//! `semlith setup`: the far half of the install one-liner.
//!
//! The script's job ends when a binary is on disk. Everything between that and
//! a working semlith — a `PATH` that finds it, weights already fetched so the
//! first `index` is not a silent four-minute wait, and an agent that knows the
//! MCP server exists — is here.
//!
//! Every step is idempotent and reports "already done" rather than doing it
//! again, which is what makes this the repair command as well as the install
//! one. [`status`] computes the same per-step answers without changing
//! anything; the portal's `/api/setup` route is that function and nothing else,
//! so the page and the terminal cannot disagree about what is set up.
//!
//! Nothing here writes outside the semlith home, the shell's rc file and the
//! model cache, and nothing here uses `sudo`.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{chunk, clients, embed, home, model_cache_dir};

/// The fences around the block this command owns in a shell rc file. They are
/// what make a second run an update instead of an append: the block between
/// them is replaced, and anything outside them is never touched.
const BEGIN: &str = "# >>> semlith >>>";
const END: &str = "# <<< semlith <<<";

/// What one step found or did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// The step ran and changed something.
    Done,
    /// The step had nothing to do.
    AlreadyDone,
    /// The step was deliberately not run — `--airgap`, or a client the user
    /// did not pick.
    Skipped,
    /// The step could not finish. Never fatal on its own: a client's CLI
    /// refusing is not a reason to abandon an otherwise good install.
    Failed,
}

/// One line of the report, in the terminal and in the portal alike.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Step {
    pub name: &'static str,
    pub state: State,
    /// One line a human can act on: a path, a version, a reason.
    pub detail: String,
}

/// Everything `semlith setup` knows, computed without touching anything.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Status {
    pub bin_dir: String,
    pub binary_installed: bool,
    pub on_path: bool,
    pub rc_file: Option<String>,
    pub rc_block_present: bool,
    pub model_cache: String,
    pub model_cached: bool,
    /// The clients whose own configuration file names semlith, read from disk
    /// rather than by asking each client's CLI.
    pub registered_clients: Vec<String>,
    pub version: &'static str,
    /// The install one-liners, so the portal shows the text the README
    /// documents rather than a second copy that can drift.
    pub install_sh: &'static str,
    pub install_ps1: &'static str,
    pub steps: Vec<Step>,
}

/// The macOS and Linux one-liner. The URL resolves at run time, which is what
/// lets a fix to the script reach people without a release.
pub const INSTALL_SH: &str =
    "curl -fsSL https://raw.githubusercontent.com/semlith/semlith/main/install.sh | sh";

/// The Windows one-liner.
pub const INSTALL_PS1: &str =
    "irm https://raw.githubusercontent.com/semlith/semlith/main/install.ps1 | iex";

/// Read-only. Every field is a question the portal asks and `run` answers.
pub fn status() -> Status {
    // The one place in this file that cannot return an error: the portal asks
    // for a status and gets one. An unresolvable home makes every step report
    // what it could not find, rather than reporting a path semlith made up.
    let bin = home::bin_dir();
    let bin_path = bin.as_deref().unwrap_or(Path::new(""));
    let rc = rc_file();
    let block = rc
        .as_deref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| text.contains(BEGIN))
        .unwrap_or(false);
    // A cache that cannot be resolved is a cache with nothing in it, which is
    // what the model step already reports.
    let cache = model_cache_dir().unwrap_or_default();

    // Read from each client's own configuration file rather than by asking
    // each client's CLI. This used to run `claude mcp list` and wait for it,
    // which on a slow machine turned the portal's Agents page into a
    // twenty-second hang — and that was for one client. Sixteen processes here
    // would be sixteen times that, on a route the page calls on every load.
    let registered = crate::clientfile::registered_clients();
    let installed = bin.is_ok() && bin_path.join(exe_name()).exists();
    let path_has_bin = bin.is_ok() && on_path(bin_path);
    let model_cached = embed::is_cached(&cache);

    let steps = vec![
        Step {
            name: "binary",
            state: if installed {
                State::AlreadyDone
            } else {
                State::Skipped
            },
            detail: match &bin {
                Ok(dir) => dir.join(exe_name()).display().to_string(),
                Err(e) => e.to_string(),
            },
        },
        Step {
            name: "path",
            // The rc file counts. A daemon started from a shell that predates
            // the edit has the old `PATH` for as long as it runs, so asking
            // only `on_path` reported "not done" immediately after the portal
            // had just written the block — and the button looked broken.
            state: if path_has_bin || block {
                State::AlreadyDone
            } else {
                State::Skipped
            },
            detail: match (&rc, path_has_bin, block) {
                (Some(p), false, true) => {
                    format!("{} — a new shell picks it up", p.display())
                }
                (Some(p), _, _) => p.display().to_string(),
                (None, _, _) => "no shell rc file found".into(),
            },
        },
        Step {
            name: "model",
            state: if model_cached {
                State::AlreadyDone
            } else {
                State::Skipped
            },
            detail: cache.display().to_string(),
        },
        Step {
            name: "agents",
            state: if registered.is_empty() {
                State::Skipped
            } else {
                State::AlreadyDone
            },
            detail: if registered.is_empty() {
                "no client on this machine has the semlith server".into()
            } else {
                format!("{} has the semlith server", registered.join(", "))
            },
        },
    ];

    Status {
        bin_dir: bin_path.display().to_string(),
        binary_installed: installed,
        on_path: path_has_bin,
        rc_file: rc.map(|p| p.display().to_string()),
        rc_block_present: block,
        model_cache: cache.display().to_string(),
        model_cached,
        registered_clients: registered,
        version: env!("CARGO_PKG_VERSION"),
        install_sh: INSTALL_SH,
        install_ps1: INSTALL_PS1,
        steps,
    }
}

/// The guided flow. `yes` takes the default at every prompt — add to `PATH`,
/// download the model — so a script or an agent can install semlith with
/// nothing attached to stdin.
///
/// Registration is not one of those prompts from 0.18.0. It runs either way,
/// because an install that leaves the agent unable to see the store is not an
/// install anybody wanted, and because every registration is now the stdio
/// form: no key lands in a file, and nothing outside the client's own registry
/// is touched. `register_all` is the one part that does ask, and it asks with
/// the list of files in front of the user.
pub fn run(yes: bool, airgap: bool, register_all: bool, service: bool) -> Result<()> {
    let _ = cliclack::intro(format!(" semlith {} setup ", env!("CARGO_PKG_VERSION")));

    // Reported as they run rather than replayed at the end, so the line about
    // a step is next to whatever that step printed.
    let report = [
        announce(step_binary(yes)?),
        announce(step_path(yes)?),
        announce(step_model(yes, airgap)?),
        announce(step_agents(register_all, yes)?),
        announce(step_service(service)?),
        announce(step_verify()?),
    ];

    if report.iter().all(|s| s.state != State::Done) {
        let _ = cliclack::log::success("Everything was already set up. Nothing changed.");
    }

    let _ = cliclack::outro(format!(
        "Next: `semlith index .` to index this directory, then `semlith start` \
         for the portal on http://127.0.0.1:7365\n  Open a new shell first if \
         {} was only just added to PATH.",
        home::bin_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|e| e.to_string())
    ));
    Ok(())
}

/// One line per step, in the order the steps ran.
fn announce(step: Step) -> Step {
    let line = format!("{}: {}", step.name, step.detail);
    let _ = match step.state {
        State::Done => cliclack::log::success(line),
        State::AlreadyDone => cliclack::log::info(format!("{line} (already done)")),
        State::Skipped => cliclack::log::info(format!("{line} (skipped)")),
        State::Failed => cliclack::log::warning(line),
    };
    step
}

/// Where a shell looks for the binary this release installs.
fn exe_name() -> &'static str {
    if cfg!(windows) {
        "semlith.exe"
    } else {
        "semlith"
    }
}

fn on_path(bin: &Path) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|p| p == bin))
        .unwrap_or(false)
}

/// Step 1. The scripts put the binary here themselves; this covers the person
/// who downloaded an archive by hand and then ran `semlith setup` out of it.
fn step_binary(yes: bool) -> Result<Step> {
    let bin = home::bin_dir()?;
    let target = bin.join(exe_name());
    let running = std::env::current_exe().context("locating the running binary")?;

    if running == target || target.exists() {
        return Ok(Step {
            name: "binary",
            state: State::AlreadyDone,
            detail: target.display().to_string(),
        });
    }

    let copy = yes
        || cliclack::confirm(format!("Copy this binary into {}?", bin.display()))
            .initial_value(true)
            .interact()
            .unwrap_or(false);
    if !copy {
        return Ok(Step {
            name: "binary",
            state: State::Skipped,
            detail: format!("left at {}", running.display()),
        });
    }

    home::secure_dir(&bin).with_context(|| format!("creating {}", bin.display()))?;
    std::fs::copy(&running, &target)
        .with_context(|| format!("copying {} to {}", running.display(), target.display()))?;
    Ok(Step {
        name: "binary",
        state: State::Done,
        detail: target.display().to_string(),
    })
}

/// The rc file of `$SHELL`. `.profile` is the fallback rather than nothing,
/// because a login shell reads it and an unknown shell is still a shell.
///
/// `SHELL` unset is the case that matters: a container, a CI runner and a cron
/// job all have a HOME and no `SHELL`, and returning `None` there left the
/// binary off `PATH` after an otherwise perfect install. Only a missing HOME
/// means there is genuinely nowhere to write.
fn rc_file() -> Option<PathBuf> {
    let base = home::user_home().ok()?;
    let shell = std::env::var("SHELL").unwrap_or_default();
    let name = Path::new(&shell)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Some(match name.as_str() {
        "zsh" => base.join(".zshrc"),
        "fish" => base.join(".config").join("fish").join("config.fish"),
        "bash" => {
            let rc = base.join(".bashrc");
            if rc.exists() {
                rc
            } else {
                base.join(".bash_profile")
            }
        }
        _ => base.join(".profile"),
    })
}

/// Step 2. The block is appended at the end and *prepends* the bin directory,
/// so the binary this install put there wins over an older one somebody
/// dropped in `/usr/local/bin` and forgot.
/// Run the PATH step on its own, non-interactively.
///
/// The portal's setup panel reports this step as not done and then tells the
/// reader to go and run a command; a panel that can see the problem can fix it.
/// `yes` is forced, because a button press is the confirmation.
pub fn run_path_step() -> Result<Step> {
    step_path(true)
}

fn step_path(yes: bool) -> Result<Step> {
    let bin = home::bin_dir()?;
    if on_path(&bin) {
        return Ok(Step {
            name: "path",
            state: State::AlreadyDone,
            detail: bin.display().to_string(),
        });
    }

    if cfg!(windows) {
        return step_path_windows(&bin, yes);
    }

    let Some(rc) = rc_file() else {
        return Ok(Step {
            name: "path",
            state: State::Failed,
            detail: format!(
                "no HOME, so no rc file to edit — add {} to PATH by hand",
                bin.display()
            ),
        });
    };

    let existing = std::fs::read_to_string(&rc).unwrap_or_default();

    // A block that is already there is replaced when it is not the block this
    // version writes, rather than left because something with the right fences
    // exists. Until 0.18.0 this returned here on sight of `BEGIN`, so a user
    // upgrading kept whatever an older version had written for ever — and what
    // 0.17.3 wrote was an export of `SEMLITH_AGENT_KEY`, a credential placed in
    // the environment of every process they start, which nothing reads any more.
    //
    // Found by the clean-container check between the tag and the publish, on a
    // release whose whole subject is that no file semlith writes carries the key.
    if existing.contains(BEGIN) {
        let wanted = path_line(&bin)?;
        let current = block_body(&existing);
        if current.as_deref() == Some(wanted.as_str()) {
            return Ok(Step {
                name: "path",
                state: State::AlreadyDone,
                detail: format!("{} already has the semlith block", rc.display()),
            });
        }
        let dropped_key = current
            .as_deref()
            .is_some_and(|body| body.contains(KEY_ENV));
        replace_block(&rc, &existing, &wanted)?;
        return Ok(Step {
            name: "path",
            state: State::Done,
            detail: if dropped_key {
                format!(
                    "{} — replaced the block an older version wrote, which exported {KEY_ENV}. \
                     Nothing reads it now: a registered client launches `semlith mcp`, which reads \
                     the key from the file itself. Open a new shell to be rid of it.",
                    rc.display()
                )
            } else {
                format!(
                    "{} — replaced the block an older version wrote",
                    rc.display()
                )
            },
        });
    }

    let add = yes
        || cliclack::confirm(format!(
            "Add {} to PATH in {}?",
            bin.display(),
            rc.display()
        ))
        .initial_value(true)
        .interact()
        .unwrap_or(false);
    if !add {
        return Ok(Step {
            name: "path",
            state: State::Skipped,
            detail: format!("add {} to PATH by hand", bin.display()),
        });
    }

    // Built before the file is touched, so a home that cannot be written into a
    // shell file leaves the rc exactly as it was rather than half-edited.
    let line = path_line(&bin)?;

    if let Some(parent) = rc.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&rc)
        .with_context(|| format!("opening {}", rc.display()))?;
    let sep = if existing.is_empty() || existing.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    write!(file, "{sep}\n{}\n{}\n{END}\n", BEGIN, line)
        .with_context(|| format!("writing {}", rc.display()))?;

    Ok(Step {
        name: "path",
        state: State::Done,
        detail: rc.display().to_string(),
    })
}

/// fish is not POSIX and `export` is a syntax error in it.
/// What sits between the fences, or `None` when the file has no closing one.
///
/// Trimmed of the newlines the fences are written with, so it compares equal to
/// what [`path_line`] returns.
fn block_body(text: &str) -> Option<String> {
    let start = text.find(BEGIN)? + BEGIN.len();
    let end = text[start..].find(END)? + start;
    Some(text[start..end].trim_matches('\n').to_string())
}

/// Replace what is between the fences, leaving every byte outside them alone.
///
/// Through a neighbouring temporary file and a rename: a half-written rc file
/// is a shell that will not start, and this is somebody's login shell.
fn replace_block(rc: &Path, existing: &str, line: &str) -> Result<()> {
    let start = existing
        .find(BEGIN)
        .context("the block's opening fence went missing between reading and writing")?;
    let end = existing[start..]
        .find(END)
        .map(|at| start + at + END.len())
        .context("the semlith block has an opening fence and no closing one; fix it by hand")?;
    let next = format!(
        "{}{BEGIN}\n{line}\n{END}{}",
        &existing[..start],
        &existing[end..]
    );

    let temp = rc.with_extension("semlith-tmp");
    std::fs::write(&temp, &next).with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, rc).with_context(|| format!("replacing {}", rc.display()))?;
    Ok(())
}

fn path_line(bin: &Path) -> Result<String> {
    let path = bin.display().to_string();
    // A newline would end the line and start another one, which is a second
    // command in a file the shell runs at every start. There is no quoting that
    // survives it, so a store home containing one is refused rather than
    // written. A NUL cannot reach a shell at all.
    if path.contains('\n') || path.contains('\r') || path.contains('\0') {
        bail!(
            "{} contains a newline, so it cannot be written into a shell startup \
             file. Point {} at a directory whose name is one line and run \
             `semlith setup` again.",
            path.escape_debug(),
            home::HOME_ENV
        );
    }

    let is_fish = std::env::var("SHELL")
        .map(|s| s.ends_with("fish"))
        .unwrap_or(false);

    // One line: the bin directory on `PATH`.
    //
    // There used to be a second, exporting `SEMLITH_AGENT_KEY` by reading the
    // key file at every shell start, because every client stanza named the
    // variable. From 0.18.0 none of them does — a registration semlith writes
    // launches `semlith mcp`, which reads the key out of the file itself — so
    // the export is a credential placed in the environment of every process the
    // user starts, in exchange for nothing. A user who pastes one of the HTTP
    // stanzas by hand can export it themselves, and `docs/clients.md` says so.
    //
    // A block written by an earlier version still holds the export. `step_path`
    // replaces the whole block between its fences on every run, so running
    // `semlith setup` once removes it.
    Ok(if is_fish {
        format!("set -gx PATH {} $PATH", fish_quote(&path))
    } else {
        format!("export PATH={}:$PATH", posix_quote(&path))
    })
}

/// A POSIX shell word that is exactly this string.
///
/// Single quotes, because inside them every character is itself — no variable
/// expansion, no command substitution, no backslash escapes. The one character
/// that cannot appear inside them is a single quote, which is closed, escaped
/// and reopened in the usual way. It was double quotes before 0.14.0, which
/// expand `$(…)`, backticks and `$VAR`, so a directory name could run a command
/// on every shell start.
fn posix_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', r"'\''"))
}

/// The same for fish, which has no `'\''` idiom: inside single quotes it
/// recognises `\'` and `\\` and nothing else.
fn fish_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\\', r"\\").replace('\'', r"\'"))
}

/// A PowerShell single-quoted string. Inside them the only special character
/// is the single quote itself, which is written twice.
fn powershell_quote(raw: &str) -> String {
    format!("'{}'", raw.replace('\'', "''"))
}

/// Windows has no rc file; the user `PATH` is a registry value, and PowerShell
/// is the supported way to set one. `-NoProfile` keeps a user's own profile
/// from changing what this does, and `Bypass` covers the default
/// execution policy refusing to run anything at all.
fn step_path_windows(bin: &Path, yes: bool) -> Result<Step> {
    let add = yes
        || cliclack::confirm(format!("Add {} to your user PATH?", bin.display()))
            .initial_value(true)
            .interact()
            .unwrap_or(false);
    if !add {
        return Ok(Step {
            name: "path",
            state: State::Skipped,
            detail: format!("add {} to PATH by hand", bin.display()),
        });
    }

    let script = format!(
        "$dir = {}; \
         $cur = [Environment]::GetEnvironmentVariable('Path','User'); \
         if ($cur -notlike \"*$dir*\") {{ \
           [Environment]::SetEnvironmentVariable('Path', \"$dir;$cur\", 'User') }}",
        powershell_quote(&bin.display().to_string())
    );
    let out = Command::new("powershell")
        .args(["-ExecutionPolicy", "Bypass", "-NoProfile", "-Command"])
        .arg(&script)
        .output();

    match out {
        Ok(o) if o.status.success() => Ok(Step {
            name: "path",
            state: State::Done,
            detail: "user PATH".into(),
        }),
        _ => Ok(Step {
            name: "path",
            state: State::Failed,
            detail: format!(
                "could not set the user PATH — add {} through System Properties > \
                 Environment Variables",
                bin.display()
            ),
        }),
    }
}

/// Step 3. The weights are ~52 MB and the first `index` otherwise spends
/// minutes on them with nothing on screen. hf-hub draws the byte progress
/// itself when it is not told to be quiet, so this reuses the loader every
/// other command uses rather than plumbing a second progress path.
fn step_model(yes: bool, airgap: bool) -> Result<Step> {
    let cache = model_cache_dir()?;
    if embed::is_cached(&cache) {
        return Ok(Step {
            name: "model",
            state: State::AlreadyDone,
            detail: cache.display().to_string(),
        });
    }
    if airgap || embed::airgap() {
        return Ok(Step {
            name: "model",
            state: State::Skipped,
            detail: format!("--airgap; pre-seed {}", cache.display()),
        });
    }

    let fetch = yes
        || cliclack::confirm("Download the default embedding model now (~52 MB)?")
            .initial_value(true)
            .interact()
            .unwrap_or(false);
    if !fetch {
        return Ok(Step {
            name: "model",
            state: State::Skipped,
            detail: "the first `semlith index` will fetch it".into(),
        });
    }

    let _ = cliclack::log::step(format!("model: downloading into {}", cache.display()));
    // The model cache is weights, and weights are what every vector in every
    // store is computed by. A cache another account can write to is a model
    // another account chooses.
    home::secure_dir(&cache).with_context(|| format!("creating {}", cache.display()))?;
    match embed::Model::default().load(cache.clone(), chunk::MAX_CHARS / 2, false) {
        Ok(_) => Ok(Step {
            name: "model",
            state: State::Done,
            detail: cache.display().to_string(),
        }),
        Err(e) => Ok(Step {
            name: "model",
            state: State::Failed,
            detail: format!("{e} — `semlith setup` again when the network is back"),
        }),
    }
}

/// Step 4. Every client with a global registration CLI, registered by running
/// it.
///
/// Until 0.18.0 this step registered one client of twenty-seven and printed the
/// other twenty-six as stanzas to paste. Two things were wrong with that and
/// neither announced itself. Of the sixteen stanzas that are a command, exactly
/// one named a scope, so a user pasting any of the other fifteen while sitting
/// in a project registered semlith for that project alone and found it gone
/// from the next one. And every HTTP stanza named `${SEMLITH_AGENT_KEY}`, which
/// only expands if something exported it — so on a machine whose shell startup
/// file did not, or for a client launched from a desktop icon that reads no
/// startup file, the client registered, connected, and failed to authenticate
/// with nothing anywhere saying why.
///
/// Both are gone. Every registration is the stdio form, so there is no header
/// to expand and no key in any file; and every command carries the scope its
/// client spells "every project". A client semlith cannot register globally is
/// named as such rather than registered wrongly and counted as a success.
fn step_agents(register_all: bool, yes: bool) -> Result<Step> {
    let all = clients::clients();
    if all.is_empty() {
        return Ok(Step {
            name: "agents",
            state: State::Skipped,
            detail: "no documented clients".into(),
        });
    }

    let mut wired: Vec<&str> = Vec::new();
    let mut replaced: Vec<String> = Vec::new();
    let mut failed: Vec<(&str, String)> = Vec::new();
    let mut absent = 0usize;
    let mut by_file: Vec<&clients::Client> = Vec::new();
    let mut nowhere: Vec<&str> = Vec::new();

    for client in all {
        match register(client) {
            Registration::Registered { replaced: was } => {
                wired.push(&client.name);
                replaced.extend(was);
            }
            Registration::Absent => absent += 1,
            Registration::Failed { reason } => failed.push((&client.name, reason)),
            Registration::ProjectScoped => by_file.push(client),
            Registration::Unregisterable => nowhere.push(&client.name),
        }
    }
    // A client with no CLI at all: its file is the only way in, so it joins the
    // `--register-all` set rather than being reported as missing.
    let extra: Vec<&clients::Client> = all
        .iter()
        .filter(|client| client.needs_a_file_written())
        .filter(|client| !by_file.iter().any(|seen| seen.name == client.name))
        .collect();
    by_file.extend(extra);

    let written = if register_all {
        write_client_files(&by_file, yes)?
    } else {
        for client in &by_file {
            print_stanza(client);
        }
        Vec::new()
    };

    for (name, reason) in &failed {
        let client = all.iter().find(|c| &c.name == name);
        let _ = cliclack::log::warning(format!("{name} would not register: {reason}"));
        if let Some(client) = client {
            print_stanza(client);
        }
    }
    for name in &nowhere {
        let _ = cliclack::log::info(format!(
            "{name}: semlith cannot register this one — it documents no file outside a project. \
             Paste the stanza below."
        ));
        if let Some(client) = all.iter().find(|c| &c.name == name) {
            print_stanza(client);
        }
    }
    for was in &replaced {
        let _ = cliclack::log::info(format!("replaced an existing entry: {was}"));
    }

    let mut detail = Vec::new();
    if !wired.is_empty() {
        detail.push(format!("registered {}", wired.join(", ")));
    }
    if !written.is_empty() {
        detail.push(format!(
            "wrote {}",
            written
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if absent > 0 {
        detail.push(format!("{absent} client CLIs are not on this machine"));
    }
    if !by_file.is_empty() && !register_all {
        detail.push(format!(
            "{} need `semlith setup --register-all` or the printed stanza",
            by_file.len()
        ));
    }

    Ok(Step {
        name: "agents",
        state: if wired.is_empty() && written.is_empty() {
            State::Skipped
        } else {
            State::Done
        },
        detail: if detail.is_empty() {
            "no client CLI on this machine; stanzas printed".into()
        } else {
            detail.join("; ")
        },
    })
}

/// Write the user-level configuration file for every client semlith cannot ask
/// to register itself.
///
/// The paths are listed and confirmed before anything is written, because this
/// is the one place semlith writes a file it does not own. A user who says no
/// gets the stanzas printed, which is what they would have had anyway.
fn write_client_files(clients: &[&clients::Client], yes: bool) -> Result<Vec<PathBuf>> {
    let plans = crate::clientfile::plan(clients);
    if plans.is_empty() {
        return Ok(Vec::new());
    }
    let listing = plans
        .iter()
        .map(|plan| plan.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let _ = cliclack::note("Files semlith would write", listing);

    // `--yes --register-all` writes without asking. The flag is the asking:
    // a user who typed `--register-all` has already said the one thing this
    // prompt exists to get, and a script with nothing on stdin would otherwise
    // take the default and silently write nothing.
    let go = yes
        || cliclack::confirm("Write these files? Each is backed up beside itself first.")
            .initial_value(false)
            .interact()
            .unwrap_or(false);
    if !go {
        for client in clients {
            print_stanza(client);
        }
        return Ok(Vec::new());
    }

    let stanzas: Vec<(&str, &clients::Stanza)> = clients
        .iter()
        .flat_map(|client| {
            client
                .config_files()
                .map(move |stanza| (client.name.as_str(), stanza))
        })
        .collect();
    let written = crate::clientfile::apply(&plans, &stanzas)?;

    // A refusal is not a failure of the run: the other files are written and
    // the one that could not be is named, with its stanza, so the user can
    // finish it by hand.
    for plan in &plans {
        if let crate::clientfile::Action::Refuse { reason } = &plan.action {
            let _ = cliclack::log::warning(format!(
                "{}: {} was left alone — {reason}",
                plan.client,
                plan.path.display()
            ));
            if let Some(client) = clients.iter().find(|c| c.name == plan.client) {
                print_stanza(client);
            }
        }
    }
    Ok(written)
}

/// Print one client's stanzas for a user to paste.
///
/// The stdio form first, because it is what semlith would have registered and
/// it needs no key. The HTTP form follows for a daemon on another machine, with
/// the live key substituted — writing the placeholder would be handing someone
/// a configuration file to go and edit, which is the thing the persisted key
/// exists to stop.
fn print_stanza(client: &clients::Client) {
    let mut body = client
        .stanzas
        .iter()
        .filter(|stanza| !stanza.unregister)
        .map(|stanza| stanza.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if let Ok(key) = home::agent_key()
        && !key.is_empty()
    {
        let http = crate::clients::http_stanzas(&key);
        if !http.is_empty() {
            body.push_str("\n\nOr over HTTP, against a running `semlith start`:\n\n");
            body.push_str(
                &http
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
    }
    let _ = cliclack::note(format!("{} — {}", client.name, client.note), body);
}

/// What happened when semlith tried to register one client.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum Registration {
    /// Registered, naming whatever was replaced to get there.
    Registered { replaced: Vec<String> },
    /// The client's CLI is not on this machine. Not a failure: most people have
    /// two or three of the twenty-seven.
    Absent,
    /// The CLI is here and did not accept the registration. The stanza is
    /// printed instead and the install continues.
    Failed { reason: String },
    /// The CLI registers the directory it is run in, so semlith does not run
    /// it. `--register-all` writes this client's user-level file instead.
    ProjectScoped,
    /// No registration CLI and no documented user-level file: Crush, Zed and
    /// Roo Code. Nothing is broken; there is nowhere to write.
    Unregisterable,
}

/// Register one client by running its own CLI.
///
/// The exit status decides and no output is parsed. That contract was written
/// for `claude mcp add`, whose flags have changed between Claude Code versions,
/// and it holds harder across sixteen CLIs than it did across one: semlith
/// controls none of them, and a message it matched on today is a message
/// somebody rewords next release.
///
/// Any `unregister` command the client documents runs first, and its status is
/// ignored. That is how an existing entry is replaced rather than added beside:
/// `claude mcp add` refuses a name that is already registered, so an install
/// repairing a stale HTTP registration would otherwise fail on the machine that
/// most needs repairing. A client may document more than one — Claude Code has
/// three, one per scope — because the entry being replaced is often not at the
/// scope the new one goes to. That is the whole of the bug this release exists
/// to fix: an entry under `projects."…".mcpServers` that made semlith invisible
/// from every other directory.
pub fn register(client: &clients::Client) -> Registration {
    if !client.registers_globally() {
        return if client.config_files().next().is_some() {
            Registration::ProjectScoped
        } else {
            Registration::Unregisterable
        };
    }
    let Some(command) = client.register_command() else {
        return Registration::Unregisterable;
    };
    let Some((program, args)) = argv(&command) else {
        return Registration::Failed {
            reason: format!("{command:?} is not a command semlith could read"),
        };
    };

    // Absence is checked by trying, rather than by walking `PATH`: the answer
    // wanted is "can this process run it", and a `PATH` walk gets that wrong
    // for a shell function, an alias and a binary the user cannot execute.
    let mut replaced = Vec::new();
    for undo in client.unregister_commands() {
        let Some((program, args)) = argv(&undo) else {
            continue;
        };
        match Command::new(&program).args(&args).output() {
            // An exit of zero means there was something there to remove, which
            // is what the user is told. A non-zero exit is the ordinary case of
            // nothing being registered, and says nothing.
            Ok(out) if out.status.success() => replaced.push(undo),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Registration::Absent;
            }
            Err(_) => {}
        }
    }

    match Command::new(&program).args(&args).output() {
        Ok(out) if out.status.success() => Registration::Registered { replaced },
        Ok(out) => Registration::Failed {
            reason: first_line(&out.stderr, &out.stdout)
                .unwrap_or_else(|| format!("{program} exited {}", out.status)),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Registration::Absent,
        Err(e) => Registration::Failed {
            reason: e.to_string(),
        },
    }
}

/// Split a documented command into a program and its arguments, without a
/// shell.
///
/// `sh -c` would be four characters shorter and would hand every one of these
/// strings to a shell that expands `$`, `` ` `` and `~`. The commands come from
/// a document compiled into the binary rather than from a user, so that is not
/// an injection today — but a registration path that evaluates shell
/// metacharacters is one edit away from being one, and one of these commands is
/// a JSON object in single quotes, which a shell would be free to mangle.
///
/// Quotes group; a backslash escapes the character after it outside quotes.
/// `None` for an empty command or an unterminated quote, which is a
/// documentation error rather than something to guess at.
pub fn argv(command: &str) -> Option<(String, Vec<String>)> {
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (None, '\\') => word.push(chars.next()?),
            (None, c) if c.is_whitespace() => {
                if started || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, c) => word.push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    if started || !word.is_empty() {
        words.push(word);
    }
    let mut words = words.into_iter();
    let program = words.next()?;
    Some((program, words.collect()))
}

/// The first non-empty line a failing CLI printed, for the one-line report.
///
/// Not parsed for meaning — only shown, so the user reads what the client said
/// rather than what semlith guessed it meant.
fn first_line(stderr: &[u8], stdout: &[u8]) -> Option<String> {
    [stderr, stdout]
        .into_iter()
        .flat_map(|bytes| {
            String::from_utf8_lossy(bytes)
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .next()
        .map(|line| line.chars().take(200).collect())
}

/// The variable a client's configuration names instead of the key itself.
///
/// Nothing semlith registers names it from 0.18.0 — a registration is the stdio
/// form and `semlith mcp` reads the key out of `~/.semlith/agent.key` itself.
/// It stays because the HTTP stanzas stay: a daemon on another machine, or a
/// client that can only speak HTTP, still needs a header, and a user who pastes
/// one of those exports this themselves.
pub const KEY_ENV: &str = "SEMLITH_AGENT_KEY";

/// Step 5. Install the daemon as a login service.
///
/// On by default, and on even when nothing can answer a prompt. The argument
/// for asking is that a background process is a thing a user should consent to;
/// the argument against is the whole of this release. A client that starts
/// before any daemon finds nothing and says nothing, and the user who meets
/// that is precisely the one who ran a piped installer and never read a
/// question. `--no-service` opts out, `semlith start --no-service` undoes it,
/// and the step prints what it installed and how to remove it.
///
/// Points at the binary `step_binary` installed rather than the one running
/// this code, which during an install is the downloaded installer.
fn step_service(wanted: bool) -> Result<Step> {
    if !wanted {
        return Ok(Step {
            name: "service",
            state: State::Skipped,
            detail: "--no-service: start the daemon yourself with `semlith start`".to_string(),
        });
    }
    let target = home::bin_dir()?.join(exe_name());
    let binary = target.exists().then_some(target);
    match crate::service::install(binary.as_deref(), None) {
        Ok(status) if status.installed => Ok(Step {
            name: "service",
            state: State::Done,
            detail: if status.started_now {
                format!(
                    "running now and from every login ({}) — remove with `semlith start --no-service`",
                    status.mechanism,
                )
            } else {
                format!(
                    "a daemon is already listening, so this one starts at the next login ({}) — remove with `semlith start --no-service`",
                    status.mechanism,
                )
            },
        }),
        // Not fatal, and not silent. A machine with no systemd user session, or
        // a binary somewhere macOS will not run a background job from, still
        // gets a working semlith — it just has to be started by hand, and this
        // is the line that says so.
        Ok(status) => Ok(Step {
            name: "service",
            state: State::Failed,
            detail: format!(
                "{} did not report the service as installed — start the daemon with `semlith start`",
                status.mechanism,
            ),
        }),
        Err(e) => Ok(Step {
            name: "service",
            state: State::Failed,
            detail: format!("{e} — start the daemon with `semlith start`"),
        }),
    }
}

/// Step 6. Runs the binary that was just installed, not this process, because
/// the question is whether the installed one works.
fn step_verify() -> Result<Step> {
    let target = home::bin_dir()?.join(exe_name());
    let which = if target.exists() {
        target
    } else {
        std::env::current_exe().context("locating the running binary")?
    };
    match Command::new(&which).arg("--version").output() {
        Ok(o) if o.status.success() => {
            let line = String::from_utf8_lossy(&o.stdout).trim().to_string();
            Ok(Step {
                name: "verify",
                state: State::AlreadyDone,
                detail: line,
            })
        }
        _ => Ok(Step {
            name: "verify",
            state: State::Failed,
            detail: format!("{} would not run", which.display()),
        }),
    }
}

// ---------------------------------------------------------------- rotation

/// Carry a rotated agent key into the client configuration files that already
/// hold the old one.
///
/// Rotation used to leave every configured client authenticating with a key
/// the daemon had stopped accepting, and the only repair was to open each file
/// and paste. This edits the files that already carry the old key — nothing
/// else. A file is rewritten only when the exact 68-character key appears in
/// it, so a path that happens to exist but names a different server is left
/// alone, and no file is ever created.
///
/// Returns the files that changed, in the order they were tried.
pub fn recarry_key(previous: &str, fresh: &str) -> Vec<PathBuf> {
    let mut done = Vec::new();
    if previous == fresh || !home::is_agent_key(previous) || !home::is_agent_key(fresh) {
        return done;
    }
    for path in client_configs() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !text.contains(previous) {
            continue;
        }
        // Written through a neighbouring temporary file: a half-written
        // `settings.json` is a client that will not start, and this runs
        // across a dozen of them.
        let swapped = text.replace(previous, fresh);
        let temp = path.with_extension(format!(
            "{}semlith-tmp",
            path.extension()
                .map(|e| format!("{}.", e.to_string_lossy()))
                .unwrap_or_default()
        ));
        // The temp file is created with the original's mode, not with the
        // process umask. A config file somebody had locked down to 0600 came
        // back as 0644 after a rotation, because the rename carried the new
        // file's permissions with it — a rotation is meant to reduce what a
        // credential is exposed to, not widen it. 0600 when the original's
        // mode cannot be read, because guessing narrow is the safe direction.
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(&path)
                .map(|m| m.permissions().mode() & 0o777)
                .unwrap_or(0o600)
        };
        let written = (|| -> std::io::Result<()> {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(mode);
            }
            let mut file = options.open(&temp)?;
            file.write_all(swapped.as_bytes())
        })();
        if written.is_ok() && std::fs::rename(&temp, &path).is_ok() {
            done.push(path);
        } else {
            let _ = std::fs::remove_file(&temp);
        }
    }
    done
}

/// Where the clients the README documents keep their configuration.
///
/// The user-level file for each, plus the project-level ones relative to
/// wherever the daemon was started. It mirrors the paths in the README's
/// client section; a path that is absent is simply skipped, so listing one
/// costs nothing and missing one costs a manual edit after a rotation.
fn client_configs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(home_dir) = home::user_home() {
        for rest in [
            // Claude Code writes the user scope here, and `claude mcp add`
            // does too.
            ".claude.json",
            ".codex/config.toml",
            ".config/opencode/opencode.json",
            ".copilot/mcp-config.json",
            ".gemini/settings.json",
            ".qwen/settings.json",
            ".config/amp/settings.json",
            ".factory/mcp.json",
            ".config/goose/config.yaml",
            ".aws/amazonq/mcp.json",
            ".openclaw/openclaw.json",
            ".codewhale/mcp.json",
            ".deepseek/mcp.json",
            ".warp/.mcp.json",
            ".cursor/mcp.json",
            ".codeium/windsurf/mcp_config.json",
            ".config/zed/settings.json",
            ".junie/mcp/mcp.json",
            ".kiro/settings/mcp.json",
            ".lmstudio/mcp.json",
            ".cache/lm-studio/mcp.json",
            ".config/kilo/kilo.jsonc",
            ".continue/config.yaml",
            // The editors that keep their MCP file inside a per-platform
            // application-support directory.
            "Library/Application Support/Claude/claude_desktop_config.json",
            "Library/Application Support/Code/User/mcp.json",
            ".config/Code/User/mcp.json",
        ] {
            out.push(home_dir.join(rest));
        }
        // Continue keeps one file per server rather than one file.
        if let Ok(entries) = std::fs::read_dir(home_dir.join(".continue/mcpServers")) {
            out.extend(entries.flatten().map(|e| e.path()));
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        out.push(appdata.join("Claude/claude_desktop_config.json"));
        out.push(appdata.join("Code/User/mcp.json"));
    }
    // The project-scoped files, against wherever this process was started.
    if let Ok(here) = std::env::current_dir() {
        for rest in [
            ".mcp.json",
            ".vscode/mcp.json",
            ".cursor/mcp.json",
            ".roo/mcp.json",
            ".kilocode/mcp.json",
            ".kiro/settings/mcp.json",
            ".junie/mcp/mcp.json",
            ".factory/mcp.json",
            ".amazonq/mcp.json",
            "opencode.json",
            "crush.json",
            "io.local.toml",
        ] {
            out.push(here.join(rest));
        }
    }
    out
}

#[cfg(test)]
mod rotation_tests {
    use super::*;

    /// The whole point: a file that carries the old key is rewritten, and a
    /// file that does not is not touched at all.
    #[test]
    fn only_the_files_holding_the_old_key_are_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let home_dir = dir.path().join("home");
        std::fs::create_dir_all(home_dir.join(".gemini")).unwrap();
        std::fs::create_dir_all(home_dir.join(".cursor")).unwrap();

        let previous = format!("sml_{}", "a".repeat(64));
        let fresh = format!("sml_{}", "b".repeat(64));

        let carries = home_dir.join(".gemini/settings.json");
        std::fs::write(
            &carries,
            format!("{{\"mcpServers\":{{\"semlith\":{{\"headers\":{{\"Authorization\":\"Bearer {previous}\"}}}}}}}}"),
        )
        .unwrap();
        let untouched = home_dir.join(".cursor/mcp.json");
        std::fs::write(&untouched, "{\"mcpServers\":{\"other\":{}}}").unwrap();
        let before = std::fs::read_to_string(&untouched).unwrap();

        // `HOME` is process-wide, so this test is serialised with the others
        // that set it by living in its own module and setting it back.
        let was = std::env::var_os(home::HOME_VAR);
        unsafe { std::env::set_var(home::HOME_VAR, &home_dir) };
        let changed = recarry_key(&previous, &fresh);
        match was {
            Some(v) => unsafe { std::env::set_var(home::HOME_VAR, v) },
            None => unsafe { std::env::remove_var(home::HOME_VAR) },
        }

        assert_eq!(changed, vec![carries.clone()], "{changed:?}");
        let text = std::fs::read_to_string(&carries).unwrap();
        assert!(
            text.contains(&fresh),
            "the new key is not in the file: {text}"
        );
        assert!(!text.contains(&previous), "the old key survived: {text}");
        assert_eq!(std::fs::read_to_string(&untouched).unwrap(), before);
        assert!(
            !carries.with_extension("json.semlith-tmp").exists(),
            "the temporary file was left behind"
        );
    }

    /// Rotating to the same key, or passing something that is not a key, does
    /// nothing rather than rewriting every file with a placeholder.
    #[test]
    fn a_non_rotation_changes_nothing() {
        let key = format!("sml_{}", "c".repeat(64));
        assert!(recarry_key(&key, &key).is_empty());
        assert!(recarry_key("sml_YOURKEY", &key).is_empty());
        assert!(recarry_key(&key, "").is_empty());
    }
}

#[cfg(test)]
mod quoting_tests {
    use super::*;

    /// Single quotes are the only POSIX construct in which every character is
    /// itself. What it has to survive is a directory name, which can hold any
    /// byte but a slash and a NUL.
    #[test]
    fn a_posix_word_is_exactly_the_path() {
        assert_eq!(
            posix_quote("/home/me/.semlith/bin"),
            "'/home/me/.semlith/bin'"
        );
        assert_eq!(posix_quote("with space"), "'with space'");
        assert_eq!(posix_quote("$(touch x)"), "'$(touch x)'");
        assert_eq!(posix_quote("`touch x`"), "'`touch x`'");
        assert_eq!(posix_quote("${HOME}"), "'${HOME}'");
        assert_eq!(posix_quote(r#"a"b"#), r#"'a"b'"#);
        assert_eq!(posix_quote(r"back\slash"), r"'back\slash'");
        // The one character that cannot appear inside single quotes.
        assert_eq!(posix_quote("it's"), r"'it'\''s'");
    }

    /// fish has no `'\''` idiom: inside single quotes it recognises a
    /// backslash escape, so the backslash itself has to be escaped first.
    #[test]
    fn a_fish_word_escapes_what_fish_reads() {
        assert_eq!(fish_quote("/home/me/bin"), "'/home/me/bin'");
        assert_eq!(fish_quote("it's"), r"'it\'s'");
        assert_eq!(fish_quote(r"back\slash"), r"'back\\slash'");
        // Order matters: escaping the quote first would then escape its own
        // backslash and end the string early.
        assert_eq!(fish_quote(r"\'"), r"'\\\''");
    }

    /// PowerShell doubles the quote and treats nothing else as special inside
    /// single quotes — `$dir` in the script is why this matters at all.
    #[test]
    fn a_powershell_word_doubles_its_quotes() {
        assert_eq!(powershell_quote(r"C:\Users\me\bin"), r"'C:\Users\me\bin'");
        assert_eq!(powershell_quote("it's"), "'it''s'");
        assert_eq!(powershell_quote("$(pwd)"), "'$(pwd)'");
    }
}
