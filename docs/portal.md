# The portal

`semlith start` serves a web page on this machine. It is the same index your
agents query over MCP and the same one `semlith search` reads from a terminal,
shown to a person rather than to a program: what is indexed, what a question
returns, what the code graph knows, which agents are connected, and what has been
recorded about all of it.

This document assumes you have never opened it and have never met the words
"vector index" or "code graph". Every page is described from what it is for, then
what each control does, then what each column, badge and number means and what
computes it. If a word is unfamiliar, the [Concepts](#concepts) section at the end
defines it once.

Every byte the page loads is compiled into the binary. There is no CDN, no build
step and no npm; the fonts and logos are bundled, and the page loads with the
cable unplugged.

## Starting it

```sh
semlith start
```

The daemon binds `127.0.0.1` and nothing else. There is no flag to change the
bind address, and `tests/` asserts that rather than the README stating it. The
default port is `7365` — it spells SEML on a phone keypad and is registered to
nothing — and `--port` or `SEMLITH_PORT` moves it.

Two lines are printed. The first, on stdout, is the URL:

```
http://127.0.0.1:7365/?token=<the session token>
```

The second, on stderr, says to open it and that Ctrl-C stops the daemon. Ctrl-C
is the whole shutdown: the watchers stop, the vector index is checkpointed, the
per-store write lock is released, and the discovery file that `semlith mcp` reads
is removed last so nothing is ever pointed at a daemon that has stopped
answering.

### The token in the URL, and why it is not a cookie

The `?token=` in that URL is the only time the session token is printed. Opening
the URL hands it to the page, which stores it and immediately rewrites the
address bar so the credential is not left in your browser history, in a bookmark
or in a screenshot of the tab.

From then on the page sends it back as a `Semlith-Token` request header on every
`/api/*` call. It is deliberately not a cookie. Every port on `localhost` is the
same site as far as a browser is concerned, so `SameSite=Strict` would not
separate this daemon from a page served by anything else on `127.0.0.1` — a
cookie set here would travel to that page. A header will not. Nothing in the
daemon sets a cookie or reads one.

Three rules sit in front of every request, and each is checked before the one
after it:

| Rule | What happens when it fails |
|---|---|
| `Host` must be `localhost`, `127.0.0.1` or `::1` | 400, before anything else runs |
| A write must carry a JSON content type and, from a browser that sends fetch metadata, `Sec-Fetch-Site: same-origin` | 403 with an empty body |
| Every `/api/*` route needs the session token; `/mcp` takes the agent key as a bearer instead | 401 with an empty body, answered after a delay |

The page itself, its stylesheet, its script, its fonts and its icons are the one
credential-free surface, because a browser attaches no header to a stylesheet.
Every response carries `Content-Security-Policy: default-src 'self'` with
`style-src 'self'` and no `unsafe-inline`, and no CORS header is emitted
anywhere. That policy is why the page has no inline `style` attribute anywhere:
one would be dropped silently. Sizes that come from data, such as a bar's width,
are set through the CSSOM instead, and `tests/portal.rs` fails on an inline
style.

## The shell

Everything except the Welcome screen and the store wizard sits inside one frame:
a header across the top, the navigation down the left, and the page.

### The navigation

Nine pages in three groups, in this order, and the page sections below are in
the same order:

| Group | Pages |
|---|---|
| Workspace | [Home](#home), [Stores](#stores), [Search](#search), [Graph](#graph) |
| Agents | [Agents](#agents), [Ledger](#ledger), [Reports](#reports) |
| Machine | [Privacy](#privacy), [Settings](#settings) |

The first group is what you have indexed and what you can ask of it, the second
is what agents do with it, and the third is this machine. Each store also has
[a page of its own](#a-stores-page), reached from Stores, Home or the header's
run pill; it lights the Stores entry and the header's breadcrumb reads
`Stores / <name>`.

Two entries carry a badge. **Stores** shows the number of files waiting for a
review decision across every store, and **Agents** shows `!` while no agent
client is registered. Neither is a count of everything that happened, only of
what is waiting for you.

**The address bar carries the page.** Every route is a fragment of the form
`#/<page>/<part>`, so any tab of any page can be bookmarked, reloaded or pasted:

| Fragment | What opens |
|---|---|
| `#/home` | Home. An empty fragment is Home too. |
| `#/stores`, `#/stores/inside` | Stores: All stores, or Inside the index. |
| `#/store/<name>`, `#/store/<name>/files`, `…/review`, `…/runs`, `…/settings` | A store's page and its tabs. |
| `#/search` | Search. |
| `#/graph`, `#/graph/blast`, `#/graph/path` | Graph: Explore, Blast radius, Path & evidence. |
| `#/agents`, `#/agents/add`, `#/agents/tools`, `#/agents/health` | Agents: Connected, Add a client, Tools, Health. |
| `#/ledger`, `#/ledger/retrievals`, `#/ledger/replay` | Ledger: Sessions, Retrievals, Session replay. |
| `#/reports` | Reports. |
| `#/privacy` | Privacy. |
| `#/settings`, `#/settings/access`, `#/settings/cloud`, `#/settings/about` | Settings: Performance, Agent access, Cloud, About. |
| `#/welcome` | The Welcome screen, on request. |
| `#/new` | The store wizard. |

Until 0.35.0 the portal had fourteen pages in four groups. Their surfaces moved
rather than went: Files is a store's Files tab, Index is the wizard and a store's
Runs tab, Inside the index is a tab on Stores, Impact is Graph's Blast radius,
the path finder and Trace are Graph's Path & evidence, Doctor is Agents › Health,
About and Cloud are Settings sections. The one list that was dropped is the
files list across every store; Search's path filter answers the same question.

**The width decides the layout, not the device.** The frame measures its own
width, so a browser in split screen gets the layout of its size. From 1100
pixels the navigation is open beside the page. Between 760 and 1100 it collapses
to a 56-pixel rail of icons, each with a tooltip and its badge. Under 760 it is
an overlay opened from the menu button, and it closes when it takes you
somewhere.

**The daemon card** sits at the foot of the navigation: the address the daemon
answers on, that it is the sole writer, how many stores are open, how many
agents are connected, and whether the ledger is recording, paused or off. In the
rail the same facts are the tooltip of the green dot. When the daemon stops
answering, a line under the header says so and names `semlith start`; the page
reconnects by itself when it is back.

### The header

From the left: the menu button, the logo (which goes to Home), the breadcrumb,
and the **run pill** while anything is indexing. The pill names the run's store,
its state and its percentage — `Indexing api · 42%`, or `Waiting for review` in
amber — and opens that store's Runs tab. On the right: **Ask the index a
question**, which opens Search and is also the `/` key from anywhere outside a
text field; **New store**, which opens the [store wizard](#the-store-wizard);
and the theme control.

**The theme** is light, dark or system. System follows the operating system's
`prefers-color-scheme` as it changes, without a reload. The choice is kept in the
browser's local storage, and a private window that refuses storage starts on
system.

### Shared pieces

A few controls behave the same everywhere, so they are described once.

- **Tables** sort by a column when its heading is pressed, page with a numbered
  pager, and let you choose how many rows a page holds. Where a table has
  checkboxes, the heading box selects the rows on the page and a bar offers to
  select every row the filter matches, naming the count before it does.
- **The tooltip** shows data rows — a label and a value — and the whole of any
  text that was cut short in its cell.
- **Confirmations** are a modal that names what is about to happen and to what.
  Escape closes it, and closes any open menu.
- **Toasts** say what an action did, in a sentence, at the foot of the window.

### Loading

The first load shows the boot animation, and a page whose data has not arrived
shows a loader with a rotating tip. Both are driven by real loading: each is on
screen exactly while that screen's first fetches are pending and gone the frame
they answer. A page whose data is already cached shows no loader at all, draws
from the cache at once, and brings itself up to date behind what is on screen.
With `prefers-reduced-motion` the animation does not move.

### What reads live

**Most of the portal reads live.** The daemon keeps one counter per data domain —
stores, runs, clients, ledger, events, privacy — and bumps it in the one function
that writes that domain. The page polls those six integers once a second from
`GET /api/changes` and refetches only the domains that moved, so a quiet daemon
with a tab open costs one small request a second and nothing else. It is a poll
rather than a stream because a stream would hold one of the daemon's eight HTTP
workers for as long as the tab is open, and eight tabs would then be a portal
that cannot answer. A tab in the background stops asking, and catches up in one
pass when it comes back.

Search, Graph and Reports deliberately do not redraw themselves: each is the
answer to a question somebody asked, and redrawing it under them would be
answering a different one. The rest say below what they follow.

## Welcome

**What it is for.** Getting from nothing to a working store. A daemon with no
store shows this screen in place of the shell; once one store exists it is not
shown again unless you open it from Settings › About (**Open the first-run
screen**) or at `#/welcome`. Agents, Privacy and Settings stay reachable while
there is no store, because connecting a client and checking the machine do not
need one.

On the left, the five steps the wizard walks — name a store, add folders or drop
files, review what looks sensitive, index, connect your agents — and two
buttons. **Create your first store** opens the [store wizard](#the-store-wizard)
with the Connect step at the end. **Adopt an existing .semlith** opens a folder
picker confined to your home, in which folders that already hold a store are
marked; the one you choose is registered and opened with nothing re-embedded.
Under them, **Prefer the terminal?** and a copyable
`semlith index ~/path/to/folder`.

On the right, **This machine**: six checks, each read from the daemon rather than
written into the page.

| Check | What it reads |
|---|---|
| Daemon | The address it answers on, loopback only, and that it is the sole writer. |
| Accelerator | The lane that will embed — the Neural Engine, a GPU or the CPU — and its state: ready, compiling or downloading with a percentage. |
| Memory | Total and free memory, and how many runs at once that leaves room for. |
| Embedding model | Whether the model is already in the model cache. When it is not, the line says it is fetched once when the first run starts; with airgap on, that the cache has to be pre-seeded first. |
| Agent clients | The clients found on this machine, by name. None found is not a fault: any MCP client can connect later. |
| Telemetry | None. No analytics, and no update check of its own. |

The model download is described in one wording here and in the wizard: it is the
only thing semlith fetches on its own, it happens once, and starting the first
run counts as your agreement to it.

## The store wizard

**What it is for.** Making a store, filling it and connecting agents to it, in
one flow that asks before anything sensitive goes in. **New store** in the
header, on Home and on Stores opens it at step 1; a store's **Add sources** opens
it at step 2 for that store. The wizard replaces the shell while it is open, and
its steps are numbered across the top with what each step chose so far summarised
down the side.

The steps are **Name**, **Sources**, **Review** and **Index**, and **Connect**
when the wizard was opened from Welcome.

### Name

A store's name is what agents see when they choose where to look, so it has
rules: lowercase letters, digits and dashes, starting with a letter or digit, up
to forty characters. The field says as you type whether the name is available
and where the store will live (`~/.semlith/stores/<name>`), or why it is not —
not the right shape, or already a store on this machine.

**What will it hold?** is one of three: **Code**, **Docs & notes** or **Both**.
It is kept with the store and sets the store's default lean in search — code
leans to source files, docs to prose, both weighs them equally — and it can be
changed any time on the store's Settings tab. It does not filter anything. The
embedding model is shown and fixed: vectors from two models are not comparable,
so a store keeps its model for life.

**Create store** creates the store, empty, at this step. A store exists from
here on; leaving the wizard before anything is indexed asks whether to **Delete
store** or **Keep it**, so an abandoned wizard does not leave an empty store
behind by accident.

### Sources

What the store should read. Nothing is copied or uploaded: every source is read
where it sits and, with **Watch for changes** on, re-indexed when it is saved.

- **Drop folders or files** onto the drop zone. See
  [Drag and drop](#drag-and-drop) for how a dropped item's real path is found.
- **Browse folders** — a picker confined to your home that can select more than
  one folder in a visit, and lists the folders other stores already read.
- **Paste a path** — one path per line. Quotes are stripped, a `file://` URL is
  decoded and a leading `~` is your home.
- **Add a URL** — one https request for exactly that URL, a page, a PDF or a file
  on GitHub, made when the run starts. Nothing is crawled and no credential is
  sent. The file is kept in the store's own downloads folder, never in your
  working tree. With airgap on, the card says the fetch will be refused.

Each source is listed with what it is — a folder and how many repositories are
inside it, a single file, a URL — and a remove button. A folder that holds
several git repositories (the daemon's own discovery decides, the same one
`semlith index --projects` uses) offers **Keep together** — one store, searched
across all of them — or **One store each**, which makes each repository its own
store named after its folder, as `semlith index --each` does. **Respect
.gitignore** skips what each repository already ignores.

### Review

Nothing is embedded at this step. **Scan** reads names and hashes only — it walks
the tree, reads and hashes each file, checks it for credentials and matches it
against the rules — and then shows what it found, with the stages ticking off as
they finish.

The summary counts what will be read: files to index, the text they hold, how
many are unchanged and will be skipped by their hash, and an estimate of the
embedding time from this machine's last measured rate.

**Needs your decision** is the grey zone: files the scan held back that a person
may let in. Each row carries the file, why it was held — a secret-shaped value or
a policy limit such as the size cap or a generated folder — the **risk if
indexed** as a percentage and a band, the likely label and kind of what matched,
the evidence with the value masked, and a suggested action. The decisions are
**Keep out**, **Redact & index** and **Index**. Redact & index swaps each detected
value for a typed placeholder before chunking, so the value never reaches the
index; it covers only what the scanner detected.

Decisions can be made one file at a time or on a selection. Filter the list by
view (undecided or decided) and by risk, select rows or **Select all shown**, and
apply one decision to all of them; **Apply suggestions to N undecided** takes
every suggested action at once, and **Undo** reverses the last one. Every
decision is logged per file as yours, whether it was made singly or in bulk. An
agent can never make one: the route refuses any caller that is not a person on
this page.

Until 0.35.0 a refused file could be accepted only one at a time. The owner
reversed that for this release; the rule that an agent never decides did not
change.

**Left out automatically** lists, by group, what needs no decision: files ignored
by `.gitignore` and the rules, build output, lockfiles and oversize files, files
with no text to read, and credential files. Credential files — keys, tokens,
`.env` — are never offered, even with your agreement. A file left undecided stays
out, and every decision can be changed later on the store's Review tab.

### Index

**Run it on** lists this machine's embedding lanes with the rate each one last
measured, and a switch for each. The lanes are the machine's, not the store's —
the same switches as [Settings › Performance](#performance) — and a lane whose
pack is not installed asks before it downloads it. **Keep watching after the
run** and **Record what agents retrieve** are this store's watch and ledger
settings.

If the embedding model is not on disk, the step says so, names the source and
the size, and treats **Start indexing** as your agreement to that one download.

While the run goes, the step shows it live: the stages, files read, chunks
written, the rate, the time left and the log, with **Pause**, **Resume** and
**Stop…**. Stop undoes what the run embedded and can also delete the store; see
[Runs](#runs). The run lives in the daemon, not the page: the tab can be closed
and the header's run pill follows it. When it finishes, **Try it** runs a search
against the new store, the way an agent would see it.

### Connect

Shown when the wizard started from Welcome. It lists the agent clients found on
this machine, says which file each one's registration writes, and that each file
is backed up beside itself before it is written. **Register** writes the
selected clients; nothing is written until it is pressed. **Set a client up by
hand instead** shows the configuration to copy. Skipping is fine: Agents › Add a
client does the same thing at any time.

### Drag and drop

No browser gives a page the path of a dropped item, so a drop cannot simply be
read from disk, and semlith does not upload anything. Instead the page sends the
daemon a description of each dropped item — its name, whether it is a file or a
folder, its size and modification time, and for a folder the names of its first
level and the sizes and times of up to twenty files — to `POST
/api/drop/resolve`, and the daemon finds the item on this machine.

The daemon tries, in order: on macOS, the drag pasteboard Finder wrote, accepted
only when it changed for this drag and every name and kind match; on Windows, the
selected items of open Explorer windows, with the same match; then on every
system, the operating system's own file index (Spotlight, Windows Search, or
plocate, localsearch or Baloo where present), the roots of existing stores and
their parents, and folders recently browsed; and last a bounded walk, capped at
two seconds, filtered by the description.

What it finds is offered for you to confirm — **Add this path?** — and nothing is
added until you do. Several matches give a picker. No match puts the cursor in
the path box and names this system's shortcut for copying a path. A path inside a
temporary folder, which is where an item dragged out of an archive lands, is
refused with a request to extract it first.

The file-manager helpers are the other way in: with them installed (Settings ›
About), selecting files in Finder, Explorer, Nautilus, Dolphin or Thunar offers
**Index with semlith**, which runs `semlith index` on the selection.

## Home

**What it is for.** What is indexed, what needs you, which agents are connected
and what they asked — each one click from where it is handled.

**Getting started** is a checklist of four: create a store, connect an agent,
run a first search, and check what leaves the machine on Privacy. Each item ticks
itself from the daemon's state, and the card can be dismissed.

**The figures** across the top:

| Figure | What it counts |
|---|---|
| Stores | How many stores, and beneath it what is going on: runs going now, stores that need a review, or that all are fresh and watched. |
| Files indexed | Files across every store, and the chunks they were cut into. |
| Agents connected | Clients talking to the daemon now, and what was asked most recently; or how many are registered and how many were found. |
| Fewer tokens | What agents read against reading those files whole, with the coverage and the tier beside it. A saving never appears without both — see [Ledger](#ledger). |

**Stores** is the store table at a glance — state, files, saved, last written —
with **+ New store** and **All stores →**. **What agents asked** lists the most
recent retrievals with what each was sent, and says so when recording is paused.
**Needs attention** lists what is waiting, each with the button that handles it:
files held for review, a store whose directory is gone or whose database did not
open, a source that is missing (**Re-point**), a ledger chain that does not
verify (**Inspect**), reclaimable space (**Compact**), no agent connected
(**Connect**), a registered client that cannot reach semlith, and a run that
finished. When nothing is waiting it says so. **Agents** summarises the
registered clients, and **Watcher** is a live feed of what has changed on disk
since the daemon started, most recent first.

**This page reads live**, on every domain it shows.

## Stores

**What it is for.** Every store this machine has, what each holds, and whether
anything is keeping it current.

A **store** is one directory holding two pieces of state that must agree: a
vector index of the text it has read, and a SQLite database holding that text,
its file paths, line spans and content hashes. A new store is created under
`~/.semlith/stores/<name>`, and a registry file beside it records which
directories on disk — its **roots** — that store covers.

One store holds several roots because a store is a corpus, not a folder. If your
work spans `~/work/api` and `~/work/web` and you want one question answered
across both, they belong in one store: a single query, a single ranking, one
answer. Separate stores are for corpora that should not be ranked against each
other.

The page has two tabs, **All stores** and **Inside the index**, and two buttons,
**Adopt existing .semlith** and **New store**.

### All stores

A filter by name or path and a kind filter (**Code**, **Docs**, **Both**) narrow
the table. One row per store:

| Column | What it means |
|---|---|
| Store | The store's name and kind, and what it read. |
| State | One of the states below. |
| Files | Files indexed into this store. |
| Chunks | Pieces those files were cut into. One file is many chunks. |
| On disk | The store's size, and how much of it is reclaimable. |
| Saved | Tokens agents did not read because of this store, with coverage and tier. |
| Written | When the store was last written. |

| State | What it means |
|---|---|
| `indexing` | A run is going, with its percentage. |
| `paused` | A run is held by its Pause. |
| `N to review` | The scan held files back and a person has not decided on them. |
| `empty` | The store exists and holds nothing. |
| `not indexed` | Sources were added and no run has embedded them yet. |
| `fresh` | Up to date, watched. |

A store whose directory is gone, or whose database could not be read, says so on
its row; other stores are unaffected. A store the daemon did not start with still
appears: every load re-reads the registry and opens any registered directory the
daemon does not already have open, so `semlith index ~/work/new-project` run from
another terminal shows up here — and answers over MCP — without a restart.

Each row's menu has **Open**, **Add sources**, **Search it**, **Re-index**,
**Compact** and **Forget…**. Re-index reads every source again and skips
unchanged files by their hash. Compact rewrites the vectors without deleted
chunks and vacuums the database; searches keep working while it runs, and the
daemon also compacts an idle store on its own past the threshold set in
Settings. **Compact N** at the foot of the table does every store with something
to reclaim. Forget deletes the store's index, vectors and ledger rows — the files
it read stay exactly where they are — and asks first.

**Adopt existing .semlith** registers a store directory created elsewhere — by an
older version, or with `--store` — so the daemon opens it without re-embedding
anything.

### Inside the index

The figures about what was indexed, measured from the stores on every read
rather than estimated. Lines of code with comments and blanks separated out;
words indexed across code, prose, slides, sheets and notebooks; how many printed
pages that would be at 500 words a page; and the reading time at 250 words a
minute. Under them, the language mix by line, with how many of those languages
carry graph edges as well as search.

The fact cards follow. **What is in the prose** counts what documents hold, not
only how many there are: PDF pages, the slides of `.pptx` and `.odp` decks, the
non-empty cells of `.xlsx`, `.ods`, `.csv` and `.tsv` sheets, and notebook cells,
each beside its file count — `412 pages · 9 PDFs`. Each reader counts as it reads
the file, in the same pass that extracts its text. Word documents, e-books, mail,
web pages, images and code have no such unit and stay counted in files. A store
written before 0.34.0 has no counts until its next index pass reads each of those
files again for its count alone, with nothing re-embedded. **Shape of the code**
gives the average line, comment and blank lines, the deepest path and the longest
file. **Time in the corpus** gives the first read, the newest write, the median
retrieval time and the runs remembered. **What the graph holds** gives symbols,
settled edges, unresolved calls, and names defined twice or more.

**Chunks added per month** is a bar per month by when semlith read it.

**Graph health** splits the call edges of every open store into extracted,
resolved, ambiguous and unresolved — the four words are defined under
[Confidence](#confidence) — and lists the names defined most often, which are
the names whose edges become ambiguous.

Each store is measured on a connection of its own, so a cold measure of a very
large store, which reads every chunk's text, does not hold up the Stores list or
a search.

**This page reads live.** The table redraws when the stores, runs or events
domains move, by asking for the rows a reload would have had rather than by
patching what is on screen, and the scroll position is kept.

## A store's page

**What it is for.** Everything about one store, in five tabs: **Overview**,
**Files**, **Review**, **Runs** and **Settings**. The header carries **Search
it**, **Re-index** and **Add sources**. A name that is no longer a store — renamed
or forgotten — says so rather than showing an empty page.

### Overview

The store's figures: files and the readers that read them, chunks and the vector
dimension, the size on disk and what is reclaimable, and the tokens saved for
agents with coverage and tier. **Sources** lists each root with whether it is
watched and its last change, and marks one whose folder is gone. **Read as**
counts files per reader — semlith reads PDF, notebooks, HTML, Word, PowerPoint,
Excel, OpenDocument, EPUB, RTF, mail and images as well as plain text, and the
reader is chosen from the extension before the bytes are looked at — and the
languages. **What agents asked this store** lists its recent retrievals.

An empty store shows how to fill it instead: add folders, files or a URL, and
semlith scans them and asks before anything sensitive goes in.

### Files

Exactly what is in the store, so that "semlith has not read this" and "semlith
read it and it does not say that" stop looking the same from the outside.

A path or glob filter (`src/**`), a type filter and a language filter narrow the
server's query, not the drawn page, so the count above the table is about the
filter. **List** is a table of path, reader, language, lines, chunks and when it
was indexed. **Tree** is the same files as an explorer: each root a top-level
entry, a folder opened one level at a time and read from the daemon only when it
opens, so a tree of a hundred thousand files costs what the open folders hold.
What is on disk but not in the store sits among the indexed files greyed, with
why — binary, refused as a secret, a credential file, over the size cap, a
generated folder, `.gitignore`, `.semlithignore`, or not indexed yet. A folder
past 1 000 entries lists the first thousand and counts the rest. These are the
facts `semlith_files {tree: true}` gives an agent and `semlith files --tree`
prints.

Selected rows can be copied as paths, re-indexed, or forgotten. **Forget** drops
a file's chunks, vectors and graph edges from the store; the file on disk is
untouched, and a watched file comes back the next time it changes.

### Review

**Waiting for your decision** is the store's grey zone: files a scan held back,
each with its risk if indexed, why it was held and the masked evidence, refused
until somebody decides. The decisions and the selection work as in the
[wizard's Review step](#review): **Keep out**, **Redact & index** or **Index**,
singly or on a selection, each logged per file as yours, and never by an agent.
When nothing waits, the tab says new files that look sensitive will appear here
before they are indexed.

**Decisions** is the record: every decision about a file in this store, yours
and the rules', with its outcome — accepted, redacted and indexed, kept out, read
as an image, never indexed — and who made it. Filter by path or reason, by
outcome and by **Yours** or **Rules**. Any decision of yours can be undone, one
row at a time or **Undo selected**: an accepted file leaves the store and is held
again with today's reasons, so it comes back to this tab for a decision.

### Runs

**Running now** is the store's current run, with **Pause**, **Resume** and
**Stop…**, or **Take out of the queue** for one that has not started. **History**
is every finished run, kept across daemon restarts: what kind it was (first
index, re-index, watcher catch-up, compact), when, how long it took, what it
indexed, and, opened, its stage timings and its log.

A run lives in the daemon: its id, its paths, its status, its counters, its
clock, the last 500 lines of its log and, when it ends, its summary. Navigating
away, refreshing, or closing the tab for the length of a run changes nothing. A
run is only ever cut short in two ways: its own **Stop**, or `semlith start`
ending.

| Status | What it means |
|---|---|
| `review` | Scanned and held for **Start indexing**. It holds no slot and no writer. |
| `queued` | Submitted and not yet under way, with its place in line when it waits on the daemon-wide queue; without a place it is waiting for its own store's writer. |
| `running` | A writer has it. |
| `pausing` | **Pause** was pressed and the engine has not reached its next batch yet. |
| `paused` | Held by its **Pause**, at a batch boundary. |
| `held` | Admitted once, then held because *runs at once* was lowered below the number running. Held runs resume first, in submission order, keeping their progress. |
| `stopping` | A stop was asked for and what the run embedded is being undone. On a large corpus the undoing takes as long as the embedding did. |
| `done` | Finished. |
| `stopped` | Stopped, and undone. |
| `failed` | Ended on an error that was not a file's — the model, or the store. |

**The figures** are files read of the total, chunks, the rate and the time left.
The rate is chunks embedded over the last ten seconds of active time, with paused
and held time left out, and is also shown per lane — `Neural Engine 141/s · cache
722/s` — where `cache` is the vector cache handing back vectors it already held.
The time left is the daemon's estimate from bytes done against bytes total; it
reads `estimating…` until it has seen five seconds of embedding, and the page
counts down between polls and corrects to the daemon's figure on each one.

**The stages** say where the time went, in the words `semlith index --verbose`
uses: `walk 0.0 s, read+hash 0.2 s, parse+chunk 0.6 s, tokenize 2.5 s, embed
wait (ane) 125.3 s, write 11.8 s`. The parts sum to the wall time, so the largest
one is where a faster run has to come from.

**The log** prints one line per file with its outcome — `indexed`, `unchanged`,
`skipped`, `removed`, `refused`, `failed` — and a `skipped`, `refused` or `failed`
line carries its reason. A refusal names the rule, or the kind of credential and
its line, never the text that matched. A file that fails to read is one line, and
the run carries on. The page reads the log by a cursor, so two tabs on the same
run each see every line once.

**Pause, Stop and Take out of the queue.** **Pause** holds the run at its next
embedding batch; the writer stays the run's and **Resume** carries on from the
same chunk. **Stop** undoes the run: everything it embedded is rolled back, so
the store is left as it was before the run, and the confirmation says so before
it happens. Its checkbox also deletes the store, ticked by default for a store
that held nothing before the run and unticked for one that held content, with the
file count named. **Take out of the queue** removes a queued run, which has
embedded nothing and holds no writer, so there is nothing to undo.

Watcher catch-ups — the walk a watcher makes when a store opens, to pick up what
changed while nothing was watching — and bursts of more than 32 file events are
runs too, admitted through the same queue as a run you start. A smaller burst is
indexed straight away without a run, because a saved file has to reach search
within seconds.

**This tab reads live** and paints in place: a run holds a log and a scroll
position of its own, and redrawing would throw both away.

### Settings

- **Name.** Rename the store. Agents pick stores by name, and a rename reaches
  them on their next call.
- **Search leans to** — **Code**, **Docs** or **Neither**. It is applied as the
  default `prefer` for searches in this store, and it multiplies rather than
  filters.
- **Watch for changes** — whether the daemon re-indexes a file in this store the
  moment it is saved.
- **Record retrievals** — whether this store's queries go into the local ledger.
  When recording is paused for the whole machine on the Ledger page, this switch
  says so.
- **Respect .gitignore** — on every run and on the watcher.
- **Maintenance** — **Compact now**, with how much there is to reclaim, and
  **Re-index everything**.
- **Where it reads from** — each root. A root whose folder has moved offers
  **Re-point…**, which changes the directory the registry names and nothing else:
  no re-embedding, nothing rewritten. A store outside the store home that the
  registry has not been told to open offers **Trust**, after which plain
  `semlith` commands, `semlith mcp` included, find it without `--store`.
- **Forget this store** — deletes the index and its ledger rows; the files it
  read stay where they are. It asks first.

## Search

**What it is for.** Asking the corpus a question, and reading the answer the way
an agent is given it.

The box takes the question. Under it, the scope (**All stores** or one store),
**+ language** and **+ path** filter chips, and four modes:

| Mode | What it answers |
|---|---|
| **Ranked** | Spans ranked by meaning, words and the graph — what `semlith_search` returns. |
| **Brief** | Exactly what an agent's `semlith_brief` call returns. |
| **Exact** | Every matching line, like `grep -E` — what `semlith_search {exact: true}` returns. |
| **Pattern** | A tree-sitter query over one language — what `semlith_pattern` returns. |

Before a search the page offers four example questions — a question, an
identifier, a how-to and a behaviour — and this tab's recent searches.

### What fusion means here

Every ranked query runs two searches at once over the same corpus. The **vector**
search embeds your question and looks for chunks whose meaning is close to it,
which is how a question in plain words finds a paragraph that never uses those
words. The **keyword** search hands the bare terms to a full-text index, which is
how an exact identifier finds the line it is written on. A store that has indexed
images runs a third: your words go through a different model's text encoder and
are compared against pictures.

Neither list is the answer. Each is searched deeper than you asked for, and the
two are then **fused** by reciprocal rank — a chunk's place in each list
contributes, and a chunk both lists ranked well beats one that only a single list
liked. The code graph is not part of search: a function that is related to a match
but does not match itself is what Graph's neighbours and blast radius find.

Scores are fusion scores. They order one result set and mean nothing across two
of them. Above the results a line reads something like `identifier-shaped ·
keyword weighted ×2`. It comes from the answer's own fields: an identifier-shaped
query weights the keyword list twice and a question-shaped one leaves the two
level, and the classifier that decided that is the one that ranked the hits.

An identifier-shaped query that the ranking answers with nothing goes to
**Exact** by itself, because every line naming an identifier is the answer to
that question.

### The badges

Each hit carries one badge per list that found it:

| Badge | What it says |
|---|---|
| `definition` | This chunk defines the name you typed. It is above the ranking rather than in it. |
| `vector` | The embedding matched — this chunk means something close to what you asked. |
| `keyword` | The terms matched — your words are literally in this text. |
| `graph` | Reached from a neighbouring symbol — not itself a match; the graph walked to it from one. |
| `image` | The picture matched the words. |

`definition` appears only for an identifier-shaped query: the chunks that define
that name are lifted to the top before the fusion's order is applied to
everything else, so an agent that already knows a term never has to read past
its own definition to find it.

Only a `graph` hit also carries a confidence, because only a graph hit was
reached by following an edge, and how well that edge was settled is what a
reader needs to know before quoting it. The four values are defined under
[Confidence](#confidence).

### Freshness

Every hit carries a freshness mark: `fresh` when the file on disk still has the
size and modification time it had when it was indexed, `stale` otherwise. It is
one `stat` per distinct path, and conservative on purpose: a bare `touch` reads
as stale, because a false "check this" costs one re-read and a false "this is
current" costs a wrong quotation. Opening a stale hit says the file changed since
it was indexed and to read it before quoting it — the lines shown are what
semlith read, at the line numbers it read them at.

### The locator, and the two stages

A **locator** is an answer that says *where*, not *what*: one line per hit with
the line span, the enclosing symbol and its kind, which lists found it, the
freshness mark and the line of the chunk with most of your query's words in it.
Hits are grouped by file, in the order the ranking put the files in.

That is the first stage, and what an agent is given by default. The second stage
is the span itself, read only for the hit that was opened. A search that returned
eight whole chunks charges for eight chunks whether or not seven were wrong; eight
locators charge for eight lines, and only the right one is paid for in full. The
footer says what the answer cost: how many hits are shown, their tokens, and the
share of the budget. A store still embedding says so above the results, because
keyword and graph cover it in the meantime and vector does not yet.

### The dials

Each dial is the control an agent sets on the tool.

| Dial | What it changes |
|---|---|
| Scope | All stores, or one. |
| **Lean to** | `code`, `docs` or `either`. A store's own lean (its Settings tab) is the default when it is searched alone. |
| **Results** | `k`, how many hits to return, 1 to 50. 8 to begin with. |
| **Budget** | Tokens one answer may cost an agent: 500, 1000, 1500, 3000 or 6000, 1500 to begin with. |
| `language` | Only this language. |
| `path` | Only paths matching this glob. |

**Lean multiplies and never filters.** `code` leans the ranking towards source
files and `docs` towards prose, but neither excludes the other side.

**Exact is grep over the index.** The query runs as a regular expression (as
literal text when it is not a valid one) over the stored text of every indexed
file the scope and filters select, prose included. Each file is listed once with
its matching lines, each naming the definition it sits in, and opening a line
reads that definition. Past the limit the page says more lines match than are
listed.

**Pattern** needs a language, chosen beside the box, and captures every node the
query matches in that language's indexed files. The placeholder shows the shape
of a query — `(call_expression function: (identifier) @f)`.

### The detail panel

Opening a hit shows its span with a line-number gutter numbered from the span's
own first line — the coordinates `semlith read` and every error message print, so
a number read here can be typed back without arithmetic.

- **Read whole symbol** widens from the matched chunk to the whole definition it
  sits in, and **Show the span only** narrows back.
- **Copy path:lines** copies the locator.
- **Open in graph →** takes the symbol to Graph's Explore tab, selected.
- **One hop around** lists what calls the symbol and what it calls, each with its
  confidence badge — what an agent's brief would add.
- A hit that is an image shows the picture. The bytes are fetched through the API
  and handed to the element as a `data:` URL, because a browser attaches no header
  to an `<img src>` and the policy allows `img-src 'self' data:`.

### Brief

Brief shows exactly what `semlith_brief` hands an agent: the best span in full,
then one line per other hit, inside the budget, and the graph edges the brief
carries. **Copy as the agent sees it** copies it as plain text. When the budget
leaves no room for a span's text, the locator is still sent and the page says so.

## Graph

**What it is for.** Showing what the code says about itself: which definitions
exist, and which ones reach which.

**One store at a time.** The toolbar has a store picker, and the graph is never
drawn over every store at once. A store's graph is self-contained, and drawing
the graph of a very large store set at once could hold up every other route.

The page has three tabs: **Explore**, **Blast radius** and **Path & evidence**.

### What a node is

A node is a **definition** that tree-sitter found in a file. Tree-sitter is a
parser: it reads source code into a syntax tree, and every grammar ships a query
that names the tree's definitions. The node's **kind** — `function`, `class`,
`struct`, `method`, `interface` — is whatever that grammar's own tags query called
it. There is one language table and the extractor reads it.

**The module node.** Every file gets one synthetic node named after the file
itself, spanning line 1 to the last line, so a top-level definition has
something to belong to and a reference written outside any definition has
somewhere to hang from.

**The four navigational kinds.** A Markdown `heading`, a YAML, TOML or JSON
`key`, a CSS `selector` and an HTML `element` are symbols — `semlith symbol` finds
them and a locator names the heading a hit sits under — but they are never the
target of an edge. Nothing calls a heading, and letting configuration keys
resolve as targets once made 44 real function names ambiguous in this
repository's own store.

### What an edge is

An edge is something one piece of source said about another. There are six kinds.

| Kind | What the source said |
|---|---|
| `defines` | This file defines this name. The module node points at every top-level definition in it. |
| `contains` | This definition is written inside that one — a method inside its class. |
| `calls` | This code calls that name. |
| `imports` | This file imports that name or module. |
| `references` | This code mentions that name without calling it — a type in a signature, a constant read. |
| `aliases` | This name stands for that one: a re-export, or an import renamed on its way in. |

`defines` and `contains` are **structural**: they say where something sits, not
what depends on what, and every symbol is one hop from the file that defines it.
The other four are **dependency** edges, and a path or a blast radius walks only
those.

An edge also carries the line the reference sat on in the source. Where that line
is unknown, nothing is printed rather than the enclosing definition's line; a
wrong call site is worse than no call site.

### Confidence

Every edge carries one of four values saying how well its target was pinned down.

| Value | What it claims |
|---|---|
| `extracted` | The file said where the target came from — a structural edge, or a call whose name or module the file also imports. |
| `resolved` | Several definitions answered to the name, and the ranking left exactly one standing. |
| `inferred` | Matched by bare name alone, and nothing corroborated it. |
| `ambiguous` | Several definitions carry this name and nothing chose between them. |

`extracted` and `resolved` are answers. `inferred` is a hint worth checking. An
`ambiguous` edge is not an answer at all: the store holds several definitions of
that name and cannot say which this edge meant, and it carries the count.

**Two are stored and two are recomputed.** `extracted` and `inferred` are written
at extraction time. `resolved` and `ambiguous` are computed on every read, because
resolution is a statement about the whole corpus: index another file that defines
the same name, and a stored `resolved` would go on claiming something the new
file falsified. The ranking prefers a definition in the same file, then one in the
file the source's own hint names, then one in a file the source imports, then the
case where the corpus holds exactly one. A caller row shows the stored value,
because an inbound edge was found by id rather than by name.

**An ambiguous edge is drawn dashed and amber, and never crossed** by a walk that
prefers verified edges. Drawing it like the others would offer a line the reader
could follow to a conclusion the store never reached. Refusing ambiguous edges is
necessary and not sufficient: a chain may only leave from the definition it
arrived at, which is what a **seam** marks below.

### Explore

**Jump to a symbol** centres the canvas on a name. The chips beside it switch
**calls**, **imports**, **inferred** and **ambiguous** edges off and on; they
filter what is drawn, at once, without asking the server again. An edge of any
other kind is always drawn, in a muted ink. **Fit** frames the whole drawing.

The canvas is a force layout: nodes push each other apart, edges pull their ends
together, and the layout is solved before the first painted frame. Drag a node to
move it; a drag that goes nowhere is a click, which selects it. Hovering a node
lifts it and its edges. With reduced motion the layout is solved once and is then
still. A view is capped at 63 symbols, because a force layout is readable at
dozens of nodes and a hairball at hundreds; the caption reads `N of M symbols · N
edges`, and the legend gives the four confidence inks and what a selected node
looks like. Only edges with both ends drawn are drawn.

**What it opens on** is not the busiest symbol: a codebase's busiest name is its
most reused one — `new`, `len` — matched by spelling to dozens of unrelated
callers. It opens on a name defined once in the store, then the one with the most
resolved edges, then raw degree.

**The selected-node panel** gives the symbol's name, file and line range, kind and
store, and two lists: **Called by** and **Calls**, each row carrying its
confidence badge and edge kind, and each clickable to move there. An ambiguous row
reads `name · N definitions` and expands to list every candidate with its file and
line, so the choice is made by a person looking at the code. **Show unresolved**
lists the names this symbol calls that no open store defines — a standard-library
call, an unindexed dependency — which are hidden by default and counted rather
than dropped. **Blast radius** and **Path from here** take the symbol to the
other two tabs.

**Subsystems** groups the store's symbols into communities over settled calls
and imports edges, with no model involved, and names each one's hubs.

### Blast radius

Reading the graph backwards from one name: you type a symbol and the tab lists
everything that reaches it, so *what would notice if I changed this* is answered
before the edit rather than by the test run after it. It is the same function
`semlith impact` and `semlith_impact` call.

**Controls.** The symbol's exact name; **Hops**, 1 to 6, 3 to begin with;
**Verified edges only**, on, which does not cross a name several definitions
answer to; and **Reach**.

**The answer opens with a sentence** — `N definitions in N files reach <name>
within N hops` — composed by the same function that prints it in the terminal,
then three figures: **Reached**, **Files**, and **Inferred**, how many rows were
reached across an `inferred` or `ambiguous` edge. The third is the part of the
answer that is not settled, stated beside the part that is.

**The table** lists each reached definition with where it is, the hop it sits at
(the fewest hops it takes) and the edge that reached it with its badge. Filter it
by name or file, by hop, and by edge (all, resolved, inferred), and page through
it. When the cap cut the answer, the page says how many more there are. Beside it,
a small reverse graph draws the answer with the subject selected; it says how many
it left undrawn, every one of which is in the table. **Files to look at** groups
the answer by file, nearest first, and **Copy list** copies it.

### Path & evidence

The forward question — is there a chain from one symbol to another, and what is it
made of — and the same chain written as something you can paste into a review.
Until 0.35.0 these were two cards, a path finder and a trace; they are one tab
because they are one answer.

**Controls.** **From**, **to**, **Swap**, **Prefer verified** (list extracted and
resolved chains first) and **Strict** (refuse to cross a name with several
definitions), and **Find the path**. Strict wins over asking for everything, as
it does on the command line.

**The answer** is one sentence naming what the chain is worth: connected in N
hops by settled edges, or not connected by resolved edges with how the nearest
chain got there. When any hop was matched by bare name or crosses a seam, a banner
says *A hypothesis, not a finding.* and **Show the inferred chain** walks again
without the refusal.

**The chain** is one row per hop: the hop's start and end as `name @ path:line`,
the edge kind and its badge. Where a hop arrives at one definition and the next
leaves from another, a **seam** row is drawn between them — every hop true and the
chain false is the failure it marks. Under the hops is a counted line: `N hops · N
extracted · N resolved · N inferred · N ambiguous`.

**Supporting lines** is one block per hop: the line of source the edge was written
on, as `path:line` and the code, read from the store rather than from the file on
disk — the same rule `semlith read` follows, so the quotation is what semlith
read. A hop with no recorded call site says so.

**Copy as evidence** puts plain text on the clipboard, byte for byte what
`semlith trace --evidence` prints: a first line `<from> → <to>`; `answer: <the
sentence>`; one numbered line per hop, `N. <from> @ <path>:<line> -> <to> @
<path>:<line>  [<support class>]`, with any seam appended to the hop it follows;
then, where there are any, `supporting lines` and one line per hop with
`[supporting fact]` or `[candidate]`. No scores, no prose, and no text the store
does not hold.

## Agents

**What it is for.** Connecting agents to this daemon, and seeing which ones are
connected.

**The endpoint** is at the top: its URL with **Copy**, and a switch. Off drops
the `/mcp` route and nothing else — the stores stay open, the watcher keeps
running, the portal keeps working — and a closed endpoint answers with a clear
refusal whether or not the client still holds a key.

The page has four tabs.

### Connected

The clients talking to this daemon now: the client's own name from the MCP
handshake, its state, its version, its transport, how many queries it has run and
when it last asked. Three windows of one client are one row with its sessions
counted. **What the tool list costs** is the thing agents pay for and nobody
measures: the size of the tool list in tokens and bytes, read once per session
before the agent has asked anything, measured from what this daemon serves now.
With no client connected the tab says how to get the first one: register it, then
restart it.

### Add a client

The clients semlith documents, grouped **Terminal**, **Editors** and **Desktop**,
each marked found or not found on this machine and registered or not. **Register**
and **Unregister** act on one client: semlith runs the client's own command where
it has one, and otherwise writes the client's configuration file after backing it
up beside itself. **Register N** at the top registers every found client at once.
For a client somewhere semlith cannot write, the tab shows the **Config file**
form and the **Terminal** form to copy. The HTTP form names
`${SEMLITH_AGENT_KEY}` rather than the key; the subprocess form, `semlith mcp`,
reads the key file itself.

These stanzas are parsed out of `docs/clients.md`, so the page shows the text a
test runs rather than a second copy.

### Tools

**What an agent can call**: every tool the endpoint serves, the question each one
answers — read from the tool's own definition — and its typical answer size in
tokens, which is the median of this machine's own answers from that tool once it
has five, and an estimate until then.

### Health

**Can each client reach semlith?** is the report `semlith doctor` prints, from
the same function: one row per client, whether semlith is registered and at what
scope, the state of its skill, steering hook (with its mode) and rule file, and
what to run to fix it. Three states are kept apart: *not installed* is a client
whose program is not on this machine, which is not a fault; *found, not
registered* would register if asked; and *switched off in N folders* is a
registration a directory override turns off, named by its directories, because
"here" for a daemon a service manager started is nobody's working directory.
**Check again** re-reads it. Under the table are the machine checks: up to six
of the [privacy rules](#rules-the-binary-enforces) that carry a reading, each
with what the daemon found.

Nothing on this page runs a client's program to answer: registration state is
read out of each client's own configuration file.

**This page reads live.** The clients domain moves when one connects, disconnects
or runs a query.

## Ledger

**What it is for.** What your agents actually retrieved, recorded locally, so the
saving semlith claims has a denominator under it.

Every surface records through one module — the stdio MCP server, the daemon's
`/mcp` route, the CLI and the portal — so a search from a terminal and the same
search from an agent are counted the same way. The rows live in each store's own
database, in a `retrievals` table, and never leave the machine.

**Recording** is a switch at the top. Off pauses recording at runtime: nothing is
written while it is off, and turning it back on continues the hash chain from the
row before the pause. `semlith start --no-ledger` and `SEMLITH_LEDGER=0` still
turn recording off for a session or a machine; a daemon started that way says so
on the switch and has to be restarted without the flag to record. A store's own
**Record retrievals** switch is on its Settings tab.

**Rows are hash-chained** — each carries the hash of the one before it — so an
edited or deleted row can be found. When the chain does not verify, a banner says
so and offers **Re-verify**. A repair appends one note row saying where the chain
broke and verifies from there; no recorded row is edited or removed, so the break
stays visible. `semlith ledger --verify` is the same walk.

**The figures.** A saving never appears without its coverage and its tier.

| Figure | What it counts |
|---|---|
| Queries recorded | Retrievals recorded, and how many distinct clients ran them. |
| Fewer tokens | Whole-file tokens over excerpt tokens, with coverage and tier. |
| Net tokens not read | Whole-file less excerpt, over rows with a hit. |
| Zero-hit | The share of queries the corpus could not answer, with the count. |

- **Coverage** is the share of recorded retrievals the saving is computed over. A
  retrieval that found nothing saved nothing and is counted in the denominator.
- **Tier** is `measured` when every credited row was counted by the store's own
  tokenizer, `modelled` when any was estimated at four characters per token. A
  mixed ledger reports `modelled`.
- **Refunds** are files an agent read whole after all — a measurement on a client
  carrying the steering hook, a floor everywhere else. A refund is an agent that
  did not reach for semlith; a zero hit is semlith that did not reach the answer;
  they are two figures because they call for opposite things.

The ratio does not count a failure as a success, does not add rows counted two
ways, and does not pretend the whole-file side was tokenized: it is the files'
size on disk at four characters per token.

**What agents read, against reading whole files** draws the two token totals as
bars. **By client** draws one bar per client, from the MCP handshake's name;
pressing one filters the tables below to that client.

**Three tabs.** **Sessions** is one row per agent conversation: its client, model,
reads, net tokens, and **Saved**, those net tokens at the input price of the model
that session ran on. **Retrievals** is one row per query: when, the query, its
hits, what was sent, and the tier; filter by store, client, tier and **Zero-hit
only**. **Session replay** reads this machine's agent session logs to show what an
agent did after each answer — whether the answer sufficed, missed, or was followed
by a whole-file read (a refund). Both tables sort and page, and export to
Markdown, CSV and JSON.

**Usage from client logs** is a switch on the tables' card. On, each retrieval
gains the model that made the call and its cost, read from that client's own
session log on this machine, matched to the tool call by tool, time and arguments;
the Sessions tab's model column comes from it. A client whose log semlith cannot
read says `not recorded`, and nothing is estimated for it. Cost is the client's
own figure where it records one, and otherwise the tokens at the price in the
table built into the binary (a models.dev snapshot), which Reports and Settings ›
About can update. `semlith ledger --usage on|off` is the same switch.

**Session replay is on by default** from 0.35.0, for a settings file that never
set it; a user who turned it off stays off. Its switch is on the Privacy page.
It reads local logs only, and uploads nothing.

**Same thing in a terminal** gives the commands: `semlith ledger --last 20`, the
verify walk, `--no-ledger`, and where the table lives.

**This page reads live.** A row lands as an agent retrieves it.

## Reports

**What it is for.** Turning what this machine already holds — the ledger, the
index and the graph — into a file somebody who will never open the portal can
read. Generated here, saved through the browser, never uploaded. The same
generator answers `semlith report` and `semlith_report`, so a file from this page
and one from the command line are the same bytes.

**The five reports**, one chosen at a time:

| Report | What it is | Reader |
|---|---|---|
| Retrieval savings | Tokens agents did not read, counted from the ledger. | for whoever approves the spend |
| AI access audit | Which agent read which file, when — from a hash-chained record. | for security review and AI-use policy |
| Change brief | Blast radius for what changed, as a note to paste in the PR. | for the reviewer, before the merge |
| Index health | Stale files, skipped formats and graph coverage. | for whoever owns the store |
| Knowledge gaps | Questions the corpus could not answer well — a docs to-do list. | for whoever writes the docs |

Choosing one generates it. There is no Generate button: the selection is the
request.

**Window** is 24 hours, 7 days, 30 days or Quarter. Only the access audit and the
change brief have a date to narrow by; on the other three the chips are disabled
and the page says the report counts the whole history. **Format** is Markdown,
CSV, JSON, HTML or PDF, rendered by the server — the PDF is typeset by a writer
inside the binary from the same blocks, not printed from the HTML. **Stores** is
all stores or a selection. **Hash the query text** replaces every query with a
digest, wherever a query reaches the document, keeping who, when and which file.
**Attach the excerpts** adds the exact lines each agent was shown.

**The preview** is the report itself, with its file name and **Copy**, and **Save
to disk**, which is the browser's own save: no route writes a report file on the
page's behalf. Its footer says what it is — rows, size, format — and that nothing
was uploaded.

**The savings, line by line** prices the savings arithmetic at a model you choose
from the price table, with the rate and the table's source and date beside it.
**Update prices** fetches the table from models.dev once, asking first; it is the
only thing on this page that reaches the network, and only when pressed.

**Without the browser** shows the `semlith report` command with the flags the
controls set, and the MCP call that returns the same document.

### Schedules

A schedule is a report the daemon writes on its own, on a cadence, into a
directory you name. **Schedule…** on the preview asks for the cadence and the
directory; the card lists each schedule with when it last ran and next will, and
**Remove schedule**.

It belongs to the daemon, not to this page: schedules live in
`~/.semlith/schedules.json`, beside `registry.json` rather than inside it, so a
truncated schedules file costs schedules and not every store. With no daemon
running a schedule is recorded and does not fire until one is up. The daemon
checks the directory and never creates it, because creating it would answer an
unmounted volume by writing into the empty mount point. The file is replaced in
place on each run, so a tracked folder shows a readable diff. Two schedules never
generate at once, and one due while its store is indexing waits.

```sh
semlith schedule list
semlith schedule add savings --every 604800 --to ~/Reports --format pdf
semlith schedule remove s1
semlith schedule set s1 off
```

**This page does not read live.** A report is a reading of a moment, and one that
redrew itself under a reader would be a different document from the one they
were quoting.

## Privacy

**What it is for.** Checking the claim that nothing leaves this machine, rather
than believing it.

**The verdict** at the top counts outbound connections since the daemon started,
from a counter the daemon keeps on every outbound request: `Nothing has left this
machine`, or how many did and when. Beside it, **Airgap**: on, any outbound
request exits instead and names what it would have fetched. It is a runtime
switch; when the daemon was started with `--airgap` or `SEMLITH_AIRGAP`, the
switch says so and cannot be turned off here.

**The facts strip**: the bind address (loopback only, no flag to change it), no
CORS header, a foreign `Host` gets 400, the page compiled in, no telemetry and no
update check of its own, the ledger in each store's own database, the model cache
and whether it holds anything, and no cloud command.

**Verify it yourself** is five steps, each copyable and each ticked off as you
go: ask the operating system what this process has open (only loopback should
appear); watch every interface but loopback while you search (nothing should
appear); arm the refusal with airgap; pull the cable — the portal loads and
searches with no network; and look at the ledger for what it is, one local table.

**Everything semlith ever fetches** lists each thing this binary can download,
with its source, its size, when it happens and whether it is already cached: the
embedding model, once, on the first run; the accelerator packs, only after their
lane is turned on; a URL you add to a store, one https request, only when you
press Fetch; and the release check and the price table, github.com and
models.dev, only when you press the button in Settings or Reports. Airgap refuses
every one of them unless the files are already in the model cache.

**What the stores already hold** is the rules pointed backwards. The rules say
what semlith would refuse today; **Check now** runs both halves of that decision —
the deny-list against each file's name, and the credential shapes against the text
the store keeps — over every file every open store holds, to find what was taken
in before a rule widened. It is the same `Semlith::scan` `semlith scan` calls.
Each row is the store, the path and why it would be refused, never the text that
matched. **Forget** drops a file's chunks; the file on disk is untouched, and the
check runs again from scratch afterwards. There is no MCP tool for it: an agent is
not the party that decides what a store may hold.

**Session replay** is the switch for the Ledger's replay tab. It reads this
machine's agent session logs to confirm what an agent did after an answer, and
keeps nothing but that. It is on by default from 0.35.0 where the setting was
never written.

### Rules the binary enforces

Each row is a rule the binary enforces, and under it what *this daemon found when
it checked* — a path, a file mode, a count. A page that states a policy and the
reading behind it is something you can disagree with.

A failing row carries the command that repairs it and, where the daemon can do it
safely, a **Repair** button. Safely is a property of the repair: it narrows access
rather than widening it, it is idempotent, it touches only a path semlith owns,
and it is confirmed by re-running that rule's own check. What it changed and what
it was before are both reported — `0700, was 0755`. The button posts to the
engine `semlith doctor --fix` calls.

The rules were rewritten for 0.35.0's changes: a person may decide on refused
files in bulk and an agent still may not; ledger recording can be paused at
runtime; session replay is on by default.

**This page reads live** on the privacy domain: a repair, a `chmod` in a terminal
or a `semlith doctor --fix` in another window turns the row without a reload.

## Settings

**What it is for.** How hard this machine works, who may reach the endpoint, and
what is installed. Four sections, chosen from the list on the left.

### Performance

**This machine** reads logical cores, total memory and memory free now, on every
request, and **Reset to what it suggests** puts every limit back.

**Limits**, each a stepper with the value this machine would pick beside it:

| Limit | What it is |
|---|---|
| Runs at once | How many stores may index at the same time. Derived from memory free less a 2 GiB reserve over what one run peaks at, capped by the cores with one kept free. |
| Threads per run | Embedding threads, with one core kept free for you. |
| Memory per store | Vectors held in memory per open store. Lower it if other apps feel slow. |
| CPU cap | The most of this machine's CPU semlith's own process may use while it indexes, 0-100 % of every core, with what it is using now beside it. 100 is no cap; 0 pauses CPU work and the run card says "paused by the CPU cap". Its stepper moves in steps of 5 (from 0.37.0-rc.7; 10 before), the same steps as the Semlith Cloud operator panel; a value set off that grid snaps onto it. From 0.37.0-rc.6. |
| Compact past | An idle store more reclaimable than this percentage is compacted on its own; 0 turns it off. |
| Keep retired definitions | How long a compaction keeps the history of a symbol that was renamed or deleted; 0 keeps everything. |
| Vector cache | Vectors kept so a chunk met again is not re-embedded, with how much of it is used and the hit rate. 0 turns it off. |

Each has a ceiling the route enforces as well as the page: the core count for the
two parallelism settings, memory free less the reserve for the memory budget. A
saved value applies at once to runs already going. A value an environment variable
sets — `SEMLITH_INDEX_PARALLEL`, `SEMLITH_EMBED_THREADS`, `SEMLITH_INDEX_MEMORY`,
`SEMLITH_VECTOR_CACHE_MB`, `SEMLITH_CPU_CAP` — is shown and cannot be changed here. What is not from
the environment is saved in `~/.semlith/settings.json`.

**Where embedding runs** has one switch per lane — the CPU, the Neural Engine, the
GPU, CUDA, TensorRT for RTX, OpenVINO and llama.cpp — each with its device and
variant, its state (`active`, `idle`, `starting`, `compiling` or `downloading`
with a percentage and the time left, `unavailable` or `failed` with the reason)
and its live share of the work. CUDA, TensorRT for RTX, OpenVINO and llama.cpp are
marked experimental: built and checked without their hardware. Turning on a lane
whose pack is not installed asks first, naming the size. It is the same control as
`semlith accel`.

From 0.37.0-rc.6 the CPU lane is always on: its switch is locked, and the CPU cap
above is how to hold it back. Every other switch draws the choice that was saved;
a lane set by `SEMLITH_ACCEL` says so and cannot be switched. Which lanes can run
is read from this machine's hardware once per daemon: no GPU the lane can use
(Metal, or a Vulkan driver from the GPU's maker) makes the GPU and llama.cpp lanes
unavailable, no NVIDIA card and driver makes CUDA and TensorRT unavailable, and a
CPU that is not Intel's makes OpenVINO unavailable. An unavailable or failed lane
can be switched off but not on, and a refusal says why.

**Check each lane against the CPU** is `semlith doctor --gpu`: each lane embeds
32 fixed chunks and compares them with CPU vectors committed to the repository,
and the row shows the lowest cosine and the rate.

### Agent access

**Agent key** — the credential a client carries over HTTP. It is shown masked;
**Reveal** fetches the real value and pressing it again puts it away. **Rotate**
mints a new one; the previous key keeps working for a grace window, and the
section says how long. A registered client that launches `semlith mcp` reads the
key file itself, so a rotation reconfigures nothing for it.

**Session token** — the portal's own, shown truncated, with **Rotate**. This tab
moves to the new token at once; any other open tab has to be reopened from the
URL `semlith start` prints.

**Start at login** — a switch that installs or removes the daemon as a login
service (a launchd user agent, a systemd user unit, a Windows logon task), with
its mechanism, the definition file it wrote, and when it last started. Removing it
asks first: after a reboot, agents cannot reach semlith until somebody runs
`semlith start`.

See [The two credentials](#the-two-credentials) for what each rotation does.

### Cloud

Semlith Cloud, from this machine's side. Two states.

**Not signed in.** What the cloud adds — one URL for cloud agents, an org's stores
beside the local ones, a pull-request check — and the two commands to copy,
`semlith cloud login <org>` and `semlith cloud connect <org>`. No price. Nothing
on this page or anywhere in the daemon reaches any host in this state: it reads
the absence of `~/.semlith/cloud.json` and nothing else.

**Signed in.** Per org, a header with its name, plan, the token's 13-character
prefix (never the token) and **Disconnect**, which takes the org's remote stores
off this machine and forgets its token here. Under it a reachability pill —
`connected`, `unreachable` or `refused` — with the sentence that says why, read
from `/api/cloud/status` when the section opens and on **Check again**; that is
the one request the page makes to the host. Then a card per remote store: its
state, files and chunks, each source with its revision, how far behind it is and
a freshness dot, and a folder field with **Push** (`semlith cloud push`). An org
with no store connected offers **Connect its stores** (`semlith cloud connect`).

**Ledger sync** lists the local stores with a switch each (`semlith cloud sync
<store> on|off`), off by default, and the last send. **What leaves this machine**
lists every kind of request and what it carries. **Reports and replay** fetches
one of the cloud's reports as text (`semlith cloud report`) and sends one
transcript you pick (`semlith cloud replay`), after a confirmation.

Remote stores also appear on the Stores page, in their own card, and in Search's
store picker, each with its `remote · <org>` badge; a search naming one, or none,
merges their hits into the list by score with the badge, revision and lag on each
row, and a host that cannot be asked adds one line saying so. The Ledger page
reads `syncing N of M stores · last sent …`, the Privacy page's Cloud card and
download list say what goes to which host, and the sidebar's daemon card ends
`· cloud: <org>`.

### About

What this binary is: the version and store format, the binary's path, size and
target, the address it is bound to, the store home, the source licence, the uptime and process id, and the MCP revisions it speaks. The
version shown here, in the sidebar and anywhere else is the running binary's,
read from the daemon.

- **Updates** — semlith never checks on its own. **Check for updates** asks
  github.com once; when a newer release exists, **Install <version>** downloads it
  and replaces this binary, asking first, and the running daemon keeps the old one
  until it is restarted. It is `semlith upgrade`.
- **Prices** — the price table behind the ledger's cost and every savings figure:
  how many models, from which source and date. **Update prices** is `semlith
  prices update`.
- **First-run screen** — opens Welcome again. Nothing is deleted.

**Languages** is every name `--lang` accepts, marked where the code graph is
extracted as well. The search filter and the graph read the same table, so the
two cannot disagree about what a language is.

**The embedding models a store can be built with are not listed.** A table of
forty-eight rows, of which a machine has fetched one, is a catalogue rather than a
fact about this binary. `semlith models` prints the full list and `GET
/api/models` still answers. This is the one capability with no view in the
portal, argued in `tests/portal.rs` and recorded in `docs/compatibility.md`.

## Concepts

### Store, root, file, chunk

A **store** is a corpus: one vector index and one SQLite database that must agree.
A **root** is a directory on disk that a store covers; one store can have several,
because a corpus is not a folder. A **file** is one file semlith read. A **chunk**
is a piece of that file — the unit that is embedded, searched and returned.
Chunks overlap slightly, which is why a span is stitched together by line number
rather than by concatenating them.

### Symbol and edge

A **symbol** is a definition tree-sitter found, plus the synthetic module node per
file. An **edge** is something one piece of source said about another, of one of
six kinds. An edge belongs to the file its source is in and dies with it; its
target is a *name*, resolved when a query runs. That asymmetry is what makes the
graph safe to re-index: symbol ids are reissued every time a file is re-extracted,
so storing an id as a target would mean re-indexing one file silently orphaned
every edge pointing into it from another. Resolving by name instead makes an edge
exactly as current as both of its ends, and lets an edge to something that was
never indexed — a standard-library call — still be recorded.

There is no `semlith graph build`. Extraction is spliced into the same pass that
re-embeds a changed file, so there is no artifact that can be stale.

### Locator

A **locator** is an answer that says where something is rather than what it says:
a path, a line span, the enclosing symbol and its kind, which lists found it, a
freshness flag and one line of text. It is the first stage of a two-stage flow —
locate, then read — and it is what keeps an answer cheap.

### Freshness

A hit is `fresh` when the file on disk still has the size and modification time it
had when it was indexed, and `stale` otherwise. A stale span is still served, with
a note saying the lines are what was read then — because the honest answer is not
"this is current" and not "there is nothing here", it is "this is what I read, and
the file has moved under it since."

### The two credentials

There are two, and what separates them is their reach. Neither can do the other's
job.

**The session token** is the portal's. It is generated when the daemon starts,
lives in memory for that run, and opens the page and every `/api/*` route. It
reaches the browser once, in the printed URL, and travels from then on in a
`Semlith-Token` header. It is shown truncated everywhere, and the only response
that carries it in full is the one that rotates it. **Rotating it takes effect
immediately**: the page that pressed Rotate is handed the new value; every other
holder of the old token stops. Nothing needs reconfiguring, because no client
stanza carries it.

**The agent key** is the agents'. It is a `sml_` prefix followed by 64 hex
characters from the operating system's random source, written to `agent.key` under
the store home, created on first read and returned unchanged forever after. It
opens `/mcp` and nothing else; no route outside `/mcp` may accept one, and a test
asserts it. It is persisted because a client's configuration is written once and
has to stay valid across restarts, upgrades and token rotations, and it is never
reminted implicitly. The page never receives it for merely being open: it arrives
only in the response to **Reveal**.

**Rotating it does not cut off an agent mid-session.** The new key takes effect at
once, and the previous key keeps being accepted for a grace window that ends after
fifteen minutes or when the daemon exits, whichever is first — long enough for a
call in flight to complete, short enough that a key rotated because it leaked does
not stay valid for as long as the machine is up. Configuration files on this
machine that carried the old key are rewritten with the new one, and the page
names them; anything configured elsewhere needs the new stanza before the window
closes.

Settings › Agent access holds both, and each control says which one it is
rotating.
