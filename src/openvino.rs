//! The OpenVINO lane: Intel's plugin execution provider, from its PyPI wheel,
//! running the fp16 model on an Intel GPU or NPU.
//!
//! It loads as the WebGPU and TensorRT lanes do — `register_ep_library`, then
//! a session on one of the devices the plugin offers. The plugin names each
//! device in its metadata (`ov_device`: `CPU`, `GPU`, `GPU.1`, `NPU`), and the
//! lane takes an Intel GPU first, then an NPU. OpenVINO's CPU device is never
//! chosen on its own — the CPU is the CPU lane's — and is used only when
//! [`DEVICE_ENV`] names it, which is how CI proves the known answer on a
//! runner with no Intel graphics.
//!
//! An NPU compiles for static shapes only, so there the model is compiled
//! once per sequence bucket at a fixed batch, and every batch is padded (and
//! masked) up to one of them.

use anyhow::{Context, Result, bail};
use std::path::Path;

/// Forces one OpenVINO device: `GPU`, `NPU` or `CPU` (or `GPU.1`, as the
/// plugin names it).
pub const DEVICE_ENV: &str = "SEMLITH_OPENVINO_DEVICE";

pub const VERSION: &str = "1.7.0";

const LINUX: &[crate::packs::Asset] = &[crate::packs::Asset {
    url: "https://files.pythonhosted.org/packages/78/b6/7d168ff711e6034c4df4c5144242a230c7e6329f24b50454103510671b85/onnxruntime_ep_openvino-1.7.0-py3-none-manylinux_2_28_x86_64.whl",
    sha256: "7016c5ba4154f2a2b6da87e8b8654e5e811151186fb97cc29bde5d2fabde6c93",
    size: 51_725_826,
    form: crate::packs::Form::Wheel {
        prefix: "onnxruntime_ep_openvino",
    },
}];

const WINDOWS: &[crate::packs::Asset] = &[crate::packs::Asset {
    url: "https://files.pythonhosted.org/packages/c3/c7/4ebac45e18e3c32655948f831ac95711849824bd7b3e793c3f8c6bc2060e/onnxruntime_ep_openvino-1.7.0-py3-none-win_amd64.whl",
    sha256: "0b48601ff6720a9fdbf06de99073665cec9b8d7983226455d4f766aa4d8be16c",
    size: 67_675_843,
    form: crate::packs::Form::Wheel {
        prefix: "onnxruntime_ep_openvino",
    },
}];

/// Intel's plugin for this platform; no assets where it is not published.
pub fn pack() -> crate::packs::Pack {
    crate::packs::Pack {
        name: "openvino",
        version: VERSION,
        assets: if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            LINUX
        } else if cfg!(all(windows, target_arch = "x86_64")) {
            WINDOWS
        } else {
            &[]
        },
    }
}

fn plugin_file() -> &'static str {
    if cfg!(windows) {
        "onnxruntime_providers_openvino_plugin.dll"
    } else {
        "libonnxruntime_providers_openvino_plugin.so"
    }
}

/// Which of the plugin's devices the lane runs on, by their `ov_device`
/// names. A forced name matches a device whose name starts with it, so `GPU`
/// takes `GPU.0`. Without one: the first GPU, then the first NPU, and never
/// the CPU.
///
/// ponytail: the first GPU OpenVINO lists, which is the integrated one on a
/// machine with both; prefer a discrete `GPU.1` when somebody has an Arc card
/// beside Iris Xe.
pub fn choose(devices: &[String], forced: Option<&str>) -> std::result::Result<usize, String> {
    let find = |prefix: &str| {
        devices
            .iter()
            .position(|d| d.to_ascii_uppercase().starts_with(prefix))
    };
    if let Some(forced) = forced.map(|f| f.trim().to_ascii_uppercase()) {
        return find(&forced).ok_or_else(|| {
            format!(
                "{DEVICE_ENV}={forced}, and OpenVINO found {}",
                if devices.is_empty() {
                    "no device".to_string()
                } else {
                    devices.join(", ")
                }
            )
        });
    }
    find("GPU")
        .or_else(|| find("NPU"))
        .ok_or_else(|| "no Intel GPU or NPU found".to_string())
}

/// Rows in every NPU batch, and the sequence lengths it is compiled for.
/// granite's chunks are cut at 400 tokens, so 512 holds the longest.
const NPU_ROWS: usize = 8;
const NPU_BUCKETS: &[usize] = &[64, 128, 256, 512];

/// What the OpenVINO worker embeds with.
pub struct Session {
    runs: Runs,
    device: String,
}

enum Runs {
    /// A GPU or the CPU: dynamic shapes, one session.
    Dynamic(ort::session::Session),
    /// An NPU: one session per sequence bucket, each at `NPU_ROWS` rows.
    Fixed(Vec<(usize, ort::session::Session)>),
}

impl Session {
    pub fn open(pack: &Path) -> Result<Self> {
        if let Some(why) = crate::accel::unavailable_here("openvino") {
            bail!("unavailable — {why}");
        }
        let model = crate::trt::fp16_model()?;
        #[cfg(windows)]
        crate::trt::native::search_beside(pack)?;
        let env = ort::environment::Environment::current().context("starting ONNX Runtime")?;
        let devices = crate::trt::plugin_devices(&env, "openvino", &pack.join(plugin_file()))?;
        let names: Vec<String> = devices.iter().map(ov_device).collect();
        // The plugin offers a device only where the hardware's vendor is
        // Intel: an AMD CPU with no Intel GPU gets nothing, not even the CPU.
        if names.is_empty() {
            bail!(
                "unavailable — OpenVINO offers Intel devices only, and this machine has none \
                 ({})",
                crate::system::cpu_name()
            );
        }
        let forced = std::env::var(DEVICE_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty());
        let at = choose(&names, forced.as_deref())
            .map_err(|why| anyhow::anyhow!("unavailable — {why}"))?;
        let name = names[at].clone();
        let vendor = devices[at]
            .hardware_device()
            .vendor()
            .unwrap_or("Intel")
            .to_string();
        let ep = devices[at]
            .ep()
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .to_string();
        let npu = name.to_ascii_uppercase().starts_with("NPU");

        // Precision: the CPU runs f32 (a Xeon with AMX would otherwise pick
        // bf16, below the known answer's floor); a GPU keeps the model's own
        // fp16, as the WebGPU lane does; an NPU runs f16, its only precision.
        // Compiled blobs are cached in the pack, so a second worker loads
        // rather than compiles.
        let hint = match name.to_ascii_uppercase().get(..3) {
            Some("CPU") => serde_json::json!({ "INFERENCE_PRECISION_HINT": "f32" }),
            Some("NPU") => serde_json::json!({ "INFERENCE_PRECISION_HINT": "f16" }),
            _ => serde_json::json!({ "EXECUTION_MODE_HINT": "ACCURACY" }),
        };
        let mut config = hint;
        let cache = pack.join("compiled");
        if std::fs::create_dir_all(&cache).is_ok() {
            config["CACHE_DIR"] = crate::plain(&cache.display().to_string()).into();
        }
        let mut root = serde_json::Map::new();
        root.insert(name.clone(), config);
        let load_config = serde_json::Value::Object(root).to_string();
        // Each NPU bucket is a session of its own on the same device, and a
        // device handle is not `Clone`: it is found again by its pointer.
        let target = {
            use ort::AsPointer;
            devices[at].ptr()
        };

        let build = |reshape: Option<String>| -> Result<ort::session::Session> {
            let mut options = vec![(format!("{ep}.load_config"), load_config.clone())];
            if let Some(shape) = reshape {
                options.push((format!("{ep}.reshape_input"), shape));
            }
            use ort::AsPointer;
            let device = env
                .devices()
                .find(|d| d.ptr() == target)
                .context("the OpenVINO device went away")?;
            ort::session::Session::builder()
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .with_devices([device], Some(&options))
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .commit_from_file(&model)
                .map_err(|e| anyhow::anyhow!("loading the fp16 model on OpenVINO {name}: {e}"))
        };
        let runs = if npu {
            let mut fixed = Vec::new();
            for &seq in NPU_BUCKETS {
                let shape = format!("input_ids[{NPU_ROWS},{seq}],attention_mask[{NPU_ROWS},{seq}]");
                fixed.push((seq, build(Some(shape))?));
            }
            Runs::Fixed(fixed)
        } else {
            Runs::Dynamic(build(None)?)
        };
        Ok(Self {
            runs,
            device: format!("{vendor} {name} (OpenVINO)"),
        })
    }

    pub fn device(&self) -> String {
        self.device.clone()
    }

    pub fn variant(&self) -> &'static str {
        "openvino"
    }

    pub fn embed(&mut self, batch: &[&[u32]]) -> Result<Vec<Vec<f32>>> {
        match &mut self.runs {
            Runs::Dynamic(session) => crate::session::run(session, batch)
                .map_err(|e| anyhow::anyhow!("on OpenVINO: {e:#}")),
            Runs::Fixed(buckets) => {
                let mut vectors = Vec::with_capacity(batch.len());
                for group in batch.chunks(NPU_ROWS) {
                    let longest = group.iter().map(|ids| ids.len()).max().unwrap_or(1);
                    let Some((seq, session)) = buckets.iter_mut().find(|(seq, _)| *seq >= longest)
                    else {
                        bail!(
                            "a chunk of {longest} tokens is longer than the NPU's largest shape \
                             ({})",
                            NPU_BUCKETS.last().copied().unwrap_or(0)
                        );
                    };
                    vectors.extend(
                        run_fixed(session, group, NPU_ROWS, *seq)
                            .map_err(|e| anyhow::anyhow!("on the NPU: {e:#}"))?,
                    );
                }
                Ok(vectors)
            }
        }
    }
}

/// The plugin's name for a device: its `ov_device` metadata, or the hardware
/// type when a build of the plugin does not say.
fn ov_device(device: &ort::device::Device<'_>) -> String {
    use ort::AsPointer;
    let api = ort::api();
    // SAFETY: the pointer is a device ORT handed out for this environment,
    // alive as long as `device` is. The key-value pairs belong to the device
    // and are only read; the value is copied before anything else runs.
    let named = unsafe {
        let pairs = (api.EpDevice_EpMetadata)(device.ptr());
        let value = if pairs.is_null() {
            std::ptr::null()
        } else {
            (api.GetKeyValue)(pairs, c"ov_device".as_ptr())
        };
        (!value.is_null()).then(|| {
            std::ffi::CStr::from_ptr(value)
                .to_string_lossy()
                .into_owned()
        })
    };
    named.unwrap_or_else(|| {
        match device.hardware_device().ty() {
            ort::memory::DeviceType::CPU => "CPU",
            ort::memory::DeviceType::GPU => "GPU",
            ort::memory::DeviceType::NPU => "NPU",
        }
        .to_string()
    })
}

/// One NPU batch at its compiled shape: `rows` by `seq`, the real rows first
/// and filler rows after, whose vectors are dropped. Every row attends to at
/// least its first position, so no row's softmax is over nothing.
fn run_fixed(
    session: &mut ort::session::Session,
    group: &[&[u32]],
    rows: usize,
    seq: usize,
) -> Result<Vec<Vec<f32>>> {
    let mut ids = vec![crate::session::PAD_ID as i64; rows * seq];
    let mut mask = vec![0i64; rows * seq];
    for row in 0..rows {
        mask[row * seq] = 1;
    }
    for (row, chunk) in group.iter().enumerate() {
        for (at, id) in chunk.iter().enumerate() {
            ids[row * seq + at] = *id as i64;
            mask[row * seq + at] = 1;
        }
    }
    let outputs = session
        .run(ort::inputs![
            "input_ids" => ort::value::Tensor::from_array(([rows, seq], ids)).map_err(|e| anyhow::anyhow!("{e}"))?,
            "attention_mask" => ort::value::Tensor::from_array(([rows, seq], mask)).map_err(|e| anyhow::anyhow!("{e}"))?,
        ])
        .map_err(|e| anyhow::anyhow!("running the model: {e}"))?;
    let (shape, data) = outputs[0]
        .try_extract_tensor::<f32>()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let dim = *shape.last().context("an output with no shape")? as usize;
    Ok((0..group.len())
        .map(|row| {
            let start = row * seq * dim;
            let mut vector = data[start..start + dim].to_vec();
            crate::normalize(&mut vector);
            vector
        })
        .collect())
}
