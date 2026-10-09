# Setting up your agent client

How to connect each supported agent client to semlith, and how to check the
connection works. `semlith setup` does most of this for you; the stanzas below
are what it runs or writes, and what to paste by hand when you need to.

This file is also read by code. `src/clients.rs` parses it at compile time for
the portal's Agents page and for `semlith setup`, and `tests/clients.rs` launches
every client stanza, so a flag renamed in the code and not here fails the build.
The setup heading, the three group headings under it and the HTTP heading are
matched literally at the levels they are written at: reword one and the Agents
page silently empties, and do not quote one of them verbatim above it, because
the parser takes the first match. Two fence annotations are read by machine and
hidden when rendered: `register` marks the one command `semlith setup` runs to
register semlith in that client, and `config` carries the path of the file
`semlith setup --register-all` may write.

### Setting it up in your client

You do not paste anything by hand for a client that has a registration CLI.
`semlith setup` runs that CLI itself, at the scope that means every project, and
always in the form where the client launches `semlith mcp` as a subprocess. Six
of the twelve clients below register that way. The rest are a file you edit
once, and `semlith setup --register-all` writes the user-level one for you,
including for an installed client whose CLI is not on `PATH`.

Every client below runs on macOS, Linux and Windows. `~` is your home directory
(`%USERPROFILE%` on Windows); where a client keeps its file elsewhere on one
system, the stanza is marked with that system. On Windows a CLI installed by npm
is a `.cmd` shim (`gemini.cmd`, `copilot.cmd`, and VS Code's `code.cmd`), and
`semlith setup` runs the shim.

A subprocess registration carries no credential: `semlith mcp` reads the agent
key from `~/.semlith/agent.key` itself when it needs one, so there is nothing in
it to rotate or leak. `semlith setup` writes no shell startup block exporting
`SEMLITH_AGENT_KEY`.

The HTTP stanzas are for a daemon on another machine or a client that speaks
only HTTP. They name `${SEMLITH_AGENT_KEY}` rather than the key, so the client
expands it from the environment and the credential stays out of every
configuration file; export it yourself from `semlith key show`. The endpoint
answers a request without that header with 401, so a stanza missing it fails to
connect rather than finding nothing.

No stanza names a store. Either form opens every store in
`~/.semlith/registry.json` (every store `semlith index` has made) plus a
`.semlith` beside the directory the agent started in, if there is one. Index
another repository and an agent already configured can search it.

`--store` still wins when given: repeat it to open a chosen set of stores, or set
`SEMLITH_STORE` to a path-separator-delimited list. A store semlith did not
create is not opened until you trust it once, because a `.semlith` directory can
arrive inside a repository you cloned: `semlith trust ./.semlith` keeps it where
it is, and `semlith adopt ./.semlith` moves it into the home so these stanzas
reach it with no flags.

Every stanza writes `${SEMLITH_BIN}` where the binary goes. It stands for the
absolute path of the semlith that is running: `semlith setup` substitutes it, and
by hand it is whatever `which semlith` prints (`~/.cargo/bin/semlith` after a
`cargo install`). The bare word `semlith` is not enough. It is on your `PATH` in
a shell, but usually not in an editor launched from a desktop icon, a launchd or
systemd agent, or a desktop app, none of which sources a profile; the client
then reports that the server exited and says nothing about why.

Every client that reads the MCP `initialize` reply's `instructions` gets the
routing sentence semlith sends there, which names the folders semlith indexes.
`--register-all` also writes the rule block for clients with a rules file.

A `.semlithignore` file in gitignore syntax, anywhere in an indexed tree, keeps
its paths out of the store the way `.gitignore` does, for what you commit but do
not want searched, such as a fixture copy of the repository. The walk, the
watcher and the daemon's catch-up all honour it, and the portal's Files ▸ Not
indexed tab counts what it left out.

#### Terminal

**Claude Code** — `claude mcp add`, or a committed `.mcp.json` in the project
root. Its scopes are `local`, `user` and `project`, defaulting to `local`, so
`--scope user` makes one registration cover every directory. `--transport http`
points it at the running daemon; in the subprocess form the `--` matters,
because Claude Code reads anything starting with a dash as its own flag. The
JSON below is the project file, which is committed on purpose, so semlith never
writes it.

```json
{
  "mcpServers": {
    "semlith": {
      "type": "http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

```sh
claude mcp add --scope user --transport http semlith http://127.0.0.1:7365/mcp --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}"
```

Or as a subprocess, which needs no key. To replace an existing entry, remove it
first: `remove` takes the same scope flag, and without one it deletes from
whichever scope it finds the name in.

```sh unregister
claude mcp remove --scope user semlith
```

```sh register
claude mcp add --scope user semlith -- "${SEMLITH_BIN}" mcp
```

`semlith setup` also writes two hook entries into `~/.claude/settings.json`. The
`PreToolUse` one fires before a `Bash` grep, rg, find, cat, `sed -n`, head, tail
or awk, a `Read`, a `Grep` or a `Glob` inside a folder semlith indexes, and adds
one line naming a semlith call that answers the same question
(`semlith_search {query: …}`, `semlith_impact {name: …}`, `semlith_read {target:
…}`, `semlith_files {tree: true}`). It stays quiet outside indexed folders, on
commands that search nothing, and after three lines in a session. The
`PostToolUse` one records that a session has called semlith. Both sit beside any
other hooks, and `semlith setup --no-hooks` removes both and leaves the rest of
the file as it was.

The default mode never blocks. `semlith setup --hook-mode gate` refuses raw
lookups until the session has made one semlith call, at most twice, then only
nudges; `--hook-mode hard` always refuses grep, rg and find in an indexed
folder. `--strict` is kept as the name for `gate`.

Gate exists because, on code, an agent with Grep reaches for Grep. On 50
held-out code questions with Opus 5.5, no wording of the instructions, tool
descriptions or soft hook moved it (0 to 5 of 50 sessions called semlith). Under
`gate`, 44 of 50 did, with two more correct answers and about 31 % more cost per
session. It stays opt-in. On documents it is the other way round: 116 of 127
sessions called semlith unforced (#189).

```json hook path=~/.claude/settings.json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash|Read|Grep|Glob",
        "hooks": [{ "type": "command", "command": "${SEMLITH_BIN} hook" }]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "mcp__.*semlith.*",
        "hooks": [{ "type": "command", "command": "${SEMLITH_BIN} hook" }]
      }
    ]
  }
}
```

It also links the semlith Agent Skill into the directory Claude Code reads user
skills from:

```text skills path=~/.claude/skills
semlith
```

It sets `"alwaysLoad": true` on the user-scope server entry in `~/.claude.json`,
so the semlith tools are in context from the first turn rather than behind
ToolSearch. `claude mcp add` has no flag for it, so the entry is upgraded in
place, backed up beside itself first. On 2026-09-25 this alone moved an agent's
first lookup from 33 % to 79 % semlith. Other clients load an MCP server's tools
when they connect and need no equivalent.

It writes a read-only research subagent, `~/.claude/agents/semlith-explorer.md`,
whose tools are the semlith tools plus a bounded `Read` for files semlith does
not index, and which Claude chooses over `Explore` for code research in an
indexed folder. `semlith setup --no-agents` removes it.

`semlith doctor` reports a missing `alwaysLoad`, the hook's mode, the research
agent, and a per-project disable: `disabledMcpServers` or
`disabledMcpjsonServers` naming semlith for the directory you run it from, which
the client's `/mcp` toggle writes and which makes the server silently absent
there. `semlith doctor --fix` clears that one entry for the current directory and
leaves the rest of `~/.claude.json` as it was. Only `--fix` touches it; `setup`
never does, because the disable was your choice in that project.

**OpenAI Codex** — `~/.codex/config.toml`, shared by the CLI, the IDE extension
and the desktop app. The table is `mcp_servers`, with an underscore. A table with
a `url` is a streamable-HTTP server, with the header in an inline `http_headers`
table. `codex mcp add` has no scope flag: its usage is `codex mcp add [OPTIONS]
<NAME> (--url <URL> | -- <COMMAND>...)` and it always writes the user-level
`~/.codex/config.toml`, which covers every project (see
`codex-rs/cli/src/mcp_cmd.rs` in `openai/codex`). For HTTP it has no flag for an
arbitrary header and reads the key from an environment variable, so put
`export SEMLITH_KEY="$SEMLITH_AGENT_KEY"` in your shell profile and run
`codex mcp add semlith --url "http://127.0.0.1:7365/mcp" --bearer-token-env-var SEMLITH_KEY`.

To replace an existing entry:

```sh unregister
codex mcp remove semlith
```

```sh register
codex mcp add semlith -- "${SEMLITH_BIN}" mcp
```

```toml config path=~/.codex/config.toml
[mcp_servers.semlith]
command = "${SEMLITH_BIN}"
args = ["mcp"]
```

Or against a daemon on another machine, over HTTP:

```toml
[mcp_servers.semlith]
url = "http://127.0.0.1:7365/mcp"
http_headers = { Authorization = "Bearer ${SEMLITH_AGENT_KEY}" }
```

Its user-level instructions file, read on every session, to which
`semlith setup --register-all` appends the rule block — see
<https://developers.openai.com/codex/guides/agents-md>.

```md rules path=~/.codex/AGENTS.md
${SEMLITH_RULES}
```

**OpenCode** — `opencode.json` in the project root, or the same file under
`~/.config/opencode/`. The root key is `mcp`, the HTTP transport is spelled
`remote`, and an entry is ignored until `enabled` is true. The installed CLI has
no global scope flag (`opencode mcp add --help` on 1.18.11 lists only `--url`,
`--env` and `--header`), so its registration lands in the project file and
semlith does not run it. OpenCode v2's documentation adds `--global`
(<https://opencode.ai/v2/docs/mcp-servers>); once your build accepts it,
`opencode mcp add semlith --global -- semlith mcp` is the form to use. Until
then the user-level file below covers every project, and `semlith setup
--register-all` writes it. The path is the same on Windows,
`%USERPROFILE%\.config\opencode\opencode.json` (not `%APPDATA%`), and an
existing `opencode.jsonc` there is read in its place.

```json config path=~/.config/opencode/opencode.json
{
  "mcp": {
    "semlith": {
      "type": "local",
      "command": ["${SEMLITH_BIN}", "mcp"],
      "enabled": true
    }
  }
}
```

OpenCode reads a user-level `AGENTS.md`. `semlith setup --register-all` appends
the rule block to it between markers of its own, backing the file up first.
Nothing outside the markers is touched, and a second run replaces what is
between them.

```md rules path=~/.config/opencode/AGENTS.md
${SEMLITH_RULES}
```

Or against a daemon on another machine, over HTTP:

```json
{
  "mcp": {
    "semlith": {
      "type": "remote",
      "url": "http://127.0.0.1:7365/mcp",
      "enabled": true,
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

```sh register scope=project
opencode mcp add semlith -- "${SEMLITH_BIN}" mcp
```

**IO CLI** — `io.local.toml` in the project root. Servers are an array of
tables, so each is its own `[[mcp]]` block with the name as a field. That file is
per-checkout, so it is not tagged for `--register-all`; the CLI is what reaches
every project. Its scopes are `user`, `project` and `local` (`--scope global` is
rejected by name), so `--scope user` means your own file everywhere. On Windows
the installer puts `io.exe` in `%LOCALAPPDATA%\io\bin` without adding it to
`PATH`; add that directory before running `semlith setup`, or the CLI reads as
not installed. `io mcp add --scope user` writes the entry into
`~/.io-cli/io.toml`, which is also where `semlith doctor` reads it:

```toml config path=~/.io-cli/io.toml
[[mcp]]
id = "semlith"
transport = "stdio"
command = "${SEMLITH_BIN}"
args = ["mcp"]
```

Or against a daemon on another machine, over HTTP:

```toml
[[mcp]]
id = "semlith"
url = "http://127.0.0.1:7365/mcp"
headers = { Authorization = "Bearer ${SEMLITH_AGENT_KEY}" }
```

To replace an existing entry (`remove` takes no `--scope`; io goes to whichever
file declares the name):

```sh unregister
io mcp remove semlith
```

```sh register
io mcp add --scope user semlith -- "${SEMLITH_BIN}" mcp
```

**GitHub Copilot CLI** — `~/.copilot/mcp-config.json`, or `/mcp add` in a
session. Its name for a subprocess is `local` rather than `stdio`; a remote
server is `http`. `copilot mcp add` has no scope flag and always writes the user
configuration at `~/.copilot/mcp-config.json`, which covers every repository; a
per-repository entry means editing `.mcp.json` by hand — see
<https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers>.

```json config path=~/.copilot/mcp-config.json
{
  "mcpServers": {
    "semlith": {
      "type": "local",
      "command": "${SEMLITH_BIN}",
      "args": ["mcp"],
      "tools": ["*"],
      "deferTools": "never"
    }
  }
}
```

Or against a daemon on another machine, over HTTP:

```json
{
  "mcpServers": {
    "semlith": {
      "type": "http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" },
      "tools": ["*"]
    }
  }
}
```

```sh
copilot mcp add --transport http semlith http://127.0.0.1:7365/mcp --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}"
```

To replace an existing entry:

```sh unregister
copilot mcp remove semlith
```

```sh register
copilot mcp add semlith -- "${SEMLITH_BIN}" mcp
```

Its user-level instructions file, read on every session, to which
`semlith setup --register-all` appends the rule block — see
<https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-custom-instructions>.

```md rules path=~/.copilot/copilot-instructions.md
${SEMLITH_RULES}
```

**Gemini CLI** — `~/.gemini/settings.json`. No Gemini-specific Agent Skill or
steering hook is written, because neither has been run against a live Gemini
CLI. The key for a streamable-HTTP server is `httpUrl`, not `url`; a plain `url`
is read as the older SSE transport and the connection fails. `gemini mcp add`
takes `-s, --scope` and writes `~/.gemini/settings.json` for `user` and
`.gemini/settings.json` for `project`, so `--scope user` covers everything
(<https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/mcp-server.md>).

```json config path=~/.gemini/settings.json
{
  "mcpServers": {
    "semlith": {
      "command": "${SEMLITH_BIN}",
      "args": ["mcp"]
    }
  }
}
```

Or against a daemon on another machine, over HTTP:

```json
{
  "mcpServers": {
    "semlith": {
      "httpUrl": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

```sh
gemini mcp add --transport http --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}" semlith http://127.0.0.1:7365/mcp
```

To replace an existing entry, name the scope: `remove` takes the same flag and
defaults to `project`.

```sh unregister
gemini mcp remove --scope user semlith
```

```sh register
gemini mcp add --scope user semlith "${SEMLITH_BIN}" mcp
```

A user-scope registration does not mean the server runs everywhere. With folder
trust on (the default), Gemini CLI disables user-level MCP servers in any folder
not listed in `~/.gemini/trustedFolders.json`, and `gemini mcp list` there shows
semlith as Disabled. Trust the folder and semlith connects.

Its user-level instructions file, read on every session, to which
`semlith setup --register-all` appends the rule block — see
<https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/gemini-md.md>.

```md rules path=~/.gemini/GEMINI.md
${SEMLITH_RULES}
```

#### Editors

**GitHub Copilot in VS Code** — `.vscode/mcp.json` for a workspace, or the
profile copy that `MCP: Open User Configuration` opens. The root key is
`servers`, not `mcpServers`. The JSON below is the per-project workspace file,
so it is not written for you; the CLI covers every workspace. `code --add-mcp`
has no scope flag and needs none: it saves into the user profile's `mcp.json`,
which every workspace sees
(<https://code.visualstudio.com/docs/agent-customization/mcp-servers>).

```json
{
  "servers": {
    "semlith": {
      "type": "http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

```sh
code --add-mcp '{"name":"semlith","type":"http","url":"http://127.0.0.1:7365/mcp","headers":{"Authorization":"Bearer ${SEMLITH_AGENT_KEY}"}}'
```

```sh register
code --add-mcp '{"name":"semlith","command":"${SEMLITH_BIN}","args":["mcp"]}'
```

`code` is on `PATH` after a Linux or Windows install (`code.cmd` on Windows), and
on macOS only after *Shell Command: Install 'code' command in PATH*. Without it,
`semlith setup --register-all` writes the profile's `mcp.json` directly, at the
path for your system:

```json config os=macos path="~/Library/Application Support/Code/User/mcp.json"
{
  "servers": {
    "semlith": {
      "type": "stdio",
      "command": "${SEMLITH_BIN}",
      "args": ["mcp"]
    }
  }
}
```

```json config os=linux path=~/.config/Code/User/mcp.json
{
  "servers": {
    "semlith": {
      "type": "stdio",
      "command": "${SEMLITH_BIN}",
      "args": ["mcp"]
    }
  }
}
```

```json config os=windows path=%APPDATA%\Code\User\mcp.json
{
  "servers": {
    "semlith": {
      "type": "stdio",
      "command": "${SEMLITH_BIN}",
      "args": ["mcp"]
    }
  }
}
```

With your own model key (BYOK) and no GitHub sign-in, Copilot Chat also needs
`chat.utilitySmallModel` pointed at one of your own models; the registration
alone is not enough.

**Cursor** — `~/.cursor/mcp.json` everywhere, or `.cursor/mcp.json` in one
repo. Cursor infers the transport from the presence of `url` and has no `type`
field; one copied from another client confuses it.

Cursor is connection-tested each release: the registration below is exercised
and its MCP log is checked for `connected=true`. Its tool calls are not tested,
so a search that returns wrong results there is worth reporting.

```json config path=~/.cursor/mcp.json
{
  "mcpServers": {
    "semlith": {
      "command": "${SEMLITH_BIN}",
      "args": ["mcp"]
    }
  }
}
```

Or against a daemon on another machine, over HTTP:

```json
{
  "mcpServers": {
    "semlith": {
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

**Zed** — `~/.config/zed/settings.json` on macOS and Linux (`$XDG_CONFIG_HOME/zed`
when that is set, and `~/.var/app/dev.zed.Zed/config/zed` for the Flatpak), and
`%APPDATA%\Zed\settings.json` on Windows; the `zed: open settings file` command
opens whichever it is. Zed calls MCP servers context servers, under
`context_servers`. Its remote support has moved between versions and older
builds start local processes only, so this is the stdio form, which needs no
key: `semlith mcp` proxies to the running daemon. Zed has no registration CLI,
so `semlith setup --register-all` writes the entry into that file.

Zed allows comments and trailing commas in `settings.json`, which a strict JSON
parser rejects. When the file has either, semlith leaves it untouched and prints
the stanza below with the reason, for you to paste in.

```json config os=macos,linux path=~/.config/zed/settings.json
{
  "context_servers": {
    "semlith": {
      "source": "custom",
      "command": "${SEMLITH_BIN}",
      "args": ["mcp"]
    }
  }
}
```

```json config os=windows path=%APPDATA%\Zed\settings.json
{
  "context_servers": {
    "semlith": {
      "source": "custom",
      "command": "${SEMLITH_BIN}",
      "args": ["mcp"]
    }
  }
}
```

Its user-level instructions file, read on every session, to which
`semlith setup --register-all` appends the rule block — see
<https://github.com/zed-industries/zed/blob/main/docs/src/ai/rules.md>.

```md rules os=macos,linux path=~/.config/zed/AGENTS.md
${SEMLITH_RULES}
```

```md rules os=windows path=%APPDATA%\Zed\AGENTS.md
${SEMLITH_RULES}
```

**Cline** — `~/.cline/data/settings/cline_mcp_settings.json`, the one file both
the VS Code extension (4.x) and the CLI (3.x) read, and the one the MCP Servers
panel's Configure button opens. It is the same path under your home on macOS,
Linux and Windows, it moves with `CLINE_DIR` when that is set, and Cline creates
it as `{"mcpServers": {}}` on first activation. `CLINE_DATA_DIR` and
`CLINE_MCP_SETTINGS_PATH` also move it, and take precedence; with either set,
paste the stanza into the file they name. `semlith setup --register-all` writes
the entry below, which covers the extension and the CLI at once
(<https://docs.cline.bot/mcp/mcp-overview>).

```json config path=~/.cline/data/settings/cline_mcp_settings.json root=CLINE_DIR
{
  "mcpServers": {
    "semlith": {
      "command": "${SEMLITH_BIN}",
      "args": ["mcp"]
    }
  }
}
```

Or against a daemon on another machine, over HTTP. The transport is
`streamableHttp` in camel case; anything else falls back to SSE and the endpoint
answers 405:

```json
{
  "mcpServers": {
    "semlith": {
      "type": "streamableHttp",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" },
      "disabled": false,
      "autoApprove": []
    }
  }
}
```

Its user-level instructions file, read on every session, to which
`semlith setup --register-all` appends the rule block — see
<https://docs.cline.bot/customization/cline-rules>.

```md rules path=~/Documents/Cline/Rules/semlith.md
${SEMLITH_RULES}
```

#### Desktop apps

**Claude Desktop** — `~/Library/Application Support/Claude/claude_desktop_config.json`
on macOS, `%APPDATA%\Claude\claude_desktop_config.json` on Windows (the Microsoft
Store build reads its own copy under
`%LOCALAPPDATA%\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Claude`, so
semlith writes both where each exists), and
`~/.config/Claude/claude_desktop_config.json` on Linux, where Anthropic's build
is a beta for Debian and Ubuntu. Settings → Developer → Edit Config opens it.
Desktop starts local processes only and has nowhere to put a header, so this is
the stdio form and needs no key: `semlith mcp` proxies to the running daemon.
Quit and reopen the app after editing.

```json config os=macos path="~/Library/Application Support/Claude/claude_desktop_config.json"
{
  "mcpServers": {
    "semlith": {
      "command": "${SEMLITH_BIN}",
      "args": ["mcp"]
    }
  }
}
```

```json config os=windows path=%APPDATA%\Claude\claude_desktop_config.json
{
  "mcpServers": {
    "semlith": {
      "command": "${SEMLITH_BIN}",
      "args": ["mcp"]
    }
  }
}
```

```json config os=windows path=%LOCALAPPDATA%\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Claude\claude_desktop_config.json
{
  "mcpServers": {
    "semlith": {
      "command": "${SEMLITH_BIN}",
      "args": ["mcp"]
    }
  }
}
```

```json config os=linux path=~/.config/Claude/claude_desktop_config.json
{
  "mcpServers": {
    "semlith": {
      "command": "${SEMLITH_BIN}",
      "args": ["mcp"]
    }
  }
}
```

**ChatGPT desktop (the Codex app)** — OpenAI's desktop app for macOS, Windows
(Microsoft Store) and Linux (a preview for Ubuntu, Debian, Fedora and Arch). Its
Codex reads the same `~/.codex/config.toml` as the Codex CLI and IDE extension
(`%USERPROFILE%\.codex\config.toml` on Windows), so one registration serves all
three. With the Codex CLI installed, its registration covers the app; without
it, `semlith setup --register-all` appends the table below to that file on every
system. Quit and reopen the app after registering; `/mcp` in a Codex thread lists
the servers it started.

```toml config path=~/.codex/config.toml
[mcp_servers.semlith]
command = "${SEMLITH_BIN}"
args = ["mcp"]
```

### Connecting over HTTP

`semlith start` also answers MCP at `http://127.0.0.1:7365/mcp`, so a client that
speaks HTTP needs no subprocess and no path. The endpoint is authenticated by the
agent key in `~/.semlith/agent.key`: 32 bytes from the OS random source, written
`sml_` and hex at mode 0600, created on first start and never reminted unless you
ask. It survives daemon restarts, upgrades and rotation of the portal's session
token, so a stanza carrying it keeps working. `semlith key show` prints the live
key and the stanza around it.

```sh
claude mcp add --scope user --transport http semlith http://127.0.0.1:7365/mcp --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}"
```

```json
{
  "mcpServers": {
    "semlith": {
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

`semlith key rotate`, or the portal's Rotate key button, mints a new key and
carries it forward: it rewrites every client configuration file on this machine
that held the old key — each client's documented path above, plus the
project-scoped files beside wherever the daemon was started. A file is touched
only when the exact old key appears in it, nothing is created, and both the
command and the page list what changed. The previous key keeps working for
fifteen minutes, and never past the running daemon's exit, so an open session
finishes; `--now` drops it immediately. A client registered through its own CLI
is unaffected, because those are subprocess registrations holding no key. A
client configured anywhere else needs the new stanza pasted in, and is named so
you know.

The stdio stanzas stay as they are: `semlith mcp` forwards to a running daemon
and opens the stores itself when none is running, so it works whether or not
`semlith start` is up. Close the HTTP endpoint with `semlith start
--no-mcp-http`, or from the portal's Agents page while the daemon runs; that
drops the route, not the daemon.

## Knowing it is there

A registration that reads correctly is not a server that answers. `semlith
doctor` runs the steps rather than reading the file: what a client would run,
whether that launches with the environment a service manager gives a job, how
many tools it lists, and, for Claude Code from a directory that is nobody's
project, what the client itself says. It names the first step that failed and
the command that shows it.

It also catches a server registered at user scope but **switched off for one
directory**. `~/.claude.json` can carry

```json
"projects": {
  "/path/to/your/repo": { "disabledMcpjsonServers": ["semlith"] }
}
```

and every check run from any other directory says the server is connected,
because it is. `semlith doctor` run in that directory names the file, the key
and the directory, and prints the command that turns it back on. It never turns
it back on itself: switching a server off is a choice.

`semlith doctor --brief` is one line and an exit code, non-zero when an agent
opening a session in this directory would not find semlith. It only reads, so
it is safe in a hook that runs on every session. For Claude Code, in
`~/.claude/settings.json`:

```json
{
  "hooks": {
    "SessionStart": [
      { "hooks": [{ "type": "command", "command": "semlith doctor --brief" }] }
    ]
  }
}
```

semlith does not write that hook for you: a hook runs on every session, and
adding one unasked is a choice you should make.

## Model, tokens and cost in the ledger

MCP tells semlith which tool a client called and with what, but nothing about
the model behind the call. With **Usage from client logs** on (the Privacy page,
or `semlith ledger --usage on`; off by default), semlith reads each client's own
session log for the model request that issued each call, and keeps that
request's model, input, output, cache-read and cache-write tokens, and the cost
where the client records one. Logs are opened read-only and only those numbers
are kept. Paths below are macOS; Linux and Windows follow each client's own home
or application-data folder.

| Client | Read from | Per request | Cost |
|---|---|---|---|
| Claude Code | `~/.claude/projects/**/*.jsonl` (`CLAUDE_CONFIG_DIR`) | yes | from the price table |
| OpenAI Codex | `~/.codex/sessions/**/rollout-*.jsonl` (`CODEX_HOME`) | yes | from the price table |
| ChatGPT desktop (the Codex app) | the same rollouts, told apart by `originator` | yes | from the price table |
| OpenCode | `~/.local/share/opencode/opencode.db` | yes | OpenCode's own |
| Gemini CLI | `~/.gemini/tmp/*/chats/session-*.jsonl` | yes | from the price table |
| GitHub Copilot CLI | `~/.copilot/session-state/*/events.jsonl` and `session-store.db` | yes | from the price table, as the API price of the same tokens |
| GitHub Copilot in VS Code | the agent debug log, `…/GitHub.copilot-chat/debug-logs/*/main.jsonl` | only with `chat.agentDebugLog.fileLogging.enabled` on; otherwise `not recorded` | from the price table |
| Cline | `~/.cline/data/sessions/*/*.messages.json` | yes | Cline's own |
| Claude Desktop | agent-mode sessions under `~/Library/Application Support/Claude/local-agent-mode-sessions` | agent mode only; chat keeps no usage on the machine and says `not recorded` | from the price table |
| IO CLI | `~/.io-cli/runs.db` | yes | from the price table |
| Zed | — | `not recorded`: Zed keeps a per-thread total, not the request behind each call | — |
| Cursor | — | `not recorded`: Cursor stores no token counts on the machine | — |

The price table is a models.dev snapshot built into the binary; `semlith prices
update` refreshes it. A call a subscription paid for, such as Copilot or Claude
on a plan, is shown at the API price of the same tokens.
