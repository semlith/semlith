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
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;
use ureq::ResponseExt;

use crate::{embed, home};

/// The repository the releases come from.
const REPO: &str = "semlith/semlith";

/// The one origin a release is fetched from.
///
/// `SEMLITH_RELEASES_ORIGIN` points the whole flow — the redirect, the archive
/// and `SHA256SUMS` — at `tests/upgrade.rs`'s fixture server, and it is read
/// **only in a debug build**. A release binary has one origin and no way to be
/// told another, because anything that can name the origin of the next semlith
/// binary can replace the semlith binary. Before 0.14.0 that variable was read
/// everywhere, so a line in a shell profile was enough.
///
/// The gate is `debug_assertions` rather than `cfg(test)` because the tests
/// that need it drive the real binary as a subprocess, and a subprocess is not
/// compiled with `cfg(test)` — a gate that looked stricter would only have
/// meant the override was never tested.
fn origin() -> String {
    #[cfg(debug_assertions)]
    if let Ok(from_env) = std::env::var("SEMLITH_RELEASES_ORIGIN") {
        return from_env;
    }
    "https://github.com".to_string()
}

/// Whether this run is pointed at something other than the real origin, which
/// only a debug build can be.
fn overridden() -> bool {
    #[cfg(debug_assertions)]
    {
        std::env::var_os("SEMLITH_RELEASES_ORIGIN").is_some()
    }
    #[cfg(not(debug_assertions))]
    false
}

/// The largest release archive that will be read.
///
/// A release is under 40 MiB; 256 MiB is room for the library that ships beside
/// the binary to grow several times over, and is still a number rather than
/// "whatever the server sends". The read stops one byte past it, so an archive
/// that lies about its length is refused rather than streamed into memory.
const MAX_ARCHIVE: u64 = 256 * 1024 * 1024;

/// `SHA256SUMS` is one line per asset. A release has under ten.
const MAX_SUMS: u64 = 64 * 1024;

/// An ed25519 SSHSIG armours to about 300 bytes.
const MAX_SIG: u64 = 16 * 1024;

/// The public keys a release may be signed with, as `authorized_keys` lines.
///
/// `release.yml` signs `SHA256SUMS` and every archive with `ssh-keygen -Y sign
/// -n file`. OpenSSH's format rather than minisign's because `ssh-keygen -Y
/// verify` ships with macOS, Linux and Windows 10+, so `install.sh` and
/// `install.ps1` check the same signature this command does without a tool the
/// user has to fetch first. The same list is in both installers.
///
/// A rotation ships both keys for one release: add the new line here and in
/// both installers, and publish that release still signed by the old key, so
/// every binary and installer from it on trusts either. Switch the
/// `SEMLITH_SIGNING_KEY` secret to the new key for the next release, and drop
/// the old line in the release after. A binary that skipped the overlap
/// release trusts only the old key and is told to reinstall.
pub const SIGNING_KEYS: &[&str] = &[
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICMTHiHMzeSHzzc+8UDZHUzJcvb6XGUBb+TJdFes91xm semlith-release",
];

/// What `ssh-keygen -lf` prints for the current release key. Asserted against
/// [`SIGNING_KEYS`] by a unit test, so the two cannot drift.
pub const SIGNING_KEY_ID: &str = "SHA256:a5oJmrakIwDHEWTDWqWe51Ttt6m2tQ0dsGzIqyxDuxs";

/// How long to wait for a connection, and for the whole download.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// Whether a tag is one this command will put in a URL or a path.
///
/// Checked before the tag is formatted into either. `v9.9.9-/../x` is a
/// plausible-looking tag and a path traversal, and `latest_tag` reads its
/// answer out of wherever a redirect landed — which is a value the other end
/// chooses.
fn valid_tag(tag: &str) -> bool {
    let Some(rest) = tag.strip_prefix('v') else {
        return false;
    };
    let (version, pre) = match rest.find(['-', '+']) {
        Some(at) => (&rest[..at], Some(&rest[at + 1..])),
        None => (rest, None),
    };
    let numbers: Vec<&str> = version.split('.').collect();
    if numbers.len() != 3
        || !numbers
            .iter()
            .all(|n| !n.is_empty() && n.len() <= 9 && n.bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    match pre {
        None => true,
        Some(pre) => {
            !pre.is_empty()
                && pre
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        }
    }
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
    // Here and not only in the CLI: the portal's Check button reaches this
    // through the daemon, and the Privacy page's switch can airgap it.
    if let Some(reason) = offline() {
        anyhow::bail!("{reason}");
    }
    let installed = env!("CARGO_PKG_VERSION").to_string();
    let latest = latest_tag()?;
    let available = newer(&latest, &installed);
    Ok(Check {
        available,
        blocked: blocked().map(|e| e.to_string()),
        installed,
        latest,
    })
}

/// Download the release for this target, verify the signature on the
/// release's `SHA256SUMS` and the archive against it, and put it where the
/// running binary is. `tag` pins a version;
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
    // Before a request is made and before the tag reaches a path. `--version
    // v9.9.9-/../x` is a plausible-looking tag and a traversal.
    if !valid_tag(&tag) {
        bail!(
            "{tag} is not a version tag — semlith installs vMAJOR.MINOR.PATCH, \
             optionally with a pre-release suffix"
        );
    }
    if !newer(&tag, installed) {
        println!("already on {installed}; {} is not newer", &tag[1..]);
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
    make_work(&work)?;
    let result = swap(&base, &archive, &work, &current, &tag);
    std::fs::remove_dir_all(&work).ok();
    result
}

/// Make the work directory new, owner-only on unix, and refuse one that is
/// already there: a directory or a symlink planted at the name would
/// otherwise be written into and then trusted (audit SEC-004, 2026-10-23).
fn make_work(work: &Path) -> Result<()> {
    // Only unix sets a mode; Windows inherits the parent directory's ACL.
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(work).with_context(|| {
        format!(
            "creating {} (one that already exists is refused, never reused)",
            work.display()
        )
    })
}

/// The download, the signature, the checksum and the rename, with the working
/// directory cleaned up by the caller whichever way this ends.
fn swap(base: &str, archive: &str, work: &Path, current: &Path, tag: &str) -> Result<()> {
    let bytes = get(&format!("{base}/{archive}"), MAX_ARCHIVE, true)?;
    let sums = get(&format!("{base}/SHA256SUMS"), MAX_SUMS, false)?;

    // A checksum fetched from the same release as the archive proves only
    // that the two were uploaded together. The signature is what ties them to
    // the project key baked into this binary, so it is checked before
    // anything in SHA256SUMS is believed, and long before anything is written.
    let sig = get(&format!("{base}/SHA256SUMS.sig"), MAX_SIG, false).with_context(|| {
        format!(
            "{tag} has no SHA256SUMS.sig, so it cannot be verified and nothing was replaced. \
             Releases before v0.40.0 are unsigned; this semlith installs only releases \
             signed by the semlith release key ({SIGNING_KEY_ID})"
        )
    })?;
    let signer = verify_sshsig(&sums, &sig, &trusted_keys())
        .context("the release signature did not verify, so nothing was replaced")?;
    println!("signature verified: SHA256SUMS is signed by {signer}");

    let sums = String::from_utf8(sums).context("SHA256SUMS was not text")?;

    let expected = sums
        .lines()
        .find_map(|line| {
            let (hash, name) = line.split_once("  ")?;
            (name.trim() == archive).then(|| hash.trim().to_ascii_lowercase())
        })
        .with_context(|| format!("{archive} has no line in the release's SHA256SUMS"))?;

    let actual = crate::embed::digest(&bytes);
    if actual != expected {
        bail!(
            "checksum mismatch for {archive}\n  expected {expected}\n  got      {actual}\n\
             nothing was replaced"
        );
    }

    // From the bytes just checked, not a copy written out and opened again: a
    // file on disk can change between the check and the read (audit SEC-004).
    let fresh = unpack(&bytes, work)?;

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

    // From 0.14.0 the Linux archives carry the ONNX Runtime the binary loads at
    // startup, and a binary installed without it cannot embed. The rest of the
    // archive — the README, the licence, the changelog — is documentation and
    // stays out of the bin directory.
    let dir = current.parent().unwrap_or(work);
    for entry in std::fs::read_dir(work).into_iter().flatten().flatten() {
        let path = entry.path();
        if !path.is_file() || !is_library(&path) {
            continue;
        }
        let target = dir.join(entry.file_name());
        std::fs::rename(&path, &target)
            .or_else(|_| std::fs::copy(&path, &target).map(|_| ()))
            .with_context(|| format!("installing {}", target.display()))?;
    }

    println!("installed {} at {}", &tag[1..], current.display());

    // A login service is re-registered by the binary just installed, so its
    // definition is the one that binary writes. Before 0.28.0 that definition
    // asked for background priority, which a daemon cannot lift itself out
    // of; re-registering also restarts the daemon onto the new binary rather
    // than leaving the old one running until the next login.
    // Only a service whose definition names the binary just replaced: a
    // label is global to the session, so "a service is installed" can be
    // somebody else's install -- a developer's own, seen from a test.
    let status = crate::service::status();
    let names_this = status
        .definition
        .as_deref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|text| text.contains(&*current.to_string_lossy()));
    if status.installed && names_this {
        match std::process::Command::new(current)
            .args(["start", "--service"])
            .output()
        {
            Ok(out) if out.status.success() => {
                println!("re-registered the login service with the new binary")
            }
            Ok(out) => println!(
                "the login service could not be re-registered ({}) — run `semlith start --service`",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => println!(
                "the login service could not be re-registered ({e}) — run `semlith start --service`"
            ),
        }
    }
    println!(
        "the previous binary is {} — delete it when you are happy",
        old.display()
    );
    Ok(())
}

/// The wire blobs of the keys a release may be signed with.
///
/// A debug build also trusts the one key `SEMLITH_EXTRA_RELEASE_KEY` names, so
/// the tests can sign a fixture release; like `SEMLITH_RELEASES_ORIGIN` it does
/// not exist in a release build, because anything that can add a trusted key
/// can replace the binary.
fn trusted_keys() -> Vec<Vec<u8>> {
    #[allow(unused_mut)]
    let mut lines: Vec<String> = SIGNING_KEYS.iter().map(|k| k.to_string()).collect();
    #[cfg(debug_assertions)]
    if let Ok(extra) = std::env::var("SEMLITH_EXTRA_RELEASE_KEY") {
        lines.push(extra);
    }
    lines.iter().filter_map(|line| key_blob(line)).collect()
}

/// The wire blob of an `ssh-ed25519 <base64> [comment]` line.
fn key_blob(line: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let mut parts = line.split_whitespace();
    if parts.next()? != "ssh-ed25519" {
        return None;
    }
    base64::engine::general_purpose::STANDARD
        .decode(parts.next()?)
        .ok()
}

/// `SHA256:<unpadded base64>`, the id `ssh-keygen -l` prints for a key.
fn key_id(blob: &[u8]) -> String {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(blob);
    format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest.as_slice())
    )
}

/// Split `n` bytes off the front of `rest`.
fn take<'a>(rest: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if rest.len() < n {
        return None;
    }
    let (head, tail) = rest.split_at(n);
    *rest = tail;
    Some(head)
}

/// An SSH wire `string`: a big-endian u32 length, then that many bytes.
fn string<'a>(rest: &mut &'a [u8]) -> Option<&'a [u8]> {
    let len = u32::from_be_bytes(take(rest, 4)?.try_into().ok()?);
    take(rest, len as usize)
}

/// The one field inside an ed25519 key or signature blob, which is
/// `string "ssh-ed25519" || string <len bytes>` and nothing after.
fn ed25519_field(mut blob: &[u8], len: usize) -> Option<&[u8]> {
    if string(&mut blob)? != b"ssh-ed25519" {
        return None;
    }
    let field = string(&mut blob)?;
    (field.len() == len && blob.is_empty()).then_some(field)
}

/// Verify an armoured `ssh-keygen -Y sign -n file` signature over `message`
/// against `trusted` key blobs, and return the signer's key id.
///
/// The format is OpenSSH's PROTOCOL.sshsig: the blob is `"SSHSIG"`, a u32
/// version of 1, then the strings public key, namespace, reserved, hash
/// algorithm and signature. What the key signed is not the message but
/// `"SSHSIG" || string namespace || string reserved || string hash_alg ||
/// string H(message)`, which is what binds a signature to its namespace: a
/// `-n git` signature from the same key is not a release signature.
fn verify_sshsig(message: &[u8], armoured: &[u8], trusted: &[Vec<u8>]) -> Result<String> {
    use base64::Engine;
    use sha2::{Digest, Sha256, Sha512};

    let malformed = || anyhow::anyhow!("SHA256SUMS.sig is not a well-formed SSH signature");
    let body: String = std::str::from_utf8(armoured)
        .ok()
        .and_then(|t| t.trim().strip_prefix("-----BEGIN SSH SIGNATURE-----"))
        .and_then(|t| t.strip_suffix("-----END SSH SIGNATURE-----"))
        .ok_or_else(malformed)?
        .split_whitespace()
        .collect();
    let blob = base64::engine::general_purpose::STANDARD
        .decode(body)
        .map_err(|_| malformed())?;

    let mut rest = blob.as_slice();
    if take(&mut rest, 6) != Some(b"SSHSIG".as_slice())
        || take(&mut rest, 4) != Some([0, 0, 0, 1].as_slice())
    {
        return Err(malformed());
    }
    let public = string(&mut rest).ok_or_else(malformed)?;
    let namespace = string(&mut rest).ok_or_else(malformed)?;
    let reserved = string(&mut rest).ok_or_else(malformed)?;
    let hash = string(&mut rest).ok_or_else(malformed)?;
    let signature = string(&mut rest).ok_or_else(malformed)?;
    if !rest.is_empty() {
        return Err(malformed());
    }

    if namespace != b"file" {
        bail!(
            "SHA256SUMS.sig was made for namespace {:?}, not \"file\", so it is not a \
             release signature",
            String::from_utf8_lossy(namespace)
        );
    }
    if !trusted.iter().any(|key| key.as_slice() == public) {
        bail!(
            "SHA256SUMS is signed by an untrusted key ({}), not the semlith release key \
             ({SIGNING_KEY_ID}). If semlith has rotated its release key since this binary \
             was built, reinstall with install.sh or install.ps1",
            key_id(public)
        );
    }
    let key = ed25519_field(public, 32).ok_or_else(malformed)?;
    let signature = ed25519_field(signature, 64).ok_or_else(malformed)?;
    let digest = match hash {
        b"sha512" => Sha512::digest(message).to_vec(),
        b"sha256" => Sha256::digest(message).to_vec(),
        other => bail!(
            "SHA256SUMS.sig uses hash {:?}, which an SSH signature may not",
            String::from_utf8_lossy(other)
        ),
    };

    let mut signed = b"SSHSIG".to_vec();
    for field in [namespace, reserved, hash, digest.as_slice()] {
        signed.extend_from_slice(&(field.len() as u32).to_be_bytes());
        signed.extend_from_slice(field);
    }
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(&signed, signature)
        .map_err(|_| {
            anyhow::anyhow!(
                "the signature does not match SHA256SUMS: the file was changed after it \
                 was signed"
            )
        })?;
    Ok(key_id(public))
}

/// Unpack the release archive, in this process.
///
/// It used to be `Command::new("tar")`, which is a program chosen by `PATH` on
/// the one code path that replaces the running binary — and a `PATH` that a
/// shell profile, a launcher or another installer can have written. It is now
/// `flate2` plus the tar reader below on unix, and the zip reader already in
/// the tree for `.docx` on Windows. Neither reads anything outside `into`.
///
/// Every regular file in the archive is extracted, not only the binary: from
/// 0.14.0 the Linux archives carry `libonnxruntime.so` beside it. Returns the
/// binary.
fn unpack(archive: &[u8], into: &Path) -> Result<PathBuf> {
    let name = if cfg!(windows) {
        "semlith.exe"
    } else {
        "semlith"
    };

    if cfg!(windows) {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
            .context("reading the release zip")?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).context("reading a zip entry")?;
            if entry.is_dir() {
                continue;
            }
            let Ok(raw) = entry.name() else {
                continue;
            };
            let Some(file_name) = strip_leading_dir(&raw).map(str::to_string) else {
                continue;
            };
            let out = into.join(&file_name);
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .with_context(|| format!("unpacking {file_name}"))?;
            std::fs::write(&out, bytes).with_context(|| format!("writing {}", out.display()))?;
        }
    } else {
        untar(flate2::read::GzDecoder::new(archive), into)?;
    }

    let out = into.join(name);
    if !out.exists() {
        bail!("the release archive has no {name}");
    }
    Ok(out)
}

/// Whether a file the archive carried is a library the binary loads.
///
/// `libonnxruntime.so.1.24.4` is the shape that matters on Linux, so the
/// extension is not enough on its own — the version is the last component.
fn is_library(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    name.ends_with(".dylib") || name.ends_with(".dll") || name.contains(".so")
}

/// The name inside the archive's one top-level directory, or nothing.
///
/// The release archives hold exactly one directory — `semlith-vX.Y.Z-<target>/`
/// — and everything is inside it. A path with no directory, more than one
/// level, a `..` component, an absolute root or a backslash is not something
/// these archives contain, so it is skipped rather than interpreted. Every
/// component also has to pass [`crate::packs::inside`], so a colon or a
/// Windows device name is refused here as it is for a pack (audit SEC-014).
fn strip_leading_dir(path: &str) -> Option<&str> {
    if path.contains('\\') || path.starts_with('/') {
        return None;
    }
    let (_, rest) = path.split_once('/')?;
    if rest.is_empty() || rest.contains('/') || rest == ".." || rest == "." {
        return None;
    }
    crate::packs::inside(path)?;
    Some(rest)
}

/// Enough of tar to read a release archive: the regular files in it, one
/// directory deep, into `into`.
///
/// A tar file is 512-byte headers, each followed by its file's bytes rounded up
/// to 512. That is the whole format this needs, and writing it out is smaller
/// than the surface of a crate that reads every tar there has ever been.
fn untar(mut reader: impl Read, into: &Path) -> Result<()> {
    let mut header = [0u8; 512];
    loop {
        if !read_exact_or_end(&mut reader, &mut header)? {
            return Ok(());
        }
        // Two zero blocks end the archive; one is enough to stop on.
        if header.iter().all(|b| *b == 0) {
            return Ok(());
        }

        let name = field(&header[0..100]);
        let size = octal(&header[124..136]).context("a tar header has no readable size")?;
        let kind = header[156];

        // '0' and NUL are a regular file; everything else — directories, links,
        // the GNU long-name headers — is skipped over rather than acted on.
        let wanted = matches!(kind, b'0' | 0)
            .then(|| strip_leading_dir(&name))
            .flatten()
            .map(|n| into.join(n));

        let mut left = size;
        match wanted {
            Some(path) => {
                let mut out = std::fs::File::create(&path)
                    .with_context(|| format!("writing {}", path.display()))?;
                let mut buffer = [0u8; 64 * 1024];
                while left > 0 {
                    let want = (left as usize).min(buffer.len());
                    reader
                        .read_exact(&mut buffer[..want])
                        .context("the release archive ended mid-file")?;
                    use std::io::Write;
                    out.write_all(&buffer[..want])
                        .with_context(|| format!("writing {}", path.display()))?;
                    left -= want as u64;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = octal(&header[100..108]).unwrap_or(0o644) as u32;
                    // The archive's own mode, masked: a release archive should
                    // never carry setuid, and honouring one would be the only
                    // way this reader could make things worse than `tar` did.
                    let _ = std::fs::set_permissions(
                        &path,
                        std::fs::Permissions::from_mode(mode & 0o755),
                    );
                }
            }
            None => skip(&mut reader, left)?,
        }

        // Files are padded to a 512-byte boundary.
        let padding = (512 - (size % 512)) % 512;
        skip(&mut reader, padding)?;
    }
}

/// Fill `buffer`, or report that the reader ended cleanly at a boundary.
fn read_exact_or_end(reader: &mut impl Read, buffer: &mut [u8]) -> Result<bool> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..])? {
            0 if filled == 0 => return Ok(false),
            0 => bail!("the release archive ended mid-header"),
            n => filled += n,
        }
    }
    Ok(true)
}

fn skip(reader: &mut impl Read, mut bytes: u64) -> Result<()> {
    let mut sink = [0u8; 64 * 1024];
    while bytes > 0 {
        let want = (bytes as usize).min(sink.len());
        reader
            .read_exact(&mut sink[..want])
            .context("the release archive ended mid-file")?;
        bytes -= want as u64;
    }
    Ok(())
}

/// A NUL-terminated tar header field.
fn field(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// A tar header's octal number field.
fn octal(bytes: &[u8]) -> Option<u64> {
    let text = field(bytes);
    let digits = text.trim().trim_end_matches(' ');
    if digits.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(digits, 8).ok()
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
        "airgap is on (SEMLITH_AIRGAP, --airgap or the Privacy page), so this run will \
         not reach the network. Upgrade on a connected machine and copy the binary across.",
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
    if !writable(&current) {
        return Some("the semlith binary is not writable by this user");
    }
    None
}

/// Whether this user can actually replace the file.
///
/// `permissions().readonly()` reads the owner's bits, which answers a different
/// question: a root-owned `0755` binary is not read-only and is not writable by
/// the person running this either. `access(W_OK)` asks the kernel the question
/// that matters — can *this* process write here — so a refusal happens before
/// the binary is moved aside rather than halfway through the swap.
#[cfg(unix)]
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // The directory has to be writable too: replacing a file is unlinking and
    // creating one, which is a directory operation.
    let Some(parent) = path.parent() else {
        return false;
    };
    let Ok(c_parent) = std::ffi::CString::new(parent.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: both pointers are NUL-terminated C strings that outlive the call,
    // and `access` reads them and returns.
    unsafe {
        libc::access(c_path.as_ptr(), libc::W_OK) == 0
            && libc::access(c_parent.as_ptr(), libc::W_OK) == 0
    }
}

#[cfg(not(unix))]
fn writable(path: &Path) -> bool {
    !std::fs::metadata(path)
        .map(|m| m.permissions().readonly())
        .unwrap_or(true)
}

/// An install this command owns is one that sits in the bin directory the
/// scripts write to. Anything else — cargo's bin, a package manager's prefix —
/// belongs to whatever put it there.
///
/// Both sides are canonicalised before they are compared. A textual comparison
/// answers "no" for a path reached through a symlink or carrying a `..`, which
/// is the ordinary shape of `/usr/local/bin` on macOS and of a home directory
/// on a machine with `/home` symlinked — and "no" here means a user who
/// installed with the script is told to upgrade the way they installed.
///
/// `SEMLITH_HOME` still moves the bin directory, because `install.sh` documents
/// it as the way to install somewhere else and a user who installed that way
/// must be able to upgrade. What it cannot do is make this command replace a
/// binary somewhere else: the running binary's own directory is what is
/// compared, so pointing the variable at another directory makes `owned` false
/// rather than making that directory upgradable.
fn owned(current: &Path) -> bool {
    let Some(parent) = current.parent() else {
        return false;
    };
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // An unresolvable home owns nothing: the question is whether this binary
    // sits in the directory the installer manages, and with no home there is no
    // such directory to be in.
    home::bin_dir().is_ok_and(|bin| real(parent) == real(&bin))
}

/// `releases/latest` redirects to the newest tag; the tag is the last segment
/// of wherever it lands. Reading the redirect rather than the API keeps this
/// unauthenticated and off the API rate limit.
fn latest_tag() -> Result<String> {
    let url = format!("{}/{REPO}/releases/latest", origin());
    crate::add::note_outbound("upgrade", &url);
    let response = agent()
        .get(&url)
        .call()
        .with_context(|| format!("asking {url} which release is newest"))?;
    let landed = response.get_uri().to_string();
    let tag = landed
        .rsplit('/')
        .next()
        .with_context(|| format!("{url} redirected to {landed}, which names no tag"))?;
    // The tag is whatever the other end's redirect landed on, and it is about
    // to be formatted into a URL and a path. Checked here so a redirect cannot
    // choose either.
    if !valid_tag(tag) {
        bail!("{url} redirected to {landed}, which does not name a version tag");
    }
    Ok(tag.to_string())
}

/// An agent that will not be talked down to plain HTTP and will not wait
/// forever.
///
/// `https_only` is what makes a redirect to `http://` an error instead of a
/// downgrade nobody sees: the checksum would still be checked, but it would be
/// checked against a `SHA256SUMS` fetched over the same downgraded connection.
/// The only build that turns it off is one pointed at the fixture server, which
/// speaks plain HTTP on loopback and exists only in a debug build.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(!overridden())
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .build()
        .into()
}

/// Fetch at most `cap` bytes, and refuse anything longer rather than reading it.
///
/// `read_to_end` on a response body is a promise to allocate whatever the other
/// end feels like sending. The reader stops one byte past the cap so "exactly
/// at the limit" and "longer than the limit" are told apart.
fn get(url: &str, cap: u64, show_progress: bool) -> Result<Vec<u8>> {
    let bar = show_progress.then(|| {
        let bar = cliclack::spinner();
        bar.start(format!("downloading {url}"));
        bar
    });
    let fetched = (|| -> Result<Vec<u8>> {
        crate::add::note_outbound("upgrade", url);
        let mut response = agent()
            .get(url)
            .call()
            .with_context(|| format!("fetching {url}"))?;
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(cap + 1)
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading {url}"))?;
        if bytes.len() as u64 > cap {
            bail!(
                "{url} is larger than the {} MiB semlith will download for a release; \
                 nothing was written",
                cap / (1024 * 1024)
            );
        }
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

/// Whether the release is actually ahead of what is running. Inequality is not
/// enough: a binary built between tags is newer than `releases/latest`, and
/// telling its user to "upgrade" to the version they already passed is how a
/// check earns being ignored. A version neither side can parse is treated as
/// not newer, because offering to replace a binary is the answer that does
/// damage when it is wrong.
fn newer(latest: &str, installed: &str) -> bool {
    match (semver(latest), semver(installed)) {
        (Some(l), Some(i)) => l > i,
        _ => false,
    }
}

fn semver(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.trim_start_matches('v').split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    // A tag may carry a pre-release suffix; the patch number is what precedes it.
    let patch = parts.next()?.split(['-', '+']).next()?.parse().ok()?;
    Some((major, minor, patch))
}

fn archive_ext() -> &'static str {
    if cfg!(windows) { "zip" } else { "tar.gz" }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tag becomes a URL and a path, so what it may contain is the whole of
    /// what this command will reach for.
    #[test]
    fn only_a_version_tag_is_a_tag() {
        assert!(valid_tag("v0.14.0"));
        assert!(valid_tag("v1.0.0"));
        assert!(valid_tag("v99.0.0"));
        assert!(valid_tag("v0.14.0-rc.1"));
        assert!(valid_tag("v0.14.0+build.5"));

        // The traversal that looks like a tag.
        assert!(!valid_tag("v9.9.9-/../x"));
        assert!(!valid_tag("v9.9.9/../../etc"));
        assert!(!valid_tag("../../etc/passwd"));
        // A tag is not a URL, a host, or a query.
        assert!(!valid_tag("v1.0.0?x=1"));
        assert!(!valid_tag("v1.0.0#x"));
        assert!(!valid_tag("https://elsewhere/v1.0.0"));
        // Shapes that are not three numbers.
        assert!(!valid_tag("v1.0"));
        assert!(!valid_tag("v1.0.0.0"));
        assert!(!valid_tag("vx.y.z"));
        assert!(!valid_tag("1.0.0"));
        assert!(!valid_tag(""));
        assert!(!valid_tag("v"));
        assert!(!valid_tag("v1.0.0 "));
        assert!(!valid_tag("v1.0.0-"));
        // A number long enough to be an overflow attempt rather than a version.
        assert!(!valid_tag("v1.0.9999999999"));
    }

    /// The reader writes into one directory and the archive does not get to
    /// choose which.
    #[test]
    fn an_archive_entry_names_one_file_in_one_directory() {
        assert_eq!(
            strip_leading_dir("semlith-v0.14.0-x86_64-unknown-linux-gnu/semlith"),
            Some("semlith")
        );
        assert_eq!(
            strip_leading_dir("semlith-v0.14.0-x86_64-unknown-linux-gnu/libonnxruntime.so"),
            Some("libonnxruntime.so")
        );

        assert_eq!(strip_leading_dir("dir/../../etc/passwd"), None);
        assert_eq!(strip_leading_dir("dir/nested/file"), None);
        assert_eq!(strip_leading_dir("/etc/passwd"), None);
        assert_eq!(strip_leading_dir("dir/.."), None);
        assert_eq!(strip_leading_dir("bare-file"), None);
        assert_eq!(strip_leading_dir("dir\\windows\\path"), None);
        assert_eq!(strip_leading_dir("dir/"), None);
    }

    /// Audit SEC-014 (2026-10-23): the release reader refuses what the pack
    /// reader refuses — a colon, which on Windows is a drive or a stream, and
    /// a device name, which on Windows is not a file at all.
    #[test]
    fn an_archive_entry_with_a_colon_or_a_device_name_is_refused() {
        for name in [
            "top/C:evil",
            "top/semlith.exe:x",
            "top/CON",
            "top/nul.txt",
            "top/Aux",
            "top/com1.log",
            "top/LPT9",
        ] {
            assert_eq!(strip_leading_dir(name), None, "{name} was accepted");
        }
        for name in ["top/console.txt", "top/nulls", "top/COM10"] {
            assert!(strip_leading_dir(name).is_some(), "{name} was refused");
        }
    }

    /// Audit SEC-004 (2026-10-23): the release is unpacked from the bytes
    /// whose checksum was checked, so the archive is never written to disk
    /// and read back, and the work directory holds only what it carried.
    #[test]
    fn a_release_is_unpacked_from_the_bytes_that_were_checked() {
        let name = if cfg!(windows) {
            "semlith.exe"
        } else {
            "semlith"
        };
        let entry = format!("semlith-v9.9.9-x/{name}");
        let mut bytes = Vec::new();
        if cfg!(windows) {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            zip.start_file(entry.as_str(), zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut zip, b"fresh").unwrap();
            zip.finish().unwrap();
        } else {
            let mut header = [0u8; 512];
            header[..entry.len()].copy_from_slice(entry.as_bytes());
            header[100..107].copy_from_slice(b"0000755");
            header[124..135].copy_from_slice(b"00000000005");
            header[156] = b'0';
            let mut tar = header.to_vec();
            tar.extend(b"fresh");
            tar.resize(1024 + 1024, 0);
            let mut gz = flate2::write::GzEncoder::new(Vec::new(), Default::default());
            std::io::Write::write_all(&mut gz, &tar).unwrap();
            bytes = gz.finish().unwrap();
        }

        let t = tempfile::tempdir().unwrap();
        let out = unpack(&bytes, t.path()).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"fresh");
        let held: Vec<_> = std::fs::read_dir(t.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            held,
            [name],
            "the work directory held more than the release"
        );
    }

    /// Audit SEC-004 (2026-10-23): the work directory is made new by this
    /// run, so one that is already there — planted, or a symlink to somewhere
    /// else — is refused rather than written into.
    #[test]
    fn a_work_directory_that_already_exists_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let existing = t.path().join(".semlith-upgrade-1");
        std::fs::create_dir(&existing).unwrap();
        assert!(
            make_work(&existing).is_err(),
            "an existing directory was used"
        );

        #[cfg(unix)]
        {
            let elsewhere = t.path().join("elsewhere");
            std::fs::create_dir(&elsewhere).unwrap();
            let link = t.path().join(".semlith-upgrade-2");
            std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
            assert!(make_work(&link).is_err(), "a symlinked directory was used");
        }

        let fresh = t.path().join(".semlith-upgrade-3");
        make_work(&fresh).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&fresh).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "the work directory is {mode:o}");
        }
    }

    /// What ships beside the binary and what does not.
    #[test]
    fn only_a_library_is_installed_beside_the_binary() {
        assert!(is_library(Path::new("libonnxruntime.so")));
        assert!(is_library(Path::new("libonnxruntime.so.1.24.4")));
        assert!(is_library(Path::new("libonnxruntime.dylib")));
        assert!(is_library(Path::new("onnxruntime.dll")));

        assert!(!is_library(Path::new("semlith")));
        assert!(!is_library(Path::new("README.md")));
        assert!(!is_library(Path::new("LICENSE")));
        assert!(!is_library(Path::new("CHANGELOG.md")));
    }

    /// The release binary has one origin. The override exists for the fixture
    /// server and is compiled out of what a user installs, so this asserts the
    /// thing that actually ships — and only in the build where it is true.
    #[test]
    #[cfg(not(debug_assertions))]
    fn a_release_build_has_one_origin_and_no_way_to_be_told_another() {
        // SAFETY: this test is the only thing in this binary reading the
        // variable, and it is reading it to prove it is not read.
        unsafe { std::env::set_var("SEMLITH_RELEASES_ORIGIN", "http://elsewhere.example") };
        assert_eq!(origin(), "https://github.com");
        assert!(!overridden());
        unsafe { std::env::remove_var("SEMLITH_RELEASES_ORIGIN") };
    }

    /// The id `semlith --version` will print is the id of the key that ships.
    #[test]
    fn the_signing_key_id_is_the_shipped_keys_fingerprint() {
        assert_eq!(key_id(&key_blob(SIGNING_KEYS[0]).unwrap()), SIGNING_KEY_ID);
    }

    /// A signature made by `ssh-keygen -Y sign -n file` with a throwaway key,
    /// once, so the parser is checked against OpenSSH's own output on every
    /// run without needing ssh-keygen installed.
    const VECTOR_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKrKLCA4hwnLZ/jBh0rO4o6fQNdGPlfuSXzNvAinLzDC vector";
    const VECTOR_MESSAGE: &[u8] = b"abc123  semlith-v9.9.9-x.tar.gz\n";
    const VECTOR_SIG: &str = "-----BEGIN SSH SIGNATURE-----
U1NIU0lHAAAAAQAAADMAAAALc3NoLWVkMjU1MTkAAAAgqsosIDiHCctn+MGHSs7ijp9A10
Y+V+5JfM28CKcvMMIAAAAEZmlsZQAAAAAAAAAGc2hhNTEyAAAAUwAAAAtzc2gtZWQyNTUx
OQAAAEBPVD44/75bMXbjPojadfnMt2QZclTsusuY/ZDdgnJhYZvGLg51cdXKFl8W5q+2gb
SvryiS85k74I0alntQywcG
-----END SSH SIGNATURE-----
";

    #[test]
    fn an_openssh_signature_verifies_and_names_its_key() {
        let trusted = vec![key_blob(VECTOR_KEY).unwrap()];
        let id = verify_sshsig(VECTOR_MESSAGE, VECTOR_SIG.as_bytes(), &trusted).unwrap();
        assert_eq!(id, "SHA256:HXLk6RetMrW9TMvxXn4n6NgMaT0iyNnWzK+Bnic8HFc");
    }

    #[test]
    fn a_signature_fails_closed() {
        let trusted = vec![key_blob(VECTOR_KEY).unwrap()];
        let shipped = vec![key_blob(SIGNING_KEYS[0]).unwrap()];
        let err = |message: &[u8], sig: &str, keys: &[Vec<u8>]| {
            format!(
                "{:#}",
                verify_sshsig(message, sig.as_bytes(), keys).unwrap_err()
            )
        };

        // One byte of the message changed.
        assert!(
            err(b"abc124  semlith-v9.9.9-x.tar.gz\n", VECTOR_SIG, &trusted)
                .contains("does not match")
        );
        // A valid signature by a key this binary does not trust.
        assert!(err(VECTOR_MESSAGE, VECTOR_SIG, &shipped).contains("untrusted key"));
        // Not an SSH signature at all, or one cut short.
        assert!(err(VECTOR_MESSAGE, "", &trusted).contains("well-formed"));
        assert!(
            err(
                VECTOR_MESSAGE,
                &VECTOR_SIG.replace("SvryiS85k74I0alntQywcG\n", ""),
                &trusted
            )
            .contains("well-formed")
        );
        // The namespace is part of what was signed: relabelling a signature
        // `file` -> `fild` changes the signed bytes, and "file" is required.
        let relabelled = {
            use base64::Engine;
            let body: String = VECTOR_SIG
                .lines()
                .filter(|l| !l.starts_with("-----"))
                .collect();
            let mut blob = base64::engine::general_purpose::STANDARD
                .decode(body)
                .unwrap();
            let at = blob.windows(4).position(|w| w == b"file").unwrap();
            blob[at + 3] = b'd';
            format!(
                "-----BEGIN SSH SIGNATURE-----\n{}\n-----END SSH SIGNATURE-----\n",
                base64::engine::general_purpose::STANDARD.encode(blob)
            )
        };
        assert!(err(VECTOR_MESSAGE, &relabelled, &trusted).contains("namespace"));
    }

    #[test]
    fn only_a_higher_version_counts_as_an_upgrade() {
        assert!(newer("v0.11.0", "0.10.0"));
        assert!(newer("v0.10.1", "0.10.0"));
        assert!(newer("v1.0.0", "0.99.99"));
        assert!(!newer("v0.10.0", "0.10.0"));
        // The case the portal found: a local build ahead of the newest tag.
        assert!(!newer("v0.9.0", "0.10.0"));
        assert!(!newer("not-a-version", "0.10.0"));
    }
}
