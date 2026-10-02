//! "Index with semlith" in the file manager: opt-in, and removable.
//!
//! A browser never gives the portal a dropped item's path; a file manager
//! always knows it. So `semlith setup --file-managers` (or the portal's
//! switch) puts one entry in each file manager this OS has, each running
//! `<absolute path to semlith> index <the selected paths>`:
//!
//! - macOS: a Finder Quick Action, `~/Library/Services/Index with semlith.workflow`.
//! - Windows: an Explorer verb under `HKCU\Software\Classes\{Directory,*}\shell\semlith`
//!   and a Send to entry, both through a windowless `.vbs` launcher. No admin.
//!   Windows 11 shows the verb under "Show more options" unless the binary is
//!   signed, and it is not.
//! - Linux: a Nautilus script, a Dolphin service menu, and a Thunar custom
//!   action merged into `uca.xml` beside the user's own.
//!
//! Nothing is installed unless asked. Every path is under the home from
//! `home::user_home`, and the registry root is a field so a test writes under
//! a scratch key rather than the real `Software\Classes`.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

const LABEL: &str = "Index with semlith";

/// Thunar's id for our action, which is how it is found again.
const UCA_ID: &str = "semlith-index";

/// Where the helpers go and what they run.
#[derive(Debug, Clone)]
pub struct Env {
    /// `std::env::consts::OS`, or another OS's for a test of its files.
    pub os: &'static str,
    pub home: PathBuf,
    /// The binary every helper runs, by absolute path.
    pub exe: PathBuf,
    /// The registry key standing for `HKCU\Software\Classes`.
    pub reg_base: String,
}

impl Env {
    /// This machine: the installed binary if setup put one in the bin
    /// directory, otherwise the one running.
    pub fn current() -> Result<Env> {
        let installed = crate::home::bin_dir()
            .ok()
            .map(|d| d.join(crate::setup::exe_name()))
            .filter(|p| p.exists());
        let exe = match installed {
            Some(p) => p,
            None => std::env::current_exe().context("finding the semlith binary")?,
        };
        Ok(Env {
            os: std::env::consts::OS,
            home: crate::home::user_home()?,
            exe,
            reg_base: r"HKCU\Software\Classes".into(),
        })
    }
}

/// One helper, as `semlith setup --file-managers` reports it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Helper {
    pub id: &'static str,
    pub label: &'static str,
    pub installed: bool,
    pub path: String,
}

enum Kind {
    /// Files semlith owns outright; `root` is removed whole.
    Files {
        root: PathBuf,
        files: Vec<(PathBuf, String, bool)>,
    },
    /// One `<action>` inside a file the user owns.
    Thunar(PathBuf),
    /// `(key, value of the command subkey)` pairs.
    Registry(Vec<(String, String)>),
}

struct Spec {
    id: &'static str,
    label: &'static str,
    path: String,
    kind: Kind,
}

fn specs(env: &Env) -> Vec<Spec> {
    let exe = env.exe.display().to_string();
    match env.os {
        "macos" => {
            let root = env
                .home
                .join("Library/Services/Index with semlith.workflow");
            let command = format!("{} index \"$@\"", crate::setup::posix_quote(&exe));
            vec![Spec {
                id: "finder-quick-action",
                label: "Finder Quick Action",
                path: root.display().to_string(),
                kind: Kind::Files {
                    files: vec![
                        (root.join("Contents/Info.plist"), INFO_PLIST.into(), false),
                        (
                            root.join("Contents/document.wflow"),
                            DOCUMENT_WFLOW.replace("{COMMAND}", &xml_escape(&command)),
                            false,
                        ),
                    ],
                    root,
                },
            }]
        }
        "windows" => {
            let launcher = launcher_path(env);
            let wscript = format!("wscript.exe \"{}\"", launcher.display());
            let verb = format!(r"{}\Directory\shell\semlith", env.reg_base);
            vec![
                Spec {
                    id: "explorer-verb",
                    label: "Explorer right-click menu",
                    path: verb.clone(),
                    kind: Kind::Registry(vec![
                        (verb, format!("{wscript} \"%V\"")),
                        (
                            format!(r"{}\*\shell\semlith", env.reg_base),
                            format!("{wscript} \"%1\""),
                        ),
                    ]),
                },
                Spec {
                    id: "send-to",
                    label: "Send to menu",
                    path: launcher.display().to_string(),
                    kind: Kind::Files {
                        files: vec![(launcher.clone(), launcher_text(env), false)],
                        root: launcher,
                    },
                },
            ]
        }
        _ => {
            let script = env
                .home
                .join(".local/share/nautilus/scripts/Index with semlith");
            let menu = env
                .home
                .join(".local/share/kio/servicemenus/semlith.desktop");
            vec![
                Spec {
                    id: "nautilus",
                    label: "Files (Nautilus) script",
                    path: script.display().to_string(),
                    kind: Kind::Files {
                        files: vec![(
                            script.clone(),
                            format!(
                                "#!/bin/sh\n# {LABEL}: written by `semlith setup --file-managers`, \
                                 removed by --no-file-managers.\nexec {} index \"$@\"\n",
                                crate::setup::posix_quote(&exe)
                            ),
                            true,
                        )],
                        root: script,
                    },
                },
                Spec {
                    id: "dolphin",
                    label: "Dolphin service menu",
                    path: menu.display().to_string(),
                    kind: Kind::Files {
                        files: vec![(
                            menu.clone(),
                            format!(
                                "[Desktop Entry]\nType=Service\nMimeType=inode/directory;all/allfiles;\n\
                                 X-KDE-ServiceTypes=KonqPopupMenu/Plugin\nActions=semlithIndex;\n\n\
                                 [Desktop Action semlithIndex]\nName={LABEL}\nIcon=system-search\n\
                                 Exec={} index %F\n",
                                desktop_quote(&exe)
                            ),
                            true,
                        )],
                        root: menu,
                    },
                },
                Spec {
                    id: "thunar",
                    label: "Thunar custom action",
                    path: env
                        .home
                        .join(".config/Thunar/uca.xml")
                        .display()
                        .to_string(),
                    kind: Kind::Thunar(env.home.join(".config/Thunar/uca.xml")),
                },
            ]
        }
    }
}

/// What each helper for this OS is and whether it is there. Reads only.
pub fn status(env: &Env) -> Vec<Helper> {
    specs(env)
        .into_iter()
        .map(|spec| Helper {
            id: spec.id,
            label: spec.label,
            installed: installed(&spec.kind),
            path: spec.path,
        })
        .collect()
}

/// Install every helper for this OS, or bring one up to date. Idempotent.
pub fn install(env: &Env) -> Result<Vec<Helper>> {
    for spec in specs(env) {
        match &spec.kind {
            Kind::Files { files, .. } => {
                for (path, body, executable) in files {
                    write_file(path, body, *executable)?;
                }
            }
            Kind::Thunar(path) => thunar_install(path, &env.exe.display().to_string())?,
            Kind::Registry(keys) => {
                for (key, command) in keys {
                    reg(&["add", key, "/ve", "/d", LABEL, "/f"])?;
                    reg(&[
                        "add",
                        &format!(r"{key}\command"),
                        "/ve",
                        "/d",
                        command,
                        "/f",
                    ])?;
                }
            }
        }
    }
    Ok(status(env))
}

/// Remove every helper for this OS that is there.
pub fn remove(env: &Env) -> Result<Vec<Helper>> {
    for spec in specs(env) {
        if !installed(&spec.kind) {
            continue;
        }
        match &spec.kind {
            Kind::Files { root, .. } if root.is_dir() => std::fs::remove_dir_all(root)
                .with_context(|| format!("removing {}", root.display()))?,
            Kind::Files { root, .. } => std::fs::remove_file(root)
                .with_context(|| format!("removing {}", root.display()))?,
            Kind::Thunar(path) => thunar_remove(path)?,
            Kind::Registry(keys) => {
                for (key, _) in keys {
                    reg(&["delete", key, "/f"])?;
                }
            }
        }
    }
    Ok(status(env))
}

/// The Send to launcher alone. `install` writes it with the verb; this is the
/// half a test can check on an OS with no registry.
pub fn write_launcher(env: &Env) -> Result<()> {
    write_file(&launcher_path(env), &launcher_text(env), false)
}

fn installed(kind: &Kind) -> bool {
    match kind {
        Kind::Files { root, .. } => root.exists(),
        Kind::Thunar(path) => std::fs::read_to_string(path)
            .is_ok_and(|t| t.contains(&format!("<unique-id>{UCA_ID}</unique-id>"))),
        Kind::Registry(keys) => keys.iter().all(|(key, _)| reg(&["query", key]).is_ok()),
    }
}

/// `%APPDATA%\Microsoft\Windows\SendTo`, spelled from the home rather than read
/// from the environment, so every write is under `home::user_home`.
fn launcher_path(env: &Env) -> PathBuf {
    env.home
        .join(r"AppData/Roaming/Microsoft/Windows/SendTo")
        .join(format!("{LABEL}.vbs"))
}

/// `wscript` runs with no console, which a verb pointing at `semlith.exe`
/// itself would flash up. Window style 0 is hidden; `False` does not wait.
fn launcher_text(env: &Env) -> String {
    format!(
        "' {LABEL}: written by `semlith setup --file-managers`, removed by --no-file-managers.\r\n\
         Set shell = CreateObject(\"WScript.Shell\")\r\n\
         cmd = \"\"\"{}\"\" index\"\r\n\
         For Each arg In WScript.Arguments\r\n\
         \x20 cmd = cmd & \" \"\"\" & arg & \"\"\"\"\r\n\
         Next\r\n\
         shell.Run cmd, 0, False\r\n",
        env.exe.display()
    )
}

fn write_file(path: &Path, body: &str, executable: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("making {} executable", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    Ok(())
}

fn reg(args: &[&str]) -> Result<()> {
    let out = std::process::Command::new("reg")
        .args(args)
        .output()
        .context("running reg.exe")?;
    if !out.status.success() {
        bail!(
            "reg {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

fn thunar_block(exe: &str) -> String {
    format!(
        "<action>\n\t<icon>system-search</icon>\n\t<name>{LABEL}</name>\n\t<submenu></submenu>\n\
         \t<unique-id>{UCA_ID}</unique-id>\n\t<command>{}</command>\n\
         \t<description>{LABEL}</description>\n\t<range></range>\n\t<patterns>*</patterns>\n\
         \t<directories/>\n\t<audio-files/>\n\t<image-files/>\n\t<other-files/>\n\
         \t<text-files/>\n\t<video-files/>\n</action>\n",
        xml_escape(&format!("{} index %F", crate::setup::posix_quote(exe)))
    )
}

/// Where our `<action>` sits in `uca.xml`, newline included.
fn thunar_span(text: &str) -> Option<std::ops::Range<usize>> {
    let id = text.find(&format!("<unique-id>{UCA_ID}</unique-id>"))?;
    let start = text[..id].rfind("<action>")?;
    let close = id + text[id..].find("</action>")? + "</action>".len();
    let end = if text[close..].starts_with('\n') {
        close + 1
    } else {
        close
    };
    Some(start..end)
}

/// Merged into the user's own file: their actions stay, ours is replaced in
/// place on a second run, and a file that is not an `<actions>` list is left
/// alone with an error rather than overwritten.
fn thunar_install(path: &Path, exe: &str) -> Result<()> {
    let block = thunar_block(exe);
    let next = match std::fs::read_to_string(path) {
        Err(_) => {
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<actions>\n{block}</actions>\n")
        }
        Ok(text) => {
            let mut text = text;
            if let Some(span) = thunar_span(&text) {
                text.replace_range(span, &block);
            } else if let Some(at) = text.rfind("</actions>") {
                text.insert_str(at, &block);
            } else {
                bail!("{} has no <actions> list; left it as it is", path.display());
            }
            crate::clientfile::back_up(path)?;
            text
        }
    };
    write_file(path, &next, false)
}

fn thunar_remove(path: &Path) -> Result<()> {
    let mut text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if let Some(span) = thunar_span(&text) {
        text.replace_range(span, "");
        write_file(path, &text, false)?;
    }
    Ok(())
}

fn xml_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// A desktop entry's `Exec` argument: double-quoted, with the quoting rule's
/// escapes doubled for the string rule read before it, and `%` doubled.
fn desktop_quote(raw: &str) -> String {
    let mut out = String::from("\"");
    for c in raw.chars() {
        match c {
            '"' | '`' | '$' => out.push_str(&format!("\\\\{c}")),
            '\\' => out.push_str("\\\\\\\\"),
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A service Finder lists for any file or folder.
const INFO_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>NSServices</key>
	<array>
		<dict>
			<key>NSMenuItem</key>
			<dict>
				<key>default</key>
				<string>Index with semlith</string>
			</dict>
			<key>NSMessage</key>
			<string>runWorkflowAsService</string>
			<key>NSRequiredContext</key>
			<dict>
				<key>NSApplicationIdentifier</key>
				<string>com.apple.finder</string>
			</dict>
			<key>NSSendFileTypes</key>
			<array>
				<string>public.item</string>
			</array>
		</dict>
	</array>
</dict>
</plist>
"#;

/// One "Run Shell Script" action, input as arguments (`inputMethod` 1), in
/// the shape Automator itself saves.
const DOCUMENT_WFLOW: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>AMApplicationBuild</key>
	<string>534</string>
	<key>AMApplicationVersion</key>
	<string>2.10</string>
	<key>AMDocumentVersion</key>
	<string>2</string>
	<key>actions</key>
	<array>
		<dict>
			<key>action</key>
			<dict>
				<key>AMAccepts</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Optional</key>
					<true/>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>AMActionVersion</key>
				<string>2.0.3</string>
				<key>AMApplication</key>
				<array>
					<string>Automator</string>
				</array>
				<key>AMParameterProperties</key>
				<dict>
					<key>COMMAND_STRING</key>
					<dict/>
					<key>CheckedForUserDefaultShell</key>
					<dict/>
					<key>inputMethod</key>
					<dict/>
					<key>shell</key>
					<dict/>
					<key>source</key>
					<dict/>
				</dict>
				<key>AMProvides</key>
				<dict>
					<key>Container</key>
					<string>List</string>
					<key>Types</key>
					<array>
						<string>com.apple.cocoa.string</string>
					</array>
				</dict>
				<key>ActionBundlePath</key>
				<string>/System/Library/Automator/Run Shell Script.action</string>
				<key>ActionName</key>
				<string>Run Shell Script</string>
				<key>ActionParameters</key>
				<dict>
					<key>COMMAND_STRING</key>
					<string>{COMMAND}</string>
					<key>CheckedForUserDefaultShell</key>
					<true/>
					<key>inputMethod</key>
					<integer>1</integer>
					<key>shell</key>
					<string>/bin/sh</string>
					<key>source</key>
					<string></string>
				</dict>
				<key>BundleIdentifier</key>
				<string>com.apple.RunShellScript</string>
				<key>CFBundleVersion</key>
				<string>2.0.3</string>
				<key>CanShowSelectedItemsWhenRun</key>
				<false/>
				<key>CanShowWhenRun</key>
				<true/>
				<key>Category</key>
				<array>
					<string>AMCategoryUtilities</string>
				</array>
				<key>Class Name</key>
				<string>RunShellScriptAction</string>
				<key>InputUUID</key>
				<string>5C2C0E4A-6C55-4F34-9D3A-1B2E9A3F7D01</string>
				<key>OutputUUID</key>
				<string>5C2C0E4A-6C55-4F34-9D3A-1B2E9A3F7D02</string>
				<key>UUID</key>
				<string>5C2C0E4A-6C55-4F34-9D3A-1B2E9A3F7D03</string>
				<key>UnlocalizedApplications</key>
				<array>
					<string>Automator</string>
				</array>
				<key>arguments</key>
				<dict>
					<key>0</key>
					<dict>
						<key>default value</key>
						<integer>0</integer>
						<key>name</key>
						<string>inputMethod</string>
						<key>required</key>
						<string>0</string>
						<key>type</key>
						<string>0</string>
						<key>uuid</key>
						<string>0</string>
					</dict>
					<key>1</key>
					<dict>
						<key>default value</key>
						<false/>
						<key>name</key>
						<string>CheckedForUserDefaultShell</string>
						<key>required</key>
						<string>0</string>
						<key>type</key>
						<string>0</string>
						<key>uuid</key>
						<string>1</string>
					</dict>
					<key>2</key>
					<dict>
						<key>default value</key>
						<string></string>
						<key>name</key>
						<string>source</string>
						<key>required</key>
						<string>0</string>
						<key>type</key>
						<string>0</string>
						<key>uuid</key>
						<string>2</string>
					</dict>
					<key>3</key>
					<dict>
						<key>default value</key>
						<string></string>
						<key>name</key>
						<string>COMMAND_STRING</string>
						<key>required</key>
						<string>0</string>
						<key>type</key>
						<string>0</string>
						<key>uuid</key>
						<string>3</string>
					</dict>
					<key>4</key>
					<dict>
						<key>default value</key>
						<string>/bin/sh</string>
						<key>name</key>
						<string>shell</string>
						<key>required</key>
						<string>0</string>
						<key>type</key>
						<string>0</string>
						<key>uuid</key>
						<string>4</string>
					</dict>
				</dict>
				<key>conversionLabel</key>
				<integer>0</integer>
				<key>isViewVisible</key>
				<integer>1</integer>
			</dict>
			<key>isViewVisible</key>
			<integer>1</integer>
		</dict>
	</array>
	<key>connectors</key>
	<dict/>
	<key>workflowMetaData</key>
	<dict>
		<key>applicationBundleID</key>
		<string>com.apple.finder</string>
		<key>applicationPath</key>
		<string>/System/Library/CoreServices/Finder.app</string>
		<key>inputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject</string>
		<key>outputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>presentationMode</key>
		<integer>15</integer>
		<key>processesInput</key>
		<false/>
		<key>serviceApplicationBundleID</key>
		<string>com.apple.finder</string>
		<key>serviceApplicationPath</key>
		<string>/System/Library/CoreServices/Finder.app</string>
		<key>serviceInputTypeIdentifier</key>
		<string>com.apple.Automator.fileSystemObject</string>
		<key>serviceOutputTypeIdentifier</key>
		<string>com.apple.Automator.nothing</string>
		<key>serviceProcessesInput</key>
		<false/>
		<key>systemImageName</key>
		<string>NSActionTemplate</string>
		<key>useAutomaticInputType</key>
		<false/>
		<key>workflowTypeIdentifier</key>
		<string>com.apple.Automator.servicesMenu</string>
	</dict>
</dict>
</plist>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_exec_argument_survives_both_escape_rules() {
        assert_eq!(desktop_quote("/opt/a b/semlith"), "\"/opt/a b/semlith\"");
        assert_eq!(desktop_quote("/x/$y%"), "\"/x/\\\\$y%%\"");
    }
}
