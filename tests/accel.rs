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
        assert_ne!(id, "remote", "the binary registers no remote lane");
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

fn semlith(home: &std::path::Path, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_semlith"));
    command
        .args(args)
        .env("SEMLITH_HOME", home)
        .env_remove("SEMLITH_ACCEL")
        .env_remove("SEMLITH_CPU_CAP");
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().unwrap()
}

/// A lane this machine's platform cannot run at all, whatever its hardware.
fn not_for_this_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "trt"
    } else {
        "ane"
    }
}

#[test]
fn the_cpu_lane_cannot_be_turned_off_and_an_old_saved_off_reads_as_on() {
    let home = tempfile::tempdir().unwrap();
    let out = semlith(home.path(), &["accel", "off", "cpu"], &[]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("the CPU lane is always on"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // What an rc.5 or older settings file could hold.
    std::fs::write(
        home.path().join("settings.json"),
        r#"{"accelerators":{"cpu":false}}"#,
    )
    .unwrap();
    let cpu = lane(&status(home.path()), "cpu").clone();
    assert_eq!(cpu["enabled"], true);
    assert_eq!(cpu["saved"], true);
    assert_eq!(cpu["locked"], true);
    assert_ne!(
        cpu["status"]["state"], "active",
        "idle while nothing embeds"
    );
}

#[test]
fn every_lane_says_what_was_saved_and_where_it_came_from() {
    let home = tempfile::tempdir().unwrap();
    let fresh = status(home.path());
    assert_eq!(lane(&fresh, "llama")["source"], "default");
    assert_eq!(lane(&fresh, "llama")["saved"], false);

    std::fs::write(
        home.path().join("settings.json"),
        r#"{"accelerators":{"llama":false,"remote":false}}"#,
    )
    .unwrap();
    let saved = status(home.path());
    assert_eq!(lane(&saved, "llama")["source"], "saved");

    let out = semlith(
        home.path(),
        &["accel", "status", "--json"],
        &[("SEMLITH_ACCEL", "cpu")],
    );
    let env: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(lane(&env, "gpu")["source"], "environment");
    assert_eq!(lane(&env, "gpu")["enabled"], false);
}

#[test]
fn the_binary_has_no_remote_lane_to_turn_on() {
    let home = tempfile::tempdir().unwrap();
    let out = semlith(home.path(), &["accel", "on", "remote"], &[]);
    assert!(!out.status.success(), "remote turned on");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("there is no lane called remote"), "{said}");
    assert!(!said.contains("remote,"), "{said}");
}

#[test]
fn an_unavailable_lane_refuses_on_with_its_reason_and_turns_off_when_it_was_on() {
    let id = not_for_this_platform();
    let home = tempfile::tempdir().unwrap();
    let out = semlith(home.path(), &["accel", "on", id], &[]);
    assert!(!out.status.success(), "{id} turned on");
    assert!(!String::from_utf8_lossy(&out.stderr).trim().is_empty());

    // Switched on by an older release, or copied from another machine.
    std::fs::write(
        home.path().join("settings.json"),
        format!(r#"{{"accelerators":{{"{id}":true}}}}"#),
    )
    .unwrap();
    let out = semlith(home.path(), &["accel", "off", id], &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(lane(&status(home.path()), id)["saved"], false);
}

#[test]
fn one_change_of_several_lanes_lands_the_same_in_any_order() {
    let saved = |order: [&str; 2]| {
        let home = tempfile::tempdir().unwrap();
        for id in order {
            let out = semlith(home.path(), &["accel", "off", id], &[]);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let text = std::fs::read_to_string(home.path().join("settings.json")).unwrap();
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["accelerators"].clone()
    };
    assert_eq!(saved(["gpu", "llama"]), saved(["llama", "gpu"]));
}

#[test]
fn the_cpu_cap_from_the_environment_is_printed_and_reported() {
    let home = tempfile::tempdir().unwrap();
    let out = semlith(
        home.path(),
        &["accel", "status"],
        &[("SEMLITH_CPU_CAP", "50")],
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("CPU cap: 50 % of all cores (set by SEMLITH_CPU_CAP)"),
        "{text}"
    );
    let out = semlith(
        home.path(),
        &["accel", "status", "--json"],
        &[("SEMLITH_CPU_CAP", "50")],
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["cpu_cap"]["percent"], 50);
    assert_eq!(json["cpu_cap"]["source"], "environment");
}

/// A daemon on a free port with its own store home, and one request to it.
struct Daemon {
    child: std::process::Child,
    port: u16,
    token: String,
    _home: tempfile::TempDir,
}

impl Daemon {
    fn start(env: &[(&str, &str)]) -> Self {
        use std::io::BufRead;
        let home = tempfile::tempdir().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_semlith"));
        command
            .args(["start", "--port", "0"])
            .env("SEMLITH_HOME", home.path())
            .env("HOME", home.path())
            .env_remove("SEMLITH_STORE")
            .env_remove("SEMLITH_PORT")
            .env_remove("SEMLITH_CPU_CAP")
            .current_dir(home.path())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        for (name, value) in env {
            command.env(name, value);
        }
        let mut child = command.spawn().unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut line)
            .unwrap();
        let rest = line.trim().strip_prefix("http://127.0.0.1:").unwrap();
        let (port, token) = rest.split_once("/?token=").unwrap();
        Self {
            child,
            port: port.parse().unwrap(),
            token: token.to_string(),
            _home: home,
        }
    }

    fn post(&self, path: &str, body: &str) -> (u16, serde_json::Value) {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        write!(
            stream,
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            self.port,
            self.token,
            body.len()
        )
        .unwrap();
        let mut raw = String::new();
        stream.read_to_string(&mut raw).unwrap();
        let status = raw
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let json = raw
            .split_once("\r\n\r\n")
            .and_then(|(_, b)| serde_json::from_str(b).ok())
            .unwrap_or_default();
        (status, json)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn the_cap_route_refuses_out_of_range_and_an_environment_cap() {
    let daemon = Daemon::start(&[]);
    let (status, _) = daemon.post("/api/index/settings", r#"{"cpu_cap_percent":101}"#);
    assert_eq!(status, 400);
    let (status, body) = daemon.post("/api/index/settings", r#"{"cpu_cap_percent":0}"#);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["limits"]["cpu_cap_percent"]["value"], 0);
    assert_eq!(body["limits"]["cpu_cap_percent"]["source"], "saved");
    drop(daemon);

    let daemon = Daemon::start(&[("SEMLITH_CPU_CAP", "40")]);
    let (status, _) = daemon.post("/api/index/settings", r#"{"cpu_cap_percent":30}"#);
    assert_eq!(status, 409);
}
