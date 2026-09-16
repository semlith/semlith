//! `semlith doctor`, as a user runs it.
//!
//! The command exists so that the next person who finds semlith missing from a
//! client reads the answer instead of bisecting a configuration file, and the
//! thing that makes it usable from a script is its exit code. Both are asserted
//! here against a temporary `HOME`, so nothing in this file can see or change
//! the machine it runs on.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Machine {
    _dir: tempfile::TempDir,
    home: PathBuf,
    store_home: PathBuf,
    bin: PathBuf,
}

impl Machine {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temp directory");
        let home = dir.path().join("home");
        let store_home = home.join(".semlith");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&store_home).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        // 0700, as semlith creates it. `create_dir_all` takes the umask, which
        // on most machines leaves 0755 — and `directory modes` would then fail
        // on what this file calls a clean machine, which is the rule doing its
        // job rather than a state worth testing here.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&store_home, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            home,
            store_home,
            bin,
            _dir: dir,
        }
    }

    /// A fake client CLI, so "installed" and "not installed" can both be
    /// arranged. Nothing here runs it; `doctor` only asks whether it is on
    /// `PATH`.
    fn install(&self, program: &str) {
        let path = self.bin.join(program);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn doctor(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_semlith"))
            .arg("doctor")
            .args(args)
            .env("HOME", &self.home)
            .env("SEMLITH_HOME", &self.store_home)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env_remove("SEMLITH_ADD_ALLOW_PRIVATE")
            .output()
            .expect("doctor runs")
    }

    fn said(out: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }
}

fn mode_of(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// A machine with nothing installed is not a machine with something wrong.
///
/// Most people have two or three of the twenty-seven clients. A report that
/// called the other twenty-four faults would be a report nobody reads twice,
/// and an exit code that went non-zero for them would be useless in a script.
#[test]
fn a_machine_with_no_client_installed_reports_no_fault_and_exits_zero() {
    let machine = Machine::new();
    let run = machine.doctor(&[]);
    let said = Machine::said(&run);
    assert!(
        run.status.success(),
        "doctor exited {:?} on a clean machine:\n{said}",
        run.status.code()
    );
    assert!(said.contains("not installed"), "no client state:\n{said}");
    assert!(
        said.contains("Rules"),
        "the rules were not reported:\n{said}"
    );
}

/// The three clients semlith cannot register are named with the reason, and
/// carry no repair — there is nothing to run.
#[test]
fn the_clients_semlith_cannot_register_are_named_rather_than_left_as_a_gap() {
    let machine = Machine::new();
    let run = machine.doctor(&["--json"]);
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).expect("--json emits JSON");
    let clients = report["clients"].as_array().expect("clients");
    assert_eq!(clients.len(), 27, "every documented client is reported");

    for name in ["Crush", "Zed", "Roo Code"] {
        let client = clients
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("{name} is not in the report"));
        assert!(
            client["note"].is_string(),
            "{name} carries no reason it cannot be registered"
        );
        assert!(
            client["repair"].is_null(),
            "{name} was given a repair it cannot act on"
        );
    }
}

/// A registration that reaches one project only is the defect this release
/// exists to end, so it is reported as one and the command that fixes it is
/// printed.
#[test]
fn a_project_scope_registration_is_reported_with_the_command_that_fixes_it() {
    let machine = Machine::new();
    machine.install("claude");
    std::fs::write(
        machine.home.join(".claude.json"),
        r#"{"projects":{"/somewhere/else":{"mcpServers":{"semlith":{"command":"semlith"}}}}}"#,
    )
    .unwrap();

    let run = machine.doctor(&["--json"]);
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).expect("--json emits JSON");
    let claude = report["clients"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Claude Code")
        .expect("Claude Code is reported");

    assert_eq!(claude["registered"], false);
    assert_eq!(claude["scope"], "project");
    assert!(
        claude["repair"]
            .as_str()
            .is_some_and(|r| r.contains("semlith setup")),
        "no repair was offered for a project-scope registration: {claude}"
    );
    assert!(
        !run.status.success(),
        "doctor exited zero with a client registered for one project only"
    );
}

/// `--fix` narrows what it can and says what it changed, and a second run has
/// nothing left to do.
#[test]
fn fix_narrows_the_store_home_and_is_idempotent() {
    let machine = Machine::new();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&machine.store_home, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(mode_of(&machine.store_home), 0o755);

    // Before: the rule fails and the manual step is offered.
    let before = machine.doctor(&[]);
    let said = Machine::said(&before);
    assert!(
        said.contains("FAIL directory modes"),
        "a loose store home was not reported:\n{said}"
    );
    assert!(
        said.contains("chmod 700"),
        "no manual step was offered:\n{said}"
    );
    assert!(
        !before.status.success(),
        "doctor exited zero with a failing rule"
    );

    let fixed = machine.doctor(&["--fix"]);
    let said = Machine::said(&fixed);
    assert!(
        said.contains("700, was 755"),
        "the fix did not report both states:\n{said}"
    );
    assert_eq!(
        mode_of(&machine.store_home),
        0o700,
        "the mode on disk did not change"
    );
    assert!(
        fixed.status.success(),
        "doctor exited non-zero after repairing everything it could:\n{said}"
    );

    // And again: nothing to apply, still zero.
    let again = machine.doctor(&["--fix"]);
    assert!(again.status.success());
    assert!(
        !Machine::said(&again).contains("was 700"),
        "the second run applied a repair to an already-narrow path"
    );
}

/// `--json` is a contract a script reads, so its shape is asserted rather than
/// grepped.
#[test]
fn json_carries_the_fields_a_script_would_read() {
    let machine = Machine::new();
    let run = machine.doctor(&["--json"]);
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).expect("--json emits JSON");

    for key in ["clients", "rules", "applied"] {
        assert!(report[key].is_array(), "{key} is missing from --json");
    }
    let client = &report["clients"][0];
    for key in ["name", "present", "registered", "files"] {
        assert!(!client[key].is_null(), "a client row has no {key}");
    }
    let rule = &report["rules"][0];
    for key in ["id", "ok", "check"] {
        assert!(!rule[key].is_null(), "a rule row has no {key}");
    }
    // The four measurable rules, and only those: the six that hold by
    // construction have nothing a repair could apply to.
    assert_eq!(report["rules"].as_array().unwrap().len(), 4);
}
