"""Savings from real sessions: the owner's own ledger, aggregated across stores, with its comparison.

    python bench/scorecard/ledger_savings.py [--exclude corpus,docs-bench] [--window all]

Each store's `semlith report savings --format json` says how many retrievals it recorded, how many it
credits (coverage, and whether the counts are measured or modelled), the tokens of the whole files those
answers came from, and the tokens of the excerpts actually returned. This sums them. The comparison is
the report's own: reading every file a retrieval answered from, which is an upper bound on what grep
and read would have cost, not a measurement of it -- the agent round is the head-to-head. Stores
that hold benchmark traffic are excluded by default. Only aggregate numbers are printed: no query
text, no paths.
"""
import argparse, json, os, re, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import RESULTS, SEMLITH, run_dir, sh, write_manifest

STORE_HOME = os.path.expanduser("~/.semlith/stores")


def number(text):
    return int(re.sub(r"[^\d]", "", text) or 0)


def store_report(store, window):
    raw = sh([SEMLITH, "report", "savings", "--store", store, "--format", "json", "--window", window]).stdout
    doc = json.loads(raw)
    out = {"store": os.path.basename(store)}
    for block in doc["blocks"]:
        if block["block"] == "text" and block["text"].startswith("Over "):
            m = re.search(r"Over ([\d ]+) recorded retrievals, ([\d ]+) of which are credited \((\d+)% coverage, (\w+)\)",
                          block["text"])
            if m:
                out.update(retrievals=number(m.group(1)), credited=number(m.group(2)), tier=m.group(4))
        for row in block.get("rows", []) or []:
            if row[0] == "Whole files not read":
                out["whole_tokens"] = number(row[1])
            elif row[0] == "Excerpts read instead":
                out["excerpt_tokens"] = number(row[1])
            elif row[0] == "Refunds":
                out["refunds"] = number(row[1])
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exclude", default="corpus,docs-bench")
    ap.add_argument("--window", default="all")
    a = ap.parse_args()
    skip = set(a.exclude.split(","))
    rows = []
    for name in sorted(os.listdir(STORE_HOME)):
        path = os.path.join(STORE_HOME, name)
        if name in skip or not os.path.exists(os.path.join(path, "store.db")):
            continue
        r = store_report(path, a.window)
        if r.get("retrievals"):
            rows.append(r)
    total = {k: sum(r.get(k, 0) for r in rows) for k in ("retrievals", "credited", "whole_tokens", "excerpt_tokens", "refunds")}
    total["stores"] = len(rows)
    total["coverage"] = round(100 * total["credited"] / total["retrievals"]) if total["retrievals"] else 0
    total["ratio"] = round(total["whole_tokens"] / total["excerpt_tokens"], 1) if total["excerpt_tokens"] else None
    total["tiers"] = sorted({r.get("tier") for r in rows})
    # One store with a few very large files can carry the total; the median store says what is typical.
    ratios = sorted(r["whole_tokens"] / r["excerpt_tokens"] for r in rows if r.get("excerpt_tokens"))
    for r in rows:
        r["ratio"] = round(r["whole_tokens"] / r["excerpt_tokens"], 1) if r.get("excerpt_tokens") else None
    total["median_store_ratio"] = round(ratios[len(ratios) // 2], 1) if ratios else None
    out = run_dir("ledger")
    json.dump({"stores": rows, "total": total, "window": a.window, "excluded": sorted(skip)},
              open(os.path.join(out, "savings.json"), "w"), indent=1)
    write_manifest(out, {"bench": "ledger-savings", "window": a.window})
    for r in rows:
        print(f"{r['store']:14} retrievals={r['retrievals']:6} credited={r['credited']:6} ({r.get('tier')}) "
              f"whole={r.get('whole_tokens', 0):12} excerpts={r.get('excerpt_tokens', 0):10} ratio={r['ratio']}")
    print(f"TOTAL {total['stores']} stores: {total['retrievals']} retrievals, {total['coverage']} % credited "
          f"({'/'.join(total['tiers'])}); excerpts {total['excerpt_tokens']} tokens against {total['whole_tokens']} "
          f"for the whole files they came from ({total['ratio']}x; median store {total['median_store_ratio']}x)")
    print(out)


if __name__ == "__main__":
    main()
