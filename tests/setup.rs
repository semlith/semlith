//! `semlith setup`, run twice.
//!
//! The whole promise of this command is that it is safe to run again: it is the
//! install step and the repair step, and a user who reruns it after moving
//! machines should not end up with two PATH blocks in their rc file or a second
//! copy of anything. That is what this file holds to — a second run must change
//! no file at all.
//!
//! Everything runs against a temporary `HOME`, `SEMLITH_HOME` and model cache,
//! so nothing here can touch the machine it runs on, and `--airgap` keeps the
//! model step from reaching the network in the default test set.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The fences `setup` writes around the block it owns.
const BEGIN: &str = "# >>> semlith >>>";

struct Machine {
    _dir: tempfile::TempDir,
    home: PathBuf,
    store_home: PathBuf,
    cache: PathBuf,
}

impl Machine {
    /// A clean machine with a pre-seeded model cache, so the model step reports
    /// "already done" instead of asking for 52 MB over the network.
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temp directory");
        let home = dir.path().join("home");
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&cache).unwrap();
        // `is_cached` asks whether anything is there, deliberately, so one file
        // is a seeded cache.
        std::fs::write(cache.join("seeded"), b"weights would be here").unwrap();
        std::fs::write(home.join(".zshrc"), "# a shell rc somebody already owns\n").unwrap();
        Self {
            store_home: home.join(".semlith"),
            home,
            cache,
            _dir: dir,
        }
    }

    /// The same run with no `SHELL` in the environment, which is what a
    /// container, a CI runner and a cron job all look like.
    fn setup_without_shell(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_semlith"));
        command
            .arg("setup")
            .args(args)
            .env("HOME", &self.home)
            .env_remove("SHELL")
            .env("SEMLITH_HOME", &self.store_home)
            .env("SEMLITH_MODEL_CACHE", &self.cache)
            .env("PATH", "/usr/bin:/bin")
            .stdin(std::process::Stdio::null());
        command.output().expect("running semlith setup")
    }

    fn setup(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_semlith"))
            .arg("setup")
            .args(args)
            .env("HOME", &self.home)
            .env("SHELL", "/bin/zsh")
            .env("SEMLITH_HOME", &self.store_home)
            .env("SEMLITH_MODEL_CACHE", &self.cache)
            // Whatever ran the test suite is on PATH; the bin directory is not,
            // which is the state the PATH step exists for.
            .env("PATH", "/usr/bin:/bin")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("running semlith setup")
    }

    /// cliclack draws on stderr, so what a user reads is both streams together.
    fn said(out: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }

    fn rc(&self) -> String {
        std::fs::read_to_string(self.home.join(".zshrc")).expect("the rc file")
    }
}

/// Every file under a directory with its bytes, so "changed nothing" is a
/// comparison rather than a claim. Content rather than mtime: a rewrite with
/// identical bytes is still a rewrite, and this is the check that would catch
/// one appending a duplicate block.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.insert(path, bytes);
            }
        }
    }
    out
}

#[test]
fn a_second_run_changes_nothing() {
    let machine = Machine::new();

    let first = machine.setup(&["--yes", "--airgap"]);
    assert!(
        first.status.success(),
        "the first `semlith setup --yes --airgap` exited {:?}:\n{}",
        first.status.code(),
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(
        machine.rc().matches(BEGIN).count(),
        1,
        "the first run should leave exactly one semlith block in the rc file, got:\n{}",
        machine.rc()
    );

    let before = snapshot(&machine.home);

    let second = machine.setup(&["--yes", "--airgap"]);
    assert!(
        second.status.success(),
        "the second run exited {:?}:\n{}",
        second.status.code(),
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        machine.rc().matches(BEGIN).count(),
        1,
        "the second run added a second semlith block:\n{}",
        machine.rc()
    );
    assert_eq!(
        snapshot(&machine.home),
        before,
        "the second run changed a file; setup is meant to be a no-op once it has run"
    );

    let said = Machine::said(&second);
    assert!(
        said.contains("Nothing changed"),
        "the second run should say it changed nothing, it said:\n{said}"
    );
}

/// The rc file belongs to the user. `setup` may add its own block to the end of
/// it and may not touch a line of what was already there.
#[test]
fn the_users_own_rc_file_survives() {
    let machine = Machine::new();
    let original = machine.rc();

    let run = machine.setup(&["--yes", "--airgap"]);
    assert!(run.status.success());

    let after = machine.rc();
    assert!(
        after.starts_with(&original),
        "setup rewrote what was already in the rc file.\nbefore:\n{original}\nafter:\n{after}"
    );
    assert!(
        after.contains(".semlith/bin"),
        "the block should put the bin directory on PATH:\n{after}"
    );
}

/// `--yes` is what a script and an installer run, so it has to finish with
/// nothing attached to stdin.
///
/// It registers agents from 0.18.0, where it used to skip them. An install that
/// finishes with the agent unable to see the store is not an install anybody
/// wanted, and the registrations are now the stdio form — no key in any file,
/// nothing written outside the client's own registry — so there is nothing left
/// for `--yes` to be protecting the user from. What it must still not do is
/// write a file semlith does not own: that is `--register-all`, and it asks.
#[test]
fn yes_completes_with_stdin_closed_and_writes_no_client_file() {
    let machine = Machine::new();

    let run = machine.setup(&["--yes", "--airgap"]);
    assert!(
        run.status.success(),
        "`--yes` with stdin closed exited {:?}",
        run.status.code()
    );

    let said = Machine::said(&run);
    assert!(
        said.contains("agents"),
        "`--yes` should report the agents step, it said:\n{said}"
    );
    // No client configuration file may appear under a home `--yes` was pointed
    // at. `--register-all` is the only path that writes one, and it confirms
    // first.
    for name in [".cursor", ".codeium", ".continue", ".lmstudio", ".warp"] {
        assert!(
            !machine.home.join(name).exists(),
            "`--yes` wrote {name}, which it may not do without --register-all"
        );
    }
}

/// An air-gapped machine's claim is that the process never reached the network.
/// With an empty cache the model step has to say so and step aside, not fail
/// the install and not try anyway.
#[test]
fn airgap_with_an_empty_cache_skips_the_model_and_exits_zero() {
    let machine = Machine::new();
    std::fs::remove_file(machine.cache.join("seeded")).unwrap();

    let run = machine.setup(&["--yes", "--airgap"]);
    assert!(
        run.status.success(),
        "--airgap with an empty cache should still exit zero, it exited {:?}",
        run.status.code()
    );

    let said = Machine::said(&run);
    assert!(
        said.contains("--airgap") && said.contains(machine.cache.to_str().unwrap()),
        "the skip note should name --airgap and the cache to pre-seed:\n{said}"
    );
}

/// Found on a clean `ubuntu:24.04` container installing the real 0.10.0
/// archive: `SHELL` is unset there, `rc_file` gave up, and the install
/// finished with a binary nobody's shell could find. A HOME with no SHELL is
/// the normal shape of a container, a CI runner and a cron job, so it gets a
/// test rather than a comment.
#[test]
fn an_unset_shell_still_gets_a_path_block() {
    let machine = Machine::new();

    let run = machine.setup_without_shell(&["--yes", "--airgap"]);
    assert!(
        run.status.success(),
        "setup with no SHELL exited {:?}:\n{}",
        run.status.code(),
        Machine::said(&run)
    );

    let profile = machine.home.join(".profile");
    let written = std::fs::read_to_string(&profile)
        .unwrap_or_else(|e| panic!("setup wrote no {}: {e}", profile.display()));
    assert!(
        written.contains(BEGIN) && written.contains(".semlith/bin"),
        "the fallback rc file has no semlith block:\n{written}"
    );

    let said = Machine::said(&run);
    assert!(
        !said.contains("no HOME"),
        "setup blamed a missing HOME when only SHELL was unset:\n{said}"
    );
}

/// A store home is a directory name, and a directory name can hold every
/// character a shell treats as syntax. The rc file is run by the user's shell
/// at every start, so what goes into it has to be a word rather than a
/// fragment somebody's path chose.
#[test]
fn a_hostile_store_home_is_quoted_into_the_rc_file_or_refused() {
    // Every character that used to expand inside the double quotes this wrote
    // before 0.14.0, plus a space and a single quote to break the new ones.
    let awkward = r#"se"m'l $(touch pwned) `touch also-pwned` ${HOME} \ith"#;

    let dir = tempfile::tempdir().expect("a temp directory");
    let home = dir.path().join("home");
    let cache = dir.path().join("cache");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("seeded"), b"weights would be here").unwrap();
    std::fs::write(home.join(".zshrc"), "# a shell rc somebody already owns\n").unwrap();
    let store_home = dir.path().join(awkward);
    std::fs::create_dir_all(store_home.join("bin")).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("setup")
        .arg("--yes")
        .arg("--airgap")
        .env("HOME", &home)
        .env("SHELL", "/bin/zsh")
        .env("SEMLITH_HOME", &store_home)
        .env("SEMLITH_MODEL_CACHE", &cache)
        .env("PATH", "/usr/bin:/bin")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("running semlith setup");
    assert!(
        out.status.success(),
        "setup failed on an awkward home:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let rc = home.join(".zshrc");
    let text = std::fs::read_to_string(&rc).expect("the rc file");
    assert!(text.contains(BEGIN), "no block was written:\n{text}");

    // The shell's own parser is the judge. A line that is syntactically wrong,
    // or that swallows the rest of the file, fails here.
    let checked = Command::new("sh")
        .arg("-n")
        .arg(&rc)
        .output()
        .expect("running sh -n");
    assert!(
        checked.status.success(),
        "the rc file is not valid shell:\n{}\n---\n{}",
        String::from_utf8_lossy(&checked.stderr),
        text
    );

    // Nothing in the path ran while the file was being written or parsed.
    assert!(!dir.path().join("pwned").exists(), "a substitution ran");
    assert!(!dir.path().join("also-pwned").exists(), "a backtick ran");

    // Sourced, the line puts exactly that directory's bin on PATH — not the
    // expansion of ${HOME}, and not a truncation at the first space.
    let sourced = Command::new("sh")
        .arg("-c")
        .arg(format!(
            ". {} >/dev/null 2>&1; printf %s \"$PATH\"",
            shell_word(&rc)
        ))
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &home)
        .output()
        .expect("sourcing the rc file");
    let path = String::from_utf8_lossy(&sourced.stdout).into_owned();
    let want = store_home.join("bin");
    assert!(
        path.split(':').any(|entry| Path::new(entry) == want),
        "PATH does not carry {}:\n{path}",
        want.display()
    );
    assert!(
        !dir.path().join("pwned").exists(),
        "a substitution ran on source"
    );

    // A newline cannot be quoted into a shell line at all, so it is refused
    // rather than written.
    let newline_home = dir.path().join("two\nlines");
    std::fs::create_dir_all(newline_home.join("bin")).unwrap();
    std::fs::write(&rc, "# a shell rc somebody already owns\n").unwrap();
    let refused = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("setup")
        .arg("--yes")
        .arg("--airgap")
        .env("HOME", &home)
        .env("SHELL", "/bin/zsh")
        .env("SEMLITH_HOME", &newline_home)
        .env("SEMLITH_MODEL_CACHE", &cache)
        .env("PATH", "/usr/bin:/bin")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("running semlith setup");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        said.contains("newline") && said.contains("SEMLITH_HOME"),
        "a newline in the home should be refused naming the variable:\n{said}"
    );
    assert_eq!(
        std::fs::read_to_string(&rc).unwrap(),
        "# a shell rc somebody already owns\n",
        "the rc file was edited despite the refusal"
    );
}

/// A POSIX shell word for a path, for the test's own `sh -c`.
fn shell_word(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// Every argument of every process is readable by every other process this
/// user owns, so a registration that put the key on a command line put it where
/// `ps` could read it. The stanza names the variable instead.
#[test]
fn the_key_never_reaches_a_command_line_or_a_config_file() {
    let m = Machine::new();
    let out = m.setup(&["--yes", "--airgap"]);
    assert!(out.status.success(), "{}", Machine::said(&out));

    // The rc block exports the key by reading the file, rather than carrying a
    // copy of it. One credential, one file, one set of permissions.
    let rc = std::fs::read_to_string(m.home.join(".zshrc")).expect("the rc file");
    assert!(
        rc.contains("SEMLITH_AGENT_KEY") && rc.contains("agent.key"),
        "the rc block does not export the key from its file:\n{rc}"
    );
    assert!(
        !rc.contains("sml_"),
        "the rc file carries a literal key:\n{rc}"
    );

    let checked = Command::new("sh")
        .arg("-n")
        .arg(m.home.join(".zshrc"))
        .output();
    assert!(
        checked.expect("running sh -n").status.success(),
        "the rc file is not valid shell:\n{rc}"
    );
}

/// A rotation is meant to reduce what a credential is exposed to. It used to
/// widen it: the temp file the rewrite went through was created with the
/// process umask, and the rename carried those permissions onto a file somebody
/// had locked down.
#[test]
fn a_rotation_never_loosens_a_config_file() {
    use std::os::unix::fs::PermissionsExt;

    let m = Machine::new();
    let config = m
        .home
        .join(".codeium")
        .join("windsurf")
        .join("mcp_config.json");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    let old = format!("sml_{}", "a".repeat(64));
    let fresh = format!("sml_{}", "b".repeat(64));
    std::fs::write(&config, format!(r#"{{"key":"{old}"}}"#)).unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();

    let before = std::fs::metadata(&config).unwrap().permissions().mode() & 0o777;
    assert_eq!(before, 0o600);

    // `recarry_key` reads HOME, so this drives it through the library rather
    // than through a second process.
    let was = std::env::var_os("HOME");
    // SAFETY: this test is single-threaded and restores the variable below.
    unsafe { std::env::set_var("HOME", &m.home) };
    let changed = semlith::setup::recarry_key(&old, &fresh);
    match was {
        Some(v) => unsafe { std::env::set_var("HOME", v) },
        None => unsafe { std::env::remove_var("HOME") },
    }

    assert_eq!(changed, vec![config.clone()], "{changed:?}");
    let after = std::fs::metadata(&config).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        after, 0o600,
        "the rotation loosened the file from {before:o} to {after:o}"
    );
    assert!(std::fs::read_to_string(&config).unwrap().contains(&fresh));
}

// ------------------------------------------------- 0.18.0: --register-all

/// The ten clients semlith writes a file for, with the user-level path each
/// one's `config path=` fence names, relative to `HOME`.
///
/// Taken from `docs/clients.md` through the library rather than retyped, so a
/// path that moves in the documentation moves here too. The count is asserted
/// because it is the release's own number: fourteen clients register by their
/// own CLI, ten by a file, and three cannot be registered at all.
fn writable_clients() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for client in semlith::clients::clients() {
        if !client.needs_a_file_written() {
            continue;
        }
        for stanza in client.config_files() {
            let Some(path) = stanza.path.as_deref() else {
                continue;
            };
            // Only the paths that resolve on this platform. Claude Desktop's
            // Windows path is in the document and is not a file this test can
            // write on macOS or Linux.
            let applies = match stanza.os.as_deref() {
                Some("windows") => cfg!(windows),
                Some("macos") => cfg!(target_os = "macos"),
                Some("linux") => cfg!(target_os = "linux"),
                _ => true,
            };
            if applies && let Some(rest) = path.strip_prefix("~/") {
                out.push((client.name.clone(), rest.to_string()));
            }
        }
    }
    out
}

/// `--register-all` writes the file of every client that has no registration
/// command, and a second run changes none of them.
///
/// This is the one place semlith writes a file it does not own, so the things
/// asserted here are the things that make that safe: an unrelated key survives,
/// a second run is byte-identical, and a backup sits beside anything that was
/// already there.
#[test]
fn register_all_writes_every_file_only_client_and_is_idempotent() {
    let machine = Machine::new();
    let expected = writable_clients();
    assert!(
        expected.len() >= 10,
        "only {} writable client paths resolve on this platform: {expected:?}",
        expected.len()
    );

    // One of them already has a configuration with something else in it, so the
    // merge has something to preserve. Cursor's schema is the common one.
    let cursor = machine.home.join(".cursor/mcp.json");
    std::fs::create_dir_all(cursor.parent().unwrap()).unwrap();
    std::fs::write(
        &cursor,
        r#"{"mcpServers":{"other":{"command":"other","args":["serve"]}}}"#,
    )
    .unwrap();

    let run = machine.setup(&["--yes", "--airgap", "--register-all"]);
    assert!(
        run.status.success(),
        "--register-all exited {:?}:\n{}",
        run.status.code(),
        Machine::said(&run)
    );

    for (client, relative) in &expected {
        let path = machine.home.join(relative);
        assert!(
            path.exists(),
            "{client}'s file was not written at {}",
            path.display()
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("semlith"),
            "{client}'s file has no semlith entry:\n{text}"
        );
        // Nothing semlith writes carries the credential. This is the release's
        // central claim, asserted over every file it wrote.
        assert!(
            !text.contains("sml_") && !text.contains("SEMLITH_AGENT_KEY"),
            "{client}'s file carries a credential:\n{text}"
        );
    }

    let merged = std::fs::read_to_string(&cursor).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&merged).expect("the merge is valid JSON");
    assert_eq!(
        parsed["mcpServers"]["other"]["command"], "other",
        "the merge dropped a server that was already there:\n{merged}"
    );
    assert_eq!(parsed["mcpServers"]["semlith"]["command"], "semlith");
    assert!(
        machine
            .home
            .join(".cursor/mcp.json.semlith-backup")
            .exists(),
        "a file that already existed was written with no backup beside it"
    );

    // A second run changes nothing. The same property `a_second_run_changes
    // _nothing` asserts for the rc file, over the files this release added.
    let before: BTreeMap<String, String> = expected
        .iter()
        .map(|(_, relative)| {
            let path = machine.home.join(relative);
            (
                relative.clone(),
                std::fs::read_to_string(&path).unwrap_or_default(),
            )
        })
        .collect();
    let again = machine.setup(&["--yes", "--airgap", "--register-all"]);
    assert!(again.status.success());
    for (relative, text) in &before {
        assert_eq!(
            &std::fs::read_to_string(machine.home.join(relative)).unwrap_or_default(),
            text,
            "{relative} changed on a second --register-all"
        );
    }
}

/// A file that does not parse is left exactly as it was, and the rest are still
/// written.
///
/// The alternative — replacing what could not be read — is how a tool that edits
/// a file it does not own eventually corrupts one, which is the rule
/// `--register-all` is bending in the first place.
#[test]
fn register_all_refuses_a_malformed_file_and_writes_the_others() {
    let machine = Machine::new();
    let cursor = machine.home.join(".cursor/mcp.json");
    std::fs::create_dir_all(cursor.parent().unwrap()).unwrap();
    let broken = "{ this was never json";
    std::fs::write(&cursor, broken).unwrap();

    let run = machine.setup(&["--yes", "--airgap", "--register-all"]);
    assert!(
        run.status.success(),
        "one unreadable file failed the whole install"
    );
    assert_eq!(
        std::fs::read_to_string(&cursor).unwrap(),
        broken,
        "a file that could not be parsed was written anyway"
    );

    let warp = machine.home.join(".warp/.mcp.json");
    assert!(
        warp.exists(),
        "the other clients were not written after one was refused"
    );
}

/// Without `--register-all`, no file semlith does not own is created.
#[test]
fn a_default_install_writes_no_client_configuration() {
    let machine = Machine::new();
    let run = machine.setup(&["--yes", "--airgap"]);
    assert!(run.status.success());
    for (client, relative) in writable_clients() {
        assert!(
            !machine.home.join(&relative).exists(),
            "a default install wrote {client}'s {relative}"
        );
    }
}
