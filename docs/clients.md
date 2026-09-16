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

### Setting it up in your client

No snippet below names a store. Most of them hand the client the HTTP endpoint
`semlith start` serves; the rest run `semlith mcp`, which proxies to the same
daemon. Either way the server opens every store registered in
`~/.semlith/registry.json` — which is every store `semlith index` has made —
plus a `.semlith` beside the directory the agent was started in, if there is
one. Index another repository and the agent that is already configured can
search it, with no edit to any of these files.

Every stanza below names `${SEMLITH_AGENT_KEY}` rather than the key itself. The
client expands it from the environment at start, and `semlith setup` writes a
block in your shell startup file that exports it by reading
`~/.semlith/agent.key` — so a rotation needs no file here rewritten, and no
configuration file on the machine carries the credential. `semlith key show`
prints the key if you would rather paste the literal value; the next section
explains where it comes from and how to rotate it. The endpoint answers a
request without that header with 401 and nothing else, so a stanza that drops it
fails to connect rather than failing to find anything.

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
root. `--transport http` points it at the running daemon; without it, the `--`
matters, because Claude Code reads anything starting with a dash as one of its
own flags.

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

```sh
claude mcp add semlith -- semlith mcp
```

**OpenAI Codex** — `~/.codex/config.toml`, shared by the CLI, the IDE extension
and the desktop app. TOML, and the table is `mcp_servers` with an underscore. A
table with a `url` in it is a streamable-HTTP server; the header goes in an
inline `http_headers` table. `codex mcp add` writes the same entry, but it has
no flag for an arbitrary header: it takes the key from an environment variable
instead, so `export SEMLITH_KEY="$SEMLITH_AGENT_KEY"` has to live in your shell profile
rather than in the one command —
`codex mcp add semlith --url "http://127.0.0.1:7365/mcp" --bearer-token-env-var SEMLITH_KEY`.

```toml
[mcp_servers.semlith]
url = "http://127.0.0.1:7365/mcp"
http_headers = { Authorization = "Bearer ${SEMLITH_AGENT_KEY}" }
```

**OpenCode** — `opencode.json` in the project root, or the same file under
`~/.config/opencode/`. The root key is `mcp`, the transport is spelled
`remote`, and an entry is ignored until `enabled` is true.

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

```sh
opencode mcp add semlith --url http://127.0.0.1:7365/mcp --header "Authorization=Bearer ${SEMLITH_AGENT_KEY}"
```

**IO CLI** — `io.local.toml` in the project root. The servers are an array of
tables rather than a map, so each one is its own `[[mcp]]` block and the name
is a field inside it.

```toml
[[mcp]]
id = "semlith"
url = "http://127.0.0.1:7365/mcp"
headers = { Authorization = "Bearer ${SEMLITH_AGENT_KEY}" }
```

```sh
io mcp add semlith --url http://127.0.0.1:7365/mcp --header 'Authorization=Bearer ${SEMLITH_AGENT_KEY}'
```

**GitHub Copilot CLI** — `~/.copilot/mcp-config.json`, or `/mcp add` in a
session. Its name for a subprocess is `local` rather than `stdio`, but a
remote server is the ordinary `http`.

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

**Gemini CLI** — `~/.gemini/settings.json`. The key for a streamable-HTTP
server is `httpUrl`, not `url`; a plain `url` is read as the older SSE
transport and the connection fails.

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

**Qwen Code** — `~/.qwen/settings.json`, the same schema as Gemini CLI down to
`httpUrl` winning over `url` when both are present.

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

**Amp** — `~/.config/amp/settings.json`, or the editor extension's own
`settings.json`. The root key is the dotted string `amp.mcpServers`, which
means the entry lives inside a wider settings file rather than in one of its
own.

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

```sh
amp mcp add semlith -- semlith mcp
```

**Crush** — `crush.json` in the project root. The root key is `mcp`, not
`mcpServers`, and the TUI reads the file once at start, so relaunch it after
editing.

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
one repository. `droid mcp add semlith http://127.0.0.1:7365/mcp --type http
--header "Authorization: Bearer ${SEMLITH_AGENT_KEY}"` writes the same object.

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

**Goose** — `goose configure` → Add Extension → Remote Extension, or
`~/.config/goose/config.yaml`. Goose calls them extensions, spells the
transport `streamable_http` with an underscore, and takes the address as `uri`
rather than `url`.

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
form, which needs no key: `semlith mcp` proxies to the running daemon.

```json
{
  "mcpServers": {
    "semlith": {
      "command": "semlith",
      "args": ["mcp"]
    }
  }
}
```

```sh
q mcp add --name semlith --command semlith --args mcp
```

**OpenClaw** — `~/.openclaw/openclaw.json`. The servers are nested two deep
under `mcp` then `servers`, and the transport field is called `transport`, not
`type`.

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

**DeepSeek** — `~/.deepseek/mcp.json`, read by DeepSeek-TUI, which has since
renamed itself Codewhale and now looks in `~/.codewhale/mcp.json` first and
falls back to the old path. Either file takes `servers` or `mcpServers` as the
root key, and needs no transport field: a `url` is enough. `codewhale mcp add`
has no header flag, so the key goes in the environment:
`export SEMLITH_KEY="$SEMLITH_AGENT_KEY"`, then
`codewhale mcp add semlith --url "http://127.0.0.1:7365/mcp" --bearer-token-env-var SEMLITH_KEY`.

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

**Warp** — `~/.warp/.mcp.json`, or Settings → AI → MCP servers → + Add, which
writes it for you. An entry carries exactly one of `command` or `url` and Warp
rejects one holding both.

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
`servers`, not `mcpServers`.

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

**Cursor** — `~/.cursor/mcp.json` everywhere, or `.cursor/mcp.json` in one
repo. Cursor infers the transport from the presence of `url` and has no `type`
field of its own; adding one copied from another client confuses it.

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
than guessing. The transport is `streamableHttp` in camel case; anything else
falls back to SSE and the endpoint answers 405.

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

```sh
cline mcp add semlith --transport http --header "Authorization: Bearer ${SEMLITH_AGENT_KEY}" http://127.0.0.1:7365/mcp --yes
```

**Roo Code** — `.roo/mcp.json` in the project, or the global file the MCP
Servers panel opens. Roo spells the same transport `streamable-http`, with the
hyphen, which is the one thing that does not copy across from a Cline config.

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
SSE and the connection fails.

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

```sh
kilo mcp add semlith --url http://127.0.0.1:7365/mcp --header "Authorization=Bearer ${SEMLITH_AGENT_KEY}"
```

**Continue** — one block file per server at
`~/.continue/mcpServers/semlith.yaml`, or the same entry inlined in
`config.yaml`. The headers hang off `requestOptions`, not off the server
itself.

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
the one you just edited.

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

```sh
kiro-cli mcp add --name semlith --command "semlith" --args "mcp" --scope global
```

#### Desktop apps

**LM Studio** — `~/.lmstudio/mcp.json`, reached from the Program tab → Install
→ Edit `mcp.json`. LM Studio follows Cursor's notation, so there is no
transport field and a `url` is enough.

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

```json
{
  "mcpServers": {
    "semlith": {
      "command": "/Users/you/.cargo/bin/semlith",
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
open finishes rather than dying mid-call; `--now` drops it immediately. Claude Code is re-registered through its
own CLI, because it is the one client Semlith has a supported way to write a
config for, and every other client that holds the old key is named so you know
what to paste the new stanza into.

The stdio stanzas above stay exactly as they are: `semlith mcp` forwards to a
running daemon and falls back to opening the stores itself when none is
running, which is what makes it work whether or not `semlith start` is up. The
HTTP endpoint is for clients that would rather hold a URL than spawn a process.
Close it with `semlith start --no-mcp-http`, or from the portal's Agents page
while the daemon runs — closing it drops the route, not the daemon.
