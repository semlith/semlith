# Setting up your agent client

Every client below is launched and answered by `tests/clients.rs`, so a flag
renamed in the code and not here fails the build rather than somebody's first
attempt. The portal's Agents page shows the same text: `src/clients.rs` parses
this file at compile time, which is the one way the page and the test cannot
drift apart.

The headings below are a parser contract as much as a table of contents. The
setup heading, the three group headings under it, and the HTTP heading are all
matched literally by `src/clients.rs`, at exactly the levels they are written
at: the stdio stanzas are read from under the setup heading and grouped by the
three below it, and the HTTP ones from under the last. Reword any of the five
and the portal's Agents page silently empties. Change the prose around them
instead, and do not quote one of them verbatim above this line — the parser
takes the first match in the file.

Two things in the fences are read by machine and shown to nobody. A fence
marked `register` holds the one command `semlith setup` runs to register semlith
in that client, and a fence marked `config` carries the path of the file
`semlith setup --register-all` may write. Markdown drops everything after the
language when it renders, so both are invisible on the page and in the portal.

### Setting it up in your client

From 0.18.0 you do not paste any of this by hand for a client that has a
registration CLI. `semlith setup` runs that CLI itself, at the scope that means
every project rather than this directory, and always in the form where the
client launches `semlith mcp` as a subprocess. Sixteen of the clients below
register that way; the rest are a file you edit once, and `semlith setup
--register-all` will write the user-level one for you.

A subprocess registration carries no credential, so there is nothing in it to
rotate and nothing on disk to leak. `semlith mcp` reads the agent key from
`~/.semlith/agent.key` itself when it needs one. The shell startup block that
earlier versions wrote to export `SEMLITH_AGENT_KEY` is gone, and `semlith
setup` no longer writes one.

The HTTP stanzas are still here and still correct. Use one when the daemon runs
on another machine, or when a client speaks only HTTP. Those stanzas name
`${SEMLITH_AGENT_KEY}` rather than the key itself — the client expands it from
the environment at start, so the credential stays out of every configuration
file, and you export it yourself from `semlith key show`. The endpoint answers a
request without that header with 401 and nothing else, so a stanza that drops it
fails to connect rather than failing to find anything.

No snippet below names a store. Either form opens every store registered in
`~/.semlith/registry.json` — which is every store `semlith index` has made —
plus a `.semlith` beside the directory the agent was started in, if there is
one. Index another repository and the agent that is already configured can
search it, with no edit to any of these files.

`--store` still works and still wins when it is given: repeat it to open a
chosen set of stores, or set `SEMLITH_STORE` to a path-separator-delimited
list. A store that sits beside its corpus keeps working where it is, and
`semlith trust ./.semlith` is enough to keep using it where it is — from 0.14.0
a store semlith did not create is not opened until you have said so once,
because a `.semlith` directory can arrive inside a repository you cloned.
`semlith adopt ./.semlith` moves it into the home instead, so these stanzas
reach it with no flags.

`cargo install semlith` puts the binary at `~/.cargo/bin/semlith`, which is on
your `PATH` in a shell but often not in an editor launched from a desktop icon
— the editor and desktop entries below use the absolute path.

#### Terminal

**Claude Code** — `claude mcp add`, or a committed `.mcp.json` in the project
root. Its scopes are `local`, `user` and `project`, defaulting to `local`, so
`--scope user` is what makes one registration cover every directory.
`--transport http` points it at the running daemon; without it, the `--`
matters, because Claude Code reads anything starting with a dash as one of its
own flags. The JSON below is the project file, which is committed on purpose,
so semlith never writes it.

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
claude mcp add --transport http semlith http://127.0.0.1:7365/mcp --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}"
```

Or as a subprocess, which needs no key:

Replacing an existing entry — `remove` takes the same scope flag, and without
one it deletes from whichever scope it finds the name in:

```sh unregister
claude mcp remove --scope user semlith
```

```sh register
claude mcp add --scope user semlith -- semlith mcp
```

**OpenAI Codex** — `~/.codex/config.toml`, shared by the CLI, the IDE extension
and the desktop app. TOML, and the table is `mcp_servers` with an underscore. A
table with a `url` in it is a streamable-HTTP server; the header goes in an
inline `http_headers` table. `codex mcp add` has no scope flag at all: its usage
is `codex mcp add [OPTIONS] <NAME> (--url <URL> | -- <COMMAND>...)` and it
always writes the user-level `~/.codex/config.toml`, which is already every
project — see `codex-rs/cli/src/mcp_cmd.rs` in `openai/codex`. For the HTTP form
it has no flag for an arbitrary header either: it takes the key from an
environment variable instead, so `export SEMLITH_KEY="$SEMLITH_AGENT_KEY"` has
to live in your shell profile rather than in the one command —
`codex mcp add semlith --url "http://127.0.0.1:7365/mcp" --bearer-token-env-var SEMLITH_KEY`.

Replacing an existing entry:

```sh unregister
codex mcp remove semlith
```

```sh register
codex mcp add semlith -- semlith mcp
```

```toml config path=~/.codex/config.toml
[mcp_servers.semlith]
command = "semlith"
args = ["mcp"]
```

Or against a daemon on another machine, over HTTP:

```toml
[mcp_servers.semlith]
url = "http://127.0.0.1:7365/mcp"
http_headers = { Authorization = "Bearer ${SEMLITH_AGENT_KEY}" }
```

**OpenCode** — `opencode.json` in the project root, or the same file under
`~/.config/opencode/`. The root key is `mcp`, the transport is spelled
`remote`, and an entry is ignored until `enabled` is true. The installed CLI has
no global scope flag: `opencode mcp add --help` on 1.18.11 lists only `--url`,
`--env` and `--header`, so the registration lands in the project file. OpenCode
v2's documentation adds a `--global` flag
(<https://opencode.ai/v2/docs/mcp-servers>), and once your build accepts it,
`opencode mcp add semlith --global -- semlith mcp` is the form to use; until
then the user-level file below is the thing that covers every project, and
`semlith setup --register-all` writes it.

```json config path=~/.config/opencode/opencode.json
{
  "mcp": {
    "semlith": {
      "type": "local",
      "command": ["semlith", "mcp"],
      "enabled": true
    }
  }
}
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
opencode mcp add semlith -- semlith mcp
```

**IO CLI** — `io.local.toml` in the project root. The servers are an array of
tables rather than a map, so each one is its own `[[mcp]]` block and the name
is a field inside it. That file is per-checkout, so it is not tagged for
`--register-all`; the CLI is what reaches every project. Its scopes are `user`,
`project` and `local` — there is no `global`, and `--scope global` is rejected
by name — so `--scope user` is the one that means your own file everywhere.

```toml
[[mcp]]
id = "semlith"
url = "http://127.0.0.1:7365/mcp"
headers = { Authorization = "Bearer ${SEMLITH_AGENT_KEY}" }
```

Replacing an existing entry — `remove` takes no `--scope`, because io goes to
whichever file declares the name:

```sh unregister
io mcp remove semlith
```

```sh register
io mcp add --scope user semlith -- semlith mcp
```

**GitHub Copilot CLI** — `~/.copilot/mcp-config.json`, or `/mcp add` in a
session. Its name for a subprocess is `local` rather than `stdio`, but a
remote server is the ordinary `http`. `copilot mcp add` has no scope flag; it
always writes the user configuration at `~/.copilot/mcp-config.json`, which
already covers every repository, and a per-repository entry means editing
`.mcp.json` by hand instead — see
<https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers>.

```json config path=~/.copilot/mcp-config.json
{
  "mcpServers": {
    "semlith": {
      "type": "local",
      "command": "semlith",
      "args": ["mcp"],
      "tools": ["*"]
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

Replacing an existing entry:

```sh unregister
copilot mcp remove semlith
```

```sh register
copilot mcp add semlith -- semlith mcp
```

**Gemini CLI** — `~/.gemini/settings.json`. The key for a streamable-HTTP
server is `httpUrl`, not `url`; a plain `url` is read as the older SSE
transport and the connection fails. `gemini mcp add` takes `-s, --scope` and
writes `~/.gemini/settings.json` for `user` and `.gemini/settings.json` for
`project`, so `--scope user` is the one that covers everything
(<https://github.com/google-gemini/gemini-cli/blob/main/docs/tools/mcp-server.md>).

```json config path=~/.gemini/settings.json
{
  "mcpServers": {
    "semlith": {
      "command": "semlith",
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

Replacing an existing entry — `remove` takes the same scope flag and defaults to
`project`, so the `user` one has to be named:

```sh unregister
gemini mcp remove --scope user semlith
```

```sh register
gemini mcp add --scope user semlith semlith mcp
```

**Qwen Code** — `~/.qwen/settings.json`, the same schema as Gemini CLI down to
`httpUrl` winning over `url` when both are present. The CLI is the same shape
too: `-s, --scope user` writes `~/.qwen/settings.json` and `--scope project`
writes `.qwen/settings.json`
(<https://github.com/QwenLM/qwen-code/blob/main/docs/users/features/mcp.md>).

```json config path=~/.qwen/settings.json
{
  "mcpServers": {
    "semlith": {
      "command": "semlith",
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
qwen mcp add --transport http semlith http://127.0.0.1:7365/mcp --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}"
```

Replacing an existing entry — unlike `add`, Qwen's `remove` documents no scope
flag at all:

```sh unregister
qwen mcp remove semlith
```

```sh register
qwen mcp add --scope user semlith semlith mcp
```

**Amp** — `~/.config/amp/settings.json`, or the editor extension's own
`settings.json`. The root key is the dotted string `amp.mcpServers`, which
means the entry lives inside a wider settings file rather than in one of its
own. `amp mcp add` has no scope flag, and does not need one: it always writes
the global user settings at `~/.config/amp/settings.json`, and a workspace
entry means editing `.amp/settings.json` by hand
(<https://ampcode.com/docs/customize/mcp>).

```json config path=~/.config/amp/settings.json
{
  "amp.mcpServers": {
    "semlith": {
      "command": "semlith",
      "args": ["mcp"]
    }
  }
}
```

Or against a daemon on another machine, over HTTP:

```json
{
  "amp.mcpServers": {
    "semlith": {
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

```sh register
amp mcp add semlith -- semlith mcp
```

**Crush** — `crush.json` in the project root. The root key is `mcp`, not
`mcpServers`, and the TUI reads the file once at start, so relaunch it after
editing. Semlith cannot register Crush for you: it has no registration CLI, and
the only configuration file Charm documents is `crush.json` in a project root,
which is per-checkout rather than global. Paste the stanza below into each
repository you want it in.

```json
{
  "$schema": "https://charm.land/crush.json",
  "mcp": {
    "semlith": {
      "type": "http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

**Droid** — `~/.factory/mcp.json` for every project, or `.factory/mcp.json` in
one repository. `droid mcp add` has no scope flag; Factory's own documentation
says servers added that way "always go to your user config", so the default is
already every project (<https://docs.factory.ai/cli/configuration/mcp>). Its
flags are `--type`, `--env`, `--header` and `--no-oauth`, and a subprocess entry
takes the command as one quoted argument rather than after a `--`.

```json config path=~/.factory/mcp.json
{
  "mcpServers": {
    "semlith": {
      "type": "stdio",
      "command": "semlith",
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
      "type": "http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

```sh
droid mcp add semlith http://127.0.0.1:7365/mcp --type http --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}"
```

Replacing an existing entry:

```sh unregister
droid mcp remove semlith
```

```sh register
droid mcp add semlith "semlith mcp" --type stdio
```

**Goose** — `goose configure` → Add Extension → Remote Extension, or
`~/.config/goose/config.yaml`. Goose calls them extensions, spells the
transport `streamable_http` with an underscore, and takes the address as `uri`
rather than `url`. `goose configure` is a wizard rather than a one-line command,
so there is nothing to register non-interactively; the file below is written
instead.

```yaml config path=~/.config/goose/config.yaml
extensions:
  semlith:
    type: stdio
    name: semlith
    enabled: true
    cmd: "semlith"
    args:
      - mcp
    timeout: 300
```

Or against a daemon on another machine, over HTTP:

```yaml
extensions:
  semlith:
    type: streamable_http
    name: semlith
    enabled: true
    uri: "http://127.0.0.1:7365/mcp"
    headers:
      Authorization: "Bearer ${SEMLITH_AGENT_KEY}"
    timeout: 300
```

**Amazon Q Developer CLI** — `~/.aws/amazonq/mcp.json` for every workspace, or
`.amazonq/mcp.json` in one. A remote entry takes only `type` and `url` and
authenticates over OAuth, with nowhere to put a header, so this is the stdio
form, which needs no key: `semlith mcp` proxies to the running daemon. The CLI
spells its scope `--scope global`, against `workspace` and `default`
(<https://github.com/aws/amazon-q-developer-cli/issues/2932>).

```json config path=~/.aws/amazonq/mcp.json
{
  "mcpServers": {
    "semlith": {
      "command": "semlith",
      "args": ["mcp"]
    }
  }
}
```

Replacing an existing entry — `remove` takes the same `--name` and `--scope`,
and `rm` is an alias for it:

```sh unregister
q mcp remove --name semlith --scope global
```

```sh register
q mcp add --name semlith --command semlith --args mcp --scope global
```

**OpenClaw** — `~/.openclaw/openclaw.json`. The servers are nested two deep
under `mcp` then `servers`, and the transport field is called `transport`, not
`type`. `openclaw mcp add` has no scope flag: its documented flags are
`--command`, `--arg`, `--env`, `--cwd` for a subprocess and `--url`,
`--transport`, `--header`, `--auth` for a remote one, and a definition saved
either way is a global one (<https://docs.openclaw.ai/cli/mcp/registry>).

```json config path=~/.openclaw/openclaw.json
{
  "mcp": {
    "servers": {
      "semlith": {
        "command": "semlith",
        "args": ["mcp"]
      }
    }
  }
}
```

Or against a daemon on another machine, over HTTP:

```json
{
  "mcp": {
    "servers": {
      "semlith": {
        "transport": "streamable-http",
        "url": "http://127.0.0.1:7365/mcp",
        "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
      }
    }
  }
}
```

```sh
openclaw mcp add semlith --url http://127.0.0.1:7365/mcp --transport streamable-http --header "Authorization=Bearer ${SEMLITH_AGENT_KEY}"
```

Replacing an existing entry — OpenClaw spells the removal `unset`, which deletes
the whole definition by name and fails if that name is not there:

```sh unregister
openclaw mcp unset semlith
```

```sh register
openclaw mcp add semlith --command semlith --arg mcp
```

**DeepSeek** — `~/.deepseek/mcp.json`, read by DeepSeek-TUI, which has since
renamed itself Codewhale and now looks in `~/.codewhale/mcp.json` first and
falls back to the old path. Either file takes `servers` or `mcpServers` as the
root key, and needs no transport field: a `url` is enough. `codewhale mcp add`
has no scope flag and no project-level file to choose between — there is one
configuration file, `~/.codewhale/mcp.json`, overridable only by
`DEEPSEEK_MCP_CONFIG` (<https://codewhale.net/en/docs/mcp>). It also has no
header flag, so for the HTTP form the key goes in the environment:
`export SEMLITH_KEY="$SEMLITH_AGENT_KEY"`, then
`codewhale mcp add semlith --url "http://127.0.0.1:7365/mcp" --bearer-token-env-var SEMLITH_KEY`.

```json config path=~/.codewhale/mcp.json
{
  "mcpServers": {
    "semlith": {
      "command": "semlith",
      "args": ["mcp"],
      "env": {},
      "disabled": false
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

```sh register
codewhale mcp add semlith --command "semlith" --arg "mcp"
```

**Warp** — `~/.warp/.mcp.json`, or Settings → AI → MCP servers → + Add, which
writes it for you. An entry carries exactly one of `command` or `url` and Warp
rejects one holding both.

```json config path=~/.warp/.mcp.json
{
  "mcpServers": {
    "semlith": {
      "command": "semlith",
      "args": ["mcp"],
      "start_on_launch": true
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
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" },
      "start_on_launch": true
    }
  }
}
```

#### Editors

**GitHub Copilot in VS Code** — `.vscode/mcp.json` for a workspace, or the
profile copy that `MCP: Open User Configuration` opens. The root key is
`servers`, not `mcpServers`. The JSON below is the workspace file, which is
per-project, so it is not written for you; the CLI is what covers every
workspace. `code --add-mcp` has no scope flag and needs none — it saves into the
user profile's `mcp.json`, which every workspace sees
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
code --add-mcp '{"name":"semlith","command":"semlith","args":["mcp"]}'
```

**Cursor** — `~/.cursor/mcp.json` everywhere, or `.cursor/mcp.json` in one
repo. Cursor infers the transport from the presence of `url` and has no `type`
field of its own; adding one copied from another client confuses it.

```json config path=~/.cursor/mcp.json
{
  "mcpServers": {
    "semlith": {
      "command": "semlith",
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

**Windsurf** — `~/.codeium/windsurf/mcp_config.json`. The address key is
`serverUrl`, not `url`, and Windsurf reloads MCP servers only on a full
restart, not on a window reload.

```json config path=~/.codeium/windsurf/mcp_config.json
{
  "mcpServers": {
    "semlith": {
      "command": "/Users/you/.cargo/bin/semlith",
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
      "serverUrl": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

**Zed** — the `zed: open settings file` command. Zed calls MCP servers context
servers and keys them under `context_servers`. Its remote support has moved
between versions and older builds start local processes only, so this is the
stdio form, which needs no key: `semlith mcp` proxies to the running daemon.
Semlith cannot register Zed for you: it has no registration CLI, and Zed
documents no path for the settings file — it is whatever `zed: open settings
file` opens, which moves with the platform and the install. Run that command and
paste the stanza in.

```json
{
  "context_servers": {
    "semlith": {
      "source": "custom",
      "command": "/Users/you/.cargo/bin/semlith",
      "args": ["mcp"]
    }
  }
}
```

**JetBrains** — Junie reads `~/.junie/mcp/mcp.json`, or `.junie/mcp/mcp.json`
per project; AI Assistant takes the same JSON under Settings → Tools → AI
Assistant → Model Context Protocol. The transport is spelled `streamable-http`
with a hyphen.

```json config path=~/.junie/mcp/mcp.json
{
  "mcpServers": {
    "semlith": {
      "command": "/Users/you/.cargo/bin/semlith",
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
      "type": "streamable-http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" }
    }
  }
}
```

**Cline** — the MCP Servers panel, Configure. Cline's own documentation gives
two different paths for the file it writes, so let the panel open it rather
than guessing, and nothing here writes it for you. The transport is
`streamableHttp` in camel case; anything else falls back to SSE and the endpoint
answers 405. The CLI spells the verb `install`, not `add`, takes everything
after `--` as the subprocess command, and has no scope flag — it keeps one
configuration file, `~/.cline/mcp.json`
(<https://github.com/cline/cline/blob/main/apps/cli/README.md>,
<https://docs.cline.bot/mcp/mcp-overview>).

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

```sh register
cline mcp install semlith -- semlith mcp
```

**Roo Code** — `.roo/mcp.json` in the project, or the global file the MCP
Servers panel opens. Roo spells the same transport `streamable-http`, with the
hyphen, which is the one thing that does not copy across from a Cline config.
Semlith cannot register Roo Code for you: it has no registration CLI, and the
only file Roo documents by path is the project one, `.roo/mcp.json` — the global
copy has no documented location, it is only ever opened by the MCP Servers
panel. Open it from the panel and paste the stanza in.

```json
{
  "mcpServers": {
    "semlith": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" },
      "alwaysAllow": ["semlith_search"]
    }
  }
}
```

**Kilo Code** — `.kilocode/mcp.json` in the project, or the global file from
the MCP Servers panel. The `type` is required here: without it Kilo Code picks
SSE and the connection fails. The file below is the project one and is not
written for you. `kilo mcp add` has no global scope flag — its options are
`--url`, `--env` and `--header`, and `--global` exists only on `kilo plugin`
(<https://kilo.ai/docs/code-with-ai/platforms/cli-reference>) — so the
registration lands in the project configuration and semlith does not run it for
you. `~/.config/kilo/kilo.json` is the global file that covers every workspace,
and that is the one `semlith setup --register-all` writes.

```json
{
  "mcpServers": {
    "semlith": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:7365/mcp",
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" },
      "alwaysAllow": ["semlith_search"],
      "disabled": false
    }
  }
}
```

The global file uses the CLI's own schema instead — the root key is `mcp`, and
a subprocess entry is `type: "local"` with the command as an array:

```json config path=~/.config/kilo/kilo.json
{
  "mcp": {
    "semlith": {
      "type": "local",
      "command": ["semlith", "mcp"],
      "enabled": true
    }
  }
}
```

```sh register scope=project
kilo mcp add semlith -- semlith mcp
```

**Continue** — one block file per server at
`~/.continue/mcpServers/semlith.yaml`, or the same entry inlined in
`config.yaml`. The headers hang off `requestOptions`, not off the server
itself.

```yaml config path=~/.continue/mcpServers/semlith.yaml
name: Semlith
version: 0.0.1
schema: v1
mcpServers:
  - name: semlith
    command: /Users/you/.cargo/bin/semlith
    args:
      - mcp
```

Or against a daemon on another machine, over HTTP:

```yaml
name: Semlith
version: 0.0.1
schema: v1
mcpServers:
  - name: semlith
    type: streamable-http
    url: "http://127.0.0.1:7365/mcp"
    requestOptions:
      headers:
        Authorization: "Bearer ${SEMLITH_AGENT_KEY}"
```

**Kiro** — `.kiro/settings/mcp.json` in the workspace, or
`~/.kiro/settings/mcp.json` for every workspace. The workspace file wins where
both name the same server, so an old copy in the repository quietly overrides
the one you just edited — only the user-level file below is written for you.
`kiro-cli mcp add` takes `--scope workspace` or `--scope global`
(<https://kiro.dev/docs/reference/cli-commands/>).

```json config path=~/.kiro/settings/mcp.json
{
  "mcpServers": {
    "semlith": {
      "command": "semlith",
      "args": ["mcp"],
      "disabled": false,
      "autoApprove": ["semlith_search"]
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
      "headers": { "Authorization": "Bearer ${SEMLITH_AGENT_KEY}" },
      "disabled": false,
      "autoApprove": ["semlith_search"]
    }
  }
}
```

Replacing an existing entry — `remove` takes the same `--name` and `--scope`:

```sh unregister
kiro-cli mcp remove --name semlith --scope global
```

```sh register
kiro-cli mcp add --name semlith --command "semlith" --args "mcp" --scope global
```

#### Desktop apps

**LM Studio** — `~/.lmstudio/mcp.json`, reached from the Program tab → Install
→ Edit `mcp.json`. LM Studio follows Cursor's notation, so there is no
transport field and a `url` is enough.

```json config path=~/.lmstudio/mcp.json
{
  "mcpServers": {
    "semlith": {
      "command": "/Users/you/.cargo/bin/semlith",
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

**Claude Desktop** — `~/Library/Application Support/Claude/claude_desktop_config.json`
on macOS, `%APPDATA%\Claude\claude_desktop_config.json` on Windows. Settings →
Developer → Edit Config opens it. Desktop starts local processes only and has
nowhere to put a header, so this is the stdio form and needs no key: `semlith
mcp` proxies to the running daemon. Quit and reopen the app after editing.

```json config os=macos path="~/Library/Application Support/Claude/claude_desktop_config.json"
{
  "mcpServers": {
    "semlith": {
      "command": "/Users/you/.cargo/bin/semlith",
      "args": ["mcp"]
    }
  }
}
```

```json config os=windows path=%APPDATA%\Claude\claude_desktop_config.json
{
  "mcpServers": {
    "semlith": {
      "command": "semlith",
      "args": ["mcp"]
    }
  }
}
```

### Connecting over HTTP

`semlith start` also answers MCP at `http://127.0.0.1:7365/mcp`, so a client
that speaks the HTTP transport needs no subprocess and no path. The endpoint is
authenticated by the agent key in `~/.semlith/agent.key`, which is created on
first start and does not change when the daemon restarts, when Semlith is
upgraded, or when the portal's session token is rotated — so a stanza carrying
it is written once and keeps working. The key is 32 bytes from the OS random
source, written `sml_` and hex at mode 0600, and it is never reminted unless
you ask. `semlith key show` prints the live key and the stanza around it.

Rotating it carries it forward. `semlith key rotate`, and the portal's Rotate
key button, rewrite every client configuration file on this machine that
already held the old key — the documented path for each client above, plus the
project-scoped files beside wherever the daemon was started. A file is only
touched when the exact old key appears in it, nothing is ever created, and both
the command and the page list what they changed. A client configured somewhere
else still needs the new stanza pasted in.

```sh
claude mcp add --transport http semlith http://127.0.0.1:7365/mcp --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}"
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

`semlith key rotate` mints a new one. The previous key keeps working for
fifteen minutes, and never past the running daemon's exit, so a session already
open finishes rather than dying mid-call; `--now` drops it immediately. A client
that semlith registered through its own CLI is never affected: those
registrations are subprocess ones and hold no key. Every other client that holds
the old key is named so you know what to paste the new stanza into.

The stdio stanzas above stay exactly as they are: `semlith mcp` forwards to a
running daemon and falls back to opening the stores itself when none is
running, which is what makes it work whether or not `semlith start` is up. The
HTTP endpoint is for clients that would rather hold a URL than spawn a process.
Close it with `semlith start --no-mcp-http`, or from the portal's Agents page
while the daemon runs — closing it drops the route, not the daemon.
