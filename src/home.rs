//! The store home, and how a command decides which store it means.
//!
//! Before 0.9.0 a store was a `.semlith` directory beside the corpus, and the
//! only way to name one was `--store`. That made every client configuration
//! carry a path, and it put a store one forgotten `.gitignore` line away from
//! being committed into the repository it indexes.
//!
//! From 0.9.0 a new store is created under `~/.semlith/stores/<name>`, and
//! `registry.json` beside it records which roots each store covers. The
//! registry is the single source of truth three things read: `semlith start`
//! opens and watches every store in it, the portal lists them, and a client
//! stanza becomes `semlith mcp` with no arguments.
//!
//! The layout inside a store directory does not change. A store written by
//! 0.8.0 opens unchanged wherever it sits, which is why the resolution order
//! below prefers an existing `./.semlith` over the home: nobody's setup changes
//! until they run `semlith adopt`.
//!
//! The registry is written by semlith and by nothing else. It is state, not
//! configuration — there is no supported way to hand-edit it, and a field it
//! does not recognise is dropped on the next write.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Overrides the store home. One greppable path on every platform beats three
/// platform conventions, and anyone who disagrees sets this.
pub const HOME_ENV: &str = "SEMLITH_HOME";

/// The variable [`user_home`] starts from, named so that a test which has to
/// save and restore it does not become a second place that reads it. The
/// invariant is that `var_os("HOME")` appears once in `src/`, inside
/// [`user_home`], and `tests/home.rs` asserts exactly that.
pub const HOME_VAR: &str = "HOME";

/// One lock for every test that touches a home directory variable.
///
/// `HOME` and `SEMLITH_HOME` are process-wide, and `cargo test` runs the whole
/// crate's tests on one thread pool, so a test that repoints either of them
/// repoints it for every other test running at that moment. Living in a
/// separate module does not serialise anything — `setup.rs` carried a comment
/// saying it did, and the test it guarded was repointing the real `HOME` out
/// from under `service::remove`, which resolves the login service's path
/// through it. That is how `removing_a_service_that_is_not_installed_is_not_
/// an_error` failed in a full run and passed on its own.
///
/// Every test that *sets* one of these takes this lock, and every test that
/// *reads* one and would be confused by a different answer takes it too.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run `body` with one environment variable set, and put it back afterwards.
///
/// Holds [`ENV_LOCK`] for the whole of it. The guard is taken with
/// `unwrap_or_else(into_inner)` so one panicking test does not poison every
/// later one into failing for a reason that is not theirs.
#[cfg(test)]
pub(crate) fn with_env_var<T>(name: &str, value: &std::ffi::OsStr, body: impl FnOnce() -> T) -> T {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let was = std::env::var_os(name);
    // SAFETY: the lock above makes this the only thread touching the
    // variable, and every test that reads it holds the same lock.
    unsafe { std::env::set_var(name, value) };
    let out = body();
    match was {
        Some(previous) => unsafe { std::env::set_var(name, previous) },
        None => unsafe { std::env::remove_var(name) },
    }
    out
}

/// The user's home directory, per platform. The only place in `src/` that
/// reads `HOME`.
///
/// `HOME` when it is set and non-empty, which covers unix and every Windows
/// shell that sets it (Git Bash, MSYS, WSL). On Windows nothing sets it by
/// default, so `USERPROFILE` comes next — it is what `install.ps1` used to
/// place the binary and what PowerShell derives `$HOME` from, so matching it
/// is what makes the installer and the runtime agree — and then the
/// `HOMEDRIVE` + `HOMEPATH` pair that predates it. Nothing else: a resolution
/// order with a default at the end is the bug this function exists to remove
/// (#70, #73).
///
/// There is deliberately no variant of this that returns a path when it does
/// not know one. Every caller takes the `Result`, so a machine with no home
/// gets an error naming the variables rather than a store, a registry and an
/// agent key written into whatever directory the process started in.
pub fn user_home() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    #[cfg(windows)]
    {
        if let Some(dir) = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(dir));
        }
        let drive = std::env::var_os("HOMEDRIVE").filter(|v| !v.is_empty());
        let path = std::env::var_os("HOMEPATH").filter(|v| !v.is_empty());
        if let (Some(mut drive), Some(path)) = (drive, path) {
            drive.push(path);
            return Ok(PathBuf::from(drive));
        }
        bail!(
            "none of HOME, USERPROFILE or HOMEDRIVE+HOMEPATH is set, so semlith does \
             not know where your home directory is. Set {HOME_ENV} to the directory \
             you want its stores in."
        );
    }
    #[cfg(not(windows))]
    bail!(
        "HOME is not set, so semlith does not know where your home directory is. \
         Set {HOME_ENV} to the directory you want its stores in."
    )
}

/// The store home: [`HOME_ENV`] when set, else `<home>/.semlith`.
///
/// With neither available this is an error rather than `./.semlith`. A semlith
/// run by a daemon supervisor, a cron job, a container entrypoint or a Windows
/// shell with no environment used to create a store, a registry and an agent
/// key in whatever directory it happened to start in, and say nothing.
pub fn home_or_error() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    Ok(user_home()?.join(".semlith"))
}

/// The directory holding one subdirectory per store.
pub fn stores_root() -> Result<PathBuf> {
    Ok(home_or_error()?.join("stores"))
}

/// Where the install scripts put the binary, and where `semlith setup` looks
/// for it. Inside the home rather than beside it so that `SEMLITH_HOME` moves
/// one directory and not two, and so an uninstall is one `rm -rf`.
pub fn bin_dir() -> Result<PathBuf> {
    Ok(home_or_error()?.join("bin"))
}

/// The registry file. Model weights deliberately do not live under the home:
/// a cache is deletable and a store is not, so weights stay in
/// `~/.cache/semlith/models`.
pub fn registry_path() -> Result<PathBuf> {
    Ok(home_or_error()?.join("registry.json"))
}

/// Where the three values the Index page can change are kept.
///
/// Beside `registry.json` and not inside it: the registry is what stores exist
/// and where, and these are how hard this machine may work. `daemon.json` is
/// not the place either — that is per-store discovery, written beside a lock
/// and deleted when the daemon exits, and these outlive a run.
///
/// It is not a user-editable config file, which AGENTS.md says this project
/// does not have. It is tool-written state, like the registry: semlith writes
/// it when somebody moves a field on the page, and nothing documents a way to
/// hand-edit it.
pub fn settings_path() -> Result<PathBuf> {
    Ok(home_or_error()?.join("settings.json"))
}

/// The projects directly under a directory: the children that are git
/// repositories, or its plain subdirectories where none of them is.
///
/// One level only. A monorepo is one store, and its nested repositories are
/// its own business — discovery that walked down would turn one corpus into
/// forty stores that each know a fortieth of it.
///
/// A `.git` file counts as much as a `.git` directory: that is what a worktree
/// and a submodule have, and both are things a developer would tick.
///
/// One implementation, called by the portal's `/api/projects` and by `semlith
/// index --projects`, so the checklist and the terminal cannot disagree about
/// what is under a folder.
pub fn projects_under(dir: &Path) -> Result<(Vec<PathBuf>, bool)> {
    let listing = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    let mut repositories = Vec::new();
    let mut plain = Vec::new();
    for entry in listing.flatten() {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let path = crate::canonical(&entry.path());
        if path.join(".git").exists() {
            repositories.push(path);
        } else {
            plain.push(path);
        }
    }
    repositories.sort();
    plain.sort();
    let found = !repositories.is_empty();
    Ok((if found { repositories } else { plain }, found))
}

/// The three values, as the file holds them.
///
/// Every field is optional and absent means "derive it". A home with no file
/// at all — which is every home until somebody moves a field — derives all
/// three, so the file's absence is a state rather than a missing default.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub runs_at_once: Option<usize>,
    pub embed_threads: Option<usize>,
    pub index_memory_mb: Option<usize>,
    /// Whether the ledger's Session replay tab may read this machine's agent
    /// transcripts. Absent means off: the files belong to another program,
    /// so nothing reads them until somebody says so on the Privacy page.
    #[serde(default)]
    pub session_replay: Option<bool>,
}

impl Settings {
    /// What the file says, or nothing at all.
    ///
    /// A file that cannot be read or parsed is the same answer as no file:
    /// these are three numbers with derivable defaults, and failing a daemon
    /// start over them would be refusing to work because of a preference.
    pub fn load() -> Self {
        let Ok(path) = settings_path() else {
            return Self::default();
        };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Write the file, creating the home if it is not there.
    ///
    /// The same temporary-file-and-rename the registry uses, for the same
    /// reason: a process killed mid-write leaves the previous settings rather
    /// than half of a new set.
    pub fn save(&self) -> Result<()> {
        let path = settings_path()?;
        let dir = path.parent().unwrap_or(Path::new("."));
        secure_dir(dir).with_context(|| format!("creating the store home {}", dir.display()))?;
        let temp = path.with_extension(format!("json.{}.new", std::process::id()));
        let body = serde_json::to_string_pretty(self)? + "\n";
        write_private(&temp, body.as_bytes())
            .with_context(|| format!("writing {}", temp.display()))?;
        if let Err(e) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            return Err(e).with_context(|| format!("writing {}", path.display()));
        }
        Ok(())
    }
}

/// Where the agent key lives.
///
/// Under the home rather than inside a store: it authorises the daemon's MCP
/// endpoint, which serves every store the daemon opened, and a credential that
/// moved when a store was adopted would be a credential every client had to be
/// told about again.
pub fn agent_key_path() -> Result<PathBuf> {
    Ok(home_or_error()?.join("agent.key"))
}

/// The prefix every agent key carries, so one is recognisable in a config file
/// somebody is looking at six months later.
pub const AGENT_KEY_PREFIX: &str = "sml_";

/// The agent key, created on first read and never regenerated implicitly.
///
/// This is the whole point of the credential: a client's configuration is
/// written once and stays valid across daemon restarts, upgrades and portal
/// token rotations. A key that changed per run would make every stanza stale
/// on every restart, with nothing able to repair the ones semlith did not
/// write — `setup.rs` already records that only Claude Code's config is safe
/// to write to.
pub fn agent_key() -> Result<String> {
    let path = agent_key_path()?;
    if path.exists() {
        let key = std::fs::read_to_string(&path)
            .with_context(|| format!("reading the agent key at {}", path.display()))?
            .trim()
            .to_string();
        check_key_mode(&path)?;
        if !is_agent_key(&key) {
            bail!(
                "{} does not hold an agent key. Delete it and start again, and a new key is                  written; every client then needs the new stanza.",
                path.display()
            );
        }
        return Ok(key);
    }
    let key = new_agent_key();
    write_agent_key(&key)?;
    Ok(key)
}

/// Mint a new key and replace the file, returning the new key.
///
/// Deliberately separate from [`agent_key`]: a key is only ever replaced
/// because somebody asked for it to be, and a function that could do either
/// depending on the state of the disk is one that rotates by accident.
pub fn rotate_agent_key() -> Result<String> {
    let key = new_agent_key();
    write_agent_key(&key)?;
    Ok(key)
}

pub fn is_agent_key(value: &str) -> bool {
    let Some(body) = value.strip_prefix(AGENT_KEY_PREFIX) else {
        return false;
    };
    body.len() == 64 && body.bytes().all(|b| b.is_ascii_hexdigit())
}

fn new_agent_key() -> String {
    // The OS random source, not a hash of the clock and a pointer: this one is
    // written to disk and lives for as long as the install does, so it has to
    // be unguessable rather than merely unique. `getrandom` is already in the
    // tree under fastembed; naming it adds no crate to the build.
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS random source");
    let mut out = String::with_capacity(AGENT_KEY_PREFIX.len() + 64);
    out.push_str(AGENT_KEY_PREFIX);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn write_agent_key(key: &str) -> Result<()> {
    let path = agent_key_path()?;
    if let Some(parent) = path.parent() {
        secure_dir(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    // The mode is set as the file is created rather than afterwards: a file
    // that is world-readable for the microsecond between the two is still a
    // file that was world-readable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("writing the agent key to {}", path.display()))?;
    use std::io::Write;
    writeln!(file, "{key}")?;
    Ok(())
}

/// Refuse a key file anyone else on the machine can read.
///
/// On Windows there is no mode to check: the home sits inside the user's
/// profile, whose default ACL grants that user alone, and semlith does not
/// carry an API to read or write an ACL. That is stated rather than silently
/// assumed.
#[cfg(unix)]
fn check_key_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        bail!(
            "{} is mode {:o}; an agent key that other users on this machine can read is not one.              Run `chmod 600 {}` and start again.",
            path.display(),
            mode,
            path.display()
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_key_mode(_path: &Path) -> Result<()> {
    Ok(())
}

/// What a store directory is called when it sits beside its corpus.
pub const LOCAL_DIR: &str = ".semlith";

/// The mode every directory semlith creates gets, and keeps.
pub const DIR_MODE: u32 = 0o700;

/// The mode every file semlith creates that is worth reading gets.
pub const FILE_MODE: u32 = 0o600;

/// Create a directory nobody else on the machine can read, and keep it that way.
///
/// A store holds the text of every file it indexed. On a shared machine — a
/// build box, a lab workstation, a container with more than one account — a
/// directory created with the process umask is usually `0755`, so the corpus
/// was readable by everyone. The tightening is applied on every open rather
/// than only at creation, because a store made by an older semlith is already
/// loose and its owner will never think to fix it by hand.
///
/// Only ever narrows. A directory somebody has deliberately opened up is not
/// something this widens back, and nothing here touches a directory semlith did
/// not make.
pub fn secure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
    tighten_dir(path);
    Ok(())
}

/// Narrow a directory to [`DIR_MODE`] if it is looser. Best effort: a
/// filesystem without modes is not a reason to fail to open a store.
pub fn tighten_dir(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(DIR_MODE));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// The same for a file.
pub fn tighten_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(FILE_MODE));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Write a file nobody else on the machine can read.
///
/// The mode is set as the file is created rather than afterwards: a file that
/// is world-readable for the microsecond between the two is a file that was
/// world-readable. The registry names every store on the machine and the roots
/// each one covers, which is a map of what this user works on.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(FILE_MODE);
    }
    let mut file = options.open(path)?;
    use std::io::Write;
    file.write_all(bytes)?;
    // An existing file keeps its own mode through `truncate`, so one left loose
    // by an older semlith is narrowed here too.
    tighten_file(path);
    Ok(())
}

/// Whether this directory is readable by anyone but its owner.
///
/// What `semlith stats` and the Stores page report, so a store that was made
/// before 0.14.0 and has not been opened since says so rather than looking
/// like every other row.
pub fn loose_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
        (mode & 0o077 != 0).then_some(mode)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// One store's entry in the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Every directory this store indexes, canonical. `semlith start` watches
    /// all of them; a root that no longer exists is reported, never fatal.
    pub roots: Vec<PathBuf>,
    /// The embedding model the store was built with, recorded so the portal
    /// and `semlith mcp` can say it without opening the store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Unix seconds. Only ever set when the entry is created.
    pub created: u64,
}

/// `registry.json` — store name to the roots it covers.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub stores: BTreeMap<String, Entry>,
    /// Store directories outside the home that this user has said may be
    /// opened, canonical.
    ///
    /// A `.semlith` beside a corpus is a store somebody put there, and from
    /// 0.14.0 that somebody has to have been this user: a repository can carry
    /// one, and a cloned store is a corpus an attacker chose, answering the
    /// questions an agent asks. `semlith adopt` moves such a store into the
    /// home and `semlith trust` records it where it is; either is a deliberate
    /// act, which is the whole difference.
    ///
    /// A registry written by an older semlith has no such field and loads as an
    /// empty list, which is why the first interactive run offers to fill it.
    #[serde(default)]
    pub trusted: Vec<PathBuf>,
}

impl Registry {
    /// Read the registry, or an empty one if there is no home yet.
    ///
    /// A registry that cannot be parsed is an error rather than an empty
    /// registry: silently starting over would create a second store for a
    /// corpus that already has one, and the first store's vectors would then
    /// quietly stop being updated.
    pub fn load() -> Result<Self> {
        let path = registry_path()?;
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        serde_json::from_str(&text).with_context(|| {
            format!(
                "{} is not readable as a registry; move it aside to start over",
                path.display()
            )
        })
    }

    /// Write the registry, creating the home if it is not there.
    ///
    /// Written to a temporary file and renamed, so a process killed mid-write
    /// leaves the previous registry rather than half of a new one.
    pub fn save(&self) -> Result<()> {
        let path = registry_path()?;
        let dir = path.parent().unwrap_or(Path::new("."));
        secure_dir(dir).with_context(|| format!("creating the store home {}", dir.display()))?;
        // A name of this process's own. `registry.json.new` was one fixed name
        // for every process on the machine, so two semliths saving at once
        // wrote the same temporary file and one of them renamed the other's
        // half-written bytes into place.
        let temp = path.with_extension(format!("json.{}.new", std::process::id()));
        let body = serde_json::to_string_pretty(self)? + "\n";
        write_private(&temp, body.as_bytes())
            .with_context(|| format!("writing {}", temp.display()))?;
        if let Err(e) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            return Err(e).with_context(|| format!("writing {}", path.display()));
        }
        Ok(())
    }

    /// Where a registered store's directory is.
    ///
    /// The name is sanitised here as well as where it is chosen: this is a
    /// public function taking a string that becomes a path, and a registry
    /// somebody edited by hand — which is not supported, and happens — should
    /// not be able to name `../..`.
    pub fn dir_of(name: &str) -> Result<PathBuf> {
        Ok(stores_root()?.join(sanitize(name)))
    }

    /// Every registered store's directory, in name order.
    pub fn dirs(&self) -> Result<Vec<PathBuf>> {
        self.stores.keys().map(|n| Self::dir_of(n)).collect()
    }

    /// The registered store whose roots cover `path`, most specific first.
    ///
    /// Most specific wins so a store registered against one package inside a
    /// monorepo keeps that package, rather than being swallowed by a store
    /// registered against the whole tree.
    pub fn covering(&self, path: &Path) -> Option<(&str, &Entry)> {
        let mut best: Option<(&str, &Entry, usize)> = None;
        for (name, entry) in &self.stores {
            for root in &entry.roots {
                if path == root || path.starts_with(root) {
                    let depth = root.components().count();
                    if best.is_none_or(|(_, _, d)| depth > d) {
                        best = Some((name, entry, depth));
                    }
                }
            }
        }
        best.map(|(n, e, _)| (n, e))
    }

    /// The name a registered store directory has, if it is one of ours.
    pub fn name_of(&self, dir: &Path) -> Option<&str> {
        let dir = crate::canonical(dir);
        self.stores
            .keys()
            .map(String::as_str)
            .find(|n| Self::dir_of(n).is_ok_and(|theirs| crate::canonical(&theirs) == dir))
    }

    /// Record a store and the root it covers, creating the entry if new.
    ///
    /// Adding a root that a recorded root already covers is a no-op, so
    /// indexing the same tree twice does not grow the watch list.
    pub fn register(&mut self, name: &str, root: &Path, model: Option<&str>) -> Result<()> {
        let root = crate::canonical(root);
        let entry = self
            .stores
            .entry(name.to_string())
            .or_insert_with(|| Entry {
                roots: Vec::new(),
                model: model.map(str::to_string),
                created: now(),
            });
        if entry.model.is_none() {
            entry.model = model.map(str::to_string);
        }
        if !entry.roots.iter().any(|r| root.starts_with(r)) {
            // A new root that contains recorded ones replaces them: two watches
            // over the same tree is two re-embeds of every save.
            entry.roots.retain(|r| !r.starts_with(&root));
            entry.roots.push(root);
            entry.roots.sort();
        }
        self.save()
    }

    /// Whether this store directory may be opened without being named.
    ///
    /// Anything under the store home is trusted by being there — semlith put it
    /// there. Anything else has to be in the list.
    pub fn trusts(&self, dir: &Path) -> bool {
        let dir = crate::canonical(dir);
        if home_or_error().is_ok_and(|home| dir.starts_with(crate::canonical(&home))) {
            return true;
        }
        self.trusted.iter().any(|t| crate::canonical(t) == dir)
    }

    /// Record a store directory as one this user has chosen to open.
    ///
    /// Idempotent, and it moves nothing: `adopt` is the command that moves a
    /// store, and somebody who wants their `.semlith` to stay beside its corpus
    /// should not have to move it to keep using it.
    pub fn trust(&mut self, dir: &Path) -> Result<PathBuf> {
        let dir = crate::canonical(dir);
        if !dir.join("store.db").exists() {
            bail!(
                "{} is not a semlith store — no store.db in it",
                dir.display()
            );
        }
        if !self.trusted.iter().any(|t| t == &dir) {
            self.trusted.push(dir.clone());
            self.trusted.sort();
            self.save()?;
        }
        Ok(dir)
    }

    /// A store name that is free, derived from `stem`.
    ///
    /// A collision gets a numeric suffix rather than merging into the store
    /// that holds the name: two directories called `api` are two corpora, and
    /// merging them would make every search answer about the wrong one.
    pub fn free_name(&self, stem: &str) -> String {
        let stem = sanitize(stem);
        let taken =
            |n: &str| self.stores.contains_key(n) || Self::dir_of(n).is_ok_and(|d| d.exists());
        if !taken(&stem) {
            return stem;
        }
        for n in 2.. {
            let candidate = format!("{stem}-{n}");
            if !taken(&candidate) {
                return candidate;
            }
        }
        unreachable!("the integers run out before the names do")
    }
}

/// Which store a command means, and why.
#[derive(Debug, Clone)]
pub enum Choice {
    /// `--store` or `SEMLITH_STORE` named them explicitly.
    Named(Vec<PathBuf>),
    /// A `.semlith` directory beside the corpus, from before the home existed.
    Local(PathBuf),
    /// A registered store whose root is the anchor or an ancestor of it.
    Registered { name: String, dir: PathBuf },
    /// Nothing covers the anchor yet, so this is the store that would be made.
    New {
        name: String,
        dir: PathBuf,
        root: PathBuf,
    },
}

impl Choice {
    /// The single store directory this choice names.
    ///
    /// `Named` with several stores is refused here rather than silently using
    /// the first: a write goes to one store, and picking one of several is a
    /// guess at somebody's other repository.
    pub fn one(&self) -> Result<PathBuf> {
        Ok(match self {
            Choice::Named(dirs) => match dirs.as_slice() {
                [one] => one.clone(),
                many => bail!(
                    "this command writes, so it takes one store, not {}: {}",
                    many.len(),
                    many.iter()
                        .map(|d| d.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            },
            Choice::Local(dir) => dir.clone(),
            Choice::Registered { dir, .. } | Choice::New { dir, .. } => dir.clone(),
        })
    }

    /// Every store directory this choice names.
    pub fn all(&self) -> Vec<PathBuf> {
        match self {
            Choice::Named(dirs) => dirs.clone(),
            Choice::Local(dir) => vec![dir.clone()],
            Choice::Registered { dir, .. } | Choice::New { dir, .. } => vec![dir.clone()],
        }
    }

    /// The one line of stderr a choice is worth, or nothing.
    ///
    /// Only `Local` says anything. A store found beside its corpus still works
    /// exactly as it did, and the hint is the only place a user learns that
    /// moving it into the home is a command rather than a migration.
    pub fn hint(&self) -> Option<String> {
        match self {
            Choice::Local(dir) => Some(format!(
                "semlith: using the store at {} — `semlith adopt {}` moves it into {} \
                 so `semlith mcp` and `semlith start` find it with no flags",
                dir.display(),
                dir.display(),
                store_home_for_humans(),
            )),
            _ => None,
        }
    }
}

/// Which store a command means, in the order the release documents:
///
/// 1. `--store` or `SEMLITH_STORE`,
/// 2. an existing `.semlith` beside `anchor`,
/// 3. a registered store whose root is `anchor` or an ancestor of it,
/// 4. otherwise a new store in the home, named after `anchor`.
///
/// `anchor` is the working directory for most commands and the first path
/// argument for `index`, so `semlith index ~/work/api` from anywhere makes a
/// store called `api` registered against `~/work/api` — which is what the
/// person typing it meant.
pub fn resolve(flags: &[PathBuf], anchor: &Path, name: Option<&str>) -> Result<Choice> {
    if !flags.is_empty() {
        return Ok(Choice::Named(flags.to_vec()));
    }
    if let Some(raw) = std::env::var_os("SEMLITH_STORE") {
        let dirs: Vec<PathBuf> = std::env::split_paths(&raw)
            .filter(|p| !p.as_os_str().is_empty())
            .collect();
        if !dirs.is_empty() {
            return Ok(Choice::Named(dirs));
        }
    }

    // Asked before anything resolves to a path under it, so a run with no
    // environment is an error naming both variables rather than a store
    // created in whatever directory it started in.
    home_or_error()?;
    let anchor_dir = directory_of(anchor);
    let registry = Registry::load()?;

    // An explicit `--name` is an instruction, so it skips the search: it names
    // the store to use or to create, and nothing else may answer for it.
    if let Some(name) = name {
        let name = sanitize(name);
        let dir = Registry::dir_of(&name)?;
        return Ok(if registry.stores.contains_key(&name) {
            Choice::Registered { name, dir }
        } else {
            Choice::New {
                name,
                dir,
                root: crate::canonical(&anchor_dir),
            }
        });
    }

    let local = anchor_dir.join(LOCAL_DIR);
    if local.join("store.db").exists() {
        if !registry.trusts(&local) {
            bail!("{}", untrusted(&local));
        }
        return Ok(Choice::Local(local));
    }

    let canonical_anchor = crate::canonical(&anchor_dir);
    if let Some((name, _)) = registry.covering(&canonical_anchor) {
        return Ok(Choice::Registered {
            name: name.to_string(),
            dir: Registry::dir_of(name)?,
        });
    }

    let stem = canonical_anchor
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "store".to_string());
    let name = registry.free_name(&stem);
    Ok(Choice::New {
        dir: Registry::dir_of(&name)?,
        name,
        root: canonical_anchor,
    })
}

/// The stores a read-only command opens with no flags: the one covering the
/// working directory, or every registered store when nothing covers it.
///
/// Standing in a repository, a question is about that repository. Standing
/// anywhere else there is no "that repository", and every store the developer
/// has is a better answer than an error about a directory that was never a
/// store.
pub fn read_dirs(flags: &[PathBuf], cwd: &Path) -> Result<Vec<PathBuf>> {
    match resolve(flags, cwd, None)? {
        Choice::New { .. } => all_dirs(flags, cwd),
        other => Ok(other.all()),
    }
}

/// Record what an index run just covered, so `semlith start` watches it.
///
/// Only a store in the home is recorded. A `--store` path and a `.semlith`
/// beside its corpus are both things the user is naming themselves every time,
/// and registering them would put a store in the portal that the next command
/// does not resolve to.
pub fn record(choice: &Choice, roots: &[PathBuf], model: &str) -> Result<()> {
    let name = match choice {
        Choice::New { name, .. } | Choice::Registered { name, .. } => name.clone(),
        Choice::Local(_) | Choice::Named(_) => return Ok(()),
    };
    let mut registry = Registry::load()?;
    for root in roots {
        let root = directory_of(&crate::canonical(root));
        registry.register(&name, &root, Some(model))?;
    }
    Ok(())
}

/// Every store a read-only command with no flags should open: each registered
/// store, plus a `.semlith` beside the working directory if there is one.
///
/// The local store is included so a developer who has not run `adopt` sees no
/// change from 0.8.0 — `semlith mcp` in a repository with a `.semlith` still
/// serves it.
pub fn all_dirs(flags: &[PathBuf], cwd: &Path) -> Result<Vec<PathBuf>> {
    if !flags.is_empty() {
        return Ok(flags.to_vec());
    }
    if let Some(raw) = std::env::var_os("SEMLITH_STORE") {
        let dirs: Vec<PathBuf> = std::env::split_paths(&raw)
            .filter(|p| !p.as_os_str().is_empty())
            .collect();
        if !dirs.is_empty() {
            return Ok(dirs);
        }
    }

    let registry = Registry::load()?;
    let mut out = Vec::new();
    let local = cwd.join(LOCAL_DIR);
    if local.join("store.db").exists() {
        // A read is not safer than a write here: the answer a search gives is
        // the whole of what a poisoned store is for.
        if !registry.trusts(&local) {
            bail!("{}", untrusted(&local));
        }
        out.push(local);
    }
    for dir in registry.dirs()? {
        if dir.join("store.db").exists() {
            out.push(dir);
        }
    }
    Ok(out)
}

/// The directories an agent may index into this store.
///
/// The roots the registry records for it, and — when the store is a `.semlith`
/// beside its corpus — the directory it sits in. That second one matters
/// because `--store` deliberately registers nothing (a path the user names
/// every time is not a store the daemon should list), so without it a store
/// created as `--store <corpus>/.semlith` has no roots at all and an agent
/// cannot index the very corpus that store is about.
///
/// A `.semlith` directory's parent *is* its corpus by construction: it is the
/// default root `semlith adopt` records for exactly that reason.
pub fn index_roots(store_dir: &Path) -> Vec<PathBuf> {
    let mut roots = Registry::load()
        .ok()
        .and_then(|registry| {
            registry
                .name_of(store_dir)
                .and_then(|name| registry.stores.get(name))
                .map(|entry| entry.roots.clone())
        })
        .unwrap_or_default();

    if store_dir.file_name().is_some_and(|n| n == LOCAL_DIR)
        && let Some(corpus) = store_dir.parent()
    {
        let corpus = crate::canonical(corpus);
        if !roots.contains(&corpus) {
            roots.push(corpus);
        }
    }
    roots
}

/// What to say about a store this user has not said may be opened.
///
/// Both commands are named because they are different answers to the same
/// question: `trust` leaves the store where it is, `adopt` moves it into the
/// home. Somebody who cloned a repository and does not recognise the store is
/// being told, in the same sentence, that there is one.
fn untrusted(dir: &Path) -> String {
    format!(
        "{} is a semlith store this machine has not been told to open.\n\
         A `.semlith` directory can arrive inside a repository, and a store is \
         what semlith answers from, so it is opened only once you have said so:\n\
         \n    semlith trust {}      keep it where it is\n\
         \n    semlith adopt {}      move it into {}\n\
         \nOr name it explicitly with --store, which is always an instruction.",
        dir.display(),
        dir.display(),
        dir.display(),
        store_home_for_humans(),
    )
}

/// The store home as a sentence fragment, for a message that is advice rather
/// than an answer. A hint is not worth failing a command over, and naming a
/// path semlith cannot resolve would be worse than naming none.
fn store_home_for_humans() -> String {
    match stores_root() {
        Ok(dir) => dir.display().to_string(),
        Err(_) => "the store home".to_string(),
    }
}

/// Move an existing store directory into the home and register it.
///
/// A rename when the home is on the same filesystem and a copy-then-remove
/// when it is not. Neither re-embeds anything: the store's bytes are the store,
/// and where they sit is not part of its format.
pub fn adopt(source: &Path, root: Option<&Path>, name: Option<&str>) -> Result<(String, PathBuf)> {
    if !source.join("store.db").exists() {
        bail!(
            "{} is not a semlith store — no store.db in it",
            source.display()
        );
    }
    let source = crate::canonical(source);

    let mut registry = Registry::load()?;
    if let Some(existing) = registry.name_of(&source) {
        bail!(
            "{} is already the registered store {existing}",
            source.display()
        );
    }

    // The root defaults to the directory the store sat in, which for a
    // `.semlith` is exactly the corpus it was indexing.
    let root = match root {
        Some(r) => crate::canonical(r),
        None => source
            .parent()
            .map(crate::canonical)
            .unwrap_or_else(|| source.clone()),
    };

    let stem = match name {
        Some(n) => n.to_string(),
        None => label_for(&source),
    };
    let name = registry.free_name(&stem);
    let target = Registry::dir_of(&name)?;

    secure_dir(&stores_root()?)?;
    match std::fs::rename(&source, &target) {
        Ok(()) => {}
        Err(_) => {
            // A different filesystem. Copy, verify the copy opens, then remove
            // the source — in that order, so a failed copy never costs the
            // original.
            copy_tree(&source, &target)
                .with_context(|| format!("copying {} to {}", source.display(), target.display()))?;
            if !target.join("store.db").exists() {
                bail!("copying {} left no store.db behind", source.display());
            }
            std::fs::remove_dir_all(&source)
                .with_context(|| format!("removing {}", source.display()))?;
        }
    }

    let model = crate::Semlith::open_existing(&target)
        .ok()
        .map(|s| s.model().to_string());
    registry.register(&name, &root, model.as_deref())?;
    Ok((name, target))
}

/// Re-point a registered store whose corpus moved.
///
/// A registry edit and nothing else: no re-embedding, no store rewritten. The
/// portal offers it beside a root it can see is missing, so the directory is
/// checked here rather than discovered by a watcher that finds nothing.
pub fn repoint(name: &str, root: &Path) -> Result<()> {
    if !root.is_dir() {
        bail!("{} is not a directory", root.display());
    }
    let mut registry = Registry::load()?;
    let Some(entry) = registry.stores.get_mut(name) else {
        bail!(
            "no registered store called {name}; these are: {}",
            registry
                .stores
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    };
    entry.roots = vec![crate::canonical(root)];
    registry.save()
}

/// Unregister a store and delete everything it holds.
///
/// The vectors, the chunk text, the graph and the ledger for that corpus, and
/// then the registry entry — in that order, so a delete interrupted halfway
/// leaves a registered store with a missing directory, which the portal
/// already reports and offers to re-point, rather than an unnamed directory
/// nothing points at.
///
/// The files on disk that were indexed are not touched. This deletes what
/// semlith derived from them.
pub fn delete_store(name: &str) -> Result<PathBuf> {
    let mut registry = Registry::load()?;
    if !registry.stores.contains_key(name) {
        bail!(
            "no registered store called {name}; these are: {}",
            registry
                .stores
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let dir = Registry::dir_of(name)?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("deleting {}", dir.display()))?;
    }
    registry.stores.remove(name);
    registry.save()?;
    Ok(dir)
}

/// What to call a store adopted from `dir`: the directory holding it, because
/// almost every adopted store is a `.semlith` whose own name says nothing.
fn label_for(dir: &Path) -> String {
    let own = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if (own.starts_with('.') || own.is_empty())
        && let Some(parent) = dir
            .parent()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
    {
        return parent;
    }
    if own.is_empty() {
        "store".to_string()
    } else {
        own
    }
}

/// The directory a path anchors to: itself, or its parent if it is a file.
fn directory_of(path: &Path) -> PathBuf {
    if path.is_file() {
        return path.parent().unwrap_or(Path::new(".")).to_path_buf();
    }
    path.to_path_buf()
}

/// A store name that is one safe path segment.
///
/// The name becomes a directory under the home and appears in the portal, so
/// anything that is not a plain character becomes a dash rather than a
/// traversal.
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(['-', '.']).to_string();
    if cleaned.is_empty() {
        "store".to_string()
    } else {
        cleaned
    }
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(roots: &[&str]) -> Entry {
        Entry {
            roots: roots.iter().map(PathBuf::from).collect(),
            model: None,
            created: 0,
        }
    }

    /// A store registered against a package inside a monorepo must not be
    /// swallowed by one registered against the whole tree: the narrower answer
    /// is the one the developer standing in that package meant.
    #[test]
    fn the_most_specific_root_wins() {
        let mut registry = Registry::default();
        registry
            .stores
            .insert("mono".into(), entry(&["/work/mono"]));
        registry
            .stores
            .insert("api".into(), entry(&["/work/mono/services/api"]));

        let (name, _) = registry
            .covering(Path::new("/work/mono/services/api/src"))
            .expect("an ancestor root covers a subdirectory");
        assert_eq!(name, "api");

        let (name, _) = registry
            .covering(Path::new("/work/mono/docs"))
            .expect("the wide root still covers everything else");
        assert_eq!(name, "mono");

        assert!(registry.covering(Path::new("/elsewhere")).is_none());
    }

    /// Two directories called `api` are two corpora. Merging them would make
    /// every search answer about whichever one was indexed second.
    #[test]
    fn a_taken_name_gets_a_suffix_rather_than_being_shared() {
        let mut registry = Registry::default();
        assert_eq!(registry.free_name("api"), "api");
        registry.stores.insert("api".into(), entry(&["/a/api"]));
        assert_eq!(registry.free_name("api"), "api-2");
        registry.stores.insert("api-2".into(), entry(&["/b/api"]));
        assert_eq!(registry.free_name("api"), "api-3");
    }

    /// The name becomes a directory under the home, so a path separator in it
    /// must not become a path.
    #[test]
    fn a_name_is_one_safe_segment() {
        assert_eq!(sanitize("../../etc"), "etc");
        assert_eq!(sanitize("my repo"), "my-repo");
        assert_eq!(sanitize("ok-name_1.2"), "ok-name_1.2");
        assert_eq!(sanitize("///"), "store");
    }

    /// Indexing the same tree twice must not grow the watch list, and a root
    /// that contains recorded ones replaces them rather than joining them.
    #[test]
    fn roots_do_not_accumulate_overlaps() {
        let mut e = entry(&[]);
        let add = |e: &mut Entry, root: &str| {
            let root = PathBuf::from(root);
            if !e.roots.iter().any(|r| root.starts_with(r)) {
                e.roots.retain(|r| !r.starts_with(&root));
                e.roots.push(root);
                e.roots.sort();
            }
        };
        add(&mut e, "/work/api/src");
        add(&mut e, "/work/api/src");
        assert_eq!(e.roots, vec![PathBuf::from("/work/api/src")]);
        add(&mut e, "/work/api/tests");
        assert_eq!(e.roots.len(), 2);
        add(&mut e, "/work/api");
        assert_eq!(
            e.roots,
            vec![PathBuf::from("/work/api")],
            "a root that contains the others replaces them"
        );
    }

    /// An adopted `.semlith` is named after the directory holding it; its own
    /// name says nothing about the corpus.
    #[test]
    fn an_adopted_store_is_named_after_its_corpus() {
        assert_eq!(label_for(Path::new("/work/api/.semlith")), "api");
        assert_eq!(label_for(Path::new("/work/api-store")), "api-store");
    }
}

#[cfg(test)]
mod agent_key_tests {
    use super::*;

    /// A key is minted once and then read, rather than reminted on every start:
    /// a client's configuration is written once and has to stay valid.
    #[test]
    fn a_key_is_created_once_and_then_read() {
        let home = tempfile::tempdir().unwrap();
        temp_env(home.path(), || {
            let first = agent_key().unwrap();
            assert!(is_agent_key(&first), "{first} is not shaped like a key");
            let second = agent_key().unwrap();
            assert_eq!(first, second, "the key was reminted on the second read");
        });
    }

    #[test]
    #[cfg(unix)]
    fn a_new_key_file_is_not_readable_by_anyone_else() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        temp_env(home.path(), || {
            agent_key().unwrap();
            let mode = std::fs::metadata(agent_key_path().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "the key file is mode {mode:o}");
        });
    }

    /// A key file the rest of the machine can read is refused rather than used.
    #[test]
    #[cfg(unix)]
    fn a_group_readable_key_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        temp_env(home.path(), || {
            agent_key().unwrap();
            let path = agent_key_path().unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            let refused = agent_key().unwrap_err().to_string();
            assert!(refused.contains("is mode 644"), "{refused}");
        });
    }

    /// Rotation replaces the key, and the replacement is a different one.
    #[test]
    fn rotation_writes_a_different_key() {
        let home = tempfile::tempdir().unwrap();
        temp_env(home.path(), || {
            let first = agent_key().unwrap();
            let second = rotate_agent_key().unwrap();
            assert_ne!(first, second);
            assert_eq!(
                second,
                agent_key().unwrap(),
                "the new key was not persisted"
            );
        });
    }

    /// `SEMLITH_HOME` is process-wide, so these run one at a time.
    ///
    /// The lock is [`super::ENV_LOCK`] rather than one of this module's own:
    /// `HOME` and `SEMLITH_HOME` both decide where a store home is, so two
    /// locks would let a test holding one repoint a directory a test holding
    /// the other was reading.
    fn temp_env(home: &Path, body: impl FnOnce()) {
        super::with_env_var(HOME_ENV, home.as_os_str(), body);
    }
}
