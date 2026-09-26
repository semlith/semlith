//! The daemon as a login service, so semlith is answering before a client asks.
//!
//! A server that is silently absent does not exist. Until 0.21.0 the daemon ran
//! only while somebody held a terminal open for it: a client that started first
//! found nothing, and nothing said why. This installs it as the thing each
//! platform already has for "run this when I log in, and keep it running" — a
//! launchd user agent on macOS, a systemd user unit on Linux, a logon task on
//! Windows.
//!
//! One public surface, three `#[cfg]` bodies and a fallback, which is the shape
//! `system.rs` already uses for the same reason: the callers — `semlith start
//! --service`, `semlith setup`, `semlith doctor` and the portal's Agents page —
//! ask the same four questions on every platform and none of them should know
//! which mechanism answered.
//!
//! **The three are not equally strong, and this module does not pretend they
//! are.** launchd's `KeepAlive` and systemd's `Restart=always` restart a daemon
//! that exits, within seconds, with no user present. A Windows logon task is
//! registered with a restart count and interval, which covers a task that
//! *fails*; it is a weaker guarantee than the other two and [`Status::restarts`]
//! says so rather than letting a caller assume parity.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// What this platform calls the service, in its own registry.
pub const LABEL: &str = "com.semlith.daemon";

/// Whether the login service is installed, and where to look when it misbehaves.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Status {
    /// Whether a service is registered for this user.
    pub installed: bool,
    /// The mechanism that answered — `launchd`, `systemd`, `schtasks`, or
    /// `none` on a platform semlith has no login service for. Named rather
    /// than inferred from the operating system, because a caller that has to
    /// guess is a caller that will guess wrong on the next platform.
    pub mechanism: &'static str,
    /// The unit, plist or task definition, when the mechanism has a file.
    pub definition: Option<PathBuf>,
    /// Where this service's own output goes, so a failing service is readable
    /// without knowing launchd, systemd or Task Scheduler.
    pub log: Option<PathBuf>,
    /// Whether the mechanism restarts a daemon that exits without a person
    /// present. False on Windows, where a logon task restarts a task that
    /// failed but does not supervise one that ended cleanly.
    pub restarts: bool,
    /// Whether this install started a daemon now, as opposed to only
    /// registering one for the next login because something was already
    /// listening on the port.
    pub started_now: bool,
}

impl Status {
    /// A service that is not there, and the base every platform body builds on.
    ///
    /// The three bodies are each compiled on one operating system only, so a
    /// field added to `Status` and filled in on two of them breaks the third on
    /// a machine the author cannot run — which is exactly how a Windows-only
    /// compile error reached CI during this release. Each body now overrides
    /// what it knows and takes the rest from here, so a new field is additive
    /// on every platform by construction rather than by remembering.
    fn absent(mechanism: &'static str) -> Self {
        Self {
            installed: false,
            mechanism,
            definition: None,
            log: None,
            restarts: false,
            started_now: false,
        }
    }
}

/// Where the service's output goes.
///
/// Under the semlith home rather than the platform's own log directory, so one
/// path is right on all three and `doctor` can print it without a `cfg`.
pub fn log_path() -> Result<PathBuf> {
    let dir = crate::home::home_or_error()?.join("logs");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    crate::home::tighten_dir(&dir);
    Ok(dir.join("daemon.log"))
}

/// The binary a service definition should name.
///
/// Resolved, never the bare command: a service manager starts a process with
/// its own environment, and the PATH a login shell builds is not the PATH
/// launchd, systemd or Task Scheduler hands a job. That is the same mistake
/// registration made until 0.21.0, and it is worth making only once.
pub fn exe() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("finding this binary")?;
    Ok(plain(exe.canonicalize().unwrap_or(exe)))
}

/// A path in the spelling a person and a service manager both accept.
///
/// `canonicalize` on Windows returns the verbatim form, `\\?\C:\…`, which is a
/// legal path for the API and an illegal-looking one everywhere a human reads
/// it — and which several tools refuse outright. Compare canonical, emit plain.
fn plain(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\UNC\") {
        Some(rest) => PathBuf::from(format!(r"\\{rest}")),
        None => match text.strip_prefix(r"\\?\") {
            Some(rest) => PathBuf::from(rest),
            None => path,
        },
    }
}

// ------------------------------------------------------------------- macOS

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    pub const MECHANISM: &str = "launchd";

    /// What the plist asks launchd for. See `install`.
    pub const PROCESS_TYPE: &str = "Standard";

    /// Whether an installed plist still asks for the efficiency cores, which
    /// every release before 0.28.0 wrote. `setup` and `upgrade` rewrite it;
    /// `doctor` names it until they have.
    pub fn stale_definition() -> Option<String> {
        let path = plist_path().ok()?;
        let text = std::fs::read_to_string(&path).ok()?;
        text.contains("<string>Background</string>").then(|| {
            format!(
                "{} asks launchd for ProcessType Background, which holds the daemon on the \
                 efficiency cores; run `semlith setup` (or `semlith start --service`) to rewrite it",
                path.display()
            )
        })
    }

    fn plist_path() -> Result<PathBuf> {
        Ok(crate::home::user_home()?
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{LABEL}.plist")))
    }

    pub fn status() -> Status {
        let Ok(path) = plist_path() else {
            return Status::absent(MECHANISM);
        };
        // The file is the definition; whether launchd knows the job is whether
        // anything is supervising it. A headless session — a CI runner, a
        // machine reached only over ssh — has no `gui/<uid>` domain, so
        // `bootstrap` fails, the `load -w` fallback starts the daemon once, and
        // nothing restarts it when it dies. The plist is still on disk, and
        // reporting that as an installed login service is the same claim
        // without the thing it claims. Measured on a GitHub macOS runner, where
        // the daemon ran, was killed, and never came back.
        let known = path.is_file()
            && run(
                "launchctl",
                &[
                    "print",
                    &format!("gui/{}/{LABEL}", unsafe { libc::getuid() }),
                ],
            )
            .is_ok();
        Status {
            installed: known,
            definition: path.is_file().then_some(path),
            log: log_path().ok(),
            restarts: true,
            ..Status::absent(MECHANISM)
        }
    }

    pub fn install(exe: &Path, port: Option<u16>) -> Result<Status> {
        let path = plist_path()?;
        let dir = path
            .parent()
            .expect("a plist has a directory")
            .to_path_buf();
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let log = log_path()?;

        let mut args = vec![exe.display().to_string(), "start".to_string()];
        if let Some(port) = port {
            args.push("--port".to_string());
            args.push(port.to_string());
        }
        let program: String = args
            .iter()
            .map(|a| format!("    <string>{}</string>\n", escape(a)))
            .collect();

        // KeepAlive, not RunAtLoad alone: the whole claim is that a daemon
        // which exits comes back without a person present.
        //
        // `Standard`, not `Background`. A job launchd starts as `Background`
        // is held on the efficiency cores and cannot lift itself out: on the
        // reference M1 it indexed at 3.3 chunks/s against 28 in a terminal,
        // and `setpriority` from inside it returned 0 and changed nothing.
        // The daemon puts itself in background state while it is idle
        // instead (`priority::manage`), which it can undo in microseconds.
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             <key>Label</key>\n  <string>{label}</string>\n\
             <key>ProgramArguments</key>\n  <array>\n{program}  </array>\n\
             <key>RunAtLoad</key>\n  <true/>\n\
             <key>KeepAlive</key>\n  <true/>\n\
             <key>ProcessType</key>\n  <string>{process_type}</string>\n\
             <key>StandardOutPath</key>\n  <string>{log}</string>\n\
             <key>StandardErrorPath</key>\n  <string>{log}</string>\n\
             </dict>\n\
             </plist>\n",
            label = LABEL,
            process_type = PROCESS_TYPE,
            log = escape(&log.display().to_string()),
        );
        std::fs::write(&path, plist).with_context(|| format!("writing {}", path.display()))?;

        // Unload first so a re-install replaces the definition rather than
        // leaving the old one running against the new file. Both are allowed
        // to fail: the first because there may be nothing loaded, the second
        // because `bootstrap` is the modern spelling and `load` the one that
        // works on older systems.
        let domain = format!("gui/{}", unsafe { libc::getuid() });
        let _ = run("launchctl", &["bootout", &format!("{domain}/{LABEL}")]);
        if run(
            "launchctl",
            &["bootstrap", &domain, &path.display().to_string()],
        )
        .is_err()
        {
            run("launchctl", &["load", "-w", &path.display().to_string()])
                .context("loading the launchd agent")?;
        }
        Ok(status())
    }

    pub fn remove() -> Result<bool> {
        let path = plist_path()?;
        let domain = format!("gui/{}", unsafe { libc::getuid() });
        let _ = run("launchctl", &["bootout", &format!("{domain}/{LABEL}")]);
        let _ = run("launchctl", &["unload", "-w", &path.display().to_string()]);
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        Ok(true)
    }

    /// XML, not a shell. A home directory with an ampersand in it is rare and
    /// a plist that silently fails to parse is not worth the odds.
    fn escape(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
}

// ------------------------------------------------------------------- Linux

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    pub const MECHANISM: &str = "systemd";
    const UNIT: &str = "semlith.service";

    /// Nothing to rewrite: the unit has always run at normal priority.
    pub fn stale_definition() -> Option<String> {
        None
    }

    fn unit_path() -> Result<PathBuf> {
        Ok(crate::home::user_home()?
            .join(".config")
            .join("systemd")
            .join("user")
            .join(UNIT))
    }

    pub fn status() -> Status {
        let Ok(path) = unit_path() else {
            return Status::absent(MECHANISM);
        };
        // The file is the definition; `is-enabled` is whether systemd agreed to
        // it. A unit file present and not enabled is a half-install, and
        // reporting it as installed is how a user ends up with no daemon and a
        // green row.
        let enabled = run("systemctl", &["--user", "is-enabled", UNIT])
            .map(|out| out.trim() == "enabled")
            .unwrap_or(false);
        Status {
            installed: path.is_file() && enabled,
            definition: path.is_file().then_some(path),
            log: log_path().ok(),
            restarts: true,
            ..Status::absent(MECHANISM)
        }
    }

    pub fn install(exe: &Path, port: Option<u16>) -> Result<Status> {
        let path = unit_path()?;
        let dir = path.parent().expect("a unit has a directory").to_path_buf();
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

        let port = port.map(|p| format!(" --port {p}")).unwrap_or_default();
        let unit = format!(
            "[Unit]\n\
             Description=semlith — local semantic index for agent clients\n\
             Documentation=https://github.com/semlith/semlith\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart={exe}{port}\n\
             Restart=always\n\
             RestartSec=2\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            exe = exe.display(),
        );
        std::fs::write(&path, unit).with_context(|| format!("writing {}", path.display()))?;

        run("systemctl", &["--user", "daemon-reload"]).context("reloading systemd --user")?;
        run("systemctl", &["--user", "enable", "--now", UNIT])
            .context("enabling the systemd user unit")?;
        Ok(status())
    }

    pub fn remove() -> Result<bool> {
        let path = unit_path()?;
        let _ = run("systemctl", &["--user", "disable", "--now", UNIT]);
        let existed = path.exists();
        if existed {
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
        let _ = run("systemctl", &["--user", "daemon-reload"]);
        Ok(existed)
    }
}

// ----------------------------------------------------------------- Windows

#[cfg(windows)]
mod platform {
    use super::*;

    pub const MECHANISM: &str = "schtasks";
    const TASK: &str = "semlith";

    /// Whether the logon task still runs at Task Scheduler's default of 7
    /// (below normal), which every release before 0.28.0 registered, and at
    /// which Windows 11 may apply EcoQoS to the whole process.
    pub fn stale_definition() -> Option<String> {
        let priority = powershell(&format!(
            "(Get-ScheduledTask -TaskName '{TASK}' -ErrorAction Stop).Settings.Priority"
        ))
        .ok()?;
        let priority = priority.trim();
        (priority != "5").then(|| {
            format!(
                "the {TASK} logon task runs at priority {priority} (below normal); run \
                 `semlith setup` (or `semlith start --service`) to register it at 5"
            )
        })
    }

    pub fn status() -> Status {
        let installed = run("schtasks", &["/Query", "/TN", TASK]).is_ok();
        Status {
            installed,
            log: log_path().ok(),
            // `restarts` and `started_now` are both false here and both come
            // from `absent`. A logon task restarts a task that *failed*; it
            // does not supervise one that ended cleanly, the way launchd's
            // KeepAlive and systemd's Restart=always do — said rather than
            // left for a caller to assume parity it does not have.
            ..Status::absent(MECHANISM)
        }
    }

    pub fn install(exe: &Path, port: Option<u16>) -> Result<Status> {
        let port = port.map(|p| format!(" --port {p}")).unwrap_or_default();
        // `-Priority 5` is normal. Without it Task Scheduler starts the task at
        // 7, below normal, and Windows 11 may then apply EcoQoS, which on a
        // hybrid CPU means the efficiency cores. The daemon lowers itself while
        // idle (`priority::manage`), so asking for normal here costs nothing.
        //
        // PowerShell rather than `schtasks /Create`, for one reason: the
        // restart settings. `schtasks` has no flag for "try again if this
        // fails", and a logon task without one is a service that gives up the
        // first time the port is busy.
        let script = format!(
            "$ErrorActionPreference='Stop'; \
             $action = New-ScheduledTaskAction -Execute '{exe}' -Argument 'start{port}'; \
             $trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME; \
             $settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries \
             -DontStopIfGoingOnBatteries -Priority 5 -RestartCount 3 \
             -RestartInterval (New-TimeSpan -Minutes 1) \
             -ExecutionTimeLimit (New-TimeSpan -Seconds 0); \
             Register-ScheduledTask -TaskName '{task}' -Action $action -Trigger $trigger \
             -Settings $settings -Force | Out-Null; \
             Start-ScheduledTask -TaskName '{task}'",
            exe = exe.display(),
            task = TASK,
        );
        powershell(&script).context("registering the logon task")?;
        Ok(status())
    }

    pub fn remove() -> Result<bool> {
        if run("schtasks", &["/Query", "/TN", TASK]).is_err() {
            return Ok(false);
        }
        run("schtasks", &["/Delete", "/TN", TASK, "/F"]).context("deleting the logon task")?;
        Ok(true)
    }

    fn powershell(script: &str) -> Result<String> {
        // `powershell` rather than `pwsh`: Windows Server and a stock Windows
        // 11 have the first and may not have the second, and this has to work
        // on the machine as it is rather than on a developer's.
        run(
            "powershell",
            &["-NoProfile", "-NonInteractive", "-Command", script],
        )
    }
}

// ------------------------------------------------------- every other target

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod platform {
    use super::*;

    pub const MECHANISM: &str = "none";

    pub fn stale_definition() -> Option<String> {
        None
    }

    pub fn status() -> Status {
        Status::absent(MECHANISM)
    }

    pub fn install(_exe: &Path, _port: Option<u16>) -> Result<Status> {
        anyhow::bail!(
            "semlith has no login service for this platform — \
             run `semlith start` from whatever this system uses to start a user service"
        )
    }

    pub fn remove() -> Result<bool> {
        Ok(false)
    }
}

/// Run a command and return its stdout, or an error carrying what it said.
///
/// Service managers report the interesting half on stderr and exit non-zero,
/// so a caller that only looked at the status gets "it failed" and nothing a
/// person can act on.
fn run(program: &str, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("running {program}"))?;
    if !out.status.success() {
        let said = String::from_utf8_lossy(&out.stderr);
        let said = if said.trim().is_empty() {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        } else {
            said.trim().to_string()
        };
        anyhow::bail!("{program} {}: {said}", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Why the installed login service needs rewriting, if it does.
///
/// Every release before 0.28.0 asked for background priority — launchd's
/// `ProcessType Background` on macOS, Task Scheduler's default of 7 on
/// Windows — and a daemon started that way runs its whole index on the
/// efficiency cores. `setup` and `upgrade` rewrite such a definition, and
/// `doctor` names it until one of them has.
pub fn stale_definition() -> Option<String> {
    platform::stale_definition()
}

/// Whether a login service is installed for this user, and where to look.
pub fn status() -> Status {
    platform::status()
}

/// Install the daemon as a login service and start it now.
///
/// Installing twice is safe: each platform's body replaces its own definition
/// rather than adding a second one.
pub fn install(binary: Option<&Path>, port: Option<u16>) -> Result<Status> {
    // `setup` names the binary it just installed, which is not the one running
    // this code: a service pointing at the downloaded installer, or at a
    // checkout someone is about to delete, is a service that stops working
    // without saying so.
    let exe = match binary {
        Some(path) => plain(path.canonicalize().unwrap_or_else(|_| path.to_path_buf())),
        None => exe()?,
    };
    // A service pointing into the temp directory is one the next cleanup
    // breaks, and it is what a test that forgot `--no-service` produces: HOME
    // is redirected, but launchd and systemd register into the real session,
    // and on 2026-09-23 a test run replaced the developer's own login service
    // with a binary under /var/folders.
    if under_temp(&exe) {
        anyhow::bail!(
            "{} is under the temporary directory, and a login service pointing there stops \
             working when it is cleaned up. Install semlith first (`semlith setup`) and \
             register the installed binary.",
            exe.display()
        );
    }
    if let Some(guarded) = tcc_guarded(&exe) {
        anyhow::bail!(
            "this binary is at {} — macOS gates {} for a background login agent, and a launchd job pointing there hangs inside the dynamic linker before it can write a word to its own log. Install semlith outside that directory and run `semlith start --service` from there; `semlith setup` puts it in {}.",
            exe.display(),
            guarded.display(),
            crate::home::bin_dir()
                .map(|d| d.display().to_string())
                .unwrap_or_else(|_| "the semlith home's bin directory".to_string()),
        );
    }
    // A daemon may already be running — started by hand, or by an earlier
    // install. Registering the service is still right, because the point is
    // the next login; starting a second one now is not. It would lose the bind
    // and exit, and launchd's `KeepAlive` and systemd's `Restart=always` would
    // start it again, and again: an install that means "semlith is always
    // there" turning into a process respawning every ten seconds into a log
    // nobody is reading.
    let answering = crate::daemon::port_of(port);
    if already_answering(answering) {
        let mut status = platform::install(&exe, port)?;
        status.started_now = false;
        return Ok(status);
    }
    let mut status = platform::install(&exe, port)?;
    // Whether a daemon actually came up, asked rather than assumed. On Linux
    // `systemctl --user enable --now` reports success and leaves the unit
    // enabled-but-not-running wherever the user session cannot run one — a
    // container, a CI runner, a machine with no lingering — and "enabled" is a
    // statement about the next login, not about now. Reporting that as running
    // is how a page comes to say semlith is there while nothing answers, which
    // is the whole of what this release is about.
    status.started_now = status.installed && waits_for(answering);
    Ok(status)
}

/// Whether a binary lives under the system's temporary directory.
fn under_temp(exe: &Path) -> bool {
    let temp = std::env::temp_dir();
    let temp = temp.canonicalize().unwrap_or(temp);
    let exe = exe.canonicalize().unwrap_or_else(|_| exe.to_path_buf());
    exe.starts_with(&temp)
        || exe.starts_with("/private/var/folders")
        || exe.starts_with("/var/folders")
}

/// Whether something answers on the port within a few seconds of being asked to.
///
/// A service manager returns as soon as it has accepted the job, not when the
/// job is listening, so a single immediate check would answer "no" on a machine
/// where everything is fine.
fn waits_for(port: u16) -> bool {
    for _ in 0..20 {
        if already_answering(port) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    false
}

/// Whether something is already listening on the daemon's port.
///
/// A connect, not a request: this asks whether the bind would fail, which is
/// the only thing the caller is about to care about. Whatever is there may not
/// even be semlith — and if it is not, starting a daemon that cannot bind is
/// still the wrong thing to do.
fn already_answering(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

/// The privacy-gated directory this path is inside, if it is inside one.
///
/// Found by running it. `semlith start --service` from a development checkout
/// under `~/Documents` installed cleanly, `launchctl` reported the job running
/// with a live pid — and nothing ever listened on the port, the log stayed
/// zero bytes, and a sample showed the process stopped in `dyld`'s `__open`.
/// It had not finished loading itself. macOS asks a foreground process for
/// consent to read these directories and simply blocks a background agent, so
/// the failure has no error, no log line and no timeout: exactly the silent
/// absence this release exists to remove, reached through the feature that was
/// meant to remove it.
///
/// macOS only. Linux and Windows have no equivalent gate, and a check that
/// refused there would be refusing something that works.
#[allow(unused_variables)]
fn tcc_guarded(exe: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = crate::home::user_home().ok()?;
        [
            "Documents",
            "Desktop",
            "Downloads",
            "Library/Mobile Documents",
        ]
        .iter()
        .map(|name| home.join(name))
        .find(|dir| exe.starts_with(dir))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Remove the login service. `false` when there was nothing to remove, which
/// is not an error — `--no-service` on a machine that never had one is a
/// statement about the end state, not a request that can fail.
pub fn remove() -> Result<bool> {
    platform::remove()
}

/// When the daemon last started, as a unix second.
///
/// Read from the mtime of any store's discovery file, which the daemon writes
/// as it comes up and removes as it goes down. No new state and no format
/// change: the file that already answers "is a daemon running" also answers
/// "since when".
pub fn last_started() -> Option<i64> {
    let registry = crate::home::Registry::load().ok()?;
    registry
        .stores
        .keys()
        .filter_map(|name| crate::home::Registry::dir_of(name).ok())
        .filter_map(|dir| std::fs::metadata(dir.join(crate::daemon::DISCOVERY_FILE)).ok())
        .filter_map(|meta| meta.modified().ok())
        .filter_map(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs() as i64)
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A verbatim path in a service definition is a path a person cannot read
    /// and several tools refuse. Windows produces one from `canonicalize`.
    #[test]
    fn a_verbatim_path_is_written_in_its_plain_spelling() {
        assert_eq!(
            plain(PathBuf::from(r"\\?\C:\Users\me\semlith.exe")),
            PathBuf::from(r"C:\Users\me\semlith.exe"),
        );
        assert_eq!(
            plain(PathBuf::from(r"\\?\UNC\server\share\semlith.exe")),
            PathBuf::from(r"\\server\share\semlith.exe"),
        );
        // A path that is already plain is left exactly as it is.
        assert_eq!(
            plain(PathBuf::from("/usr/local/bin/semlith")),
            PathBuf::from("/usr/local/bin/semlith"),
        );
    }

    /// A launchd agent pointing into a privacy-gated directory does not fail.
    /// It hangs in the dynamic linker with an empty log and a live pid, which
    /// is the one failure mode this release must not ship.
    #[test]
    #[cfg(target_os = "macos")]
    fn a_binary_in_a_privacy_gated_directory_is_refused_rather_than_hung() {
        let home = crate::home::user_home().expect("a home");
        assert_eq!(
            tcc_guarded(&home.join("Documents/code/semlith/target/release/semlith")),
            Some(home.join("Documents")),
        );
        assert_eq!(
            tcc_guarded(&home.join("Desktop/semlith")),
            Some(home.join("Desktop")),
        );
        // The directory semlith installs itself into is not gated, which is
        // what makes the refusal actionable rather than a dead end.
        assert_eq!(tcc_guarded(&home.join(".semlith/bin/semlith")), None);
        assert_eq!(tcc_guarded(Path::new("/usr/local/bin/semlith")), None);
    }

    /// Installing must not start a second daemon onto a port something already
    /// holds. It would lose the bind and exit, and `KeepAlive` would start it
    /// again — an install that means "semlith is always there" becoming a
    /// process respawning into a log nobody reads.
    #[test]
    fn a_port_something_already_holds_is_seen_as_held() {
        let held = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a port");
        let port = held.local_addr().expect("an address").port();
        assert!(
            already_answering(port),
            "a bound port was not seen as bound"
        );
        drop(held);
        // The negative half against a port this test has just proved free by
        // binding it itself (#141). Checking the port it had held raced every
        // other test in the binary: one that bound an ephemeral port in the
        // gap made "a freed port" answer. A port some other test takes between
        // our bind and our check is retried, within a bound, never excused.
        let free = (0..20).any(|_| {
            let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a port");
            let port = probe.local_addr().expect("an address").port();
            drop(probe);
            !already_answering(port)
        });
        assert!(
            free,
            "a freed port was still seen as bound, twenty times over"
        );
    }

    /// `remove` on a machine that never had a service is an end state, not a
    /// failure — `semlith start --no-service` has to be safe to run twice.
    #[test]
    fn removing_a_service_that_is_not_installed_is_not_an_error() {
        // This reads `HOME` twice, through `status` and through `remove`, and
        // another test repointing it between the two is what made this fail in
        // a full run and pass on its own.
        let _guard = crate::home::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if status().installed {
            // The developer's own machine has one installed; removing it here
            // would be this test deciding something it was not asked to.
            return;
        }
        assert!(matches!(remove(), Ok(false)), "removing nothing failed");
    }
}
