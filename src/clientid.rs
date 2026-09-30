//! Which app is on the other end of an MCP connection.
//!
//! A client names itself in `initialize`, and the name is its own choice:
//! Claude Code says `claude-code`, but Claude Desktop, Zed, Copilot in VS Code
//! and Copilot CLI all say `mcp`, IO CLI says `rmcp`, and the Codex CLI and the
//! ChatGPT desktop app both say `codex-mcp-client`. The ledger and the access
//! report filed four apps under one name. The one fact no client can blur is
//! the process that started `semlith mcp`: the stdio proxy reads its own
//! ancestry once, names the app it finds there, and hands it to the daemon.
//! [`label`] then resolves the two into one of the names `docs/clients.md`
//! uses, whichever transport the client connected by.

use std::sync::OnceLock;

/// The header the stdio proxy carries the host app in.
pub const HEADER: &str = "Semlith-Host";

/// The app that started this process, read once from its ancestors.
pub fn host() -> Option<&'static str> {
    static HOST: OnceLock<Option<String>> = OnceLock::new();
    HOST.get_or_init(|| classify_host(&ancestors())).as_deref()
}

/// Whether a `clientInfo.name` says which app it is. The generic ones name
/// the MCP library the app was built with instead — and then the version
/// beside it is the library's, not the app's, so it is not recorded.
pub fn generic(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "" | "mcp" | "rmcp" | "client" | "mcp-client" | "mcp client" | "unnamed client"
    )
}

/// The name a ledger row carries for a connection: one of the clients
/// `docs/clients.md` documents where the connection can be told apart, else
/// what the client called itself.
///
/// A specific `clientInfo` name wins over the host, because one host runs
/// several clients: VS Code's extension host runs Cline as well as Copilot. The
/// host decides where the name is generic, and where one name serves two apps.
pub fn label(client_info: Option<&str>, host: Option<&str>) -> String {
    const CHATGPT: &str = "ChatGPT desktop (the Codex app)";
    let info = client_info.map(str::trim).filter(|n| !n.is_empty());
    let lower = info.map(str::to_ascii_lowercase).unwrap_or_default();
    let named = match lower.as_str() {
        "claude-code" => Some("Claude Code"),
        "claude-ai" | "claude desktop" => Some("Claude Desktop"),
        // Claude Desktop's local agent mode starts its own connection.
        l if l.starts_with("local-agent-mode") && host == Some("Claude Desktop") => {
            Some("Claude Desktop")
        }
        "opencode" => Some("OpenCode"),
        "zed" => Some("Zed"),
        "cursor" | "cursor-vscode" => Some("Cursor"),
        "visual studio code" | "vscode" => Some("GitHub Copilot in VS Code"),
        "io" | "io-cli" => Some("IO CLI"),
        "codex-mcp-client" | "codex" => Some(if host == Some(CHATGPT) {
            CHATGPT
        } else {
            "OpenAI Codex"
        }),
        l if l.starts_with("gemini") => Some("Gemini CLI"),
        l if l.contains("cline") => Some("Cline"),
        l if l.contains("copilot") => Some(match host {
            Some("GitHub Copilot in VS Code") => "GitHub Copilot in VS Code",
            _ => "GitHub Copilot CLI",
        }),
        _ => None,
    };
    if let Some(named) = named {
        return named.to_string();
    }
    match (info, host) {
        // A name this does not know, in an app it does: both, so the row says
        // where it came from. VS Code's extension host runs more than Copilot.
        (Some(info), Some(host)) if !generic(info) => format!("{info} ({})", app_of(host)),
        (Some(info), None) if !generic(info) => info.to_string(),
        (_, Some(host)) => host.to_string(),
        (Some(info), None) => info.to_string(),
        (None, None) => "unnamed client".to_string(),
    }
}

/// The app a host label names, for an unknown client running inside it.
fn app_of(host: &str) -> &str {
    match host {
        "GitHub Copilot in VS Code" => "VS Code",
        "ChatGPT desktop (the Codex app)" => "ChatGPT desktop",
        other => other,
    }
}

/// The documented client an ancestor chain belongs to, nearest ancestor
/// first. An app bundle or install directory is matched before an executable's
/// name, because Copilot in VS Code runs its proxy under a `copilot` binary
/// that lives inside VS Code.
pub fn classify_host(ancestors: &[String]) -> Option<String> {
    for line in ancestors {
        let l = line.to_ascii_lowercase().replace('\\', "/");
        let bundles: [(&str, &[&str]); 5] = [
            (
                "Claude Desktop",
                &[
                    "/claude.app/",
                    "/anthropicclaude/",
                    "/claude_pzs8sxrjxfjjc/",
                    "/claude-desktop",
                ],
            ),
            (
                "ChatGPT desktop (the Codex app)",
                &[
                    "/chatgpt.app/",
                    "/openai.codex",
                    "/usr/lib/chatgpt/",
                    "/openai/codex/bin/",
                ],
            ),
            (
                "Cursor",
                &["/cursor.app/", "/programs/cursor/", "/cursor/cursor"],
            ),
            (
                "GitHub Copilot in VS Code",
                &[
                    "/visual studio code.app/",
                    "/microsoft vs code/",
                    "/usr/share/code/",
                ],
            ),
            (
                "Zed",
                &["/zed.app/", "/zed/zed.exe", "/zed-editor", "/zeditor"],
            ),
        ];
        if let Some((name, _)) = bundles
            .iter()
            .find(|(_, marks)| marks.iter().any(|m| l.contains(m)))
        {
            return Some((*name).to_string());
        }
        let name_of = |word: &str| {
            word.rsplit('/')
                .next()
                .unwrap_or("")
                .trim_end_matches(".exe")
                .trim_end_matches(".cmd")
                .trim_end_matches(".js")
                .to_string()
        };
        let mut words = l.split_whitespace();
        let mut base = name_of(words.next().unwrap_or(""));
        // A CLI npm installed runs as `node <script>`: the script names it.
        if matches!(
            base.as_str(),
            "node" | "bun" | "deno" | "npx" | "python" | "python3"
        ) {
            base = name_of(words.next().unwrap_or(""));
        }
        let base = base.as_str();
        let packages: [(&str, &[&str]); 3] = [
            ("GitHub Copilot CLI", &["@github/copilot"]),
            ("Gemini CLI", &["@google/gemini-cli"]),
            ("OpenAI Codex", &["@openai/codex"]),
        ];
        if let Some((name, _)) = packages
            .iter()
            .find(|(_, marks)| marks.iter().any(|m| l.contains(m)))
        {
            return Some((*name).to_string());
        }
        let by_name = match base {
            "claude" => Some("Claude Code"),
            "codex" => Some("OpenAI Codex"),
            "copilot" => Some("GitHub Copilot CLI"),
            "gemini" => Some("Gemini CLI"),
            "opencode" => Some("OpenCode"),
            "io" => Some("IO CLI"),
            "cline" | ".cline" => Some("Cline"),
            "zed" => Some("Zed"),
            "cursor" => Some("Cursor"),
            _ => None,
        };
        if let Some(name) = by_name {
            return Some(name.to_string());
        }
    }
    None
}

/// The command lines of this process's ancestors, nearest first, at most
/// eight: enough to climb through a shell, `npx` or an editor's helper to the
/// app. Empty when the platform will not say.
fn ancestors() -> Vec<String> {
    platform_ancestors(std::process::id()).unwrap_or_default()
}

#[cfg(unix)]
fn platform_ancestors(pid: u32) -> Option<Vec<String>> {
    let ps = |pid: u32| -> Option<(u32, String)> {
        let out = std::process::Command::new("ps")
            .args(["-o", "ppid=", "-o", "args=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let (ppid, args) = text.split_once(char::is_whitespace)?;
        Some((ppid.trim().parse().ok()?, args.trim().to_string()))
    };
    let (mut next, _) = ps(pid)?;
    let mut out = Vec::new();
    while next > 1 && out.len() < 8 {
        let Some((ppid, args)) = ps(next) else { break };
        out.push(args);
        next = ppid;
    }
    Some(out)
}

#[cfg(windows)]
fn platform_ancestors(pid: u32) -> Option<Vec<String>> {
    // One PowerShell for the whole chain, once per proxy: a CIM query per
    // level from Rust would be eight processes where one does.
    let script = format!(
        "$id={pid}; for($i=0;$i -lt 9;$i++){{ \
           $p=Get-CimInstance Win32_Process -Filter \"ProcessId=$id\"; if(!$p){{break}}; \
           if($i -gt 0){{ Write-Output ((\"\" + $p.ExecutablePath + ' ' + $p.CommandLine).Trim()) }}; \
           $id=$p.ParentProcessId; if($id -le 4){{break}} }}"
    );
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .ok()?;
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

#[cfg(not(any(unix, windows)))]
fn platform_ancestors(_pid: u32) -> Option<Vec<String>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    /// Ancestries read from real `semlith mcp` proxies on 2026-09-30, and
    /// the documented install paths on Linux and Windows.
    #[test]
    fn every_documented_host_is_told_apart_by_its_ancestry() {
        let cases: &[(&[&str], &str)] = &[
            (
                &[
                    "/Applications/Visual Studio Code.app/Contents/Resources/app/node_modules.asar.unpacked/@github/copilot-sdk-darwin-arm64/prebuilds/darwin-arm64/copilot",
                    "/Applications/Visual Studio Code.app/Contents/MacOS/Code",
                ],
                "GitHub Copilot in VS Code",
            ),
            (
                &[
                    "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper (Plugin).app/Contents/MacOS/Code Helper (Plugin) --type=utility",
                ],
                "GitHub Copilot in VS Code",
            ),
            (
                &[
                    "claude --dangerously-skip-permissions",
                    "/bin/zsh -l",
                    "/Applications/WezTerm.app/Contents/MacOS/wezterm-gui",
                ],
                "Claude Code",
            ),
            (
                &[
                    "/Applications/Claude.app/Contents/Frameworks/Claude Helper.app/Contents/MacOS/Claude Helper",
                    "/Applications/Claude.app/Contents/MacOS/Claude",
                ],
                "Claude Desktop",
            ),
            (
                &[
                    "/Applications/ChatGPT.app/Contents/Resources/codex app-server",
                    "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT",
                ],
                "ChatGPT desktop (the Codex app)",
            ),
            (&["/Applications/Zed.app/Contents/MacOS/zed"], "Zed"),
            (
                &[
                    "/Applications/Cursor.app/Contents/Frameworks/Cursor Helper (Plugin).app/Contents/MacOS/Cursor Helper (Plugin)",
                ],
                "Cursor",
            ),
            (
                &[
                    "node /usr/local/lib/node_modules/@github/copilot/npm-loader.js -p hi",
                    "zsh",
                ],
                "GitHub Copilot CLI",
            ),
            (
                &["node /home/me/.nvm/versions/node/v24/bin/gemini -p hi"],
                "Gemini CLI",
            ),
            (&["/home/me/.local/bin/io exec --json hi"], "IO CLI"),
            (&["/home/me/.opencode/bin/opencode run hi"], "OpenCode"),
            (
                &["/home/me/.nvm/versions/node/v24/lib/node_modules/cline/bin/.cline --yolo"],
                "Cline",
            ),
            (
                &["/usr/share/code/code --type=utility"],
                "GitHub Copilot in VS Code",
            ),
            (
                &["/usr/lib/chatgpt/chatgpt"],
                "ChatGPT desktop (the Codex app)",
            ),
            (
                &[r"C:\Users\me\AppData\Local\Programs\Microsoft VS Code\Code.exe --type=utility"],
                "GitHub Copilot in VS Code",
            ),
            (
                &[r"C:\Users\me\AppData\Local\AnthropicClaude\app-1.0\claude.exe"],
                "Claude Desktop",
            ),
            (&[r"C:\Users\me\AppData\Local\Programs\Zed\zed.exe"], "Zed"),
            (
                &[
                    r"C:\Program Files\WindowsApps\OpenAI.Codex_1.2.3_x64__2p2nqsd0c76g0\app\ChatGPT.exe",
                ],
                "ChatGPT desktop (the Codex app)",
            ),
            (
                &[
                    r"C:\Users\me\AppData\Roaming\npm\node_modules\@google\gemini-cli\bundle\gemini.js",
                ],
                "Gemini CLI",
            ),
        ];
        for (lines, want) in cases {
            assert_eq!(
                classify_host(&chain(lines)).as_deref(),
                Some(*want),
                "{lines:?}"
            );
        }
        assert_eq!(
            classify_host(&chain(&["/bin/zsh -l", "/sbin/launchd"])),
            None
        );
    }

    /// The names the twelve clients sent on 2026-09-30, with the host each ran
    /// under: every one lands on its documented name.
    #[test]
    fn each_client_is_filed_under_its_own_name() {
        let cases = [
            (Some("claude-code"), Some("Claude Code"), "Claude Code"),
            (
                Some("codex-mcp-client"),
                Some("OpenAI Codex"),
                "OpenAI Codex",
            ),
            (
                Some("codex-mcp-client"),
                Some("ChatGPT desktop (the Codex app)"),
                "ChatGPT desktop (the Codex app)",
            ),
            (Some("opencode"), None, "OpenCode"),
            (Some("rmcp"), Some("IO CLI"), "IO CLI"),
            (
                Some("mcp"),
                Some("GitHub Copilot CLI"),
                "GitHub Copilot CLI",
            ),
            (Some("gemini-cli-mcp-client"), None, "Gemini CLI"),
            (
                Some("mcp"),
                Some("GitHub Copilot in VS Code"),
                "GitHub Copilot in VS Code",
            ),
            (Some("mcp"), Some("Cursor"), "Cursor"),
            (Some("mcp"), Some("Zed"), "Zed"),
            (
                Some("@cline/core"),
                Some("GitHub Copilot in VS Code"),
                "Cline",
            ),
            (Some("mcp"), Some("Claude Desktop"), "Claude Desktop"),
            // Nothing to go on is said as such, not invented.
            (Some("mcp"), None, "mcp"),
            (None, None, "unnamed client"),
            (Some("some-new-agent"), Some("Zed"), "some-new-agent (Zed)"),
            (
                Some("some-extension"),
                Some("GitHub Copilot in VS Code"),
                "some-extension (VS Code)",
            ),
            (Some("some-new-agent"), None, "some-new-agent"),
            (
                Some("local-agent-mode-semlith"),
                Some("Claude Desktop"),
                "Claude Desktop",
            ),
            (Some("Cline"), Some("GitHub Copilot in VS Code"), "Cline"),
        ];
        for (info, host, want) in cases {
            assert_eq!(label(info, host), want, "{info:?} under {host:?}");
        }
    }
}
