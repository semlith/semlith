//! The lane switches across an upgrade: a settings file that turned CUDA on
//! keeps it on, a fresh install has it off, and every experimental lane says
//! so in `accel status`.

use std::process::Command;

fn status(home: &std::path::Path) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["accel", "status", "--json"])
        .env("SEMLITH_HOME", home)
        .env_remove("SEMLITH_ACCEL")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn lane<'a>(status: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    status["lanes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["lane"] == id)
        .unwrap_or_else(|| panic!("no {id} row in {status}"))
}

#[test]
fn a_saved_cuda_switch_survives_the_upgrade_and_a_fresh_install_has_it_off() {
    // What 0.28.0 to 0.31.0 wrote when somebody turned CUDA on.
    let upgraded = tempfile::tempdir().unwrap();
    std::fs::write(
        upgraded.path().join("settings.json"),
        r#"{"accelerators":{"cpu":true,"gpu":true,"cuda":true}}"#,
    )
    .unwrap();
    let before = status(upgraded.path());
    assert_eq!(lane(&before, "cuda")["enabled"], true);

    let fresh = tempfile::tempdir().unwrap();
    let after = status(fresh.path());
    assert_eq!(lane(&after, "cuda")["enabled"], false);
    for id in ["trt", "openvino", "llama"] {
        assert_eq!(lane(&after, id)["enabled"], false, "{id} is off by default");
    }
    assert_eq!(lane(&after, "gpu")["enabled"], true);
    assert_eq!(lane(&after, "ane")["enabled"], true);
    assert_eq!(after["gpu_beside_ane"], false);
}

#[test]
fn exactly_the_experimental_lanes_say_so() {
    let home = tempfile::tempdir().unwrap();
    let rows = status(home.path());
    for row in rows["lanes"].as_array().unwrap() {
        let id = row["lane"].as_str().unwrap();
        let want = matches!(id, "cuda" | "trt" | "openvino" | "llama");
        assert_eq!(row["experimental"], want, "{id}");
    }
    let text = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["accel", "status"])
        .env("SEMLITH_HOME", home.path())
        .env_remove("SEMLITH_ACCEL")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&text.stdout);
    for line in text.lines() {
        let lane = line.split_whitespace().next().unwrap_or("");
        let marked = line.contains("(experimental)");
        let want = matches!(lane, "cuda" | "trt" | "openvino" | "llama");
        if ["cpu", "ane", "gpu", "cuda", "trt", "openvino", "llama"].contains(&lane) {
            assert_eq!(marked, want, "{line}");
        }
    }
}
