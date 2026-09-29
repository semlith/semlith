//! The three experimental lanes built without their hardware: TensorRT for
//! RTX, OpenVINO and llama.cpp. The default set needs no network and checks
//! the pins and the decisions each lane makes. The `--ignored` ones run what
//! an Apple silicon Mac can: llama.cpp on the CPU and on Metal, through the
//! real worker, against the known-answer fixture.
//!
//! The llama.cpp ones need an unpacked pack, named by `SEMLITH_PACK_LLAMA`,
//! until the GGUF is published and pinned:
//!
//! ```sh
//! mkdir pack && tar xzf llama-b11146-bin-macos-arm64.tar.gz -C pack --strip-components 1
//! cp granite-f16.gguf pack/granite-embedding-small-english-r2-f16.gguf
//! SEMLITH_PACK_LLAMA=$PWD/pack cargo test --test blind_lanes -- --ignored --nocapture
//! ```

use semlith::{accel, llama, openvino, packs, trt};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

#[test]
fn every_pinned_asset_is_https_with_a_sha256() {
    for pack in [trt::pack(), openvino::pack(), llama::pack()] {
        for asset in pack.assets {
            assert!(asset.url.starts_with("https://"), "{}", asset.url);
            assert_eq!(asset.sha256.len(), 64, "{}", asset.url);
            assert!(
                asset.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
                "{}",
                asset.url
            );
            // A placeholder left from before a pack was published would
            // refuse every download.
            assert!(asset.sha256.bytes().any(|b| b != b'0'), "{}", asset.url);
            assert!(asset.size > 1_000_000, "{}", asset.url);
        }
    }
    // Where each is published, and nowhere else.
    let apple = cfg!(all(target_os = "macos", target_arch = "aarch64"));
    let x86 = cfg!(all(
        target_arch = "x86_64",
        any(windows, target_os = "linux")
    ));
    assert_eq!(trt::pack().assets.is_empty(), !x86);
    assert_eq!(openvino::pack().assets.is_empty(), !x86);
    assert_eq!(llama::pack().assets.len(), if apple || x86 { 2 } else { 0 });
}

#[test]
fn tensorrt_for_rtx_takes_rtx_class_cards_on_a_new_enough_driver() {
    let card = |capability: (i32, i32), driver: &str| {
        trt::judge(Some(trt::Card {
            name: "NVIDIA GeForce RTX 4070".to_string(),
            capability,
            driver: driver.to_string(),
        }))
    };
    for ok in [(7, 5), (8, 6), (8, 9), (12, 0), (12, 1)] {
        assert_eq!(
            card(ok, "570.133.07"),
            Ok("NVIDIA GeForce RTX 4070".to_string()),
            "{ok:?}"
        );
    }
    for data_centre in [(8, 0), (9, 0), (10, 0)] {
        let why = card(data_centre, "570.133.07").unwrap_err();
        assert!(why.contains("data-centre"), "{why}");
        assert!(why.contains("runs it"), "names the lane that does: {why}");
    }
    let old = card((7, 0), "570.133.07").unwrap_err();
    assert!(old.contains("older than Turing"), "{old}");
    let driver = card((8, 9), "470.256.02").unwrap_err();
    assert!(driver.contains(semlith::cuda::DRIVER_MIN), "{driver}");
    assert_eq!(trt::judge(None).unwrap_err(), "no NVIDIA card found");
}

#[test]
fn openvino_takes_a_gpu_then_an_npu_and_the_cpu_only_when_named() {
    let all = ["CPU", "GPU.0", "GPU.1", "NPU"].map(String::from);
    assert_eq!(openvino::choose(&all, None), Ok(1));
    assert_eq!(openvino::choose(&["CPU".into(), "NPU".into()], None), Ok(1));
    assert_eq!(
        openvino::choose(&["CPU".into()], None).unwrap_err(),
        "no Intel GPU or NPU found"
    );
    assert_eq!(openvino::choose(&all, Some("cpu")), Ok(0));
    assert_eq!(openvino::choose(&all, Some("GPU.1")), Ok(2));
    let why = openvino::choose(&["CPU".into()], Some("NPU")).unwrap_err();
    assert!(
        why.contains(openvino::DEVICE_ENV) && why.contains("CPU"),
        "{why}"
    );
}

#[test]
fn llama_cpp_offloads_to_a_hardware_gpu_only() {
    let mac = "Available devices:\n  MTL0: Apple M1 (5461 MiB, 5460 MiB free)\n  BLAS: Accelerate (0 MiB, 0 MiB free)\n";
    assert_eq!(
        llama::gpu_devices(mac),
        vec![("MTL0".to_string(), "Apple M1".to_string())]
    );
    let linux = "Available devices:\n  Vulkan0: llvmpipe (LLVM 17.0.6, 256 bits) (0 MiB, 0 MiB free)\n  Vulkan1: NVIDIA GeForce RTX 3060 (12288 MiB, 11000 MiB free)\n";
    assert_eq!(
        llama::gpu_devices(linux),
        vec![("Vulkan1".to_string(), "NVIDIA GeForce RTX 3060".to_string())]
    );
    assert!(llama::gpu_devices("Available devices:\n").is_empty());
}

#[test]
fn a_llama_answer_is_read_in_request_order_and_normalised() {
    let answer = serde_json::json!([
        { "index": 1, "embedding": [[0.0, 2.0]] },
        { "index": 0, "embedding": [3.0, 4.0] },
    ]);
    let vectors = llama::vectors_from(&answer, 2).unwrap();
    assert_eq!(vectors, vec![vec![0.6, 0.8], vec![0.0, 1.0]]);
    // One row may come back as an object alone.
    let one = serde_json::json!({ "index": 0, "embedding": [[1.0, 0.0]] });
    assert_eq!(llama::vectors_from(&one, 1).unwrap(), vec![vec![1.0, 0.0]]);
    let short = llama::vectors_from(&answer, 3).unwrap_err();
    assert!(
        format!("{short:#}").contains("2 vectors for 3"),
        "{short:#}"
    );
}

/// A lane whose pack is missing, or whose platform is not its own, says it
/// is unavailable rather than failing as broken.
#[test]
fn a_lane_without_its_pack_or_its_platform_is_unavailable() {
    let empty = tempfile::tempdir().unwrap();
    let text = |e: anyhow::Error| format!("{e:#}");
    let llama = text(llama::Session::open(empty.path()).err().unwrap());
    assert!(llama.starts_with("unavailable — "), "{llama}");
    for (id, error) in [
        ("trt", trt::Session::open(empty.path()).err().unwrap()),
        (
            "openvino",
            openvino::Session::open(empty.path()).err().unwrap(),
        ),
    ] {
        if let Some(why) = accel::unavailable_here(id) {
            assert_eq!(text(error), format!("unavailable — {why}"));
        }
    }
}

// ------------------------------------------------------ llama.cpp on this Mac

fn pack_dir() -> PathBuf {
    let dir = std::env::var_os("SEMLITH_PACK_LLAMA")
        .map(PathBuf::from)
        .expect("set SEMLITH_PACK_LLAMA to an unpacked llama.cpp pack (see this file's header)");
    assert!(dir.join("llama-server").exists(), "{}", dir.display());
    dir
}

/// A worker started as the daemon starts one, and the hello it said.
fn worker(device: Option<&str>) -> (Child, serde_json::Value) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_semlith"));
    command
        .args(["__embed-worker", "llama"])
        .arg(pack_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .env_remove(llama::DEVICE_ENV);
    if let Some(device) = device {
        command.env(llama::DEVICE_ENV, device);
    }
    let mut child = command.spawn().unwrap();
    let stdout = child.stdout.as_mut().unwrap();
    let hello = loop {
        let frame = accel::read_frame(stdout).expect("a hello");
        let value: serde_json::Value = serde_json::from_slice(&frame).unwrap();
        if value.get("loaded").is_none() {
            break value;
        }
    };
    (child, hello)
}

/// The llama-server a worker is running, by pid.
fn server_of(worker: &Child) -> u32 {
    let out = Command::new("pgrep")
        .args(["-P", &worker.id().to_string(), "llama-server"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .and_then(|pid| pid.trim().parse().ok())
        .expect("the worker runs a llama-server")
}

/// The addresses a process listens on, as `lsof` names them.
fn listening(pid: u32) -> Vec<String> {
    let out = Command::new("lsof")
        .args(["-nP", "-a", "-p", &pid.to_string(), "-iTCP", "-sTCP:LISTEN"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().nth(8).map(str::to_string))
        .collect()
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The known answer the worker checked itself, then a batch of the fixture's
/// ids through the frame protocol, then a clean exit that takes the server.
fn known_answer_and_round_trip(device: Option<&str>, backend: &str) {
    let (mut child, hello) = worker(device);
    println!("hello: {hello}");
    assert_eq!(hello["ok"], true, "{hello}");
    assert_eq!(hello["variant"], "gguf-f16");
    let device_name = hello["device"].as_str().unwrap();
    assert!(device_name.contains(backend), "{device_name}");
    let floor = accel::MIN_COSINE_FP16 as f64;
    assert!(hello["cosine"].as_f64().unwrap() >= floor, "{hello}");

    let server = server_of(&child);
    // Loopback only, and nothing answered without the worker's key.
    let addresses = listening(server);
    assert!(!addresses.is_empty());
    for address in &addresses {
        assert!(address.starts_with("127.0.0.1:"), "{addresses:?}");
        let refused = ureq::post(format!("http://{address}/embedding"))
            .content_type("application/json")
            .send(r#"{"content": [[50281, 50282]]}"#.as_bytes())
            .unwrap_err();
        assert!(matches!(refused, ureq::Error::StatusCode(401)), "{refused}");
    }
    let cache = semlith::model_cache_dir().unwrap();
    let tokenizer = semlith::session::tokenizer(&cache).unwrap();
    let (texts, expected) = accel::fixture();
    let batch: Vec<Vec<u32>> = texts
        .iter()
        .take(8)
        .map(|t| semlith::session::encode(&tokenizer, t).unwrap())
        .collect();
    let mut stdin = child.stdin.take().unwrap();
    accel::write_frame(&mut stdin, &accel::encode_ids(&batch)).unwrap();
    let answer = accel::read_frame(child.stdout.as_mut().unwrap()).unwrap();
    assert_eq!(answer[0], 0, "{}", String::from_utf8_lossy(&answer[1..]));
    let values: Vec<f32> = answer[1..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    assert_eq!(values.len(), batch.len() * 384);
    let worst = values
        .chunks(384)
        .zip(&expected)
        .map(|(got, want)| cosine(got, want))
        .fold(1.0f32, f32::min);
    println!(
        "{device_name}: batch of {} through the frames, lowest cosine {worst:.5}",
        batch.len()
    );
    assert!(worst >= accel::MIN_COSINE_FP16, "{worst}");

    drop(stdin);
    let status = child.wait().unwrap();
    assert!(status.success(), "{status}");
    assert!(!alive(server), "the server outlived its worker");
}

#[test]
#[ignore = "needs an unpacked llama.cpp pack in SEMLITH_PACK_LLAMA"]
fn llama_cpp_on_the_cpu_gives_the_known_answer() {
    known_answer_and_round_trip(Some("cpu"), "CPU");
}

#[test]
#[ignore = "needs an unpacked llama.cpp pack in SEMLITH_PACK_LLAMA and a Metal GPU"]
fn llama_cpp_on_metal_gives_the_known_answer() {
    known_answer_and_round_trip(None, "Metal");
}

/// The daemon stops an idle worker by killing it; its server must go too.
#[test]
#[ignore = "needs an unpacked llama.cpp pack in SEMLITH_PACK_LLAMA"]
fn a_killed_worker_takes_its_server_with_it() {
    let (mut child, hello) = worker(Some("cpu"));
    assert_eq!(hello["ok"], true, "{hello}");
    let server = server_of(&child);
    assert!(alive(server));
    child.kill().unwrap();
    child.wait().unwrap();
    let started = std::time::Instant::now();
    while alive(server) && started.elapsed() < std::time::Duration::from_secs(5) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(!alive(server), "the server outlived a killed worker");
}

/// The pinned macOS archive, fetched and unpacked by `packs::fetch` itself,
/// gives a `llama-server` that starts. It needs the unpacker to keep the
/// archive's library symlinks (`libggml-base.0.dylib` and the rest), which
/// the binary is linked against by those names.
#[test]
#[ignore = "downloads the pinned llama.cpp archive (11 MB)"]
fn the_fetched_macos_archive_gives_a_server_that_starts() {
    let pack = llama::pack();
    assert!(!pack.assets.is_empty(), "no llama.cpp build is pinned here");
    let archive = packs::Pack {
        name: "llama-archive",
        version: "test",
        assets: &pack.assets[..1],
    };
    let cache = tempfile::tempdir().unwrap();
    let dir = packs::fetch(cache.path(), &archive, &mut |_| {}).unwrap();
    let out = Command::new(dir.join("llama-server"))
        .arg("--list-devices")
        .output()
        .unwrap();
    let mut said = String::from_utf8_lossy(&out.stdout).into_owned();
    said.push_str(&String::from_utf8_lossy(&out.stderr));
    let first = said.lines().next().unwrap_or("");
    assert!(out.status.success(), "{first}");
    assert!(!llama::gpu_devices(&said).is_empty(), "{said}");
}
