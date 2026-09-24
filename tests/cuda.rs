//! The CUDA lane's detection, pack and session, as far as a machine without
//! an NVIDIA card can check them. The default set needs no network; the
//! `--ignored` ones download.

use semlith::cuda::{self, Recorded};
use std::path::Path;

/// The three recorded NVML answers the 0.28.0 contract names.
#[test]
fn nvml_detection_gives_the_recorded_answers() {
    let rtx = cuda::judge(Recorded::Device {
        name: "NVIDIA GeForce RTX 3060".to_string(),
        memory_mb: 12_288,
        driver: "570.133.07".to_string(),
    })
    .expect("an RTX 3060 on driver 570 is used");
    assert_eq!(rtx.name, "NVIDIA GeForce RTX 3060");
    assert_eq!(rtx.memory_mb, 12_288);
    assert_eq!(rtx.driver, "570.133.07");

    let old = cuda::judge(Recorded::Device {
        name: "NVIDIA GeForce GTX 1080".to_string(),
        memory_mb: 8_192,
        driver: "470.256.02".to_string(),
    })
    .expect_err("a driver below the CUDA 12 minimum is refused");
    assert!(old.contains(cuda::DRIVER_MIN), "{old}");
    assert!(old.contains("470.256.02"), "{old}");

    assert_eq!(
        cuda::judge(Recorded::NoNvml).unwrap_err(),
        "no NVIDIA device"
    );
}

#[test]
fn the_driver_minimum_compares_as_numbers_not_text() {
    let on = |driver: &str| {
        cuda::judge(Recorded::Device {
            name: "card".to_string(),
            memory_mb: 0,
            driver: driver.to_string(),
        })
        .is_ok()
    };
    assert!(on("525.60.13"));
    assert!(on("1000.1"));
    assert!(!on("525.60.9"));
    assert!(!on("99.0"));
    assert!(!on("garbage"));
}

#[test]
fn the_stated_size_is_the_sum_of_the_parts() {
    let sum: u64 = cuda::PARTS.iter().map(|part| part.size).sum();
    assert_eq!(cuda::PACK_BYTES, sum);
    assert!(
        (1_000_000_000..=2_600_000_000).contains(&cuda::PACK_BYTES),
        "the contract says 1 to 2.6 GB"
    );
    for part in cuda::PARTS {
        assert_eq!(part.sha256.len(), 64, "{}", part.name);
        assert!(part.url.starts_with("https://"), "{}", part.name);
    }
}

/// A file that does not match its pinned digest is refused and deleted.
#[test]
fn a_file_with_the_wrong_digest_is_refused_and_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("libcudart.so.12");
    std::fs::write(&file, b"not the CUDA runtime").unwrap();
    let err = cuda::verify_file(&file, cuda::PARTS[1].sha256).unwrap_err();
    assert!(format!("{err:#}").contains("does not match"), "{err:#}");
    assert!(!file.exists(), "a refused file must be deleted");

    // The same bytes, pinned correctly, are kept.
    std::fs::write(&file, b"abc").unwrap();
    cuda::verify_file(
        &file,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    )
    .unwrap();
    assert!(file.exists());
}

/// An installed pack is answered without the network, whatever `--airgap`
/// says; nothing is fetched to find that out.
#[test]
fn an_installed_pack_is_found_without_fetching() {
    let cache = tempfile::tempdir().unwrap();
    assert!(cuda::pack_installed(cache.path()).is_none());
    let dir = cache
        .path()
        .join("accel")
        .join(format!("cuda-{}", cuda::PACK_VERSION));
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    std::fs::write(dir.join("lib").join("libonnxruntime.so"), b"").unwrap();
    assert!(
        cuda::pack_installed(cache.path()).is_none(),
        "not without the stamp"
    );
    std::fs::write(dir.join(".semlith-verified"), b"").unwrap();
    assert_eq!(cuda::pack_installed(cache.path()), Some(dir.clone()));
    assert_eq!(cuda::fetch_pack(cache.path(), &mut |_| {}).unwrap(), dir);
}

/// Without a pack, `--airgap` refuses before anything is fetched.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn airgap_refuses_the_pack_unless_it_is_already_here() {
    let cache = tempfile::tempdir().unwrap();
    // SAFETY: only this test reads the variable, and it is removed after.
    unsafe { std::env::set_var(semlith::embed::AIRGAP_ENV, "1") };
    let err = cuda::fetch_pack(cache.path(), &mut |_| {});
    unsafe { std::env::remove_var(semlith::embed::AIRGAP_ENV) };
    let err = format!("{:#}", err.unwrap_err());
    assert!(err.contains(semlith::embed::AIRGAP_ENV), "{err}");
}

#[test]
fn the_lane_says_why_it_cannot_run_here() {
    let linux = cfg!(all(target_os = "linux", target_arch = "x86_64"));
    assert_eq!(cuda::unavailable_here().is_none(), linux);
    if cfg!(windows) {
        assert!(
            cuda::unavailable_here()
                .unwrap()
                .contains("NVIDIA cards use WebGPU")
        );
    }
    // Everywhere but a Linux release build, opening is refused before any
    // library is loaded, so this needs neither a pack nor a card.
    if !linux || !cfg!(feature = "dynamic-ort") {
        let err = cuda::Session::open(Path::new("/nonexistent"), Path::new("/nonexistent"))
            .err()
            .expect("no CUDA session here");
        let text = format!("{err:#}");
        assert!(text.starts_with("unavailable — "), "{text}");
        if linux {
            assert!(text.contains("Linux release build"), "{text}");
        }
    }
}

/// The smallest part, fetched for real and verified; then the same URL with a
/// wrong pin, refused, with nothing left behind.
#[test]
#[ignore = "downloads 955 KB from PyPI"]
fn the_smallest_part_downloads_and_verifies() {
    let smallest = cuda::PARTS.iter().min_by_key(|part| part.size).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut seen = 0u64;
    let file = cuda::download(smallest, dir.path(), &mut |n| seen += n).unwrap();
    assert_eq!(std::fs::metadata(&file).unwrap().len(), smallest.size);
    assert_eq!(seen, smallest.size);
    cuda::verify_file(&file, smallest.sha256).unwrap();

    let forged = cuda::Part {
        sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        ..*smallest
    };
    let other = tempfile::tempdir().unwrap();
    let err = cuda::download(&forged, other.path(), &mut |_| {}).unwrap_err();
    assert!(format!("{err:#}").contains("does not match"), "{err:#}");
    assert_eq!(
        std::fs::read_dir(other.path()).unwrap().count(),
        0,
        "a refused download leaves nothing behind"
    );
}

/// The acceptance run on a Linux runner with no NVIDIA card: the whole pack is
/// fetched and verified, loads, and the lane reports no device. Build with
/// `--no-default-features --features dynamic-ort` for the last step.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
#[ignore = "downloads the 1.9 GB CUDA pack"]
fn the_pack_fetches_loads_and_reports_no_device() {
    let cache = tempfile::tempdir().unwrap();
    let mut last = 0u8;
    let pack = cuda::fetch_pack(cache.path(), &mut |p| last = p).unwrap();
    assert_eq!(cuda::pack_installed(cache.path()), Some(pack.clone()));
    assert!(pack.join("lib").join("libonnxruntime.so").exists());
    assert!(
        pack.join("lib")
            .join("libonnxruntime_providers_cuda.so")
            .exists()
    );
    assert_eq!(last, 100);
    if cfg!(feature = "dynamic-ort") {
        let err = cuda::Session::open(&pack, Path::new("/no-model-is-needed-to-say-this"))
            .err()
            .expect("no NVIDIA device on this runner");
        assert_eq!(format!("{err:#}"), "unavailable — no NVIDIA device");
    }
}
