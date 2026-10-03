#!/usr/bin/env python3
"""Agent round of the scorecard: headless Claude Code on an unfamiliar codebase (SWE-bench Lite, file localisation).

    uv run --with pyarrow python bench/scorecard/agent/run.py sample
    python3 bench/scorecard/agent/run.py prepare
    python3 bench/scorecard/agent/run.py run g,s [--model opus|haiku] [--bin PATH] [--runs N] [--tag T] [--ids a,b]
    python3 bench/scorecard/agent/run.py score

Arms (isolated with per-run flags only, never HOME or CLAUDE_CONFIG_DIR redirects):
  g  Grep, Glob and Read; no MCP server; no user CLAUDE.md, memory, plugins or hooks.
  s  g plus what an installed semlith gives an agent: `<bin> mcp` (alwaysLoad, serving every registered store),
     the skill, and the PreToolUse/PostToolUse `<bin> hook`. Grep stays available.

Every instance worktree is a root of ONE registered store, `scorecard-agent`, so the hook's "covers" check fires
and `semlith mcp` with no --store serves it, exactly as on an installed machine.

An adoption-wording variant of the s arm is one command:
    python3 bench/scorecard/agent/run.py run s --bin /path/to/variant/semlith --tag B [--model haiku]
Its sessions land under arm `s+B`, so they never collide with the plain `s` arm and score as their own row.

Everything written lives under SCORECARD_HOME/agent (default ~/semlith-bench/scorecard/agent).
"""
import argparse, hashlib, json, math, os, re, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
from common import HOME, REPOS, RUNS, median_spread, sh, write_manifest

AGENT = os.path.join(HOME, "agent")
CLONES = os.path.join(AGENT, "repos")
WORK = os.path.join(AGENT, "work")
OUT = os.path.join(AGENT, "runs")
SAMPLE = os.path.join(AGENT, "sample.json")
STORE = "scorecard-agent"
STORE_DIR = os.path.expanduser(f"~/.semlith/stores/{STORE}")
BIN = os.path.expanduser("~/.semlith/bin/semlith")
SKILL = os.environ.get("AGENT_SKILL", os.path.expanduser("~/.claude/skills/semlith/SKILL.md"))
SEED = "20261003"
N = 50
TOOLS = "Grep,Glob,Read"
PROMPT = ("You are working in the repository at {root}. Do not modify anything. Here is an issue:\n\n{problem_statement}\n\n"
          "Which source files would have to change to fix it? End with one line `ANSWER:` followed by a JSON list of "
          "file paths relative to the repository root, most likely first, at most 10.")


def load_sample():
    return json.load(open(SAMPLE))


def sample():
    import swe
    insts = list(swe.instances(["lite"]).values())
    key = lambda i: hashlib.sha256(f"{SEED}-agent-{i['instance_id']}".encode()).hexdigest()
    insts.sort(key=key)
    count = {}
    for i in insts:
        count[i["repo"]] = count.get(i["repo"], 0) + 1
    cap = {r: max(1, math.ceil(N * c / len(insts))) for r, c in count.items()}
    picked, have = [], {}
    # One per repository first (the first by key), then fill in key order under each repository's cap.
    for i in insts:
        if i["repo"] not in have:
            picked.append(i)
            have[i["repo"]] = 1
    for i in insts:
        if len(picked) >= N:
            break
        if i not in picked and have[i["repo"]] < cap[i["repo"]]:
            picked.append(i)
            have[i["repo"]] += 1
    picked.sort(key=key)
    keep = ("instance_id", "repo", "base_commit", "gold", "problem_statement")
    os.makedirs(AGENT, exist_ok=True)
    json.dump([{k: i[k] for k in keep} for i in picked], open(SAMPLE, "w"), indent=1)
    print(len(picked), "instances ->", SAMPLE)
    for r in sorted(have):
        print(f"  {r:28} {have[r]:3} (cap {cap[r]}, lite {count[r]})")


def work_dir(inst):
    return os.path.join(WORK, inst["instance_id"])


def stats():
    out = sh([BIN, "stats", "--store", STORE_DIR], check=False).stdout
    return {k: int(v) for k, v in re.findall(r"^(chunks|vectors|files)\s+(\d+)", out, re.M)}


def prepare():
    insts = load_sample()
    os.makedirs(CLONES, exist_ok=True)
    os.makedirs(WORK, exist_ok=True)
    for inst in insts:
        name = inst["repo"].replace("/", "__")
        clone = os.path.join(CLONES, name)
        if not os.path.isdir(clone):
            sh(["git", "clone", "--quiet", "--no-checkout", os.path.join(REPOS, name), clone])
        d = work_dir(inst)
        if not os.path.isdir(d):
            sh(["git", "-C", clone, "worktree", "add", "--quiet", "--detach", d, inst["base_commit"]])
        missing = [g for g in inst["gold"] if not os.path.exists(os.path.join(d, g))]
        print(f"{inst['instance_id']}: worktree ok{'; gold not on disk: ' + str(missing) if missing else ''}", flush=True)
    roots = [work_dir(i) for i in insts]
    t = time.time()
    r = sh([BIN, "index", "--name", STORE, "-q", "--no-review", *roots], check=False, timeout=6 * 3600)
    held = "held by a running `semlith start`" in r.stderr
    print(f"index exit {r.returncode} in {time.time() - t:.0f}s{' (daemon holds the lock; waiting for it)' if held else ''}")
    if r.returncode and not held:
        raise SystemExit(r.stderr[-2000:])
    # Whoever wrote it, wait until every chunk has its vector and the count holds still.
    last = None
    while True:
        s = stats()
        if s.get("chunks") and s.get("vectors") == s.get("chunks") and s == last:
            break
        last = s
        print("waiting", s, flush=True)
        time.sleep(30)
    print("store", STORE_DIR, s)
    reg = json.load(open(os.path.expanduser("~/.semlith/registry.json")))["stores"].get(STORE, {})
    have = {os.path.realpath(p) for p in reg.get("roots", [])}
    absent = [p for p in roots if os.path.realpath(p) not in have]
    print(f"registered roots {len(have)}; sample roots not registered: {absent or 'none'}")
    ev = {"session_id": "scorecard-agent-check", "hook_event_name": "PreToolUse", "tool_name": "Grep",
          "tool_input": {"pattern": "def foo"}, "cwd": roots[0]}
    nudge = subprocess.run([BIN, "hook"], input=json.dumps(ev), capture_output=True, text=True).stdout.strip()
    print("hook:", nudge or "NOTHING (the hook did not fire)")


def _write(path, obj):
    with open(path, "w") as f:
        json.dump(obj, f)
    return path


def arm_flags(arm, bin_path, tag):
    clean = ["--setting-sources", "project,local", "--strict-mcp-config", "--tools", TOOLS]
    if arm == "g":
        return clean + ["--mcp-config", _write(os.path.join(AGENT, "mcp-off.json"), {"mcpServers": {}})]
    cfg = os.path.join(AGENT, "config", tag or "default")
    plugin = os.path.join(cfg, "plugin")
    os.makedirs(os.path.join(plugin, ".claude-plugin"), exist_ok=True)
    os.makedirs(os.path.join(plugin, "skills", "semlith"), exist_ok=True)
    with open(SKILL) as src, open(os.path.join(plugin, "skills", "semlith", "SKILL.md"), "w") as dst:
        dst.write(src.read())
    _write(os.path.join(plugin, ".claude-plugin", "plugin.json"), {"name": "semlith-bench", "version": "0.0.0", "description": "bench"})
    hook = [{"type": "command", "command": f"{bin_path} hook"}]
    hooks = {"hooks": {"PreToolUse": [{"matcher": "Bash|Read|Grep|Glob", "hooks": hook}],
                       "PostToolUse": [{"matcher": "mcp__.*semlith.*", "hooks": hook}]}}
    # A server key other than `semlith`: a per-project disable of `semlith` beats --mcp-config.
    server = {"semlith-bench": {"type": "stdio", "command": bin_path, "args": ["mcp"], "alwaysLoad": True}}
    return clean + ["--mcp-config", _write(os.path.join(cfg, "mcp-on.json"), {"mcpServers": server}),
                    "--plugin-dir", plugin, "--settings", _write(os.path.join(cfg, "settings-s.json"), hooks)]


def _has(path, needle):
    return os.path.exists(path) and needle in open(path, errors="replace").read()


def run_one(label, flags, model, run, inst):
    d = os.path.join(OUT, model, label, f"r{run}")
    path = os.path.join(d, f"{inst['instance_id']}.jsonl")
    first = path[:-6] + ".timeout1.jsonl"
    if _has(path, '"type":"result"') or (_has(path, "bench_timeout") and os.path.exists(first)):
        return None
    if _has(path, "bench_timeout"):
        os.replace(path, first)  # one re-run after a timeout; the first attempt is kept beside it
    env = {k: v for k, v in os.environ.items() if k not in ("CLAUDECODE", "CLAUDE_CODE_SSE_PORT", "CLAUDE_CODE_ENTRYPOINT")}
    env.update({"CLAUDE_CODE_DISABLE_CLAUDE_MDS": "1", "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1"})
    root = work_dir(inst)
    cmd = ["claude", "-p", PROMPT.format(root=root, problem_statement=inst["problem_statement"]), "--model", model,
           "--output-format", "stream-json", "--verbose", "--permission-mode", "bypassPermissions",
           "--max-budget-usd", "3", *flags]
    t = time.time()
    with open(path, "w") as f:
        try:
            subprocess.run(cmd, cwd=root, env=env, stdout=f, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL, timeout=1200)
        except subprocess.TimeoutExpired:
            f.write(json.dumps({"type": "bench_timeout"}) + "\n")
    timed_out = _has(path, "bench_timeout")
    if timed_out and not os.path.exists(first):
        return run_one(label, flags, model, run, inst)
    return f"{model} {label} r{run} {inst['instance_id']} {time.time() - t:.0f}s{' TIMEOUT' if timed_out else ''}"


def run(a):
    insts = load_sample()
    if a.ids:
        want = set(a.ids.split(","))
        insts = [i for i in insts if i["instance_id"] in want] if not a.ids.isdigit() else insts[:int(a.ids)]
    jobs = []
    for arm in a.arms.split(","):
        label = arm + (f"+{a.tag}" if a.tag and arm != "g" else "")
        flags = arm_flags(arm, os.path.abspath(os.path.expanduser(a.bin)), a.tag)
        for r in range(a.runs):
            os.makedirs(os.path.join(OUT, a.model, label, f"r{r}"), exist_ok=True)
            jobs += [(label, flags, r, i) for i in insts]
    par = int(os.environ.get("AGENT_PAR", "3"))
    print(f"{len(jobs)} sessions, par {par}, model {a.model}, bin {a.bin}", flush=True)
    with ThreadPoolExecutor(par) as ex:
        for msg in ex.map(lambda j: run_one(j[0], j[1], a.model, j[2], j[3]), jobs):
            if msg:
                print(msg, flush=True)
    print("ALL DONE", flush=True)


def _events(path):
    for line in open(path, errors="replace"):
        try:
            yield json.loads(line)
        except ValueError:
            pass


def session(path):
    tools, result, timeout = [], {}, False
    for e in _events(path):
        if e.get("type") == "assistant":
            for c in e["message"].get("content", []):
                if c.get("type") == "tool_use":
                    tools.append(c["name"])
        elif e.get("type") == "result":
            result = e
        elif e.get("type") == "bench_timeout":
            timeout = True
    u = result.get("usage", {})
    return {"tools": tools, "text": result.get("result", "") or "", "cost": result.get("total_cost_usd") or 0.0,
            "timeout": timeout, "error": result.get("is_error"), "subtype": result.get("subtype"),
            "tokens_in": (u.get("input_tokens") or 0) + (u.get("cache_read_input_tokens") or 0)
            + (u.get("cache_creation_input_tokens") or 0)}


def parse_answer(text):
    """The last `ANSWER:` line's JSON list of paths, or None."""
    m = None
    for m in re.finditer(r"ANSWER:\s*`*\s*(\[.*?\])", text, re.S):
        pass
    if not m:
        return None
    try:
        items = json.loads(m.group(1))
    except ValueError:
        return None
    return [i if isinstance(i, str) else i.get("path", "") for i in items if isinstance(i, (str, dict))]


def norm(path, root):
    """An answer path -> repository-relative. An absolute path under any instance worktree loses that prefix."""
    p = path.strip().strip("`")
    if os.path.isabs(p):
        if p.startswith(root.rstrip("/") + "/"):
            p = p[len(root.rstrip("/")) + 1:]
        elif p.startswith(WORK + "/"):
            p = p[len(WORK) + 1:].split("/", 1)[-1]
    return p[2:] if p.startswith("./") else p


def rows():
    insts = {i["instance_id"]: i for i in load_sample()}
    out = []
    for model in sorted(os.listdir(OUT)) if os.path.isdir(OUT) else []:
        for label in sorted(os.listdir(os.path.join(OUT, model))):
            for rd in sorted(os.listdir(os.path.join(OUT, model, label))):
                d = os.path.join(OUT, model, label, rd)
                for name in sorted(os.listdir(d)):
                    if not name.endswith(".jsonl") or ".timeout1." in name:
                        continue
                    inst = insts[name[:-6]]
                    s = session(os.path.join(d, name))
                    if not s["timeout"] and not s["text"] and not s["subtype"]:
                        continue  # still running
                    ans = parse_answer(s["text"])
                    files = [norm(p, work_dir(inst)) for p in (ans or [])][:10]
                    gold = set(inst["gold"])
                    retried = os.path.exists(os.path.join(d, name[:-6] + ".timeout1.jsonl"))
                    first_cost = session(os.path.join(d, name[:-6] + ".timeout1.jsonl"))["cost"] if retried else 0.0
                    out.append({"model": model, "arm": label, "run": int(rd[1:]), "instance_id": inst["instance_id"],
                                "repo": inst["repo"], "gold": inst["gold"], "files": files, "answered": ans is not None,
                                **{f"any@{k}": bool(set(files[:k]) & gold) for k in (1, 5, 10)},
                                **{f"all@{k}": gold <= set(files[:k]) for k in (1, 5, 10)},
                                "cost": s["cost"] + first_cost, "tokens_in": s["tokens_in"], "timeout": s["timeout"],
                                "retried": retried, "subtype": s["subtype"], "n_tools": len(s["tools"]),
                                "semlith_calls": sum("semlith" in t for t in s["tools"]),
                                "grep_calls": s["tools"].count("Grep")})
    return out


def score():
    rs = rows()
    table = []
    for model, arm in sorted({(r["model"], r["arm"]) for r in rs}):
        per_run = []
        for run in sorted({r["run"] for r in rs if r["model"] == model and r["arm"] == arm}):
            g = [r for r in rs if (r["model"], r["arm"], r["run"]) == (model, arm, run)]
            correct = sum(r["any@5"] for r in g)
            toks = sorted(r["tokens_in"] for r in g)
            m = {"n": len(g), **{k: sum(r[k] for r in g) / len(g) for k in
                                 ("any@1", "any@5", "any@10", "all@1", "all@5", "all@10")},
                 "answered": sum(r["answered"] for r in g) / len(g),
                 "cost_mean": sum(r["cost"] for r in g) / len(g), "cost_total": sum(r["cost"] for r in g),
                 "cost_per_correct": sum(r["cost"] for r in g) / correct if correct else None,
                 "tokens_in_median": toks[len(toks) // 2],
                 "used_semlith": sum(r["semlith_calls"] > 0 for r in g) / len(g),
                 "timeouts": sum(r["timeout"] for r in g), "retried": sum(r["retried"] for r in g)}
            per_run.append(m)
        row = {"model": model, "arm": arm, "runs": len(per_run), "n": [p["n"] for p in per_run]}
        for key in per_run[0]:
            if key != "n":
                vals = [p[key] for p in per_run if p[key] is not None]
                row[key], row[key + "_spread"] = median_spread(vals) if vals else (None, None)
        table.append(row)
    os.makedirs(AGENT, exist_ok=True)
    with open(os.path.join(AGENT, "rows.jsonl"), "w") as f:
        for r in rs:
            f.write(json.dumps(r) + "\n")
    with open(os.path.join(AGENT, "score.json"), "w") as f:
        json.dump(table, f, indent=1)
    write_manifest(AGENT, {"bench": "agent", "store": STORE})
    fmt = lambda v, p=3: "-" if v is None else f"{v:.{p}f}"
    keys = ("any@1", "any@5", "any@10", "all@1", "all@5", "all@10", "cost_mean", "cost_per_correct",
            "tokens_in_median", "used_semlith", "timeouts")
    print("model  arm    runs n   " + " ".join(f"{k:>13}" for k in keys))
    for r in table:
        cells = [f"{fmt(r[k], 0 if k == 'tokens_in_median' else 3)}±{fmt(r[k + '_spread'], 0 if k == 'tokens_in_median' else 2)}"
                 for k in keys]
        print(f"{r['model']:6} {r['arm']:6} {r['runs']:4} {max(r['n']):3} " + " ".join(f"{c:>13}" for c in cells))
    print(f"total cost ${sum(r['cost'] for r in rs):.2f} over {len(rs)} sessions")
    return table


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["sample", "prepare", "run", "score"])
    ap.add_argument("arms", nargs="?", default="g,s")
    ap.add_argument("--model", default="opus")
    ap.add_argument("--bin", default=BIN)
    ap.add_argument("--runs", type=int, default=RUNS)
    ap.add_argument("--tag", help="suffix for the s arm's label, for a variant binary: arm s+TAG")
    ap.add_argument("--ids", help="comma list of instance ids, or a number N for the first N of the sample")
    a = ap.parse_args()
    {"sample": sample, "prepare": prepare, "score": score}.get(a.cmd, lambda: run(a))()
