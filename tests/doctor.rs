//! `semlith doctor`, as a user runs it.
//!
//! The command exists so that the next person who finds semlith missing from a
//! client reads the answer instead of bisecting a configuration file, and the
//! thing that makes it usable from a script is its exit code. Both are asserted
//! here against a temporary `HOME`, so nothing in this file can see or change
//! the machine it runs on.

#![cfg(unix)]

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Machine {
    _dir: tempfile::TempDir,
    home: PathBuf,
    store_home: PathBuf,
    bin: PathBuf,
    /// The binary these tests run: a copy outside any build directory.
    ///
    /// `doctor` refuses to rewrite a client's configuration when the binary
    /// running it is a build artifact — a debug binary repointing a
    /// developer's real registrations at a path `cargo clean` deletes is the
    /// defect this release exists to remove, manufactured by the command meant
    /// to detect it. Running a copy is also what a user does.
    installed: PathBuf,
    /// A second `PATH` directory, searched after `bin`.
    ///
    /// Only one thing needs it, and that thing cannot be arranged without it: a
    /// machine carrying two `semlith` binaries has them in two directories, and
    /// which of them `PATH` reaches first is the whole question. It is empty on
    /// every other machine this file builds, so it costs those nothing.
    bin2: PathBuf,
}

impl Machine {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temp directory");
        let home = dir.path().join("home");
        let store_home = home.join(".semlith");
        let bin = dir.path().join("bin");
        let bin2 = dir.path().join("bin2");
        // A copy of the binary, outside any `target/` directory, because
        // `doctor` refuses to rewrite a client's configuration when the binary
        // running it is a build artifact. Running a copy is also what a user
        // does.
        let installed = dir.path().join("opt").join(if cfg!(windows) {
            "semlith.exe"
        } else {
            "semlith"
        });
        std::fs::create_dir_all(installed.parent().unwrap()).unwrap();
        common::copy_executable(Path::new(env!("CARGO_BIN_EXE_semlith")), &installed);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&store_home).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&bin2).unwrap();
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
            bin2,
            installed,
            _dir: dir,
        }
    }

    /// A fake client CLI, so "installed" and "not installed" can both be
    /// arranged. Nothing here runs it; `doctor` only asks whether it is on
    /// `PATH`.
    fn install(&self, program: &str) {
        self.install_in(&self.bin, program, "#!/bin/sh\nexit 0\n");
    }

    /// A program in a chosen `PATH` directory, with a chosen body.
    ///
    /// The body matters for exactly one program: `doctor` runs every `semlith`
    /// it finds on `PATH` with `--version`, so a shim standing in for an older
    /// install has to answer like one.
    fn install_in(&self, dir: &Path, program: &str, body: &str) {
        common::write_executable(&dir.join(program), body);
    }

    fn path(&self) -> String {
        format!(
            "{}:{}:/usr/bin:/bin",
            self.bin.display(),
            self.bin2.display()
        )
    }

    fn doctor(&self, args: &[&str]) -> Output {
        Command::new(&self.installed)
            .arg("doctor")
            .args(args)
            .env("HOME", &self.home)
            .env("SEMLITH_HOME", &self.store_home)
            .env("PATH", self.path())
            .env_remove("SEMLITH_ADD_ALLOW_PRIVATE")
            .output()
            .expect("doctor runs")
    }

    /// `doctor` run from a chosen directory.
    ///
    /// The directory is the question for a per-project override: the same
    /// machine, the same configuration file and the same binary answer
    /// differently depending on where the command was run, which is exactly
    /// what made that defect invisible.
    fn doctor_in(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(&self.installed)
            .arg("doctor")
            .args(args)
            .current_dir(cwd)
            .env("HOME", &self.home)
            .env("SEMLITH_HOME", &self.store_home)
            .env("PATH", self.path())
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
    // The four measurable Privacy rules, and only those: the six that hold by
    // construction have nothing a repair could apply to. Plus `semlith on
    // PATH`, which is a reading of the machine with the same shape and no home
    // on the Privacy page — that page renders the rules it names by id, so it
    // does not show this one. And from 0.25.0 the rescoring model, which is
    // never a fault — search answers without it — but changes the order the
    // answers come back in, so a machine that does not have it is owed the
    // sentence rather than left to wonder why its ranking differs from the
    // one the release measured.
    assert_eq!(report["rules"].as_array().unwrap().len(), 6);
    assert!(
        report["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["id"] == "rescoring model"),
        "doctor does not report the rescoring model"
    );
}

/// Registered at user scope, switched off for one directory, and silent about
/// it.
///
/// The defect of 2026-09-17 and 2026-09-18 on the owner's machine: the entry
/// was in `mcpServers` at user scope, the binary answered `initialize` in under
/// a second, `claude mcp list` from any other directory reported the server
/// connected — and a session opened in that one directory had no semlith and no
/// message naming why. Every check short of asking from the affected directory
/// passed, which is why this test asks from both.
#[test]
fn a_server_switched_off_for_one_directory_is_reported_in_that_directory() {
    let machine = Machine::new();
    machine.install("claude");
    let project = machine.home.join("work").join("repo");
    let elsewhere = machine.home.join("work").join("other");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(
        machine.home.join(".claude.json"),
        serde_json::json!({
            "mcpServers": { "semlith": { "command": "semlith", "args": ["mcp"] } },
            "projects": {
                project.to_str().unwrap(): {
                    "disabledMcpjsonServers": ["plugin:figma:figma", "semlith"],
                },
            },
        })
        .to_string(),
    )
    .unwrap();

    // From the directory it is switched off for: a fault, with the file, the
    // key and the directory named, and a non-zero exit a hook can gate on.
    let inside = machine.doctor_in(&project, &["--json"]);
    let report: serde_json::Value =
        serde_json::from_slice(&inside.stdout).expect("--json emits JSON");
    let claude = report["clients"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Claude Code")
        .expect("Claude Code is reported");
    assert_eq!(claude["registered"], true, "the entry is at user scope");
    assert_eq!(claude["disabled_here"], true);
    assert_eq!(
        claude["fault"], true,
        "a server that cannot be reached here"
    );
    let explain = claude["explain"].as_str().unwrap_or_default();
    assert!(
        explain.contains(".claude.json")
            && explain.contains("disabledMcpjsonServers")
            && explain.contains(project.to_str().unwrap()),
        "the reason did not name the file, the key and the directory: {explain}"
    );
    assert!(
        !inside.status.success(),
        "doctor exited zero from a directory with no reachable server"
    );

    // From any other directory: the server is fine, and the switched-off
    // directory is still named — the next report is otherwise "it works
    // everywhere except one repository".
    let outside = machine.doctor_in(&elsewhere, &["--json"]);
    let report: serde_json::Value =
        serde_json::from_slice(&outside.stdout).expect("--json emits JSON");
    let claude = report["clients"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Claude Code")
        .expect("Claude Code is reported");
    assert_eq!(claude["disabled_here"], false);
    assert_eq!(
        claude["disabled_in"],
        serde_json::json!([project.to_str().unwrap()]),
    );
}

/// The same override under the other spelling of the key.
///
/// `disabledMcpjsonServers` is what the client writes today. A rename would
/// otherwise turn this back into the silent absence it was.
#[test]
fn the_other_spelling_of_the_disabled_key_is_read_too() {
    let machine = Machine::new();
    machine.install("claude");
    let project = machine.home.join("work");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        machine.home.join(".claude.json"),
        serde_json::json!({
            "mcpServers": { "semlith": { "command": "semlith", "args": ["mcp"] } },
            "projects": { project.to_str().unwrap(): { "disabledMcpServers": ["semlith"] } },
        })
        .to_string(),
    )
    .unwrap();

    let run = machine.doctor_in(&project, &["--json"]);
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).expect("--json emits JSON");
    let claude = report["clients"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Claude Code")
        .expect("Claude Code is reported");
    assert_eq!(claude["disabled_here"], true);
    // And it names the spelling that actually holds it. The first thing anyone
    // does with this message is search their configuration for the key it
    // names, and until 0.25.0 it named the documented spelling whichever one
    // was really there — which sent the reader looking for a key that is not
    // in the file.
    let explain = claude["explain"].as_str().unwrap_or_default();
    assert!(
        explain.contains("disabledMcpServers") && !explain.contains("disabledMcpjsonServers"),
        "the message names the wrong key: {explain}"
    );
}

/// A registration written before 0.21.0 names the bare word `semlith`, and
/// `doctor` rewrites it where it stands.
///
/// The entry launches only from a `PATH` that happens to carry the binary, and
/// the process reading it is very often not a login shell — an editor started
/// from a desktop icon, a launchd agent, a desktop app. The client then reports
/// that the server exited and says nothing about why. Every machine registered
/// before 0.21.0 is in that state, so the repair cannot wait behind a flag
/// somebody has to know to type: it is semlith's own entry, written by semlith,
/// that cannot launch.
///
/// The second run is half the test. A repair that reports itself every time is
/// a repair nobody can tell from a fault.
#[test]
fn a_bare_command_is_rewritten_in_place_and_the_second_run_has_nothing_to_do() {
    let machine = Machine::new();
    let cursor = machine.home.join(".cursor").join("mcp.json");
    std::fs::create_dir_all(cursor.parent().unwrap()).unwrap();
    std::fs::write(
        &cursor,
        r#"{"theme":"dark","mcpServers":{"other":{"command":"other"},"semlith":{"command":"semlith","args":["mcp"]}}}"#,
    )
    .unwrap();

    let first = machine.doctor(&[]);
    let said = Machine::said(&first);
    assert!(
        said.contains(&cursor.display().to_string()),
        "the repair did not name the file it rewrote:\n{said}"
    );
    // Without the backticks since 0.26.0: the note is rendered verbatim by
    // the portal as well as by the terminal, and neither renders markdown
    // (#124).
    assert!(
        said.contains("the semlith entry named semlith,"),
        "the repair did not print the command it replaced:\n{said}"
    );

    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cursor).unwrap()).unwrap();
    let now = after["mcpServers"]["semlith"]["command"]
        .as_str()
        .expect("the entry still has a command");
    assert!(
        now.contains('/') && now.ends_with("semlith"),
        "the command was not made absolute: {now}"
    );
    assert!(
        said.contains(now),
        "the repair did not print the command it wrote:\n{said}"
    );
    // The deep merge, not a rewrite of the file: everything semlith did not
    // come to change is still there, including somebody else's server.
    assert_eq!(after["theme"], "dark", "an unrelated key was lost");
    assert_eq!(after["mcpServers"]["other"]["command"], "other");
    assert_eq!(after["mcpServers"]["semlith"]["args"][0], "mcp");

    let again = machine.doctor(&[]);
    let said = Machine::said(&again);
    assert!(
        !said.contains("`semlith`"),
        "the second run reported a repair with nothing left to repair:\n{said}"
    );
    assert_eq!(
        std::fs::read_to_string(&cursor).unwrap(),
        serde_json::to_string_pretty(&after).unwrap() + "\n",
        "the second run rewrote a file that was already right"
    );
}

/// Two semlith binaries on one `PATH`, and only one of them is what a bare
/// command reaches.
///
/// The machine this was found on had `~/.semlith/bin/semlith` at 0.20.1 first
/// on `PATH` and `~/.cargo/bin/semlith` at 0.14.0 behind it, left over from a
/// `cargo install`. Nothing said so. Two installs is not automatically broken —
/// which is why this is reported rather than failed — but it is automatically
/// worth knowing, because every spawn that resolves the bare name is a coin
/// toss decided by whichever `PATH` the spawning process inherited.
#[test]
fn every_semlith_on_path_is_listed_with_its_version_and_which_one_wins() {
    let machine = Machine::new();
    machine.install_in(
        &machine.bin,
        "semlith",
        "#!/bin/sh\necho 'semlith 0.20.1'\n",
    );
    machine.install_in(
        &machine.bin2,
        "semlith",
        "#!/bin/sh\necho 'semlith 0.14.0'\n",
    );
    let winner = machine.bin.join("semlith");
    let shadowed = machine.bin2.join("semlith");

    let run = machine.doctor(&["--json"]);
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).expect("--json emits JSON");
    let rule = report["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "semlith on PATH")
        .expect("the rule is reported");

    let check = rule["check"].as_str().unwrap_or_default();
    for expected in [
        winner.display().to_string(),
        shadowed.display().to_string(),
        "0.20.1".to_string(),
        "0.14.0".to_string(),
    ] {
        assert!(
            check.contains(&expected),
            "the report does not name {expected}: {check}"
        );
    }
    assert!(
        check.find(&winner.display().to_string()) < check.find(&shadowed.display().to_string()),
        "the one PATH reaches first is not named first: {check}"
    );

    // An offer, not a removal, and not a prompt either: `doctor` is read from
    // scripts, so the only thing it may do here is print the command.
    let manual = rule["manual"].as_str().unwrap_or_default();
    assert!(
        manual.contains(&shadowed.display().to_string())
            && !manual.contains(&winner.display().to_string()),
        "the offer does not remove the shadowed install and only that: {manual}"
    );
    assert!(shadowed.exists(), "doctor removed a binary it only offered");
    assert!(
        run.status.success(),
        "doctor exited non-zero over a second install, which is not a fault"
    );
    let said = Machine::said(&machine.doctor(&[]));
    assert!(
        said.contains(&shadowed.display().to_string()) && said.contains("0.14.0"),
        "the second install is in --json and not in the report a person reads:\n{said}"
    );
}

/// `--brief` is one line, an exit code, and nothing written.
///
/// It is the one form of `doctor` that may run on every session a person
/// opens — a shell prompt, an agent's session-start hook — and a check that
/// rewrites a configuration file as a side effect of being asked "is this
/// working?" is not a check.
#[test]
fn brief_is_one_line_and_writes_nothing() {
    let machine = Machine::new();
    let cursor = machine.home.join(".cursor").join("mcp.json");
    std::fs::create_dir_all(cursor.parent().unwrap()).unwrap();
    let before =
        r#"{"theme":"dark","mcpServers":{"semlith":{"command":"semlith","args":["mcp"]}}}"#;
    std::fs::write(&cursor, before).unwrap();
    let stamp = std::fs::metadata(&cursor).unwrap().modified().unwrap();

    let run = machine.doctor(&["--brief"]);
    let said = String::from_utf8_lossy(&run.stdout);
    assert_eq!(
        said.trim().lines().count(),
        1,
        "--brief printed more than one line:\n{said}"
    );

    assert_eq!(
        std::fs::read_to_string(&cursor).unwrap(),
        before,
        "--brief rewrote a client configuration file"
    );
    assert_eq!(
        std::fs::metadata(&cursor).unwrap().modified().unwrap(),
        stamp,
        "--brief touched a client configuration file"
    );
}

/// The switched-off directory is what `--brief` names, and it exits non-zero
/// so a prompt or a hook can act on it.
#[test]
fn brief_names_the_directory_a_server_is_switched_off_for() {
    let machine = Machine::new();
    machine.install("claude");
    let project = machine.home.join("work");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        machine.home.join(".claude.json"),
        serde_json::json!({
            "mcpServers": { "semlith": { "command": "semlith", "args": ["mcp"] } },
            "projects": { project.to_str().unwrap(): { "disabledMcpjsonServers": ["semlith"] } },
        })
        .to_string(),
    )
    .unwrap();

    let run = machine.doctor_in(&project, &["--brief"]);
    let said = String::from_utf8_lossy(&run.stdout);
    assert!(
        said.contains("Claude Code") && said.contains("switched off"),
        "--brief did not name the override:\n{said}"
    );
    assert!(
        !run.status.success(),
        "--brief exited zero from a directory with no reachable server"
    );
}

/// A build artifact does not rewrite anyone's registrations.
///
/// The repair points every entry at `current_exe()`. Run from `target/debug`,
/// that would silently repoint a developer's real Cursor, Codex and Copilot
/// registrations at a binary `cargo clean` deletes — leaving them with no
/// semlith and a configuration file that looks correct, which is the exact
/// failure this release exists to remove, manufactured by the command meant to
/// detect it. This test runs the build artifact on purpose; every other test
/// in this file runs a copy outside the build directory.
#[test]
fn a_binary_in_a_build_directory_does_not_rewrite_a_configuration_file() {
    let machine = Machine::new();
    let cursor = machine.home.join(".cursor").join("mcp.json");
    std::fs::create_dir_all(cursor.parent().unwrap()).unwrap();
    let before =
        r#"{"theme":"dark","mcpServers":{"semlith":{"command":"semlith","args":["mcp"]}}}"#;
    std::fs::write(&cursor, before).unwrap();

    let run = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("doctor")
        .env("HOME", &machine.home)
        .env("SEMLITH_HOME", &machine.store_home)
        .env("PATH", format!("{}:/usr/bin:/bin", machine.bin.display()))
        .output()
        .expect("doctor runs");

    assert_eq!(
        std::fs::read_to_string(&cursor).unwrap(),
        before,
        "a binary under target/ rewrote a client configuration:\n{}",
        Machine::said(&run),
    );
}

/// The proof tries the command a client would actually run.
///
/// Not `current_exe()`, which always launches because it is the process doing
/// the asking. A machine registered before 0.21.0 has a bare `semlith` in its
/// configuration, and the whole question is whether *that* starts from an
/// environment nobody arranged.
#[test]
fn the_proof_launches_the_command_a_client_would_run() {
    let machine = Machine::new();
    std::fs::write(
        machine.home.join(".claude.json"),
        serde_json::json!({
            "mcpServers": {
                "semlith": { "command": "semlith-that-is-not-here", "args": ["mcp"] },
            },
        })
        .to_string(),
    )
    .unwrap();

    let run = machine.doctor(&["--json"]);
    let report: serde_json::Value = serde_json::from_slice(&run.stdout).expect("--json emits JSON");
    let proof = &report["proof"];
    assert_eq!(proof["command"], "semlith-that-is-not-here");
    assert_eq!(
        proof["launched"], false,
        "a command that is not there launched"
    );
    let failed = proof["failed"].as_str().unwrap_or_default();
    assert!(
        failed.contains("semlith-that-is-not-here")
            && failed.contains("env -i")
            && failed.contains("semlith setup"),
        "the first failure did not name the command, the proof and the repair: {failed}"
    );
    assert!(
        !run.status.success(),
        "doctor exited zero with a registration that cannot launch"
    );
}

/// No string a surface renders carries a markdown backtick.
///
/// Issue #124: the Agents page printed the backticks in a repair note as
/// characters, because the note is shown verbatim by both the terminal and
/// the portal and neither renders markdown. Fixed where the string is
/// written rather than by stripping them on the way out, so the terminal
/// stops printing them too.
#[test]
fn no_repair_note_carries_a_markdown_backtick() {
    const DOCTOR: &str = include_str!("../src/doctor.rs");
    let mut offenders = Vec::new();
    for (i, line) in DOCTOR.lines().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") || code.starts_with("///") || code.starts_with('*') {
            continue;
        }
        // A backtick inside a string literal on a line of code is a markdown
        // reference in something a person will read as text.
        if line.contains('`') && line.contains('"') {
            offenders.push(format!("{}: {}", i + 1, line.trim()));
        }
    }
    assert!(
        offenders.is_empty(),
        "these strings would print their backticks:\n  {}",
        offenders.join("\n  ")
    );
}

/// 4.1: `doctor --fix`, run in a directory whose project entry disables
/// semlith, removes only that entry and leaves the rest of `~/.claude.json`
/// byte-identical; `doctor` reports a missing `alwaysLoad`.
#[test]
fn doctor_fix_clears_only_the_disable_for_this_directory() {
    let machine = Machine::new();
    machine.install("claude");
    let project = machine.home.join("work").join("repo");
    let other = machine.home.join("work").join("other");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let config = machine.home.join(".claude.json");
    let before = serde_json::json!({
        "mcpServers": { "semlith": { "command": "semlith", "args": ["mcp"] } },
        "projects": {
            project.to_str().unwrap(): { "disabledMcpjsonServers": ["plugin:figma:figma", "semlith"], "allowedTools": [] },
            other.to_str().unwrap(): { "disabledMcpServers": ["semlith"] },
        },
        "tipsHistory": { "a": 1 }
    });
    std::fs::write(&config, serde_json::to_string_pretty(&before).unwrap() + "\n").unwrap();

    let report = machine.doctor_in(&project, &["--json"]);
    let json: serde_json::Value = serde_json::from_slice(&report.stdout).unwrap();
    let claude = json["clients"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Claude Code")
        .unwrap()
        .clone();
    assert_eq!(claude["always_load"], false, "{claude}");

    let fixed = machine.doctor_in(&project, &["--fix"]);
    let _ = fixed;
    let mut expected = before.clone();
    expected["projects"][project.to_str().unwrap()]["disabledMcpjsonServers"] =
        serde_json::json!(["plugin:figma:figma"]);
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        serde_json::to_string_pretty(&expected).unwrap() + "\n",
        "doctor --fix changed more than the one entry"
    );
}
