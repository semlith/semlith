"""The Benchmarks charts, drawn from the published tables.

    python3 bench/scorecard/charts.py [--tables bench/scorecard/README.md] [--out docs/images/bench]

Reads the tables `report.py --readme` wrote between the scorecard markers, so a chart can never disagree
with its table, and writes one SVG per chart in a light and a dark variant (`<name>-light.svg`,
`<name>-dark.svg`). Standard library only. Colours and type are the portal's own: IBM Plex Sans and Mono
from `src/portal/fonts`, embedded so the chart renders the same in a README image as in the portal, and
the series hues are Semlith's amber, blue and green, stepped to pass the colour-vision checks on each
surface.
"""
import argparse, base64, html, os, re

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
FONTS = os.path.join(ROOT, "src", "portal", "fonts")

THEMES = {
    "light": {
        "surface": "#ffffff", "ink": "#1e2a35", "ink2": "#3a4d5e", "muted": "#64778a",
        "grid": "#e8edf1", "base": "#dce3e8",
        "series": ["#c97f14", "#2a6aad", "#2e8a57"],
    },
    "dark": {
        "surface": "#161f27", "ink": "#e7eef3", "ink2": "#b7c6d1", "muted": "#8a9dab",
        "grid": "#202b34", "base": "#26333d",
        "series": ["#c98320", "#5a8fd0", "#36a06a"],
    },
}

W = 760          # the SVG's width; a README column is about this wide
PAD = 24
BAR = 14         # bar thickness, under the 24 px cap
GAP = 2          # surface gap between the bars of one group
GROUP_GAP = 14


def font_face():
    faces = []
    for family, weight, name in [("Plex Sans", 400, "IBMPlexSans-Regular"), ("Plex Sans", 600, "IBMPlexSans-SemiBold"),
                                 ("Plex Mono", 400, "IBMPlexMono-Regular")]:
        data = base64.b64encode(open(os.path.join(FONTS, name + ".woff2"), "rb").read()).decode()
        faces.append(f"@font-face{{font-family:'{family}';font-weight:{weight};"
                     f"src:url(data:font/woff2;base64,{data}) format('woff2')}}")
    return "".join(faces)


FACES = None


def width_of(text, size, mono=False):
    """A text's width at `size` px, close enough to lay out by (Plex's average advance)."""
    return len(text) * size * (0.6 if mono else 0.56)


def number(cell):
    m = re.search(r"-?[\d,]*\.?\d+", cell.replace(" ", ""))
    return float(m.group(0).replace(",", "")) if m and "—" not in cell else None


def tables(path):
    """{section heading: [row dict]} for every table between the scorecard markers."""
    text = open(path).read()
    block = text.split("<!-- scorecard:begin -->", 1)[1].split("<!-- scorecard:end -->", 1)[0]
    out = {}
    for part in block.split("\n### ")[1:]:
        title, body = part.split("\n", 1)
        lines = [l for l in body.splitlines() if l.startswith("|")]
        if len(lines) < 3:
            continue
        head = [c.strip() for c in lines[0].strip("|").split("|")]
        out[title.strip()] = [dict(zip(head, (c.strip() for c in l.strip("|").split("|")))) for l in lines[2:]]
    return out


def chart(theme, title, subtitle, groups, series, top, fmt, note=None, series_colors=None):
    """Horizontal grouped bars: `groups` is [(label, [value or None per series])], `top` the scale's end."""
    t = THEMES[theme]
    colors = series_colors or list(range(len(series)))
    label_w = max(width_of(g[0], 13) for g in groups) + 16
    value_w = max(width_of(fmt(v), 12, True) for _, vs in groups for v in vs if v is not None) + 10
    plot_x, plot_w = PAD + label_w, W - 2 * PAD - label_w - value_w
    y = PAD + 22 + 20 + 26          # title, subtitle, legend
    rows = []
    for label, values in groups:
        n = len(values)
        h = n * BAR + (n - 1) * GAP
        rows.append((label, values, y, h))
        y += h + GROUP_GAP
    plot_top, plot_bottom = PAD + 22 + 20 + 26 - 6, y - GROUP_GAP + 6
    height = plot_bottom + 26 + (18 if note else 0) + PAD

    o = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{height:.0f}" viewBox="0 0 {W} {height:.0f}" '
         f'role="img" aria-label="{html.escape(title)}">',
         f"<style>{FACES}text{{font-family:'Plex Sans',sans-serif;fill:{t['ink2']}}}"
         f".t{{font-size:16px;font-weight:600;fill:{t['ink']}}}.s{{font-size:12.5px;fill:{t['muted']}}}"
         f".l{{font-size:13px}}.v{{font-family:'Plex Mono',monospace;font-size:12px;fill:{t['ink']}}}"
         f".k{{font-family:'Plex Mono',monospace;font-size:11px;fill:{t['muted']}}}</style>",
         f'<rect width="{W}" height="{height:.0f}" rx="10" fill="{t["surface"]}"/>',
         f'<text class="t" x="{PAD}" y="{PAD + 14}">{html.escape(title)}</text>',
         f'<text class="s" x="{PAD}" y="{PAD + 34}">{html.escape(subtitle)}</text>']
    # Legend, left to right, under the subtitle.
    lx = PAD
    for i, name in enumerate(series):
        o.append(f'<rect x="{lx}" y="{PAD + 47}" width="10" height="10" rx="2" fill="{t["series"][colors[i]]}"/>')
        o.append(f'<text class="l" x="{lx + 15}" y="{PAD + 56}">{html.escape(name)}</text>')
        lx += 15 + width_of(name, 13) + 22
    # Gridlines at quarters of the scale, hairline and recessive, with their ticks underneath.
    for q in range(5):
        gx = plot_x + plot_w * q / 4
        o.append(f'<line x1="{gx:.1f}" y1="{plot_top}" x2="{gx:.1f}" y2="{plot_bottom}" '
                 f'stroke="{t["grid" if q else "base"]}" stroke-width="1"/>')
        o.append(f'<text class="k" x="{gx:.1f}" y="{plot_bottom + 16}" text-anchor="middle">{fmt(top * q / 4, tick=True)}</text>')
    for label, values, gy, h in rows:
        o.append(f'<text class="l" x="{plot_x - 10}" y="{gy + h / 2 + 4.5:.1f}" text-anchor="end">{html.escape(label)}</text>')
        for i, v in enumerate(values):
            by = gy + i * (BAR + GAP)
            if v is None:
                o.append(f'<text class="k" x="{plot_x + 6}" y="{by + BAR - 3}">not run</text>')
                continue
            bw = max(plot_w * min(v, top) / top, 1.5)
            r = min(4, bw / 2)
            # Square at the baseline, 4 px round at the data end.
            o.append(f'<path d="M{plot_x},{by} h{bw - r:.1f} a{r},{r} 0 0 1 {r},{r} v{BAR - 2 * r} '
                     f'a{r},{r} 0 0 1 -{r},{r} h-{bw - r:.1f} z" fill="{t["series"][colors[i]]}"/>')
            o.append(f'<text class="v" x="{plot_x + bw + 6:.1f}" y="{by + BAR - 3}">{fmt(v)}</text>')
    if note:
        o.append(f'<text class="s" x="{PAD}" y="{plot_bottom + 38}">{html.escape(note)}</text>')
    o.append("</svg>")
    return "\n".join(o)


# Colour follows the entity: semlith is always the first hue (amber), the baseline the second (blue),
# and a second semlith arm the third (green), whatever order the legend lists them in.
SEMLITH_LAST = [1, 0, 2]


def nice(v):
    """The scale's end: the next round number at or above `v`, so the quarter ticks are round too."""
    step = 10 ** (len(str(int(v))) - 1)
    return -(-v // (step * 2)) * step * 2


def pct(v, tick=False):
    return f"{v:.0f} %" if tick else f"{v:.1f} %"


def plain(v, tick=False):
    return (f"{v / 1000:.0f}k" if tick and v >= 1000 else f"{v:,.0f}".replace(",", " "))


def ratio(v, tick=False):
    return f"{v:.2f}" if tick else f"{v:.3f}"


def dollars(v, tick=False):
    return f"${v:.2f}" if tick else f"${v:.3f}"


def build(t):
    charts = {}
    swe = t["SWE-bench, retrieval only"]
    arm = lambda s, a: next(r for r in swe if r["set"] == s and r["arm"] == a)
    groups = []
    for s in ("Lite", "Verified"):
        for k, col in (("1", "file hit@1"), ("5", "@5"), ("10", "@10")):
            groups.append((f"{s} · hit@{k}", [number(arm(s, a)[col]) for a in ("semlith", "grep then read")]))
    n = {s: arm(s, "semlith")["instances"] for s in ("Lite", "Verified")}
    charts["swe-hit"] = dict(title="SWE-bench: the right file in the top k",
                             subtitle=f"Share of instances with a gold file in the top k · Lite {n['Lite']}, Verified {n['Verified']} instances · higher is better",
                             groups=groups, series=["semlith", "grep then read"], top=100, fmt=pct)
    groups = [(s, [number(arm(s, a)["median tokens to first gold"]) for a in ("semlith", "grep then read")])
              for s in ("Lite", "Verified")]
    charts["swe-tokens"] = dict(title="SWE-bench: tokens read before the first right file",
                                subtitle="Median tokens an arm returns until its first gold file · lower is better",
                                groups=groups, series=["semlith", "grep then read"],
                                top=nice(max(v for _, vs in groups for v in vs)), fmt=plain)

    rag = t["CodeRAG-Bench, retrieval"]
    charts["coderag"] = dict(title="CodeRAG-Bench: NDCG@10 per task",
                             subtitle="Each task's own corpus and gold documents · higher is better",
                             groups=[(f"{r['task']} ({r['queries']})", [number(r["NDCG@10 semlith"]), number(r["NDCG@10 BM25"])])
                                     for r in rag],
                             series=["semlith", "BM25"], top=1.0, fmt=ratio)

    rb = t["RepoBench-R"]
    charts["repobench"] = dict(title="RepoBench-R: the right snippet in the top 5",
                               subtitle="acc@5 on a seeded 500 instances per row · keep = lines of in-file context · higher is better",
                               groups=[(f"{r['setting']} · {r['level']} · keep {r['keep']}",
                                        [number(r["acc@5 semlith"]), number(r["acc@5 BM25"])]) for r in rb],
                               series=["semlith", "BM25"], top=100, fmt=pct)

    comp = t["Competitors, SWE-bench Lite"]
    rows = [r for r in comp if not r["tool"].startswith("semlith")]
    mine = next(r for r in comp if r["tool"].startswith("semlith"))
    ranked = sorted(rows, key=lambda r: -(number(r["@5"]) or -1))
    groups = [(f"semlith ({mine['completed']}/20)", [number(mine["@5"])])]
    groups += [(f"{r['tool']} ({r['completed']})", [number(r["@5"])]) for r in ranked]
    charts["competitors"] = dict(title="Nine tools on SWE-bench Lite: the right file in the top 5",
                                 subtitle="File hit@5 on a seeded 20 instances, each tool on the instances it completed · higher is better",
                                 groups=groups, series=["semlith", "other tools"], top=100, fmt=pct,
                                 single=True,
                                 note="grepai, colgrep and ck did not finish indexing within 4x semlith's time: \"not run\".")

    ag = t["Agents on an unfamiliar codebase"]
    arms = [("grep (Grep, Glob, Read)", "grep only"), ("semlith installed, Grep built in", "semlith installed"),
            ("semlith installed, Grep gated to semlith first", "semlith first")]
    pick = lambda m, a: next(r for r in ag if r["model"] == m and r["arm"] == a)
    models = [("opus", "Opus"), ("haiku", "Haiku")]
    charts["agents-correct"] = dict(title="Agents on an unfamiliar codebase: answers correct",
                                    subtitle="Headless Claude Code, 50 held-out questions, median of the runs · higher is better",
                                    groups=[(name, [number(pick(m, a)["correct"]) for a, _ in arms]) for m, name in models],
                                    series=[s for _, s in arms], top=100, fmt=pct, colors=SEMLITH_LAST)
    charts["agents-cost"] = dict(title="Agents on an unfamiliar codebase: cost per correct answer",
                                 subtitle="Same sessions, US dollars per correct answer · lower is better",
                                 groups=[(name, [number(pick(m, a)["cost / correct answer"]) for a, _ in arms]) for m, name in models],
                                 series=[s for _, s in arms], top=0.12, fmt=dollars, colors=SEMLITH_LAST)
    return charts


def render(spec, theme):
    if spec.get("single"):
        return chart_single(theme, spec["title"], spec["subtitle"], spec["groups"], spec["series"], spec["top"],
                            spec["fmt"], spec.get("note"))
    return chart(theme, spec["title"], spec["subtitle"], spec["groups"], spec["series"], spec["top"], spec["fmt"],
                 spec.get("note"), spec.get("colors") or list(range(len(spec["series"]))))


def chart_single(theme, title, subtitle, groups, series, top, fmt, note):
    """One bar per row, coloured by `colors[row]`: the ranking chart."""
    t = THEMES[theme]
    svg = chart(theme, title, subtitle, [(g[0], g[1]) for g in groups], series[:1], top, fmt, note, [0])
    # Recolour every row after the first to the second series and add its legend entry.
    first = True
    out = []
    for line in svg.split("\n"):
        if line.startswith("<path") and t["series"][0] in line:
            if not first:
                line = line.replace(t["series"][0], t["series"][1])
            first = False
        out.append(line)
    svg = "\n".join(out)
    lx = PAD + 15 + width_of(series[0], 13) + 22
    legend = (f'<rect x="{lx}" y="{PAD + 47}" width="10" height="10" rx="2" fill="{t["series"][1]}"/>'
              f'<text class="l" x="{lx + 15}" y="{PAD + 56}">{html.escape(series[1])}</text>')
    return svg.replace("</svg>", legend + "\n</svg>")


def main():
    global FACES
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--tables", default=os.path.join(ROOT, "bench", "scorecard", "README.md"))
    ap.add_argument("--out", default=os.path.join(ROOT, "docs", "images", "bench"))
    a = ap.parse_args()
    FACES = font_face()
    os.makedirs(a.out, exist_ok=True)
    for name, spec in build(tables(a.tables)).items():
        for theme in THEMES:
            path = os.path.join(a.out, f"{name}-{theme}.svg")
            open(path, "w").write(render(spec, theme))
            print(path)


if __name__ == "__main__":
    main()
