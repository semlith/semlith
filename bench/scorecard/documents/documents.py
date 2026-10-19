#!/usr/bin/env python3
"""Document formats: does semlith find passages inside PDF, Office, OpenDocument, EPUB, notebooks and CSV?

    python3 bench/scorecard/documents/documents.py fetch [--from DIR]   the documents of manifest.jsonl, each checked against its SHA-256
    python3 bench/scorecard/documents/documents.py index          one store of them, in a scratch semlith home
    python3 bench/scorecard/documents/documents.py run [arms]     r0 (rg), r0x (rga), s1 (semlith_search), s2 (semlith_brief), RUNS runs each
    python3 bench/scorecard/documents/documents.py score <run dir>   -> bench/scorecard/documents/runs/<run id>/

The method of the 2026-10-01 document-formats round, ported. The 127 questions and the 159 gold passages they
point at (`questions.yaml`, `targets.json`) are frozen: the passages are the chunks semlith 0.34.0 extracted, and
the harness refuses to run if either file's SHA-256 differs from the one pinned here.

Line numbers are not comparable between extractors (semlith's PDF reader against poppler, pandoc), so a passage
counts as found when a delivered excerpt is in the gold file and shares a run of 6 words with the gold passage
(NFKC, case-folded, words only). The same rule applies to every arm. "File" is the gold file named at all.

Everything downloaded, indexed or written goes under SCORECARD_HOME/documents. semlith runs with SEMLITH_HOME set
to SCORECARD_HOME/documents/home, so it never opens the user's own store home, registry or daemon; SEMLITH_ACCEL=ane
and SEMLITH_LEDGER=0. Only the score files and their MANIFEST.json are written into this repository.
"""
import glob, hashlib, json, math, os, re, shutil, sqlite3, subprocess, sys, time, unicodedata
from collections import Counter, defaultdict, deque

import yaml

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
from common import BUDGETS, BYTES_PER_TOKEN, HOME, RUNS, SEMLITH, median_spread, run_dir, sh, write_manifest
import arms

DOCS = os.path.join(HOME, "documents")
ROOT = os.path.join(DOCS, "files")
SEMLITH_HOME = os.path.join(DOCS, "home")
STORE = "documents"
STORE_DIR = os.path.join(SEMLITH_HOME, "stores", STORE)
# Every semlith this process starts, the agent round's included, sees only the scratch home.
os.environ.update({"SEMLITH_HOME": SEMLITH_HOME, "SEMLITH_ACCEL": "ane", "SEMLITH_LEDGER": "0"})

FROZEN = {"questions.yaml": open(os.path.join(HERE, "FROZEN.sha256")).read().split()[0],
          "targets.json": "46d4fe7d73c45b7c332dfbe81fe7de5f1062b0cc7eb218bc89eb490f764928cb"}
MAX_TOKENS = 8000
NGRAM = 6


def sha256(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def frozen():
    for name, want in FROZEN.items():
        got = sha256(os.path.join(HERE, name))
        if got != want:
            raise SystemExit(f"{name} is frozen at {want} and reads {got}: the questions changed, so no result is comparable")


def questions():
    frozen()
    return yaml.safe_load(open(os.path.join(HERE, "questions.yaml")))["questions"]


def gold_text():
    return {t["id"]: t["text"] for t in json.load(open(os.path.join(HERE, "targets.json")))}


def manifest():
    """The kept documents; a rejected fetch of 2026-10-01 has a row too, with no hash."""
    return [r for r in map(json.loads, open(os.path.join(HERE, "manifest.jsonl"))) if "rejected" not in r]


def fmt(path):
    return os.path.splitext(path)[1].lower()


def relp(path):
    return path[len(ROOT) + 1:] if path.startswith(ROOT + "/") else path


def fetch(mirror=None):
    """Each document from its publisher's URL. A file whose bytes no longer match the pinned SHA-256 is kept and
    reported, in SCORECARD_HOME/documents/fetch.json and every run's MANIFEST, never swapped for another.

    `mirror` is an earlier fetch laid out the same way (`<format>/<filename>`): a copy there whose SHA-256 is the
    pinned one is used before the URL. Gutenberg rebuilds its EPUBs monthly, CISA's catalogue changes daily and
    NIST builds its spreadsheet on request, so only a kept copy gives those seven their pinned bytes."""
    report = {}
    for d in manifest():
        out = os.path.join(ROOT, d["format"], d["filename"])
        kept = os.path.join(mirror, d["format"], d["filename"]) if mirror else None
        if not (os.path.exists(out) and sha256(out) == d["sha256"]) and kept and os.path.exists(kept) \
                and sha256(kept) == d["sha256"]:
            os.makedirs(os.path.dirname(out), exist_ok=True)
            shutil.copyfile(kept, out)
        if not (os.path.exists(out) and sha256(out) == d["sha256"]):
            os.makedirs(os.path.dirname(out), exist_ok=True)
            # lists.apache.org takes about five minutes to build a month's mbox (and once served a short one);
            # open.canada.ca's firewall rejects browser-like agents and accepts curl's own.
            agent = [] if "open.canada.ca" in d["url"] else ["-A", "Mozilla/5.0 (Macintosh) semlith-bench/1.0"]
            r = subprocess.run(["curl", "-sSfL", "--max-time", "600", *agent, "-o", out + ".part", d["url"]],
                               capture_output=True, text=True)
            if r.returncode == 0:
                os.replace(out + ".part", out)
            time.sleep(1)
        got = sha256(out) if os.path.exists(out) else None
        report[d["filename"]] = {"ok": got == d["sha256"], "sha256": got, "pinned": d["sha256"], "url": d["url"],
                                 "bytes": os.path.getsize(out) if got else None, "pinned_bytes": d["bytes"]}
        print(("ok       " if got == d["sha256"] else "MISMATCH " if got else "MISSING  ") + f"{d['format']}/{d['filename']}", flush=True)
    with open(os.path.join(DOCS, "fetch.json"), "w") as f:
        json.dump(report, f, indent=1)
    bad = {k: v for k, v in report.items() if not v["ok"]}
    print(f"{len(report) - len(bad)}/{len(report)} match their pinned SHA-256"
          + (f"; differ or missing: {', '.join(sorted(bad))}" if bad else ""))


def fetch_status():
    try:
        return {k: v for k, v in json.load(open(os.path.join(DOCS, "fetch.json"))).items() if not v["ok"]}
    except OSError:
        return "fetch.json missing: run `documents.py fetch`"


def index():
    t = time.time()
    sh([SEMLITH, "index", "--name", STORE, "-q", "--no-review", ROOT], timeout=6 * 3600)
    chunks, vectors = arms.embedded(STORE_DIR)
    print(f"indexed {ROOT} -> {STORE_DIR} in {time.time() - t:.0f}s: {chunks} chunks, {vectors} vectors")


# --- the arms -------------------------------------------------------------------------------------------------

PAGE = re.compile(r"^Page \d+: ")


def files():
    return sorted(p for p in glob.glob(os.path.join(ROOT, "**", "*"), recursive=True) if os.path.isfile(p))


def text_len(engine, path, cache={}):
    """Length of what the engine can read in a file: rg reads raw bytes, rga reads its adapter's text."""
    if (engine, path) not in cache:
        if engine == "rg":
            cache[(engine, path)] = os.path.getsize(path)
        else:
            r = subprocess.run(["rga", "--no-heading", "-N", "--color", "never", "", path], capture_output=True, text=True, errors="replace")
            cache[(engine, path)] = max(len(r.stdout), 1)
    return cache[(engine, path)]


def bm25_grep(engine, query):
    """R0 over one engine: one pass for term counts, BM25 by the engine's own text length, ±3-line blocks to the budget.

    The code benchmarks' BM25 (`arms.bm25_files`) with two differences that keep it the 2026-10-01 method: a file's
    length is the text the engine reads (rga's adapter output, not the file's bytes), and rga's `Page N: ` prefix is
    dropped, so a term counts the same on every page."""
    ts = arms.terms(query)
    fs = files()
    if not ts:
        return "", [], 0.0
    t0 = time.perf_counter()
    weight = {t.lower(): w for t, w in ts}
    pats = sum((["-e", t] for t, _ in ts), [])
    hits = defaultdict(lambda: defaultdict(int))
    r = subprocess.run([engine, "-F", "-i", "-o", "--no-heading", "--no-messages", "-n", "--with-filename", "--color", "never", *pats, "--", *fs],
                       capture_output=True, text=True, errors="replace")
    for line in r.stdout.splitlines():
        parts = line.split(":", 2)
        if len(parts) != 3 or not parts[1].isdigit():
            continue
        hits[parts[0]][PAGE.sub("", parts[2]).lower()] += 1
    lens = {p: text_len(engine, p) for p in fs}
    avg = sum(lens.values()) / len(lens)
    df = Counter(t for p in hits for t in hits[p])
    n = len(fs)

    def bm25(p):
        norm = 1.2 * (0.25 + 0.75 * lens.get(p, avg) / avg)
        return sum(weight.get(t, 0) * math.log(1 + (n - df[t] + 0.5) / (df[t] + 0.5)) * c * 2.2 / (c + norm) for t, c in hits[p].items())

    raw, out, size, limit = [], [], 0, MAX_TOKENS * BYTES_PER_TOKEN
    for path in sorted(hits, key=lambda p: (-bm25(p), p)):
        r = subprocess.run([engine, "-F", "-i", "-n", "-C3", "--no-heading", "--no-messages", "--color", "never", *pats, "--", path],
                           capture_output=True, text=True, errors="replace")
        for block in r.stdout.split("\n--\n"):
            rows = [m for m in (re.match(r"^(\d+)[:-](.*)$", l) for l in block.splitlines()) if m]
            if not rows:
                continue
            text = "\n".join(PAGE.sub("", m.group(2)) for m in rows)
            out.append([path, int(rows[0].group(1)), int(rows[-1].group(1)), size, text[:4000]])
            chunk = f"== {path}:{rows[0].group(1)}-{rows[-1].group(1)}\n{text}\n"
            raw.append(chunk)
            size += len(chunk.encode("utf-8"))
            if size >= limit:
                return "".join(raw), out, (time.perf_counter() - t0) * 1000
    return "".join(raw), out, (time.perf_counter() - t0) * 1000


SEARCH_HEAD = re.compile(r"^\[\d+\] (?:\S+ )?(.+):(\d+)-(\d+) \(")
# A store label (`[name] `) leads only when the server holds several stores; the text below is indented four spaces.
BRIEF_HEAD = re.compile(r"^(?:\[\S+\] )?(\S.*?):(\d+)-(\d+)(?: |$)")
LOCATION_ONLY = "(text: top span only)"


def excerpts(raw, head):
    """[path, start, end, byte offset of the header, text] for each header line matching `head`; the text is what
    follows until the next one. A path printed relative is resolved against the answer's `root` line."""
    first = raw.split("\n", 1)[0]
    root = first[len("root "):].strip() if first.startswith("root ") else ""
    out, at = [], 0
    for line in raw.split("\n"):
        m = head.match(line)
        if m:
            path = m.group(1) if os.path.isabs(m.group(1)) or not root else os.path.join(root, m.group(1))
            out.append([path, int(m.group(2)), int(m.group(3)), at, []])
        elif out:
            out[-1][4].append(line)
        at += len(line.encode("utf-8")) + 1
    for e in out:
        text = "\n".join(e[4]).strip("\n")
        e[4] = "" if text.strip() == LOCATION_ONLY else text[:4000]
    return out


def semlith(mcp, tool, q):
    """One call through the long-lived `semlith mcp`, as an agent makes it."""
    if tool == "s1":
        name, args, head = "semlith_search", {"query": q["query"], "k": 50, "format": "excerpt"}, SEARCH_HEAD
    else:
        name, args, head = "semlith_brief", {"question": q["query"], "budget": MAX_TOKENS}, BRIEF_HEAD
    t = time.perf_counter()
    result = mcp._rpc("tools/call", {"name": name, "arguments": args})
    ms = (time.perf_counter() - t) * 1000
    raw = "".join(c.get("text", "") for c in result.get("content", []))
    if result.get("isError"):
        return "ERROR: " + raw, [], ms
    return raw, excerpts(raw, head), ms


def run(names):
    """Every question through every arm, RUNS times, into a new run directory under SCORECARD_HOME/results/documents."""
    qs = questions()
    out = run_dir("documents")
    mcp = arms.SemlithMCP(STORE_DIR) if {"s1", "s2"} & set(names) else None
    call = {"r0": lambda q: bm25_grep("rg", q["query"]), "r0x": lambda q: bm25_grep("rga", q["query"]),
            "s1": lambda q: semlith(mcp, "s1", q), "s2": lambda q: semlith(mcp, "s2", q)}
    t0 = time.time()
    try:
        for r in range(RUNS):
            for arm in names:
                with open(os.path.join(out, f"{arm}-r{r}.jsonl"), "w") as f:
                    for q in qs:
                        try:
                            raw, ex, ms = call[arm](q)
                        except Exception as e:
                            raw, ex, ms = f"ERROR: {e}", [], None
                        f.write(json.dumps({"id": q["id"], "arm": arm, "run": r, "ms": ms, "raw": raw, "excerpts": ex}) + "\n")
                print(f"{arm} r{r} done ({time.time() - t0:.0f}s)", flush=True)
    finally:
        if mcp:
            mcp.close()
    tools = {}
    for t, flag in (("rg", "--version"), ("rga", "--version"), ("pdftotext", "-v"), ("pandoc", "--version")):
        r = sh([t, flag], check=False)
        tools[t] = (r.stdout or r.stderr).splitlines()[0]  # pdftotext prints its version to stderr
    write_manifest(out, {"bench": "documents", "arms": names, "runs": RUNS, "store": STORE_DIR, "root": ROOT,
                         "store_stats": sh([SEMLITH, "stats", "--store", STORE_DIR], check=False).stdout,
                         "tools": tools, "fetch_mismatches": fetch_status(), "seconds": round(time.time() - t0)})
    print(out)


# --- scoring --------------------------------------------------------------------------------------------------

def words(text):
    return [w.casefold() for w in re.findall(r"\w+", unicodedata.normalize("NFKC", text or ""))]


def shingles(text):
    w = words(text)
    return {tuple(w[i:i + NGRAM]) for i in range(len(w) - NGRAM + 1)}


def found(excerpt, gold_path, gold):
    return relp(excerpt[0]).endswith(gold_path) and bool(shingles(excerpt[4]) & gold)


def in_document(path, runs):
    """Whether any 6-word run in `runs` (a shingle set) is in the text semlith extracted from `path`, root-relative.
    None when the store holds no such document. Streamed chunk by chunk: a workbook's text is tens of megabytes."""
    db = sqlite3.connect(f"file:{os.path.join(STORE_DIR, 'store.db')}?mode=ro", uri=True)
    window, seen = deque(maxlen=NGRAM), False
    for (text,) in db.execute("SELECT c.text FROM chunks c JOIN files f ON f.id = c.file_id WHERE f.path = ? ORDER BY c.ord",
                              (os.path.join(ROOT, path),)):
        seen = True
        for w in words(text):
            window.append(w)
            if len(window) == NGRAM and tuple(window) in runs:
                return True
    return False if seen else None


def score(raw_dir):
    """Per-question rows and per-arm medians of the runs -> bench/scorecard/documents/runs/<run id>/."""
    run_manifest = json.load(open(os.path.join(raw_dir, "MANIFEST.json")))  # a finished run, before anything is written
    qs = {q["id"]: q for q in questions()}
    gold = {i: shingles(t) for i, t in gold_text().items()}
    rows = []
    for path in sorted(glob.glob(os.path.join(raw_dir, "*-r*.jsonl"))):
        for line in open(path):
            r = json.loads(line)
            q = qs[r["id"]]
            row = {"id": q["id"], "arm": r["arm"], "run": r["run"], "shape": q["shape"], "ext": fmt(q["path"]),
                   "error": r["raw"].startswith("ERROR")}
            for b in BUDGETS:
                d = [e for e in r["excerpts"] if e[3] < b * BYTES_PER_TOKEN]
                row[f"hit{b}"] = int(any(found(e, q["path"], gold[q["target"]]) for e in d))
                row[f"file{b}"] = int(any(relp(e[0]).endswith(q["path"]) for e in d))
            first = next((e for e in r["excerpts"] if found(e, q["path"], gold[q["target"]])), None)
            row["first_tokens"] = None if first is None else first[3] // BYTES_PER_TOKEN
            row["ms"] = r["ms"]
            rows.append(row)
    table = []
    for arm in ("r0", "r0x", "s1", "s2"):
        runs = sorted({r["run"] for r in rows if r["arm"] == arm})
        if not runs:
            continue
        per = [[r for r in rows if r["arm"] == arm and r["run"] == k] for k in runs]
        row = {"arm": arm, "runs": len(runs), "n": len(per[0]), "errors": sum(r["error"] for p in per for r in p)}
        for key in [f"hit{b}" for b in BUDGETS] + ["file4000"]:
            row[key], row[key + "_spread"] = median_spread([sum(r[key] for r in p) for p in per])
        ft = sorted(r["first_tokens"] for r in per[0] if r["first_tokens"] is not None)
        ms = sorted(r["ms"] for p in per for r in p if r["ms"] is not None)
        row["first_tokens_median"] = ft[len(ft) // 2] if ft else None
        row["ms_median"] = round(ms[len(ms) // 2]) if ms else None
        row["by_ext"] = dict(sorted(Counter(r["ext"] for r in per[0] if r["hit4000"]).items()))
        table.append(row)
    out = os.path.join(HERE, "runs", os.path.basename(os.path.normpath(raw_dir)))
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "scores.jsonl"), "w") as f:
        for row in rows:
            f.write(json.dumps(row) + "\n")
    with open(os.path.join(out, "score.json"), "w") as f:
        json.dump(table, f, indent=1)
    # A refetched document can lose a frozen passage (Gutenberg reconverts its EPUBs monthly); such a question
    # cannot be found by any arm, so the run says which ones.
    lost = sorted(i for i, q in qs.items() if not in_document(q["path"], gold[q["target"]]))
    # The binary and command that produced the raw output, not whichever one scores it.
    write_manifest(out, {"bench": "documents", "questions": len(qs), "gold_not_in_store": lost, **{k: run_manifest[k] for k in
                   ("semlith", "semlith_binary", "machine", "tools", "store_stats", "fetch_mismatches", "seconds", "runs")},
                   "run_command": run_manifest["command"], "raw": raw_dir.replace(os.path.expanduser("~"), "~"),
                   "raw_files": run_manifest["files"]})
    for r in table:
        print(f"{r['arm']:4} " + " ".join(f"{b // 1000}k {r[f'hit{b}']}/{r['n']}" for b in BUDGETS)
              + f"  file4k {r['file4000']}/{r['n']}  first {r['first_tokens_median']}  {r['ms_median']} ms  errors {r['errors']}")
    print(f"gold passage not in the store's text: {lost or 'none'}")
    print(out)


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "run":
        run(sys.argv[2].split(",") if len(sys.argv) > 2 else ["r0", "r0x", "s1", "s2"])
    elif cmd == "score":
        score(sys.argv[2])
    elif cmd == "fetch":
        fetch(sys.argv[3] if sys.argv[2:3] == ["--from"] else None)
    else:
        {"fetch": fetch, "index": index}[cmd]()
