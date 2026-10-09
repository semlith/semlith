"""Serena (oraios): language servers exposed as MCP tools; no natural-language
search. Spoken to as an MCP server over stdio. For each identifier-like term in
the issue (in order of appearance), find_symbol with its name path
(dotted.name -> dotted/name); symbol hits come first. Then search_for_pattern
with the identifiers OR-ed (all identifiers' escaped regex), files ranked by
matching-line count, appended after the symbol hits. With no identifier in the
issue, the pattern search uses its 4+-letter non-stopwords instead.
SERENA_HOME is tools/serena-home, whose serena_config.yml puts each project's
.serena folder at tools/work/serena/<repo folder name>/.serena, not in the repo."""

import atexit
import json
import os
import re
import subprocess
import threading

from . import _common as c

NAME = "Serena"
BIN = f"{c.TOOLS}/venv-serena/bin/serena"
HOME = f"{c.TOOLS}/serena-home"
VERSION = "1.7.0"
MAX_TERMS = 15
_servers = {}


def _env():
    return c.env(SERENA_HOME=HOME)


def available():
    if not os.access(BIN, os.X_OK):
        return f"serena not installed at {BIN}"
    if not os.path.exists(f"{HOME}/serena_config.yml"):
        return f"missing {HOME}/serena_config.yml (redirects the per-project .serena folder)"
    return None


def version():
    return c.first_line([BIN, "-V"]).split()[-1]


class _Mcp:
    def __init__(self, root):
        self.p = subprocess.Popen(
            [BIN, "start-mcp-server", "--project", root, "--transport", "stdio", "--context", "agent",
             "--enable-web-dashboard", "false", "--enable-gui-log-window", "false", "--log-level", "WARNING"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=_env(), text=True)
        self.n = 0
        self.call_raw("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                     "clientInfo": {"name": "semlith-scorecard", "version": "0"}})
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def _send(self, m):
        self.p.stdin.write(json.dumps(m) + "\n")
        self.p.stdin.flush()

    def call_raw(self, method, params, timeout=c.SEARCH_TIMEOUT):
        self.n += 1
        i, box = self.n, {}
        self._send({"jsonrpc": "2.0", "id": i, "method": method, "params": params})

        def read():
            for line in self.p.stdout:
                m = json.loads(line)
                if m.get("id") == i:
                    box["m"] = m
                    return
        t = threading.Thread(target=read, daemon=True)
        t.start()
        t.join(timeout)
        if "m" not in box:
            self.close()
            raise TimeoutError(f"serena {method} gave no answer in {timeout}s")
        if "error" in box["m"]:
            raise RuntimeError(f"serena {method}: {box['m']['error']}")
        return box["m"]["result"]

    def tool(self, name, args):
        r = self.call_raw("tools/call", {"name": name, "arguments": args})
        text = r["content"][0]["text"] if r.get("content") else ""
        if r.get("isError"):
            return None
        try:
            return json.loads(text)
        except ValueError:
            return None  # e.g. "answer too long" or a plain-text notice

    def close(self):
        if self.p.poll() is None:
            self.p.kill()
            self.p.wait()


def _server(root):
    s = _servers.get(root)
    if s is None or s.p.poll() is not None:
        s = _servers[root] = _Mcp(root)
    return s


@atexit.register
def _close_all():
    for s in _servers.values():
        s.close()


def index(root, work):
    # `serena project index` fills the LSP symbol cache; files whose content is
    # unchanged are served from the cache on the next run.
    s = _servers.pop(root, None)
    if s:
        s.close()  # the server's language server must see the new checkout
    c.run([BIN, "project", "index", root, "--log-level", "WARNING"], cwd=root, env_=_env(),
          timeout=c.INDEX_TIMEOUT)


def search(query, root, work, k=50):
    s = _server(root)
    ids = c.identifiers(query)[:MAX_TERMS]
    res, seen = [], set()
    for t in ids:
        for sym in s.tool("find_symbol", {"name_path_pattern": t.replace(".", "/")}) or []:
            loc = sym.get("body_location") or {}
            key = (sym["relative_path"], loc.get("start_line"))
            if key not in seen:
                seen.add(key)
                res.append((sym["relative_path"], loc.get("start_line", 0) + 1, loc.get("end_line", 0) + 1,
                            sym["name_path"]))
    terms = ids or c.words(query)[:MAX_TERMS]
    if terms:
        hits = s.tool("search_for_pattern", {"substring_pattern": "|".join(re.escape(t) for t in terms),
                                             "max_answer_chars": 5_000_000}) or {}
        files = {p for p, *_ in res}
        for path, lines in sorted(hits.items(), key=lambda kv: -len(kv[1])):
            if path in files:
                continue
            nums = [int(m.group(1)) for m in (re.match(r"\s*>?\s*(\d+):", l) for l in lines) if m]
            if nums:
                res.append((path, min(nums), max(nums), "\n".join(lines)[:4000]))
    return res[:k]
