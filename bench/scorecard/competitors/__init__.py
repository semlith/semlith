"""Competitor arms for the retrieval scorecard: one adapter module per tool,
each exposing NAME, VERSION, available(), version(), index(root, work) and
search(query, root, work, k). See README.md."""

from . import ck, colgrep, graphify, grepai, rg, semble, serena, sourcebot, ugrep

ALL = {
    "rg": rg,
    "ugrep": ugrep,
    "colgrep": colgrep,
    "semble": semble,
    "ck": ck,
    "grepai": grepai,
    "serena": serena,
    "graphify": graphify,
    "sourcebot": sourcebot,
}
