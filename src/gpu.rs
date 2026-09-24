//! The GPU's half of [`crate::accel`]: whether this machine has one, the
//! pinned components a lane needs, and the session a worker runs.
//!
//! Only a hardware adapter is used. A machine whose only device is a software
//! renderer — Mesa's lavapipe or llvmpipe, SwiftShader, Microsoft's WARP or
//! Basic Render Driver, a hypervisor's framebuffer — counts as having no GPU:
//! lavapipe aborts the process on any MatMul (an LLVM fatal error), and even a
//! renderer that ran would be slower than the CPU it runs on. It is refused by
//! vendor before anything is downloaded, and again by the worker before any
//! session is made.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// PCI vendors that are never a hardware GPU: hypervisor framebuffers and
/// software renderers. Microsoft's is WARP, Basic Render and Hyper-V's video;
/// Mesa's is lavapipe and llvmpipe; Google's is SwiftShader.
const SOFTWARE_VENDORS: &[u32] = &[
    0x1414,  // Microsoft: WARP, Basic Render Driver, Hyper-V video
    0x10005, // Mesa: lavapipe, llvmpipe
    0x1ae0,  // Google: SwiftShader
    0x1234,  // QEMU's standard VGA
    0x15ad,  // VMware SVGA
    0x1af4,  // virtio-gpu
    0x80ee,  // VirtualBox
    0x1b36,  // Red Hat QXL
];

/// Adapter names that give a software renderer away when a vendor id does not.
const SOFTWARE_NAMES: &[&str] = &[
    "llvmpipe",
    "lavapipe",
    "swiftshader",
    "warp",
    "basic render",
    "basic display",
    "hyper-v",
    "virtio",
    "vmware",
    "virtualbox",
];

pub fn is_software(vendor: u32, name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    SOFTWARE_VENDORS.contains(&vendor) || SOFTWARE_NAMES.iter().any(|n| name.contains(n))
}

/// The hardware GPU this machine has, by name, or why it has none.
pub fn detect() -> std::result::Result<String, String> {
    platform::detect()
}

#[cfg(target_os = "macos")]
mod platform {
    /// Every Apple silicon Mac has a Metal GPU, and it is the one Metal device
    /// a GitHub macOS arm64 runner exposes too.
    pub fn detect() -> std::result::Result<String, String> {
        if cfg!(target_arch = "aarch64") {
            Ok(format!("{} GPU (Metal)", crate::system::cpu_name()))
        } else {
            Err("no Metal GPU semlith supports on an Intel Mac".to_string())
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    /// A DRM card whose PCI vendor is a real GPU maker, and a Vulkan loader
    /// for the plugin to reach it through.
    pub fn detect() -> std::result::Result<String, String> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir("/sys/class/drm")
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("card") || name.contains('-') {
                continue;
            }
            let Ok(vendor) = std::fs::read_to_string(entry.path().join("device/vendor")) else {
                continue;
            };
            let Ok(vendor) = u32::from_str_radix(vendor.trim().trim_start_matches("0x"), 16) else {
                continue;
            };
            if !is_software(vendor, "") {
                found.push(vendor_name(vendor));
            }
        }
        let Some(first) = found.first() else {
            return Err("no hardware GPU found".to_string());
        };
        // SAFETY: a NUL-terminated name; the handle is closed straight away
        // and nothing is looked up through it.
        let loader = unsafe {
            let handle = libc::dlopen(
                c"libvulkan.so.1".as_ptr(),
                libc::RTLD_NOW | libc::RTLD_LOCAL,
            );
            if !handle.is_null() {
                libc::dlclose(handle);
            }
            !handle.is_null()
        };
        if !loader {
            return Err(format!(
                "{first} found, but no Vulkan loader (libvulkan.so.1) is installed"
            ));
        }
        Ok(first.clone())
    }

    fn vendor_name(vendor: u32) -> String {
        match vendor {
            0x10de => "NVIDIA GPU".to_string(),
            0x1002 => "AMD GPU".to_string(),
            0x8086 => "Intel GPU".to_string(),
            other => format!("GPU (PCI vendor {other:04x})"),
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;

    /// The display adapters Windows lists, less the software ones. Read
    /// through PowerShell once per lane start, which is seconds apart from
    /// anything a user waits on.
    pub fn detect() -> std::result::Result<String, String> {
        let out = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-CimInstance Win32_VideoController | ForEach-Object { $_.Name + '|' + $_.PNPDeviceID }",
            ])
            .output()
            .map_err(|e| format!("could not list display adapters: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let Some((name, pnp)) = line.split_once('|') else {
                continue;
            };
            let vendor = pnp
                .to_ascii_uppercase()
                .split("VEN_")
                .nth(1)
                .and_then(|rest| u32::from_str_radix(rest.get(..4)?, 16).ok())
                .unwrap_or(0);
            if vendor != 0 && !is_software(vendor, name) {
                return Ok(name.trim().to_string());
            }
        }
        Err("no hardware GPU found".to_string())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod platform {
    pub fn detect() -> std::result::Result<String, String> {
        Err("no GPU support on this platform".to_string())
    }
}

/// Why the CUDA lane cannot run in this build, when it cannot.
pub fn cuda_unavailable() -> String {
    cuda_unavailable_here().unwrap_or_else(|| "the CUDA lane is not part of this build".to_string())
}

/// Why CUDA cannot be turned on at all on this platform. Linux only in this
/// release: an NVIDIA card on Windows runs through WebGPU (D3D12).
pub fn cuda_unavailable_here() -> Option<String> {
    crate::cuda::unavailable_here()
}

/// Where the CUDA pack lives in the model cache.
pub fn cuda_dir(cache: &Path) -> PathBuf {
    crate::cuda::pack_dir(cache)
}

/// What turning CUDA on downloads, in bytes, said before it starts.
pub fn cuda_pack_bytes() -> u64 {
    crate::cuda::PACK_BYTES
}

// ------------------------------------------------------------ the components

/// The WebGPU plugin release, and the wheel it comes in for this platform:
/// URL, SHA-256 and size, from PyPI's own listing for 0.4.0.
pub const WEBGPU_VERSION: &str = "0.4.0";

struct Wheel {
    url: &'static str,
    sha256: &'static str,
    size: u64,
}

fn wheel() -> Option<Wheel> {
    if cfg!(target_os = "macos") {
        Some(Wheel {
            url: "https://files.pythonhosted.org/packages/c3/9f/1e09865bbe4c9202ff1334545535be32dfdbeffc46e7d4c36c5de67ffafe/onnxruntime_ep_webgpu-0.4.0-py3-none-macosx_14_0_universal2.whl",
            sha256: "8d0a91d44b43d931c3068b9134aed9770f27803e353c2b58c93e1700bb68bcca",
            size: 5_200_679,
        })
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(Wheel {
            url: "https://files.pythonhosted.org/packages/c1/96/2a18a45079250afcd825687aef2895de266d5abc1cff9f52d2dede7598f2/onnxruntime_ep_webgpu-0.4.0-py3-none-manylinux_2_28_x86_64.whl",
            sha256: "5d8c961eda91a88961cad1fb7621f389f006b18497d6ea9ecf96f729f066cbfb",
            size: 6_892_120,
        })
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some(Wheel {
            url: "https://files.pythonhosted.org/packages/4e/ca/00c70322c19913c81a6bb2239aca83781b97a818b55c2ec3d25cec72dc4c/onnxruntime_ep_webgpu-0.4.0-py3-none-manylinux_2_28_aarch64.whl",
            sha256: "17f660db53b1a509c63e721ac6784a2daa1c4e91a968523ee9986505ee5b0a55",
            size: 6_096_222,
        })
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        Some(Wheel {
            url: "https://files.pythonhosted.org/packages/d7/a4/c98a9e9433b3eeb576977b26c5c1cd0364f15ba9d198bb16101e7563ab06/onnxruntime_ep_webgpu-0.4.0-py3-none-win_amd64.whl",
            sha256: "7db646669d1a2390551da675115ea125e68cb3d4d2c3a2d6e463372bf8d6ea87",
            size: 13_149_847,
        })
    } else {
        None
    }
}

/// The fp16 variant of the pinned granite revision. The int8 graph does not
/// load on WebGPU, so the GPU lanes run this one.
pub const FP16_FILES: &[(&str, &str, u64)] = &[
    (
        "onnx/model_fp16.onnx",
        "ee200de55cb2f94e858aabca54be7697a9c0805a14c858ee26ad0922b05f57d7",
        200_792,
    ),
    (
        "onnx/model_fp16.onnx_data",
        "28d16e29cd623f25cc6fa0968700c5bc31036466091a5fa06d1353c1777f050e",
        97_402_880,
    ),
];

/// The plugin library's file name on this platform.
pub fn plugin_file() -> &'static str {
    if cfg!(target_os = "macos") {
        "libonnxruntime_providers_webgpu.dylib"
    } else if cfg!(windows) {
        "onnxruntime_providers_webgpu.dll"
    } else {
        "libonnxruntime_providers_webgpu.so"
    }
}

/// Everything the WebGPU lane needs, in one directory under the model cache:
/// the plugin library (and on Windows the two shader compilers beside it) and
/// the fp16 weights, each verified by digest. Fetched on first use, when a
/// hardware GPU was found and the lane is on; refused under `--airgap`
/// unless it was pre-seeded.
pub fn fetch_webgpu(cache: &Path, progress: &mut dyn FnMut(u8)) -> Result<PathBuf> {
    let dir = crate::accel::component_dir(cache, &format!("webgpu-{WEBGPU_VERSION}"));
    let stamp = dir.join(".semlith-verified");
    if stamp.exists() && dir.join(plugin_file()).exists() {
        return Ok(dir);
    }
    if crate::embed::airgap() {
        bail!(
            "{} is set and the WebGPU components are not in {} — pre-seed them on a \
             connected machine, or drop --airgap",
            crate::embed::AIRGAP_ENV,
            crate::plain(&dir.display().to_string())
        );
    }
    let Some(wheel) = wheel() else {
        bail!("unavailable — no WebGPU plugin is published for this platform");
    };
    crate::embed::check_cache_dir(cache)?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let total = wheel.size + FP16_FILES.iter().map(|(_, _, size)| size).sum::<u64>();
    let mut done = 0u64;
    let mut report = |bytes: u64| {
        done += bytes;
        progress(((done * 100) / total.max(1)).min(100) as u8);
    };

    let wheel_path = dir.join("plugin.whl");
    download(wheel.url, &wheel_path, wheel.sha256, &mut report)?;
    extract_plugin(&wheel_path, &dir)?;
    let _ = std::fs::remove_file(&wheel_path);

    fetch_fp16_into(&dir, &mut report)?;
    crate::home::write_private(&stamp, WEBGPU_VERSION.as_bytes())?;
    Ok(dir)
}

/// The fp16 weights alone, for the CUDA lane, which needs them and not the
/// WebGPU plugin. Same directory, same digests.
pub fn fetch_fp16(cache: &Path, progress: &mut dyn FnMut(u8)) -> Result<PathBuf> {
    let dir = crate::accel::component_dir(cache, &format!("webgpu-{WEBGPU_VERSION}"));
    let total: u64 = FP16_FILES.iter().map(|(_, _, size)| size).sum();
    let present = FP16_FILES.iter().all(|(file, _, _)| {
        Path::new(file)
            .file_name()
            .is_some_and(|name| dir.join(name).exists())
    });
    if present {
        return Ok(dir);
    }
    if crate::embed::airgap() {
        bail!(
            "{} is set and the fp16 model is not in {}",
            crate::embed::AIRGAP_ENV,
            crate::plain(&dir.display().to_string())
        );
    }
    crate::embed::check_cache_dir(cache)?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut done = 0u64;
    fetch_fp16_into(&dir, &mut |bytes| {
        done += bytes;
        progress(((done * 100) / total.max(1)).min(100) as u8);
    })?;
    Ok(dir)
}

fn fetch_fp16_into(dir: &Path, report: &mut dyn FnMut(u64)) -> Result<()> {
    let base = format!(
        "https://huggingface.co/{}/resolve/{}",
        crate::embed::GRANITE_REPO,
        crate::embed::GRANITE_REVISION
    );
    for (file, sha256, _) in FP16_FILES {
        let name = Path::new(file).file_name().expect("a file name");
        download(&format!("{base}/{file}"), &dir.join(name), sha256, report)?;
    }
    Ok(())
}

/// The plugin wheel's size on this platform, for the Privacy page.
pub fn plugin_bytes() -> u64 {
    wheel().map(|w| w.size).unwrap_or(0)
}

/// Stream `url` to `to`, hashing as it goes, and refuse it unless the digest
/// is the pinned one. A refused file is deleted, never kept.
fn download(url: &str, to: &Path, sha256: &str, progress: &mut dyn FnMut(u64)) -> Result<()> {
    use sha2::{Digest, Sha256};
    if to.exists() && crate::embed::digest(&std::fs::read(to)?) == sha256 {
        progress(std::fs::metadata(to)?.len());
        return Ok(());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_connect(Some(std::time::Duration::from_secs(20)))
        .timeout_global(Some(std::time::Duration::from_secs(1800)))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("fetching {url}"))?;
    let partial = to.with_extension("partial");
    let mut out = std::fs::File::create(&partial)
        .with_context(|| format!("writing {}", partial.display()))?;
    let mut reader = response.body_mut().as_reader();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let n = reader
            .read(&mut buffer)
            .with_context(|| format!("reading {url}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        out.write_all(&buffer[..n])?;
        progress(n as u64);
    }
    out.flush()?;
    drop(out);
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != sha256 {
        let _ = std::fs::remove_file(&partial);
        bail!(
            "{url} does not match the digest semlith pins for it (expected {sha256}, got {actual}); \
             it was deleted"
        );
    }
    std::fs::rename(&partial, to).with_context(|| format!("placing {}", to.display()))?;
    crate::home::tighten_file(to);
    Ok(())
}

/// The plugin's libraries, out of the wheel's `onnxruntime_ep_webgpu/`.
fn extract_plugin(wheel: &Path, dir: &Path) -> Result<()> {
    let file = std::fs::File::open(wheel)?;
    let mut archive = zip::ZipArchive::new(file).context("reading the plugin wheel")?;
    let mut placed = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        let Some(base) = name.strip_prefix("onnxruntime_ep_webgpu/") else {
            continue;
        };
        let library = [".dylib", ".so", ".dll"]
            .iter()
            .any(|ext| base.ends_with(ext));
        if !library || base.contains('/') || base.contains("..") {
            continue;
        }
        let target = dir.join(base);
        let mut out = std::fs::File::create(&target)?;
        std::io::copy(&mut entry, &mut out)?;
        placed += 1;
    }
    if placed == 0 || !dir.join(plugin_file()).exists() {
        bail!("the plugin wheel held no {}", plugin_file());
    }
    Ok(())
}

// --------------------------------------------------------------- the session

/// What a worker embeds with.
pub enum Session {
    /// The int8 CPU session, run in a worker: the verification lane.
    Cpu(Box<fastembed::TextEmbedding>, String),
    /// fp16 on the GPU through ONNX Runtime directly, because fastembed has no
    /// way to hand a session a plugin device.
    Gpu(Box<GpuSession>),
    /// fp16 on an NVIDIA card through the CUDA pack's own ONNX Runtime.
    Cuda(Box<crate::cuda::Session>),
}

pub struct GpuSession {
    session: ort::session::Session,
    tokenizer: tokenizers::Tokenizer,
    device: String,
}

impl Session {
    pub fn open(lane: &str, dir: Option<&Path>) -> Result<Self> {
        let cache = crate::model_cache_dir()?;
        // Before anything touches ONNX Runtime: the CUDA worker loads the
        // pack's GPU core, and a CPU core loaded first would make that a
        // silent no-op.
        if lane == "cuda" {
            let pack = dir.context("the cuda lane needs its pack directory")?;
            let model = crate::accel::component_dir(&cache, &format!("webgpu-{WEBGPU_VERSION}"))
                .join("model_fp16.onnx");
            return Ok(Session::Cuda(Box::new(crate::cuda::Session::open(
                pack, &model,
            )?)));
        }
        crate::embed::link_runtime()?;
        match lane {
            "worker" => {
                let model =
                    crate::embed::Model::Granite.load(cache, crate::chunk::MAX_CHARS / 2, true)?;
                Ok(Session::Cpu(Box::new(model), crate::system::cpu_name()))
            }
            "gpu" => {
                let dir = dir.context("the gpu lane needs its component directory")?;
                Ok(Session::Gpu(Box::new(GpuSession::open(dir, &cache)?)))
            }
            other => bail!("unavailable — no lane called {other}"),
        }
    }

    pub fn device(&self) -> String {
        match self {
            Session::Cpu(_, name) => name.clone(),
            Session::Gpu(gpu) => gpu.device.clone(),
            Session::Cuda(cuda) => cuda.device(),
        }
    }

    pub fn variant(&self) -> &'static str {
        match self {
            Session::Cpu(..) => "int8-cpu",
            Session::Gpu(_) => "fp16-webgpu",
            Session::Cuda(_) => "fp16-cuda",
        }
    }

    pub fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        match self {
            Session::Cpu(model, _) => {
                let mut out = model
                    .embed(texts, Some(texts.len().max(1)))
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                for vector in &mut out {
                    crate::normalize(vector);
                }
                Ok(out)
            }
            Session::Gpu(gpu) => gpu.embed(texts),
            Session::Cuda(cuda) => cuda.embed(texts),
        }
    }
}

impl GpuSession {
    fn open(dir: &Path, cache: &Path) -> Result<Self> {
        use ort::AsPointer;
        use ort::environment::Environment;
        let env = Environment::current().context("starting ONNX Runtime")?;
        let library = env
            .register_ep_library("webgpu", dir.join(plugin_file()))
            .context("registering the WebGPU plugin")?;
        // Kept registered for the life of the worker, which is the life of the
        // session; unregistering at exit is the process ending.
        std::mem::forget(library);

        let mut chosen = Vec::new();
        let mut refused = Vec::new();
        for device in env.devices() {
            if device.ep().ok() == Some("CPUExecutionProvider") {
                continue;
            }
            let hardware = device.hardware_device();
            if hardware.ty() != ort::memory::DeviceType::GPU {
                continue;
            }
            // SAFETY: the pointer is the device ORT handed out for this
            // environment, alive as long as `env`, and the call only reads it.
            let vendor_id = unsafe { (ort::api().HardwareDevice_VendorId)(hardware.ptr()) };
            let name = hardware.vendor().unwrap_or("unknown").to_string();
            if is_software(vendor_id, &name) {
                refused.push(format!("{name} (vendor {vendor_id:04x})"));
                continue;
            }
            chosen.push((device, format!("{name} GPU (WebGPU)")));
        }
        let Some((_, label)) = chosen.first() else {
            if refused.is_empty() {
                bail!("unavailable — the WebGPU plugin found no hardware GPU");
            }
            bail!(
                "unavailable — only a software renderer was found ({}), which is never used",
                refused.join(", ")
            );
        };
        let label = label.clone();
        let devices: Vec<_> = chosen.into_iter().take(1).map(|(d, _)| d).collect();
        let session = ort::session::Session::builder()
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .with_devices(devices, None)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .commit_from_file(dir.join("model_fp16.onnx"))
            .map_err(|e| anyhow::anyhow!("loading the fp16 model on the GPU: {e}"))?;
        let tokenizer = crate::embed::granite_tokenizer(cache, crate::chunk::MAX_CHARS / 2)?;
        Ok(Self {
            session,
            tokenizer,
            device: label,
        })
    }

    /// The same preparation fastembed gives the CPU lane: its tokenizer with
    /// padding to the batch's longest and truncation at the same length,
    /// input ids and attention mask, and the CLS token's vector, normalised.
    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
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
            .map_err(|e| anyhow::anyhow!("running the model on the GPU: {e}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_renderers_are_refused_by_vendor_or_name() {
        assert!(is_software(0x10005, "llvmpipe (LLVM 17.0.6, 256 bits)"));
        assert!(is_software(0x1414, "Microsoft Basic Render Driver"));
        assert!(is_software(0, "SwiftShader Device (Subzero)"));
        assert!(is_software(0x1414, "Microsoft Hyper-V Video"));
        assert!(!is_software(0x10de, "NVIDIA GeForce RTX 3060"));
        assert!(!is_software(0x106b, "Apple"));
        assert!(!is_software(0x1002, "AMD Radeon RX 7800 XT"));
    }
}
