//! `semlith upgrade`: fetch the newest release for this machine and swap the
//! binary in place.
//!
//! A release that stops being `cargo install` owes the user a way off it. The
//! rule that shapes every line here is in AGENTS.md: semlith never phones out
//! on its own. There is no startup check, no timer, and no banner — the two
//! entry points below run when a user asks and at no other time.
//!
//! The swap is rename-old, rename-new, because that is the one sequence that
//! works on all three targets: unix will happily unlink a running executable,
//! Windows refuses to overwrite one but allows renaming it, and macOS may kill
//! a process whose file changes underneath it. The old file is left as
//! `semlith.old` and removed by the next run.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use ureq::ResponseExt;

use crate::{embed, home};

/// The repository the releases come from.
const REPO: &str = "semlith/semlith";

/// Where those releases are fetched from. Overridable so `tests/upgrade.rs`
/// can point the whole flow — the redirect, the archive and `SHA256SUMS` — at
/// a local fixture server. It is deliberately not in `docs/compatibility.md`:
/// it exists for the tests, not for users, and pointing semlith's self-update
/// at an arbitrary host is not something to invite.
fn origin() -> String {
    std::env::var("SEMLITH_RELEASES_ORIGIN").unwrap_or_else(|_| "https://github.com".to_string())
}

/// `--check` exits with this when a newer release exists, so a script can
/// branch on it without parsing anything. Zero still means "nothing to do".
pub const UPGRADE_AVAILABLE: i32 = 10;

/// What a look at the releases page found.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Check {
    pub installed: String,
    pub latest: String,
    pub available: bool,
    /// Why an upgrade cannot be applied here, when it cannot: an air-gapped
    /// run, a read-only binary, or an install this command does not own.
    pub blocked: Option<String>,
}

/// Resolve the newest release and compare it against the running binary.
/// Changes nothing.
pub fn check() -> Result<Check> {
    let installed = env!("CARGO_PKG_VERSION").to_string();
    let latest = latest_tag()?;
    let available = latest.trim_start_matches('v') != installed;
    Ok(Check {
        available,
        blocked: blocked().map(|e| e.to_string()),
        installed,
        latest,
    })
}

/// Download the release for this target, verify it against the release's
/// `SHA256SUMS`, and put it where the running binary is. `tag` pins a version;
/// `None` takes the newest.
pub fn apply(tag: Option<String>) -> Result<()> {
    if let Some(reason) = blocked() {
        bail!("{reason}");
    }

    let installed = env!("CARGO_PKG_VERSION");
    let tag = match tag {
        Some(t) if t.starts_with('v') => t,
        Some(t) => format!("v{t}"),
        None => latest_tag()?,
    };
    if tag.trim_start_matches('v') == installed {
        println!("already on {installed}");
        return Ok(());
    }

    let target = host_target()?;
    let archive = format!("semlith-{tag}-{target}.{}", archive_ext());
    let base = format!("{}/{REPO}/releases/download/{tag}", origin());

    println!("upgrading {installed} -> {} for {target}", &tag[1..]);

    // Everything lands beside the binary rather than in the system temp
    // directory: a rename across filesystems is a copy that can half-finish,
    // and this rename must be atomic.
    let current = std::env::current_exe().context("locating the running binary")?;
    let dir = current
        .parent()
        .context("the running binary has no directory")?
        .to_path_buf();
    let work = dir.join(format!(".semlith-upgrade-{}", std::process::id()));
    std::fs::create_dir_all(&work).with_context(|| format!("creating {}", work.display()))?;
    let result = swap(&base, &archive, &work, &current, &tag);
    std::fs::remove_dir_all(&work).ok();
    result
}

/// The download, the checksum and the rename, with the working directory
/// cleaned up by the caller whichever way this ends.
fn swap(base: &str, archive: &str, work: &Path, current: &Path, tag: &str) -> Result<()> {
    let bytes = get(&format!("{base}/{archive}"), true)?;
    let sums = String::from_utf8(get(&format!("{base}/SHA256SUMS"), false)?)
        .context("SHA256SUMS was not text")?;

    let expected = sums
        .lines()
        .find_map(|line| {
            let (hash, name) = line.split_once("  ")?;
            (name.trim() == archive).then(|| hash.trim().to_ascii_lowercase())
        })
        .with_context(|| format!("{archive} has no line in the release's SHA256SUMS"))?;

    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != expected {
        bail!(
            "checksum mismatch for {archive}\n  expected {expected}\n  got      {actual}\n\
             nothing was replaced"
        );
    }

    let downloaded = work.join(archive);
    std::fs::write(&downloaded, &bytes)
        .with_context(|| format!("writing {}", downloaded.display()))?;
    let fresh = unpack(&downloaded, work)?;

    // Rename first, replace second. Windows will not overwrite a running
    // executable but will rename one out of the way, and the same two renames
    // are atomic everywhere else.
    let old = current.with_extension("old");
    std::fs::remove_file(&old).ok();
    std::fs::rename(current, &old)
        .with_context(|| format!("moving {} aside", current.display()))?;
    if let Err(e) = std::fs::rename(&fresh, current) {
        std::fs::rename(&old, current).ok();
        return Err(e).with_context(|| format!("installing {}", current.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(current, std::fs::Permissions::from_mode(0o755)).ok();
    }

    println!("installed {} at {}", &tag[1..], current.display());
    println!(
        "the previous binary is {} — delete it when you are happy",
        old.display()
    );
    Ok(())
}

/// Pull the one file that matters out of the archive. `tar` is on every unix
/// that can run this binary; Windows gets the zip reader already in the tree
/// for `.docx`, so neither platform adds a crate.
fn unpack(archive: &Path, into: &Path) -> Result<PathBuf> {
    let name = if cfg!(windows) {
        "semlith.exe"
    } else {
        "semlith"
    };
    if cfg!(windows) {
        let file = std::fs::File::open(archive)
            .with_context(|| format!("opening {}", archive.display()))?;
        let mut zip = zip::ZipArchive::new(file).context("reading the release zip")?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).context("reading a zip entry")?;
            if !entry.name().ends_with(name) {
                continue;
            }
            let out = into.join(name);
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).context("unpacking semlith")?;
            std::fs::write(&out, bytes).with_context(|| format!("writing {}", out.display()))?;
            return Ok(out);
        }
        bail!("the release zip has no {name}");
    }

    let status = std::process::Command::new("tar")
        .arg("xzf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .arg("--strip-components=1")
        .status()
        .context("running tar")?;
    if !status.success() {
        bail!("tar could not unpack {}", archive.display());
    }
    let out = into.join(name);
    if !out.exists() {
        bail!("the release archive has no {name}");
    }
    Ok(out)
}

/// The reason this run must not reach the network at all. Public so `--check`
/// can refuse before it opens a connection: an air-gapped machine's whole claim
/// is that the process never reached the network, and a check that connects
/// first has already broken it.
///
/// Deliberately narrower than [`blocked`]: a binary this command cannot replace
/// is still a binary whose user deserves to be told a newer release exists.
pub fn offline() -> Option<&'static str> {
    embed::airgap().then_some(
        "SEMLITH_AIRGAP is set, so this run will not reach the network. \
         Upgrade on a connected machine and copy the binary across.",
    )
}

fn blocked() -> Option<&'static str> {
    if let Some(reason) = offline() {
        return Some(reason);
    }
    let Ok(current) = std::env::current_exe() else {
        return Some("the running binary could not be located");
    };
    if !owned(&current) {
        return Some(
            "this semlith was not installed by install.sh or install.ps1. \
             Upgrade it the way you installed it — `cargo install semlith` for \
             a cargo install, or your package manager.",
        );
    }
    if std::fs::metadata(&current)
        .map(|m| m.permissions().readonly())
        .unwrap_or(true)
    {
        return Some("the semlith binary is not writable by this user");
    }
    None
}

/// An install this command owns is one that sits in the bin directory the
/// scripts write to. Anything else — cargo's bin, a package manager's prefix —
/// belongs to whatever put it there.
fn owned(current: &Path) -> bool {
    current.parent() == Some(home::bin_dir().as_path())
}

/// `releases/latest` redirects to the newest tag; the tag is the last segment
/// of wherever it lands. Reading the redirect rather than the API keeps this
/// unauthenticated and off the API rate limit.
fn latest_tag() -> Result<String> {
    let url = format!("{}/{REPO}/releases/latest", origin());
    let response = ureq::get(&url)
        .call()
        .with_context(|| format!("asking {url} which release is newest"))?;
    let landed = response.get_uri().to_string();
    let tag = landed
        .rsplit('/')
        .next()
        .filter(|t| t.starts_with('v'))
        .with_context(|| format!("{url} redirected to {landed}, which names no tag"))?;
    Ok(tag.to_string())
}

fn get(url: &str, show_progress: bool) -> Result<Vec<u8>> {
    let bar = show_progress.then(|| {
        let bar = cliclack::spinner();
        bar.start(format!("downloading {url}"));
        bar
    });
    let fetched = (|| -> Result<Vec<u8>> {
        let mut response = ureq::get(url)
            .call()
            .with_context(|| format!("fetching {url}"))?;
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading {url}"))?;
        Ok(bytes)
    })();
    match (&bar, &fetched) {
        (Some(bar), Ok(bytes)) => bar.stop(format!("downloaded {} KiB", bytes.len() / 1024)),
        (Some(bar), Err(_)) => bar.error("download failed"),
        _ => {}
    }
    fetched
}

/// The release target triple for the machine this is running on. Kept in step
/// with `release.yml`'s matrix by hand, which is why an unknown combination is
/// an error naming the archive page rather than a guess.
fn host_target() -> Result<&'static str> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        (os, arch) => bail!(
            "there is no semlith release for {os}/{arch} — see \
             https://github.com/{REPO}/releases"
        ),
    })
}

fn archive_ext() -> &'static str {
    if cfg!(windows) { "zip" } else { "tar.gz" }
}
