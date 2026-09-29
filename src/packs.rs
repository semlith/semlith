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
    std::env::var_os(format!("SEMLITH_PACK_{}", name.to_ascii_uppercase())).map(PathBuf::from)
}

const STAMP: &str = ".semlith-verified";

/// The unpacked pack, if it is installed and was checked.
pub fn installed(cache: &Path, pack: &Pack) -> Option<PathBuf> {
    if let Some(dir) = override_for(pack.name) {
        return dir.exists().then_some(dir);
    }
    let dir = pack.dir(cache);
    dir.join(STAMP).exists().then_some(dir)
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
    crate::home::write_private(&dir.join(STAMP), pack.version.as_bytes())?;
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
        let Some(name) = inside(entry.name()) else {
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

pub const COREML_VERSION: &str = "1";

const COREML_ASSETS: &[Asset] = &[Asset {
    url: "https://github.com/semlith/semlith/releases/download/pack-coreml-v1/semlith-coreml-1.zip",
    sha256: "64343cbb2cc69465dda4ed914669995e4bea47df72c0c1da43d2dbb4b2f59ad0",
    size: 147_881_077,
    form: Form::Zip,
}];

#[cfg(test)]
mod tests {
    use super::*;

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
