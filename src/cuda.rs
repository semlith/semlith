//! The CUDA lane's half of [`crate::accel`]: whether this machine has an
//! NVIDIA card CUDA can use, the pack of libraries the lane runs on, and the
//! session its worker embeds with.
//!
//! Linux x86_64 only in this release. A Windows NVIDIA card is used through
//! WebGPU (D3D12), which needs no 2 GB download; macOS has no NVIDIA card to
//! use. On every other platform [`unavailable_here`] says so and the lane
//! reports it.
//!
//! The card is read through NVML, the management library every NVIDIA driver
//! installs as `libnvidia-ml.so.1`. It is loaded at run time, so a binary that
//! never meets an NVIDIA driver never needs it.
//!
//! The pack is Microsoft's ONNX Runtime GPU build at the version the Linux
//! release already loads dynamically (1.24.4, built with CUDA 12.8 and cuDNN
//! 9), with its CUDA provider and the shared provider bridge, and NVIDIA's own
//! CUDA 12.8 libraries from their PyPI wheels. A library the system already
//! has, at the pinned version or newer within the same major, is taken from
//! the system and its wheel is not fetched.
//!
//! Licence. None of NVIDIA's libraries pass through semlith. The wheels are
//! fetched by [`fetch_pack`], on the user's machine, from PyPI's
//! `files.pythonhosted.org`, where NVIDIA publishes them: the same bytes, from
//! the same place, as `pip install nvidia-cudnn-cu12`. semlith never hosts,
//! mirrors or ships a copy. So the redistribution grants that would otherwise
//! apply bind nobody here: the CUDA Toolkit EULA's (its Attachment A lists the
//! CUDA runtime, cuBLAS, cuFFT, cuRAND, NVRTC and nvJitLink as redistributable,
//! on conditions such as "material additional functionality") and the cuDNN
//! Software License Agreement's. The user's use of the libraries is governed by
//! the licence each wheel carries ("NVIDIA Proprietary Software"), the terms a
//! pip install of the same wheel accepts. That text is kept beside the
//! libraries, in `licences/`, when the wheel carries it. ONNX Runtime is
//! Microsoft's MIT-licensed release, fetched from its GitHub release.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The oldest Linux driver that runs CUDA 12.x code: CUDA 12.8 Update 1
/// Toolkit Release Notes, Table 3, "CUDA Toolkit and Minimum Required Driver
/// Version for CUDA Minor Version Compatibility": CUDA 12.x on Linux x86_64
/// needs 525.60.13 or newer. ONNX Runtime's CUDA EP page says a build with CUDA 12.8 runs
/// on any 12.x through that compatibility, and cuDNN 9's support matrix asks
/// the same driver for CUDA 12. Below it `cudaMalloc` fails with
/// `cudaErrorInsufficientDriver`, so a card on an older driver is refused here
/// rather than there.
pub const DRIVER_MIN: &str = "525.60.13";

/// What this pack is, and the directory it lives in under the model cache.
/// The CUDA part names the libraries' minor, because a new ONNX Runtime at the
/// same version could be built against a different one.
pub const PACK_VERSION: &str = "1.24.4-cu12.8";

/// One download: where it comes from, its digest and size as its host lists
/// them, and the libraries it supplies, in the order they must be loaded.
pub struct Part {
    pub name: &'static str,
    pub version: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
    pub libs: &'static [&'static str],
}

impl Part {
    pub fn file_name(&self) -> &'static str {
        self.url.rsplit('/').next().unwrap_or(self.name)
    }
}

/// The ONNX Runtime core, as the release archive names it. It is placed as
/// `libonnxruntime.so`, the name the Linux release already gives its CPU core.
const CORE: &str = "libonnxruntime.so.1.24.4";

/// The pack. Digests and sizes are the ones GitHub's release API and PyPI's
/// JSON API list for these exact files. The NVIDIA versions are the CUDA 12.8
/// set PyTorch 2.8's cu128 wheels pin, which is what ONNX Runtime's own
/// `onnxruntime-gpu[cuda,cudnn]` extras resolve to in practice. The order is
/// the load order: each library comes after everything it needs, so the
/// worker can preload them without `LD_LIBRARY_PATH` — the same order `ort`'s
/// own `preload_dylibs` uses, with nvJitLink ahead of cuFFT 11.3, which needs
/// it.
pub const PARTS: &[Part] = &[
    Part {
        name: "onnxruntime-linux-x64-gpu",
        version: "1.24.4",
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.24.4/onnxruntime-linux-x64-gpu-1.24.4.tgz",
        sha256: "c5f804ff5d239b436fa59e9f2fb288a39f7eb9552f6a636c8b71e792e91a8808",
        size: 205_429_115,
        libs: &[
            CORE,
            "libonnxruntime_providers_shared.so",
            "libonnxruntime_providers_cuda.so",
        ],
    },
    Part {
        name: "nvidia-cuda-runtime-cu12",
        version: "12.8.90",
        url: "https://files.pythonhosted.org/packages/0d/9b/a997b638fcd068ad6e4d53b8551a7d30fe8b404d6f1804abf1df69838932/nvidia_cuda_runtime_cu12-12.8.90-py3-none-manylinux2014_x86_64.manylinux_2_17_x86_64.whl",
        sha256: "adade8dcbd0edf427b7204d480d6066d33902cab2a4707dcfc48a2d0fd44ab90",
        size: 954_765,
        libs: &["libcudart.so.12"],
    },
    Part {
        name: "nvidia-nvjitlink-cu12",
        version: "12.8.93",
        url: "https://files.pythonhosted.org/packages/f6/74/86a07f1d0f42998ca31312f998bd3b9a7eff7f52378f4f270c8679c77fb9/nvidia_nvjitlink_cu12-12.8.93-py3-none-manylinux2010_x86_64.manylinux_2_12_x86_64.whl",
        sha256: "81ff63371a7ebd6e6451970684f916be2eab07321b73c9d244dc2b4da7f73b88",
        size: 39_254_836,
        libs: &["libnvJitLink.so.12"],
    },
    Part {
        name: "nvidia-cublas-cu12",
        version: "12.8.4.1",
        url: "https://files.pythonhosted.org/packages/dc/61/e24b560ab2e2eaeb3c839129175fb330dfcfc29e5203196e5541a4c44682/nvidia_cublas_cu12-12.8.4.1-py3-none-manylinux_2_27_x86_64.whl",
        sha256: "8ac4e771d5a348c551b2a426eda6193c19aa630236b418086020df5ba9667142",
        size: 594_346_921,
        libs: &["libcublasLt.so.12", "libcublas.so.12"],
    },
    Part {
        name: "nvidia-cufft-cu12",
        version: "11.3.3.83",
        url: "https://files.pythonhosted.org/packages/1f/13/ee4e00f30e676b66ae65b4f08cb5bcbb8392c03f54f2d5413ea99a5d1c80/nvidia_cufft_cu12-11.3.3.83-py3-none-manylinux2014_x86_64.manylinux_2_17_x86_64.whl",
        sha256: "4d2dd21ec0b88cf61b62e6b43564355e5222e4a3fb394cac0db101f2dd0d4f74",
        size: 193_118_695,
        libs: &["libcufft.so.11"],
    },
    Part {
        name: "nvidia-curand-cu12",
        version: "10.3.9.90",
        url: "https://files.pythonhosted.org/packages/fb/aa/6584b56dc84ebe9cf93226a5cde4d99080c8e90ab40f0c27bda7a0f29aa1/nvidia_curand_cu12-10.3.9.90-py3-none-manylinux_2_27_x86_64.whl",
        sha256: "b32331d4f4df5d6eefa0554c565b626c7216f87a06a4f56fab27c3b68a830ec9",
        size: 63_619_976,
        libs: &["libcurand.so.10"],
    },
    Part {
        name: "nvidia-cuda-nvrtc-cu12",
        version: "12.8.93",
        url: "https://files.pythonhosted.org/packages/05/6b/32f747947df2da6994e999492ab306a903659555dddc0fbdeb9d71f75e52/nvidia_cuda_nvrtc_cu12-12.8.93-py3-none-manylinux2010_x86_64.manylinux_2_12_x86_64.whl",
        sha256: "a7756528852ef889772a84c6cd89d41dfa74667e24cca16bb31f8f061e3e9994",
        size: 88_040_029,
        // cuDNN's runtime-compiled engines reach NVRTC by dlopen, and NVRTC
        // its builtins the same way, so neither is a NEEDED entry anything
        // would pull in.
        libs: &["libnvrtc-builtins.so.12.8", "libnvrtc.so.12"],
    },
    Part {
        name: "nvidia-cudnn-cu12",
        version: "9.10.2.21",
        url: "https://files.pythonhosted.org/packages/ba/51/e123d997aa098c61d029f76663dedbfb9bc8dcf8c60cbd6adbe42f76d049/nvidia_cudnn_cu12-9.10.2.21-py3-none-manylinux_2_27_x86_64.whl",
        sha256: "949452be657fa16687d0930933f032835951ef0892b37d2d53824d1a84dc97a8",
        size: 706_758_467,
        libs: &[
            "libcudnn.so.9",
            "libcudnn_graph.so.9",
            "libcudnn_ops.so.9",
            "libcudnn_heuristic.so.9",
            "libcudnn_adv.so.9",
            "libcudnn_cnn.so.9",
            "libcudnn_engines_precompiled.so.9",
            "libcudnn_engines_runtime_compiled.so.9",
        ],
    },
];

/// Every byte [`fetch_pack`] downloads when the system supplies none of the
/// CUDA libraries, stated before the download starts: 1.89 GB. Asserted
/// against [`PARTS`] in `tests/cuda.rs`.
pub const PACK_BYTES: u64 = 1_891_522_804;

/// Why the CUDA lane cannot run on this platform, when it cannot.
pub fn unavailable_here() -> Option<String> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        None
    } else if cfg!(windows) {
        Some("CUDA is Linux-only in this release; NVIDIA cards use WebGPU".to_string())
    } else if cfg!(target_os = "linux") {
        Some("the CUDA pack is published for x86_64 Linux only".to_string())
    } else {
        Some("CUDA is Linux-only in this release".to_string())
    }
}

// ------------------------------------------------------------ the detection

/// The NVIDIA card CUDA would run on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub memory_mb: u64,
    pub driver: String,
}

/// What NVML answered, recorded, so the decision over it is a pure function
/// the tests can feed any machine's answer.
#[derive(Debug, Clone)]
pub enum Recorded {
    /// `libnvidia-ml.so.1` would not load: no NVIDIA driver is installed.
    NoNvml,
    /// NVML loaded, but a call failed, with NVML's return code (-1 when the
    /// library lacks a symbol every driver since 2016 exports).
    NvmlFailed { call: &'static str, code: i32 },
    /// NVML is working and lists no device.
    NoDevices,
    /// The first device NVML lists, and the driver's version.
    Device {
        name: String,
        memory_mb: u64,
        driver: String,
    },
}

/// The NVIDIA card this machine would run CUDA on, or why there is none.
pub fn detect() -> Result<Device, String> {
    if let Some(why) = unavailable_here() {
        return Err(why);
    }
    judge(record())
}

/// The decision over what NVML said. A card on a driver below
/// [`DRIVER_MIN`] is refused, naming the minimum; the lane then stays off and
/// the card keeps working through WebGPU.
pub fn judge(recorded: Recorded) -> Result<Device, String> {
    match recorded {
        Recorded::NoNvml | Recorded::NoDevices => Err("no NVIDIA device".to_string()),
        Recorded::NvmlFailed { call, code } => Err(format!(
            "no usable NVIDIA device: the driver's NVML failed {call} (code {code})"
        )),
        Recorded::Device {
            name,
            memory_mb,
            driver,
        } => {
            if numbers(&driver) < numbers(DRIVER_MIN) {
                return Err(format!(
                    "{name} is on NVIDIA driver {driver}, below {DRIVER_MIN}, the oldest that \
                     runs CUDA 12 — update the driver to use CUDA; until then the card is used \
                     through WebGPU"
                ));
            }
            Ok(Device {
                name,
                memory_mb,
                driver,
            })
        }
    }
}

/// A dotted version as numbers, so `570.133.07` sorts after `525.60.13` and
/// `9.10` after `9.9`. A part that is not a number counts as zero.
fn numbers(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

#[cfg(target_os = "linux")]
fn record() -> Recorded {
    use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
    type Init = unsafe extern "C" fn() -> c_int;
    type Count = unsafe extern "C" fn(*mut c_uint) -> c_int;
    type Handle = unsafe extern "C" fn(c_uint, *mut *mut c_void) -> c_int;
    type Name = unsafe extern "C" fn(*mut c_void, *mut c_char, c_uint) -> c_int;
    // nvmlMemory_t: total, free, used, each an unsigned long long.
    type Memory = unsafe extern "C" fn(*mut c_void, *mut [u64; 3]) -> c_int;
    type Driver = unsafe extern "C" fn(*mut c_char, c_uint) -> c_int;
    type Shutdown = unsafe extern "C" fn() -> c_int;

    fn text(buffer: &[u8]) -> String {
        CStr::from_bytes_until_nul(buffer)
            .map(|c| c.to_string_lossy().trim().to_string())
            .unwrap_or_default()
    }

    // SAFETY: each symbol is looked up by its NUL-terminated name and, when
    // present, transmuted to the signature nvml.h declares for it (a function
    // pointer is pointer-sized, which `sym` asserts). Every out-parameter is a
    // live local of the size NVML is told: 96 bytes for a name
    // (NVML_DEVICE_NAME_V2_BUFFER_SIZE), 80 for the driver version
    // (NVML_SYSTEM_DRIVER_VERSION_BUFFER_SIZE), three u64s for nvmlMemory_t.
    // The device handle is used only between init and shutdown, and the
    // library is closed only after shutdown returns.
    unsafe {
        unsafe fn sym<F: Copy>(lib: *mut c_void, name: &CStr) -> Option<F> {
            assert_eq!(size_of::<F>(), size_of::<*mut c_void>());
            let found = unsafe { libc::dlsym(lib, name.as_ptr()) };
            (!found.is_null()).then(|| unsafe { std::mem::transmute_copy(&found) })
        }
        let lib = libc::dlopen(
            c"libnvidia-ml.so.1".as_ptr(),
            libc::RTLD_NOW | libc::RTLD_LOCAL,
        );
        if lib.is_null() {
            return Recorded::NoNvml;
        }
        let (
            Some(init),
            Some(count),
            Some(handle),
            Some(name),
            Some(memory),
            Some(driver),
            Some(shutdown),
        ) = (
            sym::<Init>(lib, c"nvmlInit_v2"),
            sym::<Count>(lib, c"nvmlDeviceGetCount_v2"),
            sym::<Handle>(lib, c"nvmlDeviceGetHandleByIndex_v2"),
            sym::<Name>(lib, c"nvmlDeviceGetName"),
            sym::<Memory>(lib, c"nvmlDeviceGetMemoryInfo"),
            sym::<Driver>(lib, c"nvmlSystemGetDriverVersion"),
            sym::<Shutdown>(lib, c"nvmlShutdown"),
        )
        else {
            libc::dlclose(lib);
            return Recorded::NvmlFailed {
                call: "dlsym",
                code: -1,
            };
        };
        let code = init();
        if code != 0 {
            libc::dlclose(lib);
            return Recorded::NvmlFailed {
                call: "nvmlInit_v2",
                code,
            };
        }
        let answer = 'read: {
            let mut devices: c_uint = 0;
            let code = count(&mut devices);
            if code != 0 {
                break 'read Recorded::NvmlFailed {
                    call: "nvmlDeviceGetCount_v2",
                    code,
                };
            }
            if devices == 0 {
                break 'read Recorded::NoDevices;
            }
            // ponytail: the first device only. NVML orders by PCI bus and
            // CUDA's device 0 is the fastest, so a mixed multi-GPU box may
            // name one card and run on another; pass CUDA_VISIBLE_DEVICES-style
            // selection through when somebody has two.
            let mut device: *mut c_void = std::ptr::null_mut();
            let code = handle(0, &mut device);
            if code != 0 {
                break 'read Recorded::NvmlFailed {
                    call: "nvmlDeviceGetHandleByIndex_v2",
                    code,
                };
            }
            let mut name_buffer = [0u8; 96];
            let code = name(device, name_buffer.as_mut_ptr().cast(), 96);
            if code != 0 {
                break 'read Recorded::NvmlFailed {
                    call: "nvmlDeviceGetName",
                    code,
                };
            }
            // Informational only; a card that will not say is still a card.
            let mut bytes = [0u64; 3];
            let memory_mb = if memory(device, &mut bytes) == 0 {
                bytes[0] / (1024 * 1024)
            } else {
                0
            };
            let mut version = [0u8; 80];
            let code = driver(version.as_mut_ptr().cast(), 80);
            if code != 0 {
                break 'read Recorded::NvmlFailed {
                    call: "nvmlSystemGetDriverVersion",
                    code,
                };
            }
            Recorded::Device {
                name: text(&name_buffer),
                memory_mb,
                driver: text(&version),
            }
        };
        shutdown();
        libc::dlclose(lib);
        answer
    }
}

#[cfg(not(target_os = "linux"))]
fn record() -> Recorded {
    Recorded::NoNvml
}

// ----------------------------------------------------------------- the pack

fn pack_dir(cache: &Path) -> PathBuf {
    crate::accel::component_dir(cache, &format!("cuda-{PACK_VERSION}"))
}

const STAMP: &str = ".semlith-verified";

/// The pack's directory, when every part of it was fetched, verified and
/// unpacked. Its libraries are in `lib/`.
pub fn pack_installed(cache: &Path) -> Option<PathBuf> {
    let dir = pack_dir(cache);
    (dir.join(STAMP).exists() && dir.join("lib").join("libonnxruntime.so").exists()).then_some(dir)
}

/// Fetch the pack into `cache/accel/cuda-<version>/`, verifying every file by
/// digest as it streams to disk, and unpack only its libraries into `lib/`.
/// A wheel whose libraries the system already has at a compatible version is
/// skipped. Each part is marked done once unpacked and its archive deleted, so
/// an interrupted 1.9 GB fetch resumes at the part it stopped in. Refused
/// under `--airgap` unless the pack is already here.
pub fn fetch_pack(cache: &Path, progress: &mut dyn FnMut(u8)) -> Result<PathBuf> {
    if let Some(dir) = pack_installed(cache) {
        return Ok(dir);
    }
    if let Some(why) = unavailable_here() {
        bail!("unavailable — {why}");
    }
    let dir = pack_dir(cache);
    if crate::embed::airgap() {
        bail!(
            "{} is set and the CUDA pack is not in {} — pre-seed it on a connected \
             machine, or drop --airgap",
            crate::embed::AIRGAP_ENV,
            crate::plain(&dir.display().to_string())
        );
    }
    crate::embed::check_cache_dir(cache)?;
    let lib = dir.join("lib");
    std::fs::create_dir_all(&lib).with_context(|| format!("creating {}", lib.display()))?;

    let done = |part: &Part| dir.join(format!(".{}.done", part.name));
    let todo: Vec<&Part> = PARTS
        .iter()
        .enumerate()
        .filter(|(i, part)| !done(part).exists() && (*i == 0 || !on_system(part)))
        .map(|(_, part)| part)
        .collect();
    let total: u64 = todo.iter().map(|part| part.size).sum();
    let mut got = 0u64;
    let mut report = |bytes: u64| {
        got += bytes;
        progress(((got * 100) / total.max(1)).min(100) as u8);
    };
    for part in todo {
        let archive = download(part, &dir, &mut report)?;
        if archive.extension().is_some_and(|ext| ext == "tgz") {
            untar_libs(&archive, &lib, part.libs)?;
        } else {
            unzip_libs(&archive, &dir, part)?;
        }
        let _ = std::fs::remove_file(&archive);
        crate::home::write_private(&done(part), part.version.as_bytes())?;
    }
    crate::home::write_private(&dir.join(STAMP), PACK_VERSION.as_bytes())?;
    Ok(dir)
}

/// Stream `part` into `dir`, hashing as it goes, and refuse it unless the
/// digest is the pinned one. A refused file is deleted, never kept; a file
/// already there is checked the same way and kept only if it matches.
pub fn download(part: &Part, dir: &Path, progress: &mut dyn FnMut(u64)) -> Result<PathBuf> {
    let to = dir.join(part.file_name());
    if to.exists() && verify_file(&to, part.sha256).is_ok() {
        progress(part.size);
        return Ok(to);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_connect(Some(std::time::Duration::from_secs(20)))
        // The largest part is 707 MB: three hours is 65 KB/s, below which a
        // user is better served by the WebGPU lane than by waiting.
        .timeout_global(Some(std::time::Duration::from_secs(3 * 3600)))
        .build()
        .into();
    let mut response = agent
        .get(part.url)
        .call()
        .with_context(|| format!("fetching {}", part.url))?;
    let partial = to.with_extension("partial");
    let out = std::fs::File::create(&partial)
        .with_context(|| format!("writing {}", partial.display()))?;
    let copied = copy_hashing(
        response.body_mut().as_reader(),
        std::io::BufWriter::new(out),
        progress,
    );
    let actual = match copied {
        Ok(actual) => actual,
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            return Err(e.context(format!("reading {}", part.url)));
        }
    };
    if actual != part.sha256 {
        let _ = std::fs::remove_file(&partial);
        bail!(
            "{} does not match the digest semlith pins for it (expected {}, got {actual}); \
             it was deleted",
            part.url,
            part.sha256
        );
    }
    std::fs::rename(&partial, &to).with_context(|| format!("placing {}", to.display()))?;
    crate::home::tighten_file(&to);
    Ok(to)
}

/// Check a file already on disk against its pinned digest, streaming, since
/// the largest part is 707 MB. A file that does not match is deleted.
pub fn verify_file(path: &Path, sha256: &str) -> Result<()> {
    let file = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let actual = copy_hashing(file, std::io::sink(), &mut |_| {})?;
    if actual != sha256 {
        let _ = std::fs::remove_file(path);
        bail!(
            "{} does not match the digest semlith pins for it (expected {sha256}, got {actual}); \
             it was deleted",
            path.display()
        );
    }
    Ok(())
}

/// Copy everything, hashing on the way, and answer the SHA-256 as hex.
fn copy_hashing(
    mut from: impl Read,
    mut to: impl Write,
    progress: &mut dyn FnMut(u64),
) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let n = from.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        to.write_all(&buffer[..n])?;
        progress(n as u64);
    }
    to.flush()?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// The named libraries out of a verified wheel's `nvidia/<package>/lib/`, and
/// its licence text into `licences/`.
fn unzip_libs(wheel: &Path, dir: &Path, part: &Part) -> Result<()> {
    let file = std::fs::File::open(wheel)?;
    let mut archive =
        zip::ZipArchive::new(file).with_context(|| format!("reading {}", part.file_name()))?;
    let mut placed = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        let base = name.rsplit('/').next().unwrap_or_default();
        let target = if name.starts_with("nvidia/") && part.libs.contains(&base) {
            placed += 1;
            dir.join("lib").join(base)
        } else if name.contains(".dist-info/") && base.to_ascii_lowercase().starts_with("license") {
            let licences = dir.join("licences");
            std::fs::create_dir_all(&licences)?;
            licences.join(format!("{}.txt", part.name))
        } else {
            continue;
        };
        let mut out = std::fs::File::create(&target)
            .with_context(|| format!("writing {}", target.display()))?;
        std::io::copy(&mut entry, &mut out)?;
    }
    if placed != part.libs.len() {
        bail!(
            "{} held {placed} of the {} libraries semlith expects in it",
            part.file_name(),
            part.libs.len()
        );
    }
    Ok(())
}

/// The named libraries out of a verified `.tgz`, wherever they sit in it.
/// Enough of tar for Microsoft's release archive: 512-byte headers, each
/// followed by its file padded to 512; only regular files are taken, so its
/// `libonnxruntime.so` symlinks are skipped and the core is placed under that
/// name instead.
fn untar_libs(tgz: &Path, lib: &Path, wanted: &[&str]) -> Result<()> {
    let file = std::fs::File::open(tgz)?;
    let mut reader = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
    let mut header = [0u8; 512];
    let mut placed = 0;
    loop {
        match reader.read_exact(&mut header) {
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            other => other.context("reading the ONNX Runtime archive")?,
        }
        if header.iter().all(|b| *b == 0) {
            break;
        }
        let field = |range: std::ops::Range<usize>| {
            let bytes = &header[range];
            let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
            String::from_utf8_lossy(&bytes[..end]).trim().to_string()
        };
        let name = field(0..100);
        let size = u64::from_str_radix(&field(124..136), 8)
            .context("a tar header in the ONNX Runtime archive has no readable size")?;
        let base = name.rsplit('/').next().unwrap_or_default();
        let mut entry = (&mut reader).take(size.div_ceil(512) * 512);
        if matches!(header[156], b'0' | 0) && wanted.contains(&base) {
            let target = lib.join(if base == CORE {
                "libonnxruntime.so"
            } else {
                base
            });
            let mut out = std::fs::File::create(&target)
                .with_context(|| format!("writing {}", target.display()))?;
            std::io::copy(&mut (&mut entry).take(size), &mut out)?;
            placed += 1;
        }
        std::io::copy(&mut entry, &mut std::io::sink())?;
    }
    if placed != wanted.len() {
        bail!(
            "the ONNX Runtime archive held {placed} of the {} libraries semlith expects in it",
            wanted.len()
        );
    }
    Ok(())
}

/// Whether the system supplies every library in `part` at a compatible
/// version, so its wheel need not be fetched.
fn on_system(part: &Part) -> bool {
    part.libs
        .iter()
        .all(|lib| system_lib(lib, part.version).is_some())
}

/// A system copy of `lib` at `pinned` or newer within its major, found where
/// NVIDIA's packages and the distributions put CUDA. The version is read off
/// the file the soname link resolves to, `libcudart.so.12.8.90`; a copy
/// whose name carries no minor is not trusted, and the wheel is fetched.
///
/// ponytail: fixed directories plus `LD_LIBRARY_PATH`, not the `ld.so` cache;
/// a CUDA installed anywhere else is simply fetched again from the wheels.
fn system_lib(lib: &str, pinned: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("LD_LIBRARY_PATH")
        .map(|paths| std::env::split_paths(&paths).collect())
        .unwrap_or_default();
    dirs.extend(
        [
            "/usr/local/cuda/lib64",
            "/usr/local/cuda/targets/x86_64-linux/lib",
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib64",
            "/usr/lib",
        ]
        .map(PathBuf::from),
    );
    dirs.into_iter().map(|dir| dir.join(lib)).find(|path| {
        std::fs::canonicalize(path)
            .ok()
            .and_then(|real| real.file_name().map(|n| n.to_string_lossy().into_owned()))
            .and_then(|real| real.split_once(".so.").map(|(_, v)| v.to_string()))
            .is_some_and(|found| compatible(&found, pinned))
    })
}

/// Whether a system library's version will do for the pinned one: at least
/// major.minor, and no older than the pin over the parts both name, so
/// cuDNN's `9.10.2` answers for the wheel's `9.10.2.21`.
fn compatible(found: &str, pinned: &str) -> bool {
    let (found, pinned) = (numbers(found), numbers(pinned));
    let n = found.len().min(pinned.len());
    found.len() >= 2 && found[0] == pinned[0] && found[..n] >= pinned[..n]
}

// --------------------------------------------------------------- the session

/// What the CUDA lane's worker embeds with: the fp16 model on ONNX Runtime's
/// CUDA execution provider, from the pack's own ONNX Runtime core.
pub struct Session {
    session: ort::session::Session,
    tokenizer: tokenizers::Tokenizer,
    device: String,
}

impl Session {
    /// Load the pack and open the model on the card. Must run before anything
    /// else in the process touches ONNX Runtime — in particular before
    /// `embed::link_runtime`, which would load the CPU core beside the binary
    /// and make this one's `init_from` a silent no-op; the CUDA EP would then
    /// fail with "not enabled in this build". Nothing needs `LD_LIBRARY_PATH`:
    /// every CUDA library is preloaded by full path first.
    ///
    /// The pack is loaded and the CUDA provider appended before the card is
    /// looked for, because neither needs one (ORT's provider factory ignores
    /// the `cudaMalloc` it tries); a runner with no card therefore proves the
    /// whole pack loads, then reports `unavailable — no NVIDIA device`.
    pub fn open(pack: &Path, model_fp16: &Path) -> Result<Self> {
        if let Some(why) = unavailable_here() {
            bail!("unavailable — {why}");
        }
        if !cfg!(feature = "dynamic-ort") {
            bail!(
                "unavailable — CUDA needs the Linux release build, which loads its runtime \
                 dynamically"
            );
        }
        load(pack)?;
        let mut builder = ort::session::Session::builder().map_err(|e| anyhow::anyhow!("{e}"))?;
        append_cuda(&mut builder)?;
        let card = detect().map_err(|why| anyhow::anyhow!("unavailable — {why}"))?;
        let session = builder
            .commit_from_file(model_fp16)
            .map_err(|e| anyhow::anyhow!("loading the fp16 model on {}: {e}", card.name))?;
        let tokenizer = crate::embed::granite_tokenizer(
            &crate::model_cache_dir()?,
            crate::chunk::MAX_CHARS / 2,
        )?;
        Ok(Self {
            session,
            tokenizer,
            device: format!("{} (CUDA)", card.name),
        })
    }

    pub fn device(&self) -> String {
        self.device.clone()
    }

    /// The same preparation as the WebGPU lane's, which is fastembed's: its
    /// tokenizer padded to the batch's longest, input ids and attention mask,
    /// and the CLS token's vector, normalised.
    pub fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow::anyhow!("tokenizing: {e}"))?;
        let seq = encodings[0].len();
        let n = encodings.len();
        let ids: Vec<i64> = encodings
            .iter()
            .flat_map(|e| e.get_ids().iter().map(|x| *x as i64))
            .collect();
        let mask: Vec<i64> = encodings
            .iter()
            .flat_map(|e| e.get_attention_mask().iter().map(|x| *x as i64))
            .collect();
        let outputs = self
            .session
            .run(ort::inputs![
                "input_ids" => ort::value::Tensor::from_array(([n, seq], ids)).map_err(|e| anyhow::anyhow!("{e}"))?,
                "attention_mask" => ort::value::Tensor::from_array(([n, seq], mask)).map_err(|e| anyhow::anyhow!("{e}"))?,
            ])
            .map_err(|e| anyhow::anyhow!("running the model on CUDA: {e}"))?;
        let (shape, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let dim = *shape.last().context("an output with no shape")? as usize;
        let mut vectors = Vec::with_capacity(n);
        for row in 0..n {
            let start = row * seq * dim;
            let mut vector = data[start..start + dim].to_vec();
            crate::normalize(&mut vector);
            vectors.push(vector);
        }
        Ok(vectors)
    }
}

/// Preload the CUDA libraries, each after what it needs, then hand ONNX
/// Runtime the pack's GPU core. The pack's own copy is preferred; one the
/// fetch skipped because the system had it is loaded from the system.
#[cfg(target_os = "linux")]
fn load(pack: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let lib = pack.join("lib");
    for part in &PARTS[1..] {
        for name in part.libs {
            let local = lib.join(name);
            let path = if local.exists() {
                local
            } else if let Some(system) = system_lib(name, part.version) {
                system
            } else {
                bail!(
                    "{name} is neither in the CUDA pack at {} nor on this system at a compatible \
                     version — turn CUDA off and on again to fetch the pack afresh",
                    pack.display()
                );
            };
            let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
            // SAFETY: a NUL-terminated path that outlives the call. The
            // handle is deliberately never closed: the libraries must live
            // as long as the worker, which is the life of the session.
            // `dlerror` is read on the same thread straight after the
            // failure it describes, and copied before any other dl call.
            let handle =
                unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
            if handle.is_null() {
                let why = unsafe {
                    let text = libc::dlerror();
                    if text.is_null() {
                        "no reason given".to_string()
                    } else {
                        std::ffi::CStr::from_ptr(text)
                            .to_string_lossy()
                            .into_owned()
                    }
                };
                bail!("loading {}: {why}", path.display());
            }
        }
    }
    #[cfg(feature = "dynamic-ort")]
    {
        let core = lib.join("libonnxruntime.so");
        ort::init_from(&core)
            .map_err(|e| anyhow::anyhow!("loading {}: {e}", core.display()))?
            .commit();
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn load(_pack: &Path) -> Result<()> {
    bail!("unavailable — CUDA is Linux-only in this release")
}

/// Append the CUDA execution provider with its defaults through the C API,
/// so no `ort` cargo feature (and no second ONNX Runtime download at build
/// time) is needed.
fn append_cuda(builder: &mut ort::session::builder::SessionBuilder) -> Result<()> {
    use ort::AsPointer;
    let api = ort::api();
    let failed = |status: ort::sys::OrtStatusPtr, what: &str| -> Result<()> {
        if status.0.is_null() {
            return Ok(());
        }
        // SAFETY: a non-null status ORT just returned, read once and then
        // released; the message is copied before the release.
        let message = unsafe {
            let text = std::ffi::CStr::from_ptr((api.GetErrorMessage)(status.0))
                .to_string_lossy()
                .into_owned();
            (api.ReleaseStatus)(status.0);
            text
        };
        bail!("{what}: {message}")
    };
    let mut options: *mut ort::sys::OrtCUDAProviderOptionsV2 = std::ptr::null_mut();
    // SAFETY: `options` is a live out-pointer ORT fills with an allocation it
    // owns; it is passed to the append, which copies what it needs, and then
    // released exactly once whatever the append answered. The session options
    // pointer is the builder's own, valid for as long as `builder` is.
    unsafe {
        failed(
            (api.CreateCUDAProviderOptions)(&mut options),
            "creating the CUDA provider's options",
        )?;
        let appended =
            (api.SessionOptionsAppendExecutionProvider_CUDA_V2)(builder.ptr_mut(), options);
        (api.ReleaseCUDAProviderOptions)(options);
        failed(appended, "adding the CUDA execution provider")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_system_library_counts_only_at_the_pinned_version_or_newer() {
        assert!(compatible("12.8.90", "12.8.90"));
        assert!(compatible("12.9.37", "12.8.90"));
        assert!(compatible("9.10.2", "9.10.2.21"));
        assert!(!compatible("12.2.140", "12.8.90"));
        assert!(!compatible("11.8.89", "12.8.90"));
        assert!(!compatible("13.0.48", "12.8.90"));
        // An unversioned soname says nothing about the minor.
        assert!(!compatible("12", "12.8.90"));
    }
}
