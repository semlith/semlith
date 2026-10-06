"""One check per finding from the 2026-09-17 full regression drive, and one per
v6 view and flow.

The findings document describes what the 0.20.1 portal did wrong. Every check
here asserts what the product should do instead, so a check that passes means
the bug is gone and a check that fails means it is back.

Each check is registered by its finding id:

    @finding("1.1", "a store holds no files outside its registered roots")
    def _(d): ...

`d` is a `cdp.Drive`: a headless browser plus an HTTP client for the portal,
both already holding the session token. Assertions go over whichever surface
is honest for the finding. A status code, a JSON shape or a `limit` refusal is
asserted over HTTP; a layout, a piece of copy or what a button does is
asserted through the DOM.

0.35.0 rebuilt the portal on design v6: nine pages in three groups, a store's
own page with five tabs, a Welcome screen and a store wizard, all routed on
`#/<page>/<parts>`. Every check below was migrated to that markup, never
deleted or weakened — each keeps the behaviour it asserted and points at the
surface that now carries it (the Index page's run cards are a store's Runs tab;
its folder pickers are the wizard's Sources step; Machine limits is Settings ›
Performance; Impact is Graph › Blast radius; About is Settings › About). Where
v6 deliberately reversed an old rule, the check asserts the new rule and says
"0.35.0 owner decision" beside it: v6 as drawn wins over older rules, and the
owner named the three biggest reversals (bulk decisions in review, session
replay on by default, a runtime ledger recording switch). That comment is the
only way a check's meaning is allowed to change.

The `v6.*` checks at the end are new in 0.35.0: one or more per view and flow,
each asserting that the view renders without console errors, shows what the
daemon says rather than sample values, and that its primary control works.
`v6.shots` writes a screenshot of every view in light and dark at 1440 px and
390 px into the output directory — the release record's evidence.

Two conventions worth knowing before you edit this file:

  * Nothing raises a bare `AssertionError`. Every failure names what was
    expected and what was seen, because the person reading it is looking at a
    CI log six months from now and has no other context.
  * A check whose precondition could not be met raises `Skipped`, not a
    failure. A harness that blames the product for a missing browser, a
    missing network or a missing git is worse than no harness. This is the
    same rule `.github/smoke.sh` states at the top of itself.

Where the corrected behaviour was genuinely ambiguous — the findings document
says what is wrong, not always what right looks like — the check takes the
reading the finding's own "should" sentence gives, and a comment names the
ambiguity so the next person can see the choice was made rather than assumed.
"""

import json
import random
import string
import os
import re
import time
import urllib.parse

import cdp
import fixtures

CHECKS = []


# The portal builds every control itself, so a selector here is only ever worth
# what `src/portal/app.js` actually writes. These are named rather than
# repeated because several checks share them and a guessed selector that
# matches nothing turns a check into a check of nothing.

#: A live run's card on a store's Runs tab — `sdRuns()` draws one `runCard()`
#: (`card run-card blue-edge`, the wizard's card too) per run that is not
#: finished, above the History card.
LIVE_CARD = "#main .card.blue-edge"

#: One finished run on a store's Runs tab — `sdRuns()`'s History rows, one per
#: run, keyed by store, run id and start time.
HIST_ROW = "#main .hist-row"

#: The confirm dialog `ask()` builds, and the folder picker `pickFolder()`
#: builds in the same shape. There is no `<dialog>` in the v6 page.
MODAL = cdp.Drive.MODAL

#: The Search page's query box — `aria-label: "Search query"`.
SEARCH_BOX = '#main input[aria-label="Search query"]'

#: One file's block of hits on the Search page — `card hit-group`.
RESULT_CARD = "#main .hit-group"

#: The page's one scroller. `<main class="main">` carries the overflow for
#: every page that is not a full-height one (Search and Graph are `main.fill`).
MAIN = "document.querySelector('#main')"


class CheckFailed(Exception):
    """The corrected behaviour is not there."""


class Skipped(Exception):
    """The check could not run, which is not the product's fault."""


def finding(finding_id, title):
    def register(function):
        CHECKS.append((finding_id, title, function))
        return function

    return register


def fail(message):
    raise CheckFailed(message)


def skip(reason):
    raise Skipped(reason)


def want(what, seen, expected):
    """The one assertion helper. Every failure reads the same way."""
    if seen != expected:
        fail("%s: expected %r, saw %r" % (what, expected, seen))


# ------------------------------------------------------------------ DOM sugar


def text_of(d, selector, what):
    """The visible text of one element, or a failure naming the selector."""
    value = d.eval(
        "(() => { const el = document.querySelector(%s);"
        " return el ? (el.innerText || '').trim() : null; })()" % json.dumps(selector)
    )
    if value is None:
        fail(
            "%s: nothing matched %r. If the portal's markup moved, update this "
            "check rather than deleting it." % (what, selector)
        )
    return value


def texts_of(d, selector):
    """The visible text of every matching element, in document order."""
    return d.eval(
        "[...document.querySelectorAll(%s)].map(el => (el.innerText || '').trim())"
        % json.dumps(selector)
    )


def exists(d, selector):
    return bool(d.eval("!!document.querySelector(%s)" % json.dumps(selector)))


def view_text(d):
    """Everything the current view says, for copy assertions."""
    return d.eval("(document.querySelector('#main') || document.querySelector('#root') || document.body).innerText")


def pause(d, ms):
    """A short, bounded settle inside the page, for a debounce or a repaint."""
    d.eval("new Promise(done => setTimeout(() => done(true), %d))" % ms)


def press_text(d, selector, text, what=None):
    """Click the control matching `selector` whose own words are `text`.

    The same rule as `cdp.Drive.click_text`, but returning a failure in this
    file's words rather than a protocol error, because a control that is not on
    offer is the finding, not the harness.
    """
    try:
        d.click_text(selector, text)
    except cdp.ProtocolError as error:
        fail("%s: %s" % (what or ("pressing %r" % text), error))


def rects(d, selector):
    """The bounding boxes of matching elements, for layout assertions."""
    return d.eval(
        """
        [...document.querySelectorAll(%s)].map(el => {
          const r = el.getBoundingClientRect();
          return {top: r.top, left: r.left, bottom: r.bottom, right: r.right,
                  width: r.width, height: r.height,
                  text: (el.innerText || '').trim().slice(0, 40)};
        })
        """
        % json.dumps(selector)
    )


def tip_of(d, selector):
    """The full value a truncated element hands over on hover.

    v6 hangs it on `data-tip` — the page's one rich tooltip, which follows the
    pointer and also opens on keyboard focus — and a native `title` is the
    other honest carrier. Either is a full value reachable without the DOM
    inspector, which is what finding 4.1 asked for.
    """
    return d.eval(
        "(() => { const el = document.querySelector(%s); if (!el) return null;"
        " const h = el.closest('[data-tip], [title]');"
        " return h ? (h.getAttribute('data-tip') || h.getAttribute('title')) : null; })()"
        % json.dumps(selector)
    )


# ----------------------------------------------------------- portal sugar


def stores(d):
    return d.api("/api/stores")["stores"]


def store_named(d, name):
    for row in stores(d):
        if row["name"] == name:
            return row
    fail("no store named %r is open. Stores present: %s"
         % (name, ", ".join(sorted(r["name"] for r in stores(d)))))


def doctor_state(row):
    """The state the Agents › Health page shows for one client row.

    `/api/doctor` returns the facts — `note`, `registered`, `scope`, `command`,
    `present` — and the page derives the words from them. There is no `state`
    field on the row, so a check that read one grouped every client under `''`
    and then compared their `command` values, which are the client binaries
    (`claude`, `codex`, …) and not remedies at all. This is that derivation.
    """
    if row.get("note"):
        return "cannot register"
    if row.get("registered"):
        return "registered (%s)" % (row.get("scope") or "user")
    if not row.get("command"):
        return "not registered"
    if not row.get("present"):
        return "not installed"
    if row.get("scope") == "project":
        return "one project only"
    return "installed, not registered"


def start_index(d, path, review=None):
    """Index a folder as its own store, and return (run id, store name).

    `store: "each"` is used rather than naming a store because it is the one
    submission shape that both creates the store and tells the caller what it
    was called, which makes every later assertion about that store exact.
    """
    body = {"path": [path], "store": "each"}
    if review:
        body["review"] = review
    answer = d.api("/api/index", method="POST", body=body)
    runs = answer.get("runs") or []
    if not runs or "error" in runs[0]:
        fail("indexing %s was refused: %s" % (path, json.dumps(answer)[:300]))
    return runs[0]["run"], runs[0]["store"]


def run_for(d, store_name):
    """The newest run the daemon holds for a store, or None.

    Runs are keyed by id and a store can have several, so "the store's run" is
    the most recent of them rather than whichever the route listed first — the
    first is usually one that finished several checks ago.
    """
    mine = [run for run in d.api("/api/index/runs")["runs"] if run["store"] == store_name]
    if not mine:
        return None
    return max(mine, key=lambda run: run.get("id") or 0)


def run_by_id(d, run_id):
    for run in d.api("/api/index/runs")["runs"]:
        if run.get("id") == run_id:
            return run
    return None


TERMINAL = {"done", "stopped", "failed"}

#: How long a submitted run may take to appear on `/api/index/runs` at all.
#: A run that never appears was never admitted, and no amount of further
#: waiting will change that.
RUN_APPEARS = 30

#: How long a fixture run may take to finish. The corpora this drive builds are
#: three files and six hundred files; neither is minutes of work, and a wait
#: long enough to cover a pathological machine is long enough to stall every
#: check behind it. One check used to wait ten minutes for a run that never
#: started and took the whole drive down with it.
RUN_FINISHES = 240


def wait_for_run(d, store_name, timeout=RUN_FINISHES, run_id=None):
    """Block until a run is over, and return its final snapshot.

    Bounded twice, because the two ways a run fails to finish need different
    messages. A run that never appears at all was refused a slot rather than
    started, and saying so is most of the diagnosis.
    """
    look = (lambda: run_by_id(d, run_id)) if run_id is not None else (lambda: run_for(d, store_name))
    what = "run %s on %s" % (run_id, store_name) if run_id is not None else "the run on %s" % store_name

    appears = time.time() + RUN_APPEARS
    while time.time() < appears:
        if look() is not None:
            break
        time.sleep(0.5)
    else:
        fail(
            "%s never appeared on /api/index/runs within %ds. The submission was "
            "accepted, so the run exists somewhere and was never admitted — which "
            "is what an admission slot leaked by an earlier stop looks like."
            % (what, RUN_APPEARS)
        )

    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        last = look()
        if last and str(last.get("status", "")).lower() in TERMINAL:
            return last
        time.sleep(0.5)
    fail(
        "%s never finished within %ds (last status: %s, %s of %s files). Bounded "
        "deliberately: a check that waits indefinitely stalls every check after it."
        % (
            what,
            timeout,
            last and last.get("status"),
            last and last.get("scanned"),
            last and last.get("total"),
        )
    )


def indexed_fixture(d, path):
    """Index a fixture folder and wait for it, returning the store name."""
    run_id, store_name = start_index(d, path)
    wait_for_run(d, store_name, run_id=run_id)
    return store_name


def a_store(d):
    """A store with files in it, made if there is none yet.

    With no store registered the v6 router draws the Welcome screen for almost
    every route, so a check about a page needs one before it can open that page.
    """
    for row in stores(d):
        if row.get("files") and not row.get("missing") and not row.get("unopened"):
            return row["name"]
    return indexed_fixture(d, d.fixtures.small())


def all_files(d, params=""):
    """Every row of /api/files, paged, so a check can reason about the corpus."""
    rows = []
    offset = 0
    while True:
        page = d.api("/api/files?limit=500&offset=%d%s" % (offset, params))
        rows.extend(page["files"])
        if len(page["files"]) < 500 or offset + 500 >= page["total"]:
            return rows
        offset += 500


def normalise(path):
    """One spelling of a path, so a Windows comparison is about the path.

    The verbatim prefix comes off first. `registry.json` holds what
    `std::fs::canonicalize` produced, which on Windows is `\\\\?\\C:\\...`, and
    that is deliberate — it is what the long-path APIs need. A comparison
    against a path a person typed has to meet it in the middle.
    """
    text = path.replace("\\", "/").rstrip("/").lower()
    for prefix in ("//?/unc/", "//?/"):
        if text.startswith(prefix):
            text = text[len(prefix):]
            if prefix == "//?/unc/":
                text = "//" + text
            break
    return text


def tilde(d, path):
    """`path` the way the page spells it: the home directory as `~`.

    `tilde()` in app.js shortens every path under the user's home, and the
    home is what `/api/dirs` reports, so a check comparing against the page
    asks the same question.
    """
    home = d.api("/api/dirs").get("home") or ""
    return "~" + path[len(home):] if home and path.startswith(home) else path


def sidebar_stores(d):
    """The sidebar's store count, as the daemon card says it."""
    text = text_of(d, ".nav .daemon .facts", "the sidebar's daemon card")
    found = re.search(r"(\d+) stores?", text)
    if not found:
        fail("the sidebar's daemon card does not count stores: %r" % text)
    return int(found.group(1))


def stop_quietly(d, store):
    """Stop a run if one is going, and say nothing when there is not.

    Tidying up after a check. A run that finished on its own is refused with a
    409 that names exactly that, which is the product being right rather than
    something for the drive to raise.
    """
    try:
        d.api("/api/index/control", method="POST", body={"store": store, "action": "stop"})
    except cdp.ProtocolError as refused:
        # A store its own stop deleted has nothing left to stop either.
        if not re.search(r"no run to stop|no store called|no store is open", str(refused)):
            raise


def store_row_js(name):
    """A JS expression for the Stores list row of one store, or undefined.

    The row's name is the first text of `.cellname .a`, which also holds the
    store's kind in a `.k` span after it.
    """
    return (
        "[...document.querySelectorAll('#main .gl-row.gl-stores')].find(r =>"
        " ((r.querySelector('.cellname .a') || {}).firstChild || {}).textContent"
        " && r.querySelector('.cellname .a').firstChild.textContent.trim() === %s)"
        % json.dumps(name)
    )


def stores_widest(d):
    """Show every store on one page of the Stores list.

    The list opens at 10 rows a page (0.35.0, v6 as drawn), and this drive makes
    more stores than that, so a check looking for one row asks for 100 first.
    """
    if d.eval("(() => { const s = document.querySelector('#main .pager .dd'); return !!s && s.dataset.value !== '100'; })()"):
        pick(d, "#main .pager .dd", "100")
    pause(d, 150)


def pick(d, selector, label):
    """Choose `label` in the portal's dropdown at `selector`, the way a person
    does: open it, press the item. From 0.35.0 every dropdown is the portal's
    own (a button and the menu), not a system <select>."""
    d.eval("document.querySelector(%s).click()" % json.dumps(selector))
    d.wait_for("!!document.querySelector('.menu:not([hidden]) .menu-item')", timeout=5, what="the dropdown at %s to open" % selector)
    hit = d.eval(
        "(() => { const b = [...document.querySelectorAll('.menu:not([hidden]) .menu-item')]"
        ".find(x => (x.querySelector('.ell') || x).textContent.trim() === %s);"
        " if (b) b.click(); return !!b; })()" % json.dumps(label)
    )
    if not hit:
        fail("the dropdown at %s offers no %r" % (selector, label))


def open_wizard_for(d, store):
    """The wizard's Sources step for an existing store, the way a person gets there.

    A store's page offers Add sources; the wizard opens on step 2 with the store
    already named, because naming it was done when it was made.
    """
    d.open_view("store/%s" % store)
    press_text(d, "#main button", "Add sources", "the store page's Add sources")
    d.wait_for(
        "!!document.querySelector('.wz-steps') && /STEP 2 OF/.test((document.querySelector('.wz-head') || {}).innerText || '')",
        what="the wizard's Sources step for %s" % store,
    )


def wizard_mode(d, label):
    """Open one of the Sources step's three panels: Browse folders, Paste a path, Add a URL."""
    pressed = d.eval(
        "(() => { const b = [...document.querySelectorAll('.dropzone button')]"
        ".find(b => (b.innerText || '').trim() === %s);"
        " if (!b) return null; if (b.getAttribute('aria-pressed') !== 'true') b.click(); return true; })()"
        % json.dumps(label)
    )
    if not pressed:
        fail("the wizard's Sources step offers no %r" % label)
    pause(d, 100)


def browse_where(d, scope):
    """The folder a folder picker inside `scope` is showing, as the page spells it."""
    return d.wait_for(
        "(() => { const t = ((document.querySelector(%s + ' .browse-head .dir') || {}).innerText || '').trim();"
        " return t && t !== '…' ? t : null; })()" % json.dumps(scope),
        what="the folder picker in %s to show where it is" % scope,
    )


def browse_into(d, scope, name):
    """Click the folder row named `name` in a folder picker inside `scope`."""
    before = browse_where(d, scope)
    clicked = d.eval(
        """
        (() => {
          for (const row of document.querySelectorAll(%s + ' .bitem')) {
            const n = row.querySelector('.name');
            if (n && n.textContent === %s) {
              // The wizard's picker selects on a click and opens a folder on a
              // double-click (0.35.0); the adopt picker, which has nothing to
              // select, opens on a click.
              const open = row.querySelector('button.open');
              if (row.querySelector('[role=checkbox]')) open.dispatchEvent(new MouseEvent('dblclick', {bubbles: true}));
              else open.click();
              return true;
            }
          }
          return false;
        })()
        """
        % (json.dumps(scope), json.dumps(name))
    )
    if not clicked:
        fail("the folder picker at %s lists nothing named %s: %s"
             % (before, name, texts_of(d, scope + " .bitem .name")))
    d.wait_for(
        "(() => { const t = ((document.querySelector(%s + ' .browse-head .dir') || {}).innerText || '').trim();"
        " return t && t !== '…' && t !== %s; })()" % (json.dumps(scope), json.dumps(before)),
        what="the picker to open %s" % name,
    )


def descend(d, scope, path):
    """Walk a folder picker in `scope` from wherever it is down to `path`.

    Clicking the rows it lists is the only way in. `/api/dirs` confines every
    picker to the home directory, and the picker's state is a closure with no
    handle on it from outside the page, so a check that wants the picker
    pointed somewhere has to point it the way a person would.
    """
    home = d.api("/api/dirs").get("home") or ""
    here = browse_where(d, scope)
    here_full = home + here[1:] if here.startswith("~") else here
    if normalise(path) == normalise(here_full):
        return
    if not normalise(path).startswith(normalise(here_full).rstrip("/") + "/"):
        fail("the picker is showing %s, which is not above %s" % (here, path))
    rest = path.replace("\\", "/")[len(here_full.replace("\\", "/")):].strip("/")
    for part in [p for p in rest.split("/") if p]:
        browse_into(d, scope, part)


def pick_store(d, button, store, route):
    """Pick `store` in one of the Graph page's store pickers.

    The picker lists the stores the page knew when it was drawn, and the page
    brings its store list up to date a poll later, so a store a check has just
    indexed can be a redraw away from the menu. The route is opened again until
    it is there, bounded.
    """
    deadline = time.time() + 20
    while True:
        d.click(button)
        if d.eval("[...document.querySelectorAll('.menu:not([hidden]) .menu-item .ell')].some(e => e.textContent === %s)"
                  % json.dumps(store)):
            press_text(d, ".menu .menu-item .ell", store, "picking %s" % store)
            d.wait_for("[...document.querySelectorAll(%s + ' .mono')].some(e => e.textContent.trim() === %s)"
                       % (json.dumps(button), json.dumps(store)), what="the picker to read %s" % store)
            return
        d.eval("document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape'}))")
        if time.time() > deadline:
            fail("the store picker never offered %s, a store with files: %s"
                 % (store, texts_of(d, ".menu .menu-item .ell")))
        time.sleep(1)
        d.open_view(route)


# ---------------------------------------------------------------- run cards


def live_card(store=None):
    """A JS expression for the first live run card on the open Runs tab."""
    return "document.querySelector(%s)" % json.dumps(LIVE_CARD)


def card_offers(card_js, label):
    """A JS predicate: the card offers a visible button that reads `label`."""
    return (
        "(() => { const c = %s; return !!c && [...c.querySelectorAll('button')]"
        ".some(b => !b.hidden && b.offsetParent !== null && (b.textContent || '').trim() === %s); })()"
        % (card_js, json.dumps(label))
    )


def grouped(value):
    """A count the way the page prints one: `Intl.NumberFormat("en-US")`."""
    return "{:,}".format(int(value or 0))


def newest_history_row(d, run):
    """The top History row once it describes `run`, or None.

    Waited for, not read once: the page paints from its last poll, so a run
    that finished a moment ago can be a second away from its row, and reading
    the top row in that second reads the run before it.
    """
    said = "%s indexed · %s chunks" % (grouped(run.get("indexed")), grouped(run.get("chunks")))
    try:
        return d.wait_for(
            "(() => { const r = document.querySelector(%s);"
            " return r && r.innerText.includes(%s) ? r.innerText : null; })()"
            % (json.dumps(HIST_ROW), json.dumps(said)),
            timeout=15,
            what="the top History row to describe run %s" % run.get("id"),
        )
    except cdp.ProtocolError:
        return None


def press_in(d, card_js, label):
    """Press the visible button reading `label` inside the element `card_js`."""
    pressed = d.eval(
        "(() => { const c = %s; if (!c) return 'nothing to press in';"
        " const b = [...c.querySelectorAll('button')].find(b => !b.hidden && b.offsetParent !== null"
        " && (b.textContent || '').trim() === %s);"
        " if (!b) return 'nothing on offer reads ' + %s; b.click(); return 'pressed'; })()"
        % (card_js, json.dumps(label), json.dumps(label))
    )
    if pressed != "pressed":
        fail("pressing %r: %s" % (label, pressed))


# ==========================================================================
# P1 — data or trust is wrong
# ==========================================================================


@finding("1.1", "a store holds no files outside its registered roots")
def _(d):
    # The decision this encodes: out-of-root rows are reconciled away when the
    # daemon opens a store and again on `semlith index`. So by the time the
    # portal can be asked, there are none — not a badge saying there are some.
    roots = {}
    for row in stores(d):
        roots[row["name"]] = [normalise(r["path"]) for r in row.get("roots", [])]

    strays = []
    duplicates = []
    seen = set()
    for row in all_files(d):
        store, path = row["store"], normalise(row["path"])
        registered = roots.get(store, [])
        if registered and not any(path.startswith(root + "/") or path == root for root in registered):
            strays.append("%s claims %s, whose roots are %s" % (store, row["path"], registered))
        if (store, path) in seen:
            duplicates.append("%s holds %s twice" % (store, row["path"]))
        seen.add((store, path))

    if strays:
        fail(
            "%d indexed files fall outside their store's registered roots; the "
            "daemon reconciles these away on open and on index, so there should "
            "be none. First few:\n  %s" % (len(strays), "\n  ".join(strays[:5]))
        )
    if duplicates:
        fail("a store lists the same path twice:\n  %s" % "\n  ".join(duplicates[:5]))


@finding("1.2", "one cross-store search raises the ledger's query count by exactly one")
def _(d):
    # The decision this encodes: a cross-store query writes one ledger row per
    # contributing store, all sharing one query id, and "queries recorded"
    # counts distinct query ids. So the count moves with what the agent did,
    # never with how many stores happen to be open.
    #
    # Both stores are built here rather than hoped for. This drive gets its own
    # empty store home, so when this check runs nothing is open at all and a
    # skip would quietly retire a P1 gate. Two corpora, because indexing one
    # directory twice makes one store and one store shows no multiplier.
    indexed_fixture(d, d.fixtures.small())
    indexed_fixture(d, d.fixtures.second())

    open_stores = [row for row in stores(d) if not row.get("unopened")]
    if len(open_stores) < 2:
        fail(
            "two fixture corpora were indexed and the daemon reports %d open "
            "store(s): %s. The cross-store ledger count cannot be asserted "
            "against one store."
            % (len(open_stores), ", ".join(sorted(r["name"] for r in open_stores)))
        )

    before = d.api("/api/ledger")["queries"]
    d.api("/api/search?query=release%20record%20sealed%20immutable&k=8")
    # The write is on the request's own path, but the ledger read below is a
    # separate connection; one short settle beats a retry loop nobody reads.
    time.sleep(1.0)
    after = d.api("/api/ledger")["queries"]

    if after != before + 1:
        fail(
            "one search across %d stores moved QUERIES RECORDED from %d to %d. "
            "A cross-store query is one query id, so the count should have "
            "risen by exactly 1, not by the number of open stores."
            % (len(open_stores), before, after)
        )

    # And the Ledger page counts what the route counts: the KPI tile is the
    # number a person reads.
    d.open_view("ledger")
    tile = d.eval(
        "(() => { const k = [...document.querySelectorAll('#main .kpi')].find(k =>"
        " /queries recorded/i.test((k.querySelector('.eyebrow') || {}).textContent || ''));"
        " return k ? (k.querySelector('.v') || {}).textContent : null; })()"
    )
    if tile is None:
        fail("the Ledger page has no Queries recorded tile")
    shown = int(re.sub(r"[^\d]", "", tile) or "-1")
    now = d.api("/api/ledger")["queries"]
    if shown != now:
        fail("the Queries recorded tile reads %r while /api/ledger counts %d" % (tile, now))


@finding("1.3", "a URL fetched into a store is indexed, not left orphaned")
def _(d):
    store_name = indexed_fixture(d, d.fixtures.small())
    before = store_named(d, store_name)["files"]

    status, answer = d.api_result(
        "/api/add", method="POST", body={"url": "https://example.com", "store": store_name}
    )
    if status != 200:
        message = json.dumps(answer)
        if any(word in message.lower() for word in ("dns", "resolve", "network", "timed out", "connect")):
            skip("this machine cannot reach the public internet: %s" % message[:160])
        fail("fetching a URL into %s was refused with %d: %s" % (store_name, status, message[:300]))

    final = wait_for_run(d, store_name)
    want("the run a URL fetch starts", str(final["status"]).lower(), "done")
    after = store_named(d, store_name)["files"]
    if after != before + 1:
        fail(
            "the page was fetched but not indexed: %s went from %d files to %d. "
            "The whole point of `add` is to make the URL searchable."
            % (store_name, before, after)
        )


@finding("1.4", "a finished run offers no Pause or Stop, and leaves the live area")
def _(d):
    # From 0.35.0 a run's controls live on its store's Runs tab: a live run is a
    # card with Pause and Stop, and a finished one is a History row with none.
    #
    # The old check also required a finished card to offer Remove.
    # 0.35.0 owner decision (v6 as drawn): finished runs are not dismissed by hand any more —
    # they leave the live area by themselves and are kept, newest first, in a
    # History that survives a restart. What the finding was about is unchanged
    # and asserted in full: nothing about a run that is over offers to pause it
    # or stop it, and the route refuses a stop that would latch the store.
    store_name = indexed_fixture(d, d.fixtures.small())
    d.open_view("store/%s/runs" % store_name)
    d.wait_for(
        "document.querySelectorAll(%s).length > 0" % json.dumps(HIST_ROW),
        what="a History row for the finished run on %s" % store_name,
    )
    if d.eval("document.querySelectorAll(%s).length" % json.dumps(LIVE_CARD)):
        live = [r for r in d.api("/api/index/runs")["runs"]
                if r["store"] == store_name and r["status"] not in TERMINAL]
        # The page draws from its last poll; a short follow-up run (the
        # watcher's) can finish between that poll and this read. What the
        # finding asks is that the card leaves once the run is over, so it gets
        # a few polls to do so.
        if not live:
            try:
                d.wait_for("document.querySelectorAll(%s).length === 0" % json.dumps(LIVE_CARD),
                           timeout=10, what="the finished run's card to leave the live area")
            except cdp.ProtocolError:
                pass
        if not live and d.eval("document.querySelectorAll(%s).length" % json.dumps(LIVE_CARD)):
            fail(
                "%s has no run going and its Runs tab still draws a live run card. "
                "A run that is over belongs in History, with nothing to pause or stop."
                % store_name
            )

    # Only what the History card offers a person, the row toggles aside.
    offered = d.eval(
        """
        (() => {
          const cards = [...document.querySelectorAll('#main .card')];
          const hist = cards.find(c => /history/i.test((c.querySelector('.card-t') || {}).textContent || ''));
          if (!hist) return null;
          return [...hist.querySelectorAll('button')]
            .filter(b => !b.hidden && b.offsetParent !== null && !b.classList.contains('hist-row'))
            .map(b => (b.innerText || '').trim());
        })()
        """
    )
    if offered is None:
        fail("the Runs tab of %s has no History card for its finished run" % store_name)
    live = [b for b in offered if b.lower() in ("pause", "stop", "stop…", "resume")]
    if live:
        fail(
            "a finished run on %s still offers %s. A run that is over has nothing "
            "to pause or stop." % (store_name, ", ".join(live))
        )

    # And the route agrees, so a stale page cannot latch the store either.
    status, answer = d.api_result(
        "/api/index/control", method="POST", body={"store": store_name, "action": "stop"}
    )
    if status == 200:
        fail(
            "POST /api/index/control stop on a store whose run is already over "
            "answered 200. It should refuse, because the latch it leaves is what "
            "killed the store's next run in finding 1.3. Saw: %s" % json.dumps(answer)[:200]
        )


@finding("1.5", "the live chunk counter never goes backwards during a run")
def _(d):
    run_id, store_name = start_index(d, d.fixtures.bulk())
    high = 0
    samples = []
    deadline = time.time() + 900
    final = None
    while time.time() < deadline:
        # The run this check started, by id. "The store's newest run" stopped
        # being that run in 0.28.0: a watcher burst of more than 32 files is
        # admitted as a run of its own, with its own counter from zero, and
        # following it read as this run's counter falling to 0.
        run = run_by_id(d, run_id)
        if run is None:
            time.sleep(0.5)
            continue
        chunks = run.get("chunks") or 0
        samples.append((run.get("indexed"), chunks))
        if chunks < high:
            fail(
                "the live chunk counter fell from %d to %d part way through the "
                "run (at %s files). It reports the run total, so it can only "
                "climb; a per-flush batch window shown in its place makes a long "
                "run look like it is losing work. Samples: %s"
                % (high, chunks, run.get("indexed"), samples[-6:])
            )
        high = max(high, chunks)
        if str(run.get("status", "")).lower() in TERMINAL:
            final = run
            break
        time.sleep(1.0)

    if final is None:
        fail("the bulk run never finished; %d samples taken" % len(samples))
    held = store_named(d, store_name)["chunks"]
    if final.get("chunks") != held:
        fail(
            "the run finished reporting %s chunks and the store holds %s. The "
            "final number was right in 0.20.1 and is the one thing that must "
            "stay right." % (final.get("chunks"), held)
        )


@finding("1.6", "a run's totals agree with the store it wrote")
def _(d):
    # The ambiguity: the findings explain the under-report by a race with the
    # watcher, which had already embedded two of three files before the run
    # acquired the writer. Both a fix that makes the run count that work and a
    # fix that keeps the watcher off a store with a queued run satisfy the
    # finding's complaint, which is that the card and the store disagree. The
    # assertion is therefore on the agreement, not on the mechanism.
    #
    # And the agreement is with what the store *gained*, not with what it holds.
    # A run over a corpus that is already indexed correctly reports "0 indexed,
    # N unchanged, 0 chunks", and asserting its `indexed` against the store's
    # total made a correct run look like the under-reporting one. So the store
    # is measured on both sides of the run and the deltas are what the run has
    # to match. On a full drive this is the first run over `small`, so the store
    # does not exist yet and the deltas are the whole corpus; on `--only 1.6`
    # after an earlier drive they are zero, and zero indexed against zero gained
    # is the same assertion.
    path = d.fixtures.small()
    before = {row["name"]: row for row in stores(d)}

    run_id, store_name = start_index(d, path)
    run = wait_for_run(d, store_name, run_id=run_id)

    was = before.get(store_name) or {"files": 0, "chunks": 0}
    now = store_named(d, store_name)
    gained_files = now["files"] - was["files"]
    gained_chunks = now["chunks"] - was["chunks"]

    if run.get("indexed") != gained_files:
        fail(
            "the run says it indexed %s files and %s went from %d to %d — a "
            "gain of %d. A run that under-reports makes the user's own index run "
            "look like it did almost nothing."
            % (run.get("indexed"), store_name, was["files"], now["files"], gained_files)
        )
    if run.get("chunks") != gained_chunks:
        fail(
            "the run says %s chunks and %s went from %d to %d — a gain of %d"
            % (run.get("chunks"), store_name, was["chunks"], now["chunks"], gained_chunks)
        )

    # And the run's History row on the store's page says the same numbers the
    # route does, so the page cannot under-report on its own either.
    d.open_view("store/%s/runs" % store_name)
    newest = run_for(d, store_name)
    if newest_history_row(d, newest) is None:
        fail(
            "the newest History row on %s reads %r and the daemon's newest run "
            "indexed %s files into %s chunks"
            % (store_name, d.eval("(document.querySelector(%s) || {}).innerText || ''" % json.dumps(HIST_ROW)),
               newest.get("indexed"), newest.get("chunks"))
        )


@finding("1.7", "LAST WRITE is a real time for a store that has been written")
def _(d):
    store_name = indexed_fixture(d, d.fixtures.small())
    for row in stores(d):
        if row.get("unopened"):
            continue
        if row["files"] > 0 and not row.get("last_write"):
            fail(
                "%s holds %d files and reports no last write. 'Never' meant 'not "
                "during this daemon session', which is not what the column says: "
                "a store that plainly has been written must carry the time it was."
                % (row["name"], row["files"])
            )

    # The Stores list's WRITTEN column, row by row: no store holding files may
    # read "never" there.
    d.open_view("stores")
    stores_widest(d)
    cells = d.eval(
        "[...document.querySelectorAll('#main .gl-row.gl-stores')].map(r => ({"
        " name: ((r.querySelector('.cellname .a') || {}).firstChild || {}).textContent,"
        " written: ((r.querySelector('.c-written') || {}).textContent || '').trim()}))"
    )
    if not cells:
        fail("the Stores list draws no rows, so its WRITTEN column cannot be read")
    files = {row["name"]: row["files"] for row in stores(d)}
    wrong = [c for c in cells if (c["name"] or "").strip() in files
             and files[(c["name"] or "").strip()] > 0 and re.search(r"\bnever\b", c["written"], re.I)]
    if wrong:
        fail(
            "the Stores list still prints 'never' under WRITTEN for %s, which "
            "hold files" % ", ".join(c["name"].strip() for c in wrong)
        )
    if store_name not in [(c["name"] or "").strip() for c in cells]:
        fail("the Stores list does not show %s at all" % store_name)


@finding("1.8", "ledger and portal timestamps carry the same zone, and say which")
def _(d):
    # The ambiguity: the finding says neither surface is labelled and the two
    # disagree; it does not say which zone wins. This asserts the property the
    # finding actually complains about — that a portal time and a ledger time
    # can be correlated — by requiring both to name their offset. A fix that
    # labels both as UTC would satisfy it too; this release made both local.
    #
    # Each row carries an `at` (epoch seconds, for sorting) and a `when` that
    # is the rendered timestamp, so the assertion is on `when`: an epoch is
    # already unambiguous and asserting on it would prove nothing about what
    # either surface shows a person.
    #
    # A row to read, made rather than waited for. This drive gets its own empty
    # store home, so nothing has been retrieved when this check runs and a skip
    # would quietly retire a P1 gate.
    indexed_fixture(d, d.fixtures.small())
    d.api("/api/search?query=release%20record%20sealed%20immutable&k=8")
    # The ledger write is on the search request's own path; the read below is a
    # separate connection. One short settle beats a retry loop nobody reads.
    time.sleep(1.0)

    ledger = d.api("/api/ledger")
    rows = ledger.get("rows")
    if rows is None:
        fail(
            "/api/ledger returns no rows, so the portal cannot show the ledger "
            "at all (see finding 3.16) and its timestamps cannot be compared "
            "with the CLI's"
        )
    if not rows:
        fail(
            "a search was just run through /api/search and /api/ledger lists no "
            "rows, so nothing was recorded. The ledger is what makes a retrieval "
            "auditable; an empty one is not a timestamp problem, it is a missing "
            "record."
        )

    stamped = re.compile(r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} [+-]\d{2}:\d{2}$")
    offsets = set()
    for row in rows[:20]:
        when = str(row.get("when") or "")
        if not stamped.match(when):
            fail(
                "a ledger row's `when` is %r, which is not a timestamp naming its "
                "offset. The CLI printed UTC and the portal printed local time "
                "with no marker on either, so a portal event could not be lined "
                "up with a ledger row." % when
            )
        offsets.add(when[-6:])

    # And the portal carries that string rather than a clock of its own: the
    # browser's own time was the other half of the disagreement.
    #
    # 0.35.0 owner decision (v6 as drawn): the Retrievals table prints a short
    # clock — "today 14:02" — and hangs the row's full `when`, offset and all,
    # on that cell as its tooltip. So the assertion is that the newest row's
    # cell carries `when` exactly, and that the clock it prints is that
    # timestamp's own hours and minutes, not a second clock that could drift.
    d.open_view("ledger/retrievals")
    shown = str(rows[0]["when"])
    cell = d.eval(
        "(() => { const c = [...document.querySelectorAll('#main table tbody tr td [data-tip]')]"
        ".find(e => e.getAttribute('data-tip') === %s);"
        " return c ? {tip: c.getAttribute('data-tip'), text: (c.textContent || '').trim()} : null; })()"
        % json.dumps(shown)
    )
    if cell is None:
        fail(
            "the newest ledger row is timestamped %r and no row of the Retrievals "
            "table carries it. The page used to render the browser's own clock, "
            "which is how the same retrieval read 13:40:32 in one place and "
            "19:00:18 in the other." % shown
        )
    if shown[11:16] not in cell["text"]:
        fail(
            "the Retrievals row for %r prints %r, which is not that timestamp's "
            "own %s" % (shown, cell["text"], shown[11:16])
        )

    # The CLI is the other reader of the same rows, and the finding is about
    # the two disagreeing, so it is asked directly rather than assumed.
    try:
        printed = fixtures.run([fixtures.semlith_bin(), "ledger"], timeout=120)
    except fixtures.FixtureError as error:
        skip("the binary under test could not be run: %s" % error)

    clocks = re.findall(r"\b\d{2}:\d{2}:\d{2} [+-]\d{2}:\d{2}\b", printed)
    bare = re.findall(r"\b\d{2}:\d{2}:\d{2}\b", printed)
    if not bare:
        skip("`semlith ledger` printed no rows from the fleet this shell can see")
    if not clocks:
        fail(
            "`semlith ledger` prints %s and names no offset on any of them, so "
            "its rows still cannot be lined up with the portal's."
            % ", ".join(repr(t) for t in bare[:3])
        )
    printed_offsets = {clock[-6:] for clock in clocks}
    if printed_offsets != offsets:
        fail(
            "`semlith ledger` prints times at offset %s and /api/ledger's rows "
            "carry %s. One dataset, two clocks, which is the whole of this finding."
            % (", ".join(sorted(printed_offsets)), ", ".join(sorted(offsets)))
        )


# ==========================================================================
# P2 — a feature does not work
# ==========================================================================


@finding("2.1", "a folder holding a .semlith store can be adopted through the portal")
def _(d):
    folder = d.fixtures.adoptme()
    status, answer = d.api_result("/api/adopt", method="POST", body={"path": folder})
    if status != 200:
        fail(
            "adopting %s was refused with %d: %s\nThe folder holds a valid "
            "`.semlith` built by the binary under test, and the picker cannot "
            "reach a dot-directory, so the parent has to be accepted."
            % (folder, status, json.dumps(answer)[:300])
        )

    # Not `/api/stores`: the daemon opened its stores at startup and holds
    # their locks, so a store adopted now joins on its next start — which is
    # what `restart_required` in the answer says. Asserting it is in the list
    # already would be asserting the opposite of the documented behaviour.
    want("the adopt's restart_required", answer.get("restart_required"), True)
    name = answer.get("name")
    if not name:
        fail("the adopt answered 200 with no store name: %s" % json.dumps(answer)[:300])

    # So the two halves of a successful adopt are asserted directly: the
    # registry gained the store, and the folder no longer holds it.
    registry_path = os.path.join(
        os.environ.get("SEMLITH_HOME") or os.path.join(os.path.expanduser("~"), ".semlith"),
        "registry.json",
    )
    try:
        with open(registry_path, "r", encoding="utf-8") as handle:
            registry = json.load(handle)
    except (OSError, ValueError) as error:
        fail("the adopt answered 200 and %s could not be read: %s" % (registry_path, error))

    entry = (registry.get("stores") or {}).get(name)
    if entry is None:
        fail(
            "the adopt registered %r and it is not in %s. Nothing else records "
            "that the store exists, so it would not come back on the next start. "
            "Registered: %s"
            % (name, registry_path, ", ".join(sorted(registry.get("stores") or {})) or "nothing")
        )
    roots = [normalise(root) for root in entry.get("roots") or []]
    if normalise(folder) not in roots:
        fail(
            "%s was adopted from %s and its registry entry covers %s instead. The "
            "root defaults to the directory the store sat in, which is the corpus "
            "it was indexing." % (name, folder, ", ".join(roots) or "nothing")
        )

    left = os.path.join(folder, ".semlith")
    if os.path.exists(left):
        fail(
            "the adopt answered 200 and %s is still there. Adopting moves the "
            "store into the home; a copy left behind is a second store of the "
            "same corpus that nothing will ever write to again." % left
        )


@finding("2.2", "the URL panel names its own store, and a private address is refused as one")
def _(d):
    # From 0.35.0 a URL is added in the store wizard's Sources step, which is
    # always for one named store: opened from a store's page it reads "Add
    # sources to <store>" across the top and NAME <store> in its summary. That
    # is the panel naming its own store — the store a fetch lands in is never
    # read from a control in some other group, which is what went wrong.
    target = a_store(d)
    open_wizard_for(d, target)
    wizard_mode(d, "Add a URL")
    seen = d.eval(
        """
        (() => {
          const field = [...document.querySelectorAll('.wz-body input')]
            .find(i => /web address/i.test(i.placeholder || ''));
          const top = (document.querySelector('header.top') || {}).innerText || '';
          const name = [...document.querySelectorAll('.wz-rail .sum-row')]
            .find(r => /^NAME$/i.test(((r.querySelector('.k') || {}).textContent || '').trim()));
          return {field: !!field, top,
                  name: name ? ((name.querySelector('.v') || {}).textContent || '').trim() : null};
        })()
        """
    )
    if not seen["field"]:
        fail("the wizard's Add a URL panel has no address field")
    if ("Add sources to %s" % target) not in seen["top"]:
        fail(
            "the wizard the URL panel sits in does not name the store it adds "
            "to; its header reads %r" % seen["top"][:200]
        )
    want("the store the wizard's summary names", seen["name"], target)
    press_text(d, "header.top button", "Cancel", "leaving the wizard")

    # With a store named, the request reaches URL validation, so the privacy
    # refusal for a private address is actually exercised.
    status, answer = d.api_result(
        "/api/add",
        method="POST",
        body={"url": d.portal_url + "/", "store": target},
    )
    message = json.dumps(answer).lower()
    if status == 200:
        fail("fetching a private loopback address was accepted; it must be refused")
    if "store" in message and "private" not in message and "local" not in message:
        fail(
            "fetching a private address answered %d with a store-selection error "
            "(%s). With a store named, the refusal must be the privacy one."
            % (status, json.dumps(answer)[:200])
        )


@finding("2.3", "a store's own Files tab lists that store's files and no other's")
def _(d):
    # Two stores with files, built rather than hoped for: telling a store filter
    # from no filter needs something to filter out, and this drive starts
    # against an empty store home.
    #
    # The finding: "Open in Files" from a store's menu landed on an unfiltered
    # Files page, leaving no way anywhere in the portal to look at one store's
    # files. From 0.35.0 the store's menu Open — or a click on its row — goes
    # to the store's own page, and its Files tab is that store's files.
    target = indexed_fixture(d, d.fixtures.small())
    indexed_fixture(d, d.fixtures.second())

    d.open_view("stores")
    stores_widest(d)
    d.wait_for("!!(%s)" % store_row_js(target), what="the %s row on the Stores list" % target)
    # Opened and chosen in one tick: the Stores list repaints from the live
    # poll, so a menu opened in one CDP call and clicked in the next can be
    # hanging off a row that has since been replaced.
    opened = d.eval(
        """
        (() => {
          const row = %s;
          if (!row) return 'no row for the store';
          const kebab = row.querySelector('button[aria-haspopup]');
          if (!kebab) return 'the row carries no actions control';
          kebab.click();
          const item = [...document.querySelectorAll('.menu .menu-item')]
            .find(b => (b.textContent || '').trim() === 'Open');
          if (!item) return 'the row menu opened with no Open item';
          item.click();
          return 'opened';
        })()
        """
        % store_row_js(target)
    )
    if opened != "opened":
        fail("could not drive the %s row's menu to Open: %s" % (target, opened))
    d.wait_for(
        "location.hash === %s && ((document.querySelector('#main .sd-name') || {}).textContent || '') === %s"
        % (json.dumps("#/store/%s" % target), json.dumps(target)),
        what="the row menu's Open to land on %s's own page" % target,
    )
    press_text(d, "#main .tabs .tab", "Files", "the store page's Files tab")
    d.wait_for(
        "location.hash === %s && document.querySelectorAll('#main table tbody tr').length > 0"
        % json.dumps("#/store/%s/files" % target),
        what="the Files tab of %s, with its rows drawn" % target,
    )

    # The store's roots, and its own directory: a URL fetched into a store is
    # kept in that store's downloads folder, by design, and is that store's.
    row = store_named(d, target)
    roots = [normalise(r["path"]) for r in row.get("roots") or []] + [normalise(row["dir"])]
    shown = d.eval(
        "[...document.querySelectorAll('#main table tbody tr td [data-tip]')]"
        ".map(e => e.getAttribute('data-tip')).filter(Boolean)"
    )
    if not shown:
        fail("the Files tab of %s drew rows with no full path on any of them" % target)
    wrong = [p for p in shown if not any(normalise(p).startswith(r + "/") for r in roots)]
    if wrong:
        fail(
            "the Files tab of %s lists files from outside that store's roots "
            "(%s), so it is not one store's files. First stray: %s"
            % (target, ", ".join(roots), wrong[0])
        )
    total = d.api("/api/files?store=%s&limit=1" % urllib.parse.quote(target))["total"]
    foot = text_of(d, "#main .card-foot .grow", "the Files table's count")
    if not re.search(r"of %s\b" % re.escape("{:,}".format(total)), foot):
        fail(
            "the Files tab of %s counts %r while /api/files holds %d files for "
            "that store" % (target, foot, total)
        )


@finding("2.4", "with every edge-kind chip off, the graph draws no edges and says so")
def _(d):
    a_store(d)
    d.open_view("graph")

    def summary_edges():
        match = re.search(r"([\d,]+)\s+edges", text_of(d, ".g-foot", "the graph's foot"))
        if not match:
            fail("the graph's foot no longer prints an edge count")
        return int(match.group(1).replace(",", ""))

    base = summary_edges()
    chips = d.eval("document.querySelectorAll('.g-bar .seg button[aria-pressed]').length")
    if not chips:
        skip("this store's graph has no edge-kind chips to toggle")

    d.eval(
        "[...document.querySelectorAll('.g-bar .seg button[aria-pressed=\"true\"]')].forEach(c => c.click())"
    )
    pause(d, 800)

    off = summary_edges()
    drawn = d.eval("document.querySelectorAll('.g-live svg.edges line').length")
    if off != 0 or drawn != 0:
        fail(
            "with all %d edge-kind chips off the foot says %d edges and the "
            "canvas draws %d (unfiltered: %d). 'No filter selected' must mean no "
            "edges, not every edge the chips do not name, or the summary asserts "
            "a number that does not describe what is drawn." % (chips, off, drawn, base)
        )
    # Put the chips back for the checks after this one.
    d.eval(
        "[...document.querySelectorAll('.g-bar .seg button[aria-pressed=\"false\"]')].forEach(c => c.click())"
    )


@finding("2.5", "the folder picker can be pointed somewhere other than $HOME")
def _(d):
    # The finding: "Projects under a folder…" opened at $HOME with no Up and no
    # clickable folders, so a monorepo anywhere else was out of reach. From
    # 0.35.0 the one folder picker is the wizard's Browse folders; adding a
    # folder holding several repositories offers "Keep together" or "One store
    # each", which is what the projects picker was for.
    monorepo = d.fixtures.monorepo()
    open_wizard_for(d, a_store(d))
    wizard_mode(d, "Browse folders")
    scope = ".wz-body"
    browse_where(d, scope)

    has_up = d.eval(
        "[...document.querySelectorAll('.wz-body .browse-head button')]"
        ".some(b => (b.innerText || '').trim() === 'Up')"
    )
    if not has_up:
        fail(
            "the wizard's folder picker has no Up control, so it cannot leave "
            "the folder it opened on"
        )

    # Navigation is asserted by doing it: walking down to the monorepo is the
    # whole of "somewhere other than $HOME", and every step of it is a click on
    # a folder name that used to render as an unclickable SPAN.
    descend(d, scope, monorepo)
    want("the folder the picker reached", browse_where(d, scope), tilde(d, monorepo))

    # And Up goes back up, which is the control the finding is named for.
    press_text(d, ".wz-body .browse-head button", "Up", "the picker's Up")
    d.wait_for(
        "((document.querySelector('.wz-body .browse-head .dir') || {}).innerText || '').trim() === %s"
        % json.dumps(tilde(d, os.path.dirname(monorepo))),
        what="Up to leave %s" % monorepo,
    )
    descend(d, scope, monorepo)

    offered = texts_of(d, ".wz-body .bitem .name")
    for expected in ("repo-one", "repo-two"):
        if expected not in offered:
            fail("the picker at %s does not offer %s: it lists %s"
                 % (monorepo, expected, ", ".join(offered) or "nothing"))

    # Use this folder adds it, and the wizard says it holds two repositories
    # and offers to keep them together or give each its own store.
    press_text(d, ".wz-body .browse-head button", "Use this folder", "Use this folder")
    d.wait_for(
        "/holds 2 repositories/.test((document.querySelector('.wz-body .notice') || {}).innerText || '')",
        timeout=20,
        what="the wizard to say the folder it was given holds 2 repositories",
    )

    # And the daemon agrees about which of them are repositories, so the page
    # is not listing plain subfolders and calling them projects.
    listing = d.api("/api/projects?path=%s" % urllib.parse.quote(monorepo))
    if not listing.get("repositories"):
        fail(
            "/api/projects says nothing under the monorepo fixture is a "
            "repository, though the fixture builds two with git"
        )
    found = {row.get("name") for row in listing.get("projects") or []}
    for expected in ("repo-one", "repo-two"):
        if expected not in found:
            fail("the projects listing for the monorepo fixture is missing %s: saw %s"
                 % (expected, sorted(n for n in found if n)))
    press_text(d, "header.top button", "Cancel", "leaving the wizard")


@finding("2.6", "finished runs leave the live area by themselves, and the active run sorts first")
def _(d):
    # The finding: finished runs could not be dismissed and buried the live one
    # under them. 0.35.0 owner decision (v6 as drawn): there is nothing to
    # dismiss by hand — a run that finishes leaves the live area of its store's
    # Runs tab for History on its own, and History is kept across restarts. The
    # half of the finding that is about order is asserted as before: a live run
    # is the first thing on the tab, above every finished one, and the header's
    # run pill names it from any page.
    finished = indexed_fixture(d, d.fixtures.unique("solo"))
    d.open_view("store/%s/runs" % finished)
    d.wait_for(
        "document.querySelectorAll(%s).length > 0 && document.querySelectorAll(%s).length === 0"
        % (json.dumps(HIST_ROW), json.dumps(LIVE_CARD)),
        timeout=30,
        what="the finished run on %s to be in History and not on a live card" % finished,
    )

    run_id, busy = start_index(d, d.fixtures.unique("busy", count=400))
    try:
        d.open_view("store/%s/runs" % busy)
        try:
            d.wait_for(
                "(() => { const host = document.querySelector('#main .page > .stack:last-child')"
                " || document.querySelector('#main .stack');"
                " const first = host && host.firstElementChild;"
                " return !!first && first.classList.contains('blue-edge'); })()",
                timeout=30,
                what="the live run on %s to be the first card on its Runs tab" % busy,
            )
        except cdp.ProtocolError:
            run = run_by_id(d, run_id)
            if run and run.get("status") in TERMINAL:
                skip("the run over %s finished before the ordering could be observed" % busy)
            fail(
                "a run that is still going (%s, %s) is not the first card on its "
                "store's Runs tab" % (busy, run and run.get("status"))
            )
        pill = d.eval(
            "(() => { const p = document.querySelector('header.top .run-pill');"
            " return p && !p.hidden ? (p.innerText || '').trim() : null; })()"
        )
        run = run_by_id(d, run_id)
        if run and run.get("status") not in TERMINAL and (not pill or busy not in pill):
            fail("the header's run pill reads %r while %s is running" % (pill, busy))
    finally:
        stop_quietly(d, busy)


@finding("2.7", "lang:, path: and budget re-run the query like every other filter")
def _(d):
    a_store(d)
    d.open_view("search")
    d.type(SEARCH_BOX, "release record sealed immutable")
    d.press("Enter")
    d.wait_for("document.querySelectorAll(%s).length > 0" % json.dumps(RESULT_CARD),
               what="search results")
    before = texts_of(d, RESULT_CARD + " .hit-head .p")

    # From 0.35.0 the filters are chips under the box: "+ path" opens a field
    # inside its chip, and the scope, lean, results and budget dials re-run the
    # query on a click. The path field applies on its own Enter, the same
    # contract as the graph's scope field (finding 2.8) — never the main query
    # box's Enter, which is the workaround the finding says nobody should need.
    if not exists(d, '#main input[aria-label="Path filter"]'):
        press_text(d, "#main .sr-bar button", "+ path", "the Search page's + path chip")
        d.wait_for("!!document.querySelector('#main input[aria-label=\"Path filter\"]')",
                   what="the path filter field")
    d.type('#main input[aria-label="Path filter"]', "**/*.md")
    d.eval(
        "document.querySelector('#main input[aria-label=\"Path filter\"]')"
        ".dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true}))"
    )
    try:
        d.wait_for(
            "!document.querySelector('#main .spinner') && "
            "[...document.querySelectorAll(%s)].map(e => (e.innerText || '').trim()).join('\\n') !== %s"
            % (json.dumps(RESULT_CARD + " .hit-head .p"), json.dumps("\n".join(before))),
            timeout=15,
            what="the path filter to re-run the query",
        )
    except cdp.ProtocolError:
        pass  # the assertion below says it in the finding's words
    after = texts_of(d, RESULT_CARD + " .hit-head .p")
    if after == before:
        fail(
            "setting the path filter left the results exactly as they were. The "
            "scope and lean dials re-run immediately; the user is looking at "
            "results that contradict the visible filter state."
        )
    stray = [row for row in after if not row.lower().endswith(".md")]
    if stray:
        fail("the results after path **/*.md still include %r" % stray[0][:120])

    # Budget is the other dial the finding named. It re-runs the query too:
    # counted by the requests the page makes, because a budget change need not
    # change which files come back.
    d.eval(
        "(() => { window.__searches = 0; const real = window.fetch.bind(window);"
        " window.fetch = (u, o) => { if (String(u).includes('/api/search')) window.__searches++;"
        " return real(u, o); }; })()"
    )
    d.eval(
        "[...document.querySelectorAll('#main .sr-bar .dial')]"
        ".find(b => /BUDGET/.test(b.innerText || '')).click()"
    )
    try:
        d.wait_for("window.__searches > 0", timeout=10, what="the budget dial to re-run the query")
    except cdp.ProtocolError:
        fail("pressing the budget dial changed the budget and did not re-run the query")


@finding("2.8", "the graph's symbol field says how it is applied, and both ways work")
def _(d):
    # The finding's complaint is that a text field silently waiting for Enter
    # reads as broken beside click-to-apply chips, and it allowed either of two
    # answers: apply as you type, or give the field a visible submit affordance
    # whose description says Enter also works. The product took the second in
    # 0.27.0, so that is what is asserted here — the button, the hint that names
    # the key, and both routes actually applying. From 0.35.0 the field is
    # Explore's "Jump to a symbol" box; the contract did not move with it.
    a_store(d)
    d.open_view("graph")
    field = '.g-bar input[aria-label="Jump to a symbol"]'
    if not exists(d, field):
        fail("no 'Jump to a symbol' field was found on the Graph page")

    affordance = d.eval(
        """
        (() => {
          const field = document.querySelector(%s);
          const box = field.closest('.box') || field.parentElement;
          const button = [...box.querySelectorAll('button')]
            .find(b => (b.innerText || b.getAttribute('aria-label') || '').trim().length > 0);
          const described = field.getAttribute('aria-describedby');
          const hint = described && document.getElementById(described);
          return {button: button ? (button.innerText || button.getAttribute('aria-label') || '').trim() : null,
                  describedby: described,
                  hint: hint ? (hint.textContent || '').trim() : null};
        })()
        """
        % json.dumps(field)
    )
    if not affordance["button"]:
        fail(
            "the graph's symbol field has no button beside it. Every other "
            "filter on the page is a click-to-apply chip, so a text field that "
            "answers only Enter reads as broken — it needs either a submit "
            "affordance or apply-as-you-type."
        )
    if not affordance["hint"] or "enter" not in affordance["hint"].lower():
        fail(
            "the symbol field's button is there and nothing names the key that "
            "also applies it. Its aria-describedby is %r and reads %r; it should "
            "say Enter applies it." % (affordance["describedby"], affordance["hint"])
        )

    def selected():
        return d.eval("((document.querySelector('.g-sel .nm') || {}).innerText || '').trim()")

    store = d.eval("((document.querySelector('.g-bar button[aria-haspopup] .mono') || {}).textContent || '').trim()")
    overview = d.api("/api/graph?limit=25&store=%s" % urllib.parse.quote(store))
    plain = [n["name"] for n in overview.get("nodes") or [] if n.get("name") and "." not in n["name"] and "/" not in n["name"]]
    landed = selected()
    targets = [name for name in plain if name != landed][:2]
    if len(targets) < 2:
        skip("this graph holds fewer than two symbols that can be jumped to by name")

    d.type(field, targets[0])
    press_text(d, ".g-bar .box button", affordance["button"])
    d.wait_for("((document.querySelector('.g-sel .nm') || {}).innerText || '').trim() === %s"
               % json.dumps(targets[0]), what="the button to apply %r" % targets[0])
    d.type(field, targets[1])
    d.press("Enter")
    d.wait_for("((document.querySelector('.g-sel .nm') || {}).innerText || '').trim() === %s"
               % json.dumps(targets[1]), what="Enter to apply %r, as the hint says" % targets[1])


@finding("2.9", "the PWA manifest's icons resolve, and a page load logs no errors")
def _(d):
    status, manifest = d.api_result("/icons/site.webmanifest")
    if status != 200:
        fail("/icons/site.webmanifest answered %d" % status)
    icons = manifest.get("icons") or []
    if not icons:
        fail("the manifest declares no icons")
    for icon in icons:
        src = icon.get("src", "")
        # Resolved the way a browser resolves it: relative to the manifest's
        # own URL, which is what put the second `icons/` in the path.
        resolved = src if src.startswith("/") else "/icons/" + src.lstrip("./")
        code = d.status(resolved)
        if code != 200:
            fail(
                "the manifest's icon %r resolves to %s, which answers %d. The "
                "file is one level up; the fix is to drop the `icons/` prefix or "
                "make the srcs absolute." % (src, resolved, code)
            )

    a_store(d)
    d.clear_console()
    d.open_view("home", fresh=True)
    errors = d.console_errors()
    if errors:
        fail("a page load logged %d console errors:\n  %s" % (len(errors), "\n  ".join(errors[:5])))


@finding("2.10", "an unknown path is 404, not 401")
def _(d):
    # The ambiguity: the daemon checks the token before it routes, so an
    # unknown path is indistinguishable from a guarded one to an anonymous
    # caller. Asserted here with the token held, which is the case the finding
    # reproduced and the one that misleads someone debugging a typo'd asset.
    for path in ("/nope.png", "/icons/nope.png", "/api/nope", "/icons/"):
        code = d.status(path)
        if code != 404:
            fail(
                "%s answered %d with a valid token. A 401 on a path that does "
                "not exist sends anyone debugging a typo'd asset looking at the "
                "token instead of the path." % (path, code)
            )
    for path in ("/style.css", "/app.js", "/logo.svg", "/logo-dark.svg"):
        want("%s is still served" % path, d.status(path), 200)


# ==========================================================================
# P3 — inconsistency and contradiction
# ==========================================================================


@finding("3.1", "a registry entry with no directory is badged, and not offered as a target")
def _(d):
    store_name = indexed_fixture(d, d.fixtures.doomed())
    d.fixtures.remove_doomed()
    # The daemon notices on its next read of the route; one poll is enough.
    d.api("/api/stores")
    time.sleep(1.0)

    row = store_named(d, store_name)
    roots = row.get("roots") or []
    if roots and all(r.get("present") for r in roots):
        skip("the daemon has not noticed the removed root yet")

    d.open_view("stores")
    stores_widest(d)
    # Waited for, not read once. The page is live: it repaints from the poll,
    # so a row read in the same breath as the navigation is a row drawn from
    # the answer before the root was removed.
    read_row = "(() => { const r = %s; return r ? r.innerText : null; })()" % store_row_js(store_name)
    try:
        d.wait_for(
            "(() => { const r = %s; return !!r && /missing|gone|not there|unavailable/i.test(r.innerText); })()"
            % store_row_js(store_name),
            timeout=15,
            what="the Stores row for %s to say its root is gone" % store_name,
        )
    except cdp.ProtocolError:
        pass  # the assertion below says it in the finding's own words
    row_text = d.eval(read_row)
    if row_text is None:
        fail("the Stores list no longer lists %s at all" % store_name)
    if not re.search(r"missing|gone|not there|unavailable", row_text, re.IGNORECASE):
        fail(
            "%s has no root directory and its Stores row shows no badge saying "
            "so: %r. The API already returns the data a badge needs — `present` "
            "is false — and nothing uses it." % (store_name, row_text.replace("\n", " · ")[:160])
        )

    # Not offered as a target. The old Index page offered "add to <store>" for
    # a store whose directory is gone, which is the mechanism behind finding
    # 1.1. From 0.35.0 the controls that would index into a store from its
    # registered roots are its menu's and its page's Re-index; a store with no
    # root left to read must not offer either.
    menu = d.eval(
        """
        (() => {
          const row = %s;
          if (!row) return null;
          row.querySelector('button[aria-haspopup]').click();
          const items = [...document.querySelectorAll('.menu .menu-item')]
            .map(b => ({label: (b.textContent || '').trim(), off: b.disabled}));
          document.dispatchEvent(new KeyboardEvent('keydown', {key: 'Escape'}));
          return items;
        })()
        """
        % store_row_js(store_name)
    )
    offered = [i["label"] for i in (menu or []) if i["label"].startswith("Re-index") and not i["off"]]
    if offered:
        fail(
            "the Stores row menu still offers %s for %s, whose directory does not "
            "exist. That is the same mechanism that produced finding 1.1."
            % (", ".join(offered), store_name)
        )
    d.open_view("store/%s" % store_name)
    live = d.eval(
        "[...document.querySelectorAll('#main button')].some(b => (b.textContent || '').trim() === 'Re-index' && !b.disabled)"
    )
    if live:
        fail("%s's own page offers Re-index although its root directory is gone" % store_name)


def perf_card(d):
    """Settings › Performance's Limits card: its text and every tooltip in it.

    The derivations the old Machine limits panel printed under each field are
    the daemon's `reason` strings, and v6 hangs them on each stepper as its
    tooltip, so a check about what the card says reads both.
    """
    d.open_view("settings")
    seen = d.eval(
        """
        (() => {
          const card = [...document.querySelectorAll('#main .card')]
            .find(c => /^limits$/i.test(((c.querySelector('.card-t') || {}).textContent || '').trim()));
          if (!card) return null;
          const tips = [...card.querySelectorAll('[data-tip]')].map(e => e.getAttribute('data-tip'));
          return card.innerText + '\\n' + tips.join('\\n');
        })()
        """
    )
    if seen is None:
        fail("Settings › Performance has no Limits card")
    return seen


def limit_value(d, label):
    """The value a Settings › Performance stepper shows, by its label."""
    return d.eval(
        "(() => { const r = [...document.querySelectorAll('#main .limit-row')].find(r =>"
        " ((r.querySelector('.k') || {}).textContent || '').trim() === %s);"
        " return r ? ((r.querySelector('.stepper .v') || {}).textContent || '').trim() : null; })()"
        % json.dumps(label)
    )


@finding("3.2", "the machine limits do not contradict themselves")
def _(d):
    # Machine limits moved to Settings › Performance in 0.35.0.
    panel = perf_card(d)

    if re.search(r"\b0\s*GiB is free", panel, re.IGNORECASE):
        fail(
            "the limits say '0 GiB is free' — an integer truncation — beside a "
            "free figure that is not zero:\n%s" % panel[:400]
        )
    reserve = re.search(r"([\d.]+)\s*GiB free minus a ([\d.]+)\s*GiB reserve", panel, re.IGNORECASE)
    if reserve:
        free, held = float(reserve.group(1)), float(reserve.group(2))
        if free < held and not re.search(r"floor|at least|minimum", panel, re.IGNORECASE):
            fail(
                "the limits subtract a %.1f GiB reserve from %.1f GiB free and "
                "conclude it 'allows 1' without saying a floor was applied:\n%s"
                % (held, free, panel[:400])
            )
    if re.search(r"\bMiB\b", panel) and re.search(r"\bMB\b", panel):
        fail(
            "the memory limit is shown in MiB and its help says MB. One card, "
            "two units for one number:\n%s" % panel[:400]
        )


@finding("3.3", "'threads each' recomputes when 'runs at once' changes")
def _(d):
    runs = d.api("/api/index/runs")["limits"]
    if runs["runs_at_once"].get("source") == "set by the environment":
        skip("this machine's runs-at-once is set by the environment, so the page cannot change it")
    # A home with a saved 'threads each' (#153) holds that number whatever
    # 'runs at once' says, so there is no derivation left to recompute.
    if runs["embed_threads"].get("source") == "saved":
        skip("'threads each' is saved on this home, so it does not follow 'runs at once'")

    d.open_view("settings")
    if limit_value(d, "Runs at once") is None:
        fail("Settings › Performance has no Runs at once stepper")
    # The steppers save on each press and the card is repainted from the
    # daemon's answer.
    deadline = time.time() + 20
    while time.time() < deadline:
        value = limit_value(d, "Runs at once")
        if value == "3":
            break
        button = "Raise Runs at once" if int(value or 0) < 3 else "Lower Runs at once"
        d.click('#main button[aria-label="%s"]' % button)
        d.wait_for(
            "(() => { const r = [...document.querySelectorAll('#main .limit-row')].find(r =>"
            " ((r.querySelector('.k') || {}).textContent || '').trim() === 'Runs at once');"
            " return !!r && ((r.querySelector('.stepper .v') || {}).textContent || '').trim() !== %s; })()"
            % json.dumps(value),
            timeout=10,
            what="the Runs at once stepper to move off %s" % value,
        )
    want("runs at once after stepping", limit_value(d, "Runs at once"), "3")

    help_text = d.eval(
        "(() => { const r = [...document.querySelectorAll('#main .limit-row')].find(r =>"
        " ((r.querySelector('.k') || {}).textContent || '').trim() === 'Threads per run');"
        " if (!r) return null; const s = r.querySelector('[data-tip]');"
        " return r.innerText + ' ' + (s ? s.getAttribute('data-tip') : ''); })()"
    )
    if help_text is None:
        fail("no 'Threads per run' row was found")
    if re.search(r"\bbetween 1 run\b", help_text, re.IGNORECASE):
        fail(
            "'runs at once' was set to 3 and the 'threads per run' derivation "
            "still reads %r. The sibling field's derivation is stale." % help_text
        )
    if not re.search(r"\b3 runs?\b", help_text):
        fail("'threads per run' does not mention the 3 runs now configured: %r" % help_text)
    # Left at 3 for 3.4, which reads the same card for the saved value this
    # check has just written.


@finding("3.4", "a value the daemon calls saved, the portal calls saved too")
def _(d):
    limits = d.api("/api/index/runs")["limits"]
    panel = perf_card(d)
    # 3.3 has just written a value, so this run of the drive has a saved one.
    saved = [k for k, v in limits.items() if isinstance(v, dict) and v.get("source") == "saved"]
    if re.search(r"\bderived\b", panel) and saved and not re.search(r"\bsaved\b", panel):
        fail(
            "the limits call a value 'derived' when settings.json holds a saved "
            "one (%s). The daemon logs it as '(saved)', and the two only diverge "
            "when the saved value equals the derived one — exactly when a user "
            "is trying to work out whether their setting took effect:\n%s"
            % (", ".join(saved), panel[:300])
        )
    # Put runs at once back to what this machine derives, so later checks run
    # one at a time as the rest of the drive assumes.
    d.api("/api/index/settings", method="POST",
          body={"runs_at_once": limits["runs_at_once"].get("derived") or 1})


@finding("3.5", "the graph's Called by list holds only call edges")
def _(d):
    # The finding allowed either of two answers: filter the list to `calls`, or
    # rename it to what it holds. From 0.35.0 the list is Explore's "Called by"
    # tab, fed from `/api/symbol`'s callers. Its heading promises calls, so the
    # property is that every edge behind it is one.
    a_store(d)
    d.open_view("graph")
    d.wait_for(
        "/^Called by · \\d+/.test(((document.querySelector('.g-side .tabs .tab') || {}).innerText || '').trim())",
        timeout=30,
        what="Explore's Called by tab for the symbol it opened on",
    )
    head = text_of(d, ".g-side .tabs .tab", "the Called by tab")
    name = text_of(d, ".g-sel .nm", "the selected symbol")
    store = d.eval("((document.querySelector('.g-bar button[aria-haspopup] .mono') || {}).textContent || '').trim()")
    answer = d.api("/api/symbol?name=%s&store=%s&k=40" % (urllib.parse.quote(name), urllib.parse.quote(store)))
    callers = answer.get("callers") or []
    wrong = sorted({c.get("kind") for c in callers if c.get("kind") != "calls"}, key=str)
    if re.match(r"^called by\b", head, re.IGNORECASE) and wrong:
        fail(
            "the %r list for %s holds %s edges, which are not calls. A list "
            "headed Called by holds only calls." % (head, name, ", ".join(map(str, wrong)))
        )
    shown = int(re.search(r"(\d+)", head).group(1))
    want("the Called by count beside %s" % name, shown, len(callers))


@finding("3.6", "the graph opens on something legible and offers fit and zoom")
def _(d):
    a_store(d)
    d.open_view("graph")
    controls = texts_of(d, "#main button")
    if not any(re.search(r"\b(fit|reset|zoom|\+|−|-)\b", c, re.IGNORECASE) for c in controls):
        fail(
            "the graph offers only %s. With no zoom, fit or reset there is no way "
            "to read a view whose labels have collided."
            % ", ".join(c for c in controls if c)[:160]
        )
    # `new` is the worst possible default hub in a Rust codebase: every type
    # has one, so the default view radiates every edge in the store from it.
    selected = d.eval("((document.querySelector('.g-sel .nm') || {}).innerText || '').trim()")
    if selected.lower() == "new":
        fail(
            "the graph still opens on `new`, the single worst hub in a Rust "
            "codebase. Scoped to a real symbol the same view is legible; it is "
            "the default that is unreadable."
        )


@finding("3.7", "scoping the graph cannot raise the edge count")
def _(d):
    # Read from `/api/graph`, not from the page: what the page prints is what
    # survived its edge-kind chips, which whichever check ran before this one
    # may have left switched off. Both numbers come from one route with the
    # same `limit` and the same store, so the only difference between the two
    # answers is the scope.
    LIMIT = 200
    store = a_store(d)

    def counts(data):
        return (len(data.get("nodes") or []), len(data.get("edges") or []))

    whole = d.api("/api/graph?limit=%d&store=%s" % (LIMIT, urllib.parse.quote(store)))
    base_symbols, base_edges = counts(whole)
    if not base_symbols:
        skip("this machine's stores hold no extracted symbols to scope")

    target = next(
        (n["name"] for n in whole["nodes"] if n.get("name") and "." not in n["name"] and "/" not in n["name"]),
        None,
    )
    if not target:
        skip("no symbol in this graph can be scoped to by name")

    scoped_data = d.api("/api/graph?limit=%d&store=%s&name=%s"
                        % (LIMIT, urllib.parse.quote(store), urllib.parse.quote(target)))
    scoped_symbols, scoped_edges = counts(scoped_data)

    def duplicates(data):
        seen, twice = set(), []
        for edge in data.get("edges") or []:
            key = (edge.get("from"), edge.get("to"), edge.get("kind"))
            if key in seen:
                twice.append(key)
            seen.add(key)
        return twice

    for label, data in (("the unscoped graph", whole), ("the scoped graph", scoped_data)):
        twice = duplicates(data)
        if twice:
            fail(
                "%s draws %d edge(s) twice, so its edge count is not a count of "
                "what is on the canvas: %s" % (label, len(twice), twice[:3])
            )

    if scoped_symbols > base_symbols:
        fail("a scoped graph has %d symbols, more than the whole graph's %d"
             % (scoped_symbols, base_symbols))
    if scoped_edges > base_edges:
        fail(
            "scoping the graph raised the edge count from %d to %d over %d "
            "symbols. A subgraph cannot hold more edges than the graph it came "
            "from." % (base_edges, scoped_edges, scoped_symbols)
        )
    pairs = scoped_symbols * (scoped_symbols - 1)
    if scoped_edges > pairs:
        fail(
            "the scoped graph reports %d edges over %d symbols, more than the %d "
            "ordered pairs available" % (scoped_edges, scoped_symbols, pairs)
        )


@finding("3.8", "no fix command is given for a client that is not installed")
def _(d):
    rows = d.api("/api/doctor").get("clients") or d.api("/api/doctor").get("rows") or []
    if not rows:
        skip("the doctor route lists no clients on this machine")
    for row in rows:
        state = doctor_state(row)
        fix = (row.get("repair") or "").strip()
        if state == "not installed" and fix and fix != "—":
            fail(
                "%s is reported as '%s' and still handed %r. Every other "
                "not-installed row gets an em dash." % (row.get("name"), state, fix)
            )
    # And the page agrees: Agents › Health prints "—" under To fix for every
    # client it calls not installed.
    d.open_view("agents/health")
    bad = d.eval(
        "[...document.querySelectorAll('#main table tbody tr')].filter(tr =>"
        " /not installed/.test(tr.cells[1].innerText) && tr.cells[3].innerText.trim() !== '—')"
        ".map(tr => tr.cells[0].innerText + ': ' + tr.cells[3].innerText)"
    )
    if bad:
        fail("Agents › Health hands a fix to a client it calls not installed: %s" % bad[:3])


@finding("3.9", "one client state has one fix command")
def _(d):
    rows = d.api("/api/doctor").get("clients") or []
    if not rows:
        skip("the doctor route lists no clients on this machine")

    by_state = {}
    for row in rows:
        fix = str(row.get("repair") or "").strip()
        if not fix or fix == "—":
            continue
        by_state.setdefault(doctor_state(row), {}).setdefault(fix, []).append(row.get("name"))

    for state, fixes in by_state.items():
        if len(fixes) < 2:
            continue
        # Two commands under one state are allowed when the cell says why: a
        # trailing `# <client> is registered by writing its config file …`
        # explains its own difference.
        unexplained = {fix: names for fix, names in fixes.items() if "#" not in fix}
        if len(unexplained) < 2:
            continue
        listed = "; ".join("%s → %r" % (", ".join(names), fix) for fix, names in unexplained.items())
        fail(
            "the state %r is given %d different remedies with nothing to "
            "explain the difference: %s" % (state, len(unexplained), listed)
        )


def agents_add(d, client):
    """Agents › Add a client with one client picked."""
    d.open_view("agents/add")
    clicked = d.eval(
        "(() => { const b = [...document.querySelectorAll('#main .client-row')].find(b =>"
        " ((b.querySelector('.grow') || {}).textContent || '').trim() === %s);"
        " if (!b) return false; b.click(); return true; })()" % json.dumps(client)
    )
    if not clicked:
        fail("Agents › Add a client lists no %s" % client)
    d.wait_for(
        "((document.querySelector('#main .card-b .big15') || {}).textContent || '').trim() === %s"
        % json.dumps(client),
        what="%s to be the client on show" % client,
    )


@finding("3.10", "the recommended Claude Code HTTP command carries --scope user")
def _(d):
    a_store(d)
    agents_add(d, "Claude Code")
    press_text(d, "#main .tabs .tab", "Terminal", "the Terminal tab")
    d.wait_for("document.querySelectorAll('#main .copyfield .t').length > 0", what="the terminal commands")
    command = None
    for line in texts_of(d, "#main .copyfield .t"):
        if "claude mcp add" in line and "--transport http" in line:
            command = line
            break
    if command is None:
        fail("no `claude mcp add --transport http` command is offered for Claude Code")
    if "--scope user" not in command:
        fail(
            "the copyable HTTP command omits `--scope user`, which the client's "
            "own note says is 'what makes one registration cover every "
            "directory'. Copy it and semlith is registered for one directory "
            "only. Saw: %s" % command
        )


@finding("3.11", "the Agents page renders no literal backticks")
def _(d):
    a_store(d)
    for tab in ("agents", "agents/add", "agents/tools", "agents/health"):
        d.open_view(tab)
        stray = [line.strip() for line in view_text(d).splitlines() if "`" in line]
        if stray:
            fail(
                "backticks are printed as characters on %s, while every other "
                "inline code reference is styled:\n  %s" % (tab, "\n  ".join(stray[:3]))
            )


@finding("3.12", "stopping the MCP endpoint asks first")
def _(d):
    a_store(d)
    d.open_view("agents")
    before = d.api("/api/agents")
    switch = "#main .head button[role=switch]"
    if not exists(d, switch):
        fail("the Agents page has no endpoint switch")
    try:
        d.click(switch)
        pause(d, 600)
        asked = d.modal_open()
        after = d.api("/api/agents")
        if not asked:
            fail(
                "the endpoint switch closed the MCP endpoint with no dialog. Forget "
                "store, Forget file and Stop run all confirm; the one action that "
                "severs every connected agent does not."
            )
        if json.dumps(after.get("endpoint")) != json.dumps(before.get("endpoint")):
            fail("the endpoint changed state before the confirmation was answered")
        d.press("Escape")
    finally:
        # Whatever happened, the endpoint is open again for the checks after this.
        d.api("/api/endpoint", method="POST", body={"open": True})


@finding("3.13", "nothing writes a client's config before what it writes is on screen")
def _(d):
    # The finding: "Write these files" was the primary button and the dry run
    # that showed what it would write was secondary, so the destructive button
    # was reachable without ever seeing the preview.
    #
    # 0.35.0 owner decision (v6 as drawn): there is no separate dry run. Each
    # client registers in one click, and the exact command or file that click
    # writes is printed beside the button before it can be pressed; the wizard's
    # Connect step lists what it will write per client above its Register. So
    # the property asserted is the one the finding was about — the write is
    # never reachable without what it writes being on screen next to it.
    a_store(d)
    d.open_view("agents/add")
    rows = d.eval(
        """
        [...document.querySelectorAll('#main .one-click')].map(r => ({
          what: ((r.querySelector('.t-mono-sm') || {}).textContent || '').trim(),
          button: ((r.querySelector('button') || {}).textContent || '').trim()}))
        """
    )
    clients = [c for c in (d.api("/api/agents").get("doctor") or []) if c.get("present")]
    if not rows:
        if clients:
            fail("Agents › Add a client offers no Register for %s, which is installed" % clients[0]["name"])
        skip("no client is installed on this machine, so nothing can be registered from the page")
    for row in rows:
        if row["button"] in ("Register", "Unregister") and not row["what"]:
            fail("a %s button sits beside no statement of what it writes" % row["button"])


@finding("3.14", "the model notes describe the models they sit beside")
def _(d):
    models = d.api("/api/models")
    rows = models.get("models") if isinstance(models, dict) else models
    by_name = {row.get("name") or row.get("model"): row for row in rows}

    def note_of(name):
        row = by_name.get(name)
        if row is None:
            skip("this build no longer lists the model %s" % name)
        return (row.get("note") or row.get("description") or "").lower()

    for name in ("BGEBaseENV15Q", "GTEBaseENV15", "GTEBaseENV15Q"):
        note = note_of(name)
        if "large" in note:
            fail("%s is a base model and its note calls it large: %r" % (name, note))
    for name in ("GTEBaseENV15", "GTEBaseENV15Q"):
        note = note_of(name)
        if "multilingual" in note:
            fail("%s is the English GTE model and its note calls it multilingual: %r" % (name, note))

    small = note_of("BGESmallENV15")
    if "default" in small:
        fail(
            "BGESmallENV15's note calls it the default English model. semlith's "
            "default is granite, and these notes are presented as semlith's own "
            "statements about models the user is invited to choose between: %r" % small
        )


@finding("3.15", "the model actually in use carries a size")
def _(d):
    # The ambiguity: the finding complains that 43 of 48 rows are empty and
    # that sorting by SIZE hides the five that are not. It does not say every
    # model must gain a size — upstream may not publish one. What it does imply
    # is asserted: the model semlith is running has a size.
    about = d.api("/api/about")
    in_use = about.get("model") or (about.get("embedding") or {}).get("model")
    if not in_use:
        in_use = next(
            (s["model"] for s in (d.api("/api/stores").get("stores") or []) if s.get("model")),
            None,
        )
    if not in_use:
        skip("no open store names the model it was built with")

    models = d.api("/api/models")
    rows = models.get("models") if isinstance(models, dict) else models
    match = next((r for r in rows if (r.get("name") or r.get("model")) == in_use), None)
    if match is None:
        fail("the model in use, %s, is not in the models table at all" % in_use)
    if not match.get("size") and not match.get("bytes"):
        fail(
            "the model actually in use (%s), which is downloaded and which the "
            "Privacy page reports as on disk, shows no SIZE" % in_use
        )
    # The About page's models table and its SIZE sort went in 0.27.0 (7.9
    # asserts it stays gone); the Privacy page's downloads list is where a
    # person reads the model's size now, and 8.8 asserts every size on it.


@finding("3.16", "the Retrieval ledger page shows the ledger")
def _(d):
    a_store(d)
    d.api("/api/search?query=release%20record%20sealed%20immutable&k=8")
    time.sleep(1.0)
    for tab in ("ledger", "ledger/retrievals"):
        d.open_view(tab)
        try:
            d.wait_for("document.querySelectorAll('#main table tbody tr').length > 0", timeout=10,
                       what="rows on %s" % tab)
        except cdp.ProtocolError:
            fail(
                "%s has aggregate tiles, a by-client breakdown and terminal "
                "snippets, and zero rows. The one thing described as a debugging "
                "trail is the one thing the page will not show, and the CLI "
                "prints rows where the portal does not." % tab
            )


@finding("3.17", "the lines tile's caption describes lines")
def _(d):
    # The tiles moved to Stores › Inside the index in 0.35.0.
    a_store(d)
    d.open_view("stores/inside")
    caption = d.eval(
        """
        (() => {
          const tile = [...document.querySelectorAll('#main .kpi')]
            .find(t => /^\\s*LINES\\b/i.test((t.querySelector('.eyebrow') || {}).textContent || ''));
          return tile ? tile.innerText : null;
        })()
        """
    )
    if caption is None:
        fail("no lines tile was found on Stores › Inside the index")
    if re.search(r"readers? in use", caption, re.IGNORECASE):
        fail(
            "the lines tile's caption is 'readers in use' — a fact about format "
            "handlers, not about lines. Saw: %r" % caption.replace("\n", " · ")
        )


@finding("3.18", "the WRITTEN column holds one vocabulary")
def _(d):
    a_store(d)
    d.open_view("stores")
    stores_widest(d)
    values = d.eval(
        "[...document.querySelectorAll('#main .gl-row.gl-stores .c-written')]"
        ".map(c => (c.textContent || '').trim().toLowerCase()).filter(Boolean)"
    )
    if not values:
        fail("no WRITTEN column was found on the Stores list")
    if "not opened" in values:
        fail(
            "'not opened' appears in the WRITTEN column. It is a fact about "
            "whether the daemon holds the store, not a last write — one column, "
            "two vocabularies."
        )
    # The phrases for the absence of a write. "just now" is a time, not an
    # absence, though it has no digit in it.
    phrases = {v for v in values if not re.search(r"\d", v) and v != "just now"}
    if len(phrases) > 1:
        fail("the WRITTEN column uses %d different phrases for the absence of a "
             "write: %s" % (len(phrases), sorted(phrases)))


def files_tab(d, store, glob=""):
    """A store's Files tab, filtered by a path glob or by none.

    The field is always set: the page keeps one filter for every store's Files
    tab, so a glob typed on one store's tab is still applied on the next one's.
    """
    d.open_view("store/%s/files" % store)
    d.wait_for("!!document.querySelector('#main input[aria-label=\"Path or glob\"]')",
               what="the Files tab's path field")
    if d.eval("document.querySelector('#main input[aria-label=\"Path or glob\"]').value") != glob:
        d.type('#main input[aria-label="Path or glob"]', glob)
        pause(d, 700)


@finding("3.19", "a file count respects the filter, and the Forget note goes with the rows")
def _(d):
    page = d.api("/api/files?path=**/*.no-such-extension")
    want("a filter that matches nothing returns no files", page["total"], 0)
    want("a filter that matches nothing spans no formats", page["formats"], 0)
    if page["stores"] != 0:
        fail(
            "a filter matching nothing reports '%d stores' beside '0 files · 0 "
            "formats'. Two of the three numbers respond to the filter; the third "
            "describes the daemon." % page["stores"]
        )

    # The cross-store Files page went in 0.35.0; a store's own Files tab is
    # where files are filtered and forgotten now.
    files_tab(d, a_store(d), "**/*.no-such-extension")
    d.wait_for("document.querySelectorAll('#main table tbody tr').length === 0",
               what="the filtered table to empty")
    if re.search(r"Forget drops (the|a) file's chunks", view_text(d)):
        fail(
            "the Forget footnote is still visible with no rows and no Forget "
            "buttons on the page"
        )


@finding("3.20", "the empty state after a Forget blames the corpus, not the filter")
def _(d):
    store_name = indexed_fixture(d, d.fixtures.unique("forgetme"))
    rows = all_files(d, params="&store=%s" % store_name)
    if not rows:
        skip("the fixture store holds no files to forget")
    target = rows[0]["path"]
    leaf = target.replace("\\", "/").rsplit("/", 1)[-1]

    files_tab(d, store_name, "**/" + leaf)
    d.wait_for("document.querySelectorAll('#main table tbody tr').length > 0", what="the filtered row")
    d.api("/api/forget", method="POST", body={"store": store_name, "path": target})
    # The same filter again, which is what reloads the table.
    d.type('#main input[aria-label="Path or glob"]', "**/" + leaf)
    pause(d, 900)
    body = view_text(d)
    if re.search(r"the filter, not the corpus", body, re.IGNORECASE):
        fail(
            "after forgetting the only row matching the filter, the table says "
            "'The filter, not the corpus — clear it and look again.' The corpus "
            "is exactly why there is nothing there."
        )


@finding("3.21", "a completed Forget says something")
def _(d):
    store_name = indexed_fixture(d, d.fixtures.unique("forgetsays"))
    if not all_files(d, params="&store=%s" % store_name):
        skip("the fixture store holds no files to forget")
    files_tab(d, store_name)
    d.wait_for("document.querySelectorAll('#main table tbody tr').length > 0", what="the files table")
    clicked = d.eval(
        "(() => { const b = [...document.querySelectorAll('#main table tbody tr button')]"
        ".find(b => (b.textContent || '').trim() === 'Forget'); if (!b) return false; b.click(); return true; })()"
    )
    if not clicked:
        fail("no Forget was reachable on the rows of %s's Files tab" % store_name)
    # Confirm, because Forget is one of the actions that asks.
    d.wait_for("!!document.querySelector(%s)" % json.dumps(MODAL), what="the Forget confirm")
    d.modal_press("Forget")
    d.wait_for(
        "[...document.querySelectorAll('.toast, [role=status], [aria-live]')]"
        ".some(el => /forgot/i.test(el.innerText || ''))",
        timeout=15,
        what="the Forget to say something",
    )


@finding("3.22", "the Privacy rules badge agrees with the Privacy scan")
def _(d):
    a_store(d)
    d.open_view("privacy")
    body = view_text(d)
    if re.search(r"\bScan\s*\n?\s*Scan\b|\bCheck now\s*\n?\s*Check now\b", body):
        fail("the Privacy page renders its scan control's words twice in a row")

    scan = d.api("/api/privacy/scan")
    refused = scan.get("findings") or scan.get("files") or scan.get("refused") or []
    if not refused:
        # The badge and the scan can only be caught disagreeing on a machine
        # whose stores are holding something today's rules refuse, and staging
        # one is not within a check's reach. The copy assertion above runs
        # either way.
        skip(
            "no open store is holding a file today's rules would refuse, so "
            "there is no disagreement for the badge to have with the scan"
        )
    if re.search(r"all holding", body, re.IGNORECASE):
        fail(
            "the rules badge reads 'all holding' while the page's own scan finds "
            "%d file(s) that would be refused today." % len(refused)
        )


@finding("3.23", "the theme control can return to following the system")
def _(d):
    # 0.35.0 owner decision (v6 as drawn): the theme control is Light, Dark and
    # System side by side rather than a cycle, and the root always carries the
    # resolved scheme in `data-theme` — "system" writes whichever one the OS
    # prefers, and follows it live. The finding's property is asserted on that:
    # with no stored choice the page follows the system, and once a choice has
    # been made, System puts it back to following, which is the way back the
    # finding said did not exist.
    a_store(d)
    d.eval("try { localStorage.removeItem('semlith-theme'); } catch (e) {}")
    try:
        d.emulate_media({"prefers-color-scheme": "dark"})
        d.open_view("home", fresh=True)
        theme = '.theme button[data-value="%s"]'
        if not exists(d, theme % "system"):
            fail("no System theme control was found on the page")
        want("the System button with nothing stored", d.eval(
            "document.querySelector(%s).getAttribute('aria-pressed')" % json.dumps(theme % "system")), "true")
        want("data-theme with nothing stored and the OS dark",
             d.eval("document.documentElement.getAttribute('data-theme')"), "dark")

        d.click(theme % "light")
        want("data-theme after Light", d.eval("document.documentElement.getAttribute('data-theme')"), "light")
        d.click(theme % "system")
        want("data-theme after System, the OS dark",
             d.eval("document.documentElement.getAttribute('data-theme')"), "dark")
        want("the stored theme after System", d.eval("localStorage.getItem('semlith-theme')"), "system")
        # And it follows the system live, which is what "following" means.
        d.emulate_media({"prefers-color-scheme": "light"})
        d.wait_for("document.documentElement.getAttribute('data-theme') === 'light'", timeout=5,
                   what="System to follow the OS to light")
    finally:
        d.emulate_media({})
        d.eval("try { localStorage.removeItem('semlith-theme'); } catch (e) {}")


@finding("3.24", "the search launcher has one name, and the graph's actions name their own journey")
def _(d):
    # The finding: one destination, three names. Read in two parts from 0.26.1.
    #
    # 0.35.0 owner decision (v6 as drawn): the header's launcher reads "Ask the
    # index a question" and the Search box it opens says how to ask — "Ask in
    # words, or paste an identifier" — so the box's placeholder is guidance, not
    # a second name for the launcher. What stays asserted is that the launcher
    # is one control with one name (its accessible name is the words it shows)
    # that lands on Search with the box ready; and that the graph's actions are
    # named for what they give you rather than echoing the search box.
    a_store(d)
    d.open_view("home")
    launcher = d.eval(
        "(() => { const b = document.querySelector('header.top button.ask'); if (!b) return null;"
        " return {label: b.getAttribute('aria-label'), text: ((b.querySelector('.t') || {}).textContent || '').trim()}; })()"
    )
    if launcher is None:
        fail("the header has no search launcher")
    if launcher["text"] and launcher["label"] != launcher["text"]:
        fail("the launcher shows %r and announces itself as %r; one control, one name"
             % (launcher["text"], launcher["label"]))
    d.click("header.top button.ask")
    d.wait_for("location.hash === '#/search' && !!document.querySelector(%s)" % json.dumps(SEARCH_BOX),
               what="the launcher to land on Search")
    field = d.eval("document.querySelector(%s).placeholder" % json.dumps(SEARCH_BOX))

    d.open_view("graph")
    d.wait_for("document.querySelectorAll('.g-sel .two button').length > 0", what="the graph's actions")
    actions = texts_of(d, ".g-sel .two button")
    echo = [a for a in actions if a in (launcher["label"], field)]
    if echo:
        fail(
            "the graph's action reads %r, the same words as the search box. It is "
            "a different journey and naming it after the box says nothing about "
            "what it gives you." % echo[0]
        )


@finding("3.25", "the URL field's placeholder does not change after a failed attempt")
def _(d):
    open_wizard_for(d, a_store(d))
    wizard_mode(d, "Add a URL")
    field = ".wz-body .box input"
    first = d.eval("(document.querySelector(%s) || {}).placeholder" % json.dumps(field))
    if not first:
        fail("the wizard's Add a URL field carries no placeholder at all")
    d.type(field, "not-a-url")
    d.press("Enter")
    if not d.eval("[...document.querySelectorAll('.wz-body button')].some(b => (b.textContent || '').trim() === 'Fetch' && b.disabled)"):
        fail("Fetch is on offer for 'not-a-url', which is not an https address")
    # Closed and opened again, which is the repaint the old panel changed on.
    wizard_mode(d, "Paste a path")
    wizard_mode(d, "Add a URL")
    second = d.eval("(document.querySelector(%s) || {}).placeholder" % json.dumps(field))
    if second != first:
        fail("the URL field's placeholder changed from %r to %r after a failed attempt" % (first, second))
    press_text(d, "header.top button", "Cancel", "leaving the wizard")


@finding("3.26", "keeping repositories together and giving each its own store are exclusive")
def _(d):
    # The finding: "Projects under a folder…" made each project its own store
    # while the store dropdown beside it could still read "add to proj-one",
    # and nothing reconciled the two. From 0.35.0 a folder holding several
    # repositories offers exactly that choice in the wizard — Keep together, or
    # One store each — as two chips of which one is pressed.
    monorepo = d.fixtures.monorepo()
    open_wizard_for(d, a_store(d))
    wizard_mode(d, "Paste a path")
    d.type(".wz-body .box input", monorepo)
    d.press("Enter")
    d.wait_for("/holds 2 repositories/.test((document.querySelector('.wz-body .notice') || {}).innerText || '')",
               timeout=20, what="the wizard to offer keeping the repositories together or apart")

    def pressed():
        return d.eval(
            "[...document.querySelectorAll('.wz-body .notice button')]"
            ".filter(b => b.getAttribute('aria-pressed') === 'true').map(b => b.textContent.trim())"
        )

    want("the choice pressed to begin with", pressed(), ["Keep together"])
    press_text(d, ".wz-body .notice button", "One store each")
    want("the choice pressed after One store each", pressed(), ["One store each"])
    press_text(d, ".wz-body .notice button", "Keep together")
    want("the choice pressed after Keep together", pressed(), ["Keep together"])
    press_text(d, "header.top button", "Cancel", "leaving the wizard")


# ==========================================================================
# P4 — polish
# ==========================================================================


# Whether a tooltip carries the whole of what a line shows. From 0.35.0 a long
# path is shortened in the middle ("~/a/…/z") and several roots read as the
# first plus "+ N more", so the tooltip is the longer value: every piece the
# line shows must be in it.
COVERS_JS = (
    "((held, full) => { held = (held || '').trim(); full = (full || '').trim();"
    " if (!full || !held) return false; if (held === full) return true;"
    " return full.replace(/ \\+ \\d+ more$/, '').split('\u2026').every(p => held.includes(p.trim())); })"
)


@finding("4.1", "a truncated path carries its full value on hover")
def _(d):
    # A title then, a `data-tip` from 0.35.0: the page's one tooltip, which
    # shows the full value on hover and on keyboard focus. Either carrier is the
    # full value without the DOM inspector, which is what the finding asked for.
    store = a_store(d)
    d.open_view("stores")
    d.wait_for("document.querySelectorAll('#main .gl-row.gl-stores').length > 0", what="the Stores list")
    missing = d.eval(
        "[...document.querySelectorAll('#main .gl-row.gl-stores .cellname .b')].filter(b => {"
        " const t = (b.textContent || '').trim(); if (!/[\\\\/~]/.test(t)) return false;"
        " const h = b.closest('[data-tip], [title]');"
        " return !h || !%s(h.getAttribute('data-tip') || h.getAttribute('title'), t); }).map(b => b.textContent.trim())"
        % COVERS_JS
    )
    if missing:
        fail("%d store root lines on the Stores list carry no full value on hover. First: %r"
             % (len(missing), missing[0]))
    files_tab(d, store)
    d.wait_for("document.querySelectorAll('#main table tbody tr').length > 0", what="the Files table")
    bare = d.eval(
        "[...document.querySelectorAll('#main table tbody tr')].map(r => {"
        " const cell = r.cells[1]; if (!cell) return null;"
        " const h = cell.querySelector('[data-tip], [title]');"
        " const full = h && (h.getAttribute('data-tip') || h.getAttribute('title'));"
        " return full && /[\\\\/]/.test(full) ? null : (cell.innerText || '').trim(); }).filter(Boolean)"
    )
    if bare:
        fail("%d path cells on %s's Files tab have no full path on hover. First: %r"
             % (len(bare), store, bare[0]))


@finding("4.2", "every Files column sorts")
def _(d):
    files_tab(d, a_store(d))
    d.wait_for("document.querySelectorAll('#main table tbody tr').length > 0", what="the files table")
    inert = d.eval(
        """
        [...document.querySelectorAll('#main table th')].map(th => {
          const sortable = th.hasAttribute('aria-sort') || !!th.querySelector('button');
          return sortable ? null : (th.innerText || '').trim();
        }).filter(Boolean)
        """
    )
    if inert:
        fail(
            "these Files columns carry no sort affordance and clicking them does "
            "nothing: %s." % ", ".join(inert)
        )


@finding("4.3", "the code preview shows that it scrolls")
def _(d):
    a_store(d)
    d.open_view("search")
    d.type(SEARCH_BOX, "describe_at_length releases it when the run that acquired it")
    d.press("Enter")
    d.wait_for("!!document.querySelector('#main .sr-detail .lines .l')", timeout=30,
               what="a code preview in the detail panel")
    clipped = d.eval(
        """
        (() => {
          const panes = [...document.querySelectorAll('#main pre, #main .lines, #main .code, #main .preview')];
          const pane = panes.find(p => p.scrollWidth > p.clientWidth + 2);
          if (!pane) return null;
          const style = getComputedStyle(pane);
          const cue = style.overflowX === 'auto' || style.overflowX === 'scroll';
          const fade = !!pane.parentElement.querySelector('.fade, .shadow, .more');
          return {overflowX: style.overflowX, cue, fade};
        })()
        """
    )
    if clipped is None:
        skip("no preview pane in this result set is wider than its box")
    if not clipped["cue"] and not clipped["fade"]:
        fail(
            "the preview clips at the right edge with overflow-x: %s and no fade "
            "or shadow cue." % clipped["overflowX"]
        )


def picker_rows(d, scope):
    """A folder picker's rows, as {name, folder}, from their own `.name`."""
    return d.eval(
        "[...document.querySelectorAll(%s + ' .bitem')].map(r => ({"
        " name: ((r.querySelector('.name') || {}).textContent || '').trim(),"
        " folder: !r.classList.contains('file')})).filter(r => r.name)" % json.dumps(scope)
    )


@finding("4.4", "folder pickers sort case-insensitively")
def _(d):
    cased = d.fixtures.cased()
    open_wizard_for(d, a_store(d))
    wizard_mode(d, "Browse folders")
    descend(d, ".wz-body", cased)
    rows = picker_rows(d, ".wz-body")
    folders = [r["name"] for r in rows if r["folder"]]
    files = [r["name"] for r in rows if not r["folder"]]
    if len(folders) < 2 and len(files) < 2:
        skip("the picker lists fewer than two entries of either kind here")
    for kind, names in (("folders", folders), ("files", files)):
        if names != sorted(names, key=lambda n: n.lower()):
            fail(
                "the folder picker sorts its %s case-sensitively, so every "
                "lowercase entry sinks below every uppercase one. Saw: %s"
                % (kind, ", ".join(names[:8]))
            )
    press_text(d, "header.top button", "Cancel", "leaving the wizard")


@finding("4.5", "a file in a folder picker reads as what it is: selectable")
def _(d):
    # The finding: files were listed in the picker, could not be selected, and
    # rendered at full contrast like the folders that could — what a row looked
    # like did not say what it did.
    #
    # 0.35.0 owner decision (v6 as drawn): a single file is a source in its own
    # right, so the wizard's picker offers files for selection beside folders.
    # The property asserted is the finding's — a row reads as what it does: a
    # file row carries the same checkbox a folder row does, and ticking it
    # selects it.
    cased = d.fixtures.cased()
    open_wizard_for(d, a_store(d))
    wizard_mode(d, "Browse folders")
    descend(d, ".wz-body", cased)
    seen = d.eval(
        """
        (() => {
          const files = [...document.querySelectorAll('.wz-body .bitem.file')];
          const folders = [...document.querySelectorAll('.wz-body .bitem:not(.file)')];
          if (!files.length || !folders.length) return null;
          return {unticked: files.filter(f => !f.querySelector('[role=checkbox]'))
                    .map(f => (f.innerText || '').trim()).slice(0, 3)};
        })()
        """
    )
    if seen is None:
        skip("this folder shows no mix of files and folders")
    if seen["unticked"]:
        fail("these files are listed with no checkbox, so nothing says whether they "
             "can be picked: %s" % ", ".join(seen["unticked"]))
    d.eval("document.querySelector('.wz-body .bitem.file [role=checkbox]').click()")
    d.wait_for("/1 selected/.test((document.querySelector('.wz-body .browse-head') || {}).innerText || '')",
               what="ticking a file to select it")
    press_text(d, "header.top button", "Cancel", "leaving the wizard")


def orphan_tiles(d):
    """KPI rows whose last line holds one tile alone under a fuller line."""
    return d.eval(
        """
        [...document.querySelectorAll('#main .q4, #main .q3')].map(row => {
          const tops = {};
          for (const k of row.children) {
            const t = Math.round(k.getBoundingClientRect().top);
            tops[t] = (tops[t] || 0) + 1;
          }
          const shape = Object.keys(tops).map(Number).sort((a, b) => a - b).map(t => tops[t]);
          return shape;
        }).filter(s => s.length > 1 && s[s.length - 1] === 1 && s[s.length - 2] >= 3)
        """
    )


@finding("4.6", "the stat tiles do not leave one tile alone at tablet width")
def _(d):
    # Five tiles four-and-one then; four tiles to a row everywhere from 0.35.0,
    # so the shape the finding objected to is three-and-one now. Asserted on
    # every page that draws a tile row.
    store = a_store(d)
    d.set_viewport(768, 1024)
    try:
        for route in ("home", "stores/inside", "store/%s" % store, "ledger"):
            d.open_view(route, fresh=True)
            bad = orphan_tiles(d)
            if bad:
                fail("at 768px a tile row on %s breaks %s, leaving one tile alone" % (route, bad[0]))
    finally:
        d.reset_viewport()


@finding("4.7", "a store's root path is reachable on a phone")
def _(d):
    a_store(d)
    d.set_viewport(390, 844, mobile=True)
    try:
        d.open_view("stores", fresh=True)
        roots = [r["path"] for row in stores(d) if not row.get("unopened") for r in row.get("roots", [])]
        if not roots:
            skip("no open store has a registered root to show")
        spelled = set(roots) | {tilde(d, r) for r in roots}
        reachable = d.eval(
            "[...document.querySelectorAll('#main *')].some(el => {"
            " const t = (el.getAttribute('data-tip') || el.getAttribute('title') || '');"
            " const v = el.children.length === 0 ? (el.innerText || '') : '';"
            " return %s.some(r => (t.includes(r) || (v.includes(r) && el.offsetParent !== null))); })"
            % json.dumps(sorted(spelled))
        )
        if not reachable:
            fail(
                "on a 390px Stores list nothing shows or hands over a store's root, "
                "so on a phone you cannot see what a store indexes"
            )
        # Clipped *and* unrecoverable: text cut off with nothing carrying the
        # rest. A line that ends in an ellipsis and hands its full value to the
        # tooltip is the design, and so is a screen-reader-only caption.
        clipped = d.eval(
            """
            [...document.querySelectorAll('#main *')]
              .filter(el => el.children.length === 0 && el.offsetParent !== null)
              .filter(el => !el.closest('.sr-only'))
              .filter(el => el.scrollWidth > el.clientWidth + 2)
              .filter(el => {
                const full = (el.innerText || '').trim();
                const h = el.closest('[data-tip], [title]');
                const held = h ? (h.getAttribute('data-tip') || h.getAttribute('title') || '') : '';
                return !full || !COVERS(held, full);
              })
              .map(el => (el.innerText || '').trim())
              .slice(0, 3)
            """.replace("COVERS", COVERS_JS)
        )
        if clipped:
            fail(
                "text is clipped on the phone build with nothing carrying the rest "
                "of it — no tooltip, no expansion: %s" % ", ".join(clipped)
            )
    finally:
        d.reset_viewport()


@finding("4.8", "store choice does not consume the phone's first screen")
def _(d):
    # Store chips then; one scope menu ("in all stores ▾") from 0.35.0. The
    # property is unchanged: on a 390px screen, what sits above the results
    # leaves most of the first screen to the results.
    a_store(d)
    d.set_viewport(390, 844, mobile=True)
    try:
        d.open_view("search", fresh=True)
        chips = d.eval("document.querySelectorAll('#main .sr-bar .chip[data-store], #main .store-chips .chip').length")
        scope = exists(d, "#main .sr-bar .scope-btn")
        if not scope and chips < 3:
            skip("this page offers no store choice above the results")
        bottom = d.eval("Math.round(document.querySelector('#main .sr-bar').getBoundingClientRect().bottom)")
        if bottom > 844 * 0.5:
            fail(
                "on a 390px screen the search bar and its store choice end at "
                "y=%d, more than half of the first screen, with the results below it" % bottom
            )
    finally:
        d.reset_viewport()


@finding("4.9", "a phone result card is not covered by the summary bar")
def _(d):
    a_store(d)
    d.set_viewport(390, 844, mobile=True)
    try:
        d.open_view("search", fresh=True)
        d.type(SEARCH_BOX, "release record sealed immutable")
        d.press("Enter")
        d.wait_for("document.querySelectorAll(%s).length > 0" % json.dumps(RESULT_CARD),
                   what="search results")
        summary = d.eval(
            """
            (() => {
              const el = document.querySelector('#main .sr-foot')
                || [...document.querySelectorAll('#main *')].find(e => e.children.length === 0
                     && /\\bof\\b.*shown|budget/i.test(e.innerText || ''));
              if (!el) return null;
              const r = el.getBoundingClientRect(), s = getComputedStyle(el);
              return {top: r.top, bottom: r.bottom, fixed: s.position === 'fixed' || s.position === 'sticky'};
            })()
            """
        )
        cards = rects(d, RESULT_CARD)
        if summary is None or not cards:
            skip("no summary bar or no result cards to compare")
        first = cards[0]
        if summary["fixed"] and first["top"] < summary["bottom"] - 1 and first["bottom"] > summary["top"] + 1:
            fail(
                "the first result card spans y=%.0f–%.0f, under a pinned summary "
                "bar at y=%.0f–%.0f, so the card is covered"
                % (first["top"], first["bottom"], summary["top"], summary["bottom"])
            )
    finally:
        d.reset_viewport()


@finding("4.10", "what the portal says is running matches what is running, and clears")
def _(d):
    # "N runs queued." on the old Index page; from 0.35.0 the live state is
    # told in two places: the header's run pill, and Home's Stores tile, which
    # reads "N runs going now" while anything is live. Both must agree with the
    # daemon, and both must clear when nothing is.
    a_store(d)
    d.open_view("home")
    pause(d, 800)
    live = [r for r in d.api("/api/index/runs").get("runs") or [] if r.get("status") not in TERMINAL]
    tile = d.eval(
        "(() => { const k = [...document.querySelectorAll('#main .kpi')].find(k =>"
        " ((k.querySelector('.eyebrow') || {}).textContent || '').trim() === 'Stores');"
        " return k ? ((k.querySelector('.s') || {}).textContent || '') : null; })()"
    )
    stated = re.search(r"(\d+)\s+runs?\s+going", tile or "")
    pill = d.eval("(() => { const p = document.querySelector('header.top .run-pill'); return !!p && !p.hidden; })()")
    if not live and (stated or pill):
        fail("nothing is running and the page still says %r (run pill shown: %s)" % (tile, pill))
    if live and not stated:
        fail("%d run(s) are live and Home's Stores tile says %r" % (len(live), tile))
    if live and int(stated.group(1)) != len(live):
        fail("Home says %r while %d runs are live" % (stated.group(0), len(live)))


@finding("4.11", "one run is one row: a second run of a store does not overwrite the first")
def _(d):
    first = indexed_fixture(d, d.fixtures.small())
    second_run, same_store = start_index(d, d.fixtures.small())
    wait_for_run(d, same_store, run_id=second_run)
    if same_store != first:
        skip("the second submission made a new store, so there is no shared history")

    runs = d.api("/api/index/runs")["runs"]
    ids = [r["id"] for r in runs if r["store"] == same_store]
    if len(ids) < 2:
        fail(
            "after two runs against %s the daemon reports %d run(s) for it. Runs "
            "keyed by store rather than by run are why a card's header showed the "
            "new run while its body held the previous run's log." % (same_store, len(ids))
        )

    # And the store's Runs tab draws one History row per run.
    d.open_view("store/%s/runs" % same_store)
    try:
        d.wait_for("document.querySelectorAll(%s).length >= 2" % json.dumps(HIST_ROW), timeout=15, what="History rows")
    except cdp.ProtocolError:
        pass  # the assertion below says it in the finding's words
    drawn = d.eval("document.querySelectorAll(%s).length" % json.dumps(HIST_ROW))
    if drawn < 2:
        fail(
            "the daemon holds %d runs for %s and its Runs tab draws %d History "
            "row(s). One row per store means the second run sits over the first."
            % (len(ids), same_store, drawn)
        )


@finding("4.12", "a sub-second run does not report a rate against a one-second clock")
def _(d):
    store_name = indexed_fixture(d, d.fixtures.unique("quick"))
    run = run_for(d, store_name)
    elapsed_ms = run.get("elapsed_ms") or 0
    if elapsed_ms >= 1000:
        skip("this run took %dms, so the timer's resolution is not in question" % elapsed_ms)
    d.open_view("store/%s/runs" % store_name)
    row = newest_history_row(d, run)
    if row is None:
        fail("%s's Runs tab never drew the row for its run %s" % (store_name, run.get("id")))
    if re.search(r"00:01", row) or re.search(r"chunks/s", row):
        fail(
            "a run that took %dms reports %r. A sub-second run needs either a "
            "finer unit or no rate at all." % (elapsed_ms, row)
        )


@finding("4.13", "/api/files?limit= refuses an out-of-range value instead of clamping")
def _(d):
    status, answer = d.api_result("/api/files?limit=2000")
    if status == 200:
        fail(
            "asking for limit=2000 answered 200 with %d rows and no indication it "
            "was clamped." % len(answer.get("files", []))
        )
    want("an out-of-range limit", status, 400)
    if "limit" not in json.dumps(answer).lower():
        fail("the refusal does not mention `limit`: %s" % json.dumps(answer)[:200])


@finding("4.14", "every table has a caption or an accessible name")
def _(d):
    # Stores, Files, Doctor and About then. From 0.35.0 the Stores list is not
    # a table and About has none; the tables are a store's Files and Review
    # tabs, Agents' Connected, Tools and Health (Doctor's successor), the
    # Ledger's two tabs and Graph's Blast radius. All of them are held to it.
    store = a_store(d)
    d.api("/api/search?query=release%20record&k=4")
    for route in ("store/%s/files" % store, "store/%s/review" % store, "agents", "agents/tools",
                  "agents/health", "ledger", "ledger/retrievals"):
        d.open_view(route)
        pause(d, 600)
        nameless = d.eval(
            "[...document.querySelectorAll('#main table')].map((t, i) => {"
            " const named = t.querySelector('caption') || t.getAttribute('aria-label') || t.getAttribute('aria-labelledby');"
            " return named ? null : i; }).filter(v => v !== null)"
        )
        if nameless:
            fail("%s has %d table(s) with no caption and no aria-label" % (route, len(nameless)))


@finding("4.15", "a control that moves between views is a button")
def _(d):
    # "Chunks it lives in" was an `<a>` styled as a button with preventDefault.
    # v6 has no such link; the controls that carry a reader from one view to
    # another are the graph's Blast radius and Path from here, and the search
    # detail's Open in graph. Each must be a button, because each behaves like one.
    a_store(d)
    d.open_view("graph")
    d.wait_for("document.querySelectorAll('.g-sel .two > *').length > 0", what="the graph's actions")
    tags = d.eval("[...document.querySelectorAll('.g-sel .two > *')].map(e => e.tagName)")
    if any(t != "BUTTON" for t in tags):
        fail("the graph's actions are %r; each behaves like a button and must be one" % tags)
    d.open_view("search")
    d.type(SEARCH_BOX, "Widget000 new")
    d.press("Enter")
    try:
        d.wait_for("[...document.querySelectorAll('#main .sr-detail *')].some(e => /Open in graph/.test(e.textContent || '') && e.children.length === 0)",
                   timeout=20, what="the detail panel's Open in graph")
    except cdp.ProtocolError:
        skip("no search result named a symbol, so the detail panel offers no Open in graph")
    tag = d.eval("[...document.querySelectorAll('#main .sr-detail *')].find(e => /Open in graph/.test(e.textContent || '') && e.children.length === 0).tagName")
    if tag != "BUTTON":
        fail("'Open in graph' is an <%s>; it behaves like a button and must be one" % tag.lower())


@finding("4.16", "there is a way to act on the whole result set, not just the page")
def _(d):
    store = indexed_fixture(d, d.fixtures.bulk())
    total = d.api("/api/files?store=%s&limit=1" % urllib.parse.quote(store))["total"]
    files_tab(d, store)
    d.wait_for("document.querySelectorAll('#main table tbody tr').length > 0", what="the files table")
    shown = d.eval("document.querySelectorAll('#main table tbody tr').length")
    if total <= shown:
        skip("the corpus fits on one page, so the distinction does not arise")
    d.eval("document.querySelector('#main thead [role=checkbox]').click()")
    pause(d, 400)
    body = view_text(d)
    grouped = "{:,}".format(total).replace(",", "[,  ]?")
    if not re.search(r"select all %s|all %s matching" % (grouped, grouped), body, re.IGNORECASE):
        fail(
            "the header checkbox selects the page (the selection bar honestly says "
            "so) and nothing offers to act on all %d matches" % total
        )


def adopt_picker(d):
    """Stores › Adopt existing .semlith, open."""
    d.open_view("stores")
    press_text(d, "#main .head button", "Adopt existing .semlith", "Adopt existing .semlith")
    d.wait_for("!!document.querySelector(%s + ' .browse-head .dir')" % json.dumps(MODAL),
               what="the adopt picker")
    browse_where(d, MODAL)


@finding("4.17", "a failed adopt keeps the picker open where it was")
def _(d):
    a_store(d)
    folder = d.fixtures.monorepo()  # a folder that is deliberately not a store
    adopt_picker(d)
    descend(d, MODAL, folder)
    before = browse_where(d, MODAL)
    want("the adopt picker's folder before the attempt", before, tilde(d, folder))
    d.modal_press("Adopt this store")
    d.wait_for("!!document.querySelector('.toast.bad')", what="the failed adopt to be reported")
    try:
        d.wait_for("!!document.querySelector(%s + ' .browse-head')" % json.dumps(MODAL), timeout=5)
    except cdp.ProtocolError:
        pass  # the assertion below says what that means
    if not exists(d, MODAL + " .browse-head"):
        fail(
            "a failed adopt closed the picker and dropped back to the Stores "
            "page. Opening it again starts at $HOME with all navigation lost."
        )
    after = browse_where(d, MODAL)
    if after != before:
        fail("the picker stayed open but moved from %r to %r" % (before, after))
    d.modal_press("Cancel")


@finding("4.18", "the adopt picker says which folder is adoptable")
def _(d):
    folder = d.fixtures.adoptme()
    listing = d.api("/api/dirs?path=%s" % urllib.parse.quote(os.path.dirname(folder)))
    entries = listing.get("entries") or listing.get("dirs") or []
    marked = [e for e in entries if e.get("name") == os.path.basename(folder) and (e.get("adoptable") or e.get("store"))]
    inside = d.api("/api/dirs?path=%s" % urllib.parse.quote(folder))
    names = [e.get("name") for e in inside.get("entries") or []]
    if not marked and ".semlith" not in names:
        fail(
            "the folder holding a valid `.semlith` is listed like any other "
            "folder, and inside it the picker lists %s. The picker hides the "
            "thing it adopts." % (", ".join(n for n in names if n) or "nothing")
        )
    # And the page's picker draws the mark the route gives it.
    a_store(d)
    adopt_picker(d)
    descend(d, MODAL, os.path.dirname(folder))
    badge = d.eval(
        "(() => { const r = [...document.querySelectorAll(%s + ' .bitem')].find(r =>"
        " ((r.querySelector('.name') || {}).textContent || '') === %s);"
        " return r ? (r.innerText || '').trim() : null; })()" % (json.dumps(MODAL), json.dumps(os.path.basename(folder)))
    )
    d.modal_press("Cancel")
    if badge is None or not re.search(r"\bstore\b", badge, re.IGNORECASE):
        fail("the adopt picker lists %s with no mark saying it holds a store: %r"
             % (os.path.basename(folder), badge))


# ------------------------------------------------------------------ 0.26.0
#
# The v4 surfaces, checked the same way as the findings and numbered after
# them, so the gate covers what shipped rather than only what was once broken.
# 0.35.0 moved most of them; each says where to.


@finding("5.1", "the sidebar is every page, in three groups, in order")
def _(d):
    # 0.35.0 owner decision (v6 as drawn): nine pages in three groups. Files is
    # a store's own tab, Index is the store wizard, Inside the index is a tab of
    # Stores, Impact is Graph › Blast radius, Cloud, Doctor and About are
    # sections of Settings and Agents.
    a_store(d)
    d.open_view("home")
    labels = [t for t in texts_of(d, ".nav .nav-item .lab") if t]
    want("the sidebar's entries", labels, [
        "Home", "Stores", "Search", "Graph",
        "Agents", "Ledger", "Reports",
        "Privacy", "Settings",
    ])
    # The group labels are uppercased by the stylesheet, so innerText reads
    # them that way. The design's names are what is asserted, not the type.
    groups = [t.title() for t in texts_of(d, ".nav .nav-label") if t]
    want("the sidebar's groups", groups, ["Workspace", "Agents", "Machine"])


def reach(d, symbol, store=None):
    """Graph › Blast radius for one symbol, answered."""
    d.open_view("graph/blast")
    if store:
        current = d.eval("((document.querySelector('.ctrl-card button[aria-haspopup] .mono') || {}).textContent || '').trim()")
        if current != store:
            pick_store(d, ".ctrl-card button[aria-haspopup]", store, "graph/blast")
    # Typed until the field holds exactly the symbol: the page repaints when
    # the store's hubs arrive, and a repaint between the clear and the typing
    # put the last symbol back with the caret after it ("calleecallee").
    box = '.ctrl-card input[aria-label="Symbol"]'
    for _ in range(3):
        d.type(box, symbol)
        pause(d, 300)
        if d.eval("(document.querySelector(%s) || {}).value" % json.dumps(box)) == symbol:
            break
    press_text(d, ".ctrl-card button", "Reach", "Reach")
    d.wait_for("!document.querySelector('#main .spinner') && (!!document.querySelector('#main .q3') || !!document.querySelector('#main .error-box'))",
               timeout=40, what="Blast radius to answer for %s" % symbol)


@finding("5.2", "Blast radius answers for a symbol, by hop, with a support class on every row")
def _(d):
    # Impact moved to Graph › Blast radius in 0.35.0.
    store = indexed_fixture(d, d.fixtures.small())
    overview = d.api("/api/graph?limit=1&store=%s" % urllib.parse.quote(store))
    nodes = overview.get("nodes") or []
    if not nodes:
        skip("the store's graph holds no symbol to read backwards from")
    symbol = nodes[0].get("name")
    reach(d, symbol, store)
    if exists(d, "#main .error-box"):
        fail("Blast radius for %s answered an error: %s" % (symbol, text_of(d, "#main .error-box", "the error")))
    headline = text_of(d, "#main .card.pad .big14", "the answer's headline")
    if symbol not in headline:
        fail("the answer's headline does not name %s: %r" % (symbol, headline))
    centre = text_of(d, "#main .mini-graph .g-node.sel", "the symbol at the centre of the canvas")
    want("the subject at the centre", centre, symbol)

    rows = d.eval("document.querySelectorAll('#main .split table tbody tr').length")
    if not rows:
        # Nothing reaching the symbol is a legitimate answer, and the page has
        # to say so rather than draw an empty list.
        if not re.search(r"\b0 definitions\b|nothing", headline, re.IGNORECASE):
            fail("no rows and no sentence saying why: %r" % headline)
        return
    # Every reached row carries its support class. A hop with no badge is a
    # hop a reader will assume was verified.
    unbadged = d.eval(
        "[...document.querySelectorAll('#main .split table tbody tr')].filter(tr =>"
        " !/(extracted|resolved|inferred|ambiguous)/.test((tr.querySelector('.badge') || {}).textContent || '')).length"
    )
    if unbadged:
        fail("%d of %d reached rows carry no support class" % (unbadged, rows))
    hops = [int(h) for h in d.eval("[...document.querySelectorAll('#main .split table tbody tr')].map(tr => tr.cells[3].innerText.trim())") if h.isdigit()]
    if hops != sorted(hops):
        fail("the reached rows are out of hop order: %s" % hops)


@finding("5.3", "the path finder and its evidence render on Graph › Path & evidence")
def _(d):
    root = review_tree(d, "pathfinder")
    store = indexed_fixture(d, root)
    d.open_view("graph/path")
    body = view_text(d)
    for wanted in ["Path & evidence", "FROM", "TO", "Prefer verified", "Strict", "Find the path"]:
        if wanted not in body:
            fail("Graph › Path & evidence is missing %r: %s" % (wanted, body[:300]))
    pick_store(d, ".ctrl-card button[aria-haspopup]", store, "graph/path")
    d.type('.ctrl-card input[aria-label="From"]', "caller")
    d.type('.ctrl-card input[aria-label="To"]', "callee")
    press_text(d, ".ctrl-card button", "Find the path", "Find the path")
    d.wait_for("/Copy as evidence/.test(document.querySelector('#main').innerText)", timeout=30,
               what="the path's supporting lines and Copy as evidence")


@finding("5.4", "the Graph page lists the store's communities")
def _(d):
    a_store(d)
    d.open_view("graph")
    d.wait_for("/Subsystems/.test((document.querySelector('.g-side') || {}).innerText || '')", what="the Subsystems group")
    if d.eval("(document.querySelector('.subsys-wrap .group-row') || {}).getAttribute('aria-expanded')") != "true":
        d.click(".subsys-wrap .group-row")
    d.wait_for("!/Reading/.test((document.querySelector('.subsys-wrap') || {}).innerText || '')", timeout=30,
               what="the communities to be read")
    shown = d.eval("((document.querySelector('.subsys-wrap .group-row .t-mono-sm') || {}).textContent || '').trim()")
    if not re.match(r"^\d+ of [\d,]+$", shown) and not exists(d, ".subsys-wrap .error-box"):
        fail("the Subsystems group does not say how many of how many it shows: %r" % shown)


@finding("5.5", "Inside the index states what the graph covers")
def _(d):
    a_store(d)
    d.open_view("stores/inside")
    d.wait_for("/graph health/i.test(document.querySelector('#main').innerText)", timeout=45,
               what="the Graph health card")
    body = view_text(d).lower()
    for wanted in ["language mix, by line", "chunks added per month", "graph health"]:
        if wanted not in body:
            fail("Inside the index is missing the %r card" % wanted)
    if "unresolved" not in body or "several definitions" not in body:
        fail("Graph health does not state its unresolved targets: %s" % body[:400])
    for tier in ("extracted", "resolved", "ambiguous", "unresolved"):
        if tier not in body:
            fail("the call-edge tier %r is missing from Graph health" % tier)
    # Twelve columns whatever the corpus holds: a store indexed this morning
    # has one month in it, and one bar in a full-width card is a chart that has
    # failed rather than a young corpus.
    columns = d.eval("document.querySelectorAll('#main .months > div').length")
    if columns != 12:
        fail("the month chart draws %d columns; it should always draw 12" % columns)
    # Every bar is sized through the CSSOM, because the portal is served under
    # `style-src 'self'` and a width written into the markup is dropped,
    # silently, leaving every bar full width.
    rows = d.eval("document.querySelectorAll('#main .lang-row .bar').length")
    narrower = d.eval(
        "[...document.querySelectorAll('#main .lang-row .bar')].some(track => {"
        "  const fill = track.firstElementChild; if (!fill) return false;"
        "  const w = fill.getBoundingClientRect().width, t = track.getBoundingClientRect().width;"
        "  return t > 0 && w > 0 && w < t - 1; })"
    )
    if rows > 1 and not narrower:
        fail(
            "every language-mix bar fills its whole track, so the width was "
            "written into the markup and `style-src 'self'` dropped it"
        )


@finding("5.6", "the ledger lists sessions, filters them, and exports what it shows")
def _(d):
    a_store(d)
    d.api("/api/search?query=release%20record&k=4")
    d.open_view("ledger")
    body = view_text(d)
    if "Sessions" not in body:
        fail("the ledger page has no Sessions tab")
    for wanted in ["Markdown", "CSV", "JSON"]:
        if wanted not in texts_of(d, "#main .tabs button"):
            fail("the sessions table cannot export %s" % wanted)
    # 0.35.0 owner decision (v6 as drawn): three filters — client, store and
    # tier — where there were two. Still no one model picked for every row: a
    # session's saving is priced at the model it ran on.
    selects = d.eval("[...document.querySelectorAll('#main .filterbar .dd')].map(s => s.getAttribute('aria-label'))")
    want("the sessions table's filters", selects, ["Client", "Store", "Tier"])
    if d.eval("!!document.querySelector('[aria-label=\"Cost at\"]')"):
        fail("the sessions table still prices every session at one chosen model")
    # The Model column comes with the Usage-from-client-logs switch, which is
    # where the model each session ran on is read from (0.35.0 owner decision:
    # usage from logs is a Ledger switch). Saved comes with a priced session.
    was = bool((d.api("/api/ledger").get("usage") or {}).get("enabled"))
    try:
        d.api("/api/ledger/usage", method="POST", body={"on": True})
        d.open_view("ledger", fresh=True)
        heads = d.eval("[...document.querySelectorAll('#main table th')].map(t => t.textContent.trim().toUpperCase()).join('|')")
        if "MODEL" not in heads:
            fail("with usage from client logs on, the sessions table has no MODEL column: %s" % heads)
        priced = any(s.get("saved_usd") is not None for s in d.api("/api/ledger").get("sessions") or [])
        if priced and "SAVED" not in heads:
            fail("a session is priced and the sessions table has no SAVED column: %s" % heads)
    finally:
        d.api("/api/ledger/usage", method="POST", body={"on": was})


@finding("5.7", "session replay is on by default, and Privacy can turn it off and on")
def _(d):
    # 0.35.0 owner decision: session replay defaults to on (a settings file that
    # never wrote the key reads as on; one that wrote off stays off). The old
    # check asserted the opposite default. Asserted now: replay is on in this
    # drive's pristine home, the Ledger's replay tab shows the on state, and
    # the Privacy switch turns it off — after which the tab says so and offers
    # to turn it back on, and does.
    a_store(d)
    state = d.api("/api/ledger/replay")
    if not state.get("enabled"):
        fail(
            "session replay is off in a store home that never chose: %s. It "
            "defaults to on from 0.35.0." % json.dumps(state)[:200]
        )
    d.open_view("ledger/replay")
    if not re.search(r"turn off on the Privacy page", view_text(d)):
        fail("the Ledger's Session replay tab does not show the on state: %s" % view_text(d)[:300])
    try:
        d.open_view("privacy")
        row = "#main .tg-row[role=switch]"
        d.wait_for("[...document.querySelectorAll(%s)].some(b => /Session replay · on/.test(b.innerText))"
                   % json.dumps(row), what="the Privacy page's Session replay switch, on")
        d.eval("[...document.querySelectorAll(%s)].find(b => /Session replay/.test(b.innerText)).click()"
               % json.dumps(row))
        d.wait_for("[...document.querySelectorAll(%s)].some(b => /Session replay · off/.test(b.innerText)"
                   " && b.getAttribute('aria-checked') === 'false')" % json.dumps(row),
                   what="the switch to read off, announced as unchecked")
        if d.api("/api/ledger/replay").get("enabled"):
            fail("the Privacy switch reads off and the daemon still has replay on")
        d.open_view("ledger/replay")
        if "It is off" not in view_text(d):
            fail("with replay off the Ledger's tab does not say so: %s" % view_text(d)[:300])
        press_text(d, "#main button", "Turn it on", "the Ledger's Turn it on")
        d.wait_for("/turn off on the Privacy page/.test(document.querySelector('#main').innerText)",
                   what="Turn it on to turn replay back on")
    finally:
        d.api("/api/ledger/replay", method="POST", body={"on": True})


@finding("5.8", "Reports generates all five, locally")
def _(d):
    a_store(d)
    d.open_view("reports")
    body = view_text(d)
    for wanted in ["Retrieval savings", "AI access audit", "Change brief", "Index health", "Knowledge gaps"]:
        if wanted not in body:
            fail("the Reports page is missing %r" % wanted)
    if "never uploaded" not in body:
        fail("the Reports page does not say where the data goes")
    for kind in ["savings", "access", "change", "health", "gaps"]:
        answer = d.api("/api/report?kind=%s&format=markdown" % kind)
        text = answer.get("text") or ""
        if not text.strip():
            fail("the %s report generated nothing" % kind)
        if "Generated" not in text:
            fail("the %s report does not say when it was generated" % kind)


@finding("5.9", "Settings › Cloud describes the service and contacts nothing")
def _(d):
    d.open_view("settings/cloud")
    body = view_text(d)
    # 0.37.0's not-connected state: what the cloud adds, and that this binary
    # reaches nothing until someone signs in.
    if "Semlith Cloud" not in body or "contacts nothing until you sign in" not in body:
        fail("Settings › Cloud did not render: %s" % body[:300])
    if "not connected" not in body:
        fail("Settings › Cloud does not say it is not connected")
    for word in ["Disconnect", "token prefix", "acme/api"]:
        if word in body:
            fail("Settings › Cloud drew its connected state on a machine that never signed in")


@finding("5.10", "the Agents page measures what its tool list costs")
def _(d):
    d.open_view("agents")
    body = view_text(d)
    if not re.search(r"what the tool list costs", body, re.IGNORECASE):
        fail("the Agents page does not state what the tool list costs")
    if re.search(r"\bpaid\b", body):
        fail("a tool on the Agents page is marked paid")
    listed = d.api("/api/agents")
    tools = listed.get("tools") or []
    if len(tools) != 16:
        fail("the Agents page lists %d tools, expected 16" % len(tools))
    shown = re.search(r"([\d,]+) tokens", text_of(d, "#main .big18", "the tool list's cost"))
    if not shown or int(shown.group(1).replace(",", "")) != listed.get("tool_list_tokens"):
        fail("the page states %r and /api/agents counts %s tokens"
             % (shown and shown.group(0), listed.get("tool_list_tokens")))


def every_view(d):
    """Every v6 route, plus each tab of one store's page."""
    store = a_store(d)
    return list(cdp.Drive.ROUTES) + ["store/%s" % store] + [
        "store/%s/%s" % (store, tab) for tab in cdp.Drive.STORE_TABS[1:]
    ]


@finding("5.11", "every page renders in both themes")
def _(d):
    """Every v6 page in light and in dark, set the way the page's own control
    sets it: `semlith-theme` in localStorage, then a load. The check fails if a
    page does not draw or the root does not carry the theme; the full set of
    pictures at two widths is `v6.shots`."""
    views = every_view(d)
    try:
        for theme in ("light", "dark"):
            d.eval("try { localStorage.setItem('semlith-theme', %s); } catch (e) {}" % json.dumps(theme))
            for i, view in enumerate(views):
                d.open_view(view, fresh=(i == 0))
                got = d.eval("document.documentElement.getAttribute('data-theme')")
                if got != theme:
                    fail("%s drew with data-theme=%r in the %s theme" % (view, got, theme))
    finally:
        d.eval("try { localStorage.removeItem('semlith-theme'); } catch (e) {}")


# ---------------------------------------------------------------- 0.26.1
#
# The 6.x block is the 2026-09-21 design-parity drive. Where the 5.x checks
# assert that a page exists, these assert that it says what the design says
# and that nothing on it is out of reach.


def open_welcome(d):
    """The first-run screen, which has no shell and no sidebar."""
    d.open_view("welcome", fresh=True)


@finding("6.1", "the first-run screen carries what the v6 design's welcome carries")
def _(d):
    # 0.35.0 owner decision (v6 as drawn): five steps, Connect being the fifth;
    # the version as a chip beside the wordmark; the machine checks on the
    # right, the first naming the address the daemon answers on. The v4 screen's
    # `--no-ledger` sentence is not on v6's welcome — it is on the Ledger page,
    # beside the switch that pauses recording, which 6.6 and v6.29 assert.
    open_welcome(d)
    about = d.api("/api/about")
    # The chip is drawn when /api/about answers, which a slow runner gives a
    # moment after the screen itself.
    try:
        d.wait_for("!!document.querySelector('header.top .count-chip')", timeout=15, what="the version beside the wordmark")
    except cdp.ProtocolError:
        pass
    version = text_of(d, "header.top .count-chip", "the version beside the wordmark")
    if version.lstrip("v") != about["version"]:
        fail("the welcome names version %r and the daemon is %s" % (version, about["version"]))
    steps = d.eval("document.querySelectorAll('.welcome-card .steps5 .s').length")
    want("the first-run screen's steps", steps, 5)
    body = view_text(d)
    if "Adopt an existing .semlith" not in body:
        fail("the first-run screen offers no way to adopt a store that already exists")
    host = d.eval("location.host")
    d.wait_for("!/checking…/.test((document.querySelector('.check-row') || {}).innerText || '')",
               what="the machine checks to be read")
    daemon_row = text_of(d, ".check-row", "the Daemon check")
    if host not in daemon_row:
        fail("the Daemon check reads %r and does not name %s. 'loopback only' is a "
             "claim the reader cannot check without the port." % (daemon_row, host))


@finding("6.2", "Skip for now lands on Home")
def _(d):
    # 0.35.0 owner decision (v6 as drawn): the first run's Skip for now is on
    # the wizard's last step, Connect, and lands on Home — the page about this
    # machine's index — rather than on the welcome's own Stores link it was in v4.
    reach_connect_step(d, "skipper")
    press_text(d, ".wz-foot button", "Skip for now", "Skip for now")
    d.wait_for("location.hash === '#/home'", what="Skip for now to land on Home")


@finding("6.3", "the Stores page offers the way into what the index holds")
def _(d):
    a_store(d)
    d.open_view("stores")
    press_text(d, "#main .tabs .tab", "Inside the index", "the Inside the index tab")
    d.wait_for("location.hash === '#/stores/inside'", what="the route into Inside the index")


@finding("6.4", "the Retrieval ledger offers the way into Reports")
def _(d):
    a_store(d)
    d.open_view("ledger")
    press_text(d, "#main .head button", "Build a report", "Build a report")
    d.wait_for("location.hash === '#/reports'", what="the route into Reports")


@finding("6.5", "About states the licence the binary ships under")
def _(d):
    d.open_view("settings/about")
    about = d.api("/api/about")
    if about["license"] not in view_text(d):
        fail("Settings › About does not state the licence the binary ships under")


@finding("6.6", "the sidebar states whether the ledger is recording")
def _(d):
    a_store(d)
    d.open_view("home")
    card = text_of(d, ".nav .daemon .facts", "the daemon card")
    about = d.api("/api/about")
    recording = about.get("recording")
    on = recording.get("on") if isinstance(recording, dict) else about.get("ledger") is not False
    want("the daemon card's ledger state", "ledger on" in card, bool(on))


@finding("6.7", "Reports previews the one report that is selected")
def _(d):
    a_store(d)
    d.open_view("reports")
    types = d.eval("document.querySelectorAll('#main .kind-card').length")
    if types != 5:
        fail("the Reports picker offers %d report types, expected 5" % types)
    d.wait_for("((document.querySelector('#main .preview') || {}).textContent || '').length > 40",
               what="the selected report to generate")
    first = d.eval("document.querySelector('#main .preview').textContent")
    press_text(d, "#main .kind-card .t", "Index health")
    d.wait_for("((document.querySelector('#main .preview') || {}).textContent || '') !== %s"
               " && !/Generating/.test(document.querySelector('#main .preview').textContent)" % json.dumps(first),
               what="the preview to follow the selected report")
    name = text_of(d, "#main .card.flexcol .card-h .mono", "the preview's file name")
    if not name.startswith("health-") or not name.endswith(".md"):
        fail("the preview names %r, which is not the chosen report in the chosen Markdown format" % name)
    press_text(d, "#main .seg button", "CSV")
    d.wait_for("(document.querySelector('#main .card.flexcol .card-h .mono').textContent || '').endsWith('.csv')",
               what="the format to change the file written")


@finding("6.8", "no page scrolls sideways, at any width the design supports")
def _(d):
    widths = [390, 430, 820, 1024, 1280, 1440, 1920]
    views = every_view(d)
    bad = []
    try:
        for width in widths:
            d.set_viewport(width, 844 if width < 600 else 900, mobile=width < 600)
            for i, view in enumerate(views):
                d.open_view(view, fresh=(i == 0))
                seen = d.eval("({page: document.documentElement.scrollWidth, vw: document.documentElement.clientWidth})")
                if seen["page"] > seen["vw"] + 1:
                    bad.append("%s at %dpx scrolls to %dpx" % (view, width, seen["page"]))
        open_welcome(d)
        for width in (390, 1440):
            d.set_viewport(width, 844 if width < 600 else 900, mobile=width < 600)
            seen = d.eval("({page: document.documentElement.scrollWidth, vw: document.documentElement.clientWidth})")
            if seen["page"] > seen["vw"] + 1:
                bad.append("welcome at %dpx scrolls to %dpx" % (width, seen["page"]))
    finally:
        d.reset_viewport()
    if bad:
        fail("pages scroll sideways: %s" % "; ".join(bad))


def settle_network(d, limit=15.0):
    """Block until the page stops making requests, or `limit` passes."""
    deadline = time.time() + limit
    last, stable = -1, 0
    while time.time() < deadline:
        count = d.eval("performance.getEntriesByType('resource').length")
        stable = stable + 1 if count == last else 0
        last = count
        # Three readings the same, a beat apart: enough for a page whose
        # panels fetch one after another rather than all at once.
        if stable >= 3:
            return True
    return False


@finding("6.9", "no page writes an error to the browser console")
def _(d):
    """Read once a page has stopped fetching: navigating away with a request in
    flight cancels it, and Chrome logs the cancellation against whichever page
    it lands on. A load failure that survives a quiet network is a real one."""
    bad, restless = [], []
    for view in every_view(d) + ["welcome"]:
        d.open_view(view, fresh=True)
        if not settle_network(d):
            restless.append(view)
        # Cleared after the page is quiet, so anything read below was written
        # by this page rather than by the navigation that reached it.
        d.clear_console()
        time.sleep(0.5)
        for line in d.console_errors():
            bad.append("%s: %s" % (view, line))
    if bad:
        fail("the console carried errors: %s" % "; ".join(bad[:8]))
    if restless:
        fail(
            "these pages were still fetching after 15s, so their console was "
            "never read against a settled page: %s" % ", ".join(restless)
        )


@finding("6.10", "an index run outlives the page that started it")
def _(d):
    run_id, store = start_index(d, d.fixtures.unique("outlives", count=400))
    try:
        d.open_view("store/%s/runs" % store, fresh=True)
        d.wait_for("!!document.querySelector(%s)" % json.dumps(LIVE_CARD), what="the live run's card")
        # A full document load, which is what a reload and a reopened tab are.
        d.navigate("%s/#/store/%s/runs" % (d.portal_url, store))
        d.wait_for(
            "!!document.querySelector(%s) || (document.querySelectorAll(%s).length > 0)"
            % (json.dumps(LIVE_CARD), json.dumps(HIST_ROW)),
            what="the run to come back after a reload; a run that vanishes with the "
            "page is a run bound to the request that started it, which 0.24.0 fixed",
        )
        live = run_by_id(d, run_id)
        if not live:
            fail("the daemon dropped the run for %s when the page reloaded" % store)
        if live.get("status") not in TERMINAL and not exists(d, LIVE_CARD):
            fail("after reloading, %s's run is %s and its Runs tab draws no live card"
                 % (store, live.get("status")))
    finally:
        stop_quietly(d, store)


# ---------------------------------------------------------------- 0.27.0
#
# The 7.x block: the things 0.27.0 changed underneath every page at once, plus
# the two surfaces it built from nothing.


@finding("7.1", "a page whose content overflows scrolls to its end")
def _(d):
    """`<main>` carries the overflow from 0.35.0. A child that absorbed it
    instead — the 0.27.0 defect — stops the page scrolling and crushes itself."""
    a_store(d)
    d.set_viewport(1280, 600)
    try:
        for view in ("ledger", "stores", "home", "privacy"):
            d.open_view(view)
            seen = d.eval(
                "(() => { const m = document.querySelector('#main'); m.scrollTop = 1e6;"
                " return {top: m.scrollTop, over: m.scrollHeight - m.clientHeight}; })()"
            )
            if seen["over"] > 2 and seen["top"] < seen["over"] - 2:
                fail("on %s the page holds %dpx more than it shows and scrolls only %dpx"
                     % (view, seen["over"], seen["top"]))
            clipped = d.eval(
                "[...document.querySelectorAll('#main .card')].filter(el => {"
                " const s = getComputedStyle(el); if (/(auto|scroll)/.test(s.overflowY)) return false;"
                " const r = el.getBoundingClientRect();"
                " return r.height > 0 && el.scrollHeight > Math.ceil(r.height) + 2; }).length"
            )
            if clipped:
                fail("on %s, %d card(s) are shorter than their own content" % (view, clipped))
    finally:
        d.reset_viewport()


@finding("7.2", "every page ends with the design's floor under its last card")
def _(d):
    """The v6 design's page is `20px 24px 28px`, and `14px 14px 24px` on a
    phone, so the floor is 28px and 24px (0.35.0 owner decision: v6 as drawn,
    where v4's was 44px at both). Search and Graph are the recorded exceptions:
    full-height pages whose canvas fills the frame."""
    a_store(d)
    views = ("home", "stores", "ledger", "reports", "agents", "privacy", "settings")
    try:
        for width in (1280, 390):
            d.set_viewport(width, 844 if width < 600 else 900, mobile=width < 600)
            for i, view in enumerate(views):
                d.open_view(view, fresh=(i == 0))
                floor = d.eval("(() => { const v = document.querySelector('#main > .page'); return v ? getComputedStyle(v).paddingBottom : null; })()")
                if floor is None:
                    fail("%s has no .page to measure a floor on" % view)
                # The design's phone rule is `14px 14px 24px` below 760px.
                want("the floor under %s at %dpx" % (view, width), floor, "24px" if width < 760 else "28px")
    finally:
        d.reset_viewport()
    d.open_view("graph")
    if exists(d, "#main > .page"):
        fail("the graph page now roots on .page, so the recorded exception no longer applies")


@finding("7.3", "no control in the portal renders as bare text")
def _(d):
    """A `.btn` has a fill or an edge; a pressed chip does not change weight, so
    a chip row does not reflow when one is picked. A `.lnk` is a link-styled
    button by design and is not held to the button shape."""
    a_store(d)
    for view in ("stores", "graph", "ledger", "reports", "graph/blast", "agents", "settings"):
        d.open_view(view)
        bare = d.eval(
            """
            [...document.querySelectorAll('#main .btn')].filter(el => {
              if (el.offsetParent === null) return false;
              const s = getComputedStyle(el);
              const noFill = s.backgroundColor === 'rgba(0, 0, 0, 0)' || s.backgroundColor === 'transparent';
              const noEdge = s.borderTopWidth === '0px' || s.borderTopStyle === 'none'
                          || s.borderTopColor === 'rgba(0, 0, 0, 0)';
              return noFill && noEdge;
            }).map(el => (el.textContent || el.getAttribute('aria-label') || '').trim()).slice(0, 5)
            """
        )
        if bare:
            fail("on %s these controls have neither a background nor a border: %r" % (view, bare))
        weights = d.eval(
            """
            (() => {
              const on = [...document.querySelectorAll('#main .chip[aria-pressed="true"]')];
              const off = [...document.querySelectorAll('#main .chip[aria-pressed="false"]')];
              if (!on.length || !off.length) return null;
              return [getComputedStyle(on[0]).fontWeight, getComputedStyle(off[0]).fontWeight];
            })()
            """
        )
        if weights and weights[0] != weights[1]:
            fail("on %s a pressed chip is weight %s and an unpressed one %s, so a chip "
                 "row reflows when one is picked" % (view, weights[0], weights[1]))


@finding("7.4", "no code block fades out at its right edge")
def _(d):
    a_store(d)
    for view in ("agents/add", "privacy", "reports", "settings/access"):
        d.open_view(view)
        masked = d.eval(
            """
            [...document.querySelectorAll('#main .code, #main .copyfield .t, #main .lines, #main .log, #main .preview')]
              .filter(el => { const s = getComputedStyle(el);
                return (s.maskImage && s.maskImage !== 'none') || (s.webkitMaskImage && s.webkitMaskImage !== 'none'); }).length
            """
        )
        if masked:
            fail("on %s, %d code surface(s) still fade at the right edge" % (view, masked))


@finding("7.5", "Blast radius draws its canvas beside the answer")
def _(d):
    """Impact's 320px reverse-reachability canvas moved with it to Graph ›
    Blast radius. It is the shared live renderer the Explore tab uses, it sits
    in the right column beside the reached table, and its card says what it
    draws."""
    root = review_tree(d, "canvas")
    store = indexed_fixture(d, root)
    reach(d, "callee", store)
    if not exists(d, "#main .mini-graph"):
        fail("Blast radius draws no canvas card")
    caption = text_of(d, "#main .mini-graph .label", "the canvas caption")
    if not re.match(r"^(reverse reachability|\d+ beyond the \d+ drawn)", caption):
        fail("the canvas caption reads %r" % caption)
    if not exists(d, "#main .mini-graph .g-live .g-node"):
        fail("the Blast radius canvas is not the shared live renderer")
    height = d.eval("Math.round(document.querySelector('#main .mini-graph').getBoundingClientRect().height)")
    if height < 320 or height > 460:
        fail("the canvas card is %dpx tall; it should sit between 320 and 460" % height)
    beside = d.eval(
        "(() => { const split = document.querySelector('#main .split.s-1-1'); if (!split) return false;"
        " const cols = [...split.children]; return cols.length === 2 && !!cols[0].querySelector('table')"
        " && !!cols[1].querySelector('.mini-graph') && !!cols[0].querySelector('.q3'); })()"
    )
    if not beside:
        fail("the reached table, the three figures and the canvas are not two columns side by side")
    d.shot("7.5-blast-canvas")


@finding("7.6", "the Blast radius canvas is alive: it draws, it settles, it keeps moving")
def _(d):
    root = review_tree(d, "alive")
    store = indexed_fixture(d, root)
    answer = d.api("/api/impact?name=callee&store=%s&depth=3" % urllib.parse.quote(store))
    if not ((answer.get("impact") or {}).get("reached") or []):
        skip("nothing reaches callee in the fixture, so there is no question to put to the canvas")
    reach(d, "callee", store)
    d.wait_for("document.querySelectorAll('#main .mini-graph .g-node').length >= 2", what="the canvas to draw")
    where = "[...document.querySelectorAll('#main .mini-graph .g-node')].map(n => n.style.left + ',' + n.style.top).join('|')"
    first = d.eval(where)
    # The layout cools but never freezes: every node carries a slow wander.
    # Skipped under reduced motion, where the canvas is deliberately still.
    if not d.eval("matchMedia('(prefers-reduced-motion: reduce)').matches"):
        time.sleep(1.2)
        if d.eval(where) == first:
            fail("the Blast radius canvas has not moved in 1.2s; the simulation is not running")
    reached = d.eval("((document.querySelector('#main .q3 .stat-inline .v') || {}).textContent || '').trim()")
    if reached in ("", "0"):
        fail("the answer reads %r reached while the canvas draws callers" % reached)
    d.shot("7.6-blast-canvas")


@finding("7.7", "the Reports builder offers a window and a scope, and the route takes them")
def _(d):
    a_store(d)
    d.open_view("reports")
    press_text(d, "#main .kind-card .t", "Change brief")
    body = view_text(d)
    for word in ("WINDOW", "STORES", "FORMAT"):
        if word not in body:
            fail("the Reports builder offers no %s group" % word.lower())
    windows = d.eval("[...document.querySelectorAll('#main .seg')][0].querySelectorAll('button').length")
    want("the windows the builder offers", windows, 4)
    formats = d.eval("[...document.querySelectorAll('#main .seg')][1].querySelectorAll('button').length")
    want("the formats the builder offers", formats, 5)
    week = d.api("/api/report?kind=change&window=week")
    month = d.api("/api/report?kind=change&window=month")
    if week.get("text") == month.get("text"):
        fail("/api/report returns the same change brief for a week and for a month")


@finding("7.8", "a schedule the page lists is a schedule the daemon runs")
def _(d):
    a_store(d)
    d.open_view("reports")
    if "Schedules" not in view_text(d):
        fail("the Reports page has no Schedules card")
    held = d.api("/api/schedules")
    listed = d.eval("document.querySelectorAll('#main .sched-row').length")
    want("the rows the card draws", listed, len(held.get("schedules") or {}))
    broken = [s for s in (held.get("schedules") or {}).values() if s.get("last_error")]
    if broken and "failed" not in view_text(d).lower():
        fail("a schedule recorded an error and the card does not say so")
    d.shot("7.8-schedules")


@finding("7.9", "About has lost the two blocks the design has no place for")
def _(d):
    """A forty-eight-row catalogue of embedding models and a row of MCP protocol
    revisions an agent settles in its handshake. Both routes still answer —
    what went is a view, not a capability. The v6 design's About is seven facts
    (VERSION to UPTIME) and a language table, so this holds on Settings › About."""
    d.open_view("settings/about")
    if exists(d, "#main .w-models"):
        fail("Settings › About still draws the models table")
    if "MCP revisions" in view_text(d):
        fail("Settings › About states the MCP revisions, which the design's About does not")
    if not (d.api("/api/models").get("models") or []):
        fail("/api/models stopped answering when its portal view was removed")
    if not (d.api("/api/about").get("revisions") or []):
        fail("/api/about stopped returning the MCP revisions")


@finding("7.10", "an unreadable store does not take the readable ones down with it")
def _(d):
    answer = d.api("/api/stores")
    unreadable = [s for s in (answer.get("stores") or []) if s.get("unreadable")]
    if not unreadable:
        skip("no unreadable store is registered, so there is nothing to answer around")
    for route in ("/api/graph", "/api/files"):
        got = d.api(route)
        failures = got.get("failed") or []
        if not failures:
            fail("%s answered with no failures listed while a store is unreadable" % route)
        for entry in failures:
            if not entry.get("store") or not entry.get("path"):
                fail("%s reports a failure that names neither the store nor its path" % route)


# ---------------------------------------------------------------- 0.28.0
#
# The 8.x block: the page checks of 0.28.0's items 1.3 to 1.8, run on every
# operating system. 8.1 to 8.3 are item 1.8, "a page that repaints in place".
#
# From 0.35.0 the run controls are on a store's Runs tab and the limits on
# Settings › Performance; both pages paint themselves from the one live poll.
# Each check measures the page the way a reader meets it — `<main>`'s scroll
# offset, where the pressed control sits, what holds focus, and which nodes a
# MutationObserver sees replaced — rather than reading the code's intentions.
#
# Every wait is on a condition and bounded. "Settled" means two `/api/changes`
# polls have come back since the action and no `/api/index/runs` read is still
# in flight, counted by wrapping the page's own `fetch`.

#: A short window, so the page scrolls with only a card or two on it and the
#: offset being held is not zero. Chrome does not anchor a scroller sitting at
#: 0, so an offset of 0 proves nothing about anchoring.
STILL_VIEWPORT = (1280, 440)

#: The page's own scroller: `<main>` carries the overflow.
VIEW = MAIN

STILL_INSTRUMENT = r"""
(() => {
  if (window.__still) return true;
  const still = window.__still = {changes: 0, runs: 0, runsInFlight: 0, loading: 0};
  const real = window.fetch.bind(window);
  // Counted when the body has been read, which is the moment before the page
  // paints from it, so "none in flight" means every answer has been drawn.
  window.fetch = (input, init) => {
    const url = String((input && input.url) || input);
    const runs = url.includes('/api/index/runs');
    const changes = url.includes('/api/changes');
    if (runs) still.runsInFlight++;
    const done = () => {
      if (runs) { still.runsInFlight--; still.runs++; }
      if (changes) still.changes++;
    };
    return real(input, init).then(response => {
      const json = response.json.bind(response);
      response.json = () => json().then(v => { done(); return v; }, e => { done(); throw e; });
      return response;
    }, error => { done(); throw error; });
  };
  // The page loader (`.ld`), anywhere, at any time from here on. A live update
  // is never a navigation, so it must never show the loader.
  new MutationObserver(records => {
    for (const record of records) for (const node of record.addedNodes) {
      if (node.nodeType === 1 && (node.matches('.ld') || node.querySelector('.ld'))) still.loading++;
    }
  }).observe(document.body, {childList: true, subtree: true});
  return true;
})()
"""


def still_open(d, viewport, route):
    """A route on a short window, instrumented."""
    d.set_viewport(*viewport)
    d.open_view(route, fresh=True)
    d.eval(STILL_INSTRUMENT)
    d.wait_for("window.__still.changes >= 1", timeout=15, what="the page's live poll to be running")


def settle(d, what):
    """Wait until every poll the action set off has been painted."""
    mark = d.eval("window.__still.changes")
    d.wait_for(
        "window.__still.changes >= %d && window.__still.runsInFlight === 0" % (mark + 2),
        timeout=30,
        what="the page to settle after %s: two polls answered and no runs read in flight" % what,
    )
    d.eval("new Promise(done => requestAnimationFrame(() => requestAnimationFrame(() => done(true))))")


def hold_at(d, element_js, above):
    """Scroll `<main>` so `element_js` sits `above` px under its top.

    Returns the offset, which has to be neither 0 nor the bottom: at 0 nothing
    about the offset is being tested, and at the bottom a page that shrinks
    has to clamp, which is the browser being right.
    """
    at = d.eval(
        """
        (() => {
          const view = %s, target = %s;
          if (!view || !target) return null;
          const want = view.scrollTop + target.getBoundingClientRect().top
            - view.getBoundingClientRect().top - %d;
          view.scrollTop = Math.max(0, Math.min(want, view.scrollHeight - view.clientHeight));
          return {at: view.scrollTop, max: view.scrollHeight - view.clientHeight};
        })()
        """
        % (VIEW, element_js, above)
    )
    if not at:
        fail("the page or the element to scroll to was not on screen")
    if at["at"] < 1:
        fail(
            "the page could not be scrolled at all at this window size (%d px of "
            "overflow), so there was no offset to hold" % at["max"]
        )
    return at["at"]


def press(d, element_js, label):
    """Focus a control and activate it, as a keyboard user does, recording
    where the page was. `click()` never scrolls, which is the point: the drive's
    own `click` scrolls its target into view and would move the offset itself.
    """
    pressed = d.eval(
        """
        (() => {
          const scope = %s;
          if (!scope) return 'the element holding it is not on the page';
          const wanted = %s;
          const control = scope.matches('button, input') ? scope
            : [...scope.querySelectorAll('button')].find(b => !b.hidden
                && b.offsetParent !== null && (b.textContent || b.getAttribute('aria-label') || '').trim() === wanted);
          if (!control) return 'nothing on offer reads ' + wanted;
          const view = %s;
          window.__still.pressed = control;
          window.__still.label = wanted;
          window.__still.before = {scroll: view.scrollTop, top: control.getBoundingClientRect().top,
                                   loading: window.__still.loading};
          control.focus({preventScroll: true});
          control.click();
          return 'pressed';
        })()
        """
        % (element_js, json.dumps(label), VIEW)
    )
    if pressed != "pressed":
        fail("pressing %r: %s" % (label, pressed))


def held(d, what, focus=False, place=False):
    """Assert the page held still through `what`, measured against `press`."""
    seen = d.eval(
        """
        (() => {
          const view = %s, still = window.__still, control = still.pressed;
          return {scroll: view.scrollTop, before: still.before,
                  bottom: view.scrollHeight - view.clientHeight,
                  loading: still.loading - still.before.loading,
                  connected: control.isConnected,
                  top: control.isConnected ? control.getBoundingClientRect().top : null,
                  focused: document.activeElement === control,
                  active: document.activeElement ? document.activeElement.tagName.toLowerCase()
                    + ' ' + (document.activeElement.textContent || '').trim().slice(0, 30) : null};
        })()
        """
        % VIEW
    )
    before = seen["before"]
    if seen["loading"]:
        fail("%s showed the page loader; a live update is not a navigation" % what)
    if abs(seen["scroll"] - before["scroll"]) > 0.5:
        clamped = seen["scroll"] < before["scroll"] and abs(seen["scroll"] - seen["bottom"]) < 1
        fail(
            "%s moved the page's scroll offset from %.1f to %.1f%s"
            % (what, before["scroll"], seen["scroll"],
               " — clamped: the page is now too short to hold the offset, so "
               "shorten STILL_VIEWPORT" if clamped else "")
        )
    if place and (seen["top"] is None or abs(seen["top"] - before["top"]) > 1):
        fail(
            "%s moved the pressed control on screen from y=%.1f to %s (still in "
            "the page: %s): the page held its offset but the content under the "
            "reader jumped, or the control was rebuilt"
            % (what, before["top"], seen["top"], seen["connected"])
        )
    if focus and not (seen["connected"] and seen["focused"]):
        fail(
            "%s took the focus off the control that was pressed (still in the "
            "page: %s; focus is now on %s). A control rebuilt by its own repaint "
            "loses the focus a keyboard user just gave it."
            % (what, seen["connected"], seen["active"])
        )


def tidy(d, store):
    """After a check that started a run: stop it first, then put the window
    back. In that order, because a browser that has stopped answering must not
    leave a run holding the daemon's only slot for every check after it."""
    try:
        stop_quietly(d, store)
    finally:
        try:
            d.reset_viewport()
        except Exception:
            pass


def running(d, run_id, store):
    """Wait for a run to be admitted and reading, so it can be paused."""
    deadline = time.time() + RUN_APPEARS * 2
    while time.time() < deadline:
        run = run_by_id(d, run_id)
        status = run and str(run.get("status"))
        if status == "running":
            return
        if status in TERMINAL:
            fail("the run on %s was %s before it could be paused; the corpus is "
                 "too small for this machine" % (store, status))
        time.sleep(0.2)
    fail("the run on %s was never admitted within %ds" % (store, RUN_APPEARS * 2))


def control_run(d, store, action, run_id):
    d.api("/api/index/control", method="POST", body={"store": store, "action": action, "run": run_id})


def wait_status(d, run_id, statuses, what, timeout=60):
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        last = run_by_id(d, run_id)
        if last and last.get("status") in statuses:
            return last
        time.sleep(0.2)
    fail("%s: the run never reached %s within %ds (last: %s)"
         % (what, "/".join(sorted(statuses)), timeout, last and last.get("status")))


CARD = "document.querySelector(%s)" % json.dumps(LIVE_CARD)


@finding("8.1", "Pause, Resume and the Stop dialog leave the Runs tab where it was")
def _(d):
    """Pause, Resume, the Stop dialog cancelled and the Stop dialog confirmed,
    on the store's Runs tab with `<main>` scrolled so the live card is on
    screen and the offset is neither 0 nor the bottom. After each: the offset is
    where it was, the page never showed its loader, and — where the pressed
    button is still on offer afterwards — it is where it was and holds focus."""
    # Big enough to still be running after Pause, Resume and both Stop
    # dialogs on the fastest lane (the Neural Engine finished 400 first).
    run_id, store = start_index(d, d.fixtures.unique("still", count=4000))
    try:
        running(d, run_id, store)
        control_run(d, store, "pause", run_id)
        wait_status(d, run_id, {"paused"}, "pausing %s" % store)

        still_open(d, STILL_VIEWPORT, "store/%s/runs" % store)
        d.wait_for(card_offers(CARD, "Resume"), what="the paused run's card, offering Resume")
        hold_at(d, CARD, above=60)

        press(d, CARD, "Resume")
        d.wait_for(card_offers(CARD, "Pause"), what="Resume to turn into Pause")
        settle(d, "Resume")
        held(d, "Resume", focus=True, place=True)

        press(d, CARD, "Pause")
        d.wait_for(card_offers(CARD, "Resume"), what="Pause to turn into Resume")
        wait_status(d, run_id, {"paused"}, "Pause on the card")
        settle(d, "Pause")
        held(d, "Pause", focus=True, place=True)

        press(d, CARD, "Stop…")
        d.wait_for("!!document.querySelector(%s)" % json.dumps(MODAL), what="the Stop dialog")
        d.modal_press("Keep running")
        d.wait_for("!document.querySelector(%s)" % json.dumps(MODAL), what="the Stop dialog to close")
        settle(d, "the Stop dialog cancelled")
        # The dialog hands the focus back to the button that opened it, as
        # long as the page has not replaced that button meanwhile.
        held(d, "the Stop dialog cancelled", focus=True, place=True)
        if run_by_id(d, run_id).get("status") in TERMINAL:
            fail("cancelling the Stop dialog stopped the run anyway")

        # Confirmed with "Also delete the store" as the dialog offers it: this
        # run is creating its store, so the box starts ticked, and the stop
        # takes the store with it — off the sidebar's count too, with no reload.
        stores_before = sidebar_stores(d)
        press(d, CARD, "Stop…")
        d.wait_for("!!document.querySelector(%s)" % json.dumps(MODAL), what="the Stop dialog")
        ticked = d.eval("(document.querySelector('#stop-delete') || {}).checked")
        if ticked is not True:
            fail(
                "the Stop dialog for %s, a store this run is creating, left 'Also "
                "delete the store' %s; stopping the run that makes a store should "
                "offer to take the empty store with it"
                % (store, "unticked" if ticked is False else "missing")
            )
        d.modal_press("Stop and undo")
        wait_status(d, run_id, {"stopped"}, "Stop and undo")
        # 0.35.0: the store's page goes with the store. Where the stopped card
        # said "the store was deleted" and offered Remove, the page now says
        # there is no store by that name — the same fact, on the page that was
        # open, with no reload.
        d.wait_for(
            "window.__still && /There is no store called %s/.test(document.querySelector('#main').innerText)"
            % re.escape(store),
            timeout=60,
            what="the open page to say %s is gone, without a reload" % store,
        )
        d.wait_for(
            "window.__still && (() => { const m = /(\\d+) stores?/.exec("
            "(document.querySelector('.nav .daemon .facts') || {}).textContent || '');"
            " return !!m && Number(m[1]) === %d; })()" % (stores_before - 1),
            what="the sidebar to count %d store(s) after the delete, without a reload" % (stores_before - 1),
        )
    finally:
        tidy(d, store)


@finding("8.2", "a limit saved and a run finishing leave the page where it was")
def _(d):
    """A limit saved on Settings › Performance, with `<main>` scrolled so the
    Limits card is at the top of the view, which is where a reader is when they
    change one; and a run finishing on its store's Runs tab. A save used to
    announce itself above the card being changed and push it down a line, and a
    rebuilt card put new controls in place of the focused one.

    The old check also removed a finished card and pressed "Remove all
    finished". 0.35.0 owner decision (v6 as drawn): a finished run leaves the
    live area for History by itself, which is the "run finishing" asserted
    here; there is no card to remove by hand.
    """
    run_id, store = start_index(d, d.fixtures.unique("finishing", count=200))
    try:
        running(d, run_id, store)
        control_run(d, store, "pause", run_id)
        wait_status(d, run_id, {"paused"}, "pausing %s" % store)

        limits = d.api("/api/index/runs")["limits"]
        if limits["runs_at_once"].get("source") == "set by the environment":
            skip("runs at once is set by this daemon's environment, so the page cannot save it")
        still_open(d, STILL_VIEWPORT, "settings")
        card = ("[...document.querySelectorAll('#main .card')].find(c =>"
                " /^limits$/i.test(((c.querySelector('.card-t') || {}).textContent || '').trim()))")
        hold_at(d, card, above=8)
        # Up one and back down, so the drive leaves the machine as it found it.
        for label in ("Raise Runs at once", "Lower Runs at once"):
            button = "document.querySelector('#main button[aria-label=\"%s\"]')" % label
            # At its end a stepper button is aria-disabled, so it keeps focus.
            if d.eval("!%s || %s.disabled || %s.getAttribute('aria-disabled') === 'true'" % (button, button, button)):
                continue
            press(d, button, label)
            # The daemon applies all three limits at once and says what it is
            # running with; the page shows that answer.
            try:
                d.wait_for(
                    "[...document.querySelectorAll('.toast')].some(t => /now running with/.test(t.innerText))",
                    timeout=10, what="the save to be answered",
                )
            except cdp.ProtocolError:
                fail(
                    "pressing %r saved the limit and the page never said what the "
                    "daemon is now running with — /api/index/settings answers "
                    "'applied: now running with …' and the page drops it" % label
                )
            if re.search(r"applies to the next run", view_text(d)):
                fail("the page says a limit applies to the next run; it applies now")
            settle(d, "a limit saved")
            held(d, "a limit saved (%s)" % label, focus=True, place=True)

        d.reset_viewport()
        still_open(d, STILL_VIEWPORT, "store/%s/runs" % store)
        d.wait_for(card_offers(CARD, "Resume"), what="the paused run's card")
        hold_at(d, CARD, above=30)
        press(d, CARD, "Resume")
        wait_status(d, run_id, TERMINAL, "the run on %s to finish" % store, timeout=RUN_FINISHES)
        d.wait_for("!document.querySelector(%s) && document.querySelectorAll(%s).length > 0"
                   % (json.dumps(LIVE_CARD), json.dumps(HIST_ROW)),
                   timeout=30, what="the finished run to leave the live area for History")
        settle(d, "a run finishing")
        held(d, "a run finishing")
    finally:
        tidy(d, store)


OBSERVE = r"""
new Promise(done => {
  const still = window.__still, loading = still.loading, runs = still.runs;
  const target = %(target)s;
  const keep = %(keep)s;
  const found = [];
  const name = n => n.nodeType === 1 ? n.tagName.toLowerCase()
    + (n.className && typeof n.className === 'string' ? '.' + n.className.split(' ').join('.') : '') : '#text';
  const observer = new MutationObserver(records => {
    for (const r of records) {
      if (r.type !== 'childList') continue;
      if (keep && r.target.closest && r.target.closest(keep)) continue;
      if (!target.isConnected) { found.push('the observed card itself was replaced'); continue; }
      found.push(name(r.target) + ' lost ' + [...r.removedNodes].map(name).join(',')
        + ' gained ' + [...r.addedNodes].map(name).join(','));
    }
  });
  // The observed node and every ancestor up to <main>, so a card replaced
  // whole by its parent's repaint is seen as well as one rebuilt from inside.
  observer.observe(document.querySelector('#main'), {childList: true, subtree: true});
  const rates = {samples: 0, missing: [], lapsed: [], numbered: false};
  const sampler = %(rate)s ? setInterval(() => {
    const card = document.querySelector('#main .card.blue-edge');
    if (!card) return;
    const word = (card.querySelector('.pill') || {}).textContent || '';
    if (!/indexing|catching up/.test(word)) return;
    rates.samples++;
    // The RATE tile of the one run card (0.37.0-rc.4, W8): "N chunks/s",
    // or "—" before the first batch.
    const tile = [...card.querySelectorAll('.run-stats > div')].find(t => /RATE/.test((t.querySelector('.eyebrow') || {}).textContent || ''));
    const line = ((tile && tile.querySelector('.v')) || {}).textContent || '';
    const m = /^(—|[\d.,]+)(?: chunks\/s)?$/.exec(line.trim());
    if (!m) rates.missing.push(line.slice(0, 80) || '(empty)');
    else if (m[1] === '—' && rates.numbered) rates.lapsed.push(line.slice(0, 80));
    else if (m[1] !== '—') rates.numbered = true;
  }, 250) : null;
  setTimeout(() => {
    if (sampler) clearInterval(sampler);
    observer.takeRecords();
    observer.disconnect();
    done({found: [...new Set(found)].slice(0, 5), count: found.length,
          loading: still.loading - loading, paints: still.runs - runs, rates});
  }, %(ms)d);
})
"""


def observe(d, target_js, keep, ms, rate=False):
    return d.eval(
        OBSERVE % {"target": target_js, "keep": json.dumps(keep), "ms": ms, "rate": "true" if rate else "false"},
        timeout=ms / 1000 + 60,
    )


@finding("8.3", "over a live run nothing on screen is rebuilt or retyped, and the rate is on every poll")
def _(d):
    """Sixty seconds of a live run under a MutationObserver, in three windows of
    twenty, because 0.35.0 spread the old Index page over three: Settings ›
    Performance (the limits, which may change their words but not their
    nodes), the store's Runs tab (the live card, which may grow its log and
    nothing else, and shows its rate on every read), and the store's Settings
    tab, where a name typed and not saved keeps its input, its focus and its
    text. The page used to rebuild the limits card about once a second, so
    what was being typed went with the input it was typed into — app.js's own
    header promises that never happens again. The loader never appears.

    Fifteen hundred files nobody has indexed keep the run reading for the whole
    minute; each window fails rather than passes if the page did not repaint
    often enough to prove anything."""
    # Sized for the fastest lane, not the slowest: on Apple's Neural Engine
    # (about 230 chunks a second) fifteen hundred files were done in ten
    # seconds, before the second window opened. The run is tidied away at the
    # end, so a slow CPU-only runner never waits for all of it.
    run_id, store = start_index(d, d.fixtures.unique("steady", count=15000))
    try:
        running(d, run_id, store)

        still_open(d, STILL_VIEWPORT, "settings")
        card = ("[...document.querySelectorAll('#main .card')].find(c =>"
                " /^limits$/i.test(((c.querySelector('.card-t') || {}).textContent || '').trim()))")
        seen = observe(d, card, None, 20000)
        if seen["loading"]:
            fail("the loader appeared %d time(s) on Settings › Performance during a live run" % seen["loading"])
        if seen["count"]:
            fail(
                "Settings › Performance had %d node replacement(s) in twenty seconds "
                "with no limit changed, e.g. %s. A card rebuilt under the cursor "
                "cannot be used." % (seen["count"], seen["found"])
            )
        if seen["paints"] < 5:
            fail("Settings › Performance read the runs only %d time(s) in twenty seconds, "
                 "so the observation proves nothing about repainting" % seen["paints"])

        d.reset_viewport()
        still_open(d, STILL_VIEWPORT, "store/%s/runs" % store)
        d.wait_for("!!%s" % CARD, what="the live run's card")
        # 0.37.0-rc.4 owner decision (W8): the live card is the wizard's run
        # card. Its log grows a line per file; its Pipeline is fixed in shape,
        # so nothing else on the tab may be rebuilt.
        seen = observe(d, CARD, ".log", 20000, rate=True)
        if seen["loading"]:
            fail("the loader appeared %d time(s) on the Runs tab during a live run" % seen["loading"])
        if seen["count"]:
            fail(
                "parts of the Runs tab outside the live card's log were rebuilt %d "
                "time(s) in twenty seconds instead of updated in place: %s"
                % (seen["count"], seen["found"])
            )
        rates = seen["rates"]
        if rates["missing"]:
            fail(
                "the live card showed no rate on %d of %d reads, e.g. %r: the "
                "rolling rate is on the card on every poll while a run is live, "
                "'—' until its first batch" % (len(rates["missing"]), rates["samples"], rates["missing"][:3])
            )
        if rates["lapsed"]:
            fail("the live card's rate went back to '—' after it had a number (%r)" % rates["lapsed"])
        if rates["samples"] and not rates["numbered"]:
            fail("in %d reads of a live run the card never showed a rate" % rates["samples"])
        if seen["paints"] < 5:
            fail("the Runs tab read the runs only %d time(s) in twenty seconds" % seen["paints"])

        d.reset_viewport()
        still_open(d, STILL_VIEWPORT, "store/%s/settings" % store)
        field = '#main input[aria-label="Store name"]'
        typed = d.eval(
            """
            (() => {
              const f = document.querySelector(%s);
              if (!f) return null;
              window.__still.typed = {field: f, was: f.value};
              f.focus({preventScroll: true});
              f.value = f.value + '-typed';
              f.dispatchEvent(new Event('input', {bubbles: true}));
              return f.value;
            })()
            """
            % json.dumps(field)
        )
        if typed is None:
            fail("the store's Settings tab has no name field to type into")
        seen = observe(d, "document.querySelector(%s)" % json.dumps(field), None, 20000)
        kept = d.eval(
            """
            (() => {
              const t = window.__still.typed, now = document.querySelector(%s);
              const kept = {same: now === t.field, focused: document.activeElement === (now || t.field),
                            value: (now || t.field).value};
              (now || t.field).value = t.was;
              (now || t.field).dispatchEvent(new Event('input', {bubbles: true}));
              (now || t.field).blur();
              return kept;
            })()
            """
            % json.dumps(field)
        )
        if not (kept["same"] and kept["focused"] and kept["value"] == typed):
            fail(
                "twenty seconds of repaints did not leave the field being typed into "
                "alone: the same input %s, it %s the focus, and it holds %r where %r "
                "was typed (%d node replacement(s) seen, e.g. %s)"
                % ("is still there" if kept["same"] else "was replaced",
                   "kept" if kept["focused"] else "lost",
                   kept["value"], typed, seen["count"], seen["found"][:2])
            )
        if seen["paints"] < 5:
            fail("the Settings tab read the runs only %d time(s) in twenty seconds" % seen["paints"])
    finally:
        tidy(d, store)


@finding("8.4", "Pause reads 'pausing' the moment it is pressed, and never goes back to indexing")
def _(d):
    """The route answers `{"state": "pausing"}` at once and the engine stops at
    its next batch. The card says "pausing" on that answer, not a poll later,
    and no poll asked before the click paints "indexing" back over it — nor
    does it claim "paused" before the engine is there.

    Every word the live card's pill shows is sampled every 30 ms from before
    the click until the run is paused — re-reading the card each time, so a
    card the repaint replaced is still followed."""
    run_id, store = start_index(d, d.fixtures.unique("pausing", count=400))
    try:
        running(d, run_id, store)
        still_open(d, STILL_VIEWPORT, "store/%s/runs" % store)
        d.wait_for(card_offers(CARD, "Pause"), what="the live run's card, offering Pause")
        seen = d.eval(
            """
            new Promise(done => {
              const pill = () => { const c = document.querySelector('#main .card.blue-edge');
                const p = c && c.querySelector('.pill'); return p ? p.textContent : ''; };
              const words = [];
              const note = () => { const w = pill().trim(); if (w && words[words.length - 1] !== w) words.push(w); };
              note();
              const at = words.length;
              const tick = setInterval(note, 30);
              const card = document.querySelector('#main .card.blue-edge');
              [...card.querySelectorAll('button')].find(b => !b.hidden && b.textContent.trim() === 'Pause').click();
              const started = Date.now();
              const stop = setInterval(async () => {
                const r = await fetch('/api/index/runs', {headers: {'Semlith-Token': sessionStorage.getItem('semlith.token')}}).then(x => x.json());
                const mine = (r.runs || []).find(x => x.id === %d);
                if ((mine && mine.status === 'paused' && /paused/.test(pill())) || Date.now() - started > 30000) {
                  clearInterval(tick); clearInterval(stop); note();
                  done({before: words.slice(0, at), after: words.slice(at)});
                }
              }, 200);
            })
            """
            % run_id,
            timeout=60,
        )
        after = seen["after"]
        if not after or after[0] != "pausing":
            fail(
                "after Pause the pill read %r; the first word after the click is "
                "'pausing', from the route's own answer, before the engine gets "
                "there — 'paused' at that moment claims a stop that has not "
                "happened yet" % after
            )
        if "indexing" in after:
            fail("after Pause the pill went back to 'indexing' (%r): a poll asked "
                 "before the click was painted over the pausing state" % after)
        if after[-1] != "paused":
            fail("the run never reached 'paused' within 30s of Pause: %r" % after)
    finally:
        tidy(d, store)


def quiet(d, timeout=180):
    """Wait, bounded, until no run is live anywhere, so the next check's run is
    admitted at once rather than queued behind an earlier check's tidying."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        answer = d.api("/api/index/runs")
        live = [r for r in answer.get("runs") or [] if r.get("status") not in TERMINAL]
        if not live and not answer.get("queue"):
            return
        time.sleep(0.5)


@finding("8.5", "the Stop dialog never offers to delete a store that held files, and says it stays as it was")
def _(d):
    """A store that already held files is a store somebody has: stopping a run
    on it must not delete it by default, and the dialog says what is at stake.
    (A store the run is creating is offered, ticked: 8.1.)

    0.35.0 owner decision (v6 as drawn): for a store that held files the delete
    option is not offered at all, which is the old "unticked" made stronger,
    and the dialog's sentence is the design's — what the run embedded is undone
    and the store is left exactly as it was before the run — where the v4
    dialog counted the files that stay.

    The run is the one the store's own watcher starts when four hundred new
    files land in its folder: a run on a store that held three."""
    quiet(d)
    corpus = d.fixtures.unique("kept", count=3)
    store = indexed_fixture(d, corpus)
    d.fixtures._write_corpus(corpus, 400, prefix="more")
    deadline = time.time() + RUN_APPEARS * 2
    run = None
    while time.time() < deadline and run is None:
        run = next((r for r in d.api("/api/index/runs")["runs"]
                    if r["store"] == store and r.get("status") == "running"), None)
        time.sleep(0.2)
    if run is None:
        fail("four hundred files written into %s's folder started no run on it "
             "within %ds; the store's watcher should read them" % (store, RUN_APPEARS * 2))
    run_id = run["id"]
    try:
        control_run(d, store, "pause", run_id)
        wait_status(d, run_id, {"paused"}, "pausing %s" % store)
        before = (run_by_id(d, run_id) or {}).get("files_before")
        if not before:
            fail("the daemon says %s held %r files before this run; it held at least 3" % (store, before))
        d.open_view("store/%s/runs" % store)
        d.wait_for(card_offers(CARD, "Stop…"), what="the paused run's card on %s, offering Stop" % store)
        press_in(d, CARD, "Stop…")
        d.wait_for("!!document.querySelector(%s)" % json.dumps(MODAL), what="the Stop dialog")
        dialog = d.eval("({box: !!document.querySelector('#stop-delete'), text: document.querySelector(%s).innerText})"
                        % json.dumps(MODAL))
        d.modal_press("Keep running")
        if dialog["box"]:
            fail("the Stop dialog for %s, which held %d files before this run, offers to "
                 "delete the store" % (store, before))
        if "left exactly as it was before the run" not in dialog["text"]:
            fail("the Stop dialog does not say the store is left as it was: %r" % dialog["text"][:300])
    finally:
        stop_quietly(d, store)
        quiet(d)


@finding("8.6", "a store deleted by a stop leaves the Stores list and the sidebar count without a reload")
def _(d):
    quiet(d)
    # A second store, so deleting this one does not leave the machine with
    # none, which is the Welcome screen and has no sidebar to count with.
    indexed_fixture(d, d.fixtures.unique("stays"))
    run_id, store = start_index(d, d.fixtures.unique("deleted", count=400))
    try:
        running(d, run_id, store)
        control_run(d, store, "pause", run_id)
        wait_status(d, run_id, {"paused"}, "pausing %s" % store)
        d.open_view("stores", fresh=True)
        stores_widest(d)
        listed = "!!(%s)" % store_row_js(store)
        d.wait_for(listed, what="%s in the Stores list" % store)
        before = sidebar_stores(d)
        d.eval("window.__sameDocument = true")
        d.api("/api/index/control", method="POST",
              body={"store": store, "action": "stop", "run": run_id, "delete": True})
        try:
            d.wait_for(
                "window.__sameDocument === true && !(%s) && (() => {"
                " const m = /(\\d+) stores?/.exec((document.querySelector('.nav .daemon .facts') || {}).textContent || '');"
                " return !!m && Number(m[1]) === %d; })()" % (listed, before - 1),
                timeout=60,
                what="%s to leave the Stores list and the sidebar to count %d, without a reload" % (store, before - 1),
            )
        except cdp.ProtocolError:
            seen = d.eval(
                "({same: window.__sameDocument === true,"
                " sidebar: (document.querySelector('.nav .daemon .facts') || {}).textContent})"
            )
            fail("a minute after %s's stop deleted it, the Stores list %s: the sidebar reads "
                 "%r (it counted %d before)" % (store, "is the same document" if seen["same"] else "was reloaded",
                                                seen["sidebar"], before))
    finally:
        stop_quietly(d, store)


@finding("8.7", "Settings › Performance lists the accelerator lanes, with the CPU active")
def _(d):
    """One row per lane `/api/accel` reports, each a switch with its device, its
    state and its share of the work. The CPU is always a lane and always
    active; CI runners have no GPU, so nothing here depends on one.

    Turning a lane on that needs a download says what it downloads before
    anything is posted: the dialog is opened and cancelled, and the lane is
    still off afterwards."""
    accel = d.api("/api/accel")
    lanes = accel.get("lanes") or []
    d.open_view("settings")
    d.wait_for("document.querySelectorAll('#main .lane-row').length === %d" % len(lanes),
               what="one row per accelerator lane (%d) on Settings › Performance" % len(lanes))
    rows = d.eval(
        "[...document.querySelectorAll('#main .lane-row')].map(r => ({"
        " title: ((r.querySelector('.col .row') || {}).textContent || '').trim(),"
        " state: ((r.querySelector('.col .muted') || {}).textContent || '').trim(),"
        " on: (r.querySelector('[role=switch]') || {getAttribute(){return null}}).getAttribute('aria-checked'),"
        " share: ((r.querySelector('.right') || {}).textContent || '').trim()}))"
    )
    names = [row["title"].split(" · ")[0].replace("experimental", "").strip() for row in rows]
    for wanted in ("CPU", "GPU", "CUDA"):
        if wanted not in names:
            fail("the lanes card has no %s row; it lists %r" % (wanted, names))
    cpu = rows[names.index("CPU")]
    if "active" not in cpu["state"]:
        fail("the CPU row reads %r; the CPU lane is always active" % cpu["state"])
    # Matched by name: from 0.35.0 the lanes this machine cannot run are
    # listed after the others, under their own line, and read "off".
    by_label = {(l.get("label") or l["lane"]): l for l in lanes}
    for row, name in zip(rows, names):
        lane = by_label.get(name)
        if lane is None:
            fail("the lanes card lists %r, which /api/accel does not report" % name)
        if row["on"] != ("true" if lane.get("enabled") else "false"):
            fail("the %s switch reads %s and the daemon has it %s"
                 % (row["title"], row["on"], "on" if lane.get("enabled") else "off"))
        unavailable = (lane.get("status") or {}).get("state") == "unavailable"
        if unavailable:
            if row["share"] != "off":
                fail("the %s row cannot run here and reads %r, not 'off'" % (row["title"], row["share"]))
        elif not re.match(r"^\d+% of the work$", row["share"]) and not row["share"].startswith(("cosine", "agrees", "differs")):
            fail("the %s row's share of the work reads %r, not 'N%% of the work'" % (row["title"], row["share"]))

    # A lane that downloads before it runs asks first, naming the size.
    candidate = next((l for l in lanes if not l.get("enabled") and l.get("download_bytes")
                      and not l.get("installed") and (l.get("status") or {}).get("state") != "unavailable"), None)
    if candidate is None:
        return
    label = candidate.get("label") or candidate["lane"]
    d.click('#main .lane-row button[aria-label="%s lane"]' % label)
    d.wait_for("!!document.querySelector(%s)" % json.dumps(MODAL),
               what="a confirmation before %s is turned on" % label)
    said = d.modal_text()
    d.modal_press("Cancel")
    if not re.search(r"\d+(\.\d)? (MB|GB|KB)", said):
        fail("turning %s on did not say its download size first: %r" % (label, said[:300]))
    still = next(l for l in d.api("/api/accel")["lanes"] if l["lane"] == candidate["lane"])
    if still.get("enabled"):
        fail("cancelling the %s confirmation turned it on anyway" % label)


@finding("8.8", "the Privacy page lists every download, where from, its size, when, and whether it is here")
def _(d):
    downloads = d.api("/api/privacy").get("downloads") or []
    if not downloads:
        fail("/api/privacy lists no downloads; the embedding model at least is one")
    d.open_view("privacy")
    rows = d.eval(
        """
        (() => {
          const card = [...document.querySelectorAll('#main .card')].find(c =>
            /ever fetches/.test((c.querySelector('.card-t') || {}).textContent || ''));
          if (!card) return null;
          return [...card.querySelectorAll('.dl-row')].map(r => ({
            what: ((r.querySelector('.t-m') || {}).textContent || '').trim(),
            sub: ((r.querySelector('.muted') || {}).textContent || '').trim(),
            pill: ((r.querySelector('.pill') || {}).textContent || '').trim()}));
        })()
        """
    )
    if rows is None:
        fail("the Privacy page has no list of what semlith fetches")
    if len(rows) < len(downloads):
        fail("the list has %d rows; /api/privacy lists %d downloads" % (len(rows), len(downloads)))
    for row, download in zip(rows, downloads):
        # 0.35.0 owner decision (revised v6 design): the portal never names the
        # embedding model, so the row is the route's words without the bracketed
        # model detail — "the embedding model", not which one. Everything else
        # in the row is still the route's.
        plain = re.sub(r"\s*\([^)]*\)", "", download["what"])
        expected = plain[0].upper() + plain[1:]
        if row["what"] != expected:
            fail("a download row reads %r where the route says %r" % (row["what"], download["what"]))
        parts = [p.strip() for p in row["sub"].split("·")]
        if len(parts) < 3 or parts[0] != download["source"]:
            fail("the source of %r reads %r, not %r" % (download["what"], row["sub"], download["source"]))
        if not re.match(r"^[\d.]+ (B|KB|MB|GB)$", parts[1]):
            fail("the size of %r reads %r" % (download["what"], parts[1]))
        if parts[2] != download["when"]:
            fail("when %r happens reads %r, not %r" % (download["what"], parts[2], download["when"]))
        here = "on disk" if download["cached"] else "never fetched"
        if row["pill"] != here:
            fail("%r reads %r; the route says it is %s" % (download["what"], row["pill"], here))


# ---------------------------------------------------------------- 0.29.0
#
# The eleven portal fixes the owner found using 0.28.0, one check each.


@finding("9.1", "a running card counts down from the daemon's estimate, not up from submission")
def _(d):
    """The snapshot carries `bytes`, `bytes_total` and `eta_ms`; the card says
    `estimating…` until the estimate exists and then the time left, and never
    a clock counting up."""
    quiet(d)
    run_id, store = start_index(d, d.fixtures.unique("countdown", count=400))
    try:
        running(d, run_id, store)
        deadline = time.time() + 90
        run = None
        while time.time() < deadline:
            run = run_by_id(d, run_id)
            if not run or run.get("status") in TERMINAL or run.get("eta_ms") is not None:
                break
            time.sleep(0.5)
        if not run or run.get("status") in TERMINAL:
            skip("the run finished before its rate settled; the machine is faster than the corpus")
        if run.get("eta_ms") is None:
            fail("90 seconds into a live run the snapshot still has no eta_ms: %s"
                 % json.dumps({k: run.get(k) for k in ("status", "bytes", "bytes_total", "chunks")}))
        if not run.get("bytes_total") or not (0 < run.get("bytes", 0) <= run["bytes_total"]):
            fail("the snapshot's bytes are %r of %r" % (run.get("bytes"), run.get("bytes_total")))
        d.open_view("store/%s/runs" % store)
        # 0.37.0-rc.4: the one run card says its time left in its TIME LEFT
        # meter as "about 12 min" or "under a minute" (the run-truth spec's
        # words), where the old card's line read "12 min left".
        d.wait_for("(() => { const c = %s; return !!c && /left|almost done|estimating|about \\d+ (min|h)|under a minute/.test("
                   "[...c.querySelectorAll('.run-stats .v, .t-mono-sm')].map(n => n.textContent).join(' ')); })()" % CARD,
                   timeout=20, what="%s's card to show the time left" % store)
        # The card without its log: since 0.37.0-rc.4 the log sits inside the
        # card (one run card everywhere), and its lines carry the wall-clock
        # time they were said, which is a label, not a clock counting up.
        text = d.eval("(() => { const c = (%s).cloneNode(true); c.querySelectorAll('.log').forEach((l) => l.remove()); return c.innerText; })()" % CARD)
        if re.search(r"\b\d{2}:\d{2}\b", text):
            fail("the running card still shows a clock: %r" % text[:200])
    finally:
        stop_quietly(d, store)


@finding("9.2", "a finished run says how long the work took and when it finished")
def _(d):
    store = indexed_fixture(d, d.fixtures.unique("finished"))
    run = run_for(d, store)
    if not run or not run.get("started_at") or not run.get("finished_at"):
        fail("the finished run's snapshot has no started_at/finished_at: %s" % json.dumps(run)[:300])
    if run["started_at"] < run["submitted"] or run["finished_at"] < run["started_at"]:
        fail("submitted %s, started %s, finished %s are out of order"
             % (run["submitted"], run["started_at"], run["finished_at"]))
    d.open_view("store/%s/runs" % store, fresh=True)
    text = newest_history_row(d, run)
    if text is None:
        fail("%s's Runs tab never drew the row for its run %s" % (store, run.get("id")))
    hhmm = d.eval("new Date(%d * 1000).toTimeString().slice(0, 5)" % run["finished_at"])
    if "took " not in text or hhmm not in text:
        fail("the finished run's row reads %r; it should say 'took …' and when it finished (%s)" % (text[:200], hhmm))


@finding("9.3", "no line is held above the run cards")
def _(d):
    """Nothing but cards on a store's Runs tab: no note on a line of its own
    above them, taking room while it has nothing to say."""
    store = a_store(d)
    d.open_view("store/%s/runs" % store, fresh=True)
    stray = d.eval(
        "(() => { const host = document.querySelector('#main .page > .stack:last-child');"
        " if (!host) return null; return [...host.children].filter(c => !c.classList.contains('card'))"
        ".map(c => c.className + ': ' + (c.innerText || '').slice(0, 60)); })()"
    )
    if stray is None:
        fail("the Runs tab has no list of cards to read")
    if stray:
        fail("the Runs tab holds something other than run cards above or between them: %r" % stray)


@finding("9.4", "every paginated table opens at the design's page size")
def _(d):
    # 0.35.0 owner decision (v6 as drawn): tables open at 10 rows a page, and a
    # store's Files table at 25, where v4's opened at 5. The property is that a
    # table opens at its stated size and shows no more rows than that.
    store = a_store(d)
    d.api("/api/search?query=release%20record&k=4")
    for route, size in (("stores", 10), ("store/%s/files" % store, 25), ("store/%s/review" % store, 10),
                        ("ledger", 10), ("ledger/retrievals", 10)):
        d.open_view(route, fresh=True)
        pause(d, 800)
        seen = d.eval(
            "[...document.querySelectorAll('#main .pager')].map(p => {"
            " const card = p.closest('.card') || p.closest('.grid-wrap');"
            " const rows = card.querySelectorAll('tbody tr, .gl-row.gl-stores').length;"
            " return {per: (p.querySelector('.dd') || {dataset: {}}).dataset.value, rows}; })"
        )
        if not seen:
            fail("%s draws no paginated table" % route)
        for table in seen:
            if table["per"] != str(size):
                fail("on %s a table opens at %r per page, not %d" % (route, table["per"], size))
            if table["rows"] > size:
                fail("on %s a table shows %d rows on its first page of %d" % (route, table["rows"], size))


@finding("9.5", "each store's Forget asks in a confirm that names it, and takes it")
def _(d):
    # 0.35.0 owner decision (v6 as drawn): the Stores list has no multi-select,
    # so stores are forgotten one by one from each row's menu — the bulk confirm
    # that named every store it would delete has no v6 surface. What it was for
    # is asserted per store: the confirm names the store it deletes, and when it
    # closes the store is gone from the list and from the daemon.
    names = [indexed_fixture(d, d.fixtures.unique("bulk")) for _ in range(2)]
    d.open_view("stores", fresh=True)
    for name in names:
        stores_widest(d)
        d.wait_for("!!(%s)" % store_row_js(name), what="%s in the Stores list" % name)
        opened = d.eval(
            "(() => { const row = %s; row.querySelector('button[aria-haspopup]').click();"
            " const item = [...document.querySelectorAll('.menu .menu-item')].find(b => /^Forget/.test(b.textContent.trim()));"
            " if (!item) return false; item.click(); return true; })()" % store_row_js(name)
        )
        if not opened:
            fail("the %s row's menu offers no Forget" % name)
        d.wait_for("!!document.querySelector(%s)" % json.dumps(MODAL), what="the Forget confirm for %s" % name)
        body = d.modal_text()
        if name not in body:
            fail("the Forget confirm does not name %s: %r" % (name, body[:300]))
        d.modal_press("Forget store")
        d.wait_for("!(%s)" % store_row_js(name), timeout=60, what="%s to leave the list" % name)
    left = {s["name"] for s in d.api("/api/stores")["stores"]}
    if left & set(names):
        fail("the daemon still lists %s" % sorted(left & set(names)))


@finding("9.6", "the graph's two actions are whole and on screen, and the selected name is never cut")
def _(d):
    a_store(d)
    d.open_view("graph")
    d.wait_for("document.querySelectorAll('.g-sel .two button').length === 2", timeout=30,
               what="a selected symbol's actions")
    for scrolled in ("top", "bottom"):
        d.eval("(() => { const s = document.querySelector('.g-side-scroll'); if (s) s.scrollTop = %s; })()"
               % ("0" if scrolled == "top" else "s.scrollHeight"))
        clipped = d.eval(
            "[...document.querySelectorAll('.g-sel .two button')].filter(b => {"
            " const r = b.getBoundingClientRect();"
            " return r.height === 0 || r.top < 0 || r.bottom > innerHeight + 0.5 || r.right > innerWidth + 0.5;"
            " }).map(b => b.textContent.trim())"
        )
        if clipped:
            fail("with the side column scrolled to its %s, %s is cut off" % (scrolled, clipped))
    # 0.29.0 kept a long name to one line with an ellipsis and the full name on
    # hover. 0.35.0 owner decision (v6 as drawn): the name breaks across lines
    # and is shown whole. Either way the property is that the name is never
    # cut off with nothing carrying the rest, which is asserted on both shapes.
    sym = d.eval(
        "(() => { const h = document.querySelector('.g-sel .nm'); const s = getComputedStyle(h);"
        " const t = h.closest('[data-tip], [title]');"
        " return {ws: s.whiteSpace, to: s.textOverflow, wb: s.wordBreak, cut: h.scrollWidth > h.clientWidth + 1,"
        " full: t ? (t.getAttribute('data-tip') || t.getAttribute('title')) : '', text: h.textContent}; })()"
    )
    if sym["cut"] and sym["full"] != sym["text"]:
        fail("the selected symbol's name is cut off with no full name on hover: %r" % sym)
    if sym["ws"] == "nowrap" and sym["to"] != "ellipsis" and sym["cut"]:
        fail("the selected symbol's name is held to one line and clipped without an ellipsis: %r" % sym)


@finding("9.7", "Blast radius opens on the graph's symbol and its store")
def _(d):
    a_store(d)
    d.open_view("graph")
    d.wait_for("document.querySelectorAll('.g-sel .two button').length === 2 && !/nothing yet/.test(document.querySelector('.g-sel .nm').textContent)",
               timeout=30, what="a selected symbol's actions")
    picked = d.eval(
        "({name: document.querySelector('.g-sel .nm').textContent.trim(),"
        " store: ((document.querySelector('.g-bar button[aria-haspopup] .mono') || {}).textContent || '').trim()})"
    )
    press_text(d, ".g-sel .two button", "Blast radius")
    d.wait_for("location.hash === '#/graph/blast'", what="Blast radius")
    d.wait_for("!document.querySelector('#main .spinner') && !!document.querySelector('#main .q3, #main .error-box')",
               timeout=40, what="the answer")
    asked = d.eval("document.querySelector('.ctrl-card input[aria-label=\"Symbol\"]').value")
    want("the symbol Blast radius asks about", asked, picked["name"])
    scope = d.eval("((document.querySelector('.ctrl-card button[aria-haspopup] .mono') || {}).textContent || '').trim()")
    want("the store Blast radius reads", scope, picked["store"])
    headline = text_of(d, "#main .card.pad .big14", "the answer's headline")
    if picked["name"] not in headline:
        fail("Blast radius answered %r, which does not name %s" % (headline, picked["name"]))


@finding("9.8", "the Brief view draws spans as cards, and search meta sits under the box")
def _(d):
    a_store(d)
    d.open_view("search")
    d.type(SEARCH_BOX, "release record")
    d.press("Enter")
    d.wait_for("/\\d/.test((document.querySelector('#main .sr-bar .t-mono-sm') || {}).textContent || '')",
               timeout=60, what="the search's count and timing")
    if exists(d, "#main .sr-field .t-mono-sm"):
        fail("the hit count and timing are still inside the query box")
    press_text(d, "#main .seg button", "Brief")
    d.wait_for("/BRIEF/.test(document.querySelector('#main').innerText) && !document.querySelector('#main .spinner')",
               timeout=60, what="the Brief view")
    shape = d.eval(
        "({cards: document.querySelectorAll('#main .card .lines .l .n').length,"
        " bare: document.querySelectorAll('#main pre').length,"
        " strip: /tokens[\\s\\S]*spans[\\s\\S]*dropped/.test(document.querySelector('#main').innerText)})"
    )
    if not shape["cards"] or shape["bare"] or not shape["strip"]:
        fail("the Brief view is not drawn as span cards with a summary strip: %r" % shape)


@finding("9.9", "Blast radius says what its figures mean and where to start")
def _(d):
    a_store(d)
    d.open_view("graph/blast", fresh=True)
    starts = texts_of(d, "#main .kind-card .mono")
    if not starts:
        fail("an empty Blast radius offers no symbol to start from")
    # Where to start is a symbol this store has, read from the daemon — not a
    # name the page made up because it had not read the store's hubs yet.
    store = d.eval("((document.querySelector('.ctrl-card button[aria-haspopup] .mono') || {}).textContent || '').trim()")
    for name in starts:
        found = d.api("/api/symbol?name=%s&store=%s&k=1" % (urllib.parse.quote(name), urllib.parse.quote(store)))
        if not found.get("symbols"):
            fail("Blast radius suggests starting from %r, which %s does not define" % (name, store))
    if not all(t in texts_of(d, "#main .tabs .tab") for t in ("Explore", "Path & evidence")):
        fail("Explore or Path & evidence is no longer one tab away from Blast radius")
    d.click("#main .kind-card")
    d.wait_for("!document.querySelector('#main .spinner') && !!document.querySelector('#main .q3, #main .error-box')",
               timeout=40, what="a starting symbol to be reached")
    figures = texts_of(d, "#main .q3 .stat-inline .eyebrow")
    want("the figures the answer states", figures, ["REACHED", "FILES", "INFERRED"])
    headline = text_of(d, "#main .card.pad .big14", "the sentence that says what they mean")
    if not re.search(r"reach(es)?\b.* within \d+ hops?", headline):
        fail("the answer's headline does not say what the figures mean: %r" % headline)


@finding("9.10", "the Agents page's columns line up and collapse together")
def _(d):
    a_store(d)
    for width in (1280, 820):
        d.set_viewport(width, 900)
        d.open_view("agents", fresh=True)
        cols = d.eval("(() => { const s = document.querySelector('#main .split'); if (!s) return null;"
                      " return [...s.children].map(c => { const r = c.getBoundingClientRect();"
                      " return {top: Math.round(r.top), left: Math.round(r.left), right: Math.round(r.right)}; }); })()")
        if not cols or len(cols) != 2:
            fail("at %dpx the Agents page has no two-column split: %r" % (width, cols))
        side_by_side = cols[0]["right"] <= cols[1]["left"] + 1
        if side_by_side and cols[0]["top"] != cols[1]["top"]:
            fail("at %dpx the Agents columns start at %d and %d" % (width, cols[0]["top"], cols[1]["top"]))
        if not side_by_side and cols[0]["left"] != cols[1]["left"]:
            fail("at %dpx the Agents columns neither sit side by side nor stack: %r" % (width, cols))
    d.reset_viewport()


# ---------------------------------------------------------------- 0.30.0 (10.x)

def live_aws():
    """An AWS access key id built now: live-looking, and in no source file."""
    alphabet = string.ascii_uppercase + string.digits
    return "AKIA" + "".join(random.choice(alphabet) for _ in range(16))


def live_github():
    alphabet = string.ascii_letters + string.digits
    return "ghp_" + "".join(random.choice(alphabet) for _ in range(36))


def review_tree(d, name):
    """Two files the scan refuses and a person may review, one `.env` it
    refuses and never offers, and one ordinary file."""
    root = os.path.join(d.fixtures.root, "%s-%d" % (name, random.randint(0, 10**9)))
    os.makedirs(root)
    with open(os.path.join(root, "alpha.txt"), "w") as f:
        f.write("# settings\nkey = \"%s\"\n" % live_aws())
    with open(os.path.join(root, "beta.txt"), "w") as f:
        f.write("# settings\nkey = \"%s\"\n" % live_aws())
    with open(os.path.join(root, ".env"), "w") as f:
        f.write("TOKEN=%s\n" % live_github())
    with open(os.path.join(root, "lib.rs"), "w") as f:
        f.write("pub fn callee() -> u32 { 1 }\n\npub fn caller() -> u32 {\n    callee() + 1\n}\n")
    return root


def clean_tree(d, name):
    """Two ordinary files and nothing the scan would hold back."""
    root = os.path.join(d.fixtures.root, "%s-%d" % (name, random.randint(0, 10**9)))
    os.makedirs(root)
    with open(os.path.join(root, "lib.rs"), "w") as f:
        f.write("pub fn callee() -> u32 { 1 }\n\npub fn caller() -> u32 {\n    callee() + 1\n}\n")
    with open(os.path.join(root, "notes.md"), "w") as f:
        f.write("# Notes\n\nNothing secret in here.\n")
    return root


def held_run(d, root):
    """The run the daemon holds for `root` once its scan is over."""
    wanted = normalise(os.path.realpath(root))
    deadline = time.time() + RUN_APPEARS
    while time.time() < deadline:
        for run in d.api("/api/index/runs")["runs"]:
            if wanted not in [normalise(p) for p in run.get("paths") or []]:
                continue
            if run.get("status") == "review":
                return run
            if run.get("status") in TERMINAL:
                fail("the scan of %s went to %s without holding for Start indexing" % (root, run.get("status")))
        time.sleep(0.3)
    fail("the scan of %s never held for Start indexing within %ds" % (root, RUN_APPEARS))


def release_held(d, run):
    """Drop a scan a failed check left held."""
    now = run and run_by_id(d, run["id"])
    if now and now.get("status") == "review":
        for action in ("stop", "remove"):
            d.api_result("/api/index/control", method="POST",
                         body={"store": run["store"], "action": action, "run": run["id"]})


def wizard_scan(d, store, root):
    """Scan `root` into `store` the way a person does from v6: the store's Add
    sources, the path pasted, Scan, and the review step once the scan is held.
    Every scan from the wizard holds (`review: "always"`), so nothing is
    embedded before its plan has been on screen. Returns the held run."""
    open_wizard_for(d, store)
    wizard_mode(d, "Paste a path")
    d.type(".wz-body .box input", root)
    d.press("Enter")
    d.wait_for("document.querySelectorAll('.wz-body .src-row').length > 0", what="the pasted folder as a source")
    press_text(d, ".wz-foot button", "Scan 1 source", "Scan 1 source")
    try:
        d.wait_for("!!document.querySelector('.wz-body .error-box') || /STEP 3 OF/.test((document.querySelector('.wz-head') || {}).innerText || '')"
                   " && !!document.querySelector('.wz-body .q4, .wz-body .stage-box')", timeout=30,
                   what="the scan to start")
    except cdp.ProtocolError:
        pass
    if exists(d, ".wz-body .error-box"):
        fail("the wizard's scan of %s into %s was refused: %s"
             % (root, store, text_of(d, ".wz-body .error-box", "the scan's error")))
    run = held_run(d, root)
    d.wait_for("/STEP 3 OF/.test((document.querySelector('.wz-head') || {}).innerText || '')"
               " && !!document.querySelector('.wz-body .q4')", timeout=30,
               what="the wizard's Review step, with the scan's figures")
    return run


def kpi_value(d, label):
    return d.eval(
        "(() => { const k = [...document.querySelectorAll('.wz-body .kpi, #main .kpi')].find(k =>"
        " ((k.querySelector('.eyebrow') || {}).textContent || '').trim().toLowerCase() === %s.toLowerCase());"
        " return k ? ((k.querySelector('.v') || {}).textContent || '').trim() : null; })()" % json.dumps(label)
    )


def decision_rows(d):
    """The review step's decision rows, as {file, text, buttons}."""
    return d.eval(
        "[...document.querySelectorAll('.wz-body .dec-list .dec-grid')].map(r => ({"
        " file: ((r.querySelector('.p') || {}).textContent || '').trim(), text: r.innerText,"
        " buttons: [...r.querySelectorAll('.acts button')].map(b => b.textContent.trim())}))"
    )


def decide_row(d, name, label):
    pressed = d.eval(
        "(() => { const r = [...document.querySelectorAll('.wz-body .dec-list .dec-grid')].find(r =>"
        " ((r.querySelector('.p') || {}).textContent || '').trim() === %s); if (!r) return 'no row reads ' + %s;"
        " const b = [...r.querySelectorAll('.acts button')].find(b => b.textContent.trim() === %s);"
        " if (!b) return 'the row offers no ' + %s; b.click(); return 'pressed'; })()"
        % (json.dumps(name), json.dumps(name), json.dumps(label), json.dumps(label))
    )
    if pressed != "pressed":
        fail("deciding %s as %r: %s" % (name, label, pressed))


@finding("10.1", "a scan lists reviewable files for a decision, the .env only as a no-action count, and indexes an accepted file redacted")
def _(d):
    host = indexed_fixture(d, clean_tree(d, "reviewhost"))
    root = review_tree(d, "review")
    run = wizard_scan(d, host, root)
    run_id, store = run["id"], run["store"]
    try:
        plan = run.get("plan") or {}
        want("files needing review in the plan", len(plan.get("review") or []), 2)
        if not any(p.endswith(".env") for p in plan.get("credential") or []):
            fail("the .env is not listed as a credential file: %s" % json.dumps(plan)[:300])

        d.wait_for("document.querySelectorAll('.wz-body .dec-list .dec-grid').length > 0", timeout=20,
                   what="the review step's decision rows")
        # Titled for the scan: what it is and that nothing is embedded yet.
        want("the review step's heading", text_of(d, ".wz-head .h", "the review step's heading"),
             "Review before anything is indexed")
        if not re.search(r"needs your decision", d.eval("document.querySelector('.wz-body').innerText"), re.I):
            fail("the review step has no 'Needs your decision' card")
        rows = decision_rows(d)
        want("the files the review step asks about", sorted(r["file"] for r in rows), ["alpha.txt", "beta.txt"])
        for r in rows:
            # 0.35.0 owner decision: three decisions per file — Keep out, Redact
            # & index, Index — taken on the row, one file or a selection at a
            # time, each logged per file. Where 0.30.0 read "Accept redacted",
            # "Accept as-is" and "Keep refused" through a dialog.
            want("the choices on %s" % r["file"], r["buttons"], ["Keep out", "Redact & index", "Index"])
            if re.search(r"AKIA[A-Z0-9]{16}", r["text"]):
                fail("the row for %s shows the matched value unmasked" % r["file"])
            if not re.search(r"\d+%", r["text"]):
                fail("the row for %s does not say how risky indexing it would be: %r" % (r["file"], r["text"][:200]))
        # The .env is never offered: no row, only the no-action count.
        if any(".env" in r["file"] for r in rows):
            fail("the .env has a decision row; a credential file is never offered")
        cred = kpi_value(d, "Credential files")
        if not cred or not cred.isdigit() or int(cred) < 1:
            fail("the review step counts %r credential files; the .env is one" % cred)
        opened = d.eval(
            "(() => { const g = [...document.querySelectorAll('.wz-body .group-row')].find(b => /Credential files/.test(b.innerText));"
            " if (!g) return false; g.click(); return true; })()"
        )
        if not opened:
            fail("the Left out automatically card has no Credential files group")
        d.wait_for("/\\.env/.test((document.querySelector('.wz-body .group-paths') || {}).innerText || '')",
                   what="the credential group to name the .env")
        if d.eval("(document.querySelector('.wz-body .group-paths') || document.body).querySelectorAll('button').length"):
            fail("the no-action list offers a button")

        decide_row(d, "alpha.txt", "Redact & index")
        d.wait_for("[...document.querySelectorAll('.wz-body .dec-list .dec-grid')].some(r => /alpha\\.txt/.test(r.innerText)"
                   " && /Redacted · indexed/.test(r.innerText))", what="alpha.txt to read Redacted · indexed")
        decide_row(d, "beta.txt", "Keep out")
        d.wait_for("[...document.querySelectorAll('.wz-body .dec-list .dec-grid')].some(r => /beta\\.txt/.test(r.innerText)"
                   " && /Kept out/.test(r.innerText))", what="beta.txt to read Kept out")

        press_text(d, ".wz-foot button", "Continue to index", "Continue to index")
        d.wait_for("/STEP 4 OF/.test((document.querySelector('.wz-head') || {}).innerText || '')", what="the Index step")
        press_text(d, ".wz-foot button", "Start indexing", "Start indexing")
        wait_for_run(d, store, run_id=run_id)
    finally:
        release_held(d, run)
    refused = d.api("/api/refused")
    rows = [r for s in refused["stores"] if s["store"] == store for r in s["rows"]]
    names = {os.path.basename(r["path"]): r for r in rows}
    beta = names.get("beta.txt")
    if not beta or beta.get("accepted") in ("redacted", "as-is", True):
        fail("the kept-out file is not still out of the index: %s" % json.dumps(beta))
    if names.get("alpha.txt", {}).get("accepted") != "redacted":
        fail("the accepted file is not listed as accepted (redacted): %s" % json.dumps(names.get("alpha.txt")))
    read = d.api("/api/read?target=%s" % urllib.parse.quote(os.path.join(os.path.realpath(root), "alpha.txt") + ":1-2"))
    if "REDACTED:aws" not in json.dumps(read):
        fail("the accepted file is not indexed redacted in the same run: %s" % json.dumps(read)[:300])


@finding("10.2", "a scan of a clean folder shows the plan and waits for Start indexing")
def _(d):
    host = indexed_fixture(d, clean_tree(d, "cleanhost"))
    root = clean_tree(d, "scanclean")
    run = wizard_scan(d, host, root)
    run_id, store = run["id"], run["store"]
    try:
        plan = run.get("plan") or {}
        want("files to review in a clean plan", len(plan.get("review") or []), 0)
        if not plan.get("embed"):
            fail("the held plan has nothing to embed: %s" % json.dumps(plan)[:300])
        if plan.get("seconds", 99) > 5:
            fail("the scan took %.2f s over two files" % plan["seconds"])
        head = text_of(d, ".wz-head .h", "the review step's heading")
        want("the review step's heading for a clean folder", head, "Everything is decided")
        want("the decision figure for a clean folder", kpi_value(d, "Your decision"), "done")
        if kpi_value(d, "Will be indexed") in (None, "0"):
            fail("the review step says nothing will be indexed from a clean folder")
        # Held means held: nothing starts on its own.
        time.sleep(2)
        want("the run's status two seconds after a clean scan", (run_by_id(d, run_id) or {}).get("status"), "review")
        press_text(d, ".wz-foot button", "Continue to index", "Continue to index")
        press_text(d, ".wz-foot button", "Start indexing", "Start indexing")
        final = wait_for_run(d, store, run_id=run_id)
    finally:
        release_held(d, run)
    want("the run started by Start indexing", final.get("status"), "done")


@finding("10.3", "Blast radius's Where column shows the call site, not the definition")
def _(d):
    root = review_tree(d, "impact")
    store = indexed_fixture(d, root)
    answer = d.api("/api/impact?name=callee&store=%s" % urllib.parse.quote(store))
    rows = (answer.get("impact") or {}).get("reached") or []
    if not rows or not rows[0].get("at"):
        fail("impact carries no call-site line: %s" % json.dumps(answer)[:300])
    reach(d, "callee", store)
    d.wait_for("document.querySelectorAll('#main .split table tbody tr').length > 0", timeout=30, what="a reached row")
    where = d.eval("[...document.querySelectorAll('#main .split table tbody tr')].map(tr => tr.cells[1].innerText.trim())")
    if not any(re.search(r"lib\.rs:%d$" % rows[0]["at"], w) for w in where):
        fail("the Where column reads %r, not the call at line %d" % (where, rows[0]["at"]))


@finding("10.4", "the graph's symbol box takes several names and shows each one's definition")
def _(d):
    root = review_tree(d, "graphnames")
    store = indexed_fixture(d, root)
    d.open_view("graph")
    pick_store(d, ".g-bar button[aria-haspopup]", store, "graph")
    pause(d, 300)
    d.type('.g-bar input[aria-label="Jump to a symbol"]', "callee, caller")
    d.press("Enter")
    d.wait_for("/\\d[\\d,]* of [\\d,]+ symbols/.test((document.querySelector('.g-foot') || {}).innerText || '')"
               " && !/reading/.test((document.querySelector('.g-sel') || {}).innerText || '')",
               timeout=20, what="the graph to answer for two names")
    nodes = texts_of(d, ".g-live .g-node")
    for name in ("callee", "caller"):
        if name not in nodes:
            fail("asking for 'callee, caller' drew no %s node: %r" % (name, nodes))
    side = d.eval("document.querySelector('.g-sel').innerText")
    if not re.search(r"\b2 definitions\b", side) and not ("callee" in side and "caller" in side):
        fail("the selection names neither both definitions nor how many there are: %r" % side[:300])


def explorer_tree(d, name):
    """A folder with a subfolder to open, an ordinary file beside it, and a
    binary the scan cannot index, so the tree has a greyed row to show."""
    root = os.path.join(d.fixtures.root, "%s-%d" % (name, random.randint(0, 10**9)))
    os.makedirs(os.path.join(root, "src"))
    with open(os.path.join(root, "src", "lib.rs"), "w") as f:
        f.write("pub fn callee() -> u32 { 1 }\n")
    with open(os.path.join(root, "README.md"), "w") as f:
        f.write("# Explorer\n\nA folder for the tree.\n")
    with open(os.path.join(root, "logo.png"), "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + bytes(range(256)) * 8)
    return root


TREE = """
(() => {
  const box = document.querySelector('#main .card[data-scroll-keep="tree"]');
  if (!box) return null;
  const top = box.querySelector(':scope > div > .tree-row');
  if (!top) return null;
  const kids = top.nextElementSibling;
  const rows = group => group ? [...group.children].map(c => {
    const r = c.matches('.tree-row') ? c : c.querySelector(':scope > .tree-row');
    return r ? {name: ((r.querySelector('.grow') || {}).textContent || '').trim(), cls: r.className,
                dir: r.tagName === 'BUTTON', expanded: r.getAttribute('aria-expanded'),
                meta: ((r.querySelector('.m') || {}).textContent || '').trim(),
                open: r.tagName === 'BUTTON' && !!r.nextElementSibling && !r.nextElementSibling.hidden
                      && !!r.nextElementSibling.querySelector('.tree-row')} : null; }).filter(Boolean) : [];
  const src = kids && [...kids.children].find(c => ((c.querySelector(':scope > .tree-row .grow') || {}).textContent || '') === 'src');
  return {expanded: top.getAttribute('aria-expanded'), rows: rows(kids),
          src: src ? rows(src.querySelector(':scope > .tree-row').nextElementSibling) : null};
})()
"""


@finding("10.5", "a store's Tree view opens folders on click and greys a file it did not index")
def _(d):
    root = explorer_tree(d, "explorer")
    store = indexed_fixture(d, root)
    files_tab(d, store)
    press_text(d, "#main .seg button", "Tree", "the Files tab's Tree view")
    d.wait_for("(() => { const t = %s; return !!t && t.rows.length > 0; })()" % TREE, timeout=30,
               what="the tree to draw the store's root with its children")
    tree = d.eval(TREE)
    want("the root folder's aria-expanded", tree["expanded"], "true")
    by_name = {r["name"]: r for r in tree["rows"]}
    src = by_name.get("src")
    if not src or not src["dir"]:
        fail("the root does not list the src folder as a folder: %r" % tree["rows"])
    want("the unopened src folder's aria-expanded", src["expanded"], "false")
    if src["open"]:
        fail("the src folder's children are drawn before it was opened")
    readme = by_name.get("README.md")
    if not readme or "muted" in readme["cls"].split():
        fail("README.md is not an ordinary indexed row: %r" % readme)
    logo = by_name.get("logo.png")
    if not logo or "muted" not in logo["cls"].split() or not logo["meta"].startswith("not indexed"):
        fail("the binary is not a greyed not-indexed row: %r" % tree["rows"])
    names = [r["name"] for r in tree["rows"]]
    dirs = [r["name"] for r in tree["rows"] if r["dir"]]
    if names[:len(dirs)] != dirs:
        fail("the tree does not order folders before files: %r" % names)
    # The owner's third walk: chips on an explorer were no use, so a tree offers
    # none it does not apply.
    chips = d.eval("[...document.querySelectorAll('#main .row .chip')].filter(c => c.offsetParent !== null).map(c => c.textContent.trim())")
    want("the chips offered beside the tree", chips, [])

    d.eval(
        "(() => { const box = document.querySelector('#main .card[data-scroll-keep=\"tree\"]');"
        " const b = [...box.querySelectorAll('button.tree-row')].find(b => ((b.querySelector('.grow') || {}).textContent || '') === 'src');"
        " b.click(); })()"
    )
    d.wait_for("(() => { const t = %s; return !!t && !!t.src && t.src.some(r => r.name === 'lib.rs'); })()" % TREE,
               timeout=20, what="the src folder to open and list lib.rs")
    tree = d.eval(TREE)
    want("the opened src folder's aria-expanded", next(r["expanded"] for r in tree["rows"] if r["name"] == "src"), "true")


@finding("10.6", "a store's Decisions list each decision with its reason, Undo takes one back, and a selection is logged per file")
def _(d):
    root = review_tree(d, "decisions")
    store = indexed_fixture(d, root)
    rows = [r for s in d.api("/api/refused")["stores"] if s["store"] == store for r in s["rows"]]
    alpha = next((r["path"] for r in rows if os.path.basename(r["path"]) == "alpha.txt"), None)
    beta = next((r["path"] for r in rows if os.path.basename(r["path"]) == "beta.txt"), None)
    if not alpha or not beta:
        fail("alpha.txt and beta.txt are not on the store's refused list: %s" % json.dumps(rows)[:400])
    d.api("/api/refused/accept", method="POST", body={"store": store, "path": alpha, "mode": "redacted", "reviewed": True})

    d.open_view("store/%s/review" % store, fresh=True)
    table = ("[...document.querySelectorAll('#main .card')].find(c =>"
             " ((c.querySelector('.card-t') || {}).textContent || '').trim() === 'Decisions')")
    d.wait_for("!!(%s) && (%s).querySelectorAll('tbody tr').length > 0" % (table, table), timeout=30,
               what="the Decisions table for %s" % store)
    seen = d.eval(
        "(() => { const t = %s; return {heads: [...t.querySelectorAll('thead th')].map(th => (th.textContent || '').trim()),"
        " rows: [...t.querySelectorAll('tbody tr')].map(tr => ({cells: [...tr.cells].map(td => (td.innerText || '').trim()),"
        " buttons: [...tr.querySelectorAll('button:not([role=checkbox])')].map(b => b.textContent.trim())}))}; })()" % table
    )
    # 0.35.0 owner decision (v6 as drawn): the Decisions table is Path,
    # Outcome, Why and By — yours and the rules' together, undo on any of yours —
    # where 0.30.0 listed only the person's with a "Likely real" confidence.
    for label in ("Path", "Outcome", "Why", "By"):
        if label not in seen["heads"]:
            fail("the Decisions table has no %r column: %r" % (label, seen["heads"]))
    mine = [r for r in seen["rows"] if any(c.startswith("you") for c in r["cells"])]
    want("the files decided by you", [r["cells"][1] for r in mine], ["alpha.txt"])
    cells = mine[0]["cells"]
    if "redacted · indexed" not in cells:
        fail("the decided row does not say redacted · indexed: %r" % cells)
    want("the decided row's action", mine[0]["buttons"], ["Undo"])
    if any(r["buttons"] for r in seen["rows"] if r not in mine):
        fail("a decision made by the rules offers an undo")

    d.eval("[...(%s).querySelectorAll('tbody button')].find(b => b.textContent.trim() === 'Undo').click()" % table)
    d.wait_for("!(%s) || ![...(%s).querySelectorAll('tbody tr')].some(tr => /alpha\\.txt/.test(tr.innerText) && /you/.test(tr.innerText))"
               % (table, table), timeout=30, what="Undo to take alpha.txt's decision back")
    after = [r for s in d.api("/api/refused")["stores"] if s["store"] == store for r in s["rows"]]
    if any(os.path.basename(r["path"]) == "alpha.txt" and r.get("accepted") for r in after):
        fail("Undo left alpha.txt accepted: %s" % json.dumps(after)[:400])

    # 0.35.0 owner decision: decisions take a list — bulk is allowed — and each
    # file is still logged as its own decision, by the person. Where 0.30.0
    # refused a list with a 400.
    status, answer = d.api_result("/api/refused/decide", method="POST",
                                  body={"store": store, "files": [alpha, beta], "decision": "out"})
    want("a list sent to the decide route", status, 200)
    logged = d.api("/api/refused?store=%s&decisions=1" % urllib.parse.quote(store))
    decisions = logged.get("decisions") or [r for s in logged.get("stores") or [] for r in s.get("decisions") or s.get("rows") or []]
    by_you = {os.path.basename(r["path"]) for r in decisions if r.get("by") == "you" and r.get("outcome") == "kept out"}
    if not {"alpha.txt", "beta.txt"} <= by_you:
        fail("a two-file decision was not logged as two decisions by you: %s" % json.dumps(decisions)[:400])
    d.api_result("/api/refused/decide", method="POST", body={"store": store, "files": [alpha, beta], "decision": "reset"})


@finding("10.7", "a finished run states what it did, and the store says what it did not index and what needs review")
def _(d):
    # 0.35.0 owner decision (v6 as drawn): a finished run's History row states
    # what it indexed, in how many chunks and how long it took; what the run
    # did not index and what waits for a decision are the store's Review tab —
    # the Waiting card and the rules' rows in Decisions — rather than a plan
    # line on the card.
    root = review_tree(d, "cardplan")
    store = indexed_fixture(d, root)
    d.open_view("store/%s/runs" % store, fresh=True)
    line = newest_history_row(d, run_for(d, store)) or d.eval("(document.querySelector(%s) || {}).innerText || ''" % json.dumps(HIST_ROW))
    if not re.search(r"[\d,]+ indexed · [\d,]+ chunks · took ", line):
        fail("the finished run's row reads %r" % line)
    if d.eval("document.querySelectorAll(%s + ' a').length" % json.dumps(HIST_ROW)):
        fail("the run's row carries a link")
    d.open_view("store/%s/review" % store)
    waiting = d.eval("document.querySelectorAll('#main .card.accent-edge .dec-grid:not(.head)').length")
    want("files the store says wait for a decision", waiting, 2)
    never = d.eval("[...document.querySelectorAll('#main table tbody tr')].filter(tr => /\\.env/.test(tr.innerText) && /never indexed/.test(tr.innerText)).length")
    if not never:
        fail("the store's Decisions do not list the .env as never indexed")


@finding("10.8", "the sidebar's Stores item counts the files waiting for a decision")
def _(d):
    # 0.35.0 owner decision (v6 as drawn): the Stores item carries an amber
    # count of files waiting for a decision, where 0.30.0's fourth walk had
    # taken it off. Asserted: the count is the daemon's, and it is not shown
    # when nothing waits.
    root = review_tree(d, "badge")
    indexed_fixture(d, root)
    waiting = sum(s.get("review") or 0 for s in d.api("/api/refused")["stores"])
    if not waiting:
        fail("/api/refused counts nothing waiting for review after a run that refused two files")
    d.open_view("home", fresh=True)
    d.wait_for("!!document.querySelector('.nav [data-badge=\"stores\"]:not([hidden])')", what="the Stores item's count")
    badge = text_of(d, '.nav [data-badge="stores"]', "the Stores item's count")
    want("the Stores item's count", badge, str(waiting))


@finding("10.9", "the Agents page names the installed hook mode")
def _(d):
    a_store(d)
    d.open_view("agents/health", fresh=True)
    body = view_text(d)
    clients = d.api("/api/agents").get("doctor") or []
    modes = [c for c in clients if c.get("hook_mode") and c.get("hook") and c.get("hook") != "paste"]
    for c in modes:
        if not re.search(r"hook \(%s\)" % re.escape(c["hook_mode"]), body):
            fail("%s has hook mode %s installed and Agents › Health does not name it" % (c["name"], c["hook_mode"]))


@finding("10.10", "discarding a held scan leaves no empty store behind for a folder that had none")
def _(d):
    """A scan of a new folder registers its store so the plan has somewhere to
    live. Discarding that scan — Stop… on its held card, "Also delete the store"
    ticked as it is for a store that held nothing — has to take the store with
    it, or every scan a person thinks better of leaves an empty store behind."""
    root = clean_tree(d, "discard")
    run_id, store = start_index(d, root, review="always")
    run = held_run(d, root)
    try:
        d.open_view("store/%s/runs" % store, fresh=True)
        d.wait_for(card_offers(CARD, "Stop…"), what="the held scan's card, offering Stop")
        if "Held for review" not in d.eval("(%s).innerText" % CARD):
            fail("the held scan's card does not say it is held for review")
        press_in(d, CARD, "Stop…")
        d.wait_for("!!document.querySelector(%s)" % json.dumps(MODAL), what="the discard confirm")
        if d.eval("(document.querySelector('#stop-delete') || {}).checked") is not True:
            fail("discarding the scan of a new folder does not offer, ticked, to delete its empty store")
        d.modal_press("Stop and undo")
        deadline = time.time() + 20
        while time.time() < deadline:
            if store not in {s["name"] for s in d.api("/api/stores")["stores"]}:
                break
            time.sleep(0.5)
        else:
            fail("%s is still registered 20 s after its only scan was discarded "
                 "(files_before on the held run: %r)" % (store, run.get("files_before")))
        now = run_by_id(d, run["id"])
        if now and now.get("status") not in TERMINAL:
            fail("the discarded run is still %s" % now.get("status"))
    finally:
        release_held(d, run)


@finding("10.11", "a store's size and what Compact gives back are on the Stores list, and Compact gives it back")
def _(d):
    """0.31.0: the Stores row states what the store takes on disk and what a
    compaction would reclaim; Compact gives it back. From 0.35.0 the two
    settings are Settings › Performance's Compact past and Keep retired
    definitions."""
    root = clean_tree(d, "compact")
    store = indexed_fixture(d, root)
    d.open_view("stores", fresh=True)
    headers = texts_of(d, "#main .gl-head.gl-stores > *")
    if "ON DISK" not in [h.upper() for h in headers]:
        fail("the Stores list has no On disk column: %r" % headers)
    row = next((s for s in d.api("/api/stores")["stores"] if s["name"] == store), None)
    if not row or not row.get("disk") or not row["disk"].get("total"):
        fail("/api/stores gives %s no disk figures: %r" % (store, row and row.get("disk")))
    stores_widest(d)
    shown = d.eval("(() => { const r = %s; return r ? ((r.querySelector('.c-disk .mono') || {}).textContent || '').trim() : null; })()"
                   % store_row_js(store))
    if not shown or shown == "—":
        fail("%s's row shows no size on disk: %r" % (store, shown))
    body = d.api("/api/store/compact", method="POST", body={"store": store, "wait": True})
    compact = body.get("compact") or {}
    if body.get("stopped") or not compact.get("after"):
        fail("a compaction of %s did not report its result: %r" % (store, body))
    after = next(s for s in d.api("/api/stores")["stores"] if s["name"] == store)["disk"]
    if after["reclaimable"] != 0:
        fail("%s still has %d reclaimable bytes straight after a compaction" % (store, after["reclaimable"]))
    d.open_view("settings")
    for label in ("Compact past", "Keep retired definitions"):
        if limit_value(d, label) is None:
            fail("Settings › Performance has no %r limit" % label)


# ---------------------------------------------------------------- 0.35.0 (v6.x)
#
# One or more checks per v6 view and flow. Each opens its view with the
# console cleared, compares what it shows with what the daemon says — never a
# sample value the design was drawn with — presses its primary control, and
# fails on any console error the view wrote.
#
# Some of what these assert rides on routes 0.35.0 adds alongside the page
# (empty named stores, per-store settings, the ledger recording switch, review
# decisions, runtime airgap, start at login, persisted run history, per-tool
# answer sizes). They are written against the release's API contract, so they
# pass once the daemon carries it.


def human_bytes(value):
    """`bytes()` in app.js, so a size can be compared as the page prints it."""
    size, unit, units = float(value or 0), 0, ["B", "KB", "MB", "GB", "TB"]
    while size >= 1024 and unit < len(units) - 1:
        size /= 1024
        unit += 1
    return ("%d %s" % (size, units[unit])) if unit == 0 else ("%.1f %s" % (size, units[unit]))


def open_clean(d, route, fresh=False):
    """Open a route with the console cleared before it."""
    d.clear_console()
    d.open_view(route, fresh=fresh)


def no_console_errors(d, what):
    pause(d, 300)
    errors = d.console_errors()
    if errors:
        fail("%s wrote %d console error(s):\n  %s" % (what, len(errors), "\n  ".join(errors[:5])))


def kpi(d, label):
    """A KPI tile's value and caption, by its label, anywhere on screen."""
    return d.eval(
        "(() => { const k = [...document.querySelectorAll('.kpi')].find(k =>"
        " ((k.querySelector('.eyebrow') || {}).textContent || '').trim().toLowerCase() === %s.toLowerCase());"
        " return k ? {v: ((k.querySelector('.v') || {}).textContent || '').trim(),"
        " s: ((k.querySelector('.s') || {}).textContent || '').trim()} : null; })()" % json.dumps(label)
    )


def wz_step(d, n):
    d.wait_for("/STEP %d OF/.test((document.querySelector('.wz-head') || {}).innerText || '')" % n,
               timeout=30, what="the wizard's step %d" % n)


def wizard_to(d, step, root=None):
    """Drive the first-run wizard from Welcome to `step` with a store of its own.

    1 Name, 2 Sources, 3 Review, 4 Index (before the run starts), 5 Connect.
    Returns the store's name.
    """
    name = "wz-%d" % random.randint(10000, 99999)
    d.open_view("welcome", fresh=True)
    press_text(d, ".welcome-card button", "Create your first store", "Create your first store")
    wz_step(d, 1)
    if step == 1:
        return name
    d.type('.wz-body input[aria-label="Store name"]', name)
    press_text(d, ".wz-foot button", "Create store", "Create store")
    wz_step(d, 2)
    if step == 2:
        return name
    root = root or clean_tree(d, name)
    wizard_mode(d, "Paste a path")
    d.type(".wz-body .box input", root)
    d.press("Enter")
    d.wait_for("document.querySelectorAll('.wz-body .src-row').length > 0", what="the pasted folder as a source")
    press_text(d, ".wz-foot button", "Scan 1 source", "Scan 1 source")
    wz_step(d, 3)
    d.wait_for("!!document.querySelector('.wz-body .error-box') || [...document.querySelectorAll('.wz-foot button')]"
               ".some(b => /Continue to index/.test(b.textContent) && !b.disabled)",
               timeout=60, what="the scan to finish")
    if exists(d, ".wz-body .error-box"):
        fail("the wizard's scan of %s into the new store %s was refused: %s"
             % (root, name, text_of(d, ".wz-body .error-box", "the scan's error")))
    if step == 3:
        return name
    press_text(d, ".wz-foot button", "Continue to index", "Continue to index")
    wz_step(d, 4)
    if step == 4:
        return name
    press_text(d, ".wz-foot button", "Start indexing", "Start indexing")
    d.wait_for("[...document.querySelectorAll('.wz-foot button')].some(b => /Next: connect agents/.test(b.textContent))",
               timeout=60, what="the run to start")
    press_text(d, ".wz-foot button", "Next: connect agents", "Next: connect agents")
    wz_step(d, 5)
    return name


def reach_connect_step(d, prefix):
    return wizard_to(d, 5)


@finding("v6.1", "Welcome shows this machine's checks from the daemon and opens the wizard")
def _(d):
    open_clean(d, "welcome", fresh=True)
    d.wait_for("!/checking…/.test((document.querySelector('.welcome-body .card-h') || {}).innerText || '')",
               timeout=30, what="the machine checks to be read")
    rows = d.eval(
        "Object.fromEntries([...document.querySelectorAll('.check-row')].map(r => ["
        " ((r.querySelector('.k') || {}).textContent || '').trim(),"
        " {v: ((r.querySelector('.v') || {}).textContent || '').trim(), d: ((r.querySelector('.d') || {}).textContent || '').trim()}]))"
    )
    about = d.api("/api/about")
    if about["bind"] not in rows.get("Daemon", {}).get("d", ""):
        fail("the Daemon check reads %r, not the daemon's %s" % (rows.get("Daemon"), about["bind"]))
    lanes = [l for l in d.api("/api/accel")["lanes"] if l.get("enabled")]
    best = next((l for l in lanes if l["lane"] != "cpu"), lanes[0] if lanes else None)
    if best and rows.get("Accelerator", {}).get("v") != (best.get("label") or best["lane"]):
        fail("the Accelerator check names %r; the daemon's first lane on is %s"
             % (rows.get("Accelerator"), best.get("label") or best["lane"]))
    machine = d.api("/api/index/runs")["limits"]["machine"]
    want("the Memory check", rows.get("Memory", {}).get("v"), "%d GiB" % round(machine["total_memory_mb"] / 1024))
    model = (d.api("/api/privacy").get("downloads") or [{}])[0]
    want("the Embedding model check", rows.get("Embedding model", {}).get("v"), human_bytes(model.get("bytes")))
    found = [c for c in d.api("/api/agents").get("doctor") or [] if c.get("present") or any(f.get("exists") for f in c.get("files") or [])]
    want("the Agent clients check", rows.get("Agent clients", {}).get("v"), "%d found" % len(found))
    press_text(d, ".welcome-card button", "Create your first store")
    wz_step(d, 1)
    no_console_errors(d, "Welcome and the wizard it opens")
    press_text(d, "header.top button", "Back to welcome", "Back to welcome")


@finding("v6.2", "the wizard's Name step checks the name as it is typed and creates an empty store")
def _(d):
    taken = a_store(d)
    d.clear_console()
    wizard_to(d, 1)
    box = '.wz-body input[aria-label="Store name"]'

    def state():
        return d.eval(
            "({hint: (document.querySelector('.wz-body .hint-l') || {}).textContent || '',"
            " bad: (document.querySelector('.wz-body .hint-l') || {className: ''}).className.includes('bad'),"
            " create: [...document.querySelectorAll('.wz-foot button')].find(b => /Create store/.test(b.textContent))})"
        )

    d.type(box, "Bad Name!")
    seen = state()
    if not seen["bad"] or "lowercase letters, digits and dashes" not in seen["hint"]:
        fail("a name with capitals and a '!' is not refused as it is typed: %r" % seen["hint"])
    if not d.eval("[...document.querySelectorAll('.wz-foot button')].find(b => /Create store/.test(b.textContent)).disabled"):
        fail("Create store is on offer for an invalid name")
    d.type(box, taken)
    if "already a store" not in state()["hint"]:
        fail("the name of an existing store, %s, is not refused: %r" % (taken, state()["hint"]))
    name = "wz-name-%d" % random.randint(1000, 99999)
    d.type(box, name)
    if ("~/.semlith/stores/%s" % name) not in state()["hint"]:
        fail("a free name does not say where the store will be kept: %r" % state()["hint"])
    press_text(d, ".wz-foot button", "Create store")
    wz_step(d, 2)
    row = next((s for s in d.api("/api/stores")["stores"] if s["name"] == name), None)
    if row is None or row.get("files"):
        fail("Create store did not make an empty store called %s: %r" % (name, row))
    no_console_errors(d, "the wizard's Name step")
    press_text(d, "header.top button", "Exit setup", "leaving the wizard")
    if d.modal_open():
        d.modal_press("Delete store")


@finding("v6.3", "the wizard's Sources step browses, pastes and takes a URL, and counts what it will scan")
def _(d):
    d.clear_console()
    wizard_to(d, 2)
    wizard_mode(d, "Browse folders")
    home = d.api("/api/dirs")
    want("where the folder picker opens", browse_where(d, ".wz-body"), "~")
    shown = d.eval("document.querySelectorAll('.wz-body .bitem').length")
    # The store home is left out of the picker: indexing semlith's own stores
    # is refused, so offering it would be a dead end.
    store_home = d.api("/api/about").get("store_home")
    offered = [e for e in home.get("entries") or [] if e.get("path") != store_home]
    want("the entries the picker lists", shown, len(offered))
    root = clean_tree(d, "sources")
    wizard_mode(d, "Paste a path")
    d.type(".wz-body .box input", root)
    d.press("Enter")
    d.wait_for("document.querySelectorAll('.wz-body .src-row').length === 1", what="the pasted folder as a source")
    want("the source's path", text_of(d, ".wz-body .src-row .p", "the source row"), tilde(d, root))
    want("the scan button", text_of(d, ".wz-foot button.primary", "the footer's main button"), "Scan 1 source")
    wizard_mode(d, "Add a URL")
    d.type(".wz-body .box input", "http://example.com/plain")
    fetch = ("[...document.querySelectorAll('.wz-body button')].find(b => b.textContent.trim() === 'Fetch')")
    if not d.eval("%s.disabled" % fetch):
        fail("Fetch is on offer for an http:// address; only https is fetched")
    d.type(".wz-body .box input", "https://example.com/page")
    if d.eval("%s.disabled" % fetch):
        fail("Fetch is not on offer for an https address")
    no_console_errors(d, "the wizard's Sources step")
    press_text(d, "header.top button", "Exit setup", "leaving the wizard")
    if d.modal_open():
        d.modal_press("Delete store")


@finding("v6.4", "the wizard's Review step shows the scan's own figures and takes decisions")
def _(d):
    d.clear_console()
    root = review_tree(d, "wzreview")
    wizard_to(d, 3, root=root)
    run = held_run(d, root)
    plan = run.get("plan") or {}
    left = kpi_value(d, "Your decision")
    want("the decisions left", left, "%d left" % len(plan.get("review") or []))
    want("the files that will be indexed", kpi_value(d, "Will be indexed"), grouped(plan.get("embed")))
    if int(kpi_value(d, "Credential files") or 0) < len(plan.get("credential") or []):
        fail("the review step counts fewer credential files than the scan found: %r vs %r"
             % (kpi_value(d, "Credential files"), plan.get("credential")))
    press_text(d, ".wz-body button", "Apply suggestions to 2 undecided", "applying the suggestions")
    want("the decisions left after applying the suggestions", kpi_value(d, "Your decision"), "done")
    no_console_errors(d, "the wizard's Review step")
    press_text(d, "header.top button", "Exit setup", "leaving the wizard")
    if d.modal_open():
        d.modal_press("Delete store")
    release_held(d, run)


@finding("v6.5", "the wizard's Index step runs on this machine's lanes and ends on a store that answers")
def _(d):
    d.clear_console()
    name = wizard_to(d, 4)
    lanes = [l for l in d.api("/api/accel")["lanes"] if (l.get("status") or {}).get("state") != "unavailable"]
    cards = d.eval("document.querySelectorAll('.wz-body .pick-card').length")
    want("the lanes the Index step offers", cards, len(lanes))
    press_text(d, ".wz-foot button", "Start indexing")
    d.wait_for("/is ready/.test((document.querySelector('.wz-body .done-banner') || {}).innerText || '')",
               timeout=RUN_FINISHES, what="the run to finish on the Index step")
    banner = text_of(d, ".wz-body .done-banner", "the done banner")
    files = store_named(d, name)["files"]
    if not re.search(r"\b%s files? indexed" % re.escape(grouped(files)), banner):
        fail("the done banner reads %r and %s holds %d files" % (banner, name, files))
    d.type('.wz-body input[aria-label="Try a search"]', "callee")
    press_text(d, ".wz-body button", "Search")
    d.wait_for("document.querySelectorAll('.wz-body .try-row').length > 0 || /Nothing matched/.test(document.querySelector('.wz-body').innerText)",
               timeout=30, what="the try-it search to answer")
    no_console_errors(d, "the wizard's Index step")


@finding("v6.6", "the wizard's Connect step lists this machine's clients and writes nothing until Register")
def _(d):
    d.clear_console()
    wizard_to(d, 5)
    agents = d.api("/api/agents")
    cards = d.eval("document.querySelectorAll('.wz-body .client-pick').length")
    want("the clients the Connect step lists", cards, len(agents.get("clients") or []))
    if "Nothing is written until you press Register" not in d.eval("document.querySelector('.wz-foot').innerText"):
        fail("the Connect step does not say nothing is written until Register")
    registered = [c["name"] for c in agents.get("doctor") or [] if c.get("registered")]
    after = [c["name"] for c in d.api("/api/agents").get("doctor") or [] if c.get("registered")]
    want("the clients registered by reaching the Connect step", after, registered)
    no_console_errors(d, "the wizard's Connect step")
    press_text(d, ".wz-foot button", "Skip for now")
    d.wait_for("location.hash === '#/home'", what="Skip for now to land on Home")


@finding("v6.7", "Home counts what the daemon holds and leads to it")
def _(d):
    a_store(d)
    open_clean(d, "home", fresh=True)
    live = [s for s in stores(d) if not s.get("missing") and not s.get("unopened")]
    want("the Stores tile", kpi(d, "Stores")["v"], str(len(live)))
    want("the Files indexed tile", kpi(d, "Files indexed")["v"], grouped(sum(s.get("files") or 0 for s in live)))
    rows = d.eval("document.querySelectorAll('#main .gl-row.gl-home-stores').length")
    want("the stores Home lists", rows, min(8, len(stores(d))))
    d.click("#main .kpi")
    d.wait_for("location.hash === '#/stores'", what="the Stores tile to open Stores")
    no_console_errors(d, "Home")


@finding("v6.8", "All stores lists every store, filters, and opens one")
def _(d):
    store = a_store(d)
    open_clean(d, "stores", fresh=True)
    all_stores = stores(d)
    foot = text_of(d, "#main .card-foot .grow", "the list's count")
    if "of %s" % ("%d store" % len(all_stores) + ("" if len(all_stores) == 1 else "s")) not in foot:
        fail("the list counts %r and the daemon holds %d stores" % (foot, len(all_stores)))
    d.type('#main input[aria-label="Filter stores"]', store)
    pause(d, 200)
    names = d.eval("[...document.querySelectorAll('#main .gl-row.gl-stores .cellname .a')].map(a => a.firstChild.textContent.trim())")
    if store not in names or any(store not in n for n in names):
        fail("filtering by %r lists %r" % (store, names))
    d.eval("(%s).click()" % store_row_js(store))
    d.wait_for("location.hash === %s" % json.dumps("#/store/%s" % store), what="a row to open its store")
    no_console_errors(d, "All stores")
    d.open_view("stores")
    d.type('#main input[aria-label="Filter stores"]', "")


@finding("v6.9", "Inside the index measures the stores themselves")
def _(d):
    a_store(d)
    open_clean(d, "stores/inside", fresh=True)
    corpus = [c for c in d.api("/api/corpus").get("stores") or [] if not c.get("error")]
    want("the Lines of code tile", kpi(d, "Lines of code")["v"], grouped(sum(c.get("lines") or 0 for c in corpus)))
    want("the Words indexed tile", kpi(d, "Words indexed")["v"], grouped(sum(c.get("words") or 0 for c in corpus)))
    no_console_errors(d, "Inside the index")


@finding("v6.10", "a store's Overview states its own figures and searches it")
def _(d):
    store = a_store(d)
    row = store_named(d, store)
    open_clean(d, "store/%s" % store, fresh=True)
    want("the Files tile", kpi(d, "Files")["v"], grouped(row["files"]))
    want("the Chunks tile", kpi(d, "Chunks")["v"], grouped(row["chunks"]))
    roots = d.eval("document.querySelectorAll('#main .root-row').length")
    want("the sources listed", roots, max(1, len(row.get("roots") or [])))
    press_text(d, "#main button", "Search it")
    d.wait_for("location.hash === '#/search'", what="Search it to open Search")
    # The hash moves before Search paints; wait for its scope button.
    d.wait_for("!!document.querySelector('#main .scope-btn .v')", what="Search to paint its scope")
    want("the store Search is scoped to", text_of(d, "#main .scope-btn .v", "the search scope"), store)
    no_console_errors(d, "a store's Overview")


@finding("v6.11", "a store's Files tab lists its files as the daemon counts them, and filters")
def _(d):
    store = a_store(d)
    open_clean(d, "store/%s/files" % store, fresh=True)
    d.wait_for("document.querySelectorAll('#main table tbody tr').length > 0", what="the files")
    total = d.api("/api/files?store=%s&limit=1" % urllib.parse.quote(store))["total"]
    if ("of %s" % grouped(total)) not in text_of(d, "#main .card-foot .grow", "the table's count"):
        fail("the Files tab counts %r and the daemon holds %d files" % (text_of(d, "#main .card-foot .grow", "count"), total))
    d.type('#main input[aria-label="Path or glob"]', "**/*.md")
    pause(d, 700)
    md = d.api("/api/files?store=%s&limit=1&path=%s" % (urllib.parse.quote(store), urllib.parse.quote("**/*.md")))["total"]
    if ("of %s" % grouped(md)) not in text_of(d, "#main .card-foot .grow", "the table's count"):
        fail("filtered to **/*.md the Files tab counts %r; the daemon has %d" % (text_of(d, "#main .card-foot .grow", "count"), md))
    no_console_errors(d, "a store's Files tab")


@finding("v6.12", "a store's Review tab lists what waits, and a decision takes a file out of the list")
def _(d):
    root = review_tree(d, "v6review")
    store = indexed_fixture(d, root)
    rows = [r for s in d.api("/api/refused")["stores"] if s["store"] == store for r in s["rows"]]
    waiting = [r for r in rows if r.get("reviewable") and not r.get("accepted") and not r.get("kept_out")]
    open_clean(d, "store/%s/review" % store, fresh=True)
    shown = d.eval("document.querySelectorAll('#main .card.accent-edge .dec-grid:not(.head)').length")
    want("the files waiting for a decision", shown, len(waiting))
    if not waiting:
        skip("nothing waits for a decision in the fixture store")
    d.eval("[...document.querySelectorAll('#main .card.accent-edge .dec-grid:not(.head)')][0]"
           ".querySelector('.acts button').click()")
    d.wait_for("document.querySelectorAll('#main .card.accent-edge .dec-grid:not(.head)').length === %d" % (len(waiting) - 1),
               timeout=20, what="Keep it out to take the file out of the waiting list")
    no_console_errors(d, "a store's Review tab")


@finding("v6.13", "a store's Runs tab lists its runs as the daemon holds them")
def _(d):
    store = indexed_fixture(d, d.fixtures.unique("v6runs"))
    runs = d.api("/api/index/runs")
    mine = {r["id"] for r in runs.get("runs") or [] if r["store"] == store and r["status"] in TERMINAL}
    mine |= {h["id"] for h in runs.get("history") or [] if h.get("store") == store}
    open_clean(d, "store/%s/runs" % store, fresh=True)
    d.wait_for("document.querySelectorAll(%s).length > 0" % json.dumps(HIST_ROW), what="History")
    want("the History rows", d.eval("document.querySelectorAll(%s).length" % json.dumps(HIST_ROW)), min(30, len(mine)))
    if not isinstance(runs.get("history"), list):
        fail("/api/index/runs carries no `history`, so a finished run does not outlive a restart")
    d.click(HIST_ROW)
    d.wait_for("document.querySelector(%s).getAttribute('aria-expanded') !== null" % json.dumps(HIST_ROW), what="a row to open")
    no_console_errors(d, "a store's Runs tab")


@finding("v6.14", "a store's Settings tab reads the store's settings and renames it")
def _(d):
    store = indexed_fixture(d, d.fixtures.unique("v6settings"))
    open_clean(d, "store/%s/settings" % store, fresh=True)
    want("the name field", d.eval("document.querySelector('#main input[aria-label=\"Store name\"]').value"), store)
    row = store_named(d, store)
    for title, key in (("Watch for changes", "watch"), ("Record retrievals", "record")):
        on = d.eval("[...document.querySelectorAll('#main .tg-plain')].find(b => b.innerText.startsWith(%s)).getAttribute('aria-checked')"
                    % json.dumps(title))
        want("the %s switch" % title, on, "false" if row.get(key) is False else "true")
    for key in ("kind", "lean", "watch", "record"):
        if key not in row:
            fail("/api/stores carries no %r for %s, so the tab cannot read it" % (key, store))
    renamed = store + "-r"
    d.type('#main input[aria-label="Store name"]', renamed)
    press_text(d, "#main button", "Rename")
    d.wait_for("location.hash === %s" % json.dumps("#/store/%s/settings" % renamed), timeout=20,
               what="the rename to land on the renamed store")
    if renamed not in {s["name"] for s in stores(d)}:
        fail("the page moved to %s and the daemon has no store by that name" % renamed)
    # A rename reopens the store and its watcher catches up; renaming back
    # mid-run is refused by design, so the clean-up waits for it.
    if run_for(d, renamed):
        wait_for_run(d, renamed)
    d.api("/api/store/settings", method="POST", body={"store": renamed, "rename": store})
    no_console_errors(d, "a store's Settings tab")


def search_mode(d, mode, query, lang=None):
    open_clean(d, "search", fresh=True)
    press_text(d, "#main .seg button", mode)
    if lang:
        pick(d, '#main .dd[aria-label="Pattern language"]', lang)
    d.type(SEARCH_BOX, query)
    d.press("Enter")
    d.wait_for("!document.querySelector('#main .spinner') && (document.querySelectorAll('#main .hit-group, #main .card .lines, #main .error-box').length > 0"
               " || /Nothing matched|No indexed line|matched nothing/.test(document.querySelector('#main').innerText))",
               timeout=60, what="the %s search to answer" % mode)
    if exists(d, "#main .error-box"):
        fail("the %s search answered an error: %s" % (mode, text_of(d, "#main .error-box", "the error")))


@finding("v6.15", "Search's Ranked mode shows the daemon's hits and reads the first")
def _(d):
    a_store(d)
    query = "release record sealed immutable"
    search_mode(d, "Ranked", query)
    answer = d.api("/api/search?query=%s&k=8&format=locate&max_tokens=1500" % urllib.parse.quote(query))
    files = {(h.get("store"), h["path"]) for h in answer.get("hits") or []}
    want("the files Ranked groups its hits by", d.eval("document.querySelectorAll('#main .hit-group').length"), len(files))
    d.wait_for("!!document.querySelector('#main .sr-detail .lines .l')", timeout=20, what="the first hit read into the detail panel")
    no_console_errors(d, "Search › Ranked")


@finding("v6.16", "Search's Brief mode is exactly what semlith_brief returns")
def _(d):
    a_store(d)
    query = "what keeps the index fresh"
    search_mode(d, "Brief", query)
    answer = d.api("/api/brief?question=%s&budget=1500" % urllib.parse.quote(query))
    tokens = (answer.get("brief") or {}).get("tokens") or 0
    strip = d.eval("document.querySelector('#main').innerText")
    if ("%s of 1,500" % grouped(tokens)) not in strip:
        fail("the Brief's strip does not state the brief's own %d tokens" % tokens)
    no_console_errors(d, "Search › Brief")


@finding("v6.17", "Search's Exact mode lists every matching line the daemon finds")
def _(d):
    a_store(d)
    search_mode(d, "Exact", "Widget000")
    answer = d.api("/api/search?query=Widget000&exact=1")
    lines = len(answer.get("matches") or [])
    if ("%d line" % lines) not in text_of(d, "#main .sr-foot", "the Exact foot"):
        fail("the Exact foot reads %r; the daemon matched %d lines" % (text_of(d, "#main .sr-foot", "foot"), lines))
    no_console_errors(d, "Search › Exact")


@finding("v6.18", "Search's Pattern mode runs a tree-sitter query over one language")
def _(d):
    a_store(d)
    query = "(function_item name: (identifier) @f)"
    search_mode(d, "Pattern", query, lang="rust")
    answer = d.api("/api/pattern?query=%s&lang=rust" % urllib.parse.quote(query))
    lines = len(answer.get("matches") or [])
    foot = text_of(d, "#main .sr-foot", "the Pattern foot")
    if ("%d line" % lines) not in foot or "tree-sitter · rust" not in foot:
        fail("the Pattern foot reads %r; the daemon matched %d" % (foot, lines))
    no_console_errors(d, "Search › Pattern")


@finding("v6.19", "Graph's Explore draws the store's graph and selects a node on click")
def _(d):
    a_store(d)
    open_clean(d, "graph", fresh=True)
    store = d.eval("((document.querySelector('.g-bar button[aria-haspopup] .mono') || {}).textContent || '').trim()")
    sel = d.eval("document.querySelector('.g-sel .nm').textContent.trim()")
    answer = d.api("/api/graph?store=%s&limit=63&name=%s" % (urllib.parse.quote(store), urllib.parse.quote(sel)))
    foot = text_of(d, ".g-foot", "the graph's foot")
    if ("of %s symbols" % grouped(answer.get("total"))) not in foot:
        fail("the foot reads %r; the daemon's graph holds %s symbols" % (foot, answer.get("total")))
    want("the nodes drawn", d.eval("document.querySelectorAll('.g-live .g-node').length"), len(answer.get("nodes") or []))
    other = d.eval("[...document.querySelectorAll('.g-live .g-node')].map(n => n.textContent).find(t => t !== %s)" % json.dumps(sel))
    if other:
        d.eval("[...document.querySelectorAll('.g-live .g-node')].find(n => n.textContent === %s).click()" % json.dumps(other))
        d.wait_for("document.querySelector('.g-sel .nm').textContent.trim() === %s" % json.dumps(other),
                   what="a click on %s to select it" % other)
    no_console_errors(d, "Graph › Explore")


@finding("v6.20", "Graph's Blast radius counts what the daemon reaches")
def _(d):
    root = review_tree(d, "v6blast")
    store = indexed_fixture(d, root)
    d.clear_console()
    reach(d, "callee", store)
    imp = d.api("/api/impact?name=callee&store=%s&depth=3" % urllib.parse.quote(store))["impact"]
    figures = texts_of(d, "#main .q3 .stat-inline .v")
    want("the REACHED, FILES and INFERRED figures", figures,
         [grouped(len(imp.get("reached") or []) + (imp.get("hidden") or 0)), grouped(len(imp.get("files") or [])),
          grouped((imp.get("inferred") or 0) + (imp.get("ambiguous") or 0))])
    no_console_errors(d, "Graph › Blast radius")


@finding("v6.21", "Graph's Path & evidence answers as the daemon traces")
def _(d):
    root = review_tree(d, "v6path")
    store = indexed_fixture(d, root)
    open_clean(d, "graph/path", fresh=True)
    pick_store(d, ".ctrl-card button[aria-haspopup]", store, "graph/path")
    d.type('.ctrl-card input[aria-label="From"]', "caller")
    d.type('.ctrl-card input[aria-label="To"]', "callee")
    press_text(d, ".ctrl-card button", "Find the path")
    d.wait_for("/ANSWER/.test(document.querySelector('#main').innerText)", timeout=30, what="the path's answer")
    answer = d.api("/api/trace?from=caller&to=callee&store=%s" % urllib.parse.quote(store))
    shown = text_of(d, "#main .card-b .big14", "the answer")
    want("the answer the page states", shown, (answer.get("trace") or {}).get("answer"))
    no_console_errors(d, "Graph › Path & evidence")


@finding("v6.22", "Agents › Connected shows the endpoint and what the tool list costs")
def _(d):
    a_store(d)
    open_clean(d, "agents", fresh=True)
    agents = d.api("/api/agents")
    want("the endpoint the page names", text_of(d, "#main .endpoint-box .u", "the endpoint"), (agents.get("endpoint") or {}).get("url"))
    tabs = dict((t.rsplit("\n", 1)[0], t.rsplit("\n", 1)[-1]) for t in texts_of(d, "#main .tabs .tab") if "\n" in t)
    if tabs.get("Tools") != str(len(agents.get("tools") or [])):
        fail("the Tools tab counts %r; the daemon lists %d tools" % (tabs.get("Tools"), len(agents.get("tools") or [])))
    conns = agents.get("connections") or []
    rows = d.eval("document.querySelectorAll('#main table tbody tr').length")
    want("the clients listed as connected", rows, len(conns))
    no_console_errors(d, "Agents › Connected")


@finding("v6.23", "Agents › Add a client lists every documented client and shows each one's config")
def _(d):
    a_store(d)
    open_clean(d, "agents/add", fresh=True)
    agents = d.api("/api/agents")
    want("the clients listed", d.eval("document.querySelectorAll('#main .client-row').length"), len(agents.get("clients") or []))
    name = (agents.get("clients") or [{}])[0].get("name")
    agents_add(d, name)
    press_text(d, "#main .tabs .tab", "Config file")
    code = text_of(d, "#main .code", "the client's config")
    if "semlith" not in code:
        fail("the config shown for %s does not mention semlith: %r" % (name, code[:200]))
    no_console_errors(d, "Agents › Add a client")


@finding("v6.24", "Agents › Tools lists the sixteen tools with a typical answer size each")
def _(d):
    a_store(d)
    open_clean(d, "agents/tools", fresh=True)
    tools = d.api("/api/agents").get("tools") or []
    rows = d.eval("[...document.querySelectorAll('#main table tbody tr')].map(tr => [...tr.cells].map(c => c.innerText.trim()))")
    # Listed tools first, then the ones agents are not offered, each of those
    # marked so (0.36.0).
    order = [t["name"] for t in tools if t.get("listed") is not False] + [t["name"] for t in tools if t.get("listed") is False]
    want("the tools listed", [r[0].split()[0] for r in rows], order)
    marked = [r[0].split()[0] for r in rows if "CLI and portal" in r[0]]
    want("the tools marked as not offered to agents", marked, [t["name"] for t in tools if t.get("listed") is False])
    if sum(1 for t in tools if t.get("listed") is not False) != 8:
        fail("the daemon offers %d tools to agents, expected 8" % sum(1 for t in tools if t.get("listed") is not False))
    if len(rows) != 16:
        fail("Agents › Tools lists %d tools, expected 16" % len(rows))
    sized = [r for r in rows if re.match(r"^~?[\d,]+ tok$", r[2])]
    if len(sized) != len(rows):
        fail("%d of %d tools carry no typical answer size: %r"
             % (len(rows) - len(sized), len(rows), [r[0] for r in rows if r not in sized][:4]))
    no_console_errors(d, "Agents › Tools")


@finding("v6.25", "Agents › Health lists every client's state and checks again")
def _(d):
    a_store(d)
    open_clean(d, "agents/health", fresh=True)
    doctor = d.api("/api/agents").get("doctor") or []
    want("the clients listed", d.eval("document.querySelectorAll('#main table tbody tr').length"), len(doctor))
    press_text(d, "#main button", "Check again")
    d.wait_for("[...document.querySelectorAll('.toast')].some(t => /Checked \\d+ clients?/.test(t.innerText))",
               timeout=20, what="Check again to answer")
    no_console_errors(d, "Agents › Health")


@finding("v6.26", "the Ledger's Sessions tab counts what the ledger holds")
def _(d):
    a_store(d)
    d.api("/api/search?query=release%20record&k=4")
    time.sleep(0.5)
    open_clean(d, "ledger", fresh=True)
    ledger = d.api("/api/ledger")
    want("the Queries recorded tile", kpi(d, "Queries recorded")["v"], grouped(ledger.get("queries")))
    sessions = ledger.get("sessions") or []
    want("the sessions on the first page", d.eval("document.querySelectorAll('#main table tbody tr').length"), min(10, len(sessions)))
    no_console_errors(d, "Ledger › Sessions")


@finding("v6.27", "the Ledger's Retrievals tab lists the rows and filters to zero hits")
def _(d):
    a_store(d)
    d.api("/api/search?query=zzqqxx%20nothing%20matches%20this&k=4")
    time.sleep(0.5)
    open_clean(d, "ledger/retrievals", fresh=True)
    rows = d.api("/api/ledger").get("rows") or []
    want("the retrievals on the first page", d.eval("document.querySelectorAll('#main table tbody tr').length"), min(10, len(rows)))
    press_text(d, "#main .filterbar button", "Zero-hit only")
    zero = [r for r in rows if r.get("hits") == 0]
    want("the zero-hit retrievals", d.eval("document.querySelectorAll('#main table tbody tr').length"), min(10, len(zero)))
    no_console_errors(d, "Ledger › Retrievals")


@finding("v6.28", "the Ledger's Session replay tab shows the daemon's replay state")
def _(d):
    a_store(d)
    open_clean(d, "ledger/replay", fresh=True)
    replay = d.api("/api/ledger/replay")
    body = view_text(d)
    if replay.get("enabled") and "turn off on the Privacy page" not in body:
        fail("replay is on and the tab does not show its on state")
    if not replay.get("enabled") and "Turn it on" not in body:
        fail("replay is off and the tab offers no way to turn it on")
    no_console_errors(d, "Ledger › Session replay")


@finding("v6.29", "the Ledger's recording switch pauses recording and resumes it on the same chain")
def _(d):
    a_store(d)
    open_clean(d, "ledger", fresh=True)
    switch = "#main .head button[role=switch]"
    if d.eval("document.querySelector(%s).getAttribute('aria-checked')" % json.dumps(switch)) != "true":
        fail("the recording switch is not on in a home that never paused it")
    try:
        d.click(switch)
        d.wait_for("document.querySelector(%s).getAttribute('aria-checked') === 'false'" % json.dumps(switch),
                   timeout=10, what="the switch to read paused")
        ledger = d.api("/api/ledger")
        rec = ledger.get("recording")
        if not isinstance(rec, dict) or rec.get("on") is not False or rec.get("reason") != "paused":
            fail("/api/ledger reports recording %r after the switch paused it" % rec)
        before = ledger["queries"]
        d.api("/api/search?query=paused%20recording%20probe&k=2")
        time.sleep(0.8)
        if d.api("/api/ledger")["queries"] != before:
            fail("a retrieval while recording is paused was recorded")
        if "ledger paused" not in text_of(d, ".nav .daemon .facts", "the daemon card"):
            fail("the sidebar does not say the ledger is paused")
    finally:
        d.api("/api/ledger/recording", method="POST", body={"on": True})
    d.api("/api/search?query=resumed%20recording%20probe&k=2")
    time.sleep(0.8)
    ledger = d.api("/api/ledger")
    if ledger.get("intact") is False:
        fail("resuming recording broke the ledger's hash chain")
    no_console_errors(d, "the Ledger's recording switch")


@finding("v6.30", "Reports previews the report the daemon generates")
def _(d):
    a_store(d)
    open_clean(d, "reports", fresh=True)
    d.wait_for("((document.querySelector('#main .preview') || {}).textContent || '').length > 40", what="the preview")
    model = d.eval("[...document.querySelectorAll('#main .seg button[aria-pressed=\"true\"]')].map(b => b.textContent).find(t => /claude|gpt|gemini/i.test(t)) || ''")
    answer = d.api("/api/report?kind=savings&format=markdown%s" % ("&model=" + urllib.parse.quote(model) if model else ""))
    shown = d.eval("document.querySelector('#main .preview').textContent")
    if (answer.get("text") or "").splitlines()[:1] != shown.splitlines()[:1]:
        fail("the preview opens %r and the daemon's savings report opens %r"
             % (shown.splitlines()[:1], (answer.get("text") or "").splitlines()[:1]))
    want("the schedules listed", d.eval("document.querySelectorAll('#main .sched-row').length"),
         len(d.api("/api/schedules").get("schedules") or {}))
    no_console_errors(d, "Reports")


@finding("v6.31", "Privacy states what has left the machine from the daemon's own count")
def _(d):
    a_store(d)
    open_clean(d, "privacy", fresh=True)
    privacy = d.api("/api/privacy")
    outbound, airgap = privacy.get("outbound"), privacy.get("airgap")
    if not isinstance(outbound, dict) or "count" not in outbound:
        fail("/api/privacy carries no outbound counter, so the verdict cannot be the daemon's: %r" % outbound)
    if not isinstance(airgap, dict) or "on" not in airgap:
        fail("/api/privacy carries no runtime airgap state: %r" % airgap)
    verdict = text_of(d, "#main .verdict .t", "the verdict")
    expected = "Nothing has left this machine" if not outbound["count"] else \
        "%d request%s left this machine since start" % (outbound["count"], "" if outbound["count"] == 1 else "s")
    want("the verdict", verdict, expected)
    want("the airgap switch", d.eval("document.querySelector('#main .verdict [role=switch]').getAttribute('aria-checked')"),
         "true" if airgap["on"] else "false")
    bind = d.eval("[...document.querySelectorAll('#main .fact-card')].find(c => /Bind address/i.test(c.innerText)).querySelector('.v').textContent")
    want("the bind address", bind, privacy.get("bind"))
    press_text(d, "#main button", "Check now")
    d.wait_for("!!document.querySelector('#main .notice.green, #main .notice.amber, #main .error-box')", timeout=60,
               what="the store check to answer")
    no_console_errors(d, "Privacy")


@finding("v6.32", "Settings › Performance shows the daemon's limits and lanes")
def _(d):
    open_clean(d, "settings", fresh=True)
    limits = d.api("/api/index/runs")["limits"]
    want("Runs at once", limit_value(d, "Runs at once"), grouped(limits["runs_at_once"]["value"]))
    want("Threads per run", limit_value(d, "Threads per run"), grouped(limits["embed_threads"]["value"]))
    want("Memory per store", limit_value(d, "Memory per store"), "%s MiB" % grouped(limits["index_memory_mb"]["value"]))
    want("the lanes listed", d.eval("document.querySelectorAll('#main .lane-row').length"), len(d.api("/api/accel")["lanes"]))
    no_console_errors(d, "Settings › Performance")


@finding("v6.33", "Settings › Agent access masks the key, reveals it on request, and reads the login service")
def _(d):
    open_clean(d, "settings/access", fresh=True)
    masked = text_of(d, "#main .copyfield .t", "the agent key field")
    if re.search(r"sml_[0-9a-f]{8,}", masked):
        fail("the agent key is shown unmasked before Reveal: %r" % masked)
    press_text(d, "#main button", "Reveal")
    d.wait_for("/sml_[0-9a-f]{8,}/.test(document.querySelector('#main .copyfield .t').textContent)", what="Reveal to show the key")
    press_text(d, "#main button", "Hide")
    preview = d.api("/api/privacy").get("token_preview")
    if preview and preview not in view_text(d):
        fail("the session token preview %r is not on the page" % preview)
    login = d.api("/api/about").get("login")
    if not isinstance(login, dict) or "installed" not in login:
        fail("/api/about carries no `login` state, so Start at login cannot be read: %r" % login)
    on = d.eval("[...document.querySelectorAll('#main .card')].find(c => /Start at login/.test(c.innerText)).querySelector('[role=switch]').getAttribute('aria-checked')")
    want("the Start at login switch", on, "true" if login.get("installed") else "false")
    no_console_errors(d, "Settings › Agent access")


@finding("v6.34", "Settings › Cloud renders and connects to nothing")
def _(d):
    open_clean(d, "settings/cloud", fresh=True)
    if "not connected" not in view_text(d):
        fail("Settings › Cloud does not say it is not connected")
    no_console_errors(d, "Settings › Cloud")


@finding("v6.35", "Settings › About states this binary's facts from the daemon")
def _(d):
    open_clean(d, "settings/about", fresh=True)
    about = d.api("/api/about")
    facts = d.eval("Object.fromEntries([...document.querySelectorAll('#main .kv')].map(r => [r.querySelector('.k').textContent.trim(), r.querySelector('.v').textContent.trim()]))")
    if not facts.get("VERSION", "").startswith(about["version"]):
        fail("VERSION reads %r; the daemon is %s" % (facts.get("VERSION"), about["version"]))
    want("BOUND TO", facts.get("BOUND TO"), about["bind"])
    langs = d.api("/api/languages").get("languages") or []
    if not text_of(d, "#main .card-h .card-t", "the languages card").startswith("%d language" % len(langs)):
        fail("the languages card does not count the daemon's %d languages" % len(langs))
    if not any(t == "Check for updates" for t in texts_of(d, "#main button")):
        fail("Settings › About offers no Check for updates")
    prices = d.api("/api/prices")
    if ("%s models priced" % grouped(prices.get("models"))) not in view_text(d):
        fail("the Prices card does not count the daemon's %s priced models" % prices.get("models"))
    no_console_errors(d, "Settings › About")


@finding("rc4.1", "a run's bar follows what is read and embedded, not 99 % from the start")
def _(d):
    run = wait_for_run(d, a_store(d))
    if run.get("progress") != 1.0:
        fail("a finished run reports progress %r, not 1.0" % run.get("progress"))
    open_clean(d, "stores", fresh=True)
    # The 0.37.0-rc.3 shape: an eighth of the files read, embedding keeping
    # up, so pending_share is 0.002 and the bar read 99 %.
    early = d.eval(
        "runPct({store: 'rc4', id: 1, started_at: 'a', status: 'running', progress: 0.125,"
        " pending_share: 0.002, bytes: 125, bytes_total: 1000, scanned: 561, total: 4349})"
    )
    if not 12 <= early <= 13:
        fail("an eighth read reads %r %%, not about 12 %%" % early)
    later = d.eval(
        "runPct({store: 'rc4', id: 1, started_at: 'a', status: 'running', progress: 0.1})"
    )
    if later < early:
        fail("the bar went backwards within a run: %r then %r" % (early, later))
    full = d.eval(
        "runPct({store: 'rc4', id: 2, started_at: 'b', status: 'running', progress: 0.999})"
    )
    if full >= 100:
        fail("a running run reads %r %%" % full)
    no_console_errors(d, "run progress")


# ------------------------------------------------- 0.37.0-rc.4 run truth (rc4.x)
#
# The owner's 2026-10-06 walk (W1-W7 in releases/0.37.0-rc.4/run-truth-spec.md):
# Start sat on "Starting…", the run card's stage pills were guesses, the scan
# card's stages were thresholds on a paced fraction, bulk decisions said
# nothing while they ran, and the Index step's estimate read "—". Each check
# drives the page the way the walk did. Where a figure comes from a field only
# a run-truth daemon sends, the check asserts it when the daemon sends it and
# says so when it does not.

#: Records every decide POST's body and can hold any request for a while, so a
#: check can see batches, `defer`, and a slow request's waiting line.
FETCH_SPY = r"""
(() => {
  if (window.__spy) return true;
  const spy = window.__spy = {decides: [], delay: {}};
  const real = window.fetch.bind(window);
  window.fetch = async (input, init) => {
    const url = String((input && input.url) || input);
    if (url.includes('/api/refused/decide') && init && init.body) spy.decides.push(JSON.parse(init.body));
    const hold = Object.entries(spy.delay).find(([part]) => url.includes(part));
    if (hold) await new Promise(r => setTimeout(r, hold[1]));
    return real(input, init);
  };
  return true;
})()
"""

#: Samples the run card every 50 ms: each status line it showed (and when),
#: whether it was the start sequence's, and the Pipeline panel's height.
PIPE_SAMPLER = r"""
(() => {
  const seen = window.__pipe = {lines: [], heights: [], lanes: []};
  const real = window.__realSetInterval || window.setInterval;
  seen.timer = real(() => {
    const card = document.querySelector('.wz-body .run-card');
    if (!card) return;
    const st = card.querySelector('.pipe-status');
    const text = st ? st.textContent.trim() : '';
    const last = seen.lines[seen.lines.length - 1];
    if (!last || last[0] !== text) seen.lines.push([text, Date.now(), !!(st && st.classList.contains('starting'))]);
    const pipe = card.querySelector('.pipe');
    if (pipe && !pipe.hidden && !st.classList.contains('starting') && card.querySelector('.pipe-row'))
      seen.heights.push(Math.round(pipe.getBoundingClientRect().height));
    const lanes = [...card.querySelectorAll('.pipe-row')].map(r => r.getAttribute('data-lane')).join(',');
    if (lanes && !seen.lanes.includes(lanes)) seen.lanes.push(lanes);
  }, 50);
  return true;
})()
"""


def secret_tree(d, name, count):
    """`count` files the scan refuses and a person may review, for a bulk
    decision that has to go in more than one batch."""
    root = os.path.join(d.fixtures.root, "%s-%d" % (name, random.randint(0, 10**9)))
    os.makedirs(root)
    for i in range(count):
        with open(os.path.join(root, "conf-%03d.txt" % i), "w") as f:
            f.write("# settings %d\nkey = \"%s\"\n" % (i, live_aws()))
    with open(os.path.join(root, "lib.rs"), "w") as f:
        f.write("pub fn callee() -> u32 { 1 }\n")
    return root


@finding("rc4.2", "Start lands on the run view at once and says each step on one line, the decisions recorded first")
def _(d):
    d.clear_console()
    d.eval("try { localStorage.removeItem('semlith-run-tab'); } catch (e) {}")
    root = review_tree(d, "rc4start")
    name = wizard_to(d, 3, root=root)
    held = held_run(d, root)
    press_text(d, ".wz-body button", "Apply suggestions to 2 undecided", "applying the suggestions")
    press_text(d, ".wz-foot button", "Continue to index", "Continue to index")
    wz_step(d, 4)
    d.eval(FETCH_SPY)
    # A slow request says what it waits for: the settings save is held 2.6 s.
    d.eval("window.__spy.delay['/api/store/settings'] = 2600; true")
    d.eval(PIPE_SAMPLER)
    clicked = d.eval("(() => { const b = [...document.querySelectorAll('.wz-foot button')].find(b => /Start indexing/.test(b.textContent));"
                     " if (!b) return null; window.__clicked = Date.now(); b.click(); return true; })()")
    if not clicked:
        fail("the Index step offers no Start indexing")
    try:
        # At once: the run view, with its first step, not a busy button alone.
        landed = d.eval("new Promise(done => { const t0 = window.__clicked; const tick = setInterval(() => {"
                        " const c = document.querySelector('.wz-body .run-card .pipe-status');"
                        " if (c || Date.now() - t0 > 3000) { clearInterval(tick); done(c ? Date.now() - t0 : null); } }, 20); })")
        if landed is None or landed > 500:
            fail("Start took %s ms to show the run view; it lands there at once" % landed)
        first = text_of(d, ".wz-body .run-card .pipe-status", "the start sequence's line")
        if not first.startswith("Recording 2 decisions"):
            fail("the start sequence does not begin with the decisions recorded: %r" % first)
        if d.eval("document.querySelectorAll('.wz-body .run-card .pipe-status').length") != 1:
            fail("the start sequence is drawn as more than one line")
        d.wait_for("/^Saving the store's settings… — Waiting for the daemon to save the settings/.test("
                   "(document.querySelector('.wz-body .run-card .pipe-status') || {}).textContent || '')",
                   timeout=6, what="the held settings save to say what it waits for")
        decides = d.eval("window.__spy.decides")
        if not decides or any(not b.get("defer") for b in decides):
            fail("the wizard's decisions were not sent with defer: true, so they were applied run-less "
                 "before the run (W1): %s" % json.dumps(decides)[:300])
        final = wait_for_run(d, held["store"], run_id=held["id"])
        if final.get("status") != "done":
            fail("the started run ended %s" % final.get("status"))
        d.wait_for("!!document.querySelector('.wz-body .done-banner')", timeout=30, what="the done card after the run")
        seen = d.eval("window.__pipe")
        lines = seen["lines"]
        steps = [re.split(r"…| — ", l[0])[0] for l in lines if l[2]]
        for want_label in ("Recording 2 decisions", "Saving the store's settings", "Starting the run"):
            if want_label not in steps:
                fail("the start sequence never said %r: %r" % (want_label, steps))
        names = [p.get("phase") for p in final.get("phases") or []]
        if names and "decisions" not in names:
            fail("the run that took the wizard's decisions has no decisions phase: %r" % names)
        if names and "Applying decisions" not in steps:
            fail("the start sequence never showed the run applying the decisions: %r" % steps)
        # Minimum dwell on the start sequence's line, seen on the real page:
        # each step held >= 0.7 s (sampled every 50 ms, so 0.6 s is the floor).
        start = [(re.split(r"…| — ", l[0])[0], l[1]) for l in lines if l[2]]
        firsts = [start[0]] + [start[k] for k in range(1, len(start)) if start[k][0] != start[k - 1][0]]
        short = [(firsts[k][0], firsts[k + 1][1] - firsts[k][1]) for k in range(len(firsts) - 1) if firsts[k + 1][1] - firsts[k][1] < 600]
        if short:
            fail("a start step was on the line for under 0.7 s: %r" % short)
        # The Pipeline: the four lanes in order (images hidden, there are
        # none), one height for the whole run, and a status after the start.
        want("the Pipeline's lanes", seen["lanes"], ["read,chunk,embed,images,write"])
        if len(set(seen["heights"])) > 1:
            fail("the Pipeline changed height across polls: %r" % sorted(set(seen["heights"])))
        after = [l[0] for l in lines if not l[2] and l[0]]
        if not after:
            fail("the Pipeline never said what is slowest once the run started: %r" % lines)
    finally:
        release_held(d, held)
    no_console_errors(d, "the start sequence")
    d.eval("window.__spy.delay = {}; true")
    del name


@finding("rc4.3", "the pipeline names the slowest lane from the run's own phase, and the start line keeps its dwell")
def _(d):
    a_store(d)
    open_clean(d, "stores", fresh=True)
    seen = d.eval(r"""
    (() => {
      const t0 = 1791275400000;
      const steps = [0, 50, 100, 150].map((d, i) => ({label: 'p' + i, at: t0 + d}));
      const out = [];
      for (const now of [t0 + 200, t0 + 600, t0 + 950, t0 + 1650, t0 + 2400, t0 + 2500])
        out.push(dwell('rc4.3', steps, now).steps.length);
      const late = dwell('rc4.3-late', steps, t0 + 10000).steps.length;
      const one = dwell('rc4.3-one', steps.slice(0, 1), t0 + 9000).steps.length;
      const base = {store: 'x', id: 1, status: 'running', scanned: 3148, total: 3987, rows: 79810,
        expected_chunks: 90479, chunks: 74742, images: 12, images_total: 451, backlog: 0,
        lane_rates: {ane: 230.1, cpu: 31}, saves: 3, saved_at: Date.now() - 30000, save_ms: 300};
      const cases = {
        lane: {phase: 'lane', phase_detail: 'Neural Engine compiling 40 %'},
        images: {phase: 'embed', phase_detail: 'embedding images with CLIP'},
        big: {phase: 'read', phase_detail: 'reading and chunking big.json (80 MB)'},
        drain: {phase: 'drain', phase_detail: 'x', backlog: 4864},
        save: {phase: 'save', phase_detail: 'writing'},
        backlog: {phase: 'embed', phase_detail: 'embedding', backlog: 3100},
        pace: {phase: 'embed', phase_detail: 'embedding'},
        done: {status: 'done', elapsed_ms: 872000},
      };
      const out2 = {};
      for (const [k, v] of Object.entries(cases)) {
        const node = pipeline({...base, ...v});
        out2[k] = {status: node.querySelector('.pipe-status').textContent,
          slow: [...node.querySelectorAll('.pipe-row.slow')].map(r => r.getAttribute('data-lane')),
          rows: [...node.querySelectorAll('.pipe-row')].map(r => [r.getAttribute('data-lane'), r.hidden, r.querySelector('.pipe-v').textContent]),
          full: [...node.querySelectorAll('.pipe-row .bar > i')].every(i => parseFloat(i.style.width) === 100)};
      }
      const noImages = pipeline({...base, images_total: 0, saves: 0}).querySelectorAll('.pipe-row');
      const home = {roots: [{path: '/Users/me/notes'}]};
      return {out, late, one, cases: out2,
        noImages: [...noImages].map(r => [r.getAttribute('data-lane'), r.hidden, r.querySelector('.pipe-v').textContent]),
        log: logParts({event: 'phase', phase: 'save', detail: '4,864 chunks still embedding', at: 1791275400}),
        read: logParts({event: 'file', outcome: 'indexing', path: '/Users/me/notes/a/b.md', why: '12 chunks', scanned: 1, total: 2}, home),
        image: logParts({event: 'file', outcome: 'image', path: '/Users/me/notes/p.png', scanned: 2, total: 2}, home),
        left: runLeftText({status: 'running', phases: [], eta_ms: 754000, elapsed_ms: 1000, progress: 0.5})};
    })()
    """)
    want("steps shown at 0.2, 0.6, 0.95, 1.65, 2.4 and 2.5 s after four phases in 0.15 s", seen["out"], [1, 1, 2, 3, 4, 4])
    want("steps shown when the page meets the run 10 s late", seen["late"], 4)
    want("steps shown when only one phase has happened", seen["one"], 1)
    c = seen["cases"]
    want("the lanes, in order", [r[0] for r in c["pace"]["rows"]], ["read", "chunk", "embed", "images", "write"])
    want("the lanes' figures", [r[2] for r in c["pace"]["rows"]],
         ["3,148 / 3,987 files", "79,810 chunks made", "74,742 / 90,479", "12 / 451", "index saved 3 times · last 30 s ago (0.3 s)"])
    want("a run with no images", [(r[0], r[1]) for r in seen["noImages"]][3], ("images", True))
    want("Write before the first save", seen["noImages"][4][2], "not yet")
    expect = {
        "lane": ("Waiting for the Neural Engine to load — Neural Engine compiling 40 %", ["embed"]),
        "images": ("The image model is working through images; text waits (12 of 451)", ["embed"]),
        "big": ("Reading a large file: big.json (80 MB)", ["read"]),
        "drain": ("Finishing embeddings in flight before saving: 4,864 chunks", ["embed"]),
        "save": ("Writing the index to disk", ["write"]),
        "backlog": ("Embed is the slowest step now: Neural Engine 230/s · CPU 31/s · 3,100 chunks waiting", ["embed"]),
        "pace": ("Reading and embedding keep pace", []),
        "done": ("done in 14m 32s · 3 saves", []),
    }
    for k, (status, slow) in expect.items():
        want("the status line when %s" % k, c[k]["status"], status)
        want("the highlighted lane when %s" % k, c[k]["slow"], slow)
    if not c["done"]["full"]:
        fail("a finished run's lanes are not all full")
    want("a phase line in the log", seen["log"][1:3], ["phase", "Writing the index to disk — 4,864 chunks still embedding"])
    want("a text file's log line", seen["read"][1:3], ["read", "a/b.md — 12 chunks"])
    want("an image's log line", seen["image"][1:3], ["image", "p.png"])
    want("a run-truth run's time left", seen["left"], "about 13 min")
    no_console_errors(d, "the pipeline")


@finding("rc4.4", "the scan card follows the scan's phases, each for 0.7 s, under a Scanning… heading")
def _(d):
    d.clear_console()
    wizard_to(d, 2)
    root = clean_tree(d, "rc4scan")
    wizard_mode(d, "Paste a path")
    d.type(".wz-body .box input", root)
    d.press("Enter")
    d.wait_for("document.querySelectorAll('.wz-body .src-row').length > 0", what="the pasted folder as a source")
    # Intervals the page starts from here on can be held, so the card can be
    # measured at four widths while it is on screen; the sampler is not held.
    d.eval(r"""
    (() => {
      window.__realSetInterval = window.__realSetInterval || window.setInterval.bind(window);
      window.setInterval = (fn, ms, ...a) => window.__realSetInterval(() => { if (!window.__hold) fn(); }, ms, ...a);
      const seen = window.__scan = {cur: [], heads: [], index: []};
      seen.timer = window.__realSetInterval(() => {
        const box = document.querySelector('.wz-body .stage-box.cur');
        const word = box ? box.getAttribute('data-stage') : (document.querySelector('.wz-body .stage-box') ? '' : 'gone');
        if (!seen.cur.length || seen.cur[seen.cur.length - 1][0] !== word) seen.cur.push([word, Date.now()]);
        if (document.querySelector('.wz-body .stage-box')) {
          seen.heads.push((document.querySelector('.wz-head .h') || {}).textContent);
          const row = [...document.querySelectorAll('.sum-row')].find(r => /INDEX/.test(r.innerText));
          seen.index.push(row ? row.querySelector('.v').textContent : null);
        }
      }, 50);
      return true;
    })()
    """)
    press_text(d, ".wz-foot button", "Scan 1 source", "Scan 1 source")
    d.wait_for("!!document.querySelector('.wz-body .stage-box.cur[data-stage=\"read\"], .wz-body .stage-box.cur[data-stage=\"credentials\"]')",
               timeout=30, what="the scan card to reach its second stage")
    d.eval("window.__hold = true")
    bad = []
    try:
        for width in (1440, 1024, 768, 390):
            d.set_viewport(width, 900, mobile=width < 500)
            pause(d, 60)
            for box in d.eval(r"""[...document.querySelectorAll('.wz-body .stage-box')].map(b => {
                const l = b.querySelector('.sb-label').getBoundingClientRect(), s = b.querySelector('.sb-status');
                const r = s.getBoundingClientRect(), lh = parseFloat(getComputedStyle(s).lineHeight) || 16;
                return {stage: b.getAttribute('data-stage'), below: r.top >= l.bottom - 1, oneLine: r.height <= lh * 1.5,
                        fits: s.scrollWidth <= s.clientWidth + 1 || getComputedStyle(s).textOverflow === 'ellipsis', text: s.textContent}; })"""):
                if not (box["below"] and box["oneLine"]):
                    bad.append("%dpx %s %r: %s" % (width, box["stage"], box["text"], "beside its label" if not box["below"] else "wraps"))
    finally:
        d.reset_viewport()
        d.eval("window.__hold = false")
    if bad:
        fail("a stage's status is not on one line of its own under its label: %s" % "; ".join(bad[:4]))
    d.wait_for("!!document.querySelector('.wz-body .q4')", timeout=60, what="the review step after the scan")
    seen = d.eval("clearInterval(window.__scan.timer), window.__scan")
    order = [c[0] for c in seen["cur"] if c[0] not in ("", "gone")]
    want("the stages the scan card went through", order, ["walk", "read", "credentials", "rules"])
    cur = seen["cur"]
    short = [(cur[i][0], cur[i + 1][1] - cur[i][1]) for i in range(len(cur) - 1) if cur[i][0] not in ("", "gone") and cur[i + 1][1] - cur[i][1] < 600]
    if short:
        fail("a scan stage was current for under 0.7 s: %r" % short)
    heads = set(seen["heads"])
    want("the heading while the scan card is up", heads, {"Scanning…"})
    if any(v != "after the scan" for v in seen["index"]):
        fail("the sidebar's INDEX row read %r during the scan" % sorted(set(seen["index"])))
    if text_of(d, ".wz-head .h", "the heading after the scan") == "Scanning…":
        fail("the heading still reads Scanning… on the review step")
    no_console_errors(d, "the scan card")
    press_text(d, "header.top button", "Exit setup", "leaving the wizard")
    if d.modal_open():
        d.modal_press("Delete store")


@finding("rc4.5", "a bulk decision shows its progress in batches of 100, holds the table, and ends on a done line")
def _(d):
    d.clear_console()
    store = indexed_fixture(d, secret_tree(d, "rc4bulk", 150))
    d.open_view("store/%s/review" % store, fresh=True)
    d.wait_for("document.querySelectorAll('#main .dec-list .dec-grid').length >= 150", timeout=30,
               what="150 files waiting for a decision")
    d.eval(FETCH_SPY)
    d.eval("window.__spy.delay['/api/refused/decide'] = 400; true")
    d.eval(r"""
    (() => {
      const seen = window.__bulk = {lines: [], inert: false};
      seen.timer = setInterval(() => {
        const bar = document.querySelector('#main .selbar.bulk');
        const t = bar ? bar.innerText.trim() : '';
        if (t && seen.lines[seen.lines.length - 1] !== t) seen.lines.push(t);
        if (bar && !/applied|stopped/.test(t) && document.querySelector('#main .dec-list[inert]')) seen.inert = true;
      }, 20);
      return true;
    })()
    """)
    d.eval("document.querySelector('#main .dec-grid.head [role=checkbox]').click()")
    d.wait_for("!!document.querySelector('#main .selbar:not(.bulk)')", what="the selection bar")
    press_text(d, "#main .selbar button", "Keep out", "Keep out on the selection")
    d.wait_for("/applied to|stopped at/.test((document.querySelector('#main .selbar.bulk') || {}).innerText || '')",
               timeout=60, what="the bulk decision to end")
    seen = d.eval("clearInterval(window.__bulk.timer), window.__bulk")
    sizes = [len(b["files"]) for b in d.eval("window.__spy.decides")]
    want("the decide requests' sizes", sizes, [100, 50])
    progress = [l for l in seen["lines"] if l.startswith("Applying 'Keep out' to 150 files…")]
    counts = [l.rsplit("…", 1)[1].strip() for l in progress]
    if "0 / 150" not in counts or "100 / 150" not in counts:
        fail("the progress line did not count the batches: %r" % seen["lines"])
    if not seen["inert"]:
        fail("the table took input while the decision was applied")
    if not re.search(r"'Keep out' applied to 150 files", seen["lines"][-1]):
        fail("the bulk decision did not end on a done line: %r" % seen["lines"][-1])
    kept = [r for s in d.api("/api/refused")["stores"] if s["store"] == store for r in s["rows"] if r.get("accepted") in ("refused", "kept")]
    want("files kept out", len(kept), 150)
    no_console_errors(d, "the store's bulk decision")
    # The wizard records locally, at once, and says so in the same place.
    d.clear_console()
    root = review_tree(d, "rc4wzbulk")
    wizard_to(d, 3, root=root)
    held = held_run(d, root)
    try:
        d.eval("document.querySelector('.wz-body .dec-grid.head [role=checkbox]').click()")
        press_text(d, ".wz-body .selbar button", "Keep out", "Keep out on the wizard's selection")
        line = text_of(d, ".wz-body .selbar.bulk", "the wizard's bulk line")
        if not re.search(r"'Keep out' recorded for 2 files — applied when the run starts", line):
            fail("the wizard's bulk decision says %r" % line)
        # Before leaving: dropping a held scan's store is the leave flow's, not this.
        no_console_errors(d, "bulk decisions")
    finally:
        press_text(d, "header.top button", "Exit setup", "leaving the wizard")
        if d.modal_open():
            d.modal_press("Delete store")
        release_held(d, held)


#: A card's shape: every element's tag and first class, depth first, leaving
#: out what grows (the log's lines).
CARD_SHAPE = r"""
(sel => {
  const card = document.querySelector(sel);
  if (!card) return null;
  const out = [];
  const walk = (n, depth) => {
    out.push(depth + ':' + n.tagName.toLowerCase() + '.' + ((n.getAttribute('class') || '').split(' ')[0]));
    if (n.matches('.log')) return;
    for (const k of n.children) if (k.tagName !== 'svg') walk(k, depth + 1);
  };
  walk(card, 0);
  return {shape: out, run: card.getAttribute('data-run'), cls: card.className,
          stats: [...card.querySelectorAll('.run-stats .eyebrow')].map(e => e.textContent),
          steps: card.querySelectorAll('.pipe .pipe-row').length};
})
"""


@finding("rc4.7", "the wizard's Index step and the store's Runs tab draw a live run with the same card")
def _(d):
    """W8: the owner wants one run card, the wizard's, wherever a live run is
    shown in full. The same run is read on both pages while it is live."""
    d.clear_console()
    root = d.fixtures.unique("rc4same", count=1500)
    name = wizard_to(d, 4, root=root)
    press_text(d, ".wz-foot button", "Start indexing", "Start indexing")
    d.wait_for("!!document.querySelector('.wz-body .run-card[data-run]')", timeout=60, what="the wizard's run card")
    wizard = d.eval("(%s)('.wz-body .run-card[data-run]')" % CARD_SHAPE)
    store, run_id = wizard["run"].rsplit(":", 1)
    try:
        d.open_view("store/%s/runs" % store, fresh=True)
        d.wait_for("!!document.querySelector('#main .run-card[data-run=%s]')" % json.dumps(wizard["run"]), timeout=30,
                   what="the same run's card on the Runs tab")
        tab = d.eval("(%s)('#main .run-card[data-run=%s]')" % (CARD_SHAPE, json.dumps(wizard["run"]).replace("'", "\\'")))
        live = run_by_id(d, int(run_id))
        if not live or live.get("status") in TERMINAL:
            skip("the run finished before the Runs tab was read; the machine is faster than the corpus")
        want("the card's classes on the two pages", tab["cls"], wizard["cls"])
        want("the card's counters on the two pages", tab["stats"], wizard["stats"])
        if tab["shape"] != wizard["shape"]:
            diff = [(a, b) for a, b in zip(wizard["shape"], tab["shape"]) if a != b][:4]
            fail("the two pages draw the run differently (wizard vs Runs tab): %r" % (diff or (len(wizard["shape"]), len(tab["shape"]))))
        if tab["steps"] != 5:
            fail("the Runs tab's card draws %d pipeline lanes, not Read, Chunk, Embed, images and Write" % tab["steps"])
        no_console_errors(d, "the run card on two pages")
    finally:
        stop_quietly(d, store)
    del name


@finding("rc4.8", "a run card's Pipeline | Log strip switches the panel, and the choice is kept")
def _(d):
    d.eval("try { localStorage.removeItem('semlith-run-tab'); } catch (e) {}")
    run_id, store = start_index(d, d.fixtures.unique("rc4tabs", count=1500))
    card = "document.querySelector('#main .run-card')"
    shown = ("(() => { const c = %s; return c && {pipe: !c.querySelector('.pipe').hidden, log: !c.querySelector('.log').hidden,"
             " tab: (c.querySelector('.run-tabs .tab[aria-selected=true]') || {}).textContent}; })()" % card)
    try:
        d.open_view("store/%s/runs" % store, fresh=True)
        d.wait_for("!!%s" % card, timeout=30, what="the run's card")
        want("the panel first shown", d.eval(shown), {"pipe": True, "log": False, "tab": "Pipeline"})
        press_text(d, "#main .run-card .run-tabs .tab", "Log", "the Log tab")
        want("the panel after pressing Log", d.eval(shown), {"pipe": False, "log": True, "tab": "Log"})
        d.open_view("store/%s/runs" % store, fresh=True)
        d.wait_for("!!%s" % card, timeout=30, what="the run's card after a reload")
        want("the panel after a reload", d.eval(shown), {"pipe": False, "log": True, "tab": "Log"})
        press_text(d, "#main .run-card .run-tabs .tab", "Pipeline", "the Pipeline tab")
        want("the panel after pressing Pipeline", d.eval(shown), {"pipe": True, "log": False, "tab": "Pipeline"})
        no_console_errors(d, "the run card's tabs")
    finally:
        d.eval("try { localStorage.removeItem('semlith-run-tab'); } catch (e) {}")
        stop_quietly(d, store)


@finding("rc4.9", "the Stop dialog's delete option is a checkbox and its words on one row, with no box")
def _(d):
    run_id, store = start_index(d, d.fixtures.unique("rc4stop", count=1500))
    try:
        running(d, run_id, store)
        d.open_view("store/%s/runs" % store, fresh=True)
        d.wait_for(card_offers(CARD, "Stop…"), what="the live run's card, offering Stop")
        for width, height, mobile in ((1440, 900, False), (390, 844, True)):
            d.set_viewport(width, height, mobile=mobile)
            press_in(d, CARD, "Stop…")
            d.wait_for("!!document.querySelector('#stop-delete')", what="the Stop dialog's delete option")
            row = d.eval(r"""(() => { const box = document.querySelector('#stop-delete'), label = box.closest('label');
                const words = label.querySelector('span'), a = box.getBoundingClientRect(), b = words.getBoundingClientRect();
                const cs = getComputedStyle(label);
                return {overlap: Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top), beside: b.left >= a.right - 1,
                        shadow: cs.boxShadow, modalClass: label.classList.contains('modal')}; })()""")
            if row["overlap"] <= 0 or not row["beside"]:
                fail("at %dpx the checkbox and its words are not on one row: %r" % (width, row))
            if row["shadow"] != "none" or row["modalClass"]:
                fail("at %dpx the option is drawn as a box with a shadow: %r" % (width, row))
            d.modal_press("Keep running")
            d.wait_for("!document.querySelector('#stop-delete')", what="the dialog to close")
    finally:
        d.reset_viewport()
        stop_quietly(d, store)


@finding("rc4.6","the Index step's estimate is this machine's lanes over the plan, and moves as a lane is switched")
def _(d):
    accel = d.api("/api/accel")
    rates = accel.get("rates")
    if not rates:
        skip("this daemon sends no lane rates (/api/accel `rates`), so there is nothing to estimate from")
    d.clear_console()
    name = wizard_to(d, 4)
    lanes = [l for l in accel["lanes"] if (l.get("status") or {}).get("state") != "unavailable"]
    other = next((l for l in lanes if l["lane"] != "cpu" and (rates.get(l["lane"]) or {}).get("per_s")), None)
    if not other:
        skip("no lane besides the CPU has a rate here, so switching one cannot move the estimate")

    def reading():
        return d.eval("(() => { const k = [...document.querySelectorAll('.wz-body .kpi')].find(k => /Estimate/i.test(k.innerText));"
                      " return k ? {ms: Number(k.getAttribute('data-estimate-ms')), text: k.innerText,"
                      " foot: document.querySelector('.wz-foot').innerText} : null; })()")

    before = reading()
    if not before or not before["ms"]:
        fail("the Index step shows no estimate: %r" % before)
    if "on this machine" not in before["text"] or "on this machine" not in before["foot"]:
        fail("the estimate is not labelled as this machine's: %r" % before)
    was = other["enabled"]
    label = other.get("label") or other["lane"]
    card = ("[...document.querySelectorAll('.wz-body .pick-card')].find(c =>"
            " ((c.querySelector('.t') || {}).textContent || '').trim() === %s)" % json.dumps(label))

    def switch(on):
        """Press the lane's card and read the estimate once the daemon's
        answer is drawn, so both readings come from the same rates."""
        if not d.eval("(() => { const c = %s; if (!c) return false; c.click(); return true; })()" % card):
            fail("the Index step offers no %s lane card" % label)
        d.wait_for("(() => { const c = %s; return !!c && c.getAttribute('aria-pressed') === %s; })()" % (card, json.dumps(str(on).lower())),
                   timeout=15, what="%s to read %s" % (label, "on" if on else "off"))
        pause(d, 1500)
        return reading()

    try:
        plan = (run_for(d, name) or {}).get("plan") or {}
        if plan.get("chunks") is None:
            skip("the held plan counts no chunks: %s" % json.dumps(plan)[:200])

        def expected():
            """The plan's chunks over the rates of the lanes a run would use:
            the Neural Engine alone when it is on (the GPU beside it only when
            asked), every lane that is on otherwise."""
            a = d.api("/api/accel")
            r = a.get("rates") or {}
            on = [l["lane"] for l in a["lanes"] if l.get("enabled") and (l.get("status") or {}).get("state") != "unavailable"]
            if "ane" in on and r.get("ane"):
                on = [x for x in on if x != "cpu" and (x != "gpu" or a.get("gpu_beside_ane"))]
            per_s = sum((r.get(x) or {}).get("per_s") or 0 for x in on)
            return round(plan["chunks"] / per_s * 1000) if per_s else None

        first = switch(not was)
        first_want = expected()
        second = switch(was)
        second_want = expected()
        if first["ms"] == second["ms"] and first_want != second_want:
            fail("the estimate did not move when %s was switched: %r ms both ways" % (label, first["ms"]))
        for seen, wanted, state in ((first, first_want, not was), (second, second_want, was)):
            if wanted and abs(seen["ms"] - wanted) > max(2, wanted * 0.02):
                fail("with %s %s the estimate reads %r ms; the plan's %d chunks over the lanes' rates is %r ms"
                     % (label, "on" if state else "off", seen["ms"], plan["chunks"], wanted))
    finally:
        now = next((l for l in d.api("/api/accel")["lanes"] if l["lane"] == other["lane"]), {})
        if now.get("enabled") != was:
            d.api_result("/api/accel", method="POST", body={"lane": other["lane"], "action": "on" if was else "off"})
        press_text(d, "header.top button", "Exit setup", "leaving the wizard")
        if d.modal_open():
            d.modal_press("Delete store")
    no_console_errors(d, "the Index step's estimate")


@finding("v6.shots", "every v6 view, in light and dark, at 1440px and 390px")
def _(d):
    """The release record's evidence: one screenshot per view per theme per
    width, into `<out>/shots/`. Fails only if a view does not draw."""
    views = every_view(d) + ["welcome", "new"]
    folder = os.path.join(d.out_dir, "shots")
    os.makedirs(folder, exist_ok=True)
    bad = []
    try:
        for theme in ("light", "dark"):
            d.eval("try { localStorage.setItem('semlith-theme', %s); } catch (e) {}" % json.dumps(theme))
            for width, height, mobile in ((1440, 900, False), (390, 844, True)):
                d.set_viewport(width, height, mobile=mobile)
                for i, view in enumerate(views):
                    try:
                        d.open_view(view, fresh=(i == 0 or view in ("welcome", "new")))
                        if view == "graph":
                            pause(d, 1200)  # let the live layout settle before the picture
                    except cdp.ProtocolError as error:
                        bad.append("%s %s %d: %s" % (view, theme, width, str(error)[:120]))
                        continue
                    slug = re.sub(r"[^a-z0-9]+", "-", view.lower()).strip("-")
                    d.screenshot(os.path.join(folder, "%s-%d-%s.png" % (theme, width, slug)))
    finally:
        d.reset_viewport()
        d.eval("try { localStorage.removeItem('semlith-theme'); } catch (e) {}")
    if bad:
        fail("%d view(s) did not draw: %s" % (len(bad), "; ".join(bad[:4])))
