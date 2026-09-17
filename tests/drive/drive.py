#!/usr/bin/env python3
"""Run every finding check against a live semlith portal.

    semlith start                       # in another terminal
    python3 tests/drive/drive.py

The reporting is the same as `.github/smoke.sh` and `.github/portal-check.ps1`,
deliberately, so all three harnesses read the same in a CI log and one
known-failures file governs all of them:

  * a check that passes and is not listed              -> ok
  * a check that fails and is not listed               -> FAIL, and the job fails
  * a check that fails and is listed                   -> xfail, and it does not
  * a check that PASSES and is listed                  -> XPASS, and the job fails
  * a check whose precondition did not hold            -> skip

The XPASS rule is the point of the file: a fix cannot land without the id being
taken out of known-failures.txt in the same breath, so the harness cannot
quietly stop tracking a bug it used to catch.

Environment:

  SEMLITH_PORTAL_URL   default http://127.0.0.1:7365
  SEMLITH_TOKEN        the per-run session token; discovered if unset
  SEMLITH_HOME         where the stores live; default ~/.semlith
  SEMLITH_BIN          the binary the fixtures build a store with; default `semlith`
  KNOWN_FAILURES       default .github/known-failures.txt
  CHROME_PATH          the browser to drive, if it is somewhere unusual
"""

import argparse
import json
import glob
import os
import platform
import re
import sys
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import cdp  # noqa: E402
import fixtures as fixtures_module  # noqa: E402
import findings  # noqa: E402


REPO_ROOT = os.path.dirname(os.path.dirname(HERE))
DEFAULT_URL = "http://127.0.0.1:7365"


# ------------------------------------------------------------------ platform


def platform_tokens():
    """This machine's platform token and its family, as the columns use them."""
    system = platform.system()
    if system == "Windows":
        return "windows", "windows"
    if system == "Darwin":
        return "macos", "unix"
    return "linux", "unix"


def load_known_failures(path, platform_token, family):
    """id -> issue number, for the checks listed against this platform.

    Columns are id, platform, issue, note — the same four `.github/smoke.sh`
    parses with awk and `.github/portal-check.ps1` parses with -split.
    """
    known = {}
    if not os.path.exists(path):
        return known
    with open(path, "r", encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            fields = line.split()
            if len(fields) < 3:
                continue
            check_id, where, issue = fields[0], fields[1], fields[2]
            if where in ("all", platform_token, family):
                known[check_id] = issue
    return known


# --------------------------------------------------------------------- token


def semlith_home():
    home = os.environ.get("SEMLITH_HOME")
    if home:
        return home
    return os.path.join(os.path.expanduser("~"), ".semlith")


def discover_token(portal_url):
    """The session token, read the way `semlith mcp` reads it.

    The daemon writes a `daemon.json` beside every store's lock — see
    `Discovery::write` in src/daemon.rs — carrying the pid, the port and the
    current token. Any one of them will do, so long as its port is the one
    being driven; a file naming a different port belongs to a different daemon.
    """
    wanted_port = None
    match = re.search(r":(\d+)", portal_url)
    if match:
        wanted_port = int(match.group(1))

    pattern = os.path.join(semlith_home(), "stores", "*", "daemon.json")
    for path in sorted(glob.glob(pattern)):
        try:
            with open(path, "r", encoding="utf-8") as handle:
                found = json.load(handle)
        except (OSError, ValueError):
            continue
        token = found.get("token") or ""
        if len(token) != 64:
            continue
        if wanted_port is not None and found.get("port") != wanted_port:
            continue
        return token

    raise SystemExit(
        "no session token. Set SEMLITH_TOKEN, or start a daemon so that one of\n"
        "  %s\n"
        "names the port the drive is pointed at (%s)." % (pattern, portal_url)
    )


# ---------------------------------------------------------------- the runner


def slug(title):
    """A filename fragment from a check's title."""
    cleaned = re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")
    return cleaned[:60]


class Transcript:
    """Everything printed, also written to the output directory."""

    def __init__(self, path):
        self.handle = open(path, "w", encoding="utf-8")

    def line(self, text=""):
        print(text)
        self.handle.write(text + "\n")
        self.handle.flush()

    def close(self):
        self.handle.close()


def main():
    parser = argparse.ArgumentParser(
        description="Replay the 2026-09-17 regression drive against a live portal."
    )
    parser.add_argument("--out", default="drive-out", help="where screenshots and the transcript go")
    parser.add_argument("--only", default="", help="a comma-separated list of finding ids")
    parser.add_argument("--list", action="store_true", help="print the registered ids and exit")
    parser.add_argument(
        "--keep-fixtures",
        action="store_true",
        help="leave the generated corpora on disk, for debugging a failure",
    )
    args = parser.parse_args()

    if args.list:
        for check_id, title, _ in findings.CHECKS:
            print("%-6s %s" % (check_id, title))
        print("\n%d checks" % len(findings.CHECKS))
        return 0

    wanted = {part.strip() for part in args.only.split(",") if part.strip()}
    selected = [c for c in findings.CHECKS if not wanted or c[0] in wanted]
    unknown = wanted - {c[0] for c in findings.CHECKS}
    if unknown:
        raise SystemExit("no such finding: %s" % ", ".join(sorted(unknown)))

    portal_url = os.environ.get("SEMLITH_PORTAL_URL", DEFAULT_URL).rstrip("/")
    token = os.environ.get("SEMLITH_TOKEN") or discover_token(portal_url)

    platform_token, family = platform_tokens()
    known_path = os.environ.get(
        "KNOWN_FAILURES", os.path.join(REPO_ROOT, ".github", "known-failures.txt")
    )
    known = load_known_failures(known_path, platform_token, family)

    out_dir = os.path.abspath(args.out)
    os.makedirs(out_dir, exist_ok=True)
    transcript = Transcript(os.path.join(out_dir, "transcript.txt"))

    transcript.line("platform : %s (%s)" % (platform_token, platform.system()))
    transcript.line("portal   : %s" % portal_url)
    transcript.line("home     : %s" % semlith_home())
    transcript.line("known    : %s" % known_path)
    transcript.line("out      : %s" % out_dir)
    transcript.line("checks   : %d" % len(selected))
    transcript.line()

    drive = cdp.Drive(portal_url, token, out_dir)
    corpora = fixtures_module.Fixtures(keep=args.keep_fixtures)
    drive.fixtures = corpora

    passes = fails = xfails = xpasses = skips = 0
    needs_attention = []

    try:
        drive.start()
    except cdp.BrowserMissing as error:
        transcript.line("the drive cannot run: %s" % error)
        transcript.close()
        return 1

    try:
        for check_id, title, function in selected:
            issue = known.get(check_id)
            failure = None
            skipped = None
            try:
                function(drive)
            except findings.Skipped as reason:
                skipped = str(reason)
            except Exception as error:  # a check may fail any way it likes
                failure = "%s: %s" % (type(error).__name__, error)
                if not isinstance(error, (findings.CheckFailed, cdp.ProtocolError)):
                    failure += "\n" + traceback.format_exc()

            # After the check, whatever happened: a screenshot of a failure is
            # the whole reason anyone opens the output directory.
            try:
                drive.shot("%s-%s" % (check_id, slug(title)))
            except Exception:
                pass

            if skipped is not None:
                skips += 1
                transcript.line("skip     %-6s %s" % (check_id, title))
                transcript.line("         %s" % skipped)
            elif failure is None and issue is None:
                passes += 1
                transcript.line("ok       %-6s %s" % (check_id, title))
            elif failure is not None and issue is not None:
                xfails += 1
                transcript.line("xfail    %-6s %s (#%s)" % (check_id, title, issue))
            elif failure is None and issue is not None:
                xpasses += 1
                needs_attention.append(check_id)
                transcript.line("XPASS    %-6s %s" % (check_id, title))
                transcript.line(
                    "         #%s is fixed. Remove this id from known-failures.txt." % issue
                )
            else:
                fails += 1
                needs_attention.append(check_id)
                transcript.line("FAIL     %-6s %s" % (check_id, title))
                for line in failure.splitlines()[:25]:
                    transcript.line("         %s" % line)
    finally:
        drive.stop()
        corpora.cleanup()

    transcript.line()
    transcript.line("--------------------------------------------------------------")
    transcript.line(
        "pass %d   fail %d   xfail %d   XPASS %d   skip %d"
        % (passes, fails, xfails, xpasses, skips)
    )
    if needs_attention:
        transcript.line("needs attention: %s" % " ".join(needs_attention))
    transcript.close()
    return fails + xpasses


if __name__ == "__main__":
    sys.exit(main())
