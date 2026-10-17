//! The components the 0.32.0 lanes run on, each pinned by URL and SHA-256.
//!
//! Nothing here is linked into the binary: a pack is downloaded into the model
//! cache the first time somebody turns its lane on (or, for the Neural Engine
//! on Apple silicon, by `semlith setup`), checked against the digest in this
//! file, unpacked, and stamped. A pack whose digest does not match is deleted,
//! never kept. `--airgap` refuses every download and names what it would have
//! fetched.
//!
//! | pack | what | from |
//! |---|---|---|
//! | `coreml` | granite as Core ML models, Neural Engine layout and GPU layout | this repository's `pack-coreml-v*` release, built by `packs.yml` |
//! | `llama` | `llama-server` for this platform and granite as GGUF f16 | ggml-org's llama.cpp release, and this repository's `pack-llama-v*` |
//! | `trt` | NVIDIA's TensorRT for RTX plugin execution provider | NVIDIA's wheel on PyPI |
//! | `openvino` | Intel's OpenVINO plugin execution provider | Intel's wheel on PyPI |

use anyhow::{Context, Result, bail};
use std::io::Read;
use std::path::{Path, PathBuf};

/// One file a pack is made of.
#[derive(Debug, Clone, Copy)]
pub struct Asset {
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
    pub form: Form,
}

/// What to do with a file once it is here.
#[derive(Debug, Clone, Copy)]
pub enum Form {
    /// A zip with one top-level directory: unpacked, that directory dropped.
    Zip,
    /// A zip whose files under `prefix` are wanted and nothing else, flattened
    /// into the pack directory: a Python wheel's native libraries.
    Wheel { prefix: &'static str },
    /// A tar.gz with one top-level directory: unpacked, that directory
    /// dropped. llama.cpp's Linux and macOS archives.
    TarGz,
    /// Kept as it is, under this name.
    File { name: &'static str },
}

/// A pack for this platform.
#[derive(Debug, Clone, Copy)]
pub struct Pack {
    pub name: &'static str,
    pub version: &'static str,
    pub assets: &'static [Asset],
}

impl Pack {
    pub fn bytes(&self) -> u64 {
        self.assets.iter().map(|a| a.size).sum()
    }

    /// Where it lives in the model cache.
    pub fn dir(&self, cache: &Path) -> PathBuf {
        crate::accel::component_dir(cache, &format!("{}-{}", self.name, self.version))
    }
}

/// A development override naming an unpacked pack directory, read only by
/// [`installed`]: `SEMLITH_PACK_<NAME>` (`SEMLITH_PACK_COREML`). Not part of
/// the documented environment; it is how a pack is tried before it is
/// published and pinned.
fn override_for(name: &str) -> Option<PathBuf> {
    // Compiled out of the release binary (audit S20): it loads libraries and
    // runs a server from wherever it points, with no digest.
    #[cfg(any(debug_assertions, feature = "dev-overrides"))]
    {
        std::env::var_os(format!("SEMLITH_PACK_{}", name.to_ascii_uppercase())).map(PathBuf::from)
    }
    #[cfg(not(any(debug_assertions, feature = "dev-overrides")))]
    {
        let _ = name;
        None
    }
}

const STAMP: &str = ".semlith-verified";

/// The unpacked pack, if it is installed and is still what was checked.
///
/// The archive's digest is checked once, at the download. What is loaded is
/// the unpacked files, so the stamp records each one's size, mtime and digest
/// and every load compares them (audit S15): a library swapped in the cache
/// after the check is a pack that is not installed, fetched again whole.
/// Size and mtime on the hot path, the digest only when either moved.
pub fn installed(cache: &Path, pack: &Pack) -> Option<PathBuf> {
    if let Some(dir) = override_for(pack.name) {
        return dir.exists().then_some(dir);
    }
    let dir = pack.dir(cache);
    let stamp = std::fs::read_to_string(dir.join(STAMP)).ok()?;
    match serde_json::from_str::<Manifest>(&stamp) {
        Ok(manifest) if manifest.version == pack.version && manifest.holds(&dir) => Some(dir),
        Ok(_) => None,
        // A stamp from before 0.41.0 is the version alone. The archive was
        // checked when it was written; what is on disk now is recorded, and
        // checked from here on.
        // ponytail: trust on first use for pre-0.41.0 packs; re-fetch instead
        // if a pack cache that old has to be distrusted.
        Err(_) if stamp.trim() == pack.version => {
            let manifest = Manifest::of(&dir, pack.version)?;
            crate::home::write_private(&dir.join(STAMP), &serde_json::to_vec(&manifest).ok()?)
                .ok()?;
            Some(dir)
        }
        Err(_) => None,
    }
}

/// Every unpacked file of a pack, as its stamp records it.
#[derive(serde::Serialize, serde::Deserialize)]
struct Manifest {
    version: String,
    /// Relative path to `(size, mtime in ns, blake3)`.
    files: std::collections::BTreeMap<String, (u64, i128, String)>,
}

impl Manifest {
    fn of(dir: &Path, version: &str) -> Option<Self> {
        let mut files = std::collections::BTreeMap::new();
        for path in pack_files(dir) {
            let rel = path
                .strip_prefix(dir)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            let (size, mtime) = fingerprint(&path)?;
            files.insert(rel, (size, mtime, digest_of(&path)?));
        }
        Some(Self {
            version: version.to_string(),
            files,
        })
    }

    /// Whether the directory holds exactly these files, unchanged.
    fn holds(&self, dir: &Path) -> bool {
        let on_disk = pack_files(dir);
        on_disk.len() == self.files.len()
            && on_disk.iter().all(|path| {
                let Some(rel) = path.strip_prefix(dir).ok() else {
                    return false;
                };
                let rel = rel.to_string_lossy().replace('\\', "/");
                let Some((size, mtime, digest)) = self.files.get(&rel) else {
                    return false;
                };
                match fingerprint(path) {
                    Some(now) if now == (*size, *mtime) => true,
                    Some(_) => digest_of(path).is_some_and(|d| d == *digest),
                    None => false,
                }
            })
    }
}

/// Every regular file under a pack directory, its stamp and leftovers aside.
fn pack_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        for entry in std::fs::read_dir(&at).into_iter().flatten().flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n != STAMP) {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

fn fingerprint(path: &Path) -> Option<(u64, i128)> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos() as i128;
    Some((meta.len(), mtime))
}

fn digest_of(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(hasher.finalize().to_hex().to_string())
}

/// Fetch, check and unpack a pack, reporting progress in percent. A no-op
/// for one already installed.
pub fn fetch(cache: &Path, pack: &Pack, progress: &mut dyn FnMut(u8)) -> Result<PathBuf> {
    if let Some(dir) = installed(cache, pack) {
        return Ok(dir);
    }
    let dir = pack.dir(cache);
    if crate::embed::airgap() {
        bail!(
            "{} is set and the {} pack is not in {} — pre-seed it on a connected machine, \
             or drop --airgap",
            crate::embed::AIRGAP_ENV,
            pack.name,
            crate::plain(&dir.display().to_string())
        );
    }
    if pack.assets.is_empty() {
        bail!(
            "unavailable — no {} pack is published for this platform",
            pack.name
        );
    }
    crate::embed::check_cache_dir(cache)?;
    // A half-unpacked directory from a run that died is not a pack.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let total = pack.bytes().max(1);
    let mut done = 0u64;
    for asset in pack.assets {
        let file = dir.join(format!(
            ".download-{}",
            asset.sha256.get(..12).unwrap_or("pack")
        ));
        let fetched = crate::gpu::download(asset.url, &file, asset.sha256, &mut |n| {
            done += n;
            progress(((done * 100) / total).min(100) as u8);
        });
        if let Err(e) = fetched {
            // Nothing of a pack that failed its digest is kept.
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        let unpacked = match asset.form {
            Form::Zip => unzip(&file, &dir, None),
            Form::Wheel { prefix } => unzip(&file, &dir, Some(prefix)),
            Form::TarGz => untar_gz(&file, &dir),
            Form::File { name } => {
                std::fs::rename(&file, dir.join(name)).with_context(|| format!("placing {name}"))
            }
        };
        if let Err(e) = unpacked {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        let _ = std::fs::remove_file(&file);
    }
    let manifest = Manifest::of(&dir, pack.version)
        .with_context(|| format!("recording the {} pack's files", pack.name))?;
    crate::home::write_private(&dir.join(STAMP), &serde_json::to_vec(&manifest)?)?;
    Ok(dir)
}

/// Delete a pack, and say how many bytes that freed.
pub fn remove(cache: &Path, pack: &Pack) -> Result<u64> {
    let dir = pack.dir(cache);
    let bytes = crate::accel::dir_bytes(&dir);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("removing {}", crate::plain(&dir.display().to_string())))?;
    }
    Ok(bytes)
}

/// A relative path inside an archive, refused if it could land anywhere but
/// under the pack directory.
fn inside(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::Normal(p) => out.push(p),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// The path with its first component dropped: the archive's one top-level
/// directory.
fn below_top(path: &Path) -> Option<PathBuf> {
    let mut parts = path.components();
    parts.next()?;
    let rest: PathBuf = parts.collect();
    (!rest.as_os_str().is_empty()).then_some(rest)
}

fn unzip(archive: &Path, into: &Path, wheel: Option<&str>) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).context("reading the pack archive")?;
    let mut placed = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        let Ok(raw) = entry.name() else {
            continue;
        };
        let Some(name) = inside(&raw) else {
            continue;
        };
        let target = match wheel {
            // A wheel's libraries sit under the package's own directory; the
            // rest is Python.
            Some(prefix) => match name.strip_prefix(prefix) {
                Ok(rest) if rest.components().count() == 1 => into.join(rest),
                _ => continue,
            },
            None => match below_top(&name) {
                Some(rest) => into.join(rest),
                None => continue,
            },
        };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&target)
            .with_context(|| format!("writing {}", target.display()))?;
        std::io::copy(&mut entry, &mut out)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode & 0o755));
        }
        placed += 1;
    }
    if placed == 0 {
        bail!("the pack archive held nothing to unpack");
    }
    Ok(())
}

/// Enough of tar for a pack: regular files, nested, below one top-level
/// directory. Links, devices and anything that would leave the directory are
/// skipped, not followed.
fn untar_gz(archive: &Path, into: &Path) -> Result<()> {
    let mut reader = flate2::read::GzDecoder::new(std::fs::File::open(archive)?);
    let mut header = [0u8; 512];
    let mut long_name: Option<String> = None;
    let mut placed = 0;
    loop {
        if !fill(&mut reader, &mut header)? || header.iter().all(|b| *b == 0) {
            break;
        }
        let size = octal(&header[124..136]).context("a tar header has no readable size")?;
        let kind = header[156];
        let padded = size.div_ceil(512) * 512;
        // GNU long names: the next header's name is this entry's contents.
        if kind == b'L' {
            let mut name = vec![0u8; padded as usize];
            reader.read_exact(&mut name)?;
            long_name = Some(field(&name[..size as usize]));
            continue;
        }
        let name = long_name.take().unwrap_or_else(|| {
            let prefix = field(&header[345..500]);
            let base = field(&header[0..100]);
            if prefix.is_empty() {
                base
            } else {
                format!("{prefix}/{base}")
            }
        });
        // A link to a sibling by bare name (`libggml.0.dylib` pointing at
        // `libggml.0.25.1.dylib`) is recreated: llama.cpp's binaries are linked
        // against those names. Any other link is skipped, never followed.
        if kind == b'2' {
            let link = field(&header[157..257]);
            let sibling = matches!(
                Path::new(&link).components().collect::<Vec<_>>()[..],
                [std::path::Component::Normal(_)]
            );
            #[cfg(unix)]
            if sibling
                && let Some(path) = inside(&name)
                    .and_then(|p| below_top(&p))
                    .map(|rest| into.join(rest))
            {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let _ = std::fs::remove_file(&path);
                std::os::unix::fs::symlink(&link, &path)?;
            }
            #[cfg(not(unix))]
            let _ = sibling;
            std::io::copy(&mut (&mut reader).take(padded), &mut std::io::sink())?;
            continue;
        }
        let target = matches!(kind, b'0' | 0)
            .then(|| inside(&name).and_then(|p| below_top(&p)))
            .flatten()
            .map(|rest| into.join(rest));
        match target {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut out = std::fs::File::create(&path)?;
                std::io::copy(&mut (&mut reader).take(size), &mut out)?;
                std::io::copy(&mut (&mut reader).take(padded - size), &mut std::io::sink())?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = octal(&header[100..108]).unwrap_or(0o644) as u32;
                    let _ = std::fs::set_permissions(
                        &path,
                        std::fs::Permissions::from_mode(mode & 0o755),
                    );
                }
                placed += 1;
            }
            None => {
                std::io::copy(&mut (&mut reader).take(padded), &mut std::io::sink())?;
            }
        }
    }
    if placed == 0 {
        bail!("the pack archive held nothing to unpack");
    }
    Ok(())
}

fn fill(reader: &mut impl Read, buffer: &mut [u8]) -> Result<bool> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..])? {
            0 if filled == 0 => return Ok(false),
            0 => bail!("the pack archive ended mid-header"),
            n => filled += n,
        }
    }
    Ok(true)
}

fn field(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

fn octal(bytes: &[u8]) -> Option<u64> {
    let text = field(bytes);
    u64::from_str_radix(text.trim(), 8).ok()
}

// ------------------------------------------------------------- the pins

/// granite as Core ML models: the Neural Engine lane and the macOS GPU lane.
/// Built by `.github/workflows/packs.yml` from `packs/coreml/`.
pub fn coreml() -> Pack {
    Pack {
        name: "coreml",
        version: COREML_VERSION,
        assets: if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            COREML_ASSETS
        } else {
            &[]
        },
    }
}

pub const COREML_VERSION: &str = "2";

const COREML_ASSETS: &[Asset] = &[Asset {
    url: "https://github.com/semlith/semlith/releases/download/pack-coreml-v2/semlith-coreml-2.zip",
    sha256: "799e4f7e8a4b1aab8335ab969bc385fcfb7baa8d62bfd14e02c8be9b0188a082",
    size: 147_950_175,
    form: Form::Zip,
}];

#[cfg(test)]
mod tests {
    use super::*;

    /// Audit S15: a pack's unpacked files are checked on every load, not only
    /// the archive at download. A library swapped in the cache afterwards is a
    /// pack that is not installed; a stamp from before 0.41.0 is upgraded.
    #[test]
    fn a_pack_file_changed_after_its_check_is_not_installed() {
        let cache = tempfile::tempdir().unwrap();
        let pack = Pack {
            name: "testpack",
            version: "1",
            assets: &[],
        };
        let dir = pack.dir(cache.path());
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("lib").join("libx.dylib"), b"genuine").unwrap();
        // The pre-0.41.0 stamp: the version alone.
        std::fs::write(dir.join(STAMP), "1").unwrap();
        assert_eq!(installed(cache.path(), &pack), Some(dir.clone()));
        assert!(
            std::fs::read_to_string(dir.join(STAMP))
                .unwrap()
                .contains("libx.dylib"),
            "the old stamp was not upgraded to a manifest"
        );
        assert_eq!(installed(cache.path(), &pack), Some(dir.clone()));

        std::fs::write(dir.join("lib").join("libx.dylib"), b"swapped").unwrap();
        assert_eq!(
            installed(cache.path(), &pack),
            None,
            "a swapped library loaded"
        );

        std::fs::write(dir.join("lib").join("libx.dylib"), b"genuine").unwrap();
        std::fs::write(dir.join("extra.dylib"), b"planted").unwrap();
        assert_eq!(
            installed(cache.path(), &pack),
            None,
            "a planted library loaded"
        );
    }

    #[test]
    fn an_archive_path_that_would_leave_the_pack_is_refused() {
        assert_eq!(inside("a/b/c.bin"), Some(PathBuf::from("a/b/c.bin")));
        assert_eq!(inside("./a/b"), Some(PathBuf::from("a/b")));
        assert_eq!(inside("../etc/passwd"), None);
        assert_eq!(inside("/etc/passwd"), None);
        assert_eq!(inside("a/../../b"), None);
        assert_eq!(
            below_top(Path::new("top/ane/x.mil")),
            Some(PathBuf::from("ane/x.mil"))
        );
        assert_eq!(below_top(Path::new("top")), None);
    }
}
