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
step and no npm; the page loads with the cable unplugged.

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
Every response carries `Content-Security-Policy: default-src 'self'`, and no CORS
header is emitted anywhere.

### The first run

A daemon started on a machine with nothing indexed has no navigation to show, so
it shows a single screen instead: **No stores yet**. It takes a folder path, or
opens a directory picker, and the button carries what you typed into the Inside
the index page rather than dropping it. There is a third button that skips to
About, and a
copyable `semlith index ~/Documents/work` for doing the same thing from a
terminal.

Once one folder is indexed the navigation appears and the first-run screen is not
shown again.

### The navigation

Thirteen pages in two groups, in this order, and every section below is in the
same order:

| Group | Pages |
|---|---|
| Workspace | [Stores](#stores), [Files](#files), [Inside the index](#inside-the-index) |
| Explore | [Search](#search), [Graph](#graph), [Impact](#impact) |
| Operate | [Retrieval ledger](#retrieval-ledger), [Reports](#reports), [Agents](#agents), [Cloud](#cloud), [Privacy](#privacy) |
| Machine | [Doctor](#doctor), [About](#about) |

Four groups rather than one long list: what you have indexed, what you can ask
of it, what the daemon is doing, and what this machine is running.

The address bar carries the page as a fragment — `#search`, `#graph` — so a
particular page can be bookmarked or reloaded. On a narrow screen the navigation
is a drawer that closes when it takes you somewhere.

Under the pages is a count — `2 indexing`, or `2 indexing · 3 queued` — on every
page rather than on Inside the index alone, because a run that is only visible from
the page that started it is a run that happens out of sight. It is hidden when
nothing is running.

**Most of the portal reads live.** The daemon keeps one counter per data domain —
stores, runs, clients, ledger, events, privacy — and bumps it in the one function
that writes that domain. Every page polls those six integers once a second from
`GET /api/changes` and refetches only the domains that moved, so a quiet daemon
with a tab open costs one small request a second and nothing else. It is a poll
rather than a stream because a stream would hold one of the daemon's eight HTTP
workers for as long as the tab is open, and eight tabs would then be a portal
that cannot answer. A tab in the background stops asking, and catches up in one
pass when it comes back. Which pages read live is said on each of them below;
Search, Read, Graph, Impact and Reports deliberately do not, because each is the
answer to a question somebody asked and redrawing it under them would be
answering a different one. Cloud has no route behind it and so has nothing to
follow.

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
answer. Indexing a second folder into an existing store adds a root to its
registry entry. Separate stores are for corpora that should not be ranked against
each other.

**The summary strip** totals every store: how many stores exist and how many are
being watched, how many files and how many distinct formats, how many chunks and
the dimension of the vectors holding them, how many lines and how many readers
were needed, and the bytes on disk.

**The table**, one row per store:

| Column | What it means |
|---|---|
| Store | The store's name, its directory, and the embedding model and dimension it was built with. The model is per store because two stores built with two models hold vectors that are not comparable. |
| Roots | The directories this store covers. A root the registry still lists and the disk no longer has is struck through and carries a **Re-point** button. |
| Files | Files indexed into this store. |
| Chunks | Pieces those files were cut into. One file is many chunks. |
| Last write | `not watching` if no watcher holds this store; otherwise when the last write landed, or `never`. |

Two things can appear under a store's name. **not trusted** means the store sits
outside the store home and the registry has not agreed to open it without being
told to; the button beside it records that agreement, after which plain `semlith`
commands find it without `--store`. **mode 755** or similar means the directory is
readable by other users of this machine; the next open narrows it to `700`, and
the row says so rather than silently fixing it.

**A store this daemon did not start with still appears.** Every load of this
page re-reads the registry and opens any registered directory the daemon does
not already have open, so `semlith index ~/work/new-project` run from another
terminal while the daemon is up shows up here — and answers over MCP — without a
restart. A directory the daemon could not open is listed all the same, with zero
counts and a `not opened` pill whose hover says why, because a store you can see
in `semlith stats` and not on this page reads as the portal having lost it. There are two reasons it
can happen and they are different things to do about: another process holds that
store's write lock, in which case it is being written and the next load of this
page picks it up by itself; or the registry names a directory the disk no longer
has, in which case nothing is coming. `/api/stores` carries the reason on the
row as `unopened`.

**Re-point** changes which directory a store's root names. It is a registry edit
and nothing more: no re-embedding, nothing rewritten. Use it when a project has
moved.

**Adopt existing .semlith** registers a store directory that was created
elsewhere — by an older version, or with `--store` — so the daemon opens it on
its next start.

**Delete store…** is behind the row's menu rather than beside the ordinary
actions, so a destructive click is not one pixel away from a harmless one.

The **Watcher** card is a live feed of what has changed on disk since the daemon
started, most recent first. The card beside it summarises connected agents; the
Agents page has the whole of it.

**This page reads live.** The rows and the feed redraw when the stores or events
domains move, so a file saved in a watched root, a store another process created,
and a run finishing all appear without a reload. It redraws by asking for the
rows it would have had on a reload rather than by patching what is on screen — a
second renderer is how a table ends up disagreeing with a reload of itself — and
the scroll position is kept.

## Files

**What it is for.** Exactly what is in the index, so that "semlith has not read
this" and "semlith read it and it does not say that" stop looking the same from
the outside.

**Controls.** A path glob filter (`src/**`) and a row of extension chips for the
common formats. Both narrow the server's query, not the drawn page, so the
totals underneath are about the filter and not about the ten rows on screen. The
pill in the heading reads `N files · N formats · N stores` for whatever the
filter selected.

**The columns:**

| Column | What it means |
|---|---|
| Path | The file's path on disk, one line, with the whole of it on hover. |
| Store | Which store holds it. |
| Read as | Which reader turned this file into text. |
| Language | The language the search filter would call it, or `—`. |
| Lines | Lines of text extracted. An image shows `—`. |
| Chunks | Pieces this file was cut into. |
| Indexed | When it was last read. |

**"Read as"** is the one column worth explaining. Semlith does not only index
plain text: it has readers for PDF, Jupyter notebooks, HTML, Word, PowerPoint,
Excel, OpenDocument, EPUB, RTF and mail, and it embeds images through a different
model entirely. The reader is chosen from the file's extension before its bytes
are looked at — `.docx` and its relatives are ZIP archives, and a binary check
would reject every one of them. The column names which reader ran, so you can see
that a PDF was read as `pdf` and not silently skipped, and that a `.png` says
`image` rather than showing a zero that would read as a parse failure.

**Forget** drops a file's chunks and vectors from its store. The file on disk is
untouched, and indexing the folder again brings it back. Rows can be ticked for a
bulk forget; the header tick selects every row on the current page rather than
every row behind the filter, because ten thousand files behind one tick is a
mistake nobody meant to make. A selection made across two stores becomes two
writes, because a write names the store it is for.

## Search

**What it is for.** Asking the corpus a question, and reading the answer the way
an agent is given it.

### What fusion means here

Every query runs two searches at once over the same corpus. The **vector** search
embeds your question and looks for chunks whose meaning is close to it, which is
how a question in plain words finds a paragraph that never uses those words. The
**keyword** search hands the bare terms to a full-text index, which is how an
exact identifier finds the line it is written on. A store that has indexed images
runs a third: your words go through a different model's text encoder and are
compared against pictures.

Neither list is the answer. Each is searched deeper than you asked for, and the
two are then **fused** by reciprocal rank — a chunk's place in each list
contributes, and a chunk both lists ranked well beats one that only a single list
liked. A third list is then built by walking the code graph outward from what the
first two found, so a function that is not itself a match but is called by one can
still arrive.

Scores on the page are fusion scores. They order one result set and mean nothing
across two of them.

### The badges

Each hit carries one badge per list that found it, and a hit found by two lists
carries two:

| Badge | What it says |
|---|---|
| `definition` | This chunk defines the name you typed. It is above the ranking rather than in it. |
| `vector` | The embedding matched — this chunk means something close to what you asked. |
| `keyword` | The terms matched — your words are literally in this text. |
| `graph` | Reached from a neighbouring symbol — this was not itself a match; the graph walked to it from one. |
| `image` | The picture matched the words. |

`definition` is the one badge that is not a list in the fusion. When your query
is a single identifier, the chunks that define that name are lifted to the top
before the fusion's order is applied to everything else — so an agent that
already knows a term never has to read past its own definition to find it. It
appears only for an identifier-shaped query; a sentence that happens to name a
symbol is a question, and the fusion answers questions.

Only a `graph` hit also carries a **confidence** label, and that is deliberate.
The other three lists found a chunk by comparing it against your query, and there
is nothing further to say about how. A graph hit was reached by following an edge
from something else, and how well that edge was settled is exactly the thing a
reader needs to know before quoting it. The four values are defined under
[Confidence](#confidence); on this page each one also prints a sentence under the
opened span, so a reader who has never seen the word `inferred` is not left to
guess what it claims.

### Freshness

Every hit carries a freshness mark: a dot and the word `fresh` or `stale`. It is
one `stat` per distinct path, comparing the file's current size and modification
time against what was recorded when it was indexed.

It is conservative on purpose. A bare `touch` reads as stale even though no byte
changed, because a false "check this" costs one re-read and a false "this is
current" costs a wrong quotation.

When you open a stale hit the body panel says, under the heading: *This file has
changed since it was indexed. The lines below are what was read then.* That is
the precise claim — the text is not a guess and it is not the current file; it is
what semlith read, at the line numbers it read it at.

### The locator, and the two stages

A **locator** is an answer that says *where*, not *what*. One line per hit: the
line span, the enclosing symbol and its kind, which lists found it, the freshness
mark, and a single line of the chunk — the line with most of your query's words
in it. Hits are grouped by file, in the order the ranking put the files in, so
eight hits in one file are one file to open.

That is the first stage, and it is what an agent is given by default. The second
stage is the span itself, fetched only for the row that was actually opened.

This keeps an answer cheap. A search that returned eight whole chunks charges for
eight chunks whether or not seven of them were wrong; a search that returns eight
locators charges for eight lines, and only the one that was right is paid for in
full. The footer counts it out loud: `N of M shown · N tokens · budget N`, and
`truncated: N of M` when the budget cut the list. The budget is enforced from the
lowest-ranked group upward, exactly as the tool enforces it, and the first group
always shows whatever it costs — a budget that returned nothing would turn a
search into a silent failure.

### The dials

Each dial is the same control an agent sets on the tool, shown here so a person
can see what the agent is holding.

| Dial | What it changes |
|---|---|
| Store chips | Which stores are searched. `All stores` is the default. |
| `k` | How many hits to return, 1 to 50. |
| **Prefer** | `code`, `docs` or `any`. |
| `lang:` | Only this language. |
| `path:` | Only paths matching this glob. |
| **Budget** | Tokens the locator list may cost. Default 1500, floor 200. |

**`prefer` multiplies and never filters.** `code` leans the ranking towards source
files and `docs` towards prose, but neither excludes the other side: `prefer:
code` over a corpus of prose still answers, with the best prose it has. It exists
because the query text says how a question was written and cannot say what the
asker is after.

Above the results a hint reads something like `identifier · keyword weighted
twice`. It comes from the answer's own fields, not from a second look at your
query in the browser: an identifier-shaped query weights the keyword list twice
and a question-shaped one leaves the two level, and the classifier that decided
that is the one that ranked the hits underneath it.

### The body panel

Opening a row shows the span with a line-number gutter numbered from the span's
own first line — the same coordinates `semlith read` and every error message
print, so a number read here can be typed back without arithmetic.

- **Read whole symbol** widens from the matched chunk to the whole definition it
  sits in. It is disabled when there is nothing wider to open, and its tooltip
  says which of the two reasons applies: the span is already the whole of it, or
  several definitions carry this name and which one this is was not settled.
- **Open in graph** takes the symbol to the Graph page, centred.
- A hit that is an image shows the picture rather than text. The bytes are fetched
  through the API rather than linked, because a browser attaches no header to an
  `<img src>` and the token is a header; they are then handed to the element as a
  `data:` URL, because the policy the server sends allows `img-src 'self' data:`
  and widening it would weaken the one thing this product is checkable on.

Beneath the span, **Around** draws the open hit's neighbourhood: the symbol, its
callers and callees, and one ring further out. It is the Graph page's canvas in a
smaller frame and behaves the same way — drag a node, click to select — so that
"what calls this" does not cost you the span that raised the question.

## Graph

**What it is for.** Showing what the code says about itself: which definitions
exist, and which ones reach which.

This is the page a first-time reader understands least, so it is worth building
up from the pieces.

### What a node is

A node is a **definition** that tree-sitter found in a file. Tree-sitter is a
parser: it reads source code into a syntax tree, and every grammar ships a query
that names the tree's definitions. The node's **kind** — `function`, `class`,
`struct`, `method`, `interface` — is whatever that grammar's own tags query called
it. Semlith does not maintain a kind vocabulary of its own; there is one language
table and the extractor reads it.

Two other things are also nodes.

**The module node.** Every file gets one synthetic node named after the file
itself, spanning line 1 to the last line. It exists so that a definition at the
top level of a file has something to belong to, and so that a reference written
outside any definition has somewhere to hang from.

**The four navigational kinds.** Fourteen of the languages semlith indexes are
markup, data or configuration, and structure is the only thing a graph can be
made of in them. A Markdown `heading`, a YAML, TOML or JSON `key`, a CSS
`selector` and an HTML `element` are all symbols: `semlith symbol` finds them, a
locator names the heading a hit sits under, and `semlith stats` counts them.

But they are never the target of an edge. Nothing references a heading; no
`calls` edge has ever meant one. That exclusion is enforced where an edge's target
is resolved, and it has to be, because an edge's target is a *name* and resolution
is a join on that name. Without it, forty new languages put 1 427 navigational
symbols into this repository's own store — 933 configuration keys, 264 CSS
selectors, 230 headings — and 44 of them collided with the name of a real
function: `path`, `query`, `symbol`, `graph`, `shape`, `forget`, `error`,
`version`. Every one of those names became ambiguous, a walk refuses to cross an
ambiguous name, and the retrieval harness measured the damage: hit@1 fell from 13
to 10 and hit@8 from 27 to 23 on an unchanged question set.

### What an edge is

An edge is something one piece of source said about another. There are six kinds.

| Kind | What the source said |
|---|---|
| `defines` | This file defines this name. The module node points at every top-level definition in it. |
| `contains` | This definition is written inside that one — a method inside its class, a closure inside its function. |
| `calls` | This code calls that name. |
| `imports` | This file imports that name or module. |
| `references` | This code mentions that name without calling it — a type used in a signature, a constant read. |
| `aliases` | This name stands for that one: a re-export, or an import renamed on its way in. |

`defines` and `contains` are **structural**: they are read straight off the syntax
tree and say where something sits, not what depends on what. They are true and
useless for reachability, because every symbol is one hop from the file that
defines it — including both of them in a path would make two unrelated functions
in one file look two hops apart.

The other four are **dependency** edges, and a path walks only those. `aliases`
joined them because a re-export is the only thing standing between the name a
caller wrote and the definition it meant: a walk that refused to cross one would
report "not connected" about code that is connected. A re-export that does not
rename emits nothing, since there is no name change to cross.

An edge also carries the line the reference sat on in the source — where the call
was actually written, which can be hundreds of lines from where either symbol is
defined. Where that line is unknown, nothing is printed for it rather than the
enclosing definition's line being offered in its place; a wrong call site is worse
than no call site.

### Confidence

Every edge carries one of four values saying how well its target was pinned down.

| Value | What it claims |
|---|---|
| `extracted` | The file said where the target came from — a structural edge, or a call whose name or module the file also imports. |
| `resolved` | Several definitions answered to the name, and the ranking left exactly one standing. |
| `inferred` | Matched by bare name alone, and nothing corroborated it. |
| `ambiguous` | Several definitions carry this name and nothing chose between them. |

`extracted` and `resolved` are answers. `inferred` is a hint worth checking — two
functions called `new` in different modules is the ordinary case in real code, not
a corner case. `ambiguous` is not an answer at all: it means the store holds
several definitions of that name and cannot say which one this edge meant, and it
carries the count so a reader is told "4 definitions" rather than shown four
calls.

**Two are stored and two are recomputed.** `extracted` and `inferred` are written
into the edge's row at extraction time and never change. `resolved` and
`ambiguous` are computed on every request that reads an edge outward, and are
never written down.

That split is deliberate, and it is the reason the graph cannot quietly lie to
you. Resolution is a statement about the whole corpus: `resolved` means *there is
exactly one definition of this name that fits*. Index another file tomorrow that
defines the same name, and that statement stops being true. If the value had been
stored, the row would go on saying `resolved` — a claim nobody re-examined,
surviving the very operation that falsified it, and looking exactly like a claim
that had been checked. Recomputing costs one join per read and cannot go stale. A
ranking that cannot go stale is worth more than one that is cheaper to read.

The ranking prefers, in order: a definition in the same file as the reference; one
in the file the source's own hint names; one in a file the source imports; and
failing all three, the case where the corpus holds exactly one definition of the
name. One survivor is `resolved`; several are `ambiguous`, and every candidate
comes back marked and counted.

An edge the syntax tree already settled stays `extracted` and is not re-ranked. A
fact outranks a ranking.

Callers are different. An inbound edge was found through the edge's own source
id — an id, not a name — so there is nothing to resolve, and a caller row shows
the stored value, `extracted` or `inferred`.

### Why an ambiguous edge is drawn dashed and amber, and never crossed

On the canvas an edge the source settled is a solid line. One matched by bare name
is dotted. An ambiguous one is **dashed and drawn in amber**, the same amber its
badge wears, so the picture and the rail beside it say one thing.

The amber is not decoration and not a severity. An ambiguous edge is not a claim
about this code at all: the store is saying it does not know which of several
definitions this reference meant. Drawing it like the others would offer a line
the reader could follow to a conclusion the store never reached.

For the same reason, a traversal refuses to cross one. `semlith path` walks
dependency edges and stops at an ambiguous name, answering "not connected within N
hops by resolved edges" rather than producing a chain that happens to be made of
guesses. `--all-edges` walks the old way and labels the result, with a confidence
count per hop and the line "A hypothesis, not a finding."

Refusing ambiguous edges is necessary and, on its own, not sufficient: a path
walks *definitions*, not names. At one point every hop of a five-hop chain in this
repository's own store resolved to exactly one definition and the chain was still
false, because one hop arrived at `search` in `lib.rs` and the next left from
`search` in `routes.rs`. Each hop true, the chain not. A chain may only leave from
the definition it arrived at.

### The canvas

The layout is a force simulation. Nodes push each other apart, edges pull their
two ends together, and a weak pull towards the centre keeps the whole thing in
frame. A spring is shared out by how many edges its node carries, so a hub with
forty edges is not dragged about by all forty. The layout is solved before the
first painted frame rather than exploding outward while you watch.

It then cools but never freezes: each node carries a slow wander so the picture
drifts rather than sitting still. **Pause** stops the motion, and the button reads
**Resume** while it is stopped. For a reader who has asked their system for
reduced motion there is no loop at all — the graph is solved once and is then
still — and the button reads **Settled**, because a button offering to pause a
still picture is a lie about which of the two is in charge.

Dragging a node moves it and re-heats the layout. A drag that goes nowhere is read
as a click, which selects the node: the selected node is drawn in the accent
colour, its neighbours are lifted, and the edges touching it are thickened.

Node labels longer than 26 characters are truncated on the canvas only — a box
that wide covers its neighbours — and the whole name is in the hover card and in
the rail.

**The hover card** names the node and gives four lines: its `kind`, the `file` and
line range it is defined at, which `store` holds it, and `calls` as `N in · N
out`, counted from the edges currently drawn.

**The legend** under the canvas has five keys: `extracted`, `resolved`,
`inferred`, `ambiguous` and `selected` — the four confidence values as they are
drawn, plus what a selected node looks like. The count beside it reads `N symbols
· N edges`, and the edge half is what is drawn, not what exists, so it drops when
a kind chip is turned off.

### Scoping

The graph is deliberately small. A force layout is readable at dozens of nodes and
a hairball at hundreds, so a view is capped and the scope box is how you ask for a
different part of the graph rather than for more of it at once. The whole store
drawn at once is honest and unreadable.

**What it opens on.** Not the busiest symbol, which is what it did until 0.24.0.
A codebase's busiest name is its most *reused* one — `new`, `len`, `get` — and it
earns its degree from dozens of unrelated callers the extractor could only match
by spelling, so the page opened on a star of dashed `inferred` lines between
functions with nothing to do with each other. Three keys decide it now, in order:
a name defined once in the store, then how many of its edges the extractor
actually resolved, then raw degree. The last key is what keeps a store with
nothing but inferred edges drawing what it always drew. When the cap cuts a hub's
neighbourhood, the resolved edges are the ones kept.

**When nothing is connected**, the rail says so rather than leaving a field of
unexplained boxes. It happens for two real reasons: a scope holding both ends of
no edge, and a language semlith parses for definitions but not yet for calls.

- **The scope box** takes either. A value containing `/` or `.` is read as a path
  and scopes the graph to that part of the tree; anything else is read as a symbol
  name and centres the graph on it. Enter with the box empty returns to the
  overview.
- **The store chips** appear when more than one store is open and restrict the
  graph to the ones that are lit — the canvas and the rail alike. Until 0.24.0
  they scoped only the canvas, so a name defined in two open stores listed both
  stores' callers under a chip naming one of them, and the panel contradicted the
  picture beside it.
- **The edge-kind chips** — one per kind — turn a kind off and on. This filters
  what is *drawn*, instantly, without asking the server again.

Only edges with both ends on the canvas are drawn: a line to something off-canvas
is not a line anyone can follow. Nothing is hidden by that cap — the rail lists
every caller and callee whether or not it was drawn.

### The rail

Selecting a node fills the panel on the right.

At the top: the symbol's name, its file and line range, and chips for its kind,
its store and how many lines it spans.

**Callers** — everything in the graph that points at this symbol, each row
carrying its confidence badge and the kind of edge that reached it. When nothing
does, the rail says so: *Nothing in the graph calls this.*

**Callees** — everything this symbol points at, the same way. When there is
nothing: *A leaf, as far as the extracted edges go.*

**An ambiguous row does not name a file.** It reads `name · 4 definitions` and is
a button rather than a link, because a reader who followed it would open one of
four files the reference could have meant. Pressing it expands the row and lists
every candidate with its file and line, so the choice is made by a person looking
at the code rather than by the renderer guessing.

**Show N unresolved** appears under Callees when this symbol points at names the
store holds no definition for — a standard-library call, or a dependency that was
never indexed. Those are left out by default, because a list of names this corpus
knows nothing about is noise, and they are *counted* rather than silently dropped,
because "semlith shows no callees" and "everything this calls is outside the
index" are different facts. Pressing it lists them by name and edge kind.

Under the lists, the **confidence legend** states what each of the four values
claims, once, in place. A badge whose meaning a reader has to guess is a badge
that gets read as decoration.

**Chunks it lives in** takes the symbol's name to the Search page as a query.

## Impact

**What it is for.** Reading the code graph backwards from one name. You type a
symbol and the page lists everything that reaches it — so *what would notice if
I changed this* is a question answered before the edit rather than by the test
run after it. The heading carries the chip `semlith_impact`, the name of the
tool an agent calls for the same answer, and its subtitle says the same thing:
*Reverse reachability. What breaks if this changes — before the edit, not after
the test run.*

The Graph page draws a neighbourhood and this page answers a question. The
difference is what the answer is made of: a hundred callers is a hairball on a
canvas and a hundred rows on a page, so this one is rows.

**The shape of the page.** Two columns. On the left, a card carrying the
controls, the `Changing` line and three figures, and under it the reached
symbols themselves. On the right, the canvas, then [Path finder](#path-finder),
then [Trace](#trace) — the answer on one side and the two questions that follow
from it on the other. Below about 716 pixels of content width the two become
one column and the order above is the order you read down the page; there is no
breakpoint involved, the columns simply stop fitting.

**Controls.** Four of them, in the card at the head of the left column.

- **the name field** — one symbol, matched exactly, as the placeholder says: *A
  symbol's name, matched exactly*. Enter runs it; so does **Reach** beside it.
- **hops** — how far back to walk, 1 to 10, 3 to begin with. A value outside
  that is clamped into it and the field is corrected to what was actually used,
  rather than the page walking one depth and displaying another.
- **Prefer verified edges** — on. It is the same refusal `semlith path` makes,
  pointed backwards: with it on, a name several definitions answer to is not
  crossed. Turning it off re-runs the question at once, so the two answers are
  one press apart rather than one reload.
- **Reach** — asks `/api/impact`, which is the same function `semlith impact`
  calls from a terminal.

Under the band a row states what the answer is about: **Changing**, then the
symbol, then a pill reading `depth 3 · reverse` for whatever depth was used.
Before anything is typed the page says *Name a symbol to read the graph
backwards from it.* rather than showing an empty table.

**The answer opens with a sentence**, not a number: `N definitions in N files
reach <name> within N hops`. The sentence is composed by the same function that
prints it in the terminal, so the page and `semlith impact` cannot describe one
walk two ways. Three counts sit under it — **Reached**, the definitions that get
there; **Files**, the files they are written in; and **Unsettled**, how many of
those rows were reached across an edge marked `inferred` or `ambiguous`. That
third figure is the one worth reading first: it is the part of the answer that
has not been settled, stated beside the part that has, rather than left for a
reader to count off the badges.

**The rows are grouped by hop**, nearest first, each group headed `1 hop`,
`2 hops` and so on with the number of definitions in it. The walk is breadth
first, so the hop against a definition is the *fewest* hops it takes: something
that reaches the symbol directly and again through three others is a direct
caller and is listed once.

| Column | What it means |
|---|---|
| Reached symbol | The definition that gets there. |
| Where | Its file and the line it is defined at. |
| Via | The name one hop nearer the centre, and the kind of edge that reached it, followed by that edge's support badge. |
| Hops | How many edges away it is. |

Under the hop groups is a **Files** block: one row per file, the file's path,
how many of its definitions reach the symbol, and `nearest N hops` — the closest
any of them gets. It is the same answer aggregated to the unit a person opens.
A file carries no support badge, because a file is not an edge and nothing
settled or failed to settle it.

The walk follows dependency edges — `calls`, `imports`, `references`,
`aliases` — and not the structural ones, for the reason a path does not: every
symbol is one hop from the file that defines it, so a `defines` edge is true and
says nothing about who would notice a change.

The answer is capped. When the cap cut it, a line under the rows reads `N more
left out at the limit of N`, with the limit the daemon is enforcing rather than
a number written here.

**When nothing reaches it**, the page distinguishes two cases, because they call
for different things. *The symbol is indexed; nothing in an open store calls it.*
means the walk ran and found nothing. *No definition of that name is in an open
store. Check the spelling, or index the repository that holds it.* means there
was nothing to walk from.

### The canvas, and what a ring means

The card at the top of the right column draws the same answer as a picture:
the symbol you asked about at the centre, and every symbol that reaches it
placed on a ring. **The ring is the hop count.** The innermost ring is
everything one hop away — the direct callers — the next is two hops, and so on
out to the depth you asked for. The caption says so: *reverse reachability,
rings by hop*.

A line runs from each symbol to the symbol it reached through, so a chain is a
path inwards to the centre. A name drawn in amber is one whose edge is
`inferred` or `ambiguous`, which is the same amber its badge carries in the
table on the left — a reader who has learned one has learned the other.

**The hover card** is the Graph page's, and follows the pointer the same way. For
a symbol that reaches the subject it gives `hops`, the `file` and line of the
row that reached it, what it `reaches` through and by which edge kind, and the
edge's `confidence`; the subject itself says it is hop 0.

**It does not move.** Every other canvas in this portal is a force simulation
that settles; this one is arithmetic, drawn once. That is deliberate, and it is
the whole reason the picture is worth drawing: hop distance is the only thing
this page is about, and a spring layout is a machine for destroying it. A
symbol three hops out would end up wherever repulsion happened to put it, which
is a picture that looks informative and is not. Drawn this way, the same answer
produces the same picture twice, and nothing here needs turning off for a
reader who has asked for less movement.

A ring holds as many labels as fit around it at that radius. When a hop has
more symbols than its ring can carry, the last label on the ring is followed by
a small `+N` saying how many are not drawn. They are all in the table on the
left; the canvas is a shape, and the list is the answer.

### The support classes

Every reached row carries one of the four values the Graph page's
[Confidence](#confidence) section defines, and they mean exactly the same thing
here, because they are the same edges read in the other direction.

| Badge | What it claims |
|---|---|
| `extracted` | The source named the target — the file imports it, or the syntax tree already settled it. |
| `resolved` | Several definitions answered to the name and the ranking left exactly one standing. |
| `inferred` | Matched by bare name alone, with nothing corroborating it. |
| `ambiguous` | Several definitions carry this name and nothing chose between them. |

Hovering a badge gives the one-line version — `the import names the target`,
`name and module hint agree on one definition`, `matched by bare name`, `several
definitions, none chosen` — so a reader who has not met the words is not left to
guess.

With **Prefer verified edges** on, no `ambiguous` name is crossed and nothing
behind one appears in the list at all. With it off, they are, and the page says
so above the rows rather than under them: *Walked names with several
definitions. Rows badged ambiguous are a hypothesis, not a finding.* That is the
whole of the difference. An ambiguous edge is not a weak claim about this code;
it is the store saying it does not know which of several definitions the
reference meant, and a caller list that crossed one silently would be somebody
else's callers mixed into yours.

### Path finder

The card under the results answers the forward question: is there a chain from
one symbol to another, and what is it made of. Until this release it answered
only from a terminal.

**Controls.** A **from** field, a **to** field, two chips and **Walk**. The
chips are **Prefer verified edges**, on, and **Strict**, off. They are one
control with two faces: pressing **Strict** turns **Prefer verified edges** on
with it, and turning **Prefer verified edges** off clears **Strict**, so the
pair cannot be left saying two things. Names with several definitions are
walked only when neither is on. `Strict` wins over asking for everything, for
the same reason it does on the command line: asking for both is asking for the
refusal you named explicitly. The depth is fixed at six hops, and the chip in
the card's heading says so — `max 6 hops`, which becomes `<from> → <to> · max 6
hops` once a walk has run.

**When there is no chain** the card says which question was refused rather than
printing nothing: *Not connected within 6 hops by resolved edges* when ambiguous
names were not crossed, *Not connected within 6 hops by any edge in the store*
when they were. The first carries a **Show inferred chain** link that turns both
chips off and walks again, so the weaker answer is available without being the
default.

**A chain** is one row per hop, numbered: the hop's start as `name @ path:line`,
an arrow, its end the same way, the kind of edge, and that edge's support badge.
Where a hop arrives at one definition and the next leaves from another, a
**seam** row is drawn between them reading `seam · <name>: N definitions ·
continues from <name> @ path:line`. That is the failure the Graph section
describes — every hop true and the chain false — shown where it happens rather
than summarised afterwards.

Under the hops is a counted line: `N hops · N extracted · N resolved · N
inferred · N ambiguous`, and where the chain crossed a seam, `· N through
ambiguous names` with each of those names and how many definitions it has. A
chain made of guesses also carries the sentence *A hypothesis, not a finding.
Read the seam before you rely on it.*

### Trace

The last card is the same chain written as something a person can paste into a
review. Its heading carries the chip `evidence view` and, on the right, **Copy
as evidence**.

**Controls.** **from**, **to**, and **Trace**. There is deliberately no depth
and no edge toggle here: this card does not walk the graph a second time.
`/api/trace` reads the chain the finder would produce and then fetches one line
of source per hop, and it fetches that line *from the store* rather than from
the file on disk — the same rule `semlith read` follows, so the quotation is
what semlith actually read.

**Answer** is one sentence naming what the chain is worth: *Connected in N hops:
N extracted, N resolved. Every hop names its target through something the
extractor read or the ranking settled.* when every hop was settled, and a
sentence beginning *Not connected by resolved edges* when the nearest chain is a
hypothesis — saying whether it got there by crossing ambiguous names or by
matching bare names, and ending *treat it as a hypothesis*.

**Chain** is the same rows and the same seams the path finder draws, from the
same renderer.

**Supporting lines** is one block per hop: the line of source the edge was
written on, as `path:line` and the code, or *no call line recorded* where the
store holds no call site for that edge. Under it is the hop it belongs to and a
mark — `supporting fact` for a hop that was `extracted` or `resolved`, and
`candidate — corroborate before use` for one that was not. A wrong call site is
worse than no call site, which is why the second case is stated rather than
filled in with the enclosing definition's line.

**What "Copy as evidence" puts on the clipboard** is plain text, no markup, and
byte for byte what `semlith trace --evidence` prints — the same function
produces both. It is: a first line reading `<from> → <to>`; a second reading
`answer: <the sentence above>`; then one numbered line per hop, `N. <from> @
<path>:<line> -> <to> @ <path>:<line>  [<support class>]`, with `seam: <name>: N
definitions · continues from <name> @ <path>:<line>` appended to the hop it
follows; then, where there are any, a blank line, the word `supporting lines`,
and one line per hop reading `<path>:<line>   <the code>   [supporting fact]` or
`[candidate]`. Nothing else — no scores, no prose, and no text the store does
not hold.

**This page does not read live.** Impact, a path and a trace are each the answer
to a question somebody asked, and the rule the rest of the portal follows
applies here too: redrawing an answer under its reader would be answering a
different question.

## Inside the index

**What it is for.** Reading folders, or one fetched URL, into stores, and
watching every run the daemon is carrying.

The daemon is the writer for every store it opened. This page does not index
anything itself, and it no longer holds a run either: it asks the daemon to queue
one and then reads it back, the same way it reads anything else. That is the
difference worth knowing about. The page used to *be* the run — it held a
streaming response open and the run existed only as the events travelling down
it — so the tab that pressed the button was the only thing that knew the run was
happening, and leaving the page threw that away. The work carried on, because the
work is the store's, but nothing on screen could find it again.

A run lives in the daemon now: its id, its paths, its status, its counters, its
clock, the last 500 lines of its log and, when it ends, its summary. The same
`say` closure that emits every event writes that record, so the card and the log
cannot disagree about what happened. Navigating to Search and back, refreshing,
or closing the tab for the length of a run changes nothing — every run is on its
card where it actually is, with its log carrying on from the last line this page
saw. A run is only ever cut short in two ways: its own **Stop**, or
`semlith start` ending.

**Controls.** Everything the button band opens appears in one place, directly
under the band. Four of these buttons open a panel, and they used to open it in
four different parts of the page — the folder picker under the band, the
repository checklist and the URL card below the summary cards, and the machine
limits at the very foot. Pressing one button opened something you were looking
at; pressing another opened something a screen and a half away, with nothing on
screen saying it had happened.

- **paths** — one path per line, so several folders can be started without
  opening a picker at all. The first-run screen carries a path into this field.
- **Choose folders…** — a directory picker rooted at your home, which can tick
  more than one folder in a visit.
- **Projects under a folder…** — the repository checklist below.
- **Add from a URL** — one https request for exactly that URL, a page, a PDF or a
  file. Nothing is crawled, no credential is sent, and what lands is written
  inside the store's own `downloads/` directory rather than into your working
  tree. It then goes through the same queue and becomes a card like any other, so
  there is no second progress mechanism to learn. The fetch itself is the one
  synchronous part, because its refusals — the URL was `http`, the body was too
  large, nothing here reads that content type — are what the card has to show.
- **the target** — where the paths go. Three answers, below.
- **Start indexing** — queues the runs and answers at once with how many were
  queued. The cards are what happens next; the button does not become a control
  for them, because there is more than one run.

The three pickers are mutually exclusive: two of them open at once is two answers
to one question.

**Where the paths go.** The selector offers `each folder becomes its own store`
and an `add to <name>` per open store, and the route behind it takes a third
answer a script can use:

| Target | What happens |
|---|---|
| `each` | One store per path, each resolved exactly as `semlith index <path>` resolves it — same name, same directory under the store home, same registry entry, including the numeric suffix when a second folder is also called `api`. It is the same function, not a second copy of its rules, which is what stops indexing a folder here and again from a terminal producing two stores. |
| a named store | Every path goes into that one store, and each becomes one of its roots so the watcher keeps it current. |
| nothing named | `each` for more than one path, and the single-store behaviour for one. Three folders are three corpora, and putting them in one store is a choice nobody made. |

A path that could not be queued is named with its reason and the ones beside it
still start: a typo in the third folder should not take the two that were with
it.

**Projects under a folder** is the case the directory picker handles badly —
ticking twelve repositories one directory at a time is twelve walks into and back
out of the same parent. It asks the daemon which children of a folder are git
repositories and offers them all at once, already ticked. A `.git` file counts as
much as a `.git` directory, so a worktree and a submodule are on the list. One
level only: a monorepo is one store, and its nested repositories are its own
business. Where none of the children is a repository the plain subfolders are
offered instead and the card says so, because a folder of folders is still what
you were pointing at. A child an existing store already covers is listed with
`in <store>` and starts unticked — shown rather than omitted, because "nothing
here" and "all of it is already done" are different answers. **All**, **None** and
**Use N** fill the paths field and set the target to `each`.

**One card per store**, and the card is the run — the one that is going, or the
last one that finished, kept afterwards so a page opened later says what it did
rather than showing nothing. Each card carries its store's name, a status pill, a
percentage and bar, files scanned over files found, chunks written, a
chunks-per-second rate, the elapsed clock, the paths, and the log.

**This page reads live**, on the runs and stores domains, and it is the one page
that paints in place rather than redrawing itself: a card holds a log and a
scroll position of its own, and redrawing would throw both away. A store whose
run the daemon has forgotten — it was deleted, or the daemon restarted — loses
its card rather than keeping a stale one.

| Status | What it means |
|---|---|
| `queued` | Submitted and not yet under way. The pill carries its place in line when it is waiting on the daemon-wide queue below; without a place it is waiting on its own store's writer, which is the watcher being mid-file — one writer per store is the rule that stops two passes corrupting each other. |
| `running` | A writer has it. |
| `paused` | Held between files by this card's **Pause**. |
| `stopping` | A stop was asked for and what the run embedded is being undone. Told apart from `stopped` because on a large corpus the undoing takes as long as the embedding did, and a card that jumped straight to `stopped` would be claiming the store was already back to what it was. |
| `done` | Finished. |
| `stopped` | Stopped, and undone. The bar returns to 0% rather than filling, because a full bar would say the opposite of what happened. |
| `failed` | Ended on an error that was not a file's — the model, or the store. |

**The log** under each card prints one line per file with the outcome that file
got — `indexed`, `unchanged`, `skipped`, `removed`, `refused`, `failed` —
coloured so that a re-index of an unchanged corpus reads as a wall of background
with the handful of real writes standing out of it. The run ends with a counted
line: indexed, unchanged, skipped, removed, failed where there were any, chunks,
and the total elapsed.

The card reads its log by a cursor rather than by how much it has drawn, so two
tabs open on the same run each see every line exactly once and neither is
affected by what the other has read. The daemon keeps the last 500 lines; a tab
away for longer than that restarts from the oldest line still held rather than
showing a gap as though it were continuity.

**Three of those outcomes owe an explanation, and the line gives it.** A
`skipped`, `refused` or `failed` line carries the reason beside the path, because
a log that says "skipped" against two thousand files and nothing else is a log
nobody can act on. A refusal names the rule that refused it — the credential
deny-list, or the kind of credential the content scan found and the line it sat
on, never the text that matched. A skip names one of a closed set: `empty`, `over
8 MiB`, `not a regular file`, `unreadable` with the operating system's own words,
`binary`, `no text in this document`, `not a decodable image`. And the final
counted line is followed by what the skipped count was made of, kind by kind, so
that a large number is a fact rather than an invitation to assume something was
lost.

**`failed` is the outcome that used to be the end of the run.** Before 0.19.0 a
file that could not be read — a truncated image a decoder rejected, a source file
tree-sitter could not parse — ended the whole pass, so a tree of ten thousand
files stopped at the first bad one. It is now one line with the decoder's or the
parser's own message on it, and the run carries on to the next file. A refusal is
a decision semlith made; a failure is one it could not avoid, and the two are
coloured and counted apart for that reason. Every failed path is named again on
the closing line rather than only counted, because a run that ends with "eleven
failed" and no names is a run whose eleven files nobody goes and looks at.

**The elapsed clock** sits beside the counts, because files, chunks, rate and
elapsed are one reading of one run. It starts when the run is submitted rather
than when a writer picks it up: the wait for a writer is time you are waiting.
The daemon measures it, the card ticks between polls so the seconds move, and
every poll corrects to the daemon's figure — which only ever goes forward, so a
correction never makes the reading jump backwards. It is the run's time and not
the tab's, which matters because a browser throttles a background tab's timers to
about once a minute. It stops while a run is held, because held time is not time
anything is happening, and it freezes on `done`, `stopped` or `failed`, holding
the total.

It also spans the whole run rather than the slice. A run hands the writer back to
the watcher every 45 seconds and returns as a fresh job, so a clock measured
around that work restarted from zero every 45 seconds and the page faithfully
redrew a run that had just begun. The clock belongs to the run now.

**What Stop does that Pause does not, and what Remove does that neither does.**

**Pause** holds the run between files. The writer is still this run's — the lock
is not handed back, the store is left mid-corpus, and **Resume** carries on from
where it stopped. Nothing that has been embedded is touched.

**Stop** undoes the run. Everything it embedded is rolled back, so the store is
left exactly as it was before the run started, the bar returns to 0% rather than
filling, and indexing the same folder again begins from the beginning. The
confirmation says so before it happens. Pause is "wait"; Stop is "as though it
never ran".

**Remove** is what the button says on a run that is still `queued`, and it takes
that run out of the line. It answers at once and confirms nothing, because there
is nothing to undo: a queued run has embedded no file and holds no writer. That
is the whole difference between removing a folder from the queue and stopping one
that is going.

One more event can appear in the log: `slice`, saying the writer has handed
itself back to the watcher and put the rest of this run back on the queue. It is
one run and one log, and there is nothing for you to do about it.

**Waiting** appears under the cards when more than the machine can carry has been
asked for, listing the queued runs in the order they will start, each with its
store, its paths and a **Remove**. A store's writer used to take the next job on
its own queue with nothing above it, so eleven open stores meant eleven runs at
once whatever the machine had; there is one daemon-wide queue now, ordered by
submission, and a run is admitted only while fewer than *runs at once* are
running. Anything submitted while the queue exists joins its end. The head is
admitted the moment a run finishes, stops or fails — with this page open or not,
which is the point of the run living in the daemon. The watcher's own re-embeds
do not pass through the queue: they are small, they already interleave with runs,
and holding a file save behind eleven queued repositories would make the watcher
useless exactly when the machine is busy.

**How hard this machine may work** is the last card: three numbers, each with the
machine reading behind it and the sentence that derived it. The daemon reads
logical cores, total memory and memory free *now* on every request, so the figure
in front of you is about this machine at this moment rather than at startup.

| Setting | Derived from |
|---|---|
| runs at once | Memory free less a 2 GiB reserve, divided by what one run peaks at, and no more than the cores allow with one kept free so the portal still answers while every writer is busy. Floor 1, and 1 outright when the memory reading failed — a failed reading is not a machine with no memory. |
| threads each | The embedder's own thread count divided between the runs, clamped so runs × threads never exceeds the cores. Floor 1. |
| MiB per store | 512 MiB of vectors, doubled once past 16 GiB free beyond the reserve and again past 64 GiB. Two steps rather than a curve, because a figure you recognise is worth more here than a fitted one. |

Each is yours to change, and changing one is the ordinary thing to do with it —
a laptop doing nothing else can index harder than a default chosen for a laptop
that might be. Above the derived value the field says what that means rather
than warning you: nothing here is drawn in red, because none of it is a mistake.

**Each also has a ceiling, and the ceiling is not the derived value.** The
derived value is what this machine would pick left alone. The ceiling is where
the answer stops being a preference and becomes a machine that swaps: the
logical core count for the two parallelism settings, and memory free now less
the reserve for the store budget. The field will not go past it and neither
will the route, because a cap only the page knows about is not a cap. An
environment variable is exempt — somebody who exported one has said what they
mean more deliberately than somebody typing in a box.

The three answer each other. *threads each* is derived from the runs actually in
force rather than from the runs this machine would have chosen, so raising *runs
at once* changes what *threads each* suggests underneath you. That is why they
are one card.

Changing *runs at once* takes
effect on the next admission: raising it admits the head immediately, lowering it
stops nothing already going, because a run holds a writer and undoing it would
cost the work it has done. The other two apply to the next run queued.

A value the environment sets — `SEMLITH_INDEX_PARALLEL`, `SEMLITH_EMBED_THREADS`
or `SEMLITH_INDEX_MEMORY` — is shown, disabled, and says so. An explicit variable
is an instruction from whoever started the process, and a page in a browser may
not overrule it. What is not from the environment is saved in
`~/.semlith/settings.json`, written the first time a field is moved and not
before, so a home without that file is a home deriving all three.

**What survives what.** Navigating away, coming back, refreshing and closing the
tab change nothing about any run: the daemon holds them, and this page redraws
what it finds. A daemon restart is the one thing that does not survive. A run that
was going when the process ended is not resumed — the files it had committed are
in the store and indexing again reports them as `unchanged` — and a run that was
still queued never started at all, which would otherwise leave no trace anywhere.
The next `semlith start` says both on the store's own event feed, once, on the
Stores page.

## Retrieval ledger

**What it is for.** What your agents actually retrieved, recorded locally, so the
saving semlith claims has a denominator under it.

Recording is on by default, from every surface: the stdio MCP server, the
daemon's `/mcp` route, the CLI and the portal all write through one module, so a
search from a terminal and the same search from an agent are counted the same
way. The rows live in the store beside the chunks, in a `retrievals` table, and
never leave the machine. The daemon says on every start that it is recording and
names the flag that stops it. `semlith start --no-ledger` stops a session;
`SEMLITH_LEDGER=0` stops a machine; erasing every row is one `DELETE`.

Rows are hash-chained — each carries the hash of the one before it — so an edited
or deleted row can be found. `semlith ledger --verify` re-walks the chain and
names the first row that does not verify, and the page reports whether the chain
is intact. From 0.24.0 `--verify` also prints the savings block below it: a
verify that says only "intact" proves the record was not edited and says nothing
about what it records, which is the question somebody defending a figure is
actually asked.

**The figures, and why each one is beside the others.** A saving never appears
without its coverage and its tier, on this page or anywhere else, because a
figure a reader cannot check is one they are being asked to take on trust.

- **Coverage** — the share of recorded retrievals the saving is computed over.
  A retrieval that found nothing saved nothing and is excluded from the figure
  and counted in the denominator.
- **Tier** — `measured` when every credited row was counted by the store's own
  tokenizer, `modelled` when any of them was estimated at four characters per
  token. A mixed ledger reports `modelled`, because that is what the weaker half
  makes the whole.
- **Zero-hit** — the share of retrievals semlith answered with nothing, as the
  v4 design draws it: a percentage, with the count and its denominator kept in
  the caption under it. `0 of 4` is two numbers a reader has to divide, and the
  quotient is what they were dividing for. Read from the ledger rather than
  derived by subtracting credited from total, which since 0.24.0 would have
  counted every raw read as a question the corpus could not answer.
- **Refunds** — files an agent read whole after all, on a file this store holds.
  It is a *measurement* on a client carrying the steering hook, which writes one
  `raw-read` row for each such read, and a *floor* everywhere else — the tile
  says which, and a floor carries a `+`. Without it the ledger counts only the
  questions semlith was asked, which flatters every ratio on the page by leaving
  out the ones it was not.

Refunds and zero-hit are two figures rather than one on purpose. A refund is an
agent that did not reach for semlith; a zero hit is semlith that did not reach
the answer. They call for opposite things, so adding them together would say
neither.

Each store's row on the Stores page carries the same three facts in one cell —
tokens, coverage, tier — and no chart.

**The figures:**

| Figure | What it counts |
|---|---|
| Queries recorded | Retrievals recorded, and how many distinct clients ran them. |
| Excerpt tokens | What agents were actually sent. |
| Whole-file tokens | What reading those same files whole would have cost. |
| Measured ratio | Whole-file tokens over excerpt tokens, with its coverage and tier beside it. |
| Coverage | What share of recorded retrievals were credited. |
| Net tokens | Whole-file less excerpt, over rows that found something. |
| Zero-hit | The share of queries the corpus could not answer. |
| Tier | `measured` when the store's own tokenizer counted, `modelled` when it did not. |

**What the ratio deliberately does not do.**

It never appears alone. Coverage says how much of the ledger it was computed over
and the tier says whether the tokens were counted or estimated; without both, a
number like 18.3× is a marketing claim rather than a measurement.

It does not count a failure as a success. A query that found nothing is still
recorded and is credited nothing — a ledger that remembers only its successes is a
marketing document — which is why Coverage and Zero-hit sit beside the ratio
rather than being quietly left out of it.

It does not sum figures counted two different ways. Each row records which
instrument counted it, and rows counted by the store's own embedding tokenizer are
never added to rows estimated at four characters per token. A session that never
loaded a model — a graph-only session, or an airgapped machine with no cached
model — produces `modelled` rows, and the page says so.

And it does not pretend the denominator was measured as carefully as the
numerator. The whole-file side is the files' size on disk at four characters per
token, never tokenized, because reading every file back to count it precisely
would cost more than the saving being measured.

**by client** breaks the count down under each client's own name, taken from the
MCP handshake rather than guessed at here.

**The rows and the replay are two tabs**, not two stacked cards. They are two
readings of one ledger — what was retrieved, and what the agent did afterwards —
so a tab says they are alternatives, where stacking them made the second one
something you found by scrolling past the first. The sessions table keeps its
own card above both, because it is the summary the two tabs are of.

**This page reads live.** Every surface that records a retrieval writes through
one function, and that is where the ledger domain is bumped, so a row lands here
as an agent retrieves it — whichever window the agent is working in.

## Reports

**What it is for.** Turning what this machine already holds — the ledger, the
index and the graph — into a file somebody who will never open the portal can
read. The subtitle states the two things that matter about it: *Generated
locally, exported as a file you own.* The heading carries the chip
`semlith_report`.

Nothing in a report is a model's opinion and nothing reaches the network. The
three sources are the retrieval ledger, the index and the code graph, and the
same generator answers `semlith report` in a terminal, so a file saved from this
page and one written from the command line are the same bytes rather than two
renderers that will eventually disagree.

The page is three things down the screen: a picker of five reports, a builder
for the one you picked, and the preview it produced.

### The five reports

The picker is a grid of five cards and exactly one is chosen. Each card carries
the report's name, what it is, and who reads it — the third line being the part
usually left out, on the grounds that a report nobody can name a reader for is a
report nobody asks for.

| Report | What it is | Reader |
|---|---|---|
| Retrieval savings | Tokens the agents did not read, counted from the ledger instead of claimed. | for whoever approves the spend |
| AI access audit | Which agent read which file, when, in a hash-chained record nothing can quietly edit. | for security review and AI-use policy |
| Change brief | Blast radius for what this machine re-read, written as a note you paste into the pull request. | for the reviewer, before the merge |
| Index health | Stale files, roots never indexed, formats skipped, and how much of the tree the graph covers. | for the person who owns the store |
| Knowledge gaps | Questions the agents asked that your corpus could not answer well — a to-do list for documentation. | for whoever writes the docs |

Choosing one generates it immediately. There is no Generate button, because the
selection is the request: the page used to be five cards each with its own
Generate and its own row of four format buttons, which is twenty-five controls
for five reports and a preview at the bottom that could have been about any of
them.

### The builder

The card on the left is about the chosen report and nothing else. At the top,
its name and the sentence saying what it answers once it exists — *What
retrieval actually saved, per client, with the arithmetic shown.* for Retrieval
savings, and one of its own for each of the other four. Under that, the report's own span — what
it covers, said by the report rather than inferred from the chips.

**Window** is a row of four chips: **24 hours**, **7 days**, **30 days** and
**This quarter**. Two of the five reports have a date to narrow by — the access
audit and the change brief — and on the other three the chips are drawn inactive
and the report says under its own title that it could not honour a window rather
than printing a span it did not apply.

**Scope** is **all stores** and one chip per open store. It is the filter
`/api/report` used to read as `store` and throw away, so a request that asked for
one store of six got all six with nothing saying so.

**Format** is a row of five chips: **Markdown**, **CSV**, **JSON**, **HTML** and
**PDF**. Pressing one re-generates at once, because the format is part of the
request rather than something applied to the text afterwards — the server renders
each one, and the preview is what the file will contain. The PDF is typeset from
the same block structure the other four render, not a print of the HTML, by a
writer inside the binary: there is no browser in this path and no network.

**Price tokens at** is a row of model chips: **Sonnet 5**, **Opus 5** and
**Haiku 4.5**. It sets which model's input price the savings arithmetic is
costed at. The prices are written into the binary rather than fetched, for the
same reason the rest of the page is: a report has to generate on a machine with
no network, and a price whose source a reader cannot see is worse than one they
can argue with.

The equivalent command is not in this card. It has one of its own — *Same thing
without the browser* — where the design puts it, with the flags the chips above
set, so the same file can be produced from a script without this page.

### The preview

The card on the right is the report itself. Its bar carries the file name it
would be saved as and two controls. **Copy** puts the generated text on the
clipboard. **Export** hands the same bytes to the browser as a download, with the
content type of the format it is in. Both use the one request that produced the
preview, so the thing on screen, the thing copied and the thing saved cannot be
three different reports.

The footer states **rows shown, the full file size, the format and the
signature**. The numbers are real rather than estimated: the row count is summed
from the report's own table blocks, and the size is the length of the bytes
Export would write, not a guess from the length of the preview. The signature
reads `unsigned`, and will until there is something to sign with — see the
toggles above.

**Save to disk** beside them is the browser's own save. No route writes a report
file: a report is a reading of a moment handed to the person who asked for it,
and the daemon writing one into a directory on their behalf is what a schedule is
for.

The sentence beside them is the point of the page: *generated here, written only
where you save it.* The report is built on this machine, handed to the browser
and not to any server, and until you save it, it exists nowhere on disk.

**This page does not read live.** A report is a reading of a moment, and one
that redrew itself under a reader would be a different document from the one
they were quoting.

### The three toggles

Two of them do something, and the third says what it is waiting for.

**Hash the query text** replaces every query with a digest of it — `hash:` and
sixteen characters of the same blake3 the ledger chains its rows with. What it
keeps is the who, the when, the tool and the token counts; what it drops is what
was asked. It applies *wherever a query reaches the document*, not only to the
table it sits above: the gaps report names the questions that found nothing, and
a toggle that hid a query in one table and printed it in another would be a
promise broken by the document that made it.

**Attach retrieved excerpts** unrolls the access report's session lines into the
individual retrievals behind them, newest first, to a ceiling of 500. A session
row says an agent asked forty times; this is which forty. The two compose: with
both on you get every retrieval, with its agent, its tool and its cost, and no
query text.

**Sign the report** is drawn and disabled. A detached signature needs a key, and
where that key lives, how it is rotated and what a reader checks it against are
decisions this product has not made. A switch drawn as though it worked would put
a promise on the page that the file does not keep, and one quietly left out would
hide a gap the design says should be visible — so it is drawn, off, and says so.

`semlith report access --excerpts --redact` is the same two toggles from a
terminal.

### Schedules

A schedule is a report the daemon writes on its own, on a cadence, into a
directory you name. It is the one thing on this page that is not a reading of
right now.

**It belongs to the daemon, not to this page.** The card shows what the daemon
holds; adding a schedule here writes a record and the daemon does the work. If
nothing is running `semlith start`, a schedule is still recorded and still
listed — it simply does not fire until a daemon is up. Everything the card shows
is on disk, so a restart loses none of it.

**Where it lives.** `~/.semlith/schedules.json`, beside `registry.json` rather
than inside it. That is deliberate: a schedules file somebody's editor truncated
should cost them their schedules and not every store on the machine. It does not
exist until the first schedule is added, and its absence is the normal state
rather than an error. Like the registry and the lock file, it is tool-written
state — semlith writes it, and nothing documents a way to hand-edit it.

**Cadence.** The chips are the common ones. What the record holds is an interval
in seconds, so a chip is a shortcut for a number rather than the whole
vocabulary, and a cadence no chip spells still round-trips and still says what
it means. The floor is a minute: a report reads every open store's database, so
a schedule firing every second would be a background process nobody asked for.

**What it records, and why all of it is shown.** Each schedule carries when it
last ran, when it next will, and the path it last wrote. A cadence with no
outcome beside it cannot say whether the thing is working, which is the failure
this feature would otherwise have: a row that reads *on* beside a folder that
never fills. So a run that wrote nothing says why, in full, and the path it
wrote is cleared — a schedule can never show "failed" and a filename together.

The failure this will actually meet is a destination that has gone away:
deleted, renamed, or on a volume that is not mounted this morning. The daemon
checks the directory and never creates it. Creating it would answer an unmounted
volume by writing the report into the empty mount point, which looks like
success and loses the file.

**Two schedules never generate at once**, and a schedule due while its store is
still indexing waits rather than reporting on a half-written index.

**Same thing without the browser.** Per the parity rule, the CLI reaches the
same schedules this card does — both read and write the one file the daemon
owns, so there is no second list to disagree.

```sh
semlith schedule list
semlith schedule add savings --every 604800 --to ~/Reports --format pdf
semlith schedule remove s1
semlith schedule set s1 off
```

`--every` is seconds, as the record holds it. `--to` must be absolute: the
daemon runs the schedule from a working directory that is not yours, so a
relative path is refused rather than resolved against somewhere nobody meant. A
running daemon notices a schedule added from a terminal within a minute.

## Agents

**What it is for.** Connecting an agent to this daemon, and seeing which ones are
connected.

**The endpoint** is in the page heading, with a copy button, a state pill reading
`answering` or `closed`, and a **Start** / **Stop** control. Stopping it drops the
`/mcp` route and nothing else: the stores stay open, the watcher keeps running,
and the portal keeps working. A closed endpoint answers 404 rather than 401, so a
client that has been told to stop learns the same thing whether or not it still
holds a key.

**Login service.** Whether the daemon is installed as one — a launchd user
agent, a systemd user unit, a Windows logon task — and when it last started. The
page has always said "one endpoint, every client"; until 0.21.0 that endpoint
existed only as long as somebody held a terminal open for it. The card names the
mechanism, the definition it wrote and the log to read when it misbehaves. On
Windows it also says that a logon task restarts a task that failed and does not
supervise one that exited cleanly, because the three mechanisms are not equally
strong and a page that implied they were would be wrong on one of them. When no
service is installed, the card carries the command — `semlith start --service` —
rather than a button: installing a background process on a machine is not
something a web page should do on a click.

**Clients** is the same four-step report `semlith doctor` prints, from the same
function, so the page and the terminal cannot disagree about which client can
reach semlith. One row per client that is actually on this machine; the three
semlith cannot register carry their reason and are not faults.

A client with semlith registered at user scope and switched off for one
directory is shown with the directories that override names — not as a state of
this page. "Here" for the daemon is wherever a service manager started it, which
is nobody's working directory; the reader is the one who knows which of those
directories they work in. Run `semlith doctor` in the directory itself for the
verdict that applies to it.

**Agent key.** The credential a client carries. It is shown masked — `sml_`
followed by dots — and the route does not send a preview either, because a preview
of an agent key still begins `sml_` and the point is that nothing about the
credential arrives unasked. **Reveal** fetches the real value; pressing it again
puts it away, so a page left open on a screen does not keep showing a live
credential because somebody looked at it once.

**Rotate key** mints a new one. What it reports afterwards has two halves with
different consequences, and it says both: which configuration files on this
machine carried the old key and were rewritten with the new one, and that anything
configured elsewhere needs the new stanza. See
[The two credentials](#the-two-credentials) for what rotation does to a client
that is mid-session.

**Connected** lists the clients talking to this daemon right now: the client's
own name from the MCP handshake, its transport, which protocol revision it
negotiated, and how many queries it has run.

**Tools exposed** lists every tool the endpoint serves, each with the description
from its own definition rather than a second copy written here. Above the list is
the thing agents pay for and nobody thinks to measure: the size of the tool list
itself, in tools, bytes and approximate tokens, read once per session before the
agent has asked anything. It is measured from what this daemon is serving right
now, so it cannot go stale on the page.

**The stanzas** are grouped by Terminal, Editors and Desktop, with a chip per
client. Each client gets one configuration block, not two: a client that can reach
the HTTP endpoint is shown the endpoint, and one that has nowhere to put a header
is shown the subprocess form it can run instead. Printing both would leave a
reader choosing between two answers with nothing to choose on.

By default every stanza names `${SEMLITH_AGENT_KEY}` rather than the key itself.
That is what a client should carry: it survives a rotation with no file rewritten,
and the key stays in one file with one set of permissions. Pressing Reveal
substitutes the literal value, for pasting somewhere that cannot read an
environment variable.

These are the same stanzas the README documents — they are parsed out of it and
executed by a test, so the portal shows text that is known to work rather than
text somebody typed twice.

**This page reads live.** The clients domain moves when one connects,
disconnects or runs a query, so **Connected** and its query counts follow an
agent working in another window without a reload.

## Cloud

**What it is for.** Saying what the hosted option is, without leaving the
portal to find out. It describes a service; it is not a client for one.

**This page contacts nothing.** The pill beside the heading reads `not
connected`, and that is not a state this build can leave: there is no
`semlith cloud` command in this release, no token store, and no code path that
opens a socket to any host. The page is three reasons, two command blocks and a
link.

The paragraph under the heading states what the service is: *Semlith Cloud is
one hosted store for a whole organisation: every connected repository a root of
it, indexed on push, served over MCP to any agent, with a pull-request impact
check and a team ledger. This binary works without it. Nothing here contacts a
server.*

**The three reasons**, each a line and its explanation:

| Reason | What it means |
|---|---|
| One URL for cloud agents | Claude Code cloud sessions, routines and CI cannot reach a laptop; they can reach an org store. |
| The whole organisation in one index | Cross-repository paths, and documents beside code. |
| A pull-request check that states what the graph proves | With no model and no guess. |

**The two commands are text to copy and nothing more.** Neither exists in this
release, and each says what it will do when it does, rather than being shown as
though it worked:

- `semlith cloud login` — *Not in this release. When it arrives it will store an
  org token under `~/.semlith/` and send it to that host and no other.*
- `semlith cloud connect acme` — *Will add the org's store to this machine's
  registry as a remote store, listed beside the local ones with a remote badge.*

Under them the page says it again, because a page of commands is read as a page
of things that run: *This build has no cloud command and opens no connection to
any host. The Privacy page's own reading is where to check that rather than take
it from here.* That is the deliberate omission worth naming — the claim is
checkable on [Privacy](#privacy), which reads this machine, and not here, which
only states it.

At the foot is one link, **What the cloud stores and deletes**, to
`semlith.com/data`. It is the only external link on the page, and following it
is the only thing on this page that reaches a network — your browser doing it,
not the daemon.

**This page reads nothing.** It has no route behind it, so there is no live
domain for it to follow and nothing on it can go stale.

## Privacy

**What it is for.** Checking the claim that nothing leaves this machine, rather
than believing it.

**The facts strip** states the bind address, that no CORS header is emitted
anywhere, that a foreign `Host` gets 400 before anything runs, that the page is
compiled into the binary, that there is no telemetry and no update check semlith
makes on its own, that the retrieval ledger records locally and how to stop it,
and where the model cache is and whether anything is in it. The pill in the
heading says whether `--airgap` is armed.

**Verify it yourself** is four copyable steps, in increasing order of how much
they prove:

1. Ask the operating system what this process has open. Only loopback should
   appear.
2. Watch every interface but loopback while you search. Nothing should appear.
3. Arm the refusal with `semlith start --airgap`; anything that would reach the
   network exits instead, naming what it refused.
4. Or pull the cable — the portal loads and searches with no network at all.

**Rules** is the part worth reading twice. Each row is a rule the binary enforces,
and under it is what *this daemon found when it checked* — a path, a file mode, a
count. A page that states a policy is a page; a page that states a policy and the
reading behind it is something you can disagree with.

From 0.18.0 a failing row also carries the command that repairs it, copyable, and
— where the daemon can do it safely — a **Fix** button. Safely is a property of
the repair rather than a judgement made per button: it narrows access rather than
widening it, it is idempotent, it touches only a path semlith owns, and it is
confirmed by re-running that rule's own check, so a row turns green because the
daemon looked again and not because a click succeeded. What it changed and what it
was before are both reported — `0700, was 0755` — so it can be undone by hand.

Six of the ten rules hold by construction and have nothing to fix: they are
enforced in `http::answer` before any handler runs. Of the four that are readings
of this machine, `directory modes` and `agent key` are a `chmod` that removes
group and other bits; `model cache` is the same where the cache is yours and no
button at all where another account owns it, and the row says which case it is;
and `private addresses` gets the manual step and no button, because it fails on a
variable in the environment the daemon inherited and no process can unset a
variable in its parent's. On Windows the two mode rules have no reading and no
repair, and the row says so rather than showing a tick it has not earned.

The button posts to the engine `semlith doctor --fix` calls. Neither surface has a
repair the other lacks.

**Scan** is the rules pointed backwards, at what the stores are already holding.
The rows above say what semlith would refuse today; this says what it took in
before those rules were what they are — a file indexed before the credential
deny-list widened, or before the content scan existed at all. Press **Scan** and
the daemon runs both halves of the decision over every file every open store
holds: the deny-list against the file's name, and the credential shapes against
the text the store is actually keeping. It is the same `Semlith::scan` that
`semlith scan` calls in a terminal, because two implementations of "what should
not be here" would eventually disagree and the one that mattered would be
whichever you did not run.

Each row is the store, the path, and why it would be refused — the rule, or the
kind of credential and the line it sits on. It is never the text that matched:
the route does not return it, and a page whose subject is what stays on this
machine would be a poor place to reprint a secret in order to report that one
was found.

**Forget** on a row drops that file's chunks and vectors from its store, and
**Forget all** does the whole list; both confirm first, both post to the same
route the Files page's Forget uses, and the file on disk is untouched either way.
A list spanning two stores becomes two writes, because a write names the store it
is for.
Afterwards the scan is run again from scratch rather than the row being spliced
out, so the list you are left looking at is a fresh reading rather than the
assumption that the write did what it was asked. When nothing is found the card
says so, which is a different statement from a card that has not been pressed.

This is not a rule row and has no Fix button, and that is the distinction worth
keeping: the rules are readings of this machine that the daemon can repair, and
this is a list of files whose removal is a decision about your corpus. There is
also no MCP tool for it — an agent is not the party that decides what a store may
hold.

**The one outbound connection that exists** is the embedding model, downloaded
once on first index and cached. `semlith upgrade` and `semlith add` reach the
network only in the second you ask them to. `--airgap` refuses all three and exits
naming what it refused.

**Session token.** Shown truncated, with a **Rotate** button. See below for what
rotation does; the short version is on the page itself, including the sentence
that matters most — this is the portal's own token, not the agent key, so nothing
needs reconfiguring.

**Content-Security-Policy** prints the policy every response carries, and the line
under it names the three `Host` values that are answered. Everything else gets
400.

**The rules read live.** A repair moves the privacy domain in the one function
that applies one, so a `chmod` run in a terminal or a `semlith doctor --fix` in
another window turns the row here without a reload. The rest of the page is a
reading of the binary rather than of a changing thing, and a **Scan** is a list
you asked for; neither is refetched on a timer.

## Doctor

**What it is for.** Whether each agent client on this machine can reach semlith,
and what to run for the ones that cannot — so the next person who hits an
unregistered client reads the answer instead of bisecting a configuration file.

**Rules first, then the clients.** The rules answer the question the page exists
for — is this machine set up the way it claims — and the client table is
twenty-seven rows that page ten at a time, so under the table the rules were
below a screenful on every visit.

**Clients** is one row per documented client: its name, whether semlith is
registered in it and at what scope, and the command that fixes the row when it
needs fixing. Four states are kept apart on purpose. *Not installed* is a client
whose CLI is not on this machine, which is not a fault — most people have two or
three of the twenty-seven. *Installed, not registered* is one that would register
if asked. *One project only* is the defect this release exists to end: semlith
registered for the directory somebody was standing in. And *cannot register* is
one of the three — Crush, Zed and Roo Code — that document no user-level
configuration path at all, where the page prints the reason rather than a repair
it cannot offer.

**Rules** is the same four measurable Privacy rules, with the same manual step
and the same Fix button, read from the same function. The page and
`semlith doctor` in a terminal cannot disagree about whether a client is
registered or a rule holds, because they call the same two functions.

Nothing on this page runs a client's CLI. The registration state is read out of
each client's own configuration file, because asking sixteen command-line tools on
a route the portal loads every time is sixteen processes per page load.

## About

**What it is for.** What this binary is.

One card of facts: the version and the store format version, the binary's path,
size and target triple, what it is bound to, where the store home is, where the
model cache is, and how long it has been up with its process id. The MCP protocol
revisions the server speaks are a wire contract with an agent client rather than
something a reader can act on, so they are not shown here; `GET /api/about`
still returns them, and `docs/compatibility.md` says what dropping one means.

**Languages** is every name `--lang` accepts, with a mark on the ones the code
graph is extracted from. It lives here rather than on a page of its own because it
is a fact about the binary, and the search filter and the graph read the same
table, so the two cannot disagree about what a language is.

**The embedding models a store can be built with are not shown here.** A table
of forty-eight rows, of which any given machine has fetched one, is a catalogue
rather than a fact about this binary, and the v4 design has no place for it on a
page that is seven facts and a language table. `semlith models` prints the full
list — name, dimension, size where it has been fetched, and a note — and
`GET /api/models` still answers for anything that reads it. This is the one
capability in the product with no view in the portal, and it is argued in
`tests/portal.rs` and recorded in `docs/compatibility.md` rather than assumed.

The line that matters operationally stays: the model is fixed when a store is
created, because vectors from two models are not comparable. Switching means
deleting the store and indexing again.

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

There is no `semlith graph build`, and its absence is a feature. Extraction is
spliced into the same pass that re-embeds a changed file, after the old rows are
deleted and while the new chunk ids are in hand. The pass that re-reads a file is
the pass that re-extracts it, so there is no artifact that can be stale.

### Locator

A **locator** is an answer that says where something is rather than what it says:
a path, a line span, the enclosing symbol and its kind, which lists found it, a
freshness flag and one line of text. It is the first stage of a two-stage flow —
locate, then read — and it is what keeps an answer cheap, because a list of
candidates is paid for by the line and only the candidate that was right is paid
for in full.

### Freshness

A hit is `fresh` when the file on disk still has the size and modification time it
had when it was indexed, and `stale` otherwise. The check is one `stat` per
distinct path and is conservative: a `touch` reads as stale. A stale span is still
served, with a note saying the lines below are what was read then — because the
honest answer is not "this is current" and not "there is nothing here", it is
"this is what I read, and the file has moved under it since."

### The two credentials

There are two, and what separates them is their reach. Neither can do the other's
job, and that asymmetry is the whole design.

**The session token** is the portal's. It is generated when the daemon starts,
lives in memory for that run, and opens the page and every `/api/*` route. It
reaches the browser once, in the printed URL, and travels from then on in a
`Semlith-Token` header — never a cookie, because every port on localhost is the
same site. It is shown truncated everywhere, and the only response that carries it
in full is the one that rotates it.

**Rotating it takes effect immediately.** The old token stops working on the next
request that uses it. The page that pressed Rotate keeps working because the
response hands it the new value and it uses that from then on; every other holder
of the old token — another browser tab that was opened from the old URL — stops.
Nothing needs reconfiguring, because no client stanza carries the session token.

**The agent key** is the agents'. It is a `sml_` prefix followed by 64 hex
characters from the operating system's random source, written to `agent.key` under
the store home, created on first read and returned unchanged forever after. It
opens `/mcp` and nothing else. No route outside `/mcp` may ever learn to accept
one, and a test asserts it — a credential that sits in a configuration file on
disk must not be able to rotate a token, adopt a store or start an upgrade.

It is persisted precisely because a client's configuration is written once and has
to stay valid across daemon restarts, upgrades and portal token rotations. A key
that changed per run would make every stanza stale on every restart, and semlith
can only repair one client's configuration file automatically; everything else
would need a person. It is never reminted implicitly: creating and rotating are
separate functions, so nothing can rotate by accident from the state of the disk.

The page never receives it for merely being open. `/api/agents` does not return
it, and the masked display is not a truncation of the real value. It arrives only
in the response to pressing **Reveal**.

**Rotating it does not cut off an agent mid-session.** The new key takes effect at
once, and the previous key keeps being accepted for a grace window afterwards, so
a tool call in flight completes and a client that has to be restarted has time to
be. The grace is bounded at both ends: it ends when the daemon exits, since the
previous key is held in memory, and it ends after fifteen minutes in any case — on
a daemon somebody leaves running for a week, "until it exits" would not be a grace
period but a second live credential, and a key rotated *because* it leaked would
stay valid for as long as the machine was up. Configuration files on this machine
that carried the old key are rewritten with the new one as part of the rotation,
and the page names them; anything configured elsewhere needs the new stanza before
the window closes.

Rotating one has no effect on the other. The Privacy page rotates the session
token; the Agents page rotates the agent key; each says which one it is holding.
