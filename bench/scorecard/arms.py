"""The arms every benchmark shares. Each returns [Excerpt] in the order an agent would read them.

`semlith` is `semlith search --json` against one store, rendered the way the
harness counts tokens for every arm: a `== path:start-end` header, then the
text. `r0` is the grep-then-read baseline of the 2026-09-30 accuracy benchmark:
one `rg` pass counts the query's terms per file, BM25 ranks the files, and the
files are then read whole in rank order, which is what an agent that greps and
reads pays.
"""
import json, math, os, re, subprocess
from collections import defaultdict

from common import SEMLITH, Excerpt, sh

STOP = set("""a about above after again against all also an and any are as at be because been before being
below between both but by can could did do does doing done down during each few for from further had has have
having how however if in into is it its itself just like make makes more most much must no nor not now of off
on once only or other our out over own same should so some such than that the their them then there these they
this those through to too under until up upon use used uses using very was way we were what when where which
while who whom why will with within without would you your called call calls code file files function functions
method methods defined define defines implemented implement happens work works find show tell""".split())
IDENTISH = re.compile(r"`([^`]+)`|\b([A-Za-z_][\w]*(?:(?:::|\.)[\w]+)+|[a-z]+_[\w]+|[A-Za-z]*[a-z][A-Z]\w*|[A-Z]{2,}\w*)\b")
MAX_TERMS = 40


def render(path, start, end, text):
    return f"== {path}:{start}-{end}\n{text}\n"


def collect(items):
    """[(path, start, end, text)] -> [Excerpt] with byte offsets of each header."""
    out, size = [], 0
    for path, start, end, text in items:
        out.append(Excerpt(path, start, end, text, size))
        size += len(render(path, start, end, text).encode("utf-8"))
    return out


def semlith(store, query, k=50, paths=()):
    args = [SEMLITH, "search", "--store", store, "--json", "-k", str(k)]
    for p in paths:
        args += ["--path", p]
    rows = json.loads(sh(args + ["--", query], timeout=600).stdout or "[]")
    return collect((r["path"], r["start_line"], r["end_line"], r.get("text", "")) for r in rows)


def terms(query):
    """[(term, weight)]: identifier-like 3, other words of 4+ letters 1, stopwords dropped, first MAX_TERMS."""
    out, seen = [], set()
    for m in IDENTISH.finditer(query):
        t = (m.group(1) or m.group(2)).strip()
        if t and len(t) < 80 and t.lower() not in seen:
            out.append((t, 3)); seen.add(t.lower())
    for w in re.findall(r"[A-Za-z]{4,}", query):
        if w.lower() not in seen and w.lower() not in STOP:
            out.append((w.lower(), 1)); seen.add(w.lower())
    return out[:MAX_TERMS]


def bm25_files(query, root, files):
    """Files ranked by BM25 over one `rg` pass (k1 1.2, b 0.75, identifier weight 3)."""
    ts = terms(query)
    if not ts or not files:
        return []
    weight = {t.lower(): w for t, w in ts}
    hits = defaultdict(lambda: defaultdict(int))
    args = ["rg", "-F", "-i", "-o", "--no-heading", "--no-messages", "-n", "--with-filename"] + sum((["-e", t] for t, _ in ts), [])
    for i in range(0, len(files), 1000):
        r = subprocess.run(args + ["--", *files[i:i + 1000]], cwd=root, capture_output=True, text=True, errors="replace")
        for line in r.stdout.splitlines():
            parts = line.split(":", 2)
            if len(parts) != 3 or not parts[1].isdigit():
                continue
            hits[parts[0]][parts[2].lower()] += 1
    sizes = {f: os.path.getsize(os.path.join(root, f)) for f in hits}
    avg = max(sum(os.path.getsize(os.path.join(root, f)) for f in files if os.path.exists(os.path.join(root, f))) / len(files), 1.0)
    df = defaultdict(int)
    for p in hits:
        for t in hits[p]:
            df[t] += 1
    n = len(files)

    def score(p):
        norm = 1.2 * (0.25 + 0.75 * sizes[p] / avg)
        return sum(weight.get(t, 0) * math.log(1 + (n - df[t] + 0.5) / (df[t] + 0.5)) * c * 2.2 / (c + norm)
                   for t, c in hits[p].items())

    return sorted(hits, key=lambda p: (-score(p), p))


def r0(query, root, files, max_files=20):
    """Grep, then read each ranked file whole."""
    items = []
    for f in bm25_files(query, root, files)[:max_files]:
        with open(os.path.join(root, f), errors="replace") as fh:
            text = fh.read()
        items.append((f, 1, text.count("\n") + 1, text))
    return collect(items)


def store_files(store):
    return [l for l in sh([SEMLITH, "files", "--store", store]).stdout.splitlines() if l.strip()]


class SemlithMCP:
    """`semlith_search` through one long-lived `semlith mcp --store` process, as an agent calls it.

    The CLI loads the model on every call (about 0.6 s); this pays it once, so a benchmark of tens
    of thousands of queries is minutes, not a day. Returns [Excerpt] with offsets into the tool's
    own text answer.
    """

    def __init__(self, store):
        self.proc = subprocess.Popen([SEMLITH, "mcp", "--store", store], stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
        self.next = 0
        self._rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                 "clientInfo": {"name": "semlith-scorecard", "version": "1"}})
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def _send(self, msg):
        self.proc.stdin.write(json.dumps(msg) + "\n")
        self.proc.stdin.flush()

    def _rpc(self, method, params):
        self.next += 1
        self._send({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params})
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise RuntimeError("semlith mcp exited")
            msg = json.loads(line)
            if msg.get("id") == self.next:
                if "error" in msg:
                    raise RuntimeError(msg["error"])
                return msg["result"]

    def search(self, query, k=50, paths=()):
        args = {"query": query, "k": k}
        if paths:
            args["path"] = list(paths)
        result = self._rpc("tools/call", {"name": "semlith_search", "arguments": args})
        raw = "".join(c.get("text", "") for c in result.get("content", []))
        return parse_search(raw)

    def close(self):
        self.proc.kill()


SPAN = re.compile(r"^\s+(\d+)-(\d+)\s")


def parse_search(raw):
    """semlith_search's text answer: a path line, then its indented `start-end` span lines."""
    out, path, offset = [], None, 0
    for line in raw.splitlines(keepends=True):
        m = SPAN.match(line)
        if m and path:
            out.append(Excerpt(path, int(m.group(1)), int(m.group(2)), line.strip(), offset))
        elif line.strip() and not line.startswith(" ") and ("/" in line or os.sep in line) and " · " not in line:
            path = line.strip()
        offset += len(line.encode("utf-8"))
    return out
