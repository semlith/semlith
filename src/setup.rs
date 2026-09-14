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
    /// `None` when the `claude` CLI is not on `PATH`, so the page can say
    /// "not checked" rather than "not registered".
    pub claude_registered: Option<bool>,
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
    let bin = home::bin_dir();
    let rc = rc_file();
    let block = rc
        .as_deref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| text.contains(BEGIN))
        .unwrap_or(false);
    let cache = model_cache_dir();

    // Each of these is asked once and then reused. `claude_registered` spawns
    // `claude mcp list` and waits for it, and this function used to call it
    // four times — twice for the agents step alone, once for its state and
    // once for its detail. On a machine where that CLI is slow it turned the
    // portal's Agents page into a twenty-second wait that looked like a hang.
    let registered = claude_registered();
    let installed = bin.join(exe_name()).exists();
    let path_has_bin = on_path(&bin);
    let model_cached = embed::is_cached(&cache);

    let steps = vec![
        Step {
            name: "binary",
            state: if installed {
                State::AlreadyDone
            } else {
                State::Skipped
            },
            detail: bin.join(exe_name()).display().to_string(),
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
            state: match registered {
                Some(true) => State::AlreadyDone,
                Some(false) | None => State::Skipped,
            },
            detail: match registered {
                Some(true) => "Claude Code has the semlith server".into(),
                Some(false) => "Claude Code is installed but has no semlith server".into(),
                None => "the claude CLI is not on PATH".into(),
            },
        },
    ];

    Status {
        bin_dir: bin.display().to_string(),
        binary_installed: installed,
        on_path: path_has_bin,
        rc_file: rc.map(|p| p.display().to_string()),
        rc_block_present: block,
        model_cache: cache.display().to_string(),
        model_cached,
        claude_registered: registered,
        version: env!("CARGO_PKG_VERSION"),
        install_sh: INSTALL_SH,
        install_ps1: INSTALL_PS1,
        steps,
    }
}

/// The guided flow. `yes` takes the default at every prompt — add to `PATH`,
/// download the model, register no agent — so a script or an agent can install
/// semlith with nothing attached to stdin.
pub fn run(yes: bool, airgap: bool) -> Result<()> {
    let _ = cliclack::intro(format!(" semlith {} setup ", env!("CARGO_PKG_VERSION")));

    // Reported as they run rather than replayed at the end, so the line about
    // a step is next to whatever that step printed.
    let report = [
        announce(step_binary(yes)?),
        announce(step_path(yes)?),
        announce(step_model(yes, airgap)?),
        announce(step_agents(yes)?),
        announce(step_verify()?),
    ];

    if report.iter().all(|s| s.state != State::Done) {
        let _ = cliclack::log::success("Everything was already set up. Nothing changed.");
    }

    let _ = cliclack::outro(format!(
        "Next: `semlith index .` to index this directory, then `semlith start` \
         for the portal on http://127.0.0.1:7365\n  Open a new shell first if \
         {} was only just added to PATH.",
        home::bin_dir().display()
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
    let bin = home::bin_dir();
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
    let base = PathBuf::from(std::env::var_os("HOME")?);
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
    let bin = home::bin_dir();
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
    if existing.contains(BEGIN) {
        return Ok(Step {
            name: "path",
            state: State::AlreadyDone,
            detail: format!("{} already has the semlith block", rc.display()),
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
    let key = home::agent_key_path().display().to_string();
    if key.contains('\n') || key.contains('\r') || key.contains('\0') {
        bail!(
            "the agent key path contains a newline, so it cannot be written into a shell startup file"
        );
    }

    // Two lines: the bin directory on PATH, and the agent key in the
    // environment. The key is exported by *reading the file* at shell start
    // rather than by being written into this file, so the credential lives in
    // one place with one set of permissions, and rotating it needs nothing
    // rewritten — every client stanza names the variable.
    Ok(if is_fish {
        format!(
            "set -gx PATH {} $PATH\n\
             test -r {key_q}; and set -gx {KEY_ENV} (cat {key_q})",
            fish_quote(&path),
            key_q = fish_quote(&key),
        )
    } else {
        format!(
            "export PATH={}:$PATH\n\
             [ -r {key_q} ] && export {KEY_ENV}=\"$(cat {key_q})\"",
            posix_quote(&path),
            key_q = posix_quote(&key),
        )
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
    let cache = model_cache_dir();
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

/// Step 4. Claude Code is registered by running its own CLI, because that is
/// the only client whose config file location is stable enough to write to.
/// Every other client gets the stanza and the path printed, which is what the
/// README documents and `tests/clients.rs` executes.
fn step_agents(yes: bool) -> Result<Step> {
    if yes {
        return Ok(Step {
            name: "agents",
            state: State::Skipped,
            detail: "--yes registers no agent; run `semlith setup` to pick one".into(),
        });
    }

    let all = clients::clients();
    if all.is_empty() {
        return Ok(Step {
            name: "agents",
            state: State::Skipped,
            detail: "no documented clients".into(),
        });
    }

    let mut prompt = cliclack::multiselect::<usize>("Which agents do you use? (space to pick)");
    for (i, client) in all.iter().enumerate() {
        prompt = prompt.item(i, &client.name, "");
    }
    let picked = prompt.required(false).interact().unwrap_or_default();
    if picked.is_empty() {
        return Ok(Step {
            name: "agents",
            state: State::Skipped,
            detail: "none picked".into(),
        });
    }

    // The HTTP form carries the live agent key, so a stanza this prints is one
    // that connects. Writing the placeholder would be handing someone a
    // configuration file to go and edit, which is the thing the persisted key
    // exists to stop.
    let key = home::agent_key().unwrap_or_default();
    let http = crate::clients::http_stanzas(&key);

    let mut wired = Vec::new();
    for i in picked {
        let client = &all[i];
        if client.name.eq_ignore_ascii_case("Claude Code") && register_claude() {
            wired.push(client.name.clone());
            continue;
        }
        let mut body = client
            .stanzas
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        if !key.is_empty() {
            body.push_str("\n\nOr over HTTP, against a running `semlith start`:\n\n");
            body.push_str(
                &http
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        let _ = cliclack::note(format!("{} — {}", client.name, client.note), body);
    }

    Ok(Step {
        name: "agents",
        state: if wired.is_empty() {
            State::Skipped
        } else {
            State::Done
        },
        detail: if wired.is_empty() {
            "stanzas printed; paste them into each client's config".into()
        } else {
            wired.join(", ")
        },
    })
}

/// `claude mcp add` has changed flags between Claude Code versions, so its exit
/// status decides and nothing here parses its output. A non-zero exit falls
/// back to the printed stanza; it never fails the install.
pub fn register_claude() -> bool {
    Command::new("claude")
        .args([
            "mcp", "add", "--scope", "user", "semlith", "--", "semlith", "mcp",
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Re-register Claude Code against the HTTP endpoint with the current key.
///
/// This is the one client semlith writes a configuration for, because it has a
/// CLI for it. Every other client is named and left to the user — a tool that
/// edits a file it does not own is a tool that eventually corrupts one.
pub fn register_claude_http(url: &str) -> bool {
    // Replaced rather than added beside: `claude mcp add` refuses a name that
    // is already registered, so an existing entry is removed first. A missing
    // one makes the remove fail, which is fine and is why its status is
    // ignored.
    let _ = Command::new("claude")
        .args(["mcp", "remove", "--scope", "user", "semlith"])
        .output();
    Command::new("claude")
        .args([
            "mcp",
            "add",
            "--scope",
            "user",
            "--transport",
            "http",
            "semlith",
            url,
            "--header",
            // The name of the variable, not the key. Every argument of every
            // process on this machine is readable by every other process the
            // user owns, and `ps` during a registration used to show the
            // credential that opens the MCP endpoint. The client expands this
            // itself, from the variable the rc block exports, so the config
            // file never carries the key either — and rotating it needs no
            // config rewritten at all.
            &format!("Authorization: Bearer ${{{KEY_ENV}}}"),
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The variable a client's configuration names instead of the key itself.
pub const KEY_ENV: &str = "SEMLITH_AGENT_KEY";

/// Whether the Claude Code CLI is on this machine at all.
pub fn claude_present() -> bool {
    Command::new("claude")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `None` when the CLI is absent, so "not checked" and "not registered" stay
/// different answers.
fn claude_registered() -> Option<bool> {
    let out = Command::new("claude").args(["mcp", "list"]).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).contains("semlith"))
}

/// Step 5. Runs the binary that was just installed, not this process, because
/// the question is whether the installed one works.
fn step_verify() -> Result<Step> {
    let target = home::bin_dir().join(exe_name());
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
    if let Some(home_dir) = std::env::var_os("HOME").map(PathBuf::from) {
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
        let was = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", &home_dir) };
        let changed = recarry_key(&previous, &fresh);
        match was {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
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
