# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Agents use semlith, and it answers them well

A benchmark of 216 headless Claude Code sessions on 2026-09-25 found that the
setup `semlith setup` wrote got 7 % of an agent's lookups, and that forcing the
agent onto semlith cost answer quality, because the tools failed on exactly the
questions agents ask. This release fixes the tools first, then the levers that
bring the agent to them.

**What breaks if this changes.** `semlith_impact` walks from a definition, not a
name: `Fleet::search_preferring`, `module::function` and `Type.method` resolve to
the definition that owner holds, each reaching row carries the call-site line,
and the answer stays under 16 000 characters at any depth — past the cap, rows
collapse into per-file counts with a `more:` line. The same question at the
default depth was 75 012 characters on 0.29.0, over Claude Code's MCP limit.
The Rust extractor now records a method's owner and a receiver's declared type
(`self` in `impl T`, `x: T`, `T::new()`, a field `store: T`), so the two
`search_preferring` methods are told apart; callers of a name with several
definitions say which one they mean (`→ Fleet::search_preferring`).
`semlith_neighbors` and `semlith_symbol` take the same cap.

**Answers an agent can act on.** Paths in answers are relative to the store
root, named once at the top, and `semlith_read` resolves them — `src/lib.rs`
no longer matches the fixture copy. A locate row is one line and names the
definition a chunk is about with its start line (`search_preferring method
@3295`). `semlith_read` with a symbol name returns each definition whole, and
serves an edited file's current lines from disk, marked, through the same
secret scan. `semlith_symbol` takes up to 20 names and answers with one row per
definition. `semlith_files {tree: true}` gives a directory view with counts,
languages, per-file lines, symbols and first definitions, and what is on disk
but not indexed, and why. `brief` gives its one text span to code for a code
question, labels the others honestly, and leaves Markdown headings out of its
graph lines. Tests rank below product code unless the question names tests,
identical copies collapse into one hit, and `semlith_stats` collapses its long
tail. A `git checkout` no longer marks unchanged files stale. A `.semlithignore`
leaves paths out of a store; this repository's leaves the retrieval corpus copy
out.

**Bringing the agent to them.** `semlith setup` sets `alwaysLoad` on Claude
Code's entry, writes a hook that reads `Bash` as well as `Read`, `Grep` and
`Glob` and names one concrete semlith call (soft by default; `--hook-mode gate`
or `hard` opt-in), a rewritten skill, and a read-only `semlith-explorer`
research agent. The server instructions name the indexed folders and route by
question. `semlith doctor` reports all of it, and `doctor --fix` clears a
per-project disable for the current directory.

### The secret scan tells test dummies from keys, and says what it did not index

A match that a declared rule says is a test dummy — a published documentation
example, a body saying `EXAMPLE` or `FAKE` or one character repeated, a
private-key header with no key — no longer refuses its file; one live-looking
match anywhere still does. A secret-sounding name assigned a random literal,
quoted or not, now refuses its file too, which catches an AWS secret access key
beside its id. Every file that was not indexed is listed, per store, in five
classes, and a secret carries a 0–100 % estimate that it is real with the
signals behind it. A person — never an agent, never in bulk — can accept one
file at a time, redacted or as-is, from `semlith refused accept` or the portal's
Files ▸ Not indexed tab; the acceptance remembers salted fingerprints, never
values, and a new secret refuses the file again. A credential file is never
acceptable. Every run opens with a model-free scan phase and, when a person
started it, stops for review only when something is theirs to decide.

### Fixed

- The remaining-time estimate no longer jumps to minutes while a run writes its
  index, and never more than doubles between polls. Closes #143.
- `a_port_something_already_holds_is_seen_as_held` no longer races the rest of
  the suite. Closes #141.
- The daemon's watcher said `0 indexed at startup` for a catch-up it had only
  queued; it now says the catch-up was deferred, then what it found.

## [0.29.0] - 2026-09-25

### Eleven portal fixes found using 0.28.0

Using 0.28.0 day to day turned up eleven places where the portal said the wrong
thing, hid a control or made a common task slow. This release fixes those and
nothing else. Nothing in ranking, chunking or embedding changed. Stores and
settings are unchanged, so upgrading and downgrading need nothing.

**Index run cards count down.** A clock counting up from the button press told
someone watching a run how long they had waited, not when it would be done. The
index pass now measures each file it will open, one `stat` per file, and reports
bytes done against bytes total; a file being embedded counts in proportion to
its chunks. The daemon keeps a bytes/s rate over the last 10 s of active time,
and `/api/index/runs` carries `eta_ms`, `bytes`, `bytes_total`, `started_at` and
`finished_at` beside `submitted`. `eta_ms` is null until 5 s of embedding has
been seen and whenever the run is not running. A running card reads `about 3
min left`, `about 40 s left` or `almost done`, and `estimating…` until the rate
settles. A queued card reads `waiting 12s`. A finished card reads `took 5m 18s ·
finished 21:35`, timed from the run's start rather than its submission, without time paused, and
names any time spent queued separately (`· queued 1m 02s`). The "N runs queued."
note moved into the button row beside Start indexing and empties to nothing, so
a finished run no longer leaves a blank line above the cards.

**Stores can be deleted together.** The Stores table has a checkbox column, a
select-all per page, and a bar with **Delete N stores** whose confirmation names
each store. `POST /api/store/delete` takes `{"stores": ["a", "b"]}` as well as
`{"store": "a"}`. Each store goes through the same removal, and a store that
could not be deleted is named with its reason while the others still go. The
list form answers `{deleted, failed: [{store, error}], message}`: 200 if any
store went, and 409 with an `error` naming why when none did. The result note
shows above the table rather than at the foot of the page. The table's "See
what is actually inside the index" link opens Inside the index again; it had
pointed at Index since 0.27.0 split the page.

**Impact says what it shows and where to start.** The page keeps its layout. A
three-line guide under the figures says what reached, inferred and hops mean. An
empty page says how to get there from Graph and has an **Open Graph** button.
Arriving from Graph's **Blast radius**, the page answers in the store the symbol
was picked in, shown as an `in <store> ×` chip that clears the scope, and the
path finder and trace use the same scope. The Where column truncates, so Via
and Hops stay in view.

**Graph's rail keeps its buttons in view.** "Chunks it lives in" and "Blast
radius" were sticky at an offset that left them 18 px below the visible edge
once the side column became the scroller. They now sit in a footer of the side
column that does not scroll. A long symbol name ends in an ellipsis, with the
full name on hover, rather than breaking mid-word. The rail's lists were
centred at the width of their longest row, so a long caller spilled off both
sides of the panel and it scrolled sideways; they now fill the rail and every
row truncates.

**Smaller fixes.** Every paginated table opens at 5 rows per page; tables used
10, 12, 15 or 25, and the ledger's 12 and 15 matched no page-size button. The
Stores table keeps its page, page size and sort when a watcher write redraws the
page; each redraw used to put the reader back on page 1. On
Search, the hit count and timing moved out of the query box to the line under
it. In the Brief view, each span with text is drawn as the results view draws a
hit: a span card with a path header, line-numbered code and the lists that
found it. Spans the budget left without text are compact rows, and the token
footer is a strip of facts: tokens of budget, spans, counted with, and dropped.
On Agents, both rows of cards share one two-column grid and collapse to one
column at the same width, 899 px. On the Machine limits card, a lane's Remove
downloaded files button sits under the lane's name, closer to its lane than to
the next.

## [0.28.0] - 2026-09-24

### The service indexes 4.6× faster, the GPU works beside the CPU, and the run controls act at once

On the reference M1 (4 performance and 4 efficiency cores, 8 GB), the login
service indexed at 3.3 chunks/s while the same binary in a terminal did 28,
measured on 2026-09-23. Every throughput figure in the documentation had been
taken in a terminal, so nothing measured the path most users are on. Two of the
three machine limits the Index page offered were saved and never applied.
Watcher catch-ups ignored the concurrency limit, and Pause and Stop could take a
minute to act.

**Priority now follows the work.** The launchd plist asked for `ProcessType
Background`, which on Apple silicon keeps a process on the efficiency cores. A
process launchd starts that way cannot lift itself out: `setpriority` returns 0
and the scheduler priority stays at 4. The plist now asks for `Standard`, and the
daemon switches its own priority. One process-wide count covers every embed
pass: a portal run, a watcher catch-up, a watcher batch, `add`, and the query
embedding behind a search. When the count goes from 0 to 1, the daemon clears
its darwin background state. When it returns to 0, the daemon waits 300 ms and
sets the state again, so a burst of searches does not toggle it. Each switch is
logged with its direction and how long it took, which was under 100 µs in both
directions on the M1. `/api/about` reports the current state and the number of
switches. Every HTTP request lifts the daemon too, so Pause, Stop and a limit
save answer at once on a machine that is busy with something else. Only a
request that goes on to embed is logged; a portal page polling once a second
would otherwise write two lines a second to the log.
Measured on the M1 over the pinned corpus (the `src/` of v0.27.0, 3 746
chunks), three interleaved rounds, median: a portal run through the service on
the CPU alone ran at 15.1 chunks/s, 4.6× the 3.3 of 0.27.0's service, against
24.7 for `semlith index` in a terminal in the same rounds. That is 61 %, short
of the 85 % this release aimed at. The same binary started with `semlith start`
from a terminal reached 89 %, so the rest is how launchd schedules an agent's
threads on Apple silicon: they sit at priority 20 against 31 from a terminal,
and neither a QoS request nor `ProcessType Interactive` changed the rate. The
release record keeps it as an open question.

This trades some battery for speed. While it embeds, the daemon runs at full
priority on the performance cores, where before it ran on the efficiency cores
whatever it was doing. While idle it is in background state, as the old plist
intended. An idle daemon uses no CPU, so being idle costs nothing either way.

`semlith setup` and `semlith upgrade` rewrite an installed plist that still
says `Background`. `upgrade` also re-registers the login service, so the new
binary is what runs. Until the plist has been rewritten, `semlith doctor`
reports `FAIL service priority` and names the file. On Windows the logon task
is registered at `-Priority 5`. It used to run at Task Scheduler's default of 7,
below normal, and on Windows 11 that can bring EcoQoS throttling onto the
efficiency cores. While idle, the daemon sets itself to `BELOW_NORMAL` with
power throttling on. While embedding, it runs at `NORMAL` with power
throttling off. `setup`, `upgrade` and `doctor` treat a task at any priority
other than 5 the way they treat the old plist. The Linux systemd unit does not
change. An unprivileged process cannot lower its nice value again once it has
raised it, so Linux has no idle switch.

**Machine limits apply as soon as they are saved.** A new *threads each* value
reaches every writer at its next batch. A writer whose session was built with a
different thread count rebuilds the session there. Each run card shows the
thread count its session was actually built with. A new *MiB per store* value
applies to every open index at once, and any resident shards above the new
budget are released. Raising *runs at once* admits waiting runs at once, as it
did before. Lowering it now holds the newest runs above the limit at their next
batch and puts them at the front of the queue as `held`. They keep their list
of written files, so a later Stop still undoes everything they embedded, and
they resume in submission order as slots free. The environment variables keep
their precedence. The route's reply states the values the engine now runs with.
The limits line `semlith start` prints shows the values in force, so it no
longer says "1 embedder thread(s) each" while every session runs 4. A derived
*threads each* is split between the runs embedding at that moment rather than
the most that could be: a run going alone gets every thread, as it would in a
terminal, and each run rebuilds its session at its next batch when another
starts or ends. A saved or environment value is used as given.

**Values saved under 0.27.0 take effect now.** In 0.27.0, *threads each* and
*MiB per store* were written to `~/.semlith/settings.json` and then ignored.
After the upgrade they are applied. A *threads each* of 1 saved earlier now
limits every writer to one thread, on a machine whose writers were running 4.
If indexing is slower after the upgrade, check the Machine limits card, or
remove the field from `settings.json` to go back to the derived value.

**Everything the daemon embeds goes through one queue.** A watcher catch-up, the
walk of a whole root when a store is opened, is now admitted like a portal run.
It has a card of its own on the Index page, with kind `catch-up`, Pause and
Stop. A watcher event batch of up to 32 files still runs straight away, because
a saved file has to reach search within seconds. A larger batch, such as a `git
checkout` that touches thousands of files, is admitted as a run of kind `batch`.
With *runs at once* at N, no more than N runs embed at the same time, apart
from the event batches of 32 files or fewer.

**Pause and Stop act at the next batch.** The run checks its control before
every embedding batch, inside a file as well as between files. Before, it
checked once per file, and a file can hold thousands of chunks. Pause holds the
unflushed batch in memory and commits nothing partial. Stop inside a file
removes that file's rows along with every file the run wrote. The file's hash
is written only after its last chunk is durable, so a half-written file is
never recorded as indexed. The undo now removes everything and then writes the
index once. Before, it called `forget_held` once per file, and each call
rewrote the whole index. `POST /api/index/control` replies at once with the
requested state, `pausing` or `stopping`. The card shows that state as soon as
the button is pressed and changes to `paused` or `stopped` when the engine
confirms it.

**Stop can delete the store.** The Stop confirmation has a checkbox, "Also
delete the store". It is ticked by default when the store held no files before
the run, and unticked when it held content. The daemon records the pre-run file
and chunk counts on the run (`files_before`, `chunks_before`), so the page does
not have to guess them. With the box ticked, the undo is followed by the same
removal `semlith drop` performs: the store directory, its registry entry and its
watcher thread. The card then says the store was deleted. On Windows, an open
handle stops a directory from being removed, so the store directory is first
renamed out of the way. If the rename is refused, nothing is deleted and the
error names the directory. A partial delete does not happen.

**The chunks/s figure is a rolling rate.** A run reports `rate`, the chunks it
embedded over the last 10 seconds of active time, and `rate_average`, the rate
since its first batch. Paused and held time is excluded from both, as is the
time spent queued or walking unchanged files. `rate` is null only before the
first batch, and it is still reported while the index is being written to disk.
Watcher batch lines report how long the batch actually took. Every catch-up
used to print "in 0.0s".

**Idle sessions are released, and all readers share one query session.** ONNX
Runtime's arenas grow to the largest batch a session has run and never shrink,
so a writer that indexed once kept that peak for as long as the daemon ran. On
the M1 that came to 5 392 MB across seven stores on an 8 GB machine, measured
with `footprint` on 2026-09-23. A writer now drops its session, its arena and
its threads after 60 seconds without embedding, and loads them again on the
next embed, which takes about a second. The portal's reader and the MCP reader
used to hold one query session each. They now share one per model, and it stays
loaded, so a search never waits for a model load. `/api/about` reports
`sessions.writers` and `sessions.query`.
Measured with `footprint` on the M1, seven stores open, with GPU on: 452 MB
idle, 888 MB during a portal run of the pinned corpus, and 465 MB 60 seconds
after it ended, with no writer session loaded. 0.27.0 held 5 392 MB.

**Chunks are batched by length.** A batch pads every text to the length of its
longest one. The index pass used to embed chunks in file order, eight at a
time, so one long chunk made seven short ones as expensive as itself. It now
holds a window of up to 64 chunks, sorts it by length, and embeds it in batches
of 8 on the CPU, 16 on WebGPU and 64 on CUDA. The window is formed in walk order and sorted stably, so the same
corpus on one lane always produces the same batches. Sorting changes each
chunk's padding, so int8 vectors can differ in their last bits from the ones
0.27.0 made. On the M1, the pinned corpus (the `src/` of v0.27.0, 3 746 chunks)
on the CPU alone ran at 27.6 chunks/s sorted against 23.5 unsorted in the same
binary, median of three: 1.17×. A standalone script had measured 19.5 against
29.1 before the work began, but the unsorted path already embeds eight
neighbouring chunks of one file at a time, and those are close in length, so
there was less padding to remove than the script suggested. The release record
lowers the criterion from 1.3× to 1.1× and says why.

**The GPU works beside the CPU, on by default.** Each accelerator is a *lane*.
The CPU lane is the run's own in-process session. A GPU lane is a worker process,
`semlith __embed-worker <lane>`, which exchanges length-framed batches with the
daemon over stdin and stdout, and one worker per lane serves every run. Every
enabled lane takes the next sorted batch when it is free, so a faster device
ends up doing more of the work with no tuning. A driver crash kills the worker
and not the daemon. The worker's batch goes back to the queue for another lane,
the lane is marked `failed` with the reason, and the run finishes with the same
files and chunks. A batch that takes longer than 30 seconds fails the lane.
A worker that has had nothing to do for 60 seconds exits, and its GPU memory is
freed when it does.

- **WebGPU** runs on Metal on macOS, D3D12 on Windows and Vulkan on Linux,
  through Microsoft's WebGPU plugin execution provider 0.4.0. The plugin comes
  from its PyPI wheel, which is 5.2 to 13.1 MB depending on the platform. It
  runs the fp16 export of the same granite model at the same pinned revision,
  97.6 MB, because the int8 graph does not load on WebGPU. Only a hardware
  adapter is used. The following software adapters are rejected by vendor and
  name before anything is downloaded or any session is created: Mesa lavapipe
  and llvmpipe, SwiftShader, Microsoft WARP and the Basic Render Driver.
  lavapipe aborts the process on any MatMul. On the M1's Metal, at batch 16
  with length-sorted batches, the lane embedded 50.6 chunks/s. The CPU path of
  the time did 19.5.
- **CUDA** is available on x86_64 Linux in this release. It stays off until it
  is turned on explicitly, because turning it on downloads a 1.89 GB pack. The
  pack is Microsoft's ONNX Runtime 1.24.4 GPU build and NVIDIA's CUDA 12.8,
  cuBLAS, cuFFT, cuRAND, NVRTC, nvJitLink and cuDNN 9 libraries, all pinned by
  digest. A library the system already has at a compatible version is used
  from the system instead. The card is read through NVML, which is loaded at
  run time. A driver older than 525.60.13 is reported as too old and the card
  is not used. On Windows, an NVIDIA card is used through WebGPU on D3D12.
- **Switches.** `semlith accel [status | on <lane> | off <lane> | remove
  <lane>]`, the new Accelerators section on the Machine limits card, the
  `accelerators` key in `settings.json` and `SEMLITH_ACCEL` (for example
  `cpu,gpu`) all control the same lanes. `SEMLITH_ACCEL` takes precedence, like
  the other limit variables. By default CPU and GPU are on and CUDA is off. A
  change reaches every run at its next window of chunks. The CPU can be turned
  off only while a GPU lane is on and usable. With no usable GPU the CPU keeps
  indexing, and the card says it is the fallback. Turning a lane off never
  deletes what it downloaded. `accel remove` deletes it and reports how much
  space that freed.
- **Where each vector came from.** A store keeps a count of the chunks each
  variant embedded (`int8-cpu`, `fp16-webgpu`, `fp16-cuda`) in one meta row.
  `semlith stats` prints it as a `variants` line, and `semlith_stats` lists it
  along with the lanes that are on. Tested on the 2026-09-23 corpus, int8 and
  fp16 vectors of the same text agree at cosine 0.987.
- **Lanes are checked before they are trusted.** Before a lane's first real
  batch, its worker embeds 32 fixed chunks and compares them with CPU fp32
  vectors committed under `tests/fixtures/gpu/`. An fp16 lane with a cosine
  below 0.999 on any chunk is refused. `semlith doctor --gpu` runs the same check
  on every lane the machine has and prints the device, the variant, the lowest
  cosine of the 32 and the chunks/s. On the M1's Metal, WebGPU scored cosine
  1.0000 against CPU fp32 on all 32 chunks, and the CPU int8 lane scored
  0.9851.
- Only the daemon uses GPU lanes. `semlith index` in a terminal and the
  retrieval harness run on the CPU alone, which keeps their results
  reproducible (#88).

With the default settings, CPU and WebGPU, a portal run through the service
indexed the pinned corpus at 36.8 chunks/s, median of three interleaved rounds:
1.52× the unsorted CPU path of the same binary, with the card showing `GPU
22.4/s · CPU 11.6/s`. The two lanes share the M1's four performance cores, so
neither reaches the rate it has alone (50.6 and 29.1). A GPU lane now keeps two
batches queued, because the writer used to hand it the next one only between
its own CPU batches; that alone took a hybrid run from 25.8 to 31.6 chunks/s.
A store embedded by both lanes holds int8 and fp16 vectors side by side, which
agree at cosine 0.987. The sealed split of 30 questions, CPU lane alone, median
of three: all int8 24/27/29 at hit@1/3/8, the 0.25.0 figures exactly; all fp16
25/27/28; half and half with int8 queries 25/27/28; half and half with fp16
queries 25/26/28. The mix is within one question of all-int8 at every k, so
mixing is allowed and queries stay int8. All 11 identifier questions stay in
the top three in every arrangement.

**A reader panic fails one file, not the store.** `raw_text_tag` sliced a
`str` at a byte offset, which panicked on a multi-byte character directly after
a four-letter tag (`<abbré`). That panic killed the store's writer thread. The
function now compares bytes. The index pass also catches a panic during one
file's extraction and reports that file as `failed` with the panic message. A
writer thread that does exit sets `watching` to false, and `/api/stores`
reports why in `stopped_because`.

**The hybrid-core thread count covers Linux and Windows.** On Intel hybrid
CPUs, the default thread count is the number of performance cores. Linux reads
it from `/sys/devices/cpu_core/cpus`, and Windows reads the cores in the
highest `EfficiencyClass` from `GetSystemCpuSetInformation`. Any other machine
keeps `available_parallelism`, as before.

**Fixed.** Concurrent `POST /api/index` requests for a new folder could answer
409 with `SQLITE_IOERR_SHORT_READ` or "duplicate column name" (#132). Store
creation is now serialised, and a store the daemon already serves is reused
rather than opened again. Opening a store retries a busy or short read for up
to 2 seconds. Schema upgrades run under the write lock. `registry.json` and
`settings.json` are written through a temporary file unique to each save,
rather than one per process that concurrent routes overwrote.
`tests/doctor.rs` closes its fake client before running it, so Linux can no
longer fail the test with `ETXTBSY` (#135).

**Fixed, found while building this release.** On Linux the watcher counted its
own reads as changes: every file the index pass opened raised an inotify
`IN_OPEN`, which queued the file again, so an idle store kept re-reading its
folder and spent 3 to 6 seconds of CPU a minute doing nothing. Access events
are no longer changes. A stopped run's queued watcher events are dropped with
the run, where before the watcher re-indexed what the stop had just undone and
a stop with the delete box could time out waiting for it. `/api/stores`
reported zero files and chunks when the semlith home was reached through a
symlink, because the first fleet opened stores under a path the registry did
not use. Deleting a store now removes its registry entry before announcing the
change, so a page re-reading the store list on that announcement no longer
sees the deleted store. `tests/setup.rs` ran `setup --yes` without
`--no-service` and replaced the developer's own login service with a temporary
binary; those calls now pass `--no-service`, and `service install` refuses a
binary under the temporary directory.

**The Privacy route lists every download.** `/api/privacy` returns a
`downloads` list. Each entry gives what is downloaded, where from, its size,
when it happens and whether it is already cached. The list covers the
embedding model, the WebGPU plugin with the fp16 model, and the CUDA pack.
`--airgap` refuses all three unless they are already in the model cache.
Search through an idle daemon costs what it did. Measured side by side on the M1
over one 6 993-chunk store, three rounds of twenty searches through
`/api/search` with half a second of idle before each, so every search carries
its own priority lift: 0.27.0 402.0 ms, 0.28.0 401.9 ms at the median. The
37.9 ms in the 0.25.0 notes was taken in-process by `tests/measure.rs` on a
quieter day, not through the daemon, so it is not the comparison here.

### CI

A push that changes only prose, and a draft pull request, skip the slow lane:
the GPU and CUDA jobs, the release suites and the mixed-vector harness. A push
is compared with the previous head only when that head's run finished green, so
a run cancelled by the next push never lets code through untested. The slow
lane itself runs in parallel. Each release suite has its own macOS runner and
each harness run its own Linux runner, which cuts the longest job from 68
minutes to the longest single suite. `native-smoke` builds the binary once per
operating system and runs the portal check, the smoke harness and the browser
drive side by side, down from forty minutes in a row. The models are cached per
digest, `check` no longer links a second release binary, and `native-smoke`
cancels a run a newer push has replaced.

## [0.27.0] - 2026-09-23

### The portal drawn the way the design draws it, and reports that write themselves

0.26.1 made the portal's *structure* agree with the v4 design. What it did not
do is make its *content* agree: Reports was a stub against a six-section design,
Impact never drew its canvas, the ledger's bottom collapsed under a shared flex
rule, the body-copy class ran one and a half pixels hot on every page at once,
and a row of pressed chips read as tinted text rather than as six controls.

**`semlith start` against a store a daemon already holds is no longer an error.**
On a machine where the daemon starts at login this fired on the most ordinary
command there is, and printed an anyhow chain for something that had not failed.
It now says which store, names the holding daemon's pid, port and version, prints
the portal's full URL with its token on a line of its own so a terminal makes it
clickable, and exits 0. Only a *live, answering* daemon named by a valid discovery
file takes that path: with no daemon, a stale discovery file, or one naming a dead
port, today's error chain prints unchanged and the exit code is still non-zero. A
multi-store `start` serves the free stores and names the held ones.

**Reports schedules itself.** A schedule is a report the daemon writes on a
cadence, to a directory you name: report type, window, scope, format, toggles,
cadence, destination. It lives in `~/.semlith/schedules.json`, its own file beside
the registry rather than a section of it, so a schedules file somebody's editor
truncated costs them their schedules and not every store on the machine. The
cadence is stored as an interval in seconds, so the three chips on the page are
shortcuts over a number rather than the whole vocabulary. A daemon restart loses
nothing. Two schedules never generate at once, a schedule due while its store is
still indexing waits rather than reporting on a half-written index, and a
destination that has gone away — deleted, renamed, on a volume that is not
mounted — is recorded and shown rather than silently skipped, which was the worst
available outcome. `semlith schedule list|add|remove|set` reaches the same
schedules the page does.

**Reports is the page the design specifies.** The builder gains the `Window` and
`Scope` chip groups it has never had, a fifth format, and the three toggles; the
preview gains `Save to disk` and a meta line that states rows, size, format and
signature; and the `Retrieval savings`, `Names that mislead` and `Written
reports` cards are built. The CLI block moves out of the builder into its own
card. Where the design's mock and real data disagree — a `for whoever approves
the spend` card priced against a model this binary does not price, a cadence
chip for a git commit nothing hooks, a `schedules.toml` crontab that is neither
TOML nor a crontab — the real thing is drawn and the difference recorded.

**Two of the builder's three toggles do what they say.** `Hash the query text`
replaces every query with a digest of it *wherever a query reaches the document*,
not only in the table the toggle sits above — the who, the when, the tool and the
token counts all stay. `Attach retrieved excerpts` unrolls the access report's
session lines into the individual retrievals behind them, to a ceiling: a session
row says an agent asked forty times, and an auditor's next question is which
forty. The third, `Sign the report`, is drawn disabled and says what it is waiting
for. Signing needs a key, and where that key lives, how it rotates and what a
reader checks it against are decisions this product has not made; a switch drawn
as though it worked would put a promise on the page that the file does not keep.

**A fifth report format: PDF**, typeset from the same block structure the other
four render rather than printed from the HTML, by a pure-Rust writer that adds no
native build dependency and no runtime browser.

**`/api/report` takes a window and a scope.** It read a `store` parameter and
threw it away, so a request asking for one store of six got all six with nothing
saying so. `scope` is that filter, `store` is kept as its alias, and `window`
narrows to a day, week, month or quarter. Three of the five reports have no date
to narrow by and say so under their title rather than printing a span they did not
apply. A request naming neither is byte-identical to 0.26.x. `semlith report`
takes `--window`, `--scope` and `--format pdf`.

**Impact draws the canvas its caption promised.** The right column of that page
was a large empty area. It now carries a reverse-reachability canvas above Path
finder and Trace, drawn by the same force simulation the Graph page uses: the
subject selected, everything that reaches it around it, nodes that move, can be
dragged and carry the Graph page's hover card — hops, file and line, what they
reach through and with what confidence. The reached set and its files are tables
now, one group row per hop, instead of columns that wrapped. The `Changing` row
and the three figures move into a card at the head of the left column, where the
design has them, instead of two unboxed bands across the page. A label near the
edge of either canvas no longer runs out of the frame: the bound is half the
node's own measured width, not a margin sized for a dot.

**Index and Inside the index are two pages**, as the design draws them. Index
carries the controls that start and watch a run. Inside the index carries what
the corpus is — lines of code, words, the language mix by line, the shape of the
code, the indexing span, chunks by month, graph health and the vectors themselves
— measured from the store each time the page opens, and saying on the page where
what it can measure differs from what the design prints. Building it found that
the recorded query time read a column called `ms` where the column is `micros`,
so a store that had answered hundreds of queries said it had never been asked one.

**Session replay looks like the design in all three places it appears**: a dashed
panel on the ledger when it is off, with a button that goes to Privacy; a timeline
of one row per answer when it is on, carrying what the answer cost and what the
agent did next; and a switch on Privacy whose status line names the state it is
in rather than the state pressing it would reach.

**One hover card, everywhere.** Every hover in the portal — the Graph and Search
canvases, the Impact canvas, the language mix, the month chart, the edge tiers —
is the same card built by one function, in the design's measurements, and it
follows the pointer as the Graph canvas's always did. Keyboard focus still hangs
it off the focused element, which has no pointer to follow.

**One unreadable store no longer takes the readable ones down with it.** A store
whose database could not be read made every route that aggregates across stores
return `500 disk I/O error`, blanking Graph, Search and Files, with a message that
named neither the store nor the fact that the others were fine. They answer from
every store they can read now and return the rest as a `failed` list carrying the
store, its path, the whole error chain and a runnable remedy; the page draws what
the healthy stores returned with a notice above it naming what is missing. A
request scoped to a store that cannot be read still fails, because there is no
partial answer to give.

The readability probe is one indexed row out of each of the two large tables,
not a row count. The obvious probe is `stats()`, and `stats()` is three full
scans: measured at 2.65 ms against 0.24 ms on a 10 390-chunk store, growing
linearly, on a path that every search takes. It is not weaker for being cheaper
— what it has to catch is a database that opened and cannot be read now, and a
btree descent into a page that is no longer there fails exactly as a scan would.

The fix is not where the issue said it was. `with_fleet` is not the shared point
— Search and Files never go through it, because they need the fleet mutably — so
a guard there would have fixed neither. Every aggregating read selects its
members through one of two functions in `fleet.rs`, and that is where the probe
lives, which covers seventeen routes including every sibling the issue does not
name. The notice carries no button: the remedy is `semlith drop`, and a one-click
drop beside what may be an unplugged volume is how somebody loses a store they
could have had back by plugging it in.

**Pages scroll to their end.** `.view > *` set `flex-shrink: 0` and `.scroller`
put it back at equal specificity and later in the file, so on any page whose
content overflowed one child absorbed the whole overflow: the page stopped
scrolling and the scroller was crushed toward zero. It is fixed in the shared rule
— five pages put a `.scroller` directly under `.view` — and every page now ends
with the design's 44px floor under its last card instead of 24px.

**Controls read as controls.** The pressed chip takes the design's ink fill
instead of a blue wash, and stops changing font weight on press, so a chip row no
longer reflows when one is picked. `.button.ghost` — no background, no border,
muted text, and no counterpart anywhere in the design — is gone, and all fourteen of
its call sites take the secondary shape. The Impact page's two toggles come up from
11px mono at roughly 17px tall to the design's chip shape. Nothing in the portal
renders with neither a background nor a border at rest.

**Two columns become one on a phone.** `.grid.two` held both of its tracks at
every width, and its first track has a 320px minimum that cannot be honoured at
390px: the browser kept two columns and gave the second whatever was left. On
Reports that was about thirty pixels of preview setting one character per line.
About had the same defect and nobody had looked at it on a phone.

**Every page's panels are in the design's order.** All thirteen were walked
against the reference. Four already agreed and one — Doctor — has no design
counterpart at all, so the whole page is this project's own. Five were
interleaved: the ledger had its recording-off notice second from the bottom, so
a ledger recording nothing said so after everything it had failed to record;
Agents had four unrelated cards between the endpoint note and `Connected`;
Privacy had two between the promise and the check; Stores put four stacked
pickers between the page head and the numbers; and Inside the index had the run
cards fifth on a page whose whole point is watching a run.

**Scroll position survives a live repaint.** The ledger and the Stores table
redraw themselves as rows arrive, and the code that preserved the reader's place
across that redraw read `.scroller`, which never scrolls — `.view` carries the
overflow. It read 0 and wrote 0 back, returning the reader to the top every time
a row landed.

**Code blocks lose the fade.** A 28px right-edge mask stood on `pre.code` to say
"the line continues"; every block that class draws is a command you are meant to
select and copy, and the fade made its last characters unreadable.

**The type scale is the design's.** `.subtitle` carried nearly every muted
explanatory line in the portal at 14px against the design's 12.5px — one class,
and the most visible drift in the product. The design has three sizes for it and
all three are now expressed. The pass is symmetric: the shapes the design draws
*larger* are corrected too. Every declaration that moved is listed with its before
value, its after value and the design line that sets it; three the plan asked for
turned out to have no design line behind them and are recorded as not done, with
what the design actually says.

**Three layouts that fought the page.** The corpus page's new rules redefined
`.kv`, `.dot` and `.chips` globally: the Agents page's setup steps stacked and
centred, and the Privacy rule lights turned grey. They carry their own names now.
On Agents, `This machine` sits under `Tools exposed` and the registration card is
full width; on Privacy, `What is already stored` sits under `Verify it yourself`
and `Rules` spans the page, three abreast, instead of a narrow column several
screens tall. A two-column grid with cards after it no longer grows into a tall
window's spare height and pushes them a screenful down.

**The Stores page notices a root folder that has gone.** Deleting a store's
corpus writes nothing to any store, so the counter the page polls stood still and
the `root missing` badge appeared only on the next navigation. `/api/changes`
now compares which roots are on disk against the last poll — a `stat` per root, a
handful a second while a page is open — and moves the counter when one goes or
comes back.

**A saved limit is called saved on every branch.** The machine-limits panel
dropped the word from its explanation whenever the saved value sat above what the
machine would derive, which is exactly when free memory is low and someone is
trying to work out whether their setting took effect.

**About loses two blocks.** The `MCP revisions` row states a wire contract an
agent settles in its handshake and a person never acts on. The models table is a
forty-eight-row catalogue of which any machine has fetched one. `semlith models`
still prints the full list and both routes still answer; this leaves `/api/models`
as the one capability with no portal view, argued in `tests/portal.rs` and recorded
in `docs/compatibility.md`.

**The retrieval harness gains an image class.** Issue #122 — a source file
outranking an actual red circle for "a red circle" — survived a whole retrieval
release because nothing the release measured could see it: the pinned corpus holds
no images. The class has a second pinned corpus of its own, so the first one's hash
does not move and every figure read against it still reproduces. Eight questions;
seven reach their picture at rank 1 and one does not, which is left failing and
named rather than rewritten until it passed.


## [0.26.1] - 2026-09-21

### The portal, opened beside the design it was built from

0.26.0 shipped the v4 design's structure — its tokens, its thirteen pages, its
two nav groups — but the result was never driven end to end in a browser
against the reference file. This release is that pass. Every page was opened in
a real browser at seven widths in both themes, every control on every page was
clicked, and what follows is what that found.

**The first-run screen is complete.** It now carries the version beside the
mark, all four of the design's steps rather than three, the sentence that says
queries are recorded to a local file in that store and never leave the machine,
and a way to adopt a store that already exists — on the one screen whose whole
job is getting a first store open. Its footer names the address *and the port*
actually bound, because "loopback only" is a claim the reader cannot check
without the port. "Skip for now" lands on Stores, which is the page it is
skipping ahead to, rather than on About.

**The menu is four groups again.** v3 grouped the pages Workspace, Explore,
Operate and Account; v4 flattened that to two groups of six and seven, which
is one long list with two headings in it rather than a menu. The thirteen
pages are unchanged and so is their order within each group — what came back
is the grouping, with *Explore* holding the three pages you ask questions of
and *Machine* holding the two that describe what you are running, in place of
v3's *Account* and the licence page a free binary does not have. The Impact
and Retrieval ledger marks are v3's too.

**The routes the design draws between pages are there.** "See what is actually
inside the index", at the foot of the Stores table, which is the question that
table raises and never answered. "Build a report", in the Retrieval ledger's
header. "Chunks it lives in", beside "Blast radius" on the Graph rail, so
reading the text of a selected symbol no longer means retyping its name into
Search. And "Replay first-run screen" under the daemon card: without it, that
screen was reachable only by emptying the registry.

**The Graph page's right column was painting over itself.** The Map's own
height — one row per community — shrank the selection rail above it to a few
pixels in the flex column, and the rail overflows visibly by design, so a
selected symbol, its callers and its callees rendered straight over the map
underneath, with the rail's buttons stranded at the bottom of the page.

**Reports is the page the design draws.** One picker of five reports, a builder
for the one chosen, and the preview it produced, with Copy and Export on the
preview's own bar. It was five cards each carrying its own Generate and its own
row of four format buttons — twenty-five controls for five reports, above a
preview that could have been about any of them.

**`--model opus_5` priced at Sonnet 5 and said so.** The report generator
matched a model by its display name only and fell back to the first price on no
match, so a slug was accepted, ignored, and the resulting figure in money was
labelled with the wrong model. Model names now resolve however they are
punctuated, and a name this binary does not price is refused by `semlith report`
and by `/api/report` with the three that would have worked.

**The browser's own paragraph margin was a second spacing system.** Blocks
written as a `<p>` carried 13px above and below on top of whatever gap their
card already set, and blocks written as a `<div>` carried none — so the same
note sat differently on two cards for no reason anybody had chosen.

Also: the About page states which MCP revisions this binary speaks and the
licence it ships under, and its 46-language table is full width under the two
columns rather than inside one, which was cutting the model table beside it off
mid-row; the daemon card says whether the ledger is recording; Impact lays its
answer beside the path finder and Trace rather than stacked a metre wide; the
Cloud page is held to the design's measure and its reasons are a title over an
explanation; Privacy carries the fifth verification step, the one that reads the
ledger as what it is — a table in a file on this disk; the cards on Inside the
index are as tall as what they hold; a store's roots on a phone are truncated
from the front rather than broken into eight-character pieces down the cell; a
page header on a phone puts its buttons after the sentence that says what the
page is, not between it and the title; and the Stores strip says "read by 1
reader".

### Documented

`docs/portal.md` gains the three pages 0.26.0 added and never documented —
Impact, Reports and Cloud — and its page order, its navigation table and two
headings now match what the sidebar says.

### Gated

`tests/drive` gains a `6.x` block: the first-run screen's contents, each of the
three new routes, the About facts, the daemon card, the Reports picker and
preview, and two sweeps that had no gate before — every page at seven widths
with no horizontal scroll, and every page read for console errors.

Nothing in the retrieval path, the index writer or the graph extractor changed,
and the 0.25.0 figures reproduce.

## [0.26.0] - 2026-09-20

### The whole portal, free, in the binary

Thirteen pages, no plan, no key, no lock and no price. Everything the v4 design
draws now renders from your own store, and everything under it answers from the
terminal and over MCP as well — three new commands, three new tools, and a page
for each.

**Impact.** The graph read backwards: from a symbol, every caller and every
file that reaches it, breadth first to a hop limit, each edge carrying the
support class the store already recorded. `semlith impact`, `semlith_impact`
and a page of its own. This left the free product in 0.13.0 to be sold and
never was; the monetization hold of 2026-09-15 made the binary free whole, so
it is back with no key of any kind.

**Trace.** A chain turned into evidence: the answer sentence, the hops, and one
supporting source line per hop, each marked a supporting fact or a candidate to
corroborate, with a control that copies the block for a review. `semlith trace`,
`semlith_trace`, and the panel. Nothing re-walks the graph — it reads the chain
`semlith path` produced, so the two cannot describe one path differently.

**The path finder, drawn.** `semlith path` has answered from a terminal since
0.14.0 and had no page. It has one now, with the seams, the `Prefer verified
edges` and `Strict` toggles, the refusal that names the rule, and the
hypothesis line on a chain that is one.

**Map.** The store's subsystems as a list: label, member count, three hubs with
file and line, and the strongest edge out of each. Label propagation over the
settled `calls` and `imports` edges, seeded by name order and capped, so the
same store gives the same communities twice. No new dependency, no model, no
second picture beside the canvas.

**Inside the index.** The Index page is now what the design calls it, and
carries the language mix, chunks by month indexed, and Graph health: call edges
by support class as one bar, the top unresolved call targets, how many names
carry several definitions, and how many languages carry edges — read from the
counts `semlith stats` reports rather than recomputed, so the page and the
terminal cannot disagree.

**The ledger, session by session.** A table of every agent session: when it was
last seen, who it was, how many reads, net tokens after refunds, what that
would cost at a model you pick, and the tier of the figure. Filters by client
and tier, sort, pagination, and export as Markdown, CSV or JSON of exactly the
rows on screen.

**Session replay.** What the agent did after each answer — read the whole file,
grepped anyway, or edited — read from this machine's Claude Code transcripts.
Off until the Privacy page turns it on, because those files belong to another
program, and nothing it reads leaves the machine.

**Reports.** Five, generated here: retrieval savings, AI access audit, change
brief, index health and knowledge gaps, each as Markdown, CSV, JSON or
print-styled HTML. `semlith report` and `semlith_report` write the same bytes
the page exports. PDF is your browser's own print of the HTML; no PDF writer
ships in the binary.

**Cloud.** One page describing an optional hosted service, in its
not-connected state and nothing else. This binary has no cloud command and
opens no connection to any host.

### Four defects, all of them older than this release

- **#122**, and this one had survived a whole retrieval release: a query
  describing a picture ranked a source file above the picture. The image list
  was fused on the flat curve while a question's vector list is steep, so a
  confident CLIP match could not reach a passing text chunk's score however
  well it matched — measured at 0.049180 against 0.093317 on the four-shape
  fixture. The image list is now steep for every shape and the confidence rides
  in the weight, which leaves an unconfident image scoring what it always did.
- **#120**: `POST /api/index` recorded whatever path it was handed as a root of
  the store *before* the run applied the boundary, so the refusal that
  `docs/security.md` has promised since 0.20.0 could never happen. The boundary
  is read first now, and a request naming no store indexes the path into the
  store `home::resolve` picks for it rather than into whichever store happened
  to be writable.
- **#121**: a forwarded `semlith_index` reported a skipped count with no
  reasons while the same call answered in process named every one. One call in
  two places may not say two different things about it.
- **#124**: a repair note printed its backticks as characters. Fixed where the
  string is written, so the terminal stops printing them too.

### Numbers

Nothing in this release touches the retrieval path, so the 0.25.0 figures were
re-measured on this binary rather than restated. Three runs on the
sealed thirty, median of three, spread zero:

| | 0.25.0 | 0.26.0 |
|---|---|---|
| hit@8 | 29 / 30 | **29 / 30** (96 %) |
| hit@3 | 27 / 30 | **27 / 30** (90 %) |
| hit@1 | 24 / 30 | **24 / 30** (80 %) |

Every identifier question in the split is in the top three — 11 of 11, which
is how many the sealed thirty holds; the 0.25.0 record said 12 of 12 and no
question file has changed since, so that figure was wrong rather than this
one. Wrong-yes is 0. By class at k=1 / k=3 / k=8: concept 7 / 10 / 12 of 13,
identifier 11 / 11 / 11 of 11, multi-hop 6 / 6 / 6 of 6.

`tools/list` is 5 840 bytes, about 1 460 tokens, for sixteen tools — against
4 473 bytes and about 1 119 for thirteen.

Performance, measured on the reference laptop with the scale test run on its
own: warm query p50 **13.6 / 13.3 / 141.4 ms** at 700 / 7 000 / 70 000 chunks,
peak resident memory while indexing 246 MB, idle resident memory 151 MB at
700 chunks and 150 MB at 70 000 — flat across a hundredfold corpus, which is
the promise that opening a store loads no vectors. Searching grows instead,
by 241 bytes per chunk. The idle watcher costs 0.01 s of CPU over 60 s and an
edit becomes searchable in 1.29 s.

Run the measure suite the way `cargo test` runs it by default and three heavy
tests share one process: the same 7 000-chunk query reads 13.3 ms alone and
47.2 ms beside the others, and two resident-memory assertions fail on the
contention rather than on the code. Every figure above was taken with
`--test-threads=1`.

The tool list an agent pays for once per session grows with the three new
tools, and the gate on it moved from 1 120 tokens to 1 600. The Agents page
states what this binary's own list measures rather than a number written down
when it was last checked.

## [0.25.0] - 2026-09-20

### The right span, measured

The sealed thirty, scored once by the release binary at completion, median of
three runs with zero spread, beside the previous release's binary on the same
split and the same corpus:

| | 0.23.0 | 0.25.0 | 0.25.0, `SEMLITH_RERANK=on` |
|---|---|---|---|
| hit@8 | 28 / 30 | **29 / 30** | 29 / 30 |
| hit@3 | 27 / 30 | **27 / 30** | 28 / 30 |
| hit@1 | 25 / 30 | 24 / 30 | 24 / 30 |

Identifiers are 12 of 12 in the top three and wrong-yes is 0, both asserted by
the run rather than read off it. The gate this release was held to — hit@8 at
least 27, hit@3 at least 26 — is met by the shipped default and was not
re-baselined. The aim stated at planning was 30 of 30 at k=8; it came one
question short, and that question is `concept-portal-parity`, whose answer is
spanned in `AGENTS.md` and which the ranking still does not return.

hit@1 is one question below the previous release. It is stated rather than
gated because on a concept question it measures the span file: several chunks
answer correctly and only the spanned ones score.

### The audit, which is most of the number

Before any ranking change, every question that missed at k=8 and every
near-miss whose first hit was unspanned was read by hand: twenty-four in all.
Eleven were answered correctly by a chunk no span covered — `docs/models.md`
says "a query about a picture goes through CLIP's own text encoder" for the
question that asks exactly that, and `src/lock.rs`'s module doc states the
advisory lock for the question about two indexers — and thirteen spans were
added. Nine were real ranking failures and gained nothing. Four could be read
either way and gained nothing, because a span added to buy a hit is a hit
bought from the measuring stick.

Each decision is recorded on the question with the number of spans it added,
and the harness fails if a question marked correct-but-unspanned gained none.
The sealed thirty were redrawn from the audited set with a new seed, and the
draw now lives in the harness with a test that fails if `split.yaml` is not
what the seed produces.

### Rescoring, and why it is off

There is a rescoring stage now: a local 37 M-parameter cross-encoder, int8,
Apache-2.0, pinned by digest, that reads the query and a candidate **together**
— which fusion never does, since fusion compares positions — and whose order is
fused with the fused one, so a candidate has to be liked by both.

It is off unless `SEMLITH_RERANK=on`, and the reason is the measurement beside
it: a search over one store takes **8.2 ms**, and **132.2 ms** with the stage
over twelve candidates. What that buys on the development seventy-seven is two
questions at k=1 and one at k=3. Worth having when one answer matters more than
a tenth of a second; not worth making every agent's every search sixteen times
slower by default. `semlith setup` fetches the model so that turning it on
never pauses a query, and `stats` and `doctor` say which ranking answered.

### What was tried, and what was kept

Every mean is a pair: three runs a side, one binary, `git diff -- src/` clean
between them. On the development seventy-seven, in the order they were tried:

- **Code chunks carry their definition** — the enclosing signature and the
  first line of its doc comment, in front of the text at embed time and nowhere
  in the stored bytes, the way a Markdown chunk already carries its heading
  path. 65 → 66 at k=8, 62 → 62 at k=3, 54 → 51 at k=1. **Kept.**
- **A cross-encoder writing the order outright** — 59/67/71 → 59/67/71. It
  moved sixteen questions and gained nothing: rank 7 to rank 1 for one, rank 1
  to rank 6 for another. **Not kept.**
- **The same cross-encoder fused with the fused order** — 59/67/71 →
  **61/68/71**. **Kept, behind the switch above.**
- **A deeper candidate pool for questions**, 32 → 50 — identical at every depth
  on the 51 questions both runs scored, and four questions slightly worse.
  **Not kept.**
- **Normalising spellings before comparing a hint against a path** — five more
  call edges of 6 930 settled, no question moved at any depth. **Not kept.**

### The rest

`semlith stats`, `semlith_stats` and one table on the portal's Index page state
what the graph covers per language: files, files whose parser gave up,
definitions, call edges by support class, and call targets no definition
satisfies. An absent edge and an unparsed file are different problems and a
single store-wide percentage hid both. A test reads the CLI and the tool over
one store and fails if they disagree.

`semlith brief` carries one span of text rather than three: 2 209 tokens per
answered question against search-then-read's 724 becomes **1 112 against 725**,
at 1.00 calls against 2.54.

`semlith doctor` now names the key that actually holds a disabled server rather
than the documented spelling of it — the defect that hid a silently absent
server on the reference machine for a third time, because the first thing
anybody does with that message is search their configuration for the key it
names.

The six daemon index-run tests of issue #118 pass: five asserted a streaming
contract the daemon replaced in 0.20.0, and the sixth found a real fault — a
forwarded `semlith_index` dropped the refusals the in-process server names, so
an agent was held to a boundary it was never told about.

### Faster, measured beside the release it replaces

Both binaries on the same laptop in the same run, median of twenty searches,
one test thread:

| chunks | 0.23.0 | 0.25.0 |
|---|---|---|
| 700 | 20.5 ms | **16.2 ms** |
| 7 000 | 46.9 ms | **37.9 ms** |
| 70 000 | 362.4 ms | **159.1 ms** |

The pair matters more than either number: the 6.5 / 13.7 / 129.9 ms the 0.23.0
record states were taken on a quieter machine, and this laptop is about three
times slower today for both binaries. Measured against each other rather than
against a figure from another day, the release is faster at every size. Peak
memory while indexing stays under 240 MB at every size, an idle watcher costs
0.00 s of CPU over 60 s, and an edit on disk is searchable in about 2 s.

A reader holding three stores costs **1 MB** per extra store beyond the first,
against 123 MB before the cross-encoder was made one per process.

### Upgrading

A store written by an earlier release is re-chunked and re-embedded by the
first full index pass, because what the model is shown has changed and a store
half embedded each way ranks its own files unevenly. The run says why it is
doing it. Nothing you indexed is touched, and the store format moves to 4; an
older binary refuses a format-4 store by name rather than misreading it.

## [0.24.0] - 2026-09-19

### The number, and the defect the number found

`semlith ledger --verify` now prints what the ledger adds up to beneath the
chain result, and the README carries a savings paragraph again — the first since
0.17.2 removed the double-counted one, and only because it is now measured.
Asking `semlith brief` all 107 questions of this repository's own retrieval
harness against a store of `src/`: 3 982 tokens per answered question is what
the agent was sent, 105 132 is what reading the 4.1 files those answers named
would have cost whole. 26×, at 100 % coverage, counted by the store's own
tokenizer. It is an upper bound and says so.

Measuring it found the defect that made it worth measuring. `semlith brief` at a
terminal, and the portal's Brief view, both recorded every answer as a retrieval
that found nothing: the ledger recovers the files an answer named by reading
them back out of the rendered reply, and both of those hand it JSON instead. The
first run of this paragraph's own measurement read *saved 0 tokens over 0 of 107
retrievals, coverage 0 %* — for the one command 0.23.0 exists for. A brief knows
which files it named; it is asked now rather than parsed. Any 0.23.0 ledger
holding CLI or portal briefs has under-counted them, and re-running the query is
the only way to correct rows that are, by design, not rewritten.

### The graph reads as a graph

Three things were wrong with what a freshly indexed project drew. The page
opened on the busiest symbol, which in any codebase is its most *reused* name —
`new`, `len`, `get` — earning its degree from forty unrelated callers the
extractor could only match by spelling, so the first thing anyone saw was a star
of dashed `inferred` lines between functions with nothing to do with each other.
The opening symbol is now the one with the most edges the extractor actually
resolved, and a name defined once in the store beats a name defined forty times
however busy it is. When the node budget cuts a neighbourhood — and on a hub it
always does — the resolved edges are drawn and the spelling matches are the ones
left out, rather than whichever sorted first.

The store chips scoped the canvas and not the panel beside it, so a symbol
defined in two open stores listed both stores' callers under a chip naming one
of them: the rail contradicted the picture next to it. It is scoped to the same
stores now.

And a view where nothing is connected says so. It is a real state — a language
semlith parses for definitions but not yet for calls, or a scope holding both
ends of no edge — and a field of unconnected boxes with no explanation reads as
a broken page.

### A corpus is the project, not its dependencies

The walk carries a table of generated and vendored directories of its own, for
every language semlith supports. `.gitignore` was the only thing standing
between a store and a dependency tree, and it is a statement about what is
*committed*: it is absent from a folder somebody downloaded rather than cloned,
it is absent where `npm install` ran outside a repository, and where a user
keeps `node_modules` in their global gitignore the walk cannot see it at all. A
Node project indexed under any of those three swallowed its whole dependency
tree — tens of thousands of chunks nobody asked about, and as many graph nodes
with no edge into the project's own code, which is what a field of unconnected
dots on the Graph page actually was.

A name that is never somebody's own source — `node_modules`, `__pycache__`,
`.venv`, `.gradle`, `DerivedData`, `.dart_tool`, `_build` and the rest — is
stepped over outright. A name that often *is* somebody's own — `target`,
`build`, `dist`, `out`, `bin`, `obj`, `vendor`, `deps` — is stepped over only
when the manifest that generates it is sitting beside it, so a project with a
hand-written `build/` and no build file keeps it. `SEMLITH_DEFAULT_IGNORES=0`
turns the table off whole.

The index run names every directory it stepped over, rather than counting the
files inside one: pruning the subtree is the point, and counting what is in it
would undo the saving in order to report it. An existing store drops what it
already holds on its next index pass; nothing needs migrating.

### One skill, installed once and linked everywhere

The binary carries an Agent Skill named `semlith`, in agentskills.io format, and
`semlith setup` writes it to `~/.semlith/skills/semlith/` and links that one copy
into every user-level skill directory a documented client reads — the
cross-client `~/.agents/skills`, and Claude Code's, Qwen Code's and Kiro's.
Canonical plus links rather than a copy per client: removing the canonical
directory removes all of them, and a link that has gone stale is reported rather
than silently left behind.

`semlith setup --register-all` also writes the short always-on rule block into
the user-level rules files semlith knows a path for, between markers of its own
and with a backup beside each file first. Nothing outside those markers is read
or written, and a second run replaces what is between them rather than appending
again. For a client whose rules file semlith does not know — Cursor and Copilot
keep theirs in their own interface — `doctor` prints the block to paste.

`semlith doctor` now reports, per client, whether the skill is linked, whether
the hook is present, and whether the rule block is there, in four states each:
linked, absent, stale, or paste needed. The portal's Doctor page shows the same
states from the same computation, beside the registration it already showed.

The MCP `initialize` result carries the `instructions` text `server/discover`
has sent since 0.20.0. Every client still on a 2025 revision — which is most of
them — never calls discover, so until now the sentence saying what this server
is for reached only the clients that needed it least.

Gemini CLI is not part of this: its MCP registration is unchanged, but no skill
link, rule block or hook is written for it, because none was ever run against a
live Gemini CLI and a client whose hook was never exercised is not evidence.

### The agent is steered, not only taught

`semlith hook` answers one `PreToolUse` event. When a registered store holds
the file a client is about to read whole — or the tree it is about to grep — it
adds one line naming the `semlith_brief` or `semlith_read` call that answers the
same question, and otherwise it says nothing at all. It decides from
`registry.json` alone, so a file no store holds costs one path comparison, and
it never sends a permission decision: answering `allow` would approve a read the
user's own rules were about to be consulted about. `--strict` refuses the first
qualifying read of a session and then reverts to the line.

Each whole-file read it sees becomes one ledger row — kind `raw-read`, the path,
what reading it whole cost, the client and the session — chained like every
other row and uncredited, because semlith answered none of it. That is what
makes Refunds a measurement rather than an estimate: until now the ledger
counted only the questions semlith was asked, which leaves out every one it was
not. The row goes through a running daemon or not at all; the hook never opens a
store from inside a client's tool call, and `--no-ledger` and `SEMLITH_LEDGER=0`
stop these rows exactly as they stop the rest.

### Every search filter negates

A leading `!` on a `--path`, `--ext` or `--lang` value — and on the `path`,
`ext` and `lang` fields of every MCP tool that takes them — excludes instead of
including. Exclusions apply after the inclusions of their own kind, so
`--path 'src/**' --path '!src/vendor/**'` is everything under `src` but the
vendored tree, and an exclusion written on its own is everything except.

A filter that admits no indexed file now says so in one sentence wherever it
happens — the terminal, `semlith_search` and `semlith_brief` — rather than in
three wordings on the commands that had a check and silence on the ones that
did not. The README's "no way to express not this path" limit is gone with it.

## [0.23.0] - 2026-09-19

An agent asking semlith a question used to spend four round trips on it: search,
read the span, ask what calls it, read those. Three of them re-established what
the first already found. `semlith brief` answers it in one.

### One call instead of four

`semlith brief "<question>"`, `semlith_brief` over MCP, and a Brief view on the
portal's Search page — the command, the tool and the view in one release. Each
returns the spans a search would find, the text of the top ones, and the
resolved callers and callees of the symbols those spans sit inside, one hop each
way, every part labelled with the ranked list or the edge that found it.

The whole assembly is fitted to a token budget the caller sets, counted with the
store's own tokenizer, defaulting to 4 000. Locators are bought first, then the
one-hop edges, then the text of the top three spans — so a small budget drops
text from the bottom of the ranking up and says what it dropped, rather than
erroring or cutting a span in half. The best-ranked locator is the one thing a
budget cannot drop, and its cost is reported as `floor`.

Measured on the pinned corpus over the questions both paths answer: **1.00 calls
against 2.61**, at 1 934 tokens against 696. One call instead of several, at more
tokens — a brief carries the one-hop neighbourhood that the search-then-read path
never asks for, and the number is stated as measured rather than reframed.

Nothing here reaches past one hop.

### A symbol has a past

A re-index no longer simply deletes the definitions of a file it is about to
rewrite: it copies them into `symbols_past` first, stamped with the content hash
they were true for. `semlith symbol --history` and `semlith_symbol` with
`history: true` answer what a definition used to be.

Additive, like the 0.12.0 graph tables, so the store format does not move: a
store written by 0.22.0 opens unchanged, is not migrated by opening, and starts
keeping history at its next index pass. Nothing prunes it in this release.

### Retrieval

On thirty sealed questions, scored once by the release binary, median of three
runs, with 0.22.0 measured on the same corpus, the same instrument and the same
split:

| | 0.22.0 | 0.23.0 |
|---|---|---|
| hit@1 | 22/30 (73 %) | **22/30 (73 %)** |
| hit@3 | 24/30 (80 %) | **24/30 (80 %)** |
| hit@8 | 27/30 (90 %) | **25/30 (83 %)** |
| identifiers in the top three | | **12 of 12** |
| wrong-yes on `path` | 0 | **0** |

Spread was zero on every figure of every configuration measured.

**hit@8 is two questions worse than 0.22.0, and this release knows exactly
why.**

The cause is full-precision rescoring, which 0.22.0 named as unbuildable because
the store kept only 4-bit codes. Stores now write an `exact.f32` sidecar beside
the codes and the query path reads back only the candidates it is about to
reorder — and reordering them by exact cosine costs recall at k=8. Reciprocal-
rank fusion weighs a candidate by its rank, so a chunk the codes placed third can
fall far enough under the true vectors to lose its contribution and leave the top
eight. The pass ships because it is the groundwork the next release needs, and
the number it costs is stated here rather than buried.

It was found by elimination, not guessed at, and four candidates were ruled out
by measurement first: the fastembed bump this release carries (0.22.0 scores 27
with fastembed 6.1.0 too), the constants extraction, the candidate-pool depth,
and a graph-seed interaction that was real, was fixed, and turned out not to be
the cause. The corpus indexes to 4 267 chunks in every one of those runs, so
chunking was never it.

Constants are extracted as symbols in twenty more languages, not Rust alone.

The sealed split is redrawn from the same 107 questions with a new recorded
seed, because the thirty drawn for 0.22.0 stopped being held out when eight
questions' spans were completed after that set had been scored. It is weaker
evidence than that one was — the work has now seen all 107 — and `split.yaml`
says so rather than leaving it to be noticed.

### Fixed

- Warm query p50 and peak indexing memory are measured again. The stage that
  produces them returned early unless `OLD` named a 0.6.0 binary, so 0.22.0
  recorded both as NOT MEASURED while the suite still exited 0. Against 0.21.0
  on identical corpora: **6.5 / 13.7 / 129.9 ms** at 700 / 7 000 / 70 000 chunks,
  against 5.2 / 12.4 / 130.4 ms. Opening a store is now flat with corpus size —
  150 MB at 700 chunks and 149 MB at 70 000, where 0.21.0 paid 215 MB for a
  small one.
- `#104` closes. The GitHub-hosted macOS runner reports an Aqua session and a gui
  domain and still will not keep a user agent alive; two probes were written for
  it and both were disproved, so the host is named in the harness's own output
  and the known-failures row is gone. A row there turns a check that cannot run
  into one that ran and failed as expected, which is how a skipped check comes to
  read as a passing run.
- The native smoke harness runs only on release pull requests. Three runners and
  eight minutes per dependabot bump bought nothing: the installer and the shipped
  artifact do not change because a transitive crate did.
- sha2 0.11 finalises to a value that no longer implements `LowerHex`. One place
  now produces every SHA-256 hex string semlith emits, and a test pins the
  published vectors, so the bump is proven to move no digest and therefore no
  store's content hashes.

### Changed

- `tools/list` is thirteen tools at 4 441 bytes, about 1 111 tokens, against
  twelve at 3 995 and 999. Both ceilings are restated at the measured figure
  rather than at a round number chosen to fit it.
- `semlith stats` says whether a store carries the rescoring sidecar and how many
  definitions it has retired. A store without either answers slightly worse and
  slightly less, and neither was visible from the answers themselves.

## [0.22.0] - 2026-09-19

Retrieval was measured by an instrument that could not measure it. This release
rebuilt the instrument first and only then moved the number, which is why the
figures below are lower than the ones they replace and mean more.

On thirty questions held out before any ranking work, scored once at the end:
hit@1 **66 %**, hit@3 **76 %**, hit@8 **83 %**, against 0.21.0's 60 / 73 / 83 on
the same corpus through the same harness. On the seventy-seven the work was
tuned against: 71 / 81 / 87 against 62 / 72 / 84 — and the distance between those
two gains is what tuning against a visible set is worth.

**The gate this release was specified against is not met.** It asked for hit@8
at 95 %, hit@3 at 85 % and hit@1 at 70 %. The gate moves to the next release
rather than being restated as met, and what stands in its way is named on the
performance page.

**Every retrieval number this project published before today was wrong**, and in
the same direction. They were taken over a corpus that moved with every commit;
against a question set in which 47 of 92 spans no longer held the symbol they
named; and by a harness in which a `path` question counted in the denominator
and could never record a hit, which capped hit@8 at 87 % by construction. They
are withdrawn rather than updated.

### The measuring stick, before the ranking work

- **The retrieval harness indexes a pinned corpus, not the working tree.**
  `tests/fixtures/retrieval/corpus` is the 0.21.0 tree at `4e8df39`, less
  `tests/drive` and less the retrieval fixtures themselves — 146 files and
  3.1 MB, recorded in `corpus.yaml` and asserted by file count and byte total
  on every run. Until now the harness copied `src`, `tests`, `docs` and
  `AGENTS.md` out of whatever tree it was compiled in, so every figure it
  printed moved with the repository as well as with the ranking. That is why
  the README said 12/18/26 of 47 on the same day the harness said 10/14/19.
- **The question set is 107 questions, split 77 development / 30 sealed.** The
  57 written for 0.14.0 are re-pinned to the snapshot — 47 of their 92 spans no
  longer held the symbol they named — and 50 new ones were written from reading
  the snapshot, reaching into the files the old set never touched. The split is
  seeded and stratified by shape and tool, and the sealed thirty are scored once
  at completion, behind `SEMLITH_RETRIEVAL_SEALED`; the harness prints which set
  it is scoring on its first line.
- **Every figure is the median of three runs, with the spread beside it,** and a
  run is an index and a scoring rather than a second scoring of one store. The
  drift issue #88 records was at index time, so three scorings of one store are
  one run reported three times. `symbol` and `neighbors` questions are scored
  rather than skipped.
- **A `path` question can be a hit.** It was counted in the denominator and could
  never record one, so every path question was a permanent miss however well the
  tool answered it — one run reported "chains found 6 of 7" and "wrong yes 0"
  beside ten path questions missing at k=8. Ten of seventy-seven questions unable
  to score capped hit@8 at 87 %, which is below the gate this release ships
  against, so the gate was unreachable by construction rather than by retrieval.
  Every retrieval figure this project has published understates itself for this
  reason.

### Retrieval

- **An identifier-shaped query puts the definition first.** Typing a name you
  already know returns the chunks that define that name above the fused order,
  badged `definition`. Reciprocal-rank fusion is nearly flat across the first
  ranks, so one authoritative list placing a definition first was outvoted by
  two vague lists placing something else fourth and fifth — and five identifier
  questions on the pinned corpus had no satisfying span in the first eight
  results, two of them definitions that ranked first in the keyword index alone.

### Fixed

- **The native smoke harness no longer writes under your own home** (#106). It
  pinned `SEMLITH_HOME` and not `HOME`, and a client configuration lives under
  the user's home, so a run on a developer's machine rewrote that developer's
  real Claude Code, Cursor and Codex registrations. It now redirects `HOME` too,
  and a new check compares every client configuration path in `docs/clients.md`
  by checksum before and after the run.
- **The smoke harness says what supervision it found** before deciding whether
  a login service can be tested — `launchctl managername` on macOS,
  `systemctl --user show-environment` on Linux — so a log says why a check was
  skipped rather than only that it was.

  **#104 is not fixed**, and the attempt is recorded rather than buried. The
  aim was to tell a host that cannot supervise from one that can, so
  `cli/service/recovers` could skip instead of being carried as an expected
  failure. Two probes were tried on real runners and both were wrong: the GitHub
  macOS runner answers `launchctl print gui/<uid>` and reports an `Aqua` session,
  and still will not bring a killed user agent back. It presents every property
  a real login session has. The known-failures row is back, now carrying that
  finding, and #104 stays open.

## [0.21.0] - 2026-09-18

On 2026-09-17 a session opened with no semlith server. The registration was
correct, at user scope, and the installed binary answered `initialize` in well
under a second over six stores. It happened again on 2026-09-18. Nothing
anywhere said why.

The cause was found before a line of this release was written: `~/.claude.json`
held `projects["…/live/semlith"].disabledMcpjsonServers` containing `semlith`.
The server was registered and switched off for one directory. `claude mcp list`
run from anywhere else reported it connected, so every check short of asking
from the affected directory passed.

This release is about that class of failure — semlith absent, with nothing
saying so — from every side it can be reached.

### semlith is there before a client asks

- `semlith start --service` installs the daemon as a login service: a launchd
  user agent on macOS, a systemd user unit on Linux, a logon task on Windows.
  It runs from the next login onwards, and on macOS and Linux a daemon that
  exits is restarted within seconds with nobody present. `semlith start
  --no-service` removes it and leaves a running daemon and every store exactly
  as they are.
- `semlith setup` and the installers install it, including where nothing can
  answer a prompt — a piped installer, CI, `--yes`. A user who never reads the
  prompt is the user a login service is for. `--no-service` opts out, and so
  does `SEMLITH_NO_SERVICE=1`, which both install scripts and `semlith setup`
  itself read — so a provisioning script gets the same answer whichever of them
  it reaches for. **Installing semlith now starts a daemon**, so anything that
  wants to run its own should opt out; that is what the harness does.
- Installing onto a port something already holds registers the service for the
  next login and leaves the running daemon alone. Starting a second one would
  lose the bind and exit, and `KeepAlive` would start it again.
- On macOS, a binary inside `~/Documents`, `~/Desktop`, `~/Downloads` or iCloud
  Drive is refused rather than installed. A launchd agent pointing at one
  installs cleanly, reports a live pid, and then stops inside the dynamic
  linker before it can write a word to its own log: no error, no timeout, no
  port. The refusal names the directory and where to install instead.

### The handshake no longer waits for the embedding model

- `semlith mcp` answers `initialize` and `tools/list` before any model is
  loaded. It used to load every distinct model first, so a client's startup
  timeout could expire on an ONNX session or a model download and report no
  server at all. The model now loads on a background thread; only a tool call
  that needs it waits. The tool list is byte-identical to 0.20.2's.
- A machine with nothing indexed gets a server instead of an exit code. A fresh
  install wired into a client used to end the process with "no semlith store
  covers …" before writing a byte of protocol. The tools are listed, and the
  first call that needs a corpus carries the message that used to be fatal.
- `semlith mcp` no longer claims a privacy rejection for a file that is not
  there. Every start printed one line per store — six on the machine this was
  found on — saying `daemon.json` "is not a file this user wrote privately",
  naming paths that answer `No such file or directory`. Those lines were the
  first thing in a client's log for anyone debugging an absent server.

### `semlith doctor` names the step that failed

- A server registered at user scope and switched off for one directory is
  reported as a fault in that directory, naming the file, the key and the
  directory, with the command that turns it back on. It is never re-enabled
  without being asked: switching a server off is a choice, and naming it is
  semlith's job. From any other directory the row is green and still says which
  directory it is off in.
- Every registration semlith writes names the resolved absolute path of the
  binary rather than the bare command `semlith`. A bare command launches only
  from a PATH that happens to carry the binary, and the PATH a login shell
  builds is not the one a service manager hands a job.

### Housekeeping

- The native smoke harness on all three platforms installs the login service,
  kills the daemon and confirms it comes back, and checks that the handshake
  answers with the embedding model made impossible to load.

## [0.20.2] - 2026-09-18

A full manual drive of every portal page on 2026-09-17 — every button, filter,
picker, toggle, sort, index run, search, graph interaction, agent action,
privacy scan and about row, at desktop, tablet and phone widths — found 62
defects. This release fixes all of them, and adds the scripted browser drive
that replays every one of their reproductions as a standing gate.

### The product no longer contradicts itself

- A store holds only what its roots cover. The index boundary treated the whole
  home directory as inside every store's boundary, so any store on the machine
  could swallow any other's corpus and be allowed — the `semlith` store held 262
  files belonging to `ultraship`, every search across all stores returned them
  twice, and the store label on the duplicate was wrong. The home directory is
  now the fallback only for a store with no registered roots. A store reconciles
  what it should not hold when the daemon opens it, logging the count and showing
  it on the Stores page, and `semlith index` does the same — with
  `--reconcile report` to see the count before anything is dropped. The files on
  disk are untouched.
- One search is one retrieval. A search wrote a ledger row into every open
  store, so every figure on the Retrieval ledger page scaled with how many
  stores happened to be open: sixty-three queries recorded for about a dozen
  searches. The rows stay per store — that is what keeps each store's hash chain
  its own and each row's token counts about its own hits — and they now share a
  query id, which is what "queries recorded" counts. Rows written before this
  version have no id and are each their own retrieval, as they were; the chain
  is not rewritten, and the page says so.
- `LAST WRITE` is the store's last write, read from the store rather than from
  this daemon's memory of its own session. A store with 438 files in it no
  longer reads `never`.
- `semlith ledger` and the portal both print local time with the zone offset,
  from the operating system rather than from UTC arithmetic. One dataset, one
  clock.

### Index runs tell the truth about what they did

- Runs are keyed by run id rather than by store. A second run against the same
  store used to write its header over the first one's card while the body kept
  the previous run's log.
- A finished run offers **Remove** and not **Stop**. Stop on a finished run
  opened a confirm promising to undo everything it had embedded, did nothing
  visible, and left the store carrying a cancellation that killed its next run
  at 0% — with the file fetched, written to disk and never indexed, and no error
  anywhere. A late stop is refused, and the cancellation is cleared when a run
  is admitted.
- Live runs sort above finished ones, a finished card folds to one line, all the
  finished cards can be cleared at once, and the "N runs queued." line clears
  when nothing is queued.
- The live chunk counter is the run's, not the slice's. It climbed to a flush
  boundary and fell back to single figures, which reads as a run losing work.
- A store the portal creates holds its watcher off until the run that is about
  to index it arrives, so a run no longer finds its own files unchanged and
  reports `1 indexed … 1 chunks` for a store that has just gained three files.
- A run that is rewriting its shards says so. Every two hundred files it flushes
  and rewrites, which on a large corpus is twenty seconds with the bar not
  moving and nothing said.

### Features that did not work

- **Adopt existing .semlith** adopts. It takes the folder that holds the store
  as readily as the store itself, the picker lists `.semlith` directories and
  marks a folder that holds one, and a failed adopt leaves the picker where it
  failed rather than closing it and losing every step of navigation.
- **Add from a URL** has its own target-store selector. The control it actually
  read was a dropdown in a different panel, so every URL — valid, private or
  nonsense — was refused with the same store-selection error, and the private
  address refusal was unreachable from the portal.
- **Open in Files** opens Files filtered to that store. The Files page has a
  store control of its own, and all seven of its columns sort, so there is a way
  in the portal to look at one store's files.
- **Projects under a folder…** has Up and drill-down, the same navigation the
  folder picker beside it has always had, so it can be pointed at a monorepo
  anywhere on the machine.
- The graph's edge-kind filters filter. With all six off it drew every edge and
  the summary asserted a number that did not describe the canvas. The edge count
  is the count of edges drawn, and an edge two stores both know is drawn once —
  a twenty-node subgraph reported more edges than the whole graph.
- Changing `lang:`, `path:` or the token budget re-runs the search, like every
  other control on the page.
- The Retrieval ledger page shows the ledger: the rows `semlith ledger` prints,
  paged and sortable.

### The HTTP surface

- An unknown path answers `404`, with `401` kept for a request that failed the
  token. A wrong path in the PWA manifest read as an authentication failure and
  sent whoever debugged it to look at the token.
- `site.webmanifest`'s icon paths resolve, so a page load logs no console error.
- `/api/files?limit=` above the maximum answers `400` naming it, the same
  contract `offset` already had, rather than clamping silently.

### Copy, states and polish

- A registry entry whose directory is missing is shown as missing, naming the
  path that is absent, and is offered nowhere a real store is offered.
- The Machine limits panel's derivations are computed from the numbers it
  prints: a negative headroom says the floor applies, MB and MiB are one unit,
  "threads each" recomputes when "runs at once" changes, and a saved value is
  called saved rather than derived.
- Doctor gives a fix command only for a client that is on this machine, and
  where two commands are genuinely right for one state the cell says why.
- The Claude Code HTTP stanza carries the `--scope user` the paragraph above it
  insists on; inline code on the Agents page renders as code; closing the MCP
  endpoint asks first; and the dry run is the primary button while the one that
  writes ten configuration files is the secondary.
- Four model notes described a different model than the row they sat on, the
  model semlith actually runs showed no size, and sorting by size buried the
  only rows that had one.
- The Privacy page's Rules badge says it is about new writes, and the scan below
  it says it is about what is already stored.
- The theme control offers system, light and dark, so following the operating
  system is reachable again.
- Truncated paths carry a `title`, four tables carry captions, unselectable rows
  in a picker are dimmed, pickers sort case-insensitively, the code preview fades
  where a line continues, the tablet stat tiles break three and two, a phone's
  store chips are one scrolling row, and a sub-second run reads in tenths.
- The first search after the daemon starts says it is loading the embedding
  model rather than showing five seconds of "searching".

### The gate

- `tests/drive/` drives Chrome over the DevTools Protocol and replays the
  reproduction for every one of the 62 findings, asserting the corrected
  behaviour and writing a transcript and a screenshot per check. It runs in the
  native smoke harness on Linux, macOS and Windows.

### Unchanged

- The MCP tool list, the tool schemas and the store format. Agents see the same
  twelve tools with the same contracts, and a 0.20.1 store opens without
  migration.

## [0.20.1] - 2026-09-17

### Fixed

- `install.ps1` can replace a semlith that is already installed. It moved the
  unpacked binary into place with a single `Move-Item -Force`, and Windows will
  not write a file over an executable that is already there — so every
  re-install ended at "Cannot create a file when that file already exists"
  while every first install worked. It now renames the installed binary out of
  the way before moving the new one in, and puts it back if that fails, which
  is the sequence `semlith upgrade` has always used. The installer is served
  from `main`, so this reaches everyone without a release.

## [0.20.0] - 2026-09-17

### A sliced run carries on instead of starting over

- A run yields the writer back to the watcher every 45 seconds and returns as a
  fresh job. That job carried the run's *roots*, so each slice walked the tree
  from the top and re-opened and re-hashed every file the run had already done,
  to be told each time that it was unchanged. It was correct — indexing is keyed
  on content hashes — and it read as a run restarting every 45 seconds, and it
  made a run of N files over S slices read N x S files instead of N. A slice now
  carries the remainder of its own walk, and the orphan sweep happens on the
  slice that reaches the end rather than on every one of them.
- The progress a page shows belongs to the run rather than to the slice, so the
  counter no longer returns to 1 partway through.

### The run clock

- Starts when the run is submitted, not when the writer reaches it: the wait for
  a writer is time you are waiting.
- Spans every slice. It used to be measured inside the one function a slice
  runs, so it restarted from zero each time.
- Stops while a run is held, and freezes at its total when the run ends.

### The Doctor page

- Has a navigation icon. It was the one item in the rail that rendered nothing.
- Its Clients and Rules sections use the same layout as every other section in
  the portal. They were bare cards with no padding, so the title sat against the
  border and the table squared off the corner beneath it.

### A run lives in the daemon, not in the page that started it

- An index run is the daemon's: its id, paths, status, counters, clock, the last
  500 lines of its log and its closing summary. Leaving the Index page,
  refreshing it or closing the tab changes nothing, and coming back shows every
  run where it is with its log carrying on from the last line that tab saw. Until
  now the run *was* the streaming response the page held open, so the tab that
  pressed the button was the only thing that knew the run was happening. The work
  carried on — the work is the store's — but nothing on screen could find it
  again. A run is cut short by its own Stop or by `semlith start` ending, and by
  nothing else.
- The snapshot is written by the same `say` closure that emits every event, so
  the card and the log cannot disagree about what happened.
- The Index page draws one card per store, each with its own Pause, its own Stop
  and its own log, read by a cursor so two tabs open on the same run each see
  every line exactly once.
- **`POST /api/index` no longer streams.** It answers immediately with the run
  ids and store names. A script reading its NDJSON is the one thing this release
  breaks; poll `GET /api/index/runs` instead. The stream stays for a forwarded
  `semlith_index`, whose caller blocks on the answer and never held an HTTP
  worker. `POST /api/add` answers the same way, and its fetch is still
  synchronous because its refusals are what the caller has to be told.
- The elapsed clock was measured around one slice. A run hands the writer back to
  the watcher every 45 seconds and returns as a fresh job, so the reading
  restarted from zero every 45 seconds and the page faithfully redrew a run that
  had just begun. The clock belongs to the run now: it starts at submission,
  spans every slice, stops while the run is held, and freezes at its total.

### One folder becomes one store

- `POST /api/index` takes `"store": "each"` — one store per path, each resolved
  through the same function `semlith index <path>` resolves through, suffix and
  all, so indexing a folder from the page and again from a terminal produces one
  store rather than two. No store named means `each` for several paths and what
  it always did for one: three folders are three corpora, and putting them in one
  store is a choice nobody made.
- `semlith index --each` is the same choice in the terminal, sequential — one
  process, one embedder, one store at a time. Somebody who wants them in parallel
  runs the daemon, which is what it is for. `--each` with `--name` is refused,
  because a name is a name for one store.
- `semlith index --projects <FOLDER>` takes the paths from the git repositories
  directly under that folder and implies `--each`. A `.git` file counts as much
  as a `.git` directory, so a worktree and a submodule are on the list; where
  none of the children is a repository its plain subfolders are used instead. One
  level only: a monorepo is one store, and its nested repositories are its own
  business.
- The Index page has the same list as a checklist, from `GET /api/projects` and
  the same discovery function, so the page and the terminal cannot disagree about
  what is under a folder. A repository an existing store already covers is shown
  unticked rather than hidden — "nothing here" and "all of it is done" are
  different answers.

### Runs wait for each other, and the daemon decides how many

- Each store's writer used to take the next job on its own queue with nothing
  above it, so eleven open stores meant eleven runs at once whatever the machine
  had. A run is admitted only while fewer than runs-at-once are running and
  otherwise waits in one queue ordered by submission, with its position on its
  card and in the snapshot. The head is admitted the moment a run finishes, stops
  or fails, with the page open or not. The watcher's own re-embeds bypass
  admission: holding a file save behind eleven queued repositories would make the
  watcher useless exactly when the machine is busy.
- `POST /api/index/control` gains `dequeue`, and a queued card's button reads
  **Remove**. It answers at once and confirms nothing, because nothing of it was
  embedded — which is the whole difference from stopping a run that is going.
- A new `src/system.rs` reads logical cores, physical cores where the platform
  says, total memory and memory free now — `/proc/meminfo`, `sysctl` and
  `host_statistics64`, `GlobalMemoryStatusEx` — with no new dependency. From that
  it derives runs at once (memory free less a 2 GiB reserve over a per-run peak,
  capped by the cores with one kept free so the portal still answers, floor 1),
  threads per writer (the embedder's threads split between the runs and held
  inside the cores) and index memory per store (512 MiB, doubled past 16 GiB of
  headroom and again past 64 GiB). Each carries the one-line reason it chose what
  it chose, and the portal prints that reason beside the field.
- The three are settings rather than constants: `SEMLITH_INDEX_PARALLEL` (new),
  `SEMLITH_EMBED_THREADS` and `SEMLITH_INDEX_MEMORY` win, then
  `~/.semlith/settings.json`, then the derivation. An explicit variable is an
  instruction from whoever started the process, so the page cannot overrule one
  and says so on a field it cannot change. The file is written the first time a
  field is moved and never before. `semlith start` prints all three with their
  source.
- Changing runs-at-once takes effect on the next admission: raising it admits the
  head immediately, lowering it stops nothing already going, because a run holds
  a writer and undoing it would cost the work it has done.
- A daemon that ends with runs queued drops them, and its next start names each
  on that store's event feed as never started — along with any run that was going
  and what it had committed. A folder that never started otherwise leaves no
  trace anywhere, and the way anybody finds out is noticing the search results
  are thin.

### The portal reads live

- `GET /api/changes` returns six monotonic integers, one per data domain, each
  bumped in the one function that writes that domain. Every page polls it once a
  second and refetches only the domains that moved, so a quiet daemon with a tab
  open costs one small request a second. The Stores rows and their event feed,
  the Index cards, the Agents clients and their query counts, Ledger rows as they
  land, Privacy rules after a fix and a run count in the navigation on every page
  all read from that one clock. A tab in the background stops asking and catches
  up in one pass when it comes back.
- It is a poll and not a server-sent stream because a stream holds one of the
  daemon's eight HTTP workers for as long as the tab is open, which is the same
  constraint the run-state work above exists to respect.
- Search, Read and Graph are not refetched on a timer. Each is the answer to a
  question somebody asked, and redrawing it under them would be answering a
  different one.

### Also

- The Doctor page has a navigation icon and the card padding every other page
  uses. It was the one rail item with nothing to identify it, and its two
  sections had titles sitting against the border.

## [0.19.0] - 2026-09-17

### One unreadable file no longer ends the run

- A file that fails on its own content — a truncated PNG, a source file
  tree-sitter cannot parse — is reported `failed` with the decoder's or the
  parser's own message, and the run indexes the next file. Until now the first
  such file ended a run over ten thousand of them, which is what an index of a
  real Windows project tree kept doing. A failure of the embedding batch itself
  is the model failing rather than a file, and is still fatal.
- Every skipped file says why, from a closed set: `empty`, `over 8 MiB`, `not a
  regular file`, `unreadable` with the operating system's own words, `binary`,
  `no text in this document`, `not a decodable image`. An Angular tree's
  generated `.component.scss` files and a Python package's `__init__.py` files
  read as `empty` rather than as two thousand files that quietly went missing.
- The `done` event, the CLI's last lines and `semlith_index`'s return count the
  skipped by reason and name every failed path. `semlith_index` returns a
  summary rather than a tool error: a run that skipped one corrupt image and
  indexed nine thousand files succeeded.
- A file whose reader produced nothing used to be counted and never reported, so
  the portal's progress bar stopped short of its own total on any tree with
  binaries in it. Every file the walk yields now produces exactly one event.
- Entries the walk itself cannot read are `failed` files on the same channel
  rather than a line on stderr the daemon never sees.
- The watcher survives a per-file failure. A save that failed to embed used to
  stop the watcher thread for that store, so every later save in that tree was
  silently never indexed; it is now a line on the store's feed and `watching`
  stays true.

### Credentials

- A content scan runs on every file's text before it is chunked, stored or
  embedded, including the text a reader produced from a `.docx` or a notebook.
  It knows fourteen credential shapes whose prefix is the issuer's own
  declaration of what the string is, plus one generic rule: a key-like name
  assigned at least twenty characters of high-entropy text. A match refuses the
  whole file and names the kind and the line — never any character of what it
  matched. `--include-secrets` indexes it anyway, and says how many it took in.
- The deny-list widened to every environment-variable file rather than `.env`
  and `.env.*` alone, and to the per-user credential files the hidden-file rule
  used to catch on its own: `.npmrc`, `.netrc`, `.pypirc`, `.pgpass`,
  `.htpasswd`, `.boto`, `.s3cfg` and `*.ppk`. `.env.example` is refused on
  purpose: the name says what the file holds, whatever today's contents are.
- A refused file that an earlier run indexed is evicted in the same pass, and
  the refusal says so. A store does not keep holding what semlith has decided it
  will not hold, so a `dev.env` that 0.18.0 indexed leaves on the first run
  under this release.
- `semlith scan [STORE]` runs both rules over everything a store already holds,
  prints each file semlith would refuse today, and exits non-zero while any
  remain. `--forget` evicts them. The portal's Privacy page has the same list,
  from the same function, with a Forget beside each row. There is no MCP tool
  for it: an agent is not the party that decides what a store may hold.

### A whitelisted dotfile is indexed rather than refused

- The hidden-file rule now applies only to a path the caller named. A `.gitkeep`
  the walk yielded because the user's own `.gitignore` asked for it — `dist/*`
  then `!dist/.gitkeep` — was being walked and then refused for being hidden,
  which is the walk contradicting itself. `semlith index ~/.npmrc` is still
  refused, and the credential rules still apply to both.
- **This changes what a store holds on a re-index.** A tree whose `.gitignore`
  whitelists dotfiles will gain them. The widened deny-list above is what keeps
  that from meaning a credential.

### Fixed

- `semlith index` from a second process while `semlith start` is running
  produced a store the portal and every agent could not see until the daemon was
  restarted. The daemon now re-reads the registry on every Stores read and on
  every by-name miss, and opens what it finds. A directory another process holds
  the lock on is listed as being written and tried again on the next read; one
  that is no longer there is listed as missing.
- `semlith_symbol` and `semlith_path` returned Windows verbatim paths —
  `\\?\C:\work\api\src\lock.rs` — which no editor opens and no shell
  completes. They, and the daemon's own file events, render like every other
  surface now. The store still keeps the verbatim form as its key.
- The home directory was canonicalised twice per file to answer a question that
  cannot change during a run, which on Windows is two opened handles per file.
  Once per run now.

### Also

- `semlith_pattern`, `semlith pattern` and `/api/pattern` take `path` — the same
  glob filter every other surface takes, which this one ignored — and `offset`,
  so a listing the 200-match cap cut short can be continued. The truncation line
  names the offset that continues it.
- The portal's Index page shows the elapsed time, corrected to the daemon's own
  clock on every file so a backgrounded tab cannot drift, counting through a
  pause and frozen on the total when the run ends.

## [0.18.0] - 2026-09-16

### Every agent client on the machine, from every project

- `semlith setup` registers semlith in every client that documents a
  registration command, at the scope that client spells "every project", rather
  than registering Claude Code alone and printing the other twenty-six as
  stanzas to paste. `--yes` registers too, where it used to skip the step.
- Every registration is the stdio form: the client launches `semlith mcp`, which
  reads the agent key from `~/.semlith/agent.key` itself. No configuration file
  semlith writes carries the key or names `${SEMLITH_AGENT_KEY}`, and a rotation
  reconfigures nothing.
- An existing semlith entry is removed before the new one is added, at every
  scope the client has — including the project-scope entry in `~/.claude.json`
  that made semlith invisible from every directory but one.
- `semlith setup --register-all`, and a control on the portal's Agents page,
  write the configuration file of the ten clients that have no registration
  command. Every path is listed before anything is written, each file is backed
  up beside itself, each merge keeps every key it did not come to change, and a
  file that does not parse is refused rather than replaced.
- `docs/clients.md` carries the registration facts in its fence info strings, so
  the document a human reads and the one `src/clients.rs` parses are one file.
- The shell startup block `semlith setup` writes carries `PATH` and nothing else.
  It used to export `SEMLITH_AGENT_KEY` by reading the key file at every shell
  start, which made sense while every client stanza named that variable; none
  does now, so the export put a credential into the environment of every process
  you start in exchange for nothing. A block written by an earlier version is
  replaced — `semlith setup` used to leave any block it found alone, so an
  upgrade kept the export for ever.

### `semlith doctor`

- New command, with `--json` and `--fix`. Per client: installed, registered, at
  what scope, and what to run otherwise. Plus the four Privacy rules that are
  readings of this machine. Exits non-zero when something is wrong.
- A portal view of the same report, from the same route.

### The Privacy page can fix what it reports

- Every failing rule carries the command that repairs it, copyable.
- `directory modes`, `agent key`, and `model cache` when the cache is your own
  carry a button as well. A repair narrows access, is idempotent, touches only a
  path semlith owns, and is confirmed by re-running the rule's own check; it
  reports `0700, was 0755` so it can be undone by hand.
- `private addresses` gets the manual step and no button. It fails on a variable
  in the environment the daemon inherited, and no process can unset a variable in
  its parent's.
- Fixed: the `agent key` check rendered the same sentence whether the mode was
  compliant or not, so a key at 0644 read exactly like one at 0600.

### Fixed

- One corpus indexed twice now produces one index, and the retrieval harness
  reproduces its own numbers (#88). Three runs of one binary over one corpus
  gave three different hit@k, the graph-only denominator moved, and bytes per
  answer tracked where the corpus was checked out. Four causes, all measured:
  the graph walk read its frontier out of a randomly-seeded `HashMap`; the file
  walk took whatever order the filesystem gave it, so chunk ids moved; the
  fusion and rerank sorts left exact ties where they found them; and the index
  checkpoint was timed rather than counted, so it split an embedding batch at a
  different point on every run — and the embedder pads a batch to its longest
  sequence, which changes the last bits of every vector in it. The maps are
  ordered, the walk is sorted, both sorts are total, and the checkpoint interval
  counts files.
- `SEMLITH_CHECKPOINT_SECS` is now `SEMLITH_CHECKPOINT_FILES`. It was never part
  of the documented environment — it exists so a test need not wait thirty
  seconds — and it is what made an index run depend on the clock.

## [0.17.3] - 2026-09-16

### Credit where the index comes from

Semlith's vector index is [turbovec](https://github.com/RyanCodrai/turbovec), by
Ryan Codrai, under the MIT licence, and it implements TurboQuant — ["TurboQuant:
Online Vector Quantization with Near-optimal Distortion
Rate"](https://arxiv.org/abs/2504.19874), Amir Zandieh, Majid Daliri, Majid
Hadian and Vahab Mirrokni. That is not an implementation detail: a
data-oblivious quantizer needs no training pass over the corpus, which is why a
store can be built from a file save with no rebuild as it grows. The README
named turbovec twice as a filename component, linked it nowhere, and did not
name the paper at all.

### Added

- A `## Prior art` section in the README, citing the paper by title, its four
  authors and its arXiv id, and turbovec by author, licence and repository, with
  two sentences on what data-oblivious buys a local index. The first prose
  mention is a link as well.
- `tests/readme.rs` asserts the README carries both the turbovec repository URL
  and `2504.19874`, so an edit that drops either fails the build. Its line
  ceiling moves from 500 to 520 for the new section, once, with the reason in
  the doc comment beside it.
- `tests/portal.rs` asserts no semver literal appears in a user-visible string
  in `src/portal/app.js`. This is the README's release-content gate applied to
  the surface a user reads inside the product, which had none.

### Fixed

- `NOTICE` named BAAI/bge-small-en-v1.5 as the default model, which the product
  stopped using. It names granite-embedding-small-english-r2 at int8 and the
  repository it is fetched from, the CLIP ViT-B/32 vision and text pair a store
  fetches once it holds an image, and the licence each one states. Its turbovec
  block cites the paper rather than gesturing at "Google Research", and ONNX
  Runtime and hf-hub are named with what each one actually downloads.
- `docs/architecture.md`, which the README calls the full account, described the
  single-file index that store format 2 replaced in 0.7.0 — including a
  crash-safety argument and an atomic-write argument stated over the wrong
  object. Every such passage is re-derived from the code that writes a shard:
  the rename gives per-shard atomicity and nothing more, and what covers an
  interrupted run is the hashes being committed last. The store diagram is now
  the directory as it is on disk.
- The portal's About page said forty grammars arrived in a named release and
  that the binary had grown by a measured number of MiB against another one —
  three versions and four figures, hardcoded, where no reader could tell which
  build they described. The passage is gone and what a reader can act on is a
  clause in the paragraph above it.

### A Ctrl-C guarantee that CI can see fail

`an_interrupted_watcher_leaves_the_store_whole` had been failing since 0.17.0
and nobody heard, because it is `#[ignore]`d and CI does not run the ignored
set (#85). The cause was the test, not the product: it gave the spawned watcher
a hard-coded two seconds to install its signal handler, and the first exec of a
freshly linked binary spends seconds inside `execve` at zero CPU while the
kernel validates the Mach-O. The forty grammars 0.17.0 added took the binary
past the point where that finished in under two seconds, so the SIGINT arrived
before `main` did. `src/watch.rs` was never wrong.

- The test now waits for the watcher's own `watching …` banner, which is
  printed after the handler is installed, instead of for a clock. No number can
  be the right number there — the binary will grow again.
- `.github/smoke.sh` gains `cli/watch/sigint-whole`, which interrupts a real
  watcher and then checks the exit status, that no half-written index was left
  behind, and that every chunk still has a vector. It runs against the
  installed binary on `ubuntu-latest` and `macos-latest` on every develop PR,
  so the guarantee now has a gate that runs. Windows has no row by design:
  `watch::stop_on_signal` is `#[cfg(unix)]` and Ctrl-C there is a documented
  no-op.

### Removed

- The About page's `MCP revisions` row. Four protocol dates are a wire contract
  between the server and an agent client, not something a person reading About
  can act on. `GET /api/about` still returns them and dropping one is still a
  declared break.

## [0.17.2] - 2026-09-16

### Documentation describes the product that exists

An audit of the README against the shipped code found it describing a security
model removed in 0.14.0, warning readers off four Linux distributions that have
worked since 0.14.0, contradicting itself about graph coverage in two places,
omitting two of the twelve agent tools, documenting a portal page deleted in
0.16.0, and quoting a binary size two and a half times smaller than the one that
ships. Every one of those is corrected, and the numbers the README states are
now asserted against the code that defines them by `tests/readme.rs`.

### Added

- `docs/portal.md`, the first documentation of the portal: one section per page,
  how the daemon starts, and the concepts behind the graph, the search list and
  the two credentials.
- `docs/clients.md`, holding the client configuration stanzas moved out of the
  README. `src/clients.rs` and `tests/clients.rs` read them from there now.
- `docs/performance.md`, holding the benchmark tables with their reproduction
  commands and the release each was measured for.
- `tests/readme.rs`, which parses the counts out of the README and asserts each
  against its source — languages, edge kinds, tools, commands, formats, image
  types and clients.

### Fixed

- The portal's Graph page drew four of the six edge kinds the store holds:
  `contains` and `aliases` were fetched from `/api/graph` and dropped before
  painting, while the hover card counted them anyway. Both are drawn, both have
  a filter chip, and the caller and callee counts are taken over what is drawn.
- `src/graph.rs`'s module comment still said six languages carry edges, which
  0.17.0 made false.
- Four places said a rotated agent key stays valid until the daemon exits,
  including the message the portal prints after a rotation. It has been fifteen
  minutes since 0.14.0.
- `docs/compatibility.md`'s covered-surface table named none of `symbol`,
  `neighbors`, `path`, `ledger` or `drop`, which are stable commands.
- `CONTRIBUTING.md` said CI runs on Linux and macOS, and did not mention the
  `msrv` or `scripts` jobs.

## [0.17.1] - 2026-09-15

Ten bugs. Four of them are one bug: Windows sets no `HOME`, and semlith read it
in nine places that each had a different answer when it was missing. The rest are
what a person meets on the first day — a path printed in a form nothing opens, a
pipe closed early, a root that cannot be read, a forget that removes nothing, a
page parameter checked and ignored. No new command, no new tool, no new argument
and no schema change.

### One home-directory helper

- **#70 — the store home resolves on Windows.** `semlith::home::user_home()` is
  the only place in `src/` that reads the environment for a home directory:
  `HOME`, then `USERPROFILE`, then `HOMEDRIVE` plus `HOMEPATH`, and an error
  naming `SEMLITH_HOME` when it knows none of them. `stores_root`, `bin_dir`,
  `registry_path`, `agent_key_path`, `Registry::dir_of` and `model_cache_dir`
  return a `Result` and every caller propagates it. The Windows store home is
  `%USERPROFILE%\.semlith`, the directory `install.ps1` already puts the binary
  in.
- **#71 — the portal's directory browser refuses instead of rooting at the
  drive.** `GET /api/dirs` answered from the filesystem root when the home was
  unknown, and then refused the user's own profile as outside it. It answers 500
  and names the variables to set.
- **#72 — the directory deny-list fails closed.** A path that cannot be checked
  against `~/.ssh`, `~/.kube` and the rest is refused rather than waved through.
  On Windows, where the home never resolved, those rules had never run at all.
- **#73 — a run with no home is an error, not a store in the working
  directory.** `home()` and its silent `./.semlith` fallback are deleted. They
  were writing a store, a registry, an agent key and 52 MB of model weights
  wherever the process happened to start.

### Fixed

- **#74 — paths print in the form the platform opens.** `canonicalize` returns
  the verbatim form on Windows, so every locator began `\\?\` — which no editor
  opens and nobody pastes back. The prefix is stripped where a path becomes
  text — search hits, file rows, image rows, symbol rows, call-site paths — on
  the CLI and its `--json`, in MCP tool results and in the portal's
  `/api/search` and `/api/files`. The store keeps the verbatim form, which is
  what makes a path over 260 characters work, and `semlith read` accepts either.
- **#75 — `semlith files | head` exits quietly.** It printed a panic and exited
  non-zero. On unix the default `SIGPIPE` disposition is restored at the top of
  `main`; on Windows, which has no `SIGPIPE`, a hook turns that one panic — a
  failed print to a closed pipe, and nothing else — into a silent exit 0.
- **#76 — an unreadable index root creates nothing and registers nothing.**
  Opening a store is what creates it, so checking the paths afterwards left an
  empty store behind for a typo and exited 0. The roots are checked first, and
  the CLI, `POST /api/index` and `semlith_index` all refuse before a store is
  chosen. A run with a mix indexes the readable roots, names each unreadable one,
  records only the readable roots, and still exits 1.
- **#77 — `semlith forget` forgets the file the listing printed, and `semlith
  drop` runs at all.** `forget` resolved its store from the working directory
  rather than from the path it was handed, so a run from anywhere but the corpus
  removed nothing and exited 0; it is anchored on the path now, and a path no
  store holds is `nothing to forget: <path> is not indexed` on stderr with a
  non-zero status. `POST /api/forget` answers 404 for that case instead of 200
  with a zero count. `drop` panicked before deleting anything, because its
  positional argument shared the id `store` with the global `--store` flag, which
  clap refuses; the argument is called `name` and the usage line is unchanged.
- **#78 — `GET /api/search` applies the offset it validates.** It asks for
  offset + k hits and returns the page from offset, echoing `offset` back the way
  `/api/files` already does. The other read routes were audited in the same pass:
  every other parameter they validate is used.

### Notes

- **Five answers change where the old one reported success:** a blocked `index`
  run, a `forget` of a path that is not indexed, `POST /api/forget` on the same
  case, the rows `GET /api/search` returns for an `offset`, and the status a
  process ends with when the reader of its pipe goes away.
  `docs/compatibility.md` records each one with what it returned before.
- **A Windows user may have a stray `.semlith`.** Everything #73 wrote into the
  working directory is still there: look in whatever directory semlith was first
  run from, for a `.semlith` holding the stores, the registry and the model
  cache. This release stops creating it and does not move the one already there.
  A user who worked around #70 by setting `SEMLITH_HOME` is unaffected — it is
  still read first, and their stores do not move.
- **#82 — the harness ledger is empty of real rows.** Every entry in
  `.github/known-failures.txt` was a check one of these issues closes, so it goes
  back to holding only its header, and `portal-check.ps1` loses the
  `SEMLITH_HOME` fallback that stood in for the broken lookup.
- **No store format change.** `FORMAT_VERSION` stays 2, no table and no column is
  added, and a 0.17.0 store and a 0.17.1 store are byte-compatible in both
  directions.

## [0.17.0] - 2026-09-15

The code graph covered six of the forty-six languages `--lang` accepts. That gap
was a promise the product did not keep: `--lang kotlin` narrowed a search
perfectly well, and then `semlith symbol` answered nothing about the same corpus
with nothing saying why. It is closed.

### One language table

- **The extractor reads `filter::LANGUAGES`, the table the search filter reads.**
  `graph::LANGUAGES` — a second copy, six entries against forty-six — is gone,
  along with the extension table beside it. `graph::has_graph` answers whether a
  language carries symbols and edges by asking whether a grammar exists, and
  `graph::language_of` dispatches through the same `filter::language_of_path`
  the filter uses. A test enumerates the table and fails for a row that has
  neither a grammar nor an entry in `graph::WITHOUT_GRAMMAR` giving the reason,
  so the two halves cannot drift apart again.
- **Every language has a fixture that says what it must extract.** A grammar
  that compiles and extracts nothing would pass every other gate in the crate.
  `tests/fixtures/graph/<language>/` holds a real file per language and
  `graph::tests::FIXTURES` declares the symbols and edges each one must yield,
  with their kinds.

### Forty grammars

- **Twelve of the new grammars ship a tags query; twenty-eight do not.** Those
  get one under `queries/<language>/tags.scm`, written against that grammar's
  own node names in the same capture vocabulary, and nothing downstream can tell
  the two families apart.
- **Nothing is vendored and nothing is a git dependency.** Every grammar
  resolves from crates.io through `tree-sitter-language`, so `cargo publish` is
  unaffected. Two languages needed a crate other than the obvious one, because
  `tree-sitter` sets `links = "tree-sitter"` and a grammar depending on
  `tree-sitter` itself cannot coexist with the version this crate links: clojure
  uses `tree-sitter-clojure-orchard`, perl uses `ts-parser-perl`.
- **`THIRD-PARTY-NOTICES` ships with the crate and with every release archive,**
  and `tests/licences.rs` refuses a grammar outside a permissive allowlist. All
  forty-six are permissive: forty-three MIT, two Apache-2.0, one CC0-1.0.

### What a symbol is in a language that has no functions

- **Fourteen of the forty-six are markup, data or configuration, and their
  structure is their symbol set.** A YAML, TOML or JSON key, a Markdown heading,
  a Terraform block label, a Dockerfile stage, a Makefile target, a GraphQL
  type, a protobuf message or RPC, a SQL table or view, a CSS selector, an HTML
  element with an id — each with a kind that says which.
- **What they reference is files.** An include, an import, a source, a `FROM`, a
  stylesheet or script `src`, a Makefile prerequisite, the table a view selects
  from. A change to a base image or a shared module has a blast radius, and
  nothing in semlith could show it before.
- **Svelte and Vue contribute their template and say so.** Their grammars hand
  the `<script>` block back as raw text, so a component's methods are not
  symbols and nothing pretends otherwise.

### Fixed

- **C and C++ attributed every call to the file rather than to the function
  making it.** Their bundled queries tag the *declarator*, so a function's range
  stopped at its signature and "where is `helper` invoked" answered `main.c`.
  Live since 0.12.0. The supplement now spans the whole definition, and a
  general rule drops a definition that another of the same name *and kind*
  contains — same name and same kind, so a Rust `mod x` holding an `fn x` keeps
  both.
- **C++ captured no calls at all, C# captured no invocations, JavaScript
  captured no imports, PHP captured neither a require nor a function call, and
  Swift captured no calls.** Each now has the supplement its bundled query
  needed.
- **Elm recorded nothing about a declaration depending on another.** Its bundled
  query captures the name in a type annotation as a reference and the body of a
  declaration not at all, so `acquire = helper` produced no edge between them.

### Surface

- **`semlith languages` prints a `graph` column,** which the portal's About page
  and `semlith_languages` already showed. The About page's language card states
  the coverage rather than a count of exceptions.
- **No new command, tool, argument or schema.** `FORMAT_VERSION` does not move,
  no column is added, a 0.16.0 store opens under 0.17.0 and the reverse. The new
  languages' symbols appear on the next index pass of those files, as every
  graph fact does.

### What it costs

- **The binary more than doubles: 47.0 MiB to 111.3 MiB, +64.3 MiB (+137%).** Measured on
  macOS arm64 against a 0.16.0 build made with the same toolchain, both already
  symbol-light — the growth is parser tables, not debug information. OCaml alone
  is 12.6 MiB of it and the five largest grammars are 36 MiB. There is no
  ceiling by decision: the number is the deliverable. Per-platform release
  archive sizes are in the release assets for this tag.
- **Retrieval moved, and the move was measured rather than assumed.** Against
  0.16.0 on the same machine, the same corpus and the same question set: hit@1
  13 to 11, hit@3 18 to 18, hit@8 27 to 25. `wrong yes` stays 0, which is a
  gate. Four causes were found and measured one at a time; three were defects
  and are fixed above, and the fourth — a same-language preference for ambiguous
  edge targets — was tried, scored two hits worse, and reverted. The residual is
  attributable: this release adds several thousand words of documentation that
  legitimately answer the harness's concept questions, and those questions
  accept only code spans. Seven questions rank lower than in 0.16.0 and three
  rank higher; all ten are named in the release record.


## [0.16.0] - 2026-09-14

0.15.0 made the free product honest and cheap. It did not make it better at
finding things. This release spends the harness that release built on the
ranking itself, and reports what the harness said — including where it said no.

### The graph list stops being a set and becomes a ranking

- **Graph neighbours are ranked by personalised PageRank.** 0.15.0 expanded one
  flat hop from the top eight hits and weighted the result by the confidence of
  the edge that reached it. That treats every neighbour of every seed as equally
  related, so a symbol reached once from a weak hit ranked alongside one reached
  from three strong ones — a set, not a ranking. The walk is now seeded with each
  hit's own fusion contribution, splits a chunk's mass between the symbols it
  holds, and pushes it outward three rounds at 0.85 damping. Neighbours are still
  read one name at a time and are cached, so a name is queried once however many
  rounds run and only the frontier is ever held; `graph::MAX_NODES` still bounds
  it, and ambiguous edges are still refused.
- **A symbol question is answered with one evidence block.** `semlith symbol` and
  `semlith_symbol` return the definition, the resolved callers and callees, and
  the ring two hops out, together. That used to be three tool calls, two of which
  had to be made before the caller knew whether the first had found the right
  symbol. The second ring is walked only through first-ring names that are
  `extracted` or `resolved`: an ambiguous name is several unrelated definitions
  wearing one label, and expanding through one would put another symbol's callers
  into this one's context.
- **An edge records the line the call was written on.** `edges` gains a nullable
  `line` column, so "where is X invoked" returns the call rather than the
  enclosing definition, which can be hundreds of lines away. NULL on every row an
  older binary wrote, and a renderer prints nothing rather than falling back to
  the definition line — a wrong call site is worse than no call site.
- **A walk crosses a re-export.** `pub use x as y`, `export { x as y }`,
  `from x import y as z` and Go's aliased import become `aliases` edges, and
  `aliases` joins the kinds a path may follow. A walk that refused to cross a
  rename answered "not connected" about code that is connected. A re-export that
  does not rename adds nothing: there is no name change to cross.

### Search reads the query before it ranks the answer

- **An identifier and a question are weighted differently.** FTS5 is exact about
  a name and vague about a sentence; the embedding is the other way round, and
  both halves were being scored as though equally likely to know. An
  identifier-shaped query now weights the keyword list twice; a question-shaped
  one leaves the two level. The rule is one token of identifier characters,
  deliberately crude and entirely inspectable, and every surface reports which
  shape it read — a caller can only correct a misread shape if it can see that
  one happened. The harness says this is the single largest win in the release:
  without it, hit@3 falls from 17 to 13 and hit@8 from 25 to 20.
- **`prefer: code | docs | any`** is the correction, on the CLI, over MCP and on
  `/api/search`. It multiplies rather than filters, so `prefer: code` over a
  corpus of prose still answers with the prose.
- **The fused set is reranked by graph distance and freshness.** Constants, no
  model, no learned weights; every input is something the store already holds,
  and both are tiebreaks — the whole span is under 1.5x, asserted — so a chunk
  the query matched badly cannot climb over one it matched well.

### Two new tools, and a tool list that did not grow

- **`semlith read`** returns one span or one symbol and nothing around it: the
  second stage after a locate answer. A name with several definitions returns the
  list rather than choosing. Reads come from the store's chunks and never from
  disk, so the boundary that governs indexing governs reading.
- **`semlith pattern`** runs a tree-sitter structural query over the indexed
  files of one language — the questions a regex cannot ask. The grammar comes
  from the extractor's own table, so a language `pattern` accepts is one the
  graph accepts. An invalid pattern returns the parser's own message rather than
  an empty list, because a caller handed "no matches" concludes the code lacks
  the shape.
- **Twelve tools now fit the budget ten used to.** `tools/list` is 3 995 bytes,
  about 999 tokens, against 3 940 for ten tools in 0.15.0. What paid for the two
  new tools was the `items` schema on the array properties: `path`, `ext`, `lang`
  and `store` have only ever held strings and their names say so. The
  `readOnlyHint` annotations stayed — a client acts on those, and buying bytes by
  making every tool need an approval prompt is the opposite of what the budget is
  for.

### A store has one name

- **The portal drew store chips the search route then refused.** `/api/stores`
  reports the name the daemon registered — what a person typed, and what the
  chips are made of — while the fleet every route answers from derived its own
  label from the store directory's basename. The two agree for a store in
  `~/.semlith/stores/<name>`, where the directory *is* the name, and disagree
  for every store opened by path: clicking the chip returned "no store called
  work is open; these are: store". All three fleets the daemon builds — the one
  it starts with, the one the portal opens lazily, and the reader forwarded MCP
  calls answer from — now take their names from the registry, so `store:` means
  the same thing on every surface.

### The portal

- **Neither new tool gets a page, and that is the decision rather than an
  omission.** `read` already has a view: it is the Search page's second stage,
  where the button sits on the hit that raised the question with the span
  already in hand. A page of its own could only be started by retyping a
  coordinate you got from Search. `pattern` takes a tree-sitter query in
  S-expression syntax, which nobody writes from memory — a page for it is a box
  you can only fill by pasting from documentation, which is a worse manual
  rather than a view. Both tools work on the CLI and over MCP, where the caller
  is an agent that can write the query, and the Agents page lists both with what
  they are for. `tests/portal.rs` records the reasoning beside `start` and `mcp`,
  which are exempt for their own reasons.
- The Search page rebuilt to that design: the query-shape hint, file cards with
  span summaries, word-pill fusion badges, the provenance sentence, a signature
  strip, a line-number gutter, a read-whole-symbol control, and an ego-graph
  panel. The `Prefer` control is live rather than a disabled promise, and the
  shape hint is drawn from what the server actually ranked with rather than from
  a second classifier in the browser.
- Three bugs found by rendering the pages rather than reading the diff: image
  previews were handed a `blob:` URL the page's own CSP had always refused and so
  had never worked; the Read and Pattern views could let a slow earlier answer
  overwrite a newer one; and the graph canvas leaked an animation loop per visit.
- **The Languages page is gone.** Its list lives on About, where the design puts
  it. `semlith languages`, `semlith_languages` and `/api/languages` are
  unchanged.

### What the harness said no to

A third rerank factor — a lift for a chunk sitting inside a named definition —
was specified, built, measured and removed. In a code repository nearly every
code chunk sits inside a definition and nearly no prose chunk does, so it is a
second and blunter `prefer: code` applied to every query, including the ones that
asked for `prefer: docs`. It costs two hits at k=3, and `prefer-code-over-prose`
ranks first without it and third with it. It is recorded in the release's
iteration log rather than quietly dropped.


## [0.15.0] - 2026-09-14

The graph stops answering questions it cannot support, the search stops sending
the thing itself when it was asked where the thing is, and the ledger starts
recording the agents it exists to measure.

A study on 2026-09-14 measured all three: the graph was wrong about half the time
on "does A reach B", with nothing in the output to say so; a search reply cost 20
to 60 times the grep it replaced when the agent already knew the term; and the
`retrievals` table was empty on real installations, because the only thing that
ever wrote to it was the portal's own search box.

### A graph that says how much it knows

- **An edge's target is resolved, not matched by name.** `edges.dst` is a name,
  and a name is not an address: a store of any size holds four `get`s and seven
  `index`es, and every one of them used to come back as its own row,
  indistinguishable from the call the source actually makes. Each edge is now
  ranked against its candidates — the calling file's own definition first, then a
  definition whose file the source pointed at, then one in a file the caller
  imports, then the corpus holding only one definition of the name. One survivor
  is `resolved`; several are `ambiguous`, and every candidate carries the
  definition count so a renderer can say "4 definitions" rather than print four
  calls. An edge the syntax tree already settled stays `extracted`: a fact
  outranks a ranking. Both new values are computed when a query runs and never
  stored, so re-indexing a target cannot make them stale.
- **`edges.hint` records what the source said about where a call goes.** The
  module of a scoped call, the receiver of a method call, the object of a
  qualified Python or TypeScript call. The source almost always says more than
  the name, and none of it was being kept. Rust's supplementary query also stops
  emitting the path segment of a scoped call as a second `calls` edge —
  `store::edges_out()` is a call to `edges_out`, not to `store`, and that edge
  invented a call the source does not make and handed the path finder a module to
  walk through.
- **A path walks definitions, not names.** Refusing ambiguous edges is not enough
  on its own: on the 0.14.0 store every hop of `call_tool -> record_retrieval`
  resolved to exactly one definition and the chain was still false, because hop 3
  arrived at `search` in `lib.rs` and hop 4 left from `search` in `routes.rs`.
  Each hop true, the chain not. A node in the traversal is now a definition, and
  a chain may only leave from the definition it arrived at. The false chains that
  are still false — `search_in` and `is_image` to `record_retrieval` — now answer
  "not connected within 6 hops by resolved edges", and a path that does exist
  still answers with nothing to qualify it. The other two pairs the study named
  became genuinely connected in this release, because this is the release that
  made every surface record a retrieval; the question set carries the measured
  chains and the reasoning.
- **`--all-edges` walks the old way and says what it found.** Both endpoints on
  every hop with file and line, a seam drawn where the chain changed subject, a
  trailer whose four confidence counts add up to the hop count and which names
  the ambiguous names crossed, and one line: "A hypothesis, not a finding."
  `--strict` states the default out loud and wins when both are given. One
  renderer serves the CLI, the MCP reply and the portal, so the three cannot
  describe one answer differently.
- **`semlith neighbors` collapses and hides.** Callees to a name with several
  definitions are one row carrying the count, because four rows saying `get` read
  as four calls and the symbol makes one. Targets the store holds no definition
  for are counted rather than silently omitted. `--all` expands both: "semlith
  shows no callees" and "everything this calls is outside the index" are
  different facts.
- **Search expansion follows only the kinds that mean "depends on"**, skips
  ambiguous names, and weighs a chunk reached through a resolved edge above one
  reached by a bare-name match. Following `defines` and `contains` pulled in
  every symbol that shared a file with a hit, which diluted the two lists that
  had answered the question.

### A search that answers where

- **`semlith_search` answers with locations by default.** A locate row is the
  store-relative path, the line span, the enclosing symbol and its kind, the
  lists that found it, provenance when the graph reached it, whether the file has
  changed since it was indexed, and one line of the text — grouped by file and
  cut to a `max_tokens` budget that states `truncated: 8 of 23` when it cuts. An
  agent that already knows the identifier wants the address, not the building:
  the study measured a warm reply at 4.9–7.2 KB at `k=8` against 100–300 bytes
  for the grep it replaced. **This changes what an MCP client gets back.**
  `format: "excerpt"` returns exactly what 0.14.0 returned, and the CLI is
  unchanged — a person reading a terminal is not paying by the token.
- **Every hit says whether its file has moved under it.** One `stat` per distinct
  path, the current size and mtime against what the store recorded. Deliberately
  conservative, so a `touch` with no edit reads as stale: a false "check this"
  costs a reread and a false "this is current" costs a wrong quotation.
- **`tools/list` is 3 940 bytes with one store open, from 8 634.** Every agent
  paid the old one once per session before asking anything, and most of it was
  advice a model either already knows or will not follow from a schema.
  Descriptions are one sentence each, `title` annotations are gone — the portal
  falls back to the description — and the language list moved to a tenth tool,
  `semlith_languages`, because it is a fact about the build and can be stated
  once rather than read by every agent every session.

### A ledger that records the user it exists for

- **Every retrieval is recorded, from every surface.** The write moved out of
  `routes.rs` into `src/ledger.rs`, and stdio MCP, the daemon's `/mcp` endpoint,
  the CLI and the portal all call it — so a search from the command line and the
  same search from an agent are counted the same way rather than nearly the same
  way. A row says which client asked, using the name the client gives itself in
  the MCP handshake (`claude-code`, `cursor`, `cli`, `portal`), and which session
  it belonged to. Graph answers are recorded against the honest denominator: the
  bytes of every file a grep for that name would have made you read. A retrieval
  that found nothing is recorded and credited nothing, because a ledger that
  remembers only its successes is a marketing document.
- **Recording is on by default, and `semlith start --ledger` is removed.** This
  reverses what 0.12.0 through 0.14.0 said, and the reversal is the point. Those
  releases stated that a local tool which starts logging without being told is
  not different from one that phones home. What that principle bought was a
  ledger nobody switched on: it measured nobody, so the savings figure had no
  denominator and the audit trail had no rows. The principle that actually holds
  is narrower and every clause of it is checkable — the rows never leave the
  store they were written into, the daemon says on every start that it is
  recording and names the flag that stops it, and erasing every row is one
  `DELETE`. `--no-ledger` stops a session, `SEMLITH_LEDGER=0` stops a machine,
  and the daemon prints `ledger: recording (local only; --no-ledger to stop)` or
  `ledger: off for this session` every time it comes up.
- **Both sides of a ratio are counted with the store's own tokenizer.** The same
  `tokenizer.json` the embedding model already loads, out of the same cache,
  under the same pinned digest. Four characters per token remains the fallback
  for a session that never loaded a model — a graph-only session never does — and
  the row says which counted it, so rows counted two different ways are never
  summed. `tokenizers` is now named in `Cargo.toml`; it was already in the tree
  under fastembed, on the same precedent as `image` and `getrandom` there.
- **`semlith ledger --verify` re-walks the chain** and exits non-zero on a break,
  naming the first row that does not verify. **`semlith stats` gains one line:**
  tokens not read, over how many of how many retrievals, with the coverage and a
  `measured` or `modelled` tier. It does not deduplicate files across one
  session's retrievals, so it is an upper bound, and the line names its tier
  rather than implying precision it does not have.
- **Four nullable columns on `retrievals`** — `session`, `tool`, `stale_hits`,
  `tokenizer` — added by an `ALTER TABLE` that runs on open. The hash chain is
  versioned by the row rather than by the store: `tool` is NULL on every row
  written before 0.15.0 and set on every row written since, which is what picks
  the formula. A store holding rows of both kinds verifies end to end, because a
  verify that reported every 0.14.0 ledger as broken would be worse than no
  verify at all.

### A number behind every retrieval claim

- **`tests/retrieval.rs`**, `#[ignore]`d like the other model-downloading tests.
  It runs a fixed set of 41 questions with ground-truth spans from
  `tests/fixtures/retrieval/questions.yaml` — the asker who knows the identifier,
  the asker who knows only the idea, and the asker whose question only the graph
  can answer — and prints hit@1, hit@3, hit@8, bytes per answer, the graph list's
  marginal contribution and the wrong-yes count for `path`. It asserts wrong-yes
  is zero and `tools/list` is under 1 000 tokens. The question set was committed
  before the work it measures, and its spans are line ranges in the 0.14.0 merge
  rather than in the tree being edited to answer them. Run it with `cargo test
  --release --test retrieval -- --ignored --nocapture`.

### Removed

- **`semlith start --ledger`.** The flag now asks for the default, and it is
  removed rather than kept as a no-op: a script that passes it fails at parse
  time and is corrected once, where a flag that silently meant its opposite would
  go on reading as though it were switching something on.

  **This is a break in the covered surface.** `docs/compatibility.md` records it,
  along with the two changes of default — the ledger recording without being
  asked, and `semlith_search` answering with locations where an MCP client
  previously received excerpts.

### Notes

- **Existing stores open unchanged, and `format_version` stays 2.** The five new
  columns are nullable and added by an `ALTER TABLE` beside the
  `CREATE TABLE IF NOT EXISTS` batch every open already runs. A 0.14.0 binary
  opens a 0.15.0 store and never asks for them; a 0.15.0 binary reads a 0.14.0
  store with a NULL hint on every edge and resolves it exactly as 0.14.0 did,
  filling the hints in for a file on the next `index` pass that touches it. The
  hint is a tie-breaker, so its absence costs the ranking a tier rather than an
  answer.

## [0.14.0] - 2026-09-14

Every finding of the security audit of 0.13.0, closed in one release, each with
the test that would have caught it. The restrictions *are* the release:
`docs/compatibility.md` records the four that break something 0.13.0 did, with
the way back for each, and `docs/security.md` is the user-facing version of what
semlith now refuses.

The prebuilt Linux binary also starts on Debian 12 again, which it has not since
0.10.0.

### Nothing else on this machine can act as you

- **H2, L6 — the portal session moved from a cookie to a request header.** Every
  port on `localhost` is the same site, so `SameSite=Strict` never separated
  semlith from a page served by anything else on `127.0.0.1`. The token now
  travels as `Semlith-Token`, which a browser attaches only for this page and
  cannot set on an `<img>`, a form or a stylesheet. The page takes it from the
  printed URL, keeps it in `sessionStorage`, and removes it from the address bar.
  A write additionally needs a JSON content type and, from any client that sends
  fetch metadata, `Sec-Fetch-Site: same-origin`; both are checked before the
  token, so a cross-origin page cannot tell a right guess from a wrong one. The
  query-string token opens nothing. The page and its own static assets are now
  served without a credential, because a browser attaches no header to a
  stylesheet.
- **M1 — the token comes from the OS random source.** It was a blake3 hash of
  the clock, this process's id and the address of a fresh allocation, none of
  which is a secret. A wrong token is answered after 250 ms, doubling to four
  seconds past twenty refusals in a minute, on a thread of its own so the
  throttle cannot become the denial of service it prevents.
- **M9 — a panicking route costs one request.** It used to kill its worker
  permanently and poison every lock it held. The handler is wrapped, a panic
  answers 500, a worker that ends is replaced, and every lock on shared state
  recovers rather than expecting.
- **L1 — what one client can hold is bounded.** The request head is read on its
  own and every refusal decided from it, so a refused request never has its body
  read into memory; a thousand cross-origin attempts leave the daemon's resident
  memory where it started. Connections are capped at 32, a request has ten
  seconds to arrive in full, a streamed response leaves the worker pool, and the
  Files route refuses an offset past ten thousand rather than allocating it.
- **L2 — an `Mcp-Session-Id` from a client is accepted only as the sixteen hex
  characters the server produces**, and anything else is replaced rather than
  reflected into a response header.

### Nothing you did not write decides what semlith answers with

- **M2 — a store semlith did not create is opened only after `semlith trust`.**
  A `.semlith` can arrive inside a repository, and a store is what semlith
  answers from. The refusal names both `semlith trust` and `semlith adopt`;
  `--store` still opens anything. `daemon.json` is read only from a trusted
  store, only when it is mode `0600`, owned by you, naming a live process and
  carrying a 64-hex token. Every connection opens with `trusted_schema` off and
  SQLite's defensive mode on, and refuses writes until a write path asks.
- **M7 — the model weights are pinned to a commit and a digest.** All three
  repositories, sixteen files, recorded in `docs/models.md` with the commit URLs.
  A file that does not match is refused by name with both digests printed. A
  model cache another account owns or can write to is refused naming the fix.
- **L3 — the parsers are bounded.** An image whose header claims more than 64
  million pixels is refused before a decoder allocates for it, a tree-sitter
  parse gives up after two seconds, and the index loop opens each file once and
  reads through a capped reader from that handle.
- **L4 — `/api/image` canonicalises before it looks up**, so an indexed name that
  now points somewhere else is a 404 rather than a read, and everything after is
  decided from one open handle.

### What a leaked agent key gets

- **M6 — the agent-facing index tools work inside a boundary**: the store's
  registered roots or the home directory, never a credential directory and never
  a file named like a credential. Ten directories, thirteen name patterns, one
  table. The hidden-file rule now applies to an explicitly named path too, which
  is how `~/.ssh/id_rsa` used to walk past every rule the walk applied. Every
  refusal is named with the rule that refused it. `semlith index` on the command
  line keeps the deny-list and loses the boundary, and `--include-secrets` turns
  the deny-list off for that run.
- **M6 — `semlith add` refuses an address that is not on the public internet**,
  at every hop: loopback, RFC 1918, link-local, carrier-grade NAT, unique local.
  `SEMLITH_ADD_ALLOW_PRIVATE=1` opts back in.
- **M4 — the agent key stops travelling.** Every stanza names
  `${SEMLITH_AGENT_KEY}`, which the shell block `semlith setup` writes exports by
  reading `~/.semlith/agent.key` at shell start — so no configuration file
  carries it and a rotation needs nothing rewritten. It never reaches a command
  line, `/api/agents` no longer returns it, and `POST /api/agents/reveal` hands
  it over once when somebody presses the button. A config file that still carries
  an old key is rewritten through a temp file created with the original's mode. A
  rotated key expires after fifteen minutes rather than when the daemon exits.

### On disk

- **M5 — what semlith writes is readable by its owner and nobody else.** Every
  directory it owns is created `0700` and narrowed on every open if something
  loosened it; `store.db`, the vector shards, `registry.json`, `agent.key` and
  `daemon.json` are `0600`, created with their mode rather than chmod'ed after.
  Nothing is ever widened. `semlith stats` and the Stores page report a store
  other users can read.
- **M3 — the PATH line `semlith setup` writes is a shell word.** It was inside
  double quotes, which expand `$(…)`, backticks and `$VAR`, so a crafted
  `SEMLITH_HOME` put a command in a file the shell runs at every start. Now
  single-quoted for POSIX shells, escaped for fish, and doubled for PowerShell; a
  home containing a newline is refused naming the variable.
- **L5 — the registry's temp file carries this process's id**, `Registry::dir_of`
  sanitises its argument, and a run with neither `HOME` nor `SEMLITH_HOME` is an
  error naming both rather than a store written into the working directory.

### The release itself

- **#57 — the prebuilt Linux binary starts on Debian 12, Ubuntu 22.04 LTS, RHEL 9
  and Amazon Linux 2023 again.** It needed glibc 2.39 since 0.10.0. The route
  recorded in the workflow — build on an older runner — could not work: the ONNX
  Runtime `ort` downloads references `__isoc23_strtol@GLIBC_2.38`, and a linker
  cannot satisfy a reference to a symbol version the target libc does not have.
  So the Linux binaries stop linking it: they load `libonnxruntime.so` from
  beside themselves, and the release packs Microsoft's own ONNX Runtime — whose
  floor is glibc 2.27 — into the archive, verified against the checksum GitHub
  publishes with it. `install.sh` and `semlith upgrade` place both files. Every
  other build, `cargo install` included, is unchanged. The floor is now asserted
  over the packaged pair and the job fails above glibc 2.35.
- **H1b — `semlith upgrade` has one origin and no way to be told another.** The
  override is compiled out of every build without `debug_assertions`, and
  `install.sh` pins the same origin with no variable at all.
- **H1c, H1d — the download is bounded and the archive is unpacked in this
  process.** The archive read stops one byte past 256 MiB and `SHA256SUMS` past
  64 KiB; the tag is checked against the shape of a version before it reaches a
  URL or a path, including whatever a redirect landed on; `flate2` and a tar
  reader replace `Command::new("tar")`, which was a program chosen by `PATH` on
  the one code path that replaces the running binary. `owned()` canonicalises
  both sides and writability is `access(W_OK)` rather than the owner's bits.
- **M8 — `release.yml` grants `contents: write` to the one job that uploads**,
  and nothing at workflow level: the build jobs held a token with push rights
  while compiling a dependency tree. CI gains a leg that builds the
  `rust-version` the crate declares, which nothing had ever checked — and it
  failed on its first run, because that number was wrong: tree-sitter 0.27 has
  required Rust 1.90 since 0.12.0, so the declared floor is corrected from 1.89
  to 1.90. Nobody on 1.89 loses anything they had; they could not build 0.12.0 or
  0.13.0 either, and what they got was a dependency error rather than a clear
  floor.
- **L7 — `install.ps1` sets TLS 1.2 before its first request**, which is what
  Windows PowerShell 5.1 needs to reach github.com at all.
- **I1, I2, I3 — `SECURITY.md` describes the release that exists.** The
  supported-versions table named a version two behind; the network section now
  lists the four outbound requests semlith makes and names `cdn.pyke.io` as the
  build-time ONNX Runtime source that `ort-sys` hash-pins. `/api/about` no longer
  reports the pid. The `unsafe` inventory is in `AGENTS.md` with the reason for
  each site.

### Added

- `semlith trust <dir>` records a store outside the store home as one you have
  agreed to open, and `semlith trust --list` prints what is trusted. Nothing is
  moved; `semlith adopt` is still the command that moves a store.
- `semlith index --include-secrets` indexes what the deny-list otherwise refuses.
- The portal's Privacy page carries a **Rules** section: one row per rule this
  release adds, each with the daemon's own check of it. The Stores page offers
  Trust beside a store that is open without having been trusted and reports one
  other users can read; the Agents page shows the key masked behind Reveal; the
  Index page lists refused paths with the rule that refused each.
- `docs/security.md`, the user-facing version of what semlith refuses, and
  `docs/models.md`, the pinned commits and digests with the commands to check
  them yourself.

## [0.13.0] - 2026-09-13

The portal becomes the product's own design, agents get an endpoint they can
keep in a config file, and a query can find a picture.

### Added

- **MCP over HTTP, at `http://127.0.0.1:7365/mcp`.** A client that would rather
  hold a URL than spawn a process now can. The same `mcp::answer` the stdio
  server runs answers it, behind the same loopback bind, `Host` allow-list and
  absent CORS the portal enforces, and `semlith mcp` over stdio keeps working
  exactly as it did.
- **A persisted agent key, in `~/.semlith/agent.key`.** 32 bytes from the OS
  random source, written once with mode 0600 and never reminted implicitly, so
  a stanza carrying it is written once and keeps working across daemon
  restarts, upgrades and portal token rotations. It authenticates `/mcp` and
  nothing else: every `/api/*` route still requires the per-run session token,
  so a credential sitting in a client's configuration file cannot reach the
  rotate, adopt or upgrade routes. Pressing Rotate on the Privacy page no longer
  disconnects a connected agent, because the two credentials have separate
  lifetimes and separate reach.
- **`semlith key show` and `semlith key rotate`.** Show prints the key and the
  stanza around it. Rotate mints a new key, tells a running daemon so the
  endpoint takes it up, keeps the previous key valid until that daemon exits so
  a session already open finishes, re-registers Claude Code through its own CLI
  and names every other client that needs the new stanza. `--now` drops the
  previous key immediately.
- **A rotation carries the key into the files that hold it.** `semlith key
  rotate` and the portal's Rotate key button rewrite every client
  configuration file on this machine that already carried the old key — each
  client's documented path, plus the project-scoped files beside wherever the
  daemon was started — and both then list what they changed. A file is touched
  only when the exact old key appears in it, it is replaced through a
  neighbouring temporary file rather than in place, and no file is ever
  created. Rotating used to leave every configured client authenticating with
  a key the daemon had stopped accepting.
- **Bulk forget, and deleting a store.** The Files page carries a checkbox per
  row and forgets the selection in one call, reporting how many files and
  chunks went and naming anything selected that was never indexed;
  `/api/forget` takes `paths` as well as `path`, and one path answers exactly
  as it did. `semlith drop <store>` and the Stores page's Delete remove a
  store's vectors, chunks, graph, ledger and registry entry. The corpus is not
  touched. A store held by a running daemon is closed through it: the watcher
  stops and releases its lock, the readers are dropped, and only then are the
  files removed — so the daemon loses a store rather than its life.
- **An index run says what it is doing.** The stream carries a line before the
  writer reaches the job — it is one thread, and it finishes what the watcher
  is doing first — and then a line per file with what happened to it:
  indexing, unchanged, skipped or removed. Only the files being embedded used
  to be reported, so a re-index of an unchanged corpus said nothing at all
  between "started" and "done", which reads as a hang. The Index view shows
  the outcome, the running chunk count and the rate beside each path.
- **A request no longer waits for the watcher.** The catch-up pass a store
  runs when the daemon starts steps aside the moment anything is queued, and
  finishes itself when the queue is empty again — so the first index from the
  portal on a cold store begins in about 200ms rather than after the whole
  tree. Shutdown interrupts it too.
- **Pause and stop an index run.** Start becomes Pause while a run is on, and
  Stop asks first: stopping undoes everything the run embedded, so the store is
  exactly as it was before it started and the next attempt begins at 0%. A
  half-indexed corpus is worse than none, because nothing in the store says
  which half it is. `POST /api/index/control` takes `pause`, `resume` or
  `stop`; the run is asked at each file boundary, never inside a file. A stop
  asked for while the job is still queued is answered from the queue itself, in
  about a millisecond, because nothing of it has run. A run longer than the
  writer's time slice is re-queued behind whatever the watcher had waiting and
  carries on by itself, on the same stream: the reader is never asked to press
  the button again, and a stop undoes every slice of the run rather than the
  one that happened to be going.
- **An endpoint switch.** `semlith start --no-mcp-http` starts with `/mcp`
  closed, and the Agents page starts and stops it while the daemon runs.
  Closing it drops the route, not the daemon: the stores stay open, the watcher
  keeps running and the portal is unaffected.
- **Image search.** `.png`, `.jpg`, `.jpeg`, `.webp` and `.gif` files are
  embedded locally with CLIP ViT-B/32 into a second vector space beside the text
  index, and a text query is embedded with the matching text encoder, so a
  sentence such as "the architecture diagram with the queue" finds the picture.
  An image hit carries its path and pixel size where a chunk carries a line
  range, in the CLI, in `--json`, over MCP and in the portal, which previews it
  inline. It is not OCR: an image is matched by what it depicts, and a dense
  text screenshot ranks poorly. The two model files are fetched on the first
  image a store indexes rather than at start, go to the same cache as the text
  model, and `--airgap` refuses them by name — a store that never holds an image
  never downloads them.
- **Sorting and pagination on every table the portal renders.** One component
  behind all of them, so a table cannot gain sorting on one page and not
  another. The Files table sorts and pages on the server, so ordering a column
  orders the whole store rather than the rows that happened to be loaded, and
  the silent truncation at 500 files is gone.
- **An `indexed` column on the Files page**, and a header that states files,
  formats and stores from the store's own queries rather than from the rows on
  screen. The Stores page gains the same kind of measured totals: lines across
  every indexed file, how many formats they span and how many readers are in
  use.

### Changed

- **The portal is rebuilt against its design.** The Graph page draws symbols as
  labelled rounded boxes on a full-bleed canvas with arrowheads, solid edges for
  extracted and dashed for inferred, the selection in accent with its
  neighbours lifted, a floating legend, edge-kind filters and a hover card
  naming a symbol's kind, file, store and caller and callee counts. Stores,
  Index, Agents, Ledger, Privacy and About are rebuilt to match. Type, spacing,
  line height, motion, hover and focus all come from one token block rather
  than from each component, and every page has one scrolling region, so a
  header no longer scrolls away with the content under it.
- **The product is written "Semlith" in prose**, and stays lowercase wherever it
  is an identifier: every command, every MCP tool name, `~/.semlith`, the crate
  and the binary. A command inside a sentence is set in mono, which is what
  makes the difference read as a rule rather than as a typo.
- **The Linux prebuilt binaries print their glibc floor**, so the number is in
  the build log and the release notes rather than discovered on someone's
  server. Lowering that floor was attempted for this release and did not work:
  building on a 22.04 runner fails to link, because the prebuilt ONNX Runtime
  that `ort` downloads references `__isoc23_strtol` and
  `std::__cxx11::basic_string<wchar_t>::_M_replace_cold` — glibc 2.38 and a
  newer libstdc++. The floor is the vendored library's, not this crate's, so
  only building ONNX Runtime from source can move it.
  [#57](https://github.com/semlith/semlith/issues/57) stays open with that
  evidence, and `cargo install semlith` builds against whatever glibc the
  machine has in the meantime.
- **`initialize`, `tools/list` and `ping` answer with no store open.** An agent
  connecting to a fresh install is told which tools exist rather than that the
  daemon has nothing to serve.

### Removed

- **`semlith impact`, the `semlith_impact` MCP tool and `GET /api/impact`.**
  Reverse reachability leaves the free product whole rather than in pieces — the
  Impact page, the Graph rail's blast-radius action, the Search page's link to
  it, `Fleet::impact_in` and `graph::impact` go with them, and the advertised
  tool count drops from ten to nine. It returns in 0.14.0 as a paid surface.

  **This is a break in the agent-facing surface.** An agent with
  `semlith_impact` in a saved prompt or a committed `.mcp.json` breaks on
  upgrade, and there is no shim. `semlith symbol`, `semlith neighbors`,
  `semlith path` and their three MCP tools are unchanged.

### Fixed

- **A tooltip inside a table was unreadable.** It was drawn as a `::after` on
  the element it described, and `.table-wrap` carries `overflow: auto`, so a
  scroll container clipped it — the Files page's path tip, which is the reason
  the feature exists, was the one that could not be read. There is now one
  tooltip for the whole portal, in a fixed element outside every scroll
  container, which flips rather than overflowing the viewport and answers
  keyboard focus as well as the pointer.
- **The Index page painted an empty bordered box.** `.note` was declared twice
  — inline in one place and as a padded card in another — and the later
  declaration won for every user of the class. `.pill` had the same defect. Both
  are now declared once.
- **`--airgap` could have fetched a model.** The check asked whether the model
  cache held anything rather than whether *that* model was in it, so a machine
  with the text model pre-seeded would have downloaded CLIP on the first image
  it indexed. It now asks about the model it is being asked to load.
- **A route that no longer exists answers 404** rather than 405. A removed
  route reading as "wrong method" is an invitation to go looking for the right
  one.
- **The install script's progress bar, properly this time.** 0.12.0 claimed to
  have fixed this by resolving the release URL's redirect first. That was the
  wrong diagnosis and the fix was close to a no-op: measured against the real
  16 MB asset it took the bouncing `#=O=-` frames from 77 to 74. The cause is
  not redirects. curl's `--progress-bar` draws that indicator for as long as it
  does not know the transfer size, and over HTTPS that is the whole connect,
  TLS and header phase — 72 of 100 frames on a request with no redirects at all
  and a `Content-Length` present. So curl's meter is off now and the installer
  draws its own bar from the size the `HEAD` already reports: zero bouncing
  frames, a bar that moves from 0% to 100%, and a failed download still fails
  the install rather than showing a finished bar over a truncated file.

## [0.12.0] - 2026-09-13

The store learns the shape of the code in it, and search uses it.

### Added

- **A code graph, extracted on the same pass that re-embeds a file.** Symbols
  and the `defines`, `calls`, `imports`, `references` and `contains` edges
  between them, read out of the syntax tree by tree-sitter, for Rust,
  TypeScript, Python, Go, Java and C. It hangs off the blake3 hash-change path
  that already drives re-embedding, so an edit under `semlith start` updates the
  graph in the same pass that updates the vectors. There is no build step and no
  artifact that can quietly go stale — which is the failure every snapshot-based
  code graph has, and the reason this one is not a snapshot.
- **Extracted or inferred, on every edge.** A call whose name the file also
  imports was resolved by the file itself; a bare name match was not. Two
  functions called `new` in different modules is the normal case in real code,
  not a corner case, so the difference is recorded and shown everywhere an edge
  appears. Nothing presents the second as the first.
- **`semlith symbol`, `semlith neighbors`, `semlith path`, `semlith impact`**, and
  the same four as the MCP tools `semlith_symbol`, `semlith_neighbors`,
  `semlith_path` and `semlith_impact`. Where a symbol is defined; what calls it
  and what it calls; the shortest chain of edges between two symbols; and what
  breaks if this one changes. `impact` is the answer to the most expensive
  mistake an agent makes on a real codebase — patching the call site that was
  reported and leaving every sibling caller broken.
- **Graph-aware search.** The top vector and keyword hits are mapped to the
  symbols in them, expanded one hop, and the chunks those neighbours live in
  become a third ranked list fused at the same weight as the other two. It costs
  no embedding and no model call, and it reaches the case neither existing list
  can: a concept spread across files that share no vocabulary. Every hit now
  says which lists found it — `v`, `f`, `g` on the command line — so a result the
  graph alone reached is legible as a neighbour of a match rather than a match.
- **A Graph page and an Impact page in the portal.** The graph on a canvas with
  a force layout that can be paused, panned, zoomed and dragged, beside a rail
  carrying the selected symbol's callers, callees and chunks. Reverse
  reachability as a table and as rings by hop, with a path finder. Both are free
  on every tier, now and after 0.13.0 makes the product paid.
- **An opt-in retrieval ledger.** `semlith start --ledger` records every query an
  agent ran into a hash-chained `retrievals` table inside the store: the query,
  the client, the hits, the excerpt tokens they actually read and the whole-file
  tokens a grep loop would have cost. Each row carries the hash of the row
  before it, so an edited or deleted row is detectable rather than merely
  unlikely. `semlith ledger --last N` prints it, the Ledger page shows the
  totals and the measured ratio, and none of it needs a key or leaves the
  machine. It is off unless asked for: a local tool that starts recording what
  you searched for without being told to is not meaningfully different from one
  that phones home.

### Fixed

- **The install script's progress bar.** `curl -L --progress-bar` draws a bar for
  every hop of a redirect, and a hop carries no content length, so curl fell
  back to its bouncing `#=O=-` spinner. A GitHub release URL always redirects,
  so every install showed that before the real bar arrived, and a working
  download looked like line noise. The final URL is resolved first, which gives
  one request against a known size and one bar from 0% to 100%.

### Notes

- **Existing stores open unchanged.** `format_version` stays 2 and nothing
  migrates: the new tables are created by the same `IF NOT EXISTS` batch every
  open already runs, so a store written by 0.11.0 opens with an empty graph and
  fills it on the next `index` pass, and a store written by 0.12.0 still opens
  under 0.11.0. Both directions are tested against the released 0.11.0 binary.
  `docs/compatibility.md` explains why the number did not move.
- **Measured.** Extraction costs 98 ms over 589 KiB of source producing 757
  symbols and 5,536 edges — 0.17% of the 58 s that corpus takes to index, because
  embedding dominates and tree-sitter is fast. The six grammars add 4.6 MB to
  the binary, 38.6 MB to 43.2 MB.
- **Six languages carry edges.** Everything else is searchable exactly as
  before, with no symbols. The About page says which is which.
- **Everything in this release is free on every tier, permanently.** 0.13.0
  introduces paid tiers by adding views above these, never by locking one.

## [0.11.0] - 2026-09-12

The rest of a real archive, and a way to put something into a store that was
never a file on the disk.

### Added

- **EPUB, RTF, `.eml` and `.mbox` readers.** A book is read as its chapters in
  the order the spine gives, which is not the order the filenames sort in — so a
  book no longer opens on its copyright page. An RTF document is read as the
  text a word processor would show, with font and colour tables, style sheets,
  embedded pictures and revision metadata skipped whole and `\'hh` and `\uN`
  escapes decoded. A message is read as its `From`, `To`, `Cc`, `Date` and
  `Subject` followed by its body: the `text/plain` part of a multipart, or the
  HTML part run through the existing HTML reader when there is no plain one.
  Headers are unfolded and RFC 2047 encoded-words decoded, so a subject with an
  accent in it is searchable by the word rather than by `=?utf-8?Q?`. An
  attachment is named and never decoded. An `.mbox` is every message in the
  file, each one marked with its own subject. None of it adds a dependency: an
  EPUB is a ZIP of XHTML, so it costs the archive reader and the HTML scanner
  that were already here, and the other two are hand-written scanners.
- **`semlith add <URL>`**, on the CLI, as the `semlith_add` MCP tool, and as a
  URL field on the portal's Index page. Fetches one https URL into the store's
  own `downloads/` directory and indexes it through the readers that already
  exist — a web page, a PDF such as an arXiv paper, or a file on GitHub, whose
  `/blob/` link is rewritten to the raw file it displays. The directory is
  registered as a root of the store, so `semlith start` keeps what was added
  current.
- **Twenty-one more languages for `--lang`**: zig, dart, elixir, terraform,
  powershell, julia, fortran, r, perl, dockerfile, makefile, vue, svelte, proto,
  graphql, nix, clojure, erlang, elm, groovy and objective-c, bringing the table
  to 46. Two of them have no extension to match on, so a language may now be
  identified by filename as well: `--lang dockerfile` finds a bare `Dockerfile`
  and a `Dockerfile.prod`, and `--lang makefile` finds `Makefile` and
  `GNUmakefile`.
- **A Languages view in the portal**, listing all 46 with the extensions and
  filenames that make each one up, read from the same table the filter uses.

### Changed

- The portal's Files view shows a language for a file that has no extension,
  where it previously showed nothing.

### Security

- `semlith add` is the first outbound connection semlith makes that is not the
  model download or `semlith upgrade`, and it holds to the same rules. It is
  https-only and refuses plain http rather than silently upgrading it; it
  refuses a redirect that leaves https and a chain longer than five; it caps a
  body at 32 MiB; it refuses a content type no reader handles, cross-checking
  the declared type against the first bytes; it never sends a credential; and
  under `--airgap` it refuses before a socket is opened. The path it writes to
  is derived from the URL, so escapes are decoded before the path is split,
  every segment is sanitised, and the result is checked to be inside the
  downloads directory before anything is written. Nothing is crawled and nothing
  is re-fetched. See [#50](https://github.com/semlith/semlith/issues/50).

## [0.10.0] - 2026-09-12

One command on a fresh machine. No Rust toolchain, no package manager, nothing
installed first — and on Linux, no OpenBLAS and no OpenSSL either, because the
binary no longer needs them.

### Added

- **`install.sh` and `install.ps1`**, at the root of the `main` branch and
  reachable from `raw.githubusercontent.com`. Each detects the machine's
  target, resolves the newest release through the `releases/latest` redirect
  (`SEMLITH_VERSION` pins one instead), downloads the archive with a progress
  bar, verifies it against the release's `SHA256SUMS` before writing anything,
  unpacks into `~/.semlith/bin`, and hands off to `semlith setup`. A failed
  download or a bad checksum leaves nothing behind: no bin directory, no
  partial file outside a temp directory removed on exit. Intel macOS gets the
  ONNX Runtime explanation and no partial install.
- **`semlith setup [--yes]`**: a guided terminal flow that puts
  `~/.semlith/bin` on `PATH` for your shell, pre-downloads the embedding model
  so the first `index` is not a silent wait, registers semlith with the agents
  you pick — Claude Code through its own CLI, every other client by printing
  the stanza and the config path — and ends by running the installed binary's
  `--version`. Every step is idempotent and reports what is already done, so it
  is the repair command as well as the install one. `--yes` takes the default
  at every prompt and needs no terminal, so a script or an agent can install
  semlith unattended.
- **`semlith upgrade [--check] [--version <tag>] [--airgap]`**: fetches the
  newest release for this machine, verifies it against `SHA256SUMS`, and swaps
  the binary by renaming the old one aside and the new one into place — the one
  sequence that works on Linux, macOS and Windows alike. `--check` prints both
  versions, changes nothing, and exits 10 when an upgrade exists. It refuses
  under `--airgap` before opening a connection, refuses a binary it did not
  install, and never runs unprompted: no startup check, no timer, no banner.
- **Portal parity.** The welcome screen and the Agents view gain an "Install and
  setup" panel: both one-liners with copy buttons, a row per setup step with
  what it found, whether the bin directory is on `PATH`, the installed version,
  and Check-for-updates and Install buttons that only ever fire on a click. The
  panel reads `/api/setup`, which is `setup::status()` — the same function the
  terminal prints — so the page cannot claim `PATH` is set up when it is not.

### Changed

- **The Linux binary needs nothing but glibc and libstdc++.** fastembed moves to
  `ort-download-binaries-rustls-tls` and hf-hub to its ureq-only feature set,
  which takes reqwest, tokio and native-tls out of the build entirely — 607
  lines leave `Cargo.lock`. The release workflow now reads the built binary's
  `NEEDED` entries with `readelf`, prints them, and fails on `libssl`,
  `libcrypto` or `libopenblas`, so this cannot regress quietly.
- **The OpenBLAS instruction is gone** from the README, `CONTRIBUTING.md`,
  `AGENTS.md` and both workflows. turbovec 1.0.0 dropped its BLAS dependency and
  the documentation had not caught up: the step had been unnecessary for a
  release already.
- **The README's Install section leads with the two one-liners.** Homebrew,
  winget and Scoop are listed as coming; `cargo install` and the release archive
  are kept below as alternatives. The release notes' install text is now lifted
  out of the README between markers, so the two cannot drift apart.
- **One new runtime crate**: cliclack 0.5.6, for the prompts, the spinner and
  the progress bar. `sha2` and `ureq` are now named directly and were already in
  the tree through hf-hub, so neither adds anything to the build.

### Documentation

- `docs/compatibility.md` now covers the install script URLs and the
  environment variables they honour, the release archive layout both they and
  `semlith upgrade` read, `semlith setup --yes`, and `semlith upgrade --check`'s
  exit codes. `SEMLITH_RELEASES_ORIGIN` is listed as explicitly not covered: it
  redirects the release lookup for the tests and is not a way to self-host.

## [0.9.0] - 2026-09-12

A store no longer has to live inside the repository it indexes, and a portal in
your browser shows you what is in it. `semlith start` is one process that owns
every store, keeps them current as you save, and serves that page on
`127.0.0.1` — which also ends the choice between a watcher keeping a store
current and an agent being able to write to it.

### Added

- **A store home.** `semlith index` with no flags now creates
  `~/.semlith/stores/<name>` and records the directory it indexed in
  `~/.semlith/registry.json`. `SEMLITH_HOME` moves it; `--name` names a store
  explicitly. A store can no longer land in a tracked tree by default.
- **`semlith adopt <dir>`** moves an existing store into the home and registers
  it — a rename where it can be, a verified copy where it cannot, and no
  re-embedding either way. `--root` re-points a registered store whose corpus
  moved.
- **`semlith start`**: takes every registered store's write lock, runs the
  watcher over their roots, and serves a portal on `127.0.0.1:7365`. `--port`
  and `SEMLITH_PORT` change the port; nothing changes the address. If the port
  is taken it exits saying so rather than picking another, because the URL is
  meant to be a bookmark.
- **The portal**, compiled into the binary: a first-run welcome screen, and
  Stores, Files, Search, Index, Agents, Privacy and About. It shows each store's
  counts and the watcher's live event feed, the indexed files with the CLI's
  `path`/`ext`/`lang` filters and the reader that parsed each one, the same
  fused search the CLI and the MCP tools run, a folder picker whose index run
  streams progress as it happens, the client stanza for every documented agent,
  and a Privacy page with a packet-capture recipe.
- **`semlith mcp` forwards to a running daemon.** An agent's `semlith_index`
  used to be refused for as long as a watcher held the store; now the daemon is
  the writer and the agent is a client of it. Client configuration does not
  change, and with no daemon running `semlith mcp` behaves exactly as 0.8.0 did.
- **`--airgap`**, and `SEMLITH_AIRGAP=1`, on `index` and `start`: refuse to
  download model weights and exit naming the cache path, so a machine that
  pre-seeded `SEMLITH_MODEL_CACHE` can prove nothing was fetched.
- **A token rotate route**, behind the Privacy page's button: the old token is
  refused from the next request.

### Changed

- **Client stanzas lost their paths.** Every README stanza is now `semlith mcp`
  with no arguments, because the server opens every registered store plus a
  `.semlith` beside the working directory. Index a second repository and the
  agent that is already configured can search it, with no file to edit.
  `--store` still works and still wins when given.
- `semlith mcp` with no flags opens every registered store rather than
  `./.semlith` alone.
- The out-of-scope note in `AGENTS.md` and `docs/architecture.md` now states the
  loopback rule instead of forbidding a server, per
  [#41](https://github.com/semlith/semlith/issues/41). A CLI command or MCP tool
  is not done until its portal view exists, and `tests/portal.rs` is the gate.
- `turbovec` 0.9.0 → 1.0.0: `search_with_allowlist` returns a `Result` instead
  of panicking on an id the index does not hold, which is the better contract
  for a shard that legitimately lacks an id its range covers.
- `fastembed` 6.0.1 → 6.0.3.
- Three further Dependabot updates that landed on `develop` while this release
  was in flight and are merged into it rather than shipped separately: a
  patch-updates group of two crates, `Swatinem/rust-cache` in both workflows,
  and `softprops/action-gh-release` 3.0.2 → 3.0.3. Both actions stay SHA-pinned
  with their trailing version comment.
- `chacha20` in the lockfile, which had been yanked upstream. It arrives through
  `pdf-extract` → `lopdf` → `rand` and nothing here calls it, but
  `cargo install --locked` would otherwise pull a yanked crate.

### Security

- The portal binds `127.0.0.1` only, with no flag to change it. Every request
  needs a per-run token carried in a `SameSite=Strict; HttpOnly` cookie — 401
  and an empty body without it. The `Host` header must be `localhost`,
  `127.0.0.1` or `::1`, or the request gets 400. Every response carries a
  `Content-Security-Policy` allowing only `'self'`, and no CORS header is
  emitted anywhere. Each of those is asserted by a test.

### Dependencies

- **No new crate.** The HTTP server, the HTTP client the proxy uses, and the
  portal are all written against `std`, so the dependency count is unchanged at
  14 direct dependencies plus one dev-dependency — the same list 0.8.0 shipped,
  with `turbovec` and `fastembed` bumped. The portal adds 149 KB of embedded
  assets — HTML, CSS, JavaScript and five IBM Plex latin faces under the SIL
  OFL 1.1 — against a 1 MiB budget a test enforces.

## [0.8.0] - 2026-07-30

The documents a corpus is actually made of. A notebook, a Word file, a slide
deck, a spreadsheet or an HTML page is now read as the text a person opening it
would see, rather than as its markup, its JSON, or not at all.

### Added

- **Nine more formats, behind the same extraction the PDF reader already sat
  behind.** No new command and no new flag: `semlith index` reads them because
  it walked past them.

  | Extension | What is taken from it | Markers |
  | --- | --- | --- |
  | `.ipynb` | Every cell in notebook order, source and outputs. Stream output and a result's `text/plain` are kept, truncated at 2000 characters each; images, widgets and other MIME types are dropped. | `# Cell 3 (code)`, `# Output:` |
  | `.html`, `.htm` | The page's text. Tags go, `<script>` and `<style>` contents go with them, character entities are decoded. | — |
  | `.docx` | Paragraphs in document order, one per line; a table row's cells tab-separated. | — |
  | `.pptx` | Each slide's text, slides in numeric order. Speaker notes are not included. | `# Slide 11` |
  | `.xlsx` | Each sheet in workbook order, a line per row, tab-separated cells, shared and inline strings resolved. | `## Sheet: Q3 Notes` |
  | `.odt`, `.odp`, `.ods` | The same, from OpenDocument's `content.xml`. | `# Slide 2 (Intro)`, `## Sheet: Q3 Notes` |

  A marker exists wherever a format has a division a line number cannot
  express, so an excerpt says which slide or which cell it came from.

  Extracted HTML keeps every newline the source had, including the ones inside
  the tags that were removed. That is what keeps a hit's `file:line` range
  pointing at the line of the file on disk where the sentence lives, rather than
  at a line number that only exists after extraction. A spreadsheet is indexed
  as its cached cell values; formulas are not evaluated.

- **A cap on what an archive may decompress to: 32 MiB of text.** Six of the
  nine formats are ZIP archives, and the existing 8 MiB file cap only bounds the
  compressed file on disk.

  *Why.* A few hundred kilobytes of zeros expand to gigabytes. Without a bound
  on what comes out, the size of a run's largest allocation would be chosen by
  whoever wrote the file rather than by semlith. 32 MiB is more text than any
  real document holds and small enough that reaching it is a decision.

- **`zip` 8.6.0** (MIT), with `default-features = false` and only
  `deflate-flate2` — the decompressor and nothing else, no compressors and no
  ciphers.

### Changed

- **HTML files and notebooks are indexed differently than before, not just
  additionally.** Under 0.7.0 an `.html` file was indexed as its raw markup and
  a notebook as its raw JSON; both are now indexed as their text.

  *Why.* A notebook chunk was mostly `"cell_type"`, `"outputs"` and escaped
  newlines, and an HTML chunk was mostly attributes and closing tags. What a
  person is searching for is the third line of the fourth cell, or the sentence
  in the paragraph — so that is what gets embedded.

  *What to do.* Nothing for files indexed from now on. But indexing is keyed on
  content hashes, so a file already in a store is not re-read while its contents
  are unchanged: an existing store keeps its old markup and JSON chunks for
  those files until they change or the store is rebuilt. To convert one file,
  `semlith forget <PATH>` — it drops the file's chunks and its recorded hash, so
  the next `index` run reads it afresh. It takes exactly one path and no globs,
  so for a corpus of them the shorter route is to delete the store directory and
  index again. Neither is urgent; a stale chunk is worse retrieval, not a
  broken store.

- **A document that cannot be read is a skipped file.** Corrupt, truncated,
  password-protected, or over a cap — it lands in the run's `skipped` total,
  exactly as an unreadable PDF has since 0.1.0, and the run still exits 0 with
  every other file indexed. A panic inside an extractor is caught and becomes a
  skipped file too: these readers sit downstream of a decompressor and a
  document somebody else wrote, and one bad file should not end a run that is
  minutes from finishing.

- **No store format change and no migration.** `format_version` is untouched, a
  0.8.0 store is readable by 0.7.0 and a 0.7.0 store by 0.8.0, and downgrading
  loses the new formats and nothing else.

## [0.7.0] - 2026-07-30

A corpus larger than a repository. The first index of one says where it has got
to, survives being interrupted, and searching it costs a bounded amount of
memory rather than an amount that grows with the corpus.

### Changed

- **Breaking: a store created by 0.7.0 has a new layout, and an older semlith
  will not read it.** Vectors now live in a directory of fixed-size shards under
  `index/` instead of a single `index.tv`, recorded as `format_version` 2.

  *Why.* Everything else in this release follows from it. A search can hold a
  few shards instead of the whole corpus; a change to one file rewrites one
  shard instead of the entire index, which is what `semlith watch` does on every
  save; and an index run can make its work durable as it goes, because a
  checkpoint is a shard that has landed. None of the three is possible while the
  vectors are one file that must be written whole.

  *What to do.* Nothing, for a store you already have: 0.7.0 reads, searches and
  indexes into every store written before it exactly as 0.6.0 did, leaves it on
  its single `index.tv`, and does not touch its `format_version`. Only stores
  created by 0.7.0 are sharded. To move an existing store onto the new layout,
  delete the store directory and index it again — the vectors in an `index.tv`
  are quantized and cannot be split back out, so there is no migration that
  would not re-embed the corpus anyway. For a large corpus that costs hours;
  there is no hurry, and nothing breaks if you never do it.

  *If you downgrade.* A 0.6.0 binary meeting a 0.7.0 store refuses it, naming
  both format numbers. A 0.5.0 binary is older than `format_version` and has
  nothing to check, so it reads such a store as an empty corpus — if you keep a
  0.5.0 binary around, do not point it at a store 0.7.0 created.

### Added

- **A resident-memory budget for the vector index.** `SEMLITH_INDEX_MEMORY`, in
  megabytes, defaults to 512. A sharded store holds at most as many shards as
  that allows, putting down the coldest one to make room, and says on stderr
  when it has had to. `semlith stats` reports the shard count and the budget, so
  what a store costs to search is legible before searching it.

- **Checkpointed indexing.** A sharded store makes its vectors durable every
  thirty seconds and only then records the files they cover as indexed — in that
  order, because a hash written ahead of its vectors is a file the next run
  believes it has. An index run killed partway keeps everything committed so
  far, answers searches from it immediately, and the next run walks past it
  rather than starting again. Under 0.6.0 the same interruption left nothing.

- **Progress that predicts.** `semlith index` now says how many files of how
  many it has walked, the chunks per second it is managing, and an estimate of
  what is left. A run that resumes says how many files it skipped as already
  indexed.

- **Opening a store loads no vectors.** `stats`, `files` and `forget` read the
  index only if they must, and an MCP server holding several stores open for an
  agent costs nothing for them until something is searched.

### Performance

- Changing one file in a sharded store rewrites one shard rather than the whole
  index — the difference between watching a monorepo and not being able to.
- Search latency is unchanged for a store that fits inside its budget. A store
  larger than its budget pays to read shards back on each query; that cost is
  measured and reported rather than smoothed over.

## [0.6.0] - 2026-07-30

semlith works from whichever agent you already use, the MCP tools cover the
store rather than a fifth of it, and what counts as stable is written down.

### Fixed

- **The server no longer claims protocol revisions it cannot speak.**
  `initialize` answered with whatever `protocolVersion` the client asked for, so
  a client on `2026-07-28` was told semlith spoke `2026-07-28` — a revision that
  had removed the very handshake it was answering. semlith now holds a list of
  the revisions it implements and answers with the requested one when it is on
  that list, or with the newest one it does implement when it is not.

### Added

- **MCP `2026-07-28`, the stateless revision, alongside the handshake.** That
  revision deleted `initialize`, made `server/discover` mandatory and moved the
  protocol version onto every request. semlith serves both eras, deciding per
  message rather than per connection: a request carrying
  `_meta.io.modelcontextprotocol/protocolVersion` gets `resultType`, server
  identity in `_meta`, and `ttlMs`/`cacheScope` on `tools/list`; anything else
  gets exactly the answer 0.5.0 sent. A revision semlith does not implement
  comes back as `-32022` naming the ones it does.

  The advertised list is `2026-07-28`, `2025-11-25`, `2025-06-18`,
  `2024-11-05`, and every entry has a recorded session in `tests/mcp.rs`
  proving it. `2025-03-26` is deliberately absent: it is the only revision that
  required JSON-RPC batching.

- **Three more tools, so the MCP surface covers what the CLI does.**

  | Tool | What it does |
  | --- | --- |
  | `semlith_files` | Which files are indexed, narrowed by the same `path`/`ext`/`lang`/`store`, capped and saying how many it left out. |
  | `semlith_index` | Index a path into an open store, so a corpus becomes searchable mid-conversation. |
  | `semlith_forget` | Drop one file from a store. The file on disk is untouched. |

  `semlith_files` exists because an agent that cannot ask "is this indexed"
  reads an empty search result as "the corpus does not discuss this".

  Both writers take the store's lock for the call and give it back, so a store
  a `semlith watch` is holding comes back as a tool error naming the holder
  rather than as a corrupted index. With more than one store open they require
  a `store` argument: a store takes one writer, and there is no "the" store to
  guess at.

  `semlith_index` works to a wall-clock budget — 45 seconds, under the
  60-second tool timeout clients default to — then returns what it reached and
  how much is left. Calling it again continues rather than restarting, because
  indexing has been keyed on content hashes since 0.1.0. It never creates a
  store, so no tool call can contain a model download.

- **A setup stanza for every MCP client in common use**, each verified against
  that client's own documentation: Claude Code, Claude Desktop, Codex, GitHub
  Copilot in VS Code, Copilot CLI, Cursor, Windsurf, Zed, Gemini CLI,
  JetBrains, Cline and Goose. `tests/clients.rs` extracts every stanza from the
  README, parses it, and runs its command line against a real server, so a flag
  renamed in the code and not in the README fails the build rather than
  somebody's first attempt.

- **[`docs/compatibility.md`](docs/compatibility.md)** — which surfaces are a
  contract (CLI commands and flags, MCP tool names and schemas, the advertised
  protocol revisions, the store on disk, the `lib.rs` API) and which are free to
  change (ranking scores, human-readable output, stderr, the default model,
  additive JSON fields). It is explicit that 0.x is what backs the promise.

- **`format_version` in the store's meta table.** A store created by 0.6.0
  records format 1; a store written before the key existed is read as format 1
  and never rewritten; a store from a format the binary does not know is refused
  naming both numbers instead of misread. Written down now, while nothing has
  changed, so the first change that does happen fails loudly.

- **`SEMLITH_MCP_INDEX_BUDGET`**, in seconds, for a client whose tool timeout is
  not the usual one.

- **The MCP server says what it is on stderr** — the stores it opened, and the
  revision it negotiated when that differs from what was asked. stdout is the
  protocol; stderr is the only channel a stdio client captures, and "the agent
  sees no tools" was otherwise undiagnosable.

### Changed

- **`semlith forget` takes the store's write lock.** It rewrites `index.tv`
  exactly as indexing does, so it is a writer and now waits its turn like one.
  A `forget` that ran while `semlith watch` was saving could leave the index and
  the database disagreeing about which chunks exist. It now exits non-zero
  naming the holder instead.

### Performance

Measured on an Apple M1:

- The tool list grew from 2220 bytes for two tools to 4790 for five — roughly
  555 estimated tokens to 1197, measured through both release binaries. It is
  loaded into an agent's context once per session whether or not a tool is
  called, so three more tools cost about 640 tokens a session.
- An MCP server at rest holds no store lock: `semlith index` in another terminal
  against the store an open server is serving succeeds.

## [0.5.0] - 2026-07-30

One query across several stores, so an agent working across repositories asks
one question instead of one per repository.

### Added

- **`--store` is repeatable on the read commands.** `search`, `stats`, `files`
  and `mcp` cover every store named:

  ```sh
  semlith search "how is the store lock taken" -s ../api/.semlith -s ../cli/.semlith
  ```

  ```
  1. 0.033  [api] src/lock.rs:14-31
  2. 0.032  [cli] src/main.rs:96-104
  2 hits in 4.1ms across 2 stores: api 1, cli 1
  ```

  `-k` stays global — ten results over three stores is ten results. Filters
  apply to every store, and a filter matching files in only one of them returns
  that one's hits rather than reporting that nothing matched. Stores whose
  embedding models differ can be searched together: each embeds the query with
  its own model, and nothing downstream compares two models' numbers.

- **Every hit says which store it came from**, in the text output, in `--json`
  as a `store` field, and over MCP. A store is named after the directory holding
  it, so `../api/.semlith` is `api`; two stores that would collide get their
  paths instead.

- **`SEMLITH_STORE` accepts a list**, split the way `PATH` is, so an MCP server
  definition can name several stores without a wrapper script.

- **One MCP server, several stores.** `semlith_search` gained an optional
  `store` array that narrows a query, the tool description lists the stores that
  are open, an unknown name comes back as a tool error naming the real ones, and
  `semlith_stats` reports one line per store.

  Adding a store is cheap: measured on three 300-file stores sharing a model,
  one query embed per search rather than one per store, a median 3.4ms for one
  store against 4.0ms for three, and a server holding 137 MB whether it was
  opened on one store or on three — one loaded model, not three.

### Changed

- **A read command refuses a store path that is not already a store**, instead
  of creating one. `search`, `stats`, `files` and `mcp` exit non-zero naming the
  path. Previously a mistyped `--store` became an empty store that answered
  every question with nothing — invisible in a multi-store query, where the
  other stores still return hits. `index` still creates the store it is given.
- **`index`, `watch` and `forget` take exactly one `--store`** and exit non-zero
  if given more. They write, and a store has one writer.

### Notes

No schema change and no index format change: a 0.4.0 store is searched by 0.5.0
with no re-index, and a store written by 0.5.0 is read by the 0.4.0 binary. A
single-store invocation prints and serializes exactly what 0.4.0 did — the store
label is absent when there is nothing to tell apart.

## [0.4.0] - 2026-07-29

Keeps a store current while you work: files are re-embedded as they are saved,
and an agent already connected over MCP sees the change without restarting.

### Added

- **`semlith watch [PATHS...]`.** Stays running and re-embeds files as they are
  saved, so `semlith index` stops being something you have to remember.

  ```sh
  semlith watch ~/notes ./src
  ```

  It begins with the same incremental pass `index` runs — so whatever changed
  while nothing was watching is caught up — and then waits on filesystem events
  rather than polling. Measured on a 1000-file corpus: no measurable CPU over a
  60-second idle window, and about a second from a save to that text being
  searchable.

  New files are indexed, deleted files lose their chunks and vectors, and a
  rename moves the file rather than duplicating it. An editor that saves by
  writing a temp file and renaming it over the original is treated as an edit,
  not as a deletion followed by a new file. Ignore rules come from the same walk
  `index` uses, so `.gitignore`, hidden files and the store's own directory are
  skipped by construction rather than by a second set of rules.

  Events are batched until things go quiet for `--debounce` milliseconds (500 by
  default), so one save — or a formatter rewriting a file three times — costs one
  re-embed and one index write.

- **A long-running reader now sees another process's writes.** The store counts
  index rewrites, and a search reloads the vector index when that count has
  moved. In practice: run `semlith watch` beside your agent's `semlith mcp`
  server, and the agent's answers track your working tree with nothing
  restarted. The check is one SQLite read per search.

- **Ctrl-C stops a watcher cleanly.** `SIGINT` and `SIGTERM` end it at a batch
  boundary, so `index.tv` is written whole or not at all and no temp index is
  left behind. A second signal kills it outright. Unix only; on Windows the
  default terminate-immediately behaviour applies.

### Changed

- `semlith watch` holds the store's write lock for as long as it runs. A store
  has one writer, so `semlith index` against a watched store exits non-zero and
  names the watcher's process. Searching is unaffected.
- An indexing pass that changes nothing no longer rewrites `index.tv`.

### Compatibility

No schema change and no index format change. A 0.3.0 store is watched without
re-indexing, and a store written by 0.4.0 opens, searches and indexes under
0.3.0 — both verified against the 0.3.0 release binary. The only addition to the
store is one row in the existing `meta` table.

## [0.3.0] - 2026-07-29

Narrows a search to part of a corpus, so one store per repository can answer a
question about one subsystem.

### Added

- **`--path`, `--ext` and `--lang` on `semlith search`.** Each is repeatable.
  Repeats union, kinds intersect: `--ext rs --ext toml` is "Rust or TOML",
  `--path 'src/**' --ext md` is "Markdown, under `src`".

  ```sh
  semlith search "how does retry backoff work" --path 'src/http/**'
  semlith search "how does retry backoff work" --lang rust
  ```

  The filter is applied *before* either half of the search picks its results.
  Asking for eight hits inside a subdirectory returns the eight best hits in
  that subdirectory, not whatever survives filtering the eight best hits in the
  repository — which for a small subdirectory is usually nothing. Concretely:
  the vector index is scanned under a turbovec allowlist and the FTS5 query
  carries the same path predicate, both derived from one id-selection query, so
  rank fusion never sees a chunk one half was forbidden to return.

  Path patterns are SQLite `GLOB`. A pattern that does not start with `/` is
  anchored as `*/<pattern>` against the stored absolute path, so `src/**`
  works from any working directory; an absolute pattern means exactly itself.
  `*` crosses `/`, so `src/*` already reaches the whole subtree. Matching
  ignores case, so `--ext md` finds `README.MD`.

  A filter that selects no indexed file reports that, rather than reporting
  that the corpus does not match the query.

- **The `semlith_search` MCP tool takes the same filters**, as optional `path`,
  `ext` and `lang` arrays, which is the point of the release: an agent working
  on one subsystem can scope its question to that subsystem.

- **`semlith languages`** prints the language names `--lang` accepts and the
  extensions each covers. An unrecognised name is an error naming this command,
  not a silent empty result.

### Notes

No store format change. `files.path` has been recorded since 0.1.0, so
filtering works on an existing store with no re-indexing, and a store written
by 0.3.0 is readable by 0.2.0.

## [0.2.0] - 2026-07-29

Removes three of the four limitations 0.1.0 shipped with, cuts peak indexing
memory to a third, and changes the default embedding model.

### Changed

- **New default embedding model: `granite-embedding-small-english-r2`, int8.**
  Replaces `BGESmallENV15` for newly created stores. On a 6260-chunk benchmark
  of mixed Rust, Markdown and TypeScript it scored 16.00 code MRR@10 against
  BGE-small's 14.84, in a 52 MB download instead of 133 MB, at the same query
  latency. It is 384-dimensional, so the index geometry is unchanged.

  **Existing stores are not affected and are not migrated.** A store keeps the
  model it was built with, because vectors from two models are not comparable.
  To move an existing store onto the new default, delete the store directory
  and re-index.

- **Search is now hybrid.** Every query consults SQLite FTS5 for literal terms
  alongside the vector index, and the two rankings are fused by reciprocal rank.
  There is no flag: dense-only search cannot reliably find an exact identifier,
  which is one of the most common things asked of a code corpus. Existing
  stores have their keyword index built on first open, which costs no
  re-embedding.

  The `score` on a hit is now a fusion score rather than a cosine similarity.
  It orders results within one query and means nothing across queries.

- **ONNX Runtime thread count is chosen rather than defaulted.** On Apple
  silicon it is the performance-core count; elsewhere the full core count.
  ONNX Runtime synchronises threads per operator, so one thread on an
  efficiency core paces the whole batch — measured 16.5 chunks/sec at four
  threads against 13.9 at eight on a 4P+4E M1. Override with
  `SEMLITH_EMBED_THREADS`.

- **Minimum supported Rust version is now 1.89**, for `std::fs::File::try_lock`.

### Added

- **One writer per store.** An index run holds an OS advisory lock on the store
  for its duration. A second run exits non-zero naming the process that holds
  it, instead of interleaving writes until the vector index and the database
  disagree. The kernel releases the lock if the process dies, so an interrupted
  run leaves nothing to clean up and no stale file to delete by hand.

- `semlith models` lists the new default alongside fastembed's built-in models.

### Fixed

- A release whose git tag disagrees with `Cargo.toml` now fails the release
  build instead of publishing binaries labelled with a version they were not
  built from.

### Performance

Measured on a 4P+4E Apple Silicon laptop with 8 GB of RAM, over corpora of
mixed Rust, Markdown and TypeScript.

| store | warm query, p50 | indexing | peak RSS |
|---|---|---|---|
| 1.2k chunks | 2.7 ms | 27.6 chunks/sec | 600 MB |
| 9.9k chunks | 5.4 ms | 24.3 chunks/sec | 637 MB |
| 105k chunks | 22.7 ms | 23.5 chunks/sec | 595 MB |

- **Peak indexing memory is now roughly 600 MB and no longer grows with the
  corpus**, against ~1.7 GB in 0.1.0. Across a 1.2k, 9.9k and 105k chunk corpus
  it varies by 7 percent, and the largest corpus uses the least. Two causes:
  embedding batches were being flushed once per file rather than once per
  batch, so a single large file could hold thousands of chunks in memory; and
  the batch size itself was three times larger than it needed to be.
- **Indexing throughput is up from ~13 to ~23 chunks/sec**, from sizing the
  ONNX Runtime thread pool to the machine's performance cores.
- **Retrieval quality**, measured end to end through the binary over a
  6527-chunk corpus with 1809 self-labelled queries:

  | | code MRR@10 | docs MRR@10 |
  |---|---|---|
  | 0.1.0 (BGE-small, dense only) | 14.84 | 15.09 |
  | 0.2.0 (granite int8, dense only) | 16.00 | 14.08 |
  | **0.2.0 as shipped (granite int8 + FTS5)** | **17.90** | **15.62** |

  The model change alone would have regressed prose retrieval. Keyword fusion
  more than recovers it, which is why the two ship together.

### Library API

`semlith` is a published crate, so these are breaking changes to its Rust API.
The library API is unstable before 1.0 and changes with the minor version.

- `DEFAULT_MODEL: EmbeddingModel` is replaced by `default_model() -> Model`.
- `Semlith::open` takes `Option<Model>` rather than `Option<EmbeddingModel>`,
  and `Semlith::model` returns `&Model`. `Model` is an enum over fastembed's
  built-ins and the new default, which is not one of them.
- New modules: `embed` (model identity and loading) and `lock` (store locking).


## [0.1.0] - 2026-07-26

First release.

### Added

- **Local semantic search over files.** `semlith index` walks paths, extracts
  text, chunks it, embeds it locally, and stores the result. `semlith search`
  returns the best-matching excerpts with their file path and line range.
- **turbovec-backed vector index.** Vectors are quantized to 4 bits per
  coordinate and searched with SIMD. No training step, no rebuild as the corpus
  grows.
- **SQLite for everything else.** Chunk text, file paths, line spans and content
  hashes live in `store.db`; the vector index holds only vectors keyed by chunk
  id.
- **Incremental indexing.** Files are content-hashed with BLAKE3, so re-running
  `index` only re-embeds what changed and prunes what disappeared from disk. A
  file's hash is committed only after the vector index is durable, so an
  interrupted run re-indexes rather than leaving chunks that can never match.
- **MCP server.** `semlith mcp` speaks JSON-RPC over stdio and exposes
  `semlith_search` and `semlith_stats`, so any MCP-capable agent can query the
  store as a tool. The embedding model is loaded at startup so the first call is
  as fast as the rest.
- **PDF support.** PDFs are extracted to text automatically. Parser panics are
  caught so one malformed file cannot abort an indexing run.
- **Sensible skipping.** Honours `.gitignore` (including outside git repos),
  skips hidden files, binaries, and files over 8 MiB.
- **Commands** for inspecting and maintaining a store: `stats`, `files`,
  `forget`, `models`.
- **Model selection.** `--model` picks the embedding model when a store is
  created; the choice is then pinned, since vectors from two models are not
  comparable.

### Performance

Measured on an 8-core Apple Silicon laptop with 8 GB of RAM over 79 Rust source
files (1.5 MB, 2375 chunks):

- Warm query: ~6 ms
- Cold CLI start: ~250 ms, almost all of it loading the ONNX model
- Indexing: ~13 chunks/sec, ~1.7 GB peak RSS
- Re-index with nothing changed: 17 ms

[Unreleased]: https://github.com/semlith/semlith/compare/v0.29.0...HEAD
[0.29.0]: https://github.com/semlith/semlith/compare/v0.28.0...v0.29.0
[0.28.0]: https://github.com/semlith/semlith/compare/v0.27.0...v0.28.0
[0.27.0]: https://github.com/semlith/semlith/compare/v0.26.1...v0.27.0
[0.26.1]: https://github.com/semlith/semlith/compare/v0.26.0...v0.26.1
[0.26.0]: https://github.com/semlith/semlith/compare/v0.25.0...v0.26.0
[0.25.0]: https://github.com/semlith/semlith/compare/v0.24.0...v0.25.0
[0.24.0]: https://github.com/semlith/semlith/compare/v0.23.0...v0.24.0
[0.23.0]: https://github.com/semlith/semlith/compare/v0.22.0...v0.23.0
[0.22.0]: https://github.com/semlith/semlith/compare/v0.21.0...v0.22.0
[0.21.0]: https://github.com/semlith/semlith/compare/v0.20.2...v0.21.0
[0.20.2]: https://github.com/semlith/semlith/compare/v0.20.1...v0.20.2
[0.20.1]: https://github.com/semlith/semlith/compare/v0.20.0...v0.20.1
[0.20.0]: https://github.com/semlith/semlith/compare/v0.19.0...v0.20.0
[0.19.0]: https://github.com/semlith/semlith/compare/v0.18.0...v0.19.0
[0.18.0]: https://github.com/semlith/semlith/compare/v0.17.3...v0.18.0
[0.17.3]: https://github.com/semlith/semlith/compare/v0.17.2...v0.17.3
[0.17.2]: https://github.com/semlith/semlith/compare/v0.17.1...v0.17.2
[0.17.1]: https://github.com/semlith/semlith/compare/v0.17.0...v0.17.1
[0.17.0]: https://github.com/semlith/semlith/compare/v0.16.0...v0.17.0
[0.16.0]: https://github.com/semlith/semlith/compare/v0.15.0...v0.16.0
[0.15.0]: https://github.com/semlith/semlith/compare/v0.14.0...v0.15.0
[0.14.0]: https://github.com/semlith/semlith/compare/v0.13.0...v0.14.0
[0.13.0]: https://github.com/semlith/semlith/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/semlith/semlith/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/semlith/semlith/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/semlith/semlith/releases/tag/v0.10.0
[0.9.0]: https://github.com/semlith/semlith/releases/tag/v0.9.0
[0.8.0]: https://github.com/semlith/semlith/releases/tag/v0.8.0
[0.7.0]: https://github.com/semlith/semlith/releases/tag/v0.7.0
[0.6.0]: https://github.com/semlith/semlith/releases/tag/v0.6.0
[0.5.0]: https://github.com/semlith/semlith/releases/tag/v0.5.0
[0.4.0]: https://github.com/semlith/semlith/releases/tag/v0.4.0
[0.3.0]: https://github.com/semlith/semlith/releases/tag/v0.3.0
[0.2.0]: https://github.com/semlith/semlith/releases/tag/v0.2.0
[0.1.0]: https://github.com/semlith/semlith/releases/tag/v0.1.0
