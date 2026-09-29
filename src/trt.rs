//! The TensorRT for RTX lane: NVIDIA's plugin execution provider, from its
//! PyPI wheel, running the fp16 model on a GeForce/RTX-class card.
//!
//! It loads the way the WebGPU lane does — `register_ep_library` over the
//! wheel's library, then a session on the device the plugin offers — and is
//! gated first on what NVML says about the card, because TensorRT for RTX
//! builds for consumer and workstation parts only: compute capability 7.5,
//! 8.6, 8.9 and 12.x in NVIDIA's support matrix. A data-centre card (A100,
//! H100, B200) is refused with that reason; on Linux the CUDA lane runs it.
//!
//! NVIDIA's licence for TensorRT for RTX restricts publishing benchmark
//! results, so nothing here claims a speed. The hello's chunks per second is
//! a local reading and stays local.
//!
//! Also home to the two pieces the OpenVINO lane shares with this one:
//! [`fp16_model`] and [`plugin_devices`].

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------ the pack

/// The wheel's version and the CUDA major it is built against. cu12, not
/// cu13, because its driver floor is the CUDA lane's (`cuda::DRIVER_MIN`).
pub const VERSION: &str = "0.4.0-cu12";

const LINUX: &[crate::packs::Asset] = &[crate::packs::Asset {
    url: "https://files.pythonhosted.org/packages/52/a6/1405069d40e2d6d37b21031e5dc2cc8b8197fbb2e3d4e4f65b9155c57e4f/onnxruntime_ep_nv_tensorrt_rtx_cu12-0.4.0-py3-none-manylinux_2_28_x86_64.whl",
    sha256: "221807b1797a6270f37dfd7981f66e25ca7483f3883bfe23ef2b6a4198fe2b10",
    size: 167_721_016,
    form: crate::packs::Form::Wheel {
        prefix: "onnxruntime_ep_nv_tensorrt_rtx",
    },
}];

const WINDOWS: &[crate::packs::Asset] = &[crate::packs::Asset {
    url: "https://files.pythonhosted.org/packages/8b/e4/f19fdbffcf8faf18d9f81973f81e3d378ef371f857b049a3f355535a015c/onnxruntime_ep_nv_tensorrt_rtx_cu12-0.4.0-py3-none-win_amd64.whl",
    sha256: "5d9dba6aafd8e0863f34d0e5f51ad7c7ae9a5d6a92a55fe45d9637d3a5f1e1a7",
    size: 105_822_356,
    form: crate::packs::Form::Wheel {
        prefix: "onnxruntime_ep_nv_tensorrt_rtx",
    },
}];

/// NVIDIA's plugin for this platform; no assets where it is not published.
pub fn pack() -> crate::packs::Pack {
    crate::packs::Pack {
        name: "trt",
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
        "onnxruntime_providers_nv_tensorrt_rtx.dll"
    } else {
        "libonnxruntime_providers_nv_tensorrt_rtx.so"
    }
}

// ------------------------------------------------------------- the detection

/// What NVML says about the first NVIDIA card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub name: String,
    /// CUDA compute capability, major and minor.
    pub capability: (i32, i32),
    pub driver: String,
}

/// Whether TensorRT for RTX runs on this card: its name when it does, the
/// reason when it does not. A pure function so any machine's answer can be
/// checked without that machine.
pub fn judge(card: Option<Card>) -> std::result::Result<String, String> {
    let Some(Card {
        name,
        capability: (major, minor),
        driver,
    }) = card
    else {
        return Err("no NVIDIA card found".to_string());
    };
    if numbers(&driver) < numbers(crate::cuda::DRIVER_MIN) {
        return Err(format!(
            "{name} is on NVIDIA driver {driver}, below {}, the oldest TensorRT for RTX runs \
             on — update the driver",
            crate::cuda::DRIVER_MIN
        ));
    }
    // NVIDIA's support matrix for TensorRT for RTX 1.6: Turing (7.5), Ampere
    // GeForce (8.6), Ada (8.9) and Blackwell GeForce (12.x). A100 is 8.0,
    // Hopper 9.0 and data-centre Blackwell 10.x; none of them is supported.
    let supported = matches!((major, minor), (7, 5) | (8, 6) | (8, 9)) || major >= 12;
    if supported {
        return Ok(name);
    }
    let why = if (major, minor) < (7, 5) {
        "is older than Turing, the oldest generation TensorRT for RTX supports".to_string()
    } else {
        "is a data-centre or embedded part, which TensorRT for RTX does not support".to_string()
    };
    Err(format!(
        "{name} (compute capability {major}.{minor}) {why}; {}",
        fallback()
    ))
}

/// The lane that runs an NVIDIA card this one refuses.
fn fallback() -> &'static str {
    if cfg!(windows) {
        "the GPU lane runs it through WebGPU"
    } else {
        "the CUDA lane runs it"
    }
}

/// A dotted version as numbers, so `570.133.07` sorts after `525.60.13`.
fn numbers(version: &str) -> Vec<u64> {
    version
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

/// NVML's answer for device 0, read through the driver's own library.
///
/// ponytail: device 0 only, as `cuda::record`; the plugin's first GPU is
/// CUDA's device 0, which on a mixed box may not be NVML's. Pass a device id
/// through when somebody has two cards.
#[cfg(any(target_os = "linux", windows))]
fn nvml() -> Option<Card> {
    use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
    type Init = unsafe extern "C" fn() -> c_int;
    type Handle = unsafe extern "C" fn(c_uint, *mut *mut c_void) -> c_int;
    type Name = unsafe extern "C" fn(*mut c_void, *mut c_char, c_uint) -> c_int;
    type Capability = unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int) -> c_int;
    type Driver = unsafe extern "C" fn(*mut c_char, c_uint) -> c_int;
    type Shutdown = unsafe extern "C" fn() -> c_int;
    fn text(buffer: &[u8]) -> String {
        CStr::from_bytes_until_nul(buffer)
            .map(|c| c.to_string_lossy().trim().to_string())
            .unwrap_or_default()
    }

    let lib = native::Library::open(if cfg!(windows) {
        c"nvml.dll"
    } else {
        c"libnvidia-ml.so.1"
    })?;
    // SAFETY: each symbol is looked up by its NUL-terminated name and, when
    // present, used at the signature nvml.h declares for it. Every
    // out-parameter is a live local of the size NVML is told: 96 bytes for a
    // name (NVML_DEVICE_NAME_V2_BUFFER_SIZE), 80 for the driver version
    // (NVML_SYSTEM_DRIVER_VERSION_BUFFER_SIZE), two ints for the capability.
    // The device handle is used only between init and shutdown, and `lib`
    // outlives both.
    unsafe {
        let init: Init = lib.sym(c"nvmlInit_v2")?;
        let handle: Handle = lib.sym(c"nvmlDeviceGetHandleByIndex_v2")?;
        let name: Name = lib.sym(c"nvmlDeviceGetName")?;
        let capability: Capability = lib.sym(c"nvmlDeviceGetCudaComputeCapability")?;
        let driver: Driver = lib.sym(c"nvmlSystemGetDriverVersion")?;
        let shutdown: Shutdown = lib.sym(c"nvmlShutdown")?;
        if init() != 0 {
            return None;
        }
        let card = 'read: {
            let mut device: *mut c_void = std::ptr::null_mut();
            if handle(0, &mut device) != 0 {
                break 'read None;
            }
            let mut name_buffer = [0u8; 96];
            let (mut major, mut minor): (c_int, c_int) = (0, 0);
            let mut version = [0u8; 80];
            if name(device, name_buffer.as_mut_ptr().cast(), 96) != 0
                || capability(device, &mut major, &mut minor) != 0
                || driver(version.as_mut_ptr().cast(), 80) != 0
            {
                break 'read None;
            }
            Some(Card {
                name: text(&name_buffer),
                capability: (major, minor),
                driver: text(&version),
            })
        };
        shutdown();
        card
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn nvml() -> Option<Card> {
    None
}

// --------------------------------------------------------------- the session

/// What the TensorRT for RTX worker embeds with.
pub struct Session {
    session: ort::session::Session,
    device: String,
}

impl Session {
    /// Check the card, make the wheel's libraries findable, register the
    /// plugin and open the fp16 model on the GPU it offers. TensorRT builds
    /// an engine for the card on the first run of each shape; the runtime
    /// cache in the pack directory keeps that work across workers.
    pub fn open(pack: &Path) -> Result<Self> {
        if let Some(why) = crate::accel::unavailable_here("trt") {
            bail!("unavailable — {why}");
        }
        let card = judge(nvml()).map_err(|why| anyhow::anyhow!("unavailable — {why}"))?;
        let model = fp16_model()?;
        prepare(pack)?;
        let env = ort::environment::Environment::current().context("starting ONNX Runtime")?;
        let devices = plugin_devices(&env, "nv_tensorrt_rtx", &pack.join(plugin_file()))?;
        let Some(device) = devices
            .into_iter()
            .find(|d| d.hardware_device().ty() == ort::memory::DeviceType::GPU)
        else {
            bail!("unavailable — the TensorRT for RTX plugin offered no GPU for {card}");
        };
        let ep = device.ep().map_err(|e| anyhow::anyhow!("{e}"))?.to_string();
        let cache = pack.join("runtime-cache");
        let mut options = Vec::new();
        // A read-only pack (a development override) runs without the cache
        // rather than not at all.
        if std::fs::create_dir_all(&cache).is_ok() {
            options.push((
                format!("{ep}.nv_runtime_cache_path"),
                crate::plain(&cache.display().to_string()),
            ));
        }
        let session = ort::session::Session::builder()
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .with_devices([device], Some(&options))
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .commit_from_file(&model)
            .map_err(|e| anyhow::anyhow!("loading the fp16 model with TensorRT for RTX: {e}"))?;
        Ok(Self {
            session,
            device: format!("{card} (TensorRT for RTX)"),
        })
    }

    pub fn device(&self) -> String {
        self.device.clone()
    }

    pub fn variant(&self) -> &'static str {
        "fp16-trt"
    }

    pub fn embed(&mut self, batch: &[&[u32]]) -> Result<Vec<Vec<f32>>> {
        crate::session::run(&mut self.session, batch)
            .map_err(|e| anyhow::anyhow!("on TensorRT for RTX: {e:#}"))
    }
}

/// The wheel ships each TensorRT library once, at its full version, and the
/// plugin's RUNPATH is NVIDIA's build machine, not `$ORIGIN`. So, as the
/// wheel's own `__init__.py` does: the SONAME and unversioned names are
/// linked beside each library (TensorRT opens `libtensorrt_rtx.so` by that
/// name while it builds), and the three the plugin needs are loaded by full
/// path, globally, so the plugin's `NEEDED` entries resolve to them.
#[cfg(target_os = "linux")]
fn prepare(pack: &Path) -> Result<()> {
    // In load order: TensorRT before its ONNX parser, as NVIDIA's loader has it.
    const LINKS: &[(&str, &[&str])] = &[
        ("libcudart.so.12.9.79", &["libcudart.so.12"]),
        (
            "libtensorrt_rtx.so.1.6.1",
            &["libtensorrt_rtx.so.1", "libtensorrt_rtx.so"],
        ),
        (
            "libtensorrt_onnxparser_rtx.so.1.6.1",
            &[
                "libtensorrt_onnxparser_rtx.so.1",
                "libtensorrt_onnxparser_rtx.so",
            ],
        ),
    ];
    for (real, aliases) in LINKS {
        for alias in *aliases {
            let link = pack.join(alias);
            if std::fs::symlink_metadata(&link).is_err() {
                // A read-only pack keeps working through the preloads below.
                let _ = std::os::unix::fs::symlink(real, &link);
            }
        }
    }
    for (real, _) in LINKS {
        native::preload(&pack.join(real))?;
    }
    Ok(())
}

/// Windows resolves the plugin's delay-loaded TensorRT and CUDA runtime DLLs
/// through the process's search path, which does not include the plugin's
/// own directory.
#[cfg(windows)]
fn prepare(pack: &Path) -> Result<()> {
    native::search_beside(pack)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn prepare(_pack: &Path) -> Result<()> {
    bail!("unavailable — the TensorRT for RTX lane is built for Windows and Linux on x86_64")
}

// ------------------------------------------------- shared with the OpenVINO lane

/// The fp16 export the WebGPU lane fetched, which the plugin lanes run too.
pub(crate) fn fp16_model() -> Result<PathBuf> {
    let cache = crate::model_cache_dir()?;
    let model =
        crate::accel::component_dir(&cache, &format!("webgpu-{}", crate::gpu::WEBGPU_VERSION))
            .join("model_fp16.onnx");
    if !model.exists() {
        bail!(
            "the fp16 model is not in {} — turning the lane on again fetches it",
            crate::plain(&model.display().to_string())
        );
    }
    Ok(model)
}

/// Register a plugin library and return the devices it added: every device
/// whose provider was not listed before it was registered. Nothing else in
/// a worker registers a plugin, so that is exactly the plugin's own.
pub(crate) fn plugin_devices<'e>(
    env: &'e std::sync::Arc<ort::environment::Environment>,
    name: &str,
    library: &Path,
) -> Result<Vec<ort::device::Device<'e>>> {
    if !library.exists() {
        bail!(
            "{} is not in the pack — turning the lane off and on again fetches it afresh",
            crate::plain(&library.display().to_string())
        );
    }
    let before: Vec<String> = env
        .devices()
        .filter_map(|d| d.ep().ok().map(str::to_string))
        .collect();
    let registered = env
        .register_ep_library(name, library)
        .with_context(|| format!("registering {}", library.display()))?;
    // Kept registered for the life of the worker, which is the life of the
    // session, as the WebGPU plugin is.
    std::mem::forget(registered);
    Ok(env
        .devices()
        .filter(|d| d.ep().is_ok_and(|ep| !before.iter().any(|b| b == ep)))
        .collect())
}

/// The dynamic-loader calls the two plugin lanes make, per platform.
pub(crate) mod native {
    #[cfg(target_os = "linux")]
    use anyhow::bail;
    #[cfg(any(target_os = "linux", windows))]
    use std::ffi::{CStr, c_void};

    /// A library opened by name, closed when dropped.
    #[cfg(any(target_os = "linux", windows))]
    pub struct Library(*mut c_void);

    #[cfg(windows)]
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryA(name: *const std::ffi::c_char) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
        fn FreeLibrary(module: *mut c_void) -> i32;
        fn SetDllDirectoryW(path: *const u16) -> i32;
    }

    #[cfg(any(target_os = "linux", windows))]
    impl Library {
        pub fn open(name: &CStr) -> Option<Self> {
            // SAFETY: a NUL-terminated name that outlives the call; a null
            // answer is checked before the handle is kept.
            #[cfg(target_os = "linux")]
            let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
            #[cfg(windows)]
            let handle = unsafe { LoadLibraryA(name.as_ptr()) };
            // Never `then_some(Self(handle))`: that builds the value first,
            // and a `Library(null)` dropped unused is `dlclose(NULL)`, a
            // segfault on every Linux machine without NVIDIA's driver.
            if handle.is_null() {
                None
            } else {
                Some(Self(handle))
            }
        }

        /// # Safety
        /// `F` must be the function-pointer type the symbol has in C.
        pub unsafe fn sym<F: Copy>(&self, name: &CStr) -> Option<F> {
            assert_eq!(size_of::<F>(), size_of::<*mut c_void>());
            // SAFETY: a live handle and a NUL-terminated name; the caller
            // vouches for `F`, which is pointer-sized, as asserted.
            unsafe {
                #[cfg(target_os = "linux")]
                let found = libc::dlsym(self.0, name.as_ptr());
                #[cfg(windows)]
                let found = GetProcAddress(self.0, name.as_ptr());
                (!found.is_null()).then(|| std::mem::transmute_copy(&found))
            }
        }
    }

    #[cfg(any(target_os = "linux", windows))]
    impl Drop for Library {
        fn drop(&mut self) {
            // SAFETY: the handle this value opened, closed once.
            unsafe {
                #[cfg(target_os = "linux")]
                libc::dlclose(self.0);
                #[cfg(windows)]
                FreeLibrary(self.0);
            }
        }
    }

    /// Load a library by full path, globally, for the life of the process.
    #[cfg(target_os = "linux")]
    pub fn preload(path: &std::path::Path) -> anyhow::Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
        // SAFETY: a NUL-terminated path that outlives the call. The handle is
        // deliberately never closed: the library must live as long as the
        // worker. `dlerror` is read on the same thread straight after the
        // failure it describes, null-checked, and copied.
        let why = unsafe {
            let handle = libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL);
            if !handle.is_null() {
                return Ok(());
            }
            let text = libc::dlerror();
            if text.is_null() {
                "no reason given".to_string()
            } else {
                CStr::from_ptr(text).to_string_lossy().into_owned()
            }
        };
        bail!(
            "loading {}: {why} — turning the lane off and on again fetches the pack afresh",
            path.display()
        )
    }

    /// Put `dir` on this process's DLL search path, after the executable's
    /// own directory. A worker runs one lane, so nothing else is affected.
    #[cfg(windows)]
    pub fn search_beside(dir: &std::path::Path) -> anyhow::Result<()> {
        let plain = crate::plain(&dir.display().to_string());
        let wide: Vec<u16> = plain.encode_utf16().chain(Some(0)).collect();
        // SAFETY: a NUL-terminated UTF-16 path that outlives the call, which
        // copies it.
        if unsafe { SetDllDirectoryW(wide.as_ptr()) } == 0 {
            anyhow::bail!("could not add {plain} to the DLL search path");
        }
        Ok(())
    }
}
