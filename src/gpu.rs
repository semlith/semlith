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
        // A directory an older semlith verified has no rewritten graph yet.
        attention_graph(&dir)?;
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
    attention_graph(&dir)?;
    crate::home::write_private(&stamp, WEBGPU_VERSION.as_bytes())?;
    Ok(dir)
}

/// The graph the WebGPU lane loads: the pinned fp16 graph with each of its
/// twelve `MultiHeadAttention` nodes written out as plain ops (#197). The
/// plugin's own attention kernel loses precision on every NVIDIA card tried,
/// fp16 or fp32 alike, and fails the known-answer check at 0.9989; the plain
/// ops pass. Only this lane loads it: CUDA, TensorRT and OpenVINO keep
/// `model_fp16.onnx`. It shares that graph's weights file.
pub const WEBGPU_GRAPH: &str = "model_fp16_webgpu.onnx";

/// What the rewrite of the pinned graph comes to, byte for byte. A change to
/// the rewrite changes this, and with it every cached copy is made again.
const WEBGPU_GRAPH_SHA256: &str =
    "9d0ebf0f7fd30bfe5192870265cb2cf8c41fb53e16437833a83d9502e8529a42";

/// Make [`WEBGPU_GRAPH`] beside the fp16 graph, once, on this machine: no
/// download, so a directory pre-seeded for `--airgap` by an older semlith
/// gets it too. Refused unless both the source and the result are the pinned
/// bytes.
fn attention_graph(dir: &Path) -> Result<()> {
    let target = dir.join(WEBGPU_GRAPH);
    if std::fs::read(&target).is_ok_and(|bytes| crate::embed::digest(&bytes) == WEBGPU_GRAPH_SHA256)
    {
        return Ok(());
    }
    let source = std::fs::read(dir.join("model_fp16.onnx")).context("reading the fp16 graph")?;
    if crate::embed::digest(&source) != FP16_FILES[0].1 {
        bail!("the fp16 graph in {} is not the pinned one", dir.display());
    }
    let graph = attention::rewrite(&source).context("rewriting the fp16 graph's attention")?;
    let got = crate::embed::digest(&graph);
    if got != WEBGPU_GRAPH_SHA256 {
        bail!(
            "the rewritten fp16 graph does not match the digest semlith pins for it \
             (expected {WEBGPU_GRAPH_SHA256}, got {got})"
        );
    }
    let partial = target.with_extension("partial");
    crate::home::write_private(&partial, &graph)?;
    std::fs::rename(&partial, &target).with_context(|| format!("placing {}", target.display()))
}

/// `MultiHeadAttention` as Reshape, Transpose, MatMul, Mul, Add and Softmax,
/// done on the ONNX protobuf's wire format directly: every other byte of the
/// graph is copied as it came. Only the shape the pinned graph uses is
/// accepted (query, key, value and an attention bias; `num_heads` and
/// `scale`); anything else is refused rather than guessed at.
mod attention {
    use anyhow::{Context, Result, bail};

    // Field numbers from onnx.proto.
    const MODEL_GRAPH: u32 = 7;
    const GRAPH_NODE: u32 = 1;
    const GRAPH_INITIALIZER: u32 = 5;
    const NODE_INPUT: u32 = 1;
    const NODE_OUTPUT: u32 = 2;
    const NODE_NAME: u32 = 3;
    const NODE_OP_TYPE: u32 = 4;
    const NODE_ATTRIBUTE: u32 = 5;
    const NODE_DOMAIN: u32 = 7;
    const ATTR_NAME: u32 = 1;
    const ATTR_F: u32 = 2;
    const ATTR_I: u32 = 3;
    const ATTR_INTS: u32 = 8;
    const ATTR_TYPE: u32 = 20;
    const TENSOR_DIMS: u32 = 1;
    const TENSOR_DATA_TYPE: u32 = 2;
    const TENSOR_NAME: u32 = 8;
    const TENSOR_RAW_DATA: u32 = 9;
    const INT: u64 = 2;
    const INTS: u64 = 7;
    const INT64: u64 = 7;
    const FLOAT16: u64 = 10;

    /// One field of a message: its number, its value (a varint's number or a
    /// length-delimited payload), and its whole encoding, tag included.
    pub(super) struct Field<'a> {
        pub number: u32,
        pub varint: u64,
        pub bytes: &'a [u8],
        pub raw: &'a [u8],
    }

    fn varint(buf: &[u8], at: &mut usize) -> Result<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *buf.get(*at).context("a varint runs past the end")?;
            *at += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        bail!("a varint longer than ten bytes")
    }

    pub(super) fn fields(buf: &[u8]) -> Result<Vec<Field<'_>>> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < buf.len() {
            let start = at;
            let tag = varint(buf, &mut at)?;
            let (number, wire) = ((tag >> 3) as u32, tag & 7);
            let (mut value, mut bytes) = (0, &buf[0..0]);
            match wire {
                0 => value = varint(buf, &mut at)?,
                1 | 5 => {
                    let width = if wire == 1 { 8 } else { 4 };
                    bytes = buf
                        .get(at..at + width)
                        .context("a fixed field runs past the end")?;
                    at += width;
                }
                2 => {
                    let len = varint(buf, &mut at)? as usize;
                    bytes = buf
                        .get(at..at.checked_add(len).context("a length overflows")?)
                        .context("a field runs past the end")?;
                    at += len;
                }
                other => bail!("wire type {other} is not used by ONNX"),
            }
            out.push(Field {
                number,
                varint: value,
                bytes,
                raw: &buf[start..at],
            });
        }
        Ok(out)
    }

    fn put_varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 {
            out.push(value as u8 | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }

    fn put_bytes(out: &mut Vec<u8>, number: u32, bytes: &[u8]) {
        put_varint(out, u64::from(number) << 3 | 2);
        put_varint(out, bytes.len() as u64);
        out.extend_from_slice(bytes);
    }

    fn put_u64(out: &mut Vec<u8>, number: u32, value: u64) {
        put_varint(out, u64::from(number) << 3);
        put_varint(out, value);
    }

    fn text(bytes: &[u8]) -> Result<&str> {
        std::str::from_utf8(bytes).context("a name that is not UTF-8")
    }

    /// One node: `inputs -> op -> outputs`, with its attributes encoded.
    fn node(out: &mut Vec<u8>, op: &str, inputs: &[&str], output: &str, attributes: &[Vec<u8>]) {
        let mut body = Vec::new();
        for input in inputs {
            put_bytes(&mut body, NODE_INPUT, input.as_bytes());
        }
        put_bytes(&mut body, NODE_OUTPUT, output.as_bytes());
        put_bytes(&mut body, NODE_OP_TYPE, op.as_bytes());
        for attribute in attributes {
            put_bytes(&mut body, NODE_ATTRIBUTE, attribute);
        }
        put_bytes(out, GRAPH_NODE, &body);
    }

    fn ints(name: &str, values: &[i64]) -> Vec<u8> {
        let mut out = Vec::new();
        put_bytes(&mut out, ATTR_NAME, name.as_bytes());
        for value in values {
            put_u64(&mut out, ATTR_INTS, *value as u64);
        }
        put_u64(&mut out, ATTR_TYPE, INTS);
        out
    }

    fn int(name: &str, value: i64) -> Vec<u8> {
        let mut out = Vec::new();
        put_bytes(&mut out, ATTR_NAME, name.as_bytes());
        put_u64(&mut out, ATTR_I, value as u64);
        put_u64(&mut out, ATTR_TYPE, INT);
        out
    }

    fn tensor(out: &mut Vec<u8>, name: &str, dims: &[i64], data_type: u64, raw: &[u8]) {
        let mut body = Vec::new();
        for dim in dims {
            put_u64(&mut body, TENSOR_DIMS, *dim as u64);
        }
        put_u64(&mut body, TENSOR_DATA_TYPE, data_type);
        put_bytes(&mut body, TENSOR_NAME, name.as_bytes());
        put_bytes(&mut body, TENSOR_RAW_DATA, raw);
        put_bytes(out, GRAPH_INITIALIZER, &body);
    }

    /// An f32 as IEEE half, rounded to nearest even. Only normal halves: the
    /// one value converted is attention's scale.
    pub(super) fn half(value: f32) -> Result<u16> {
        let bits = value.to_bits();
        let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
        if !(1..=30).contains(&exponent) {
            bail!("{value} is outside a half's normal range");
        }
        let mantissa = bits & 0x7f_ffff;
        let mut half = ((bits >> 16) & 0x8000) | ((exponent as u32) << 10) | (mantissa >> 13);
        let rest = mantissa & 0x1fff;
        if rest > 0x1000 || (rest == 0x1000 && half & 1 == 1) {
            half += 1;
        }
        Ok(half as u16)
    }

    pub fn rewrite(model: &[u8]) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(model.len() + 4096);
        let mut graphs = 0;
        for field in fields(model)? {
            if field.number == MODEL_GRAPH {
                put_bytes(&mut out, MODEL_GRAPH, &graph(field.bytes)?);
                graphs += 1;
            } else {
                out.extend_from_slice(field.raw);
            }
        }
        if graphs != 1 {
            bail!("a model with {graphs} graphs");
        }
        Ok(out)
    }

    fn graph(graph: &[u8]) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(graph.len() + 4096);
        let mut constants = Vec::new();
        let mut rewritten = 0;
        for field in fields(graph)? {
            if field.number != GRAPH_NODE {
                out.extend_from_slice(field.raw);
                continue;
            }
            let mut inputs = Vec::new();
            let (mut outputs, mut name, mut op, mut domain) = (Vec::new(), "", "", "");
            let (mut heads, mut scale, mut unknown) = (None, None, Vec::new());
            for part in fields(field.bytes)? {
                match part.number {
                    NODE_INPUT => inputs.push(text(part.bytes)?),
                    NODE_OUTPUT => outputs.push(text(part.bytes)?),
                    NODE_NAME => name = text(part.bytes)?,
                    NODE_OP_TYPE => op = text(part.bytes)?,
                    NODE_DOMAIN => domain = text(part.bytes)?,
                    NODE_ATTRIBUTE => {
                        let attribute = fields(part.bytes)?;
                        let named = attribute
                            .iter()
                            .find(|a| a.number == ATTR_NAME)
                            .map(|a| text(a.bytes))
                            .transpose()?;
                        let value = |number| attribute.iter().find(|a| a.number == number);
                        match named {
                            Some("num_heads") => heads = value(ATTR_I).map(|a| a.varint as i64),
                            Some("scale") => {
                                scale = value(ATTR_F)
                                    .and_then(|a| a.bytes.try_into().ok())
                                    .map(f32::from_le_bytes)
                            }
                            other => unknown.push(format!("{other:?}")),
                        }
                    }
                    _ => {}
                }
            }
            if op != "MultiHeadAttention" || domain != "com.microsoft" {
                out.extend_from_slice(field.raw);
                continue;
            }
            let at = |i: usize| inputs.get(i).copied().unwrap_or("");
            let (q, k, v, bias) = (at(0), at(1), at(2), at(5));
            let extra = [3, 4, 6, 7].iter().any(|&i| !at(i).is_empty()) || inputs.len() > 8;
            let (Some(heads), Some(scale), [output], true) =
                (heads, scale, outputs.as_slice(), unknown.is_empty())
            else {
                bail!(
                    "{name}: not the num_heads, scale and one output the rewrite handles \
                     (other attributes: {})",
                    unknown.join(", ")
                );
            };
            if q.is_empty() || k.is_empty() || v.is_empty() || extra {
                bail!("{name}: inputs other than query, key, value and attention bias");
            }
            let p = |suffix: &str| format!("{name}/{suffix}");
            let (shape, flat, factor) = (p("heads_shape"), p("flat_shape"), p("scale"));
            let raw = |values: &[i64]| {
                values
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>()
            };
            tensor(
                &mut constants,
                &shape,
                &[4],
                INT64,
                &raw(&[0, 0, heads, -1]),
            );
            tensor(&mut constants, &flat, &[3], INT64, &raw(&[0, 0, -1]));
            tensor(
                &mut constants,
                &factor,
                &[],
                FLOAT16,
                &half(scale)?.to_le_bytes(),
            );

            let perm = |order: &[i64]| vec![ints("perm", order)];
            for (input, key, order) in [
                (q, "q", [0, 2, 1, 3]),
                (k, "k", [0, 2, 3, 1]),
                (v, "v", [0, 2, 1, 3]),
            ] {
                node(
                    &mut out,
                    "Reshape",
                    &[input, &shape],
                    &p(&format!("{key}4")),
                    &[],
                );
                node(
                    &mut out,
                    "Transpose",
                    &[&p(&format!("{key}4"))],
                    &p(&format!("{key}t")),
                    &perm(&order),
                );
            }
            node(&mut out, "MatMul", &[&p("qt"), &p("kt")], &p("scores"), &[]);
            node(&mut out, "Mul", &[&p("scores"), &factor], &p("scaled"), &[]);
            let mut logits = p("scaled");
            if !bias.is_empty() {
                node(&mut out, "Add", &[&logits, bias], &p("biased"), &[]);
                logits = p("biased");
            }
            node(
                &mut out,
                "Softmax",
                &[&logits],
                &p("probs"),
                &[int("axis", -1)],
            );
            node(
                &mut out,
                "MatMul",
                &[&p("probs"), &p("vt")],
                &p("context4"),
                &[],
            );
            node(
                &mut out,
                "Transpose",
                &[&p("context4")],
                &p("context"),
                &perm(&[0, 2, 1, 3]),
            );
            node(&mut out, "Reshape", &[&p("context"), &flat], output, &[]);
            rewritten += 1;
        }
        if rewritten == 0 {
            bail!("no MultiHeadAttention node to rewrite");
        }
        out.extend_from_slice(&constants);
        Ok(out)
    }
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
pub(crate) fn download(
    url: &str,
    to: &Path,
    sha256: &str,
    progress: &mut dyn FnMut(u64),
) -> Result<()> {
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
    crate::add::note_outbound("pack", url);
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
    Cpu(Box<crate::session::CpuSession>, String),
    /// fp16 on the GPU through ONNX Runtime directly, because fastembed has no
    /// way to hand a session a plugin device.
    Gpu(Box<GpuSession>),
    /// fp16 on an NVIDIA card through the CUDA pack's own ONNX Runtime.
    Cuda(Box<crate::cuda::Session>),
    /// The Neural Engine, or the GPU, through Core ML: the pack's native
    /// models rather than an ONNX graph.
    #[cfg(target_os = "macos")]
    CoreMl(Box<crate::coreml::Session>),
    /// fp16 on an RTX-class NVIDIA card through NVIDIA's TensorRT for RTX
    /// plugin.
    Trt(Box<crate::trt::Session>),
    /// fp16 on an Intel GPU or NPU through Intel's OpenVINO plugin.
    OpenVino(Box<crate::openvino::Session>),
    /// granite as GGUF f16 in a `llama-server` the worker runs.
    Llama(Box<crate::llama::Session>),
}

pub struct GpuSession {
    session: ort::session::Session,
    device: String,
}

/// Names the WebGPU adapter to prefer, by a substring of its name, over the
/// one semlith would choose. `semlith doctor --gpu` prints the choice.
pub const ADAPTER_ENV: &str = "SEMLITH_GPU_ADAPTER";

impl Session {
    /// `progress(loaded, of)` is told as a lane with several models loads
    /// them: the Neural Engine's first load compiles each for this Mac.
    pub fn open(
        lane: &str,
        dir: Option<&Path>,
        adapter: Option<&str>,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<Self> {
        // Only the Core ML lanes load several models and say so.
        #[cfg(not(target_os = "macos"))]
        let _ = progress;
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
        #[cfg(target_os = "macos")]
        if matches!(lane, "ane" | "gpu-coreml") {
            let pack = dir.context("a Core ML lane needs its pack directory")?;
            let kind = if lane == "ane" {
                crate::coreml::Kind::NeuralEngine
            } else {
                crate::coreml::Kind::Gpu
            };
            return Ok(Session::CoreMl(Box::new(crate::coreml::Session::open(
                pack, kind, progress,
            )?)));
        }
        // llama.cpp is a server of its own; this process never loads ONNX
        // Runtime for it.
        if lane == "llama" {
            let pack = dir.context("the llama lane needs its pack directory")?;
            return Ok(Session::Llama(Box::new(crate::llama::Session::open(pack)?)));
        }
        crate::embed::link_runtime()?;
        match lane {
            "worker" => {
                let model = crate::session::CpuSession::open(
                    &cache,
                    crate::embed::Variant::Int8,
                    crate::embed::embed_threads(),
                    true,
                )?;
                Ok(Session::Cpu(Box::new(model), crate::system::cpu_name()))
            }
            "gpu" => {
                let dir = dir.context("the gpu lane needs its component directory")?;
                Ok(Session::Gpu(Box::new(GpuSession::open(dir, adapter)?)))
            }
            "trt" => {
                let pack = dir.context("the trt lane needs its pack directory")?;
                Ok(Session::Trt(Box::new(crate::trt::Session::open(pack)?)))
            }
            "openvino" => {
                let pack = dir.context("the openvino lane needs its pack directory")?;
                Ok(Session::OpenVino(Box::new(crate::openvino::Session::open(
                    pack,
                )?)))
            }
            other => bail!("unavailable — no lane called {other}"),
        }
    }

    pub fn device(&self) -> String {
        match self {
            Session::Cpu(_, name) => name.clone(),
            Session::Gpu(gpu) => gpu.device.clone(),
            Session::Cuda(cuda) => cuda.device(),
            #[cfg(target_os = "macos")]
            Session::CoreMl(coreml) => coreml.device(),
            Session::Trt(trt) => trt.device(),
            Session::OpenVino(openvino) => openvino.device(),
            Session::Llama(llama) => llama.device(),
        }
    }

    pub fn variant(&self) -> &'static str {
        match self {
            Session::Cpu(..) => "int8-cpu",
            Session::Gpu(_) => "fp16-webgpu",
            Session::Cuda(_) => "fp16-cuda",
            #[cfg(target_os = "macos")]
            Session::CoreMl(coreml) => coreml.variant(),
            Session::Trt(trt) => trt.variant(),
            Session::OpenVino(openvino) => openvino.variant(),
            Session::Llama(llama) => llama.variant(),
        }
    }

    /// One batch of token ids. The worker never tokenises: the run did,
    /// once, in its prepare stage.
    pub fn embed(&mut self, batch: &[&[u32]]) -> Result<Vec<Vec<f32>>> {
        match self {
            Session::Cpu(model, _) => model.embed(batch),
            Session::Gpu(gpu) => crate::session::run(&mut gpu.session, batch)
                .map_err(|e| anyhow::anyhow!("on the GPU: {e:#}")),
            Session::Cuda(cuda) => cuda.embed(batch),
            #[cfg(target_os = "macos")]
            Session::CoreMl(coreml) => coreml.embed(batch),
            Session::Trt(trt) => trt.embed(batch),
            Session::OpenVino(openvino) => openvino.embed(batch),
            Session::Llama(llama) => llama.embed(batch),
        }
    }
}

impl GpuSession {
    fn open(dir: &Path, named: Option<&str>) -> Result<Self> {
        use ort::AsPointer;
        use ort::environment::Environment;
        let env = Environment::current().context("starting ONNX Runtime")?;
        let library = env
            .register_ep_library("webgpu", dir.join(plugin_file()))
            .context("registering the WebGPU plugin")?;
        // Kept registered for the life of the worker, which is the life of the
        // session; unregistering at exit is the process ending.
        std::mem::forget(library);

        let mut devices = Vec::new();
        let mut candidates = Vec::new();
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
            candidates.push(Adapter {
                name,
                vendor: vendor_id,
            });
            devices.push(device);
        }
        let Some((at, why)) = choose(&candidates, named) else {
            if refused.is_empty() {
                bail!("unavailable — the WebGPU plugin found no hardware GPU");
            }
            bail!(
                "unavailable — only a software renderer was found ({}), which is never used",
                refused.join(", ")
            );
        };
        let label = format!("{} GPU (WebGPU; {why})", candidates[at].name);
        let devices = vec![devices.swap_remove(at)];
        let session = ort::session::Session::builder()
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .with_devices(devices, None)
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .commit_from_file(dir.join(WEBGPU_GRAPH))
            .map_err(|e| anyhow::anyhow!("loading the fp16 model on the GPU: {e}"))?;
        Ok(Self {
            session,
            device: label,
        })
    }
}

/// One hardware adapter the WebGPU plugin offered.
#[derive(Debug, Clone)]
pub struct Adapter {
    pub name: String,
    pub vendor: u32,
}

/// Whether a vendor's GPU is a discrete card. NVIDIA and AMD make discrete
/// cards (AMD's APUs are the exception this cannot see); Intel's, Apple's and
/// Qualcomm's are integrated.
fn discrete(vendor: u32) -> bool {
    matches!(vendor, 0x10de | 0x1002)
}

/// Which adapter the lane runs on, and why. A name the setting gives, by a
/// case-insensitive substring, wins; then a discrete GPU over an integrated
/// one, because the first adapter a machine lists is usually the one driving
/// the screen; then the first listed.
///
/// ponytail: discrete is read from the vendor. ORT's hardware-device metadata
/// carries the adapter's own answer on Windows; read it when an AMD APU turns
/// up on the wrong side of this.
pub fn choose(adapters: &[Adapter], named: Option<&str>) -> Option<(usize, String)> {
    if adapters.is_empty() {
        return None;
    }
    if let Some(named) = named.map(str::to_ascii_lowercase)
        && let Some(at) = adapters
            .iter()
            .position(|a| a.name.to_ascii_lowercase().contains(&named))
    {
        return Some((at, format!("named by the gpu_adapter setting ({named})")));
    }
    if adapters.len() == 1 {
        return Some((0, "the only hardware GPU".to_string()));
    }
    match adapters.iter().position(|a| discrete(a.vendor)) {
        Some(at) => Some((
            at,
            "a discrete GPU, preferred over an integrated one".to_string(),
        )),
        None => Some((0, "the first of several integrated GPUs".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_discrete_adapter_is_chosen_over_an_integrated_one_unless_one_is_named() {
        let adapters = vec![
            Adapter {
                name: "Intel(R) UHD Graphics 770".into(),
                vendor: 0x8086,
            },
            Adapter {
                name: "NVIDIA GeForce RTX 4070".into(),
                vendor: 0x10de,
            },
        ];
        let (at, why) = choose(&adapters, None).unwrap();
        assert_eq!(at, 1, "{why}");
        assert!(why.contains("discrete"));
        let (at, why) = choose(&adapters, Some("uhd")).unwrap();
        assert_eq!(at, 0);
        assert!(why.contains("named"));
        // A name nothing matches falls back to the rule.
        assert_eq!(choose(&adapters, Some("radeon")).unwrap().0, 1);
        assert!(choose(&[], None).is_none());
    }

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

    /// A message field, length-delimited, as protobuf writes it.
    fn field(number: u32, bytes: &[u8]) -> Vec<u8> {
        let mut out = vec![(number << 3 | 2) as u8];
        let mut len = bytes.len();
        while len >= 0x80 {
            out.push(len as u8 | 0x80);
            len >>= 7;
        }
        out.push(len as u8);
        out.extend_from_slice(bytes);
        out
    }

    /// `MultiHeadAttention` with these inputs, `num_heads` 2 and `scale`
    /// 0.5, in a model beside an `Add` the rewrite must leave alone.
    fn model(inputs: &[&str]) -> (Vec<u8>, Vec<u8>) {
        let add = [
            field(1, b"x"),
            field(1, b"y"),
            field(2, b"q"),
            field(4, b"Add"),
        ]
        .concat();
        let mut mha = Vec::new();
        for input in inputs {
            mha.extend(field(1, input.as_bytes()));
        }
        let heads = [field(1, b"num_heads"), vec![3 << 3, 2, 0xa0, 0x01, 2]].concat();
        let mut scale = field(1, b"scale");
        scale.push(2 << 3 | 5);
        scale.extend(0.5f32.to_le_bytes());
        mha.extend(
            [
                field(2, b"out"),
                field(3, b"/attn"),
                field(4, b"MultiHeadAttention"),
                field(5, &heads),
                field(5, &scale),
                field(7, b"com.microsoft"),
            ]
            .concat(),
        );
        let io = [field(11, b"x-info"), field(12, b"out-info")].concat();
        let graph = [field(1, &add), field(1, &mha), io.clone()].concat();
        let mut model = vec![1 << 3, 10]; // ir_version
        model.extend(field(7, &graph));
        (model, add)
    }

    fn graph_of(model: &[u8]) -> Vec<u8> {
        let fields = attention::fields(model).unwrap();
        fields
            .iter()
            .find(|f| f.number == 7)
            .unwrap()
            .bytes
            .to_vec()
    }

    fn op_of(node: &[u8]) -> String {
        let fields = attention::fields(node).unwrap();
        String::from_utf8(
            fields
                .iter()
                .find(|f| f.number == 4)
                .unwrap()
                .bytes
                .to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn attention_is_rewritten_as_plain_ops_and_every_other_byte_is_kept() {
        let (model, add) = model(&["q", "k", "v", "", "", "bias", "", ""]);
        let rewritten = attention::rewrite(&model).unwrap();
        // Deterministic: the pinned digest depends on it.
        assert_eq!(rewritten, attention::rewrite(&model).unwrap());
        let graph = graph_of(&rewritten);
        let fields = attention::fields(&graph).unwrap();
        let nodes: Vec<&[u8]> = fields
            .iter()
            .filter(|f| f.number == 1)
            .map(|f| f.bytes)
            .collect();
        let ops: Vec<String> = nodes.iter().map(|n| op_of(n)).collect();
        assert!(!ops.iter().any(|op| op == "MultiHeadAttention"), "{ops:?}");
        assert_eq!(nodes[0], add.as_slice(), "the Add is copied byte for byte");
        assert_eq!(
            ops[1..],
            [
                "Reshape",
                "Transpose",
                "Reshape",
                "Transpose",
                "Reshape",
                "Transpose",
                "MatMul",
                "Mul",
                "Add",
                "Softmax",
                "MatMul",
                "Transpose",
                "Reshape"
            ]
        );
        // The last node writes the attention's own output, so the rest of the
        // graph reads it unchanged.
        let last = attention::fields(nodes.last().unwrap()).unwrap();
        assert_eq!(last.iter().find(|f| f.number == 2).unwrap().bytes, b"out");
        // Graph inputs and outputs untouched; three constants added.
        let io: Vec<&[u8]> = fields
            .iter()
            .filter(|f| f.number == 11 || f.number == 12)
            .map(|f| f.raw)
            .collect();
        assert_eq!(
            io,
            [
                field(11, b"x-info").as_slice(),
                field(12, b"out-info").as_slice()
            ]
        );
        assert_eq!(fields.iter().filter(|f| f.number == 5).count(), 3);
        // Everything outside the graph is the source's.
        assert_eq!(&rewritten[..2], &model[..2]);
    }

    #[test]
    fn attention_with_inputs_the_rewrite_does_not_handle_is_refused() {
        let (masked, _) = model(&["q", "k", "v", "", "mask", "bias"]);
        assert!(attention::rewrite(&masked).is_err());
        let (none, _) = model(&["q", "k", "v"]);
        let (plain, _) = model(&[]);
        assert!(attention::rewrite(&none).is_ok(), "the bias is optional");
        assert!(attention::rewrite(&plain).is_err());
    }

    #[test]
    fn halves_round_to_nearest_even() {
        assert_eq!(attention::half(1.0).unwrap(), 0x3c00);
        assert_eq!(attention::half(0.5).unwrap(), 0x3800);
        // granite's attention scale, 1/sqrt(32), as numpy rounds it.
        assert_eq!(attention::half(0.176_776_69).unwrap(), 0x31a8);
        assert_eq!(attention::half(-2.0).unwrap(), 0xc000);
        assert!(attention::half(1e9).is_err());
    }

    /// The pinned digest, against the real graph:
    /// `SEMLITH_FP16_GRAPH=<model_fp16.onnx> cargo test -- --ignored rewritten_graph`.
    /// It writes the rewrite beside the source for a look with onnx's tools.
    #[test]
    #[ignore = "needs the pinned fp16 graph"]
    fn the_rewritten_graph_is_the_pinned_bytes() {
        let source = std::env::var("SEMLITH_FP16_GRAPH").expect("SEMLITH_FP16_GRAPH");
        let source = Path::new(&source);
        let dir = source.parent().unwrap();
        let _ = std::fs::remove_file(dir.join(WEBGPU_GRAPH));
        let result = attention_graph(dir);
        let rewritten = attention::rewrite(&std::fs::read(source).unwrap()).unwrap();
        std::fs::write(dir.join("model_fp16_webgpu.check.onnx"), &rewritten).unwrap();
        println!("digest {}", crate::embed::digest(&rewritten));
        result.unwrap();
    }
}
