//! File-manager helpers: "Index with semlith" in Finder, Explorer, Nautilus,
//! Dolphin and Thunar, installed only when asked and removed cleanly.
//!
//! Every test writes into a scratch home. The Windows registry half writes
//! under a scratch `HKCU\Software\semlith-test-…` key, never the real
//! `Software\Classes`, and deletes it afterwards. The file-based helpers of
//! every OS are plain files, so they are checked here on whatever OS runs the
//! suite; only the registry needs Windows.

use semlith::helpers::{self, Env};
use std::path::{Path, PathBuf};

fn env(os: &'static str, home: &Path) -> Env {
    Env {
        os,
        home: home.to_path_buf(),
        exe: PathBuf::from("/opt/semlith dir/bin/semlith"),
        reg_base: format!(r"HKCU\Software\semlith-test-{}\Classes", std::process::id()),
    }
}

fn ids(list: &[helpers::Helper]) -> Vec<&str> {
    list.iter().map(|h| h.id).collect()
}

#[test]
fn the_finder_quick_action_is_a_service_bundle_running_index() {
    let dir = tempfile::tempdir().unwrap();
    let env = env("macos", dir.path());
    let before = helpers::status(&env);
    assert_eq!(ids(&before), ["finder-quick-action"]);
    assert!(!before[0].installed);

    let after = helpers::install(&env).unwrap();
    assert!(after[0].installed, "{after:?}");
    let bundle = dir
        .path()
        .join("Library/Services/Index with semlith.workflow");
    assert_eq!(after[0].path, bundle.display().to_string());
    let info = std::fs::read_to_string(bundle.join("Contents/Info.plist")).unwrap();
    assert!(info.contains("runWorkflowAsService") && info.contains("Index with semlith"));
    let doc = std::fs::read_to_string(bundle.join("Contents/document.wflow")).unwrap();
    // The installed binary's absolute path, quoted for sh, then for XML.
    assert!(
        doc.contains("&apos;/opt/semlith dir/bin/semlith&apos; index &quot;$@&quot;")
            || doc.contains("'/opt/semlith dir/bin/semlith' index \"$@\""),
        "{doc}"
    );

    // Idempotent, then gone.
    assert!(helpers::install(&env).unwrap()[0].installed);
    let removed = helpers::remove(&env).unwrap();
    assert!(!removed[0].installed);
    assert!(!bundle.exists());
    assert!(
        dir.path().join("Library/Services").exists(),
        "only the bundle goes"
    );
}

#[test]
fn the_linux_helpers_are_written_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let env = env("linux", home);
    assert_eq!(
        ids(&helpers::status(&env)),
        ["nautilus", "dolphin", "thunar"]
    );

    // Somebody's own Thunar action, which must survive both directions.
    let uca = home.join(".config/Thunar/uca.xml");
    std::fs::create_dir_all(uca.parent().unwrap()).unwrap();
    let theirs = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<actions>\n<action>\n\t<name>Open Terminal Here</name>\n\t<unique-id>1-1</unique-id>\n\t<command>exo-open --launch TerminalEmulator</command>\n</action>\n</actions>\n";
    std::fs::write(&uca, theirs).unwrap();

    let after = helpers::install(&env).unwrap();
    assert!(after.iter().all(|h| h.installed), "{after:?}");

    let script = home.join(".local/share/nautilus/scripts/Index with semlith");
    let text = std::fs::read_to_string(&script).unwrap();
    assert!(text.starts_with("#!/bin/sh\n"), "{text}");
    assert!(
        text.contains("exec '/opt/semlith dir/bin/semlith' index \"$@\""),
        "{text}"
    );
    let menu = home.join(".local/share/kio/servicemenus/semlith.desktop");
    let text = std::fs::read_to_string(&menu).unwrap();
    assert!(
        text.contains("Exec=\"/opt/semlith dir/bin/semlith\" index %F"),
        "{text}"
    );
    assert!(text.contains("MimeType=") && text.contains("inode/directory"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [&script, &menu] {
            let mode = std::fs::metadata(path).unwrap().permissions().mode();
            assert!(mode & 0o100 != 0, "{} is not executable", path.display());
        }
    }

    let merged = std::fs::read_to_string(&uca).unwrap();
    assert!(merged.contains("Open Terminal Here"), "{merged}");
    assert!(
        merged.contains("<unique-id>semlith-index</unique-id>"),
        "{merged}"
    );
    assert!(
        merged.contains("&apos;/opt/semlith dir/bin/semlith&apos; index %F"),
        "{merged}"
    );
    assert_eq!(merged.matches("<actions>").count(), 1);
    let backup = home.join(".config/Thunar/uca.xml.semlith-backup");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), theirs);

    // A second install updates in place rather than appending a second entry.
    helpers::install(&env).unwrap();
    let again = std::fs::read_to_string(&uca).unwrap();
    assert_eq!(again.matches("semlith-index").count(), 1, "{again}");

    let removed = helpers::remove(&env).unwrap();
    assert!(removed.iter().all(|h| !h.installed), "{removed:?}");
    assert!(!script.exists() && !menu.exists());
    assert_eq!(std::fs::read_to_string(&uca).unwrap(), theirs);
}

#[test]
fn a_thunar_file_that_does_not_parse_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let uca = dir.path().join(".config/Thunar/uca.xml");
    std::fs::create_dir_all(uca.parent().unwrap()).unwrap();
    std::fs::write(&uca, "not xml at all").unwrap();
    let err = helpers::install(&env("linux", dir.path())).unwrap_err();
    assert!(format!("{err:#}").contains("uca.xml"), "{err:#}");
    assert_eq!(std::fs::read_to_string(&uca).unwrap(), "not xml at all");
}

#[test]
fn the_send_to_launcher_is_windowless_and_passes_every_path() {
    // The file half of the Windows helpers, on any OS. The registry half runs
    // only on Windows, below.
    let dir = tempfile::tempdir().unwrap();
    let env = env("windows", dir.path());
    assert_eq!(ids(&helpers::status(&env)), ["explorer-verb", "send-to"]);
    let send_to = helpers::status(&env).pop().unwrap();
    let launcher = dir
        .path()
        .join(r"AppData/Roaming/Microsoft/Windows/SendTo/Index with semlith.vbs");
    assert_eq!(PathBuf::from(&send_to.path), launcher);

    helpers::write_launcher(&env).unwrap();
    let text = std::fs::read_to_string(&launcher).unwrap();
    assert!(
        text.contains("\"\"/opt/semlith dir/bin/semlith\"\" index"),
        "{text}"
    );
    assert!(text.contains("For Each arg In WScript.Arguments"), "{text}");
    // 0 = hidden window, False = do not wait.
    assert!(text.contains(", 0, False"), "{text}");
}

#[cfg(windows)]
#[test]
fn the_explorer_verb_is_written_under_a_scratch_key_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    let env = env("windows", dir.path());
    let root = env.reg_base.trim_end_matches(r"\Classes").to_string();
    let query = |key: &str| {
        std::process::Command::new("reg")
            .args(["query", key, "/ve"])
            .output()
            .unwrap()
    };

    let after = helpers::install(&env).unwrap();
    assert!(after.iter().all(|h| h.installed), "{after:?}");
    let verb = format!(r"{}\Directory\shell\semlith\command", env.reg_base);
    let out = query(&verb);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("wscript.exe") && text.contains("%V"),
        "{text}"
    );
    assert!(
        query(&format!(r"{}\*\shell\semlith\command", env.reg_base))
            .status
            .success()
    );

    let removed = helpers::remove(&env).unwrap();
    assert!(removed.iter().all(|h| !h.installed), "{removed:?}");
    assert!(!query(&verb).status.success());
    let _ = std::process::Command::new("reg")
        .args(["delete", &root, "/f"])
        .output();
}

/// `semlith setup` leaves them alone unless asked, installs them with
/// `--file-managers` and removes them with `--no-file-managers`. Unix only,
/// like `tests/setup.rs`: on Windows the binary would write the real HKCU.
#[cfg(unix)]
#[test]
fn setup_installs_them_only_when_asked_and_removes_them() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let cache = dir.path().join("cache");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("seeded"), b"weights would be here").unwrap();
    let setup = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
            .arg("setup")
            // A login service is not a file: launchctl would register into the
            // real session whatever HOME says.
            .arg("--no-service")
            .args(["--yes", "--airgap", "--no-hooks", "--no-agents"])
            .args(args)
            .env("HOME", &home)
            .env("SEMLITH_HOME", home.join(".semlith"))
            .env("SEMLITH_MODEL_CACHE", &cache)
            .env("PATH", "/usr/bin:/bin")
            .env_remove("SHELL")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    };
    let installed = || {
        let env = Env {
            os: std::env::consts::OS,
            home: home.clone(),
            exe: PathBuf::from("unused"),
            reg_base: String::new(),
        };
        helpers::status(&env).iter().any(|h| h.installed)
    };

    let out = setup(&[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!installed(), "setup installed a helper nobody asked for");

    let out = setup(&["--file-managers"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(installed(), "--file-managers installed nothing");

    let out = setup(&["--no-file-managers"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!installed(), "--no-file-managers left a helper behind");

    let both = setup(&["--file-managers", "--no-file-managers"]);
    assert!(
        !both.status.success(),
        "the two flags together must be refused"
    );
}
