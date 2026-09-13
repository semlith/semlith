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
/// nothing attached to stdin, and it must not reach into any agent's config on
/// the way.
#[test]
fn yes_completes_with_stdin_closed_and_registers_no_agent() {
    let machine = Machine::new();

    let run = machine.setup(&["--yes", "--airgap"]);
    assert!(
        run.status.success(),
        "`--yes` with stdin closed exited {:?}",
        run.status.code()
    );

    let said = Machine::said(&run);
    assert!(
        said.contains("registers no agent"),
        "`--yes` should say it registered no agent, it said:\n{said}"
    );
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
