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
opens a directory picker, and the button carries what you typed into the Index
page rather than dropping it. There is a third button that skips to About, and a
copyable `semlith index ~/Documents/work` for doing the same thing from a
terminal.

Once one folder is indexed the navigation appears and the first-run screen is not
shown again.

### The navigation

Ten pages in three groups, plus About:

| Group | Pages |
|---|---|
| Workspace | Stores, Files, Index |
| Explore | Search, Graph |
| Operate | Agents, Ledger, Privacy, Doctor |
| About | About |

The address bar carries the page as a fragment — `#search`, `#graph` — so a
particular page can be bookmarked or reloaded. On a narrow screen the navigation
is a drawer that closes when it takes you somewhere.

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

## Index

**What it is for.** Reading a folder, or one fetched URL, into a store, and
watching it happen.

The daemon is the writer for every store it opened. This page does not index
anything itself: it puts a job on that store's write queue and streams back what
the writer reports, which is the same path `semlith_index` takes over MCP. That
is why a run can sit at `queued` — the watcher is mid-file, and one writer per
store is the rule that stops two passes corrupting each other.

**Controls.**

- **path** — the folder or file to index. The first-run screen carries a path
  into this field.
- **Choose folder…** — a directory picker rooted at your home. It and the URL
  card are mutually exclusive: offering both at once offers two answers to one
  question.
- **Add from a URL** — one https request for exactly that URL, a page, a PDF or a
  file. Nothing is crawled, no credential is sent, and what lands is written
  inside the store's own `downloads/` directory rather than into your working
  tree. It then goes through the same queue and reports through the same event
  stream, so there is no second progress mechanism to learn.
- **The store selector** — shown only when more than one store is open.
- **Start indexing** — becomes **Pause** while a run is on.
- **Stop** — appears only while a run is on.
- **queue depth** — how many jobs are waiting across every store.

**Progress** shows a percentage, a bar and a status line carrying files scanned
over files found, chunks written, and a chunks-per-second rate. The log beneath
it prints one line per file with the outcome that file got — `indexed`,
`unchanged`, `skipped`, `removed`, `refused`, `failed` — coloured so that a
re-index of an unchanged corpus reads as a wall of background with the handful of
real writes standing out of it. The run ends with a counted line: indexed,
unchanged, skipped, removed, failed where there were any, chunks, and the total
elapsed.

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

**The elapsed clock** sits beside the counts. It reads `00:00` from the moment you
press the button, and begins moving when the run actually starts — a run that
queues behind the watcher can be seconds from starting, and a clock that appeared
only then would look like a page that did nothing, while one that counted the
wait would be reporting time this run did not spend indexing. It ticks once a
second, and every `file` event
corrects it to the daemon's own measurement, so it is the run's elapsed time and
not the tab's: a browser throttles a background tab's timers to about once a
minute, and a clock that only counted here would be minutes short by the time
anybody looked at it. It keeps counting through a pause, because a paused run is
still a run that has been going this long, and it is not reset by a slice. It
freezes on `done`, `stopped` or `failed`, holding the daemon's total.

**What Stop does that Pause does not.**

**Pause** holds the run between files. The writer is still this run's — the lock
is not handed back, the store is left mid-corpus, and **Resume** carries on from
where it stopped. Nothing that has been embedded is touched.

**Stop** undoes the run. Everything it embedded is rolled back, so the store is
left exactly as it was before the run started, the bar returns to 0% rather than
filling, and indexing the same folder again begins from the beginning. The
confirmation says so before it happens. Pause is "wait"; Stop is "as though it
never ran".

One more event can appear in the log: `slice`, saying the writer has handed
itself back to the watcher and put the rest of this run back on the queue. It is
one run and one stream, and there is nothing for you to do about it.

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
| `vector` | The embedding matched — this chunk means something close to what you asked. |
| `keyword` | The terms matched — your words are literally in this text. |
| `graph` | Reached from a neighbouring symbol — this was not itself a match; the graph walked to it from one. |
| `image` | The picture matched the words. |

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
different part of the graph rather than for more of it at once. The page opens on
the busiest symbol's neighbourhood rather than on everything: the whole store
drawn at once is honest and unreadable.

- **The scope box** takes either. A value containing `/` or `.` is read as a path
  and scopes the graph to that part of the tree; anything else is read as a symbol
  name and centres the graph on it. Enter with the box empty returns to the
  overview.
- **The store chips** appear when more than one store is open and restrict the
  graph to the ones that are lit.
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

## Agents

**What it is for.** Connecting an agent to this daemon, and seeing which ones are
connected.

**The endpoint** is in the page heading, with a copy button, a state pill reading
`answering` or `closed`, and a **Start** / **Stop** control. Stopping it drops the
`/mcp` route and nothing else: the stores stay open, the watcher keeps running,
and the portal keeps working. A closed endpoint answers 404 rather than 401, so a
client that has been told to stop learns the same thing whether or not it still
holds a key.

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

## Ledger

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
is intact.

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

## Doctor

**What it is for.** Whether each agent client on this machine can reach semlith,
and what to run for the ones that cannot — so the next person who hits an
unregistered client reads the answer instead of bisecting a configuration file.

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

**Models** lists the embedding models a store can be built with, each with its
dimension, its size on disk where this machine has fetched it, and a note. A model
this machine has never fetched shows `—` rather than a guessed size.

The line under it is the one that matters operationally: the model is fixed when a
store is created, because vectors from two models are not comparable. Switching
means deleting the store and indexing again.

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
