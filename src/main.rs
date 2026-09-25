use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use fastembed::TextEmbedding;
use semlith::home;
use semlith::{Semlith, embed, embed::Model, filter::Filter, fleet::Fleet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// What an index run does about files the store holds from outside its roots.
///
/// A store is about its roots, and a row for anything else is a row whose
/// search results carry the wrong store label. Reconciling on write is the half
/// that stops it recurring; the daemon reconciles on open, which is the half
/// that fixes what is already there.
#[derive(Copy, Clone, PartialEq, Eq, Debug, ValueEnum)]
enum Reconcile {
    /// Drop them, and say how many. The default.
    Drop,
    /// Count them and say so, changing nothing. The dry run to look at first.
    Report,
    /// Leave them alone.
    Off,
}

/// A local semantic cache for AI agents.
#[derive(Parser)]
#[command(name = "semlith", version, about, long_about = None)]
struct Cli {
    /// Store directory (also settable with SEMLITH_STORE). Repeatable for
    /// `search`, `stats`, `files` and `mcp`, which read every store named;
    /// `index`, `watch` and `forget` write, and take exactly one.
    ///
    /// With no flag, semlith resolves a store itself: a `.semlith` beside the
    /// corpus if there is one, else the registered store covering this
    /// directory, else a new one under the store home.
    #[arg(long, short, global = true)]
    store: Vec<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Index files and directories. Re-running only re-embeds what changed.
    Index {
        /// Paths to index. Defaults to the current directory.
        paths: Vec<PathBuf>,

        /// Embedding model, used only when creating a new store.
        #[arg(long, short)]
        model: Option<String>,

        /// Name the store in the store home, instead of naming it after the
        /// directory being indexed.
        #[arg(long)]
        name: Option<String>,

        /// Suppress per-file output and download progress.
        #[arg(long, short)]
        quiet: bool,

        /// Refuse to download model weights. Exits non-zero naming the cache
        /// path if the model is not already there, so an air-gapped machine can
        /// prove this process never reached the network.
        #[arg(long)]
        airgap: bool,

        /// Index files semlith otherwise refuses: `.env`, private keys,
        /// anything under `~/.ssh` and the rest of the deny-list. Off by
        /// default, because indexing a private key by accident is a mistake and
        /// this flag is how you say you meant it. It has no effect on the MCP
        /// tools or the portal, which are held to the boundary either way.
        #[arg(long)]
        include_secrets: bool,

        /// One store per path, each named and placed exactly as `semlith index
        /// <path>` would name and place it. Without this, several paths go
        /// into one store, which is what they have always done.
        ///
        /// Sequential: one process, one embedder, one store at a time. A user
        /// who wants them in parallel runs the daemon, which is what it is for.
        #[arg(long)]
        each: bool,

        /// Take the paths from the git repositories directly under this folder,
        /// or from its plain subfolders where none of them is a repository.
        /// One level only. Implies `--each`.
        #[arg(long, value_name = "FOLDER")]
        projects: Option<PathBuf>,

        /// What to do about files the store already holds from outside its
        /// registered roots. `report` counts them and changes nothing, which
        /// is the dry run worth doing first on a store you did not build today.
        #[arg(long, value_name = "MODE", default_value = "drop")]
        reconcile: Reconcile,

        /// Scan and print the plan — what would be embedded, what is
        /// unchanged, what is not indexed and why — without embedding.
        #[arg(long)]
        scan_only: bool,

        /// Never stop to ask about a file the scan refused: clean files are
        /// indexed and refused ones are listed for `semlith refused`. A run
        /// with no terminal never asks either.
        #[arg(long)]
        no_review: bool,
    },

    /// Run the daemon: hold every registered store's write lock, keep them
    /// current as files are saved, and serve the portal on 127.0.0.1. Runs
    /// until interrupted.
    Start {
        /// Install the daemon as a login service and start it, instead of
        /// running in this terminal.
        ///
        /// A launchd user agent on macOS, a systemd user unit on Linux, a logon
        /// task on Windows. It runs from the next login onwards, and on macOS
        /// and Linux a daemon that exits is restarted. Safe to run twice.
        #[arg(long, conflicts_with = "no_service")]
        service: bool,

        /// Remove the login service. Leaves a running daemon and every store
        /// exactly as they are, and is not an error when none is installed.
        #[arg(long)]
        no_service: bool,

        /// Stores to open. Defaults to every registered store.
        paths: Vec<PathBuf>,

        /// Do not record what agents retrieve into each store's ledger.
        ///
        /// Recording is on from 0.15.0. The rows never leave the machine, the
        /// daemon says on every start that it is recording and how to stop,
        /// and deleting every row is one statement. A record an agent cannot
        /// see being written is what makes the savings figure and the audit
        /// trail true rather than optional — and until 0.15.0 the ledger was
        /// opt-in, which in practice meant empty.
        ///
        /// `SEMLITH_LEDGER=0` is the same thing for a machine that should
        /// never record at all. `--ledger` was removed rather than kept as a
        /// flag that does nothing, so a script that passes it fails here and
        /// is corrected once.
        #[arg(long)]
        no_ledger: bool,

        /// Port to listen on (also settable with SEMLITH_PORT). Never falls
        /// back to another port: the URL is meant to be a bookmark.
        #[arg(long)]
        port: Option<u16>,

        /// Quiet period in milliseconds after the last change before
        /// re-embedding, so one editor save costs one re-embed.
        #[arg(long, default_value_t = semlith::watch::DEBOUNCE.as_millis() as u64)]
        debounce: u64,

        /// Refuse to download model weights.
        #[arg(long)]
        airgap: bool,

        /// Do not answer MCP over HTTP. The portal and `semlith mcp` over
        /// stdio are unaffected; only the `/mcp` endpoint is closed.
        #[arg(long)]
        no_mcp_http: bool,
    },

    /// Show or rotate the agent key that authenticates the HTTP MCP endpoint.
    Key {
        #[command(subcommand)]
        what: KeyCommand,
    },

    /// Move an existing store directory into the store home and register it,
    /// so `semlith mcp` and `semlith start` find it with no flags. Nothing is
    /// re-embedded and the store's format does not change.
    Adopt {
        /// The store directory to move — usually `./.semlith`.
        store_dir: PathBuf,

        /// The directory this store indexes. Defaults to the directory the
        /// store sat in. Given on its own with `--name`, re-points a
        /// registered store whose corpus moved.
        #[arg(long)]
        root: Option<PathBuf>,

        /// Name it in the home explicitly.
        #[arg(long)]
        name: Option<String>,
    },

    /// Say that a store directory outside the store home may be opened.
    ///
    /// A `.semlith` directory can arrive inside a repository somebody else
    /// wrote, and a store is what semlith answers from — so one that semlith
    /// did not create is opened only after you have said so, once. Nothing is
    /// moved and nothing is re-embedded; `semlith adopt` is the command that
    /// moves a store into the home.
    Trust {
        /// The store directory to trust — usually `./.semlith`.
        store_dir: Option<PathBuf>,

        /// Print what is trusted and change nothing.
        #[arg(long)]
        list: bool,
    },

    /// Keep the store current: re-embed files as they are saved. Runs until
    /// interrupted, and holds the store's write lock while it does.
    Watch {
        /// Paths to watch. Defaults to the current directory.
        paths: Vec<PathBuf>,

        /// Quiet period in milliseconds after the last change before
        /// re-embedding, so one editor save costs one re-embed.
        #[arg(long, default_value_t = semlith::watch::DEBOUNCE.as_millis() as u64)]
        debounce: u64,

        /// Suppress per-file output and download progress.
        #[arg(long, short)]
        quiet: bool,
    },

    /// Search the store.
    Search {
        query: String,

        /// Number of results.
        #[arg(long, short, default_value_t = 8)]
        k: usize,

        /// Only search files matching this glob. Repeatable; a relative
        /// pattern matches anywhere in the tree. A leading `!` excludes:
        /// `--path 'src/**' --path '!src/vendor/**'`.
        #[arg(long, short)]
        path: Vec<String>,

        /// Only search files with this extension. Repeatable; a leading `!` excludes.
        #[arg(long, short)]
        ext: Vec<String>,

        /// Only search files of this language. Repeatable; a leading `!`
        /// excludes. See `semlith languages`.
        #[arg(long, short)]
        lang: Vec<String>,

        /// Lift the implementation (`code`), the prose about it (`docs`), or
        /// neither (`any`, the default). A bias, not a filter.
        #[arg(long, default_value = "any")]
        prefer: String,

        /// Every indexed line matching the query as a regular expression
        /// (`grep -E`; literal text when it is not one), each with the
        /// definition it sits in.
        #[arg(long)]
        exact: bool,

        /// With --exact: skip this many matching lines, to page past the cap.
        #[arg(long, default_value_t = 0, requires = "exact")]
        offset: usize,

        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },

    /// Everything one question needs, in one call, under a token budget.
    ///
    /// The spans a search would find, the text of the top ones, and the
    /// callers and callees of the symbols they sit inside, one hop each way --
    /// what `search`, `read` and `neighbors` answer in four round trips.
    ///
    /// `semlith brief "how does a store decide it is stale"`
    Brief {
        question: String,

        /// The token ceiling for the whole answer, counted with the store's
        /// own tokenizer. Locators and edges are kept first, so a small budget
        /// drops span text from the bottom of the ranking up and says so.
        #[arg(long, short, default_value_t = semlith::brief::DEFAULT_BUDGET)]
        budget: i64,

        /// Only look at files matching this glob. Repeatable; a leading `!` excludes.
        #[arg(long, short)]
        path: Vec<String>,

        /// Only look at files with this extension. Repeatable; a leading `!` excludes.
        #[arg(long, short)]
        ext: Vec<String>,

        /// Only look at files of this language. Repeatable; a leading `!` excludes.
        #[arg(long, short)]
        lang: Vec<String>,

        /// Lift the implementation (`code`), the prose about it (`docs`), or
        /// neither (`any`, the default).
        #[arg(long, default_value = "any")]
        prefer: String,

        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },

    /// Print one span, or one symbol's definition, and nothing around it.
    ///
    /// The second stage after a search: `semlith read src/store.rs:1041-1080`
    /// or `semlith read record_retrieval`.
    Read {
        /// `path:start-end`, `path:line`, or a symbol name.
        target: String,

        /// Only read files matching this glob. Repeatable; a leading `!` excludes.
        #[arg(long, short)]
        path: Vec<String>,

        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },

    /// Run a tree-sitter structural pattern over the indexed files of one
    /// language.
    ///
    /// `semlith pattern --lang rust '(call_expression function: (identifier) @f)'`
    Pattern {
        /// The pattern, in tree-sitter's S-expression query syntax.
        query: String,

        /// The language to parse. Required; see `semlith languages`.
        #[arg(long, short)]
        lang: String,

        /// Only search files matching this glob. Repeatable; a leading `!` excludes.
        #[arg(long, short)]
        path: Vec<String>,

        /// Skip this many matches, to continue a listing the cap cut short.
        #[arg(long, default_value_t = 0)]
        offset: usize,

        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },

    /// Show what the store contains.
    Stats,

    /// List the files currently indexed.
    Files {
        /// Directories with their file and chunk counts and languages, each
        /// file with its lines, symbols and first definitions, and what is on
        /// disk but not indexed, and why.
        #[arg(long)]
        tree: bool,

        /// How many directory levels the tree shows.
        #[arg(long, default_value_t = 2)]
        depth: usize,

        /// name, size, symbols, or recent (newest indexed first).
        #[arg(long, default_value = "name")]
        sort: String,

        /// Only paths matching these globs, as `semlith search --path` takes.
        #[arg(long)]
        path: Vec<String>,
    },

    /// Remove a file from the store.
    Forget { path: PathBuf },

    /// Every file that was not indexed, and why; accept or revoke one.
    ///
    /// Five classes: a secret-shaped value (reviewable), a credential file
    /// (never acceptable), a policy limit such as the size cap (reviewable),
    /// a file no reader can take, and your own ignore rules. A secret row
    /// carries a 0-100 % estimate that it is real, with the signals behind
    /// it, and never the value.
    Refused {
        #[command(subcommand)]
        action: Option<RefusedCommand>,

        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },

    /// List every file the store holds that semlith would refuse today.
    ///
    /// The path for a store indexed before a rule widened, or before the
    /// credential content scan existed. Exits non-zero while anything is
    /// found, so it can be a check in a script.
    Scan {
        /// The registered store's name, as `semlith stats` prints it. Named
        /// `name` for the same reason `drop` is: `--store` is a global
        /// argument and two arguments with one id is a parse-time panic.
        #[arg(value_name = "STORE")]
        name: Option<String>,

        /// Remove what was found, the way `semlith forget` removes a file.
        #[arg(long)]
        forget: bool,

        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },

    /// Delete a store: its vectors, chunks, graph and ledger, and the registry
    /// entry naming it. The files it indexed are not touched.
    Drop {
        /// The registered store's name, as `semlith stats` prints it.
        ///
        /// Called `name` rather than `store` because `--store` is a global
        /// argument: two arguments with the same id in one subcommand is a
        /// clap panic at parse time, so every `semlith drop` panicked instead
        /// of deleting anything. The value name keeps the usage line reading
        /// `semlith drop <STORE>`.
        #[arg(value_name = "STORE")]
        name: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },

    /// Fetch one URL into the store and index it: a web page, a PDF such as an
    /// arXiv paper, or a file on GitHub. One request, for exactly the URL
    /// given — semlith never follows links, re-fetches, or sends a credential.
    Add {
        /// The https URL to fetch. A `github.com/.../blob/...` link is
        /// rewritten to the raw file it displays.
        url: String,

        /// Name the store in the store home, instead of naming it after the
        /// current directory.
        #[arg(long)]
        name: Option<String>,

        /// Refuse to reach the network, which is the whole of this command, so
        /// it exits before opening a socket.
        #[arg(long)]
        airgap: bool,
    },

    /// Run as an MCP server over stdio, for agents to call as a tool.
    Mcp,

    /// Answer one `PreToolUse` event, for an agent client to call before it
    /// reads a file or greps the tree.
    ///
    /// Reads the event as JSON on stdin and writes the client's answer on
    /// stdout. When a registered store holds the file, that answer is one line
    /// naming the semlith call which would have answered the same question; for
    /// anything else it is nothing at all. It never blocks and never fails: a
    /// hook that errors inside a client's tool call is a broken client.
    ///
    /// `semlith setup` writes this into the clients that support it, so it is
    /// rarely typed by hand.
    Hook {
        /// soft (the default) only adds a line; gate refuses raw lookups until
        /// the session has made one semlith call, twice at most; hard always
        /// refuses grep, rg and find in an indexed folder.
        #[arg(long, default_value = "soft")]
        mode: String,

        /// The same as `--mode gate`, kept for hooks written before 0.30.0.
        #[arg(long)]
        strict: bool,

        /// The client's own name for itself, as the ledger should record it.
        #[arg(long, default_value = "claude-code")]
        client: String,
    },

    /// Put semlith on PATH, pre-fetch the embedding model and register it
    /// with the agents you use. Every step is idempotent, so this is also the
    /// repair command.
    Setup {
        /// Do not install the daemon as a login service.
        ///
        /// It is installed by default, including when nothing can answer a
        /// prompt — a piped installer, CI, `--yes`. A user who never reads the
        /// prompt is exactly the user a login service is for, and the cost of
        /// opting out is this flag.
        #[arg(long)]
        no_service: bool,

        /// Take the default at every prompt — add to PATH, download the model,
        /// register no agent — so a script or an agent can run it unattended.
        #[arg(long, short)]
        yes: bool,

        /// Refuse to download model weights. The other steps still run.
        #[arg(long)]
        airgap: bool,

        /// Also write the configuration file of every client that has no
        /// registration command of its own, and the rules file of every client
        /// that documents one. Every path is listed before anything is written,
        /// each file is backed up beside itself, and a file that does not parse
        /// is left alone.
        #[arg(long)]
        register_all: bool,

        /// Do not write the `PreToolUse` steering hook, and remove it if it is
        /// already there.
        ///
        /// It is written by default: a hook nobody installs steers nobody. The
        /// file it edits is backed up beside itself first, and removing the
        /// hook leaves every other hook in that file exactly as it was.
        #[arg(long)]
        no_hooks: bool,

        /// How the hook steers: soft (the default) only adds a line; gate
        /// refuses raw lookups until the session has made one semlith call,
        /// at most twice; hard always refuses grep, rg and find in an indexed
        /// folder.
        #[arg(long, default_value = "soft")]
        hook_mode: String,

        /// The same as `--hook-mode gate`.
        #[arg(long)]
        strict: bool,

        /// Do not write the semlith-explorer research agent, and remove it if
        /// it is already there.
        #[arg(long)]
        no_agents: bool,
    },

    /// Report whether each agent client on this machine can reach semlith, and
    /// what to run for the ones that cannot.
    ///
    /// Reads files and runs nothing, so it is safe on a machine that is
    /// misbehaving. `--fix` applies the repairs that narrow access to something
    /// semlith owns, and nothing else.
    Doctor {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,

        /// Apply the repairs that qualify as safe: idempotent, narrowing, and
        /// confined to a path semlith owns. Each names what it changed and what
        /// it was before.
        #[arg(long)]
        fix: bool,

        /// One line and an exit code, for a shell prompt or an agent's
        /// session-start hook.
        ///
        /// Reads and never writes: the full `doctor` repairs a registration
        /// that cannot launch, and a hook that runs on every session opened is
        /// not a place to edit configuration files from.
        #[arg(long, conflicts_with_all = ["json", "fix"])]
        brief: bool,

        /// Embed 32 fixed chunks on every accelerator lane this machine has
        /// and compare them with CPU fp32 vectors committed to the
        /// repository: the cosine, the rate and the device, per lane. How an
        /// owner of a GPU checks it gives the right answers.
        #[arg(long, conflicts_with_all = ["fix", "brief"])]
        gpu: bool,
    },

    /// Which devices embed: the CPU, a GPU through WebGPU, and NVIDIA's CUDA.
    ///
    /// `status` names each lane, its device and whether it is on. `on` and
    /// `off` take effect at the next batch of every run. `remove` deletes a
    /// lane's downloaded components; turning a lane off never does.
    Accel {
        /// status, on, off or remove.
        #[arg(default_value = "status")]
        action: String,
        /// cpu, gpu or cuda.
        lane: Option<String>,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },

    /// An accelerator lane's worker: embeds batches on stdin for the daemon.
    /// Started by the daemon and nothing else; not part of the interface.
    #[command(name = "__embed-worker", hide = true)]
    EmbedWorker { lane: String, dir: Option<PathBuf> },

    /// Replace this binary with the newest release for this machine. Runs only
    /// when asked: semlith never checks for an update on its own.
    Upgrade {
        /// Say whether a newer release exists and change nothing. Exits 0 when
        /// current and 10 when an upgrade is available.
        #[arg(long)]
        check: bool,

        /// Install this tag instead of the newest release, e.g. `v0.9.0`.
        #[arg(long)]
        version: Option<String>,

        /// Refuse to reach the network. Every path below exits before opening
        /// a connection, which is what an air-gapped machine needs to prove.
        #[arg(long)]
        airgap: bool,
    },

    /// List available embedding models.
    Models,

    /// List the language names `--lang` accepts, and their extensions.
    Languages,

    /// Print what agents retrieved from this store, newest first. Needs no
    /// key: recording and this dump are free on every tier.
    Ledger {
        /// How many retrievals to print.
        #[arg(long, default_value_t = 20)]
        last: usize,

        /// Re-walk the hash chain and exit non-zero if it is broken.
        ///
        /// Prints nothing else. For a cron job or a CI step that wants to know
        /// the record has not been edited, rather than a person reading rows.
        #[arg(long)]
        verify: bool,

        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },

    /// Find where a symbol is defined.
    Symbol {
        /// The symbol's name, matched exactly. `Type::method` narrows to one
        /// owner. Several names print one row per definition and no rings.
        #[arg(required = true, num_args = 1..)]
        names: Vec<String>,

        /// Most definitions to print.
        #[arg(long, short, default_value_t = 20)]
        k: usize,

        /// What this name used to be: the definitions a re-index replaced,
        /// each with the content hash of the file version it was true for.
        ///
        /// Empty on a store that has not re-indexed since 0.23.0, which is
        /// what `semlith stats` says.
        #[arg(long)]
        history: bool,

        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },

    /// List what calls a symbol and what it calls.
    Neighbors {
        /// The symbol's name, matched exactly.
        name: String,

        /// Only follow edges of this kind. Repeatable; one of defines, calls,
        /// imports, references, contains. Every kind by default.
        #[arg(long, short)]
        kind: Vec<String>,

        /// Show every definition behind a collapsed row, and the targets this
        /// store holds no definition for.
        ///
        /// Off by default. A name with four definitions is one row saying so,
        /// because four rows would read as four calls; and a call into a
        /// dependency that was never indexed is left out, because a list of
        /// names the store knows nothing about is noise in an answer about
        /// this codebase.
        #[arg(long)]
        all: bool,

        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },

    /// List everything that reaches a symbol, and the files it lives in.
    ///
    /// The graph read backwards: who would notice if this changed. Breadth
    /// first, so the hop against a caller is the fewest hops it takes.
    Impact {
        /// The symbol's name, matched exactly.
        name: String,

        /// Only follow edges of this kind. Repeatable; one of defines, calls,
        /// imports, references, contains. Every kind by default.
        #[arg(long, short)]
        kind: Vec<String>,

        /// Most hops to walk back before stopping.
        #[arg(long, short, default_value_t = 3)]
        depth: u32,

        /// Walk names with several definitions too, and label what that found.
        ///
        /// Off by default, for the reason `semlith path` refuses them: a name
        /// like `record` or `index` can be several unrelated functions, and
        /// crossing one puts somebody else's callers in this answer.
        #[arg(long)]
        all_edges: bool,

        /// Refuse to cross a name with several definitions. On by default.
        ///
        /// Here so a script can state the default rather than rely on it.
        /// When both this and --all-edges are given, this one wins.
        #[arg(long)]
        strict: bool,

        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },

    /// Turn the chain between two symbols into evidence.
    ///
    /// The answer sentence, the chain, and one supporting line of source per
    /// hop, each marked a supporting fact or a candidate to corroborate.
    Trace {
        /// The symbol the chain starts at.
        from: String,

        /// The symbol the chain ends at.
        to: String,

        /// Most hops to search before giving up.
        #[arg(long, short, default_value_t = 6)]
        depth: u32,

        /// Walk names with several definitions too, and label what that found.
        #[arg(long)]
        all_edges: bool,

        /// Refuse to cross a name with several definitions. On by default.
        #[arg(long)]
        strict: bool,

        /// Print the plain-text block the portal's "Copy as evidence" copies.
        #[arg(long)]
        evidence: bool,

        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },

    /// Generate one of the five reports from this machine's own data.
    ///
    /// Nothing reaches the network and nothing is a model's opinion: the
    /// ledger, the index and the graph are the only sources.
    Report {
        /// Which report: savings, access, change, health or gaps.
        kind: String,

        /// markdown, csv, json, html or pdf.
        ///
        /// A PDF is bytes rather than text, so it needs `--out`: writing it to
        /// a terminal would be a screenful of binary and a file nobody has.
        #[arg(long, short, default_value = "markdown")]
        format: String,

        /// Which model's prices the savings report costs tokens at.
        #[arg(long, default_value = "Sonnet 5")]
        model: String,

        /// Narrow the report to a period: all, day, week, month or quarter.
        ///
        /// Three of the five reports are lifetime aggregates with no date to
        /// narrow by. They say so under their title rather than printing a
        /// span they did not apply.
        #[arg(long)]
        window: Option<String>,

        /// Narrow the report to these stores, by label. Repeatable.
        ///
        /// Every open store when none is named, which is what this command has
        /// always done.
        #[arg(long)]
        scope: Vec<String>,

        /// Attach the individual retrievals, not only the session totals.
        ///
        /// A session line says an agent asked forty times; this is which forty.
        /// Access report only.
        #[arg(long)]
        excerpts: bool,

        /// Replace every query text with a digest of it.
        ///
        /// Keeps the who, the when and the which-file, and drops what was
        /// asked. Applies wherever a query reaches the page, not only to the
        /// attachment above.
        #[arg(long)]
        redact: bool,

        /// Write to this file instead of standard output.
        #[arg(long, short)]
        out: Option<std::path::PathBuf>,
    },

    /// Add, list and remove the reports the daemon writes on a cadence.
    ///
    /// A schedule belongs to the daemon, not to this command: `semlith
    /// schedule add` writes the record and a running `semlith start` picks it
    /// up and does the work. Nothing is generated here.
    Schedule {
        #[command(subcommand)]
        what: ScheduleCommand,
    },

    /// Show the shortest chain of edges between two symbols.
    Path {
        /// The symbol the chain starts at.
        from: String,

        /// The symbol the chain ends at.
        to: String,

        /// Most hops to search before giving up.
        #[arg(long, short, default_value_t = 6)]
        depth: u32,

        /// Walk names with several definitions too, and label what that found.
        ///
        /// Off by default. A name like `record` or `index` can be several
        /// unrelated functions, and a chain that crosses one has changed
        /// subject halfway through without saying so. This prints those chains
        /// with the join marked and the sentence that says what they are worth.
        #[arg(long)]
        all_edges: bool,

        /// Refuse to cross a name with several definitions. On by default.
        ///
        /// Here so a script can state the default rather than rely on it. When
        /// both this and --all-edges are given, this one wins.
        #[arg(long)]
        strict: bool,

        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RefusedCommand {
    /// Accept one refused file, after reading what was found in it.
    ///
    /// One path per call, never a list or a glob. Prints each masked match,
    /// its line and confidence, and asks for the file's name to be typed.
    Accept {
        path: PathBuf,
        /// Replace each detected value with `[REDACTED:…]` before anything is
        /// stored. Covers only what the scanner detected.
        #[arg(long, conflicts_with = "as_is", required_unless_present = "as_is")]
        redact: bool,
        /// Index the file's full text, values included.
        #[arg(long)]
        as_is: bool,
        /// Skip the typed confirmation, for a script. Still one path.
        #[arg(long)]
        yes: bool,
    },
    /// Undo an acceptance: the file leaves the store and is listed again.
    Revoke { path: PathBuf },
    /// Keep out a file that was let through because every match in it is a
    /// declared test dummy.
    Refuse {
        path: PathBuf,
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum KeyCommand {
    /// Print the agent key and the stanza that carries it.
    Show {
        /// Print the key alone, for a script.
        #[arg(long)]
        quiet: bool,
    },

    /// Mint a new agent key. Every client configured with the old one needs
    /// the new stanza.
    Rotate {
        /// Stop accepting the previous key at once, rather than letting a
        /// session that is already open finish.
        #[arg(long)]
        now: bool,
    },
}

/// Windows gives a process's main thread 1 MiB of stack, and `run` below is one
/// `match` over twenty-six subcommands whose frame holds every arm's locals at
/// once. 0.23.0 added an arm and the binary started dying on `semlith brief`
/// before it could print anything -- a test saw an empty stderr and an exit
/// status that was not success, which is exactly what a refusal looks like, so
/// the only reason this was caught rather than shipped is that the test
/// asserted on the *words* of the refusal and not just on the exit code.
///
/// Moving one arm into its own function bought a little room and did not fix
/// it, because the frame is the sum of all of them. So the work runs on a
/// thread with a stack that is not the platform's default, and the next arm
/// added does not have to think about it. Unix is unaffected: 8 MiB there
/// already, and this asks for 16.
fn main() -> Result<()> {
    quiet_on_a_closed_pipe();
    let worker = std::thread::Builder::new()
        .name("semlith".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(run)?;
    match worker.join() {
        Ok(result) => result,
        // The thread panicked and has already printed its own message. Exiting
        // with the status a panicking process uses keeps the shell's view of it
        // the same as before this indirection existed.
        Err(_) => std::process::exit(101),
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    match cli.command {
        Command::Doctor {
            json, gpu: true, ..
        } => {
            let checks = semlith::accel::check_all(|line| eprintln!("{line}"));
            if json {
                println!("{}", serde_json::to_string_pretty(&checks)?);
            } else {
                print_gpu_checks(&checks);
            }
            if checks
                .iter()
                .any(|c| c.get("passed") == Some(&serde_json::json!(false)))
            {
                std::process::exit(1);
            }
        }

        Command::Doctor {
            json, fix, brief, ..
        } => {
            let stores: Vec<(String, std::path::PathBuf)> = home::Registry::load()
                .unwrap_or_default()
                .stores
                .into_keys()
                .filter_map(|name| home::Registry::dir_of(&name).ok().map(|dir| (name, dir)))
                .collect();
            let applied = if fix {
                semlith::doctor::apply_all(&stores)
            } else {
                Vec::new()
            };
            // The one repair that edits a client's file, and only for the
            // directory doctor runs in: the disable was the user's choice there.
            if fix && let Some((dir, key)) = semlith::doctor::clear_disable_here(&cwd)? {
                eprintln!(
                    "cleared: semlith was switched off for {dir} under {key} in ~/.claude.json; \
                     a backup is beside it"
                );
            }
            // `--brief` takes the read-only report. It is the one flag that
            // may run on every session a person opens, and a check that
            // rewrites a configuration file as a side effect of being asked
            // "is this working?" is not a check.
            let report = if brief {
                semlith::doctor::clients_report_read_only()
            } else {
                semlith::doctor::clients_report()
            };
            let rules = semlith::doctor::privacy_findings(&stores);
            // Not for `--brief`, which may run on every session a person
            // opens, and not from the portal, which reads `clients_report` on
            // every page load. This one starts a server and asks a client.
            let proof = (!brief).then(semlith::doctor::proof);

            if brief {
                print_brief(&report, &rules);
                let faults = report.iter().filter(|c| c.fault).count()
                    + rules.iter().filter(|r| !r.ok).count();
                if faults > 0 {
                    std::process::exit(1);
                }
                return Ok(());
            }

            if json {
                let out = serde_json::json!({
                    "clients": report,
                    "rules": rules,
                    "service": semlith::service::status(),
                    "proof": proof,
                    "applied": applied
                        .iter()
                        .map(|r| match r {
                            Ok(a) => serde_json::json!(a),
                            Err(e) => serde_json::json!({ "error": e.to_string() }),
                        })
                        .collect::<Vec<_>>(),
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                print_doctor(&report, &rules, &applied);
                if let Some(proof) = &proof {
                    print_proof(proof);
                }
            }

            // Non-zero when something on this machine is not as it should be,
            // so a script can gate on it. A client that is simply not installed
            // is not a fault: most people have two or three of twenty-seven.
            let faults = report.iter().filter(|c| c.fault).count()
                + rules.iter().filter(|r| !r.ok).count()
                + usize::from(proof.as_ref().is_some_and(|p| p.failed.is_some()));
            if faults > 0 {
                std::process::exit(1);
            }
        }

        Command::Setup {
            yes,
            airgap,
            register_all,
            no_service,
            no_hooks,
            hook_mode,
            strict,
            no_agents,
        } => {
            arm_airgap(airgap);
            // The flag or the variable. The installers translate their own
            // `SEMLITH_NO_SERVICE` into the flag, but `semlith setup` is also
            // run directly — by a provisioning script, by CI, by the native
            // smoke harness — and a knob that only works through one of two
            // entry points is a knob somebody will set and watch do nothing.
            let no_service =
                no_service || std::env::var(semlith::setup::NO_SERVICE_ENV).is_ok_and(|v| v == "1");
            let mode = if strict {
                semlith::hook::Mode::Gate
            } else {
                semlith::hook::Mode::parse(&hook_mode)?
            };
            semlith::setup::run(
                yes,
                airgap,
                register_all,
                !no_service,
                !no_hooks,
                mode,
                !no_agents,
            )?;
        }

        Command::Accel { action, lane, json } => match (action.as_str(), lane.as_deref()) {
            ("status", _) => {
                let status = semlith::accel::snapshot();
                if json {
                    println!("{}", serde_json::to_string_pretty(&status)?);
                } else {
                    for row in status["lanes"].as_array().into_iter().flatten() {
                        let state = &row["status"];
                        let detail = state["reason"]
                            .as_str()
                            .map(|r| format!("{} — {r}", state["state"].as_str().unwrap_or("")))
                            .unwrap_or_else(|| state["state"].as_str().unwrap_or("").to_string());
                        println!(
                            "{:<5} {:<4} {:<28} {detail}",
                            row["lane"].as_str().unwrap_or("?"),
                            if row["enabled"].as_bool() == Some(true) {
                                "on"
                            } else {
                                "off"
                            },
                            row["device"].as_str().unwrap_or("not asked for yet"),
                        );
                    }
                    println!("switches: {}", status["source"].as_str().unwrap_or(""));
                }
            }
            ("on" | "off", Some(lane)) => {
                println!("{}", semlith::accel::set(lane, action == "on")?)
            }
            ("remove", Some(lane)) => {
                let freed = semlith::accel::remove(lane)?;
                println!(
                    "{lane}: components removed, {} freed",
                    semlith::human_bytes(freed as i64)
                );
            }
            _ => anyhow::bail!(
                "usage: semlith accel [status | on <lane> | off <lane> | remove <lane>], lanes cpu, gpu, cuda"
            ),
        },

        Command::EmbedWorker { lane, dir } => {
            std::process::exit(semlith::accel::worker_main(&lane, dir.as_deref()));
        }

        Command::Upgrade {
            check,
            version,
            airgap,
        } => {
            arm_airgap(airgap);
            if check {
                if let Some(reason) = semlith::upgrade::offline() {
                    bail!("{reason}");
                }
                let found = semlith::upgrade::check()?;
                println!("installed {}", found.installed);
                println!("latest    {}", found.latest.trim_start_matches('v'));
                if let Some(reason) = &found.blocked {
                    println!("note: {reason}");
                }
                if found.available {
                    println!("run `semlith upgrade` to install it");
                    std::process::exit(semlith::upgrade::UPGRADE_AVAILABLE);
                }
                println!("already current");
            } else {
                semlith::upgrade::apply(version)?;
            }
        }

        Command::Models => {
            // The default is listed first and separately: it is not one of
            // fastembed's built-ins, so it never appears in their list.
            println!(
                "{:<44} {:>5} dim  default. IBM Granite R2 small, int8, English",
                embed::GRANITE_NAME,
                384
            );
            for info in TextEmbedding::list_supported_models() {
                println!(
                    "{:<44} {:>5} dim  {}",
                    info.model, info.dim, info.description
                );
            }
        }

        Command::Index {
            paths,
            model,
            name,
            quiet,
            airgap,
            include_secrets,
            each,
            projects,
            reconcile,
            scan_only,
            no_review,
        } => {
            arm_airgap(airgap);

            // `--projects` turns a folder into the paths under it, and means
            // `--each`: asking which repositories are under a folder and then
            // merging them into one store answers a different question.
            let (roots, each) = match &projects {
                Some(folder) => {
                    let (found, repositories) = home::projects_under(folder)?;
                    if found.is_empty() {
                        bail!(
                            "nothing under {} to index: no repository and no subfolder",
                            folder.display()
                        );
                    }
                    eprintln!(
                        "{} {} under {}",
                        found.len(),
                        if repositories {
                            "repositories"
                        } else {
                            "folders"
                        },
                        folder.display()
                    );
                    (found, true)
                }
                None if paths.is_empty() => (vec![PathBuf::from(".")], each),
                None => (paths, each),
            };

            // Before a store is opened, because opening one creates it. A run
            // whose paths cannot be read used to leave an empty store behind,
            // registered under the typo's own name, and exit 0 (#76).
            let (roots, unreadable) = semlith::check_roots(&roots);
            for (path, why) in &unreadable {
                eprintln!("cannot index {}: {why}", path.display());
            }
            if roots.is_empty() {
                bail!(
                    "nothing to index: no path given could be read. Nothing was created \
                     and nothing was registered."
                );
            }
            // A name is a name for one store. With `--each` every store is
            // named after its own path, so one `--name` for all of them is an
            // instruction that cannot be carried out.
            if each && name.is_some() {
                bail!(
                    "--each names each store after its own path, so --name cannot apply to \
                     all of them. Run semlith index --name <name> <path> per folder instead."
                );
            }

            // One run over every path, as always — or one run per path, which
            // is what `--each` means. Sequential either way: one process, one
            // embedder, one store at a time.
            let runs: Vec<Vec<PathBuf>> = if each {
                roots.iter().cloned().map(|path| vec![path]).collect()
            } else {
                vec![roots.clone()]
            };

            for roots in &runs {
                let first = roots[0].clone();

                // The first path is what the store is about, so it is what names
                // the store and what the registry records as its root. `semlith
                // index ~/work/api` from anywhere means the api store, not a store
                // named after wherever the shell happened to be.
                let choice = home::resolve(&cli.store, &first, name.as_deref())?;
                if let Some(hint) = choice.hint() {
                    eprintln!("{hint}");
                }
                let dir = choice.one()?;
                // Parsed per run rather than once: it names the model a *new*
                // store is built with, and `--each` may create several.
                let model = model
                    .clone()
                    .map(|m| m.parse::<Model>().map_err(anyhow::Error::msg))
                    .transpose()?;
                let mut store = Semlith::open(&dir, model)?;
                store.quiet = quiet;
                // No confinement on the command line: the person typing it owns the
                // machine. The deny-list still applies, because indexing a private
                // key by accident is a mistake rather than a decision.
                store.boundary = semlith::Boundary {
                    roots: None,
                    allow_secrets: include_secrets,
                };

                // The scan phase (2.7): the whole plan before the model is
                // even loaded, and a stop for review only when a person is at
                // the terminal and something is theirs to decide.
                let plan = store.plan(roots)?;
                if !quiet || scan_only {
                    print_plan(&plan);
                }
                if scan_only {
                    continue;
                }
                use std::io::IsTerminal;
                let interactive =
                    !no_review && std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
                if interactive && !plan.review.is_empty() {
                    review_plan(&mut store, &plan)?;
                }

                let started = Instant::now();
                // Throttled, not per file: a corpus large enough to need an
                // estimate is one where a line per file is the noise the estimate
                // is trying to cut through.
                let mut spoke = Instant::now();
                let report = store.index_paths(roots, |path, p| {
                    if quiet {
                        return;
                    }
                    if p.outcome == semlith::FileOutcome::Indexing {
                        eprintln!("  + {}", display(path));
                    }
                    if p.outcome == semlith::FileOutcome::Refused {
                        eprintln!("  - {}", display(path));
                    }
                    // A failure is louder than a refusal because it is not a
                    // decision: something went wrong with this one file and the
                    // run carried on, which is exactly the case a silent line
                    // would hide.
                    if p.outcome == semlith::FileOutcome::Failed {
                        eprintln!(
                            "  ! {} — {}",
                            display(path),
                            p.why.as_deref().unwrap_or("failed")
                        );
                    }
                    if spoke.elapsed() >= PROGRESS_INTERVAL {
                        spoke = Instant::now();
                        eprintln!("    {}", predict(p, started.elapsed()));
                    }
                })?;

                // Recorded after the run, not before it: a registry entry for a
                // store that failed to index is a store the daemon opens and the
                // portal lists with nothing in it.
                let model_name = store.model().to_string();
                home::record(&choice, roots, &model_name)?;

                // After the registry entry, because the roots this measures
                // against are the ones that entry has just recorded. A store
                // that holds files from outside them is a store whose hits
                // carry another store's label, so the default is to put it
                // right; `--reconcile report` says how many without touching
                // them, and `off` leaves them.
                if reconcile != Reconcile::Off {
                    let boundary = home::index_roots(&dir);
                    let strays = store.out_of_root(&boundary)?;
                    if !strays.is_empty() {
                        if reconcile == Reconcile::Report {
                            eprintln!(
                                "{} file(s) in this store sit outside its roots. \
                                 `--reconcile drop` removes them.",
                                strays.len()
                            );
                        } else {
                            let dropped = store.prune_out_of_root(&boundary)?;
                            eprintln!(
                                "reconciled: {dropped} file(s) dropped, held from outside \
                                 this store's roots. The files on disk are untouched."
                            );
                        }
                    }
                }

                let (files, chunks, bytes) = store.stats()?;
                // Images are counted apart from chunks because they are not
                // chunks: one image is one vector, and folding it into a chunk
                // count would make the number mean two things.
                let images = if report.images > 0 {
                    format!(", {} images", report.images)
                } else {
                    String::new()
                };
                // Stepped over whole. A person hunting for a file that is not
                // in their store needs the name of the directory that was
                // skipped, and a corpus that quietly excluded a dependency tree
                // without saying so is one nobody can reason about.
                if !report.generated.is_empty() {
                    eprintln!(
                        "not indexed, generated or vendored: {}",
                        report
                            .generated
                            .iter()
                            .map(|p| semlith::plain(p))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    eprintln!("  `SEMLITH_DEFAULT_IGNORES=0` indexes them anyway.");
                }
                // Named one per line. A refusal reported as a count is one the
                // person retries with the same arguments.
                for (path, why) in &report.refused {
                    eprintln!("refused: {} — {why}", semlith::plain(path));
                }
                if !report.refused.is_empty() && !include_secrets {
                    eprintln!("  `--include-secrets` indexes these anyway, if you meant to.");
                }
                // Named one per line, like the refused. A run that finishes having
                // failed on eleven files and says only "11 failed" is a run whose
                // eleven files nobody goes and looks at.
                for (path, why) in &report.failed {
                    eprintln!("failed: {} — {why}", semlith::plain(path));
                }
                // What the flag actually did. A store built with
                // `--include-secrets` that never says how many credentials it took
                // in is a store whose owner has no idea what is in it.
                if report.secrets_indexed > 0 {
                    eprintln!(
                        "indexed {} file(s) the credential scan would have refused, because \
                     `--include-secrets` was given",
                        report.secrets_indexed
                    );
                }
                // Skipped, broken out. "1 847 skipped" is the line that sent this
                // release's Windows logs in; "1 840 empty, 7 binary" is the same
                // fact and needs no investigation.
                let by_reason = if report.skipped_reasons.is_empty() {
                    String::new()
                } else {
                    let mut parts: Vec<(usize, &str)> = report
                        .skipped_reasons
                        .iter()
                        .map(|(kind, n)| (*n, kind.as_str()))
                        .collect();
                    parts.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
                    format!(
                        " ({})",
                        parts
                            .iter()
                            .map(|(n, kind)| format!("{n} {kind}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                let failed = if report.failed.is_empty() {
                    String::new()
                } else {
                    format!(", {} failed", report.failed.len())
                };
                if report.rechunked {
                    eprintln!(
                        "re-indexed every file: this release shows the embedding model a code \
                         chunk's own definition — its signature and the first line of its doc \
                         comment — so every chunk was cut and embedded again. The files \
                         themselves were not touched."
                    );
                }
                eprintln!(
                    "indexed {} files ({} chunks{images}) in {:.1}s — {} already indexed, {} skipped{by_reason}, {} removed{failed}",
                    report.indexed,
                    report.chunks,
                    started.elapsed().as_secs_f32(),
                    report.unchanged,
                    report.skipped,
                    report.removed,
                );
                eprintln!(
                    "store: {files} files, {chunks} chunks, {} at {}",
                    semlith::human_bytes(bytes),
                    dir.display()
                );
                if report.dummies_indexed > 0 {
                    eprintln!(
                        "{} file(s) indexed holding only declared test dummies (semlith refused lists them)",
                        report.dummies_indexed
                    );
                }
                let rows = semlith::store::refusals(store.db())?;
                let not: usize = rows
                    .iter()
                    .filter(|r| r.class != semlith::store::class::DUMMY && r.accepted.is_none())
                    .map(|r| r.files.max(1) as usize)
                    .sum();
                let review = rows
                    .iter()
                    .filter(|r| r.reviewable && r.accepted.is_none())
                    .count();
                if not > 0 {
                    eprintln!("{not} not indexed · {review} need review · semlith refused");
                }
            }

            // The readable roots are indexed and recorded; the status is what
            // changes, so a script that indexed several roots and tolerated a
            // missing one sees the failure it was told about (#76).
            if !unreadable.is_empty() {
                bail!(
                    "{} of {} paths could not be read, and were not indexed",
                    unreadable.len(),
                    unreadable.len() + roots.len()
                );
            }
        }

        Command::Watch {
            paths,
            debounce,
            quiet,
        } => {
            let roots = if paths.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                paths
            };

            let choice = home::resolve(&cli.store, &roots[0], None)?;
            if let Some(hint) = choice.hint() {
                eprintln!("{hint}");
            }
            let dir = choice.one()?;
            let mut store = Semlith::open(&dir, None)?;
            store.quiet = quiet;
            home::record(&choice, &roots, &store.model().to_string())?;

            // Installed before the first event: Ctrl-C is how this command
            // ends, so it has to leave the store whole.
            semlith::watch::stop_on_signal();

            let shown: Vec<String> = roots.iter().map(|r| display(r)).collect();
            semlith::watch::run(
                &mut store,
                &roots,
                std::time::Duration::from_millis(debounce),
                &semlith::watch::STOP,
                |progress| {
                    use semlith::watch::Progress;
                    match progress {
                        // A watcher that has quietly stopped updating looks
                        // exactly like a corpus nobody edited, so it says what
                        // it is watching and what it did.
                        Progress::Ready {
                            catch_up,
                            files,
                            chunks,
                        } if catch_up.remaining > 0 => eprintln!(
                            "watching {} — {files} files, {chunks} chunks \
                             (catch-up deferred: {} files queued)",
                            shown.join(", "),
                            catch_up.remaining,
                        ),
                        Progress::Ready {
                            catch_up,
                            files,
                            chunks,
                        } => eprintln!(
                            "watching {} — {files} files, {chunks} chunks \
                             ({} indexed at startup, {} unchanged)",
                            shown.join(", "),
                            catch_up.indexed,
                            catch_up.unchanged,
                        ),
                        Progress::File(path) if !quiet => eprintln!("  ~ {}", display(path)),
                        Progress::Batch(report, elapsed) if !quiet => eprintln!(
                            "  {} re-embedded, {} removed, {} chunks in {:.1}s",
                            report.indexed,
                            report.removed,
                            report.chunks,
                            elapsed.as_secs_f32(),
                        ),
                        Progress::Error(e) => eprintln!("semlith: watch error: {e}"),
                        _ => {}
                    }
                },
            )?;
        }

        Command::Brief {
            question,
            budget,
            path,
            ext,
            lang,
            prefer,
            json,
        } => brief(
            &cli.store, &cwd, question, budget, &path, &ext, &lang, &prefer, json,
        )?,

        Command::Search {
            query,
            k,
            path,
            ext,
            lang,
            prefer,
            exact,
            offset,
            json,
        } => {
            // Built before any store is opened, so an unknown language name
            // fails immediately rather than after a model load.
            let filter = Filter::new(&path, &ext, &lang)?;
            if exact {
                // No model: a grep reads the stored text and nothing else.
                let fleet = read_fleet(&cli.store, &cwd, false)?;
                let found = fleet.grep_in(None, &query, &filter, offset)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&found)?);
                } else {
                    let paths = fleet.shortener();
                    println!(
                        "{}",
                        paths.with_header(semlith::mcp::render_grep(&found, offset, &|p| {
                            paths.short(p)
                        }))
                    );
                }
                return Ok(());
            }
            // Parsed before the model loads, like the filter, so a typo in the
            // argument costs nothing.
            let prefer = semlith::Prefer::parse(&prefer)?;

            let mut fleet = read_fleet(&cli.store, &cwd, false)?;
            fleet.quiet = json;

            // A glob that selects nothing is a different answer from a corpus
            // that does not discuss the query, and only one of them is the
            // user's typo. Across several stores this is one question about the
            // whole selection: a filter that matches nothing in one store but
            // something in another has not selected nothing.
            let selected = (!filter.is_empty())
                .then(|| fleet.matching_files(&filter))
                .transpose()?;
            if selected == Some(0) {
                if json {
                    println!("[]");
                } else {
                    eprintln!("{}", fleet.no_match_reason(&filter));
                }
                return Ok(());
            }

            let started = Instant::now();
            let hits = fleet.search_preferring(None, &query, k, &filter, prefer)?;
            let elapsed = started.elapsed();
            // The command line is a client like any other, and its retrievals
            // count for exactly as much as an agent's. Recorded under `cli`, in
            // the same table, through the same path.
            semlith::ledger::search(&fleet, &CLI_LEDGER, &query, &hits, elapsed);

            if json {
                println!("{}", serde_json::to_string_pretty(&hits)?);
            } else if hits.is_empty() {
                eprintln!("{}", fleet.no_match_reason(&filter));
            } else {
                let mut out = std::io::stdout().lock();
                for (i, h) in hits.iter().enumerate() {
                    // The store is named only when there is more than one to
                    // tell apart, so a single-store search prints what it
                    // always printed.
                    let from = match &h.store {
                        Some(label) => format!("[{label}] "),
                        None => String::new(),
                    };
                    // One letter per list that found it: v vector, f full
                    // text, g graph. A hit the graph alone reached is a
                    // neighbour of a match rather than a match.
                    let via: String = h
                        .lists
                        .iter()
                        .map(|l| match *l {
                            "vector" => 'v',
                            "keyword" => 'f',
                            "image" => 'i',
                            _ => 'g',
                        })
                        .collect();
                    // An image carries its pixel size where a chunk carries a
                    // line range, and has no excerpt to print under it.
                    let where_in = match h.image {
                        Some(px) => format!("{}x{} px", px.width, px.height),
                        None => format!("{}-{}", h.start_line, h.end_line),
                    };
                    writeln!(
                        out,
                        "{}{}. {:.3} {via:<3} {from}{}:{where_in}{}",
                        bold(),
                        i + 1,
                        h.score,
                        display(std::path::Path::new(&h.path)),
                        reset()
                    )?;
                    for line in h.text.lines() {
                        writeln!(out, "   {line}")?;
                    }
                    writeln!(out)?;
                }
                let across = if fleet.len() > 1 {
                    // A store that contributed nothing is worth seeing: it is
                    // otherwise indistinguishable from one that was never
                    // opened.
                    let breakdown: Vec<String> = fleet
                        .labels()
                        .iter()
                        .map(|label| {
                            let n = hits
                                .iter()
                                .filter(|h| h.store.as_deref() == Some(*label))
                                .count();
                            format!("{label} {n}")
                        })
                        .collect();
                    format!(" across {} stores: {}", fleet.len(), breakdown.join(", "))
                } else {
                    String::new()
                };
                match selected {
                    Some(n) => eprintln!(
                        "{} hits in {:?} (filter selected {n} of {} files){across}",
                        hits.len(),
                        elapsed,
                        fleet.files()?,
                    ),
                    None => eprintln!("{} hits in {:?}{across}", hits.len(), elapsed),
                }
                // How the query was read, and what that did. Printed on every
                // answer rather than only when it was surprising: a caller can
                // only correct a misread shape if it can see one happened.
                let shape = semlith::shape_of(&query);
                match prefer {
                    semlith::Prefer::Any => {
                        eprintln!("{} · {}", shape.as_str(), shape.weighting())
                    }
                    chosen => eprintln!(
                        "{} · {} · prefer {}",
                        shape.as_str(),
                        shape.weighting(),
                        chosen.as_str()
                    ),
                }
                // Only when it happened. A store inside its budget never sees
                // this line, and a store past it should not have to guess why
                // its queries got slower.
                let evicted: u64 = fleet.each().map(|(_, s)| s.evictions()).sum();
                if evicted > 0 {
                    eprintln!(
                        "semlith: put down {evicted} shard(s) to stay inside the {} MB index \
                         budget; raise {} to hold more",
                        semlith::index::budget_mb(),
                        semlith::index::INDEX_MEMORY_ENV,
                    );
                }
            }
        }

        Command::Ledger { last, verify, json } => {
            let fleet = read_fleet(&cli.store, &cwd, false)?;
            let many = fleet.len() > 1;
            if verify {
                let mut broken = false;
                for (label, store) in fleet.each() {
                    match semlith::store::ledger_break(store.db())? {
                        Some(row) => {
                            broken = true;
                            println!("{label}: the chain does not verify from row {row} onwards");
                        }
                        None => println!("{label}: the chain is intact"),
                    }
                    // What the chain is a chain of. A verify that says only
                    // "intact" proves the record was not edited and says
                    // nothing about what it records, which is the question
                    // somebody defending a savings figure is actually asked.
                    print_savings(store, "  ")?;
                }
                // Non-zero so a script can act on it. A verify that reported a
                // broken chain and exited 0 would be worse than no verify.
                if broken {
                    std::process::exit(1);
                }
                return Ok(());
            }
            let mut any = false;
            for (label, store) in fleet.each() {
                let rows = semlith::store::retrievals(store.db(), last)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                    any = any || !rows.is_empty();
                    continue;
                }
                if many {
                    println!("{}{label}{}", bold(), reset());
                }
                if rows.is_empty() {
                    continue;
                }
                any = true;
                let mut out = std::io::stdout().lock();
                for row in &rows {
                    writeln!(
                        out,
                        "{}  {:<14} {:>3} hits  {:>6} ms  {}",
                        human_time(row.at),
                        row.client,
                        row.hits,
                        row.micros / 1000,
                        row.query,
                    )?;
                }
                // A chain that does not verify is the one thing this table can
                // say that a log file cannot, so it is said loudly.
                if let Some(broken) = semlith::store::ledger_break(store.db())? {
                    writeln!(
                        out,
                        "
  the chain does not verify from row {broken} onwards: \
                         these rows have been edited or removed"
                    )?;
                }
            }
            if any && !json {
                for (label, store) in fleet.each() {
                    if many {
                        println!();
                        println!("{}{label}{}", bold(), reset());
                    } else {
                        println!();
                    }
                    print_savings(store, "  ")?;
                }
            }
            if !any && !json {
                eprintln!(
                    "nothing recorded yet. The daemon records by default from 0.15.0 and says \
                     so on every start; `--no-ledger` or SEMLITH_LEDGER=0 stops it. Nothing \
                     recorded ever leaves this machine."
                );
            }
        }

        Command::Symbol {
            names,
            k,
            history,
            json,
        } => {
            let fleet = read_fleet(&cli.store, &cwd, false)?;
            if names.len() > 1 && !history {
                if names.len() > semlith::graph::NAMES_LIMIT {
                    bail!(
                        "at most {} names at once; {} given",
                        semlith::graph::NAMES_LIMIT,
                        names.len()
                    );
                }
                let rows = fleet.signatures_in(None, &names)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&rows)?);
                } else if rows.is_empty() {
                    eprintln!("{}", nothing_known(&fleet, &names.join(", ")));
                } else {
                    println!(
                        "{}",
                        semlith::graph::render_signatures(&rows, &|p| display(
                            std::path::Path::new(p)
                        ))
                    );
                }
                return Ok(());
            }
            let name = names[0].clone();
            if history {
                let past = fleet.past_in(None, &name, k)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&past)?);
                } else if past.is_empty() {
                    eprintln!(
                        "no earlier definition of {name} is recorded \
                         (a store keeps history from its first index pass under 0.23.0)"
                    );
                } else {
                    let mut out = std::io::stdout().lock();
                    for row in &past {
                        writeln!(
                            out,
                            "{}{} {}{} {}:{}-{}  until {}  hash {}",
                            bold(),
                            row.kind,
                            row.qualified,
                            reset(),
                            display(std::path::Path::new(&row.path)),
                            row.start_line,
                            row.end_line,
                            semlith::clock::local_stamp(row.retired_at),
                            &row.content_hash[..row.content_hash.len().min(12)],
                        )?;
                    }
                }
                return Ok(());
            }
            // One answer rather than three: the definition, who calls it, what
            // it calls, and the ring beyond that. Asking for a definition and
            // then having to ask twice more to know whether it was the right
            // one is what this replaces.
            let found =
                fleet.evidence_in(None, &name, &semlith::graph::dependency_kinds(), k, false)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&found)?);
            } else if found.definitions.is_empty() {
                eprintln!("{}", nothing_known(&fleet, &name));
            } else {
                let mut out = std::io::stdout().lock();
                writeln!(
                    out,
                    "{}",
                    found.render(bold(), reset(), &|p| display(std::path::Path::new(p)))
                )?;
            }
        }

        Command::Read { target, path, json } => {
            let filter = Filter::new(&path, &[], &[])?;
            let target = semlith::Target::parse(&target);
            let fleet = read_fleet(&cli.store, &cwd, false)?;
            let started = Instant::now();
            let found = fleet.read_in(None, &target, &filter)?;
            semlith::ledger::graph(
                &fleet,
                &CLI_LEDGER,
                "read",
                match &target {
                    semlith::Target::Span { path, .. } => path,
                    semlith::Target::Symbol(name) => name,
                },
                "",
                found.is_some(),
                started.elapsed(),
            );
            match found {
                None => eprintln!(
                    "nothing indexed at that span or under that name (store has {} chunks)",
                    fleet.chunks()
                ),
                Some(found) if json => println!("{}", serde_json::to_string_pretty(&found)?),
                // Several definitions and nothing to choose between them, so
                // the list is the answer rather than a guess at which one.
                Some(semlith::Read::Choose(rows)) => {
                    let mut out = std::io::stdout().lock();
                    writeln!(out, "{} definitions of this name:", rows.len())?;
                    for row in &rows {
                        writeln!(
                            out,
                            "  {} {}  {}{}:{}-{}",
                            row.name,
                            row.kind,
                            store_prefix(&row.store),
                            display(std::path::Path::new(&row.path)),
                            row.start_line,
                            row.end_line,
                        )?;
                    }
                }
                Some(semlith::Read::One(span)) => {
                    let mut out = std::io::stdout().lock();
                    let named = match (&span.symbol, &span.symbol_kind) {
                        (Some(name), Some(kind)) => format!("  {name} {kind}"),
                        (Some(name), None) => format!("  {name}"),
                        _ => String::new(),
                    };
                    writeln!(
                        out,
                        "{}{}{}:{}-{}{named}{}{}",
                        bold(),
                        store_prefix(&span.store),
                        display(std::path::Path::new(&span.path)),
                        span.start_line,
                        span.end_line,
                        if span.fresh { "" } else { "  · stale" },
                        reset(),
                    )?;
                    for line in span.text.lines() {
                        writeln!(out, "{line}")?;
                    }
                }
            }
        }

        Command::Pattern {
            query,
            lang,
            path,
            offset,
            json,
        } => {
            let filter = Filter::new(&path, &[], &[])?;
            let fleet = read_fleet(&cli.store, &cwd, false)?;
            let started = Instant::now();
            let found = fleet.pattern_in(None, &lang, &query, &filter, offset)?;
            semlith::ledger::graph(
                &fleet,
                &CLI_LEDGER,
                "pattern",
                &query,
                "",
                !found.matches.is_empty(),
                started.elapsed(),
            );
            if json {
                println!("{}", serde_json::to_string_pretty(&found)?);
            } else if found.matches.is_empty() {
                // Two different facts, and only one of them means the caller
                // should change the pattern.
                if found.files == 0 {
                    eprintln!("no indexed file is {}", found.language);
                } else {
                    eprintln!("no match in {} {} files", found.files, found.language);
                }
            } else {
                let mut out = std::io::stdout().lock();
                for found in &found.matches {
                    writeln!(
                        out,
                        "{}{}{}:{}-{}{} @{}  {}",
                        bold(),
                        store_prefix(&found.store),
                        display(std::path::Path::new(&found.path)),
                        found.start_line,
                        found.end_line,
                        reset(),
                        found.capture,
                        found.text,
                    )?;
                }
                eprintln!(
                    "{} match(es) in {} {} files{}",
                    found.matches.len(),
                    found.files,
                    found.language,
                    if found.truncated {
                        // The offset that continues it, not just the fact that
                        // something was cut off.
                        format!(
                            " (truncated — --offset {} for the rest)",
                            offset + found.matches.len()
                        )
                    } else {
                        String::new()
                    },
                );
            }
        }

        Command::Neighbors {
            name,
            kind,
            all,
            json,
        } => {
            for k in &kind {
                if !semlith::graph::KINDS.contains(&k.as_str()) {
                    anyhow::bail!(
                        "unknown edge kind {k:?}; the kinds are {}",
                        semlith::graph::KINDS.join(", ")
                    );
                }
            }
            let fleet = read_fleet(&cli.store, &cwd, false)?;
            let started = Instant::now();
            let neighbours = fleet.neighbours_in(None, &name, &kind, all)?;
            let found = !neighbours.callers.is_empty() || !neighbours.callees.is_empty();
            semlith::ledger::graph(
                &fleet,
                &CLI_LEDGER,
                "neighbors",
                &name,
                "",
                found,
                started.elapsed(),
            );
            if json {
                println!("{}", serde_json::to_string_pretty(&neighbours)?);
            } else if neighbours.callers.is_empty()
                && neighbours.callees.is_empty()
                && neighbours.hidden == 0
                && neighbours.unresolved.is_empty()
            {
                eprintln!("{}", nothing_known(&fleet, &name));
            } else {
                let mut out = std::io::stdout().lock();
                print_ends(&mut out, "callers", &neighbours.callers)?;
                print_ends(&mut out, "callees", &neighbours.callees)?;
                if neighbours.hidden > 0 {
                    writeln!(
                        out,
                        "
{} target{} outside this store, not listed (--all)",
                        neighbours.hidden,
                        if neighbours.hidden == 1 { "" } else { "s" },
                    )?;
                }
                if !neighbours.unresolved.is_empty() {
                    writeln!(
                        out,
                        "
{}outside this store{}",
                        bold(),
                        reset()
                    )?;
                    for end in &neighbours.unresolved {
                        writeln!(out, "  {} via {}", end.name, end.kind)?;
                    }
                }
            }
        }

        Command::Impact {
            name,
            kind,
            depth,
            all_edges,
            strict,
            json,
        } => {
            for k in &kind {
                if !semlith::graph::KINDS.contains(&k.as_str()) {
                    anyhow::bail!(
                        "unknown edge kind {k:?}; the kinds are {}",
                        semlith::graph::KINDS.join(", ")
                    );
                }
            }
            let all_edges = all_edges && !strict;
            let fleet = read_fleet(&cli.store, &cwd, false)?;
            let started = Instant::now();
            let impact = fleet.impact_in(None, &name, &kind, depth, all_edges)?;
            semlith::ledger::graph(
                &fleet,
                &CLI_LEDGER,
                "impact",
                &name,
                "",
                !impact.reached.is_empty(),
                started.elapsed(),
            );
            if json {
                println!("{}", serde_json::to_string_pretty(&impact)?);
            } else {
                println!(
                    "{}",
                    impact.render(bold(), reset(), &|p| display(std::path::Path::new(p)))
                );
            }
        }

        Command::Trace {
            from,
            to,
            depth,
            all_edges,
            strict,
            evidence,
            json,
        } => {
            let all_edges = all_edges && !strict;
            let fleet = read_fleet(&cli.store, &cwd, false)?;
            let started = Instant::now();
            let shorten = |p: &str| display(std::path::Path::new(p));
            let trace = fleet.trace_in(None, &from, &to, depth, all_edges, &shorten)?;
            semlith::ledger::graph(
                &fleet,
                &CLI_LEDGER,
                "trace",
                &from,
                &to,
                trace.chain.is_some(),
                started.elapsed(),
            );
            if json {
                println!("{}", serde_json::to_string_pretty(&trace)?);
            } else if evidence {
                print!("{}", trace.evidence(&shorten));
            } else {
                print!("{}", trace.render(bold(), reset(), &shorten));
            }
        }

        Command::Report {
            kind,
            format,
            model,
            window,
            scope,
            excerpts,
            redact,
            out,
        } => {
            // Both names checked before a store is opened: a typo in either
            // is a question about the argument, not about the directory the
            // command happened to run in.
            semlith::report::kind_of(&kind)?;
            semlith::report::check_any_format(&format)?;
            let window = semlith::report::window_of(window.as_deref())?;
            // The third of the three, for the same reason: `--model opus_5`
            // used to be accepted and priced at Sonnet 5, so the report said
            // Sonnet 5 and the reader had asked for Opus.
            if semlith::report::price_named(&model).is_none() {
                anyhow::bail!(
                    "no prices for model {model:?} — this binary prices {}",
                    semlith::report::price_names()
                );
            }
            // Refused before the fleet is opened, for the same reason: a PDF
            // on standard output is a terminal full of binary and no file.
            if format == semlith::report::PDF && out.is_none() {
                anyhow::bail!("a pdf is bytes rather than text — name a file with --out");
            }
            let fleet = read_fleet(&cli.store, &cwd, false)?;
            let report = semlith::report::generate_with(
                &fleet,
                &kind,
                &model,
                window,
                &scope,
                semlith::report::Options { excerpts, redact },
            )?;
            match out {
                Some(path) => {
                    std::fs::write(&path, report.render_bytes(&format)?)?;
                    println!("{} written to {}", report.title, display(&path));
                }
                None => print!("{}", report.render(&format)?),
            }
        }

        Command::Schedule { what } => run_schedule(what)?,

        Command::Path {
            from,
            to,
            depth,
            all_edges,
            strict,
            json,
        } => {
            let all_edges = all_edges && !strict;
            let fleet = read_fleet(&cli.store, &cwd, false)?;
            let started = Instant::now();
            let chain = fleet.path_in(None, &from, &to, depth, all_edges)?;
            semlith::ledger::graph(
                &fleet,
                &CLI_LEDGER,
                "path",
                &from,
                "",
                chain.is_some(),
                started.elapsed(),
            );
            if json {
                println!("{}", serde_json::to_string_pretty(&chain)?);
            } else {
                match chain {
                    // An empty chain is `from == to`, which is a path of no
                    // hops rather than no path.
                    Some(chain) if chain.steps.is_empty() => {
                        println!("{from} is {to}");
                    }
                    Some(chain) => {
                        let mut out = std::io::stdout().lock();
                        write!(
                            out,
                            "{}",
                            chain.render(bold(), reset(), &|p| display(std::path::Path::new(p)))
                        )?;
                    }
                    None => eprintln!(
                        "{}",
                        semlith::graph::not_connected(&from, &to, depth, all_edges)
                    ),
                }
            }
        }

        Command::Languages => {
            for entry in semlith::filter::LANGUAGES {
                // Extensions print with their dot and filenames without one,
                // which is the difference a reader has to see: `.mk` is an
                // extension and `Makefile` is the whole name of the file.
                let what = entry
                    .extensions
                    .iter()
                    .map(|e| format!(".{e}"))
                    .chain(entry.filenames.iter().map(|f| f.to_string()))
                    .collect::<Vec<_>>()
                    .join(" ");
                // The same column the portal's About page and
                // `semlith_languages` show: whether this language carries
                // symbols and edges as well as text.
                let graph = if semlith::graph::has_graph(entry.name) {
                    "graph"
                } else {
                    "     "
                };
                println!("{:<12} {graph}  {what}", entry.name);
            }
        }

        Command::Stats => {
            let fleet = read_fleet(&cli.store, &cwd, false)?;
            let many = fleet.len() > 1;
            let mut totals = (0, 0, 0);
            for (label, store) in fleet.each() {
                let (files, chunks, bytes) = store.stats()?;
                totals = (totals.0 + files, totals.1 + chunks, totals.2 + bytes);
                if many {
                    println!("{label}");
                }
                // The store's own directory, not the flag order: a store named
                // twice was opened once.
                println!("store    {}", store.dir().display());
                // Said where somebody will see it. A store directory other
                // accounts on this machine can read holds the text of every
                // file it indexed, and one made before 0.14.0 is loose until
                // this binary opens it.
                if let Some(mode) = home::loose_mode(store.dir()) {
                    println!(
                        "warning  the store directory is mode {mode:o}; other users on this \
                         machine can read what it indexed. Opening it with this semlith \
                         narrows it to 700."
                    );
                }
                println!("model    {} ({} dim)", store.model(), store.dim());
                // Which order the answers came in. A store searched without
                // the cross-encoder ranks by fusion alone, and that is a
                // different answer from the one the release measured.
                let cache = semlith::model_cache_dir().unwrap_or_default();
                println!(
                    "ranking  {}",
                    if !semlith::rerank::enabled() {
                        format!(
                            "fusion alone — {}=on adds {}, at about 124 ms a search",
                            semlith::rerank::RERANK_ENV,
                            semlith::rerank::RERANK_NAME
                        )
                    } else if semlith::rerank::cached(&cache) {
                        format!("fusion, then {}", semlith::rerank::RERANK_NAME)
                    } else {
                        "fusion alone — run `semlith setup` to add the rescoring model".to_string()
                    }
                );
                println!("files    {files}");
                // Which rule cut them, because from 0.22.0 there are two and a
                // store keeps the one it was last swept under. A store still on
                // fixed windows is a store whose next full index pass will
                // re-chunk it, and that is worth knowing before wondering why a
                // definition and its doc comment answer separately.
                println!(
                    "chunks   {chunks} (cut at {})",
                    semlith::store::chunking(store.db())?
                );
                // Which variant of the model embedded them, where the store
                // has counted: int8 on the CPU and fp16 on a GPU are two
                // variants of one model that agree at cosine 0.987.
                let variants = store.variants();
                if !variants.is_empty() {
                    let parts: Vec<String> =
                        variants.iter().map(|(v, n)| format!("{n} {v}")).collect();
                    println!("variants {}", parts.join(", "));
                }
                let on = semlith::accel::enabled();
                let lanes: Vec<&str> = [("cpu", on.cpu), ("gpu", on.gpu), ("cuda", on.cuda)]
                    .into_iter()
                    .filter(|(_, on)| *on)
                    .map(|(lane, _)| lane)
                    .collect();
                println!("lanes    {} ({})", lanes.join(", "), on.source);
                let images = store.image_count()?;
                if images > 0 {
                    println!("images   {images}");
                }
                println!("vectors  {}", store.len());
                if let Some((shards, max)) = store.shards() {
                    // What the store costs to search, before searching it.
                    println!(
                        "shards   {shards}, up to {max} resident ({} MB budget, {})",
                        semlith::index::budget_mb(),
                        semlith::index::INDEX_MEMORY_ENV,
                    );
                }
                println!("indexed  {}", semlith::human_bytes(bytes));
                // What rescoring costs in bytes, and whether this store has it
                // at all. A store from before 0.23.0 says so rather than
                // quietly ranking by codes alone.
                match store.exact_bytes() {
                    Some(exact) => println!(
                        "exact    {} (full-precision rescoring)",
                        semlith::human_bytes(exact as i64)
                    ),
                    None => println!("exact    none (re-index to rescore in full precision)"),
                }
                // Zero here reads the same as "nothing ever changed", so the
                // line says which by naming the table rather than the number
                // alone.
                println!("history  {} retired definitions", store.retired_symbols()?);
                // What the graph covers, per language, from the rows
                // themselves. An absent edge and an unparsed file look
                // identical in a single store-wide percentage, and the
                // README's resolution figure was reproducible from nothing a
                // user could run until this table existed.
                let coverage = semlith::store::coverage_by_language(store.db())?;
                if !coverage.is_empty() {
                    println!(
                        "graph    language      files  unparsed  defs  extracted  resolved  \
                         ambiguous  unresolved  settled"
                    );
                    for row in &coverage {
                        println!(
                            "         {:<12} {:>5} {:>9} {:>5} {:>10} {:>9} {:>10} {:>11} {:>7} %",
                            row.language,
                            row.files,
                            row.parser_failed,
                            row.definitions,
                            row.extracted,
                            row.resolved,
                            row.ambiguous,
                            row.unresolved,
                            row.settled_share(),
                        );
                    }
                    // The two figures the per-language table cannot hold,
                    // because neither belongs to a language: what the graph
                    // points at and cannot find, and how many names it could
                    // not tell apart. The portal's Graph health card reads
                    // exactly these, so the page and this table agree.
                    let (top, distinct) = semlith::store::unresolved_targets(store.db(), 5)?;
                    let several = semlith::store::names_with_several_definitions(store.db())?;
                    let named = top
                        .iter()
                        .map(|(name, n)| format!("{name} ({n})"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    println!(
                        "graph    {distinct} call targets with no definition here{}",
                        if named.is_empty() {
                            String::new()
                        } else {
                            format!(" — commonest {named}")
                        }
                    );
                    println!("graph    {several} names with several definitions");
                }
                // One line, and never a number without its denominator. A
                // store that has recorded nothing prints nothing here rather
                // than a zero that reads like a measurement.
                let savings = semlith::store::ledger_savings(store.db())?;
                if savings.total > 0 {
                    println!(
                        "saved    {} tokens not read, over {} of {} retrievals \
                         ({}% coverage, {})",
                        savings.net,
                        savings.credited,
                        savings.total,
                        savings.coverage(),
                        savings.tier(),
                    );
                }
                if many {
                    println!();
                }
            }
            // The per-store blocks are the diagnostic; the total is the answer
            // to "how much is indexed".
            if many {
                println!(
                    "total    {} stores, {} files, {} chunks, {}",
                    fleet.len(),
                    totals.0,
                    totals.1,
                    semlith::human_bytes(totals.2),
                );
            }
        }

        Command::Files {
            tree,
            depth,
            sort,
            path,
        } => {
            let fleet = read_fleet(&cli.store, &cwd, false)?;
            if tree {
                let filter = Filter::new(&path, &[], &[])?;
                let sort = semlith::tree::Sort::parse(&sort)?;
                println!(
                    "{}",
                    semlith::tree::render(&fleet, None, &filter, depth.max(1), sort)?
                );
                return Ok(());
            }
            let many = fleet.len() > 1;
            for (label, store) in fleet.each() {
                if many {
                    println!("{label}");
                }
                for path in semlith::store::all_paths(store.db())? {
                    println!("{}", display(std::path::Path::new(&path)));
                }
                if many {
                    println!();
                }
            }
        }

        Command::Add { url, name, airgap } => {
            arm_airgap(airgap);

            // The current directory is the anchor, the way it is for `index`
            // with no path: "add this to the store I am working in".
            let choice = home::resolve(&cli.store, &cwd, name.as_deref())?;
            let choice = resolve_for_add(choice, &cli.store, name.as_deref())?;
            if let Some(hint) = choice.hint() {
                eprintln!("{hint}");
            }
            let dir = choice.one()?;

            let spinner = cliclack::spinner();
            spinner.start(format!("fetching {url}"));
            let fetched = match semlith::add::fetch(&url, &dir) {
                Ok(fetched) => {
                    spinner.stop(format!(
                        "fetched {}",
                        semlith::human_bytes(fetched.bytes as i64)
                    ));
                    fetched
                }
                Err(e) => {
                    spinner.error("fetch failed");
                    return Err(e);
                }
            };

            let mut store = Semlith::open(&dir, None)?;
            store.quiet = true;
            let report = store.index_paths(std::slice::from_ref(&fetched.path), |_, _| {})?;

            // The root recorded is the downloads directory, not the file: a
            // root is what `semlith start` watches, and watching one file would
            // leave the next thing added to the same store unwatched.
            //
            // Recorded after the fetch and the index both succeeded, so a
            // failed add leaves no registry entry pointing at nothing.
            let model_name = store.model().to_string();
            home::record(&choice, &[semlith::add::downloads_dir(&dir)], &model_name)?;

            eprintln!("{} -> {}", fetched.url, display(&fetched.path));
            eprintln!("indexed {} chunks into {}", report.chunks, dir.display());
        }

        Command::Scan { name, forget, json } => {
            let choice = home::resolve(&cli.store, &cwd, name.as_deref())?;
            if let Some(hint) = choice.hint() {
                eprintln!("{hint}");
            }
            if matches!(choice, home::Choice::New { .. }) {
                anyhow::bail!("there is no store here to scan");
            }
            let mut store = Semlith::open(&choice.one()?, None)?;
            let findings = store.scan()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&findings)?);
            } else {
                for finding in &findings {
                    println!(
                        "{} — {}",
                        display(std::path::Path::new(&finding.path)),
                        finding.why
                    );
                }
            }
            if findings.is_empty() {
                if !json {
                    println!("nothing this store holds would be refused today.");
                }
                return Ok(());
            }
            if forget {
                let gone = store.forget_findings(&findings)?;
                if !json {
                    println!("removed {gone} file(s) from the store.");
                }
                return Ok(());
            }
            if !json {
                println!(
                    "{} file(s) would be refused today. `semlith scan --forget` removes them.",
                    findings.len()
                );
            }
            // Non-zero while anything is still in the store, so this is usable
            // as a check. A scan that found credentials and exited 0 is a
            // check that passes on the case it exists to catch.
            std::process::exit(1);
        }

        Command::Refused { action, json } => {
            let dirs = home::all_dirs(&cli.store, &cwd)?;
            let upstream = semlith::proxy::find(&dirs);
            match action {
                None => {
                    let listing: serde_json::Value = match &upstream {
                        Some(daemon) => serde_json::from_str(&daemon.get("/api/refused")?)?,
                        None => {
                            let fleet = read_fleet(&cli.store, &cwd, true)?;
                            let mut stores = Vec::new();
                            let (mut total, mut review) = (0usize, 0usize);
                            for (label, store) in fleet.each() {
                                let rows = semlith::store::refusals(store.db())?;
                                let needs = rows
                                    .iter()
                                    .filter(|r| r.reviewable && r.accepted.is_none())
                                    .count();
                                total += rows
                                    .iter()
                                    .filter(|r| {
                                        r.class != semlith::store::class::DUMMY
                                            && r.accepted.is_none()
                                    })
                                    .map(|r| r.files.max(1) as usize)
                                    .sum::<usize>();
                                review += needs;
                                stores.push(serde_json::json!({ "store": label, "rows": rows, "review": needs }));
                            }
                            serde_json::json!({ "stores": stores, "total": total, "review": review })
                        }
                    };
                    if json {
                        println!("{}", serde_json::to_string_pretty(&listing)?);
                    } else {
                        print_refused(&listing);
                    }
                }
                Some(RefusedCommand::Revoke { path }) => {
                    let key = semlith::canonical(&path).to_string_lossy().into_owned();
                    let body =
                        serde_json::json!({ "path": key, "store": store_of(&cli.store, &path)? });
                    match &upstream {
                        Some(daemon) => {
                            daemon.post("/api/refused/revoke", &body)?;
                        }
                        None => {
                            let dir = home::resolve(&cli.store, &path, None)?.one()?;
                            let mut store = Semlith::open(&dir, None)?;
                            if !store.revoke(&key)? {
                                bail!("{} has no acceptance to revoke", path.display());
                            }
                            store.index_paths(&[PathBuf::from(&key)], |_, _| {})?;
                        }
                    }
                    eprintln!("revoked {}; it is refused again and listed", path.display());
                }
                Some(RefusedCommand::Accept {
                    path,
                    redact,
                    as_is: _,
                    yes,
                }) => {
                    decide_refused(
                        &cli.store,
                        upstream.as_ref(),
                        &path,
                        if redact { "redacted" } else { "as-is" },
                        yes,
                    )?;
                }
                Some(RefusedCommand::Refuse { path, yes }) => {
                    decide_refused(&cli.store, upstream.as_ref(), &path, "refused", yes)?;
                }
            }
        }

        Command::Forget { path } => {
            // Anchored on the file, the way `index` is anchored on the path it
            // is given. Resolving from the working directory asked whichever
            // store covers wherever the user happened to be standing, which for
            // a forget run from anywhere but the corpus is a store that has
            // never held the file: it removed nothing, said so, and exited 0
            // (#77). A caller that believes the exit status believes the file
            // is gone.
            // The directory holding the file, and the working directory for a
            // bare name with no directory in it at all — which is how somebody
            // standing in the corpus types it.
            let anchor = if path.is_dir() {
                path.clone()
            } else {
                match path.parent() {
                    Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
                    _ => cwd.clone(),
                }
            };
            let choice = home::resolve(&cli.store, &anchor, None)?;
            if let Some(hint) = choice.hint() {
                eprintln!("{hint}");
            }
            // `New` is the store that *would* be created for this path. Opening
            // it would create an empty store directory to delete nothing from.
            if matches!(choice, home::Choice::New { .. }) {
                anyhow::bail!(not_indexed(&path));
            }
            let mut store = Semlith::open(&choice.one()?, None)?;
            let (chunks, images) = store.forget(&path)?;
            if chunks == 0 && images == 0 {
                anyhow::bail!(not_indexed(&path));
            }
            // An image has no chunks, so a message counting only chunks would
            // report a successful forget as having done nothing.
            let what = match (chunks, images) {
                (0, images) => format!("{images} image vector(s)"),
                (chunks, 0) => format!("{chunks} chunks"),
                (chunks, images) => format!("{chunks} chunks and {images} image vector(s)"),
            };
            eprintln!("removed {what} for {}", path.display());
        }

        Command::Drop { name: store, yes } => {
            let dir = semlith::home::Registry::dir_of(&store)?;
            if !semlith::home::Registry::load()?.stores.contains_key(&store) {
                anyhow::bail!("no registered store called {store}");
            }
            if !yes {
                let go = cliclack::confirm(format!(
                    "Delete {store}? Its vectors, chunks, graph and ledger go;                      the files it indexed are untouched."
                ))
                .initial_value(false)
                .interact()
                .unwrap_or(false);
                if !go {
                    eprintln!("nothing was deleted");
                    return Ok(());
                }
            }

            // Through the daemon when one is holding it: it owns the lock, and
            // deleting the files under an open writer is how half a store is
            // left behind.
            let dirs = semlith::home::all_dirs(&cli.store, &cwd).unwrap_or_default();
            let through_daemon = match semlith::proxy::find(&dirs) {
                Some(upstream) => upstream.delete_store(&store).is_ok(),
                None => false,
            };
            if !through_daemon {
                semlith::home::delete_store(&store)?;
            }
            eprintln!("deleted {store} ({})", dir.display());
            eprintln!("the files it indexed are untouched");
        }

        Command::Start {
            service,
            no_service,
            paths,
            no_ledger,
            port,
            debounce,
            airgap,
            no_mcp_http,
        } => {
            // Both of these end the process. `--service` hands the daemon to
            // the thing that will keep starting it; running one in this
            // terminal as well would be two daemons racing for every store's
            // write lock, and the second would lose and say so.
            if no_service {
                let removed = semlith::service::remove()?;
                println!(
                    "{}",
                    if removed {
                        "semlith: login service removed — a daemon already running is untouched"
                    } else {
                        "semlith: no login service was installed"
                    }
                );
                return Ok(());
            }
            if service {
                let installed = semlith::service::install(None, port)?;
                print_service(&installed);
                return Ok(());
            }
            arm_airgap(airgap);
            let dirs = semlith::daemon::stores_to_open(&cli.store, &paths, &cwd)?;
            if dirs.is_empty() {
                // Not an error: the portal's welcome screen exists for exactly
                // this, and telling someone to go index something first is what
                // the screen does better than a bail! does.
                eprintln!("semlith: no store registered yet — the portal will offer to make one");
            }
            // A store a live daemon already holds is not a failure — it is the
            // daemon the user asked for, already running. On a machine where
            // the login service starts one, the error chain this used to print
            // fired on the most ordinary command there is.
            let (held, free) = semlith::daemon::held_by_daemon(&dirs);
            for (dir, found) in &held {
                println!(
                    "semlith: {} is already served by a running daemon — pid {}, port {}, semlith {}",
                    dir.display(),
                    found.pid,
                    found.port,
                    found.version
                );
                // Alone on its line so a terminal makes it clickable, and on
                // stdout for the reason `daemon::run` prints its own URL there:
                // the token is in it and stderr is the log. Same shape as
                // `http::Server::url`, which is a method on a running server
                // and cannot be called about somebody else's.
                println!("http://127.0.0.1:{}/?token={}", found.port, found.token);
            }
            // The held stores are skipped, not fatal. An invocation naming
            // three stores of which one has a daemon still serves the other
            // two: refusing all three would make the free stores hostage to
            // the held one, and the held one is already being served by
            // definition. Only when nothing is left to open does this return —
            // with success, because nothing failed.
            if free.is_empty() && !held.is_empty() {
                return Ok(());
            }
            semlith::daemon::run(
                &free,
                semlith::daemon::port_of(port),
                std::time::Duration::from_millis(debounce),
                semlith::embed::airgap(),
                !no_ledger && semlith::ledger::enabled(),
                !no_mcp_http,
                |line| eprintln!("semlith: {line}"),
            )?;
        }

        Command::Key { what } => match what {
            KeyCommand::Show { quiet } => {
                let key = semlith::home::agent_key()?;
                if quiet {
                    println!("{key}");
                } else {
                    let port = semlith::daemon::port_of(None);
                    println!("{}{key}{}", bold(), reset());
                    println!("{}", semlith::home::agent_key_path()?.display());
                    println!();
                    println!("It authenticates the MCP endpoint and nothing else, and it does not");
                    println!("change when the daemon restarts or the portal's token is rotated.");
                    for stanza in semlith::clients::http_stanzas(&key) {
                        println!();
                        println!("{}{}{}", bold(), stanza.format, reset());
                        print!("{}", stanza.text);
                    }
                    println!();
                    println!("Endpoint: http://127.0.0.1:{port}/mcp");
                }
            }
            KeyCommand::Rotate { now } => {
                // Read before it is replaced: it is what identifies the
                // stanzas to rewrite.
                let previous = semlith::home::agent_key().unwrap_or_default();
                let fresh = semlith::home::rotate_agent_key()?;
                let carried = semlith::setup::recarry_key(&previous, &fresh);
                // A running daemon holds the key in memory, so it is told
                // rather than left serving only the key it started with.
                let dirs = semlith::home::all_dirs(&cli.store, &cwd).unwrap_or_default();
                let reached = match semlith::proxy::find(&dirs) {
                    Some(upstream) => upstream.adopt_key(&fresh, now).is_ok(),
                    None => false,
                };

                println!("{}{fresh}{}", bold(), reset());
                if reached {
                    if now {
                        println!("The running daemon took it up; the previous key is refused now.");
                    } else {
                        println!(
                            "The running daemon took it up. The previous key keeps working until \
                             that daemon exits, so a session already open finishes."
                        );
                    }
                } else {
                    println!(
                        "No daemon is running here; the next `semlith start` reads the new key."
                    );
                }

                // Nothing is re-registered. From 0.18.0 every registration
                // semlith writes is the stdio form: the client launches
                // `semlith mcp`, which reads the key out of
                // `~/.semlith/agent.key` itself, so a rotation reaches it with
                // no configuration rewritten anywhere. That is the property the
                // stdio form was chosen for, and it is worth saying out loud on
                // the command that used to have to repair one client by hand.
                if carried.is_empty() {
                    println!(
                        "No configuration file on this machine carried the old key, and none \
                         needed to: a registration semlith wrote launches `semlith mcp`, which \
                         reads the key from the file you just rotated."
                    );
                } else {
                    println!();
                    println!("Carried the new key into:");
                    for path in &carried {
                        println!("  {}", path.display());
                    }
                }

                println!();
                println!("Any client configured somewhere else needs the stanza below pasted in.");
                for stanza in semlith::clients::http_stanzas(&fresh) {
                    println!();
                    print!("{}", stanza.text);
                }
            }
        },

        Command::Trust { store_dir, list } => {
            let mut registry = home::Registry::load()?;
            if list || store_dir.is_none() {
                if registry.trusted.is_empty() {
                    eprintln!(
                        "no store outside {} is trusted. Every store semlith made is \
                         opened without asking; a `.semlith` that arrived some other \
                         way needs `semlith trust <dir>` once.",
                        home::home_or_error()?.display()
                    );
                } else {
                    for dir in &registry.trusted {
                        let missing = if dir.join("store.db").exists() {
                            ""
                        } else {
                            "  (gone)"
                        };
                        println!("{}{missing}", dir.display());
                    }
                }
                return Ok(());
            }
            let dir = registry.trust(store_dir.as_deref().expect("checked above"))?;
            eprintln!(
                "{} is trusted. semlith will open it from this directory without \
                 --store; `semlith adopt` moves it into {} if you would rather it \
                 lived with the others.",
                dir.display(),
                home::stores_root()?.display()
            );
        }

        Command::Adopt {
            store_dir,
            root,
            name,
        } => {
            // `--root` with a store that is already registered is the
            // re-point: the corpus moved, the store did not.
            let registered = home::Registry::load()?
                .name_of(&store_dir)
                .map(str::to_string)
                .or_else(|| {
                    name.as_deref()
                        .filter(|n| home::Registry::load().is_ok_and(|r| r.stores.contains_key(*n)))
                        .map(str::to_string)
                });
            match (registered, &root) {
                (Some(name), Some(root)) => {
                    home::repoint(&name, root)?;
                    eprintln!("{name} now indexes {}", root.display());
                }
                _ => {
                    let (name, target) = home::adopt(&store_dir, root.as_deref(), name.as_deref())?;
                    let store = Semlith::open_existing(&target)?;
                    let (files, chunks, bytes) = store.stats()?;
                    eprintln!(
                        "adopted {} as {name} — {files} files, {chunks} chunks, {} at {}",
                        store_dir.display(),
                        semlith::human_bytes(bytes),
                        target.display(),
                    );
                }
            }
        }

        Command::Hook {
            mode,
            strict,
            client,
        } => {
            // Everything here is best-effort and silent. This runs inside
            // another program's tool call, where stderr is noise in somebody's
            // terminal and a non-zero exit is a client reporting a broken hook.
            let mut input = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin().lock(), &mut input);
            let mode = if strict {
                semlith::hook::Mode::Gate
            } else {
                semlith::hook::Mode::parse(&mode).unwrap_or(semlith::hook::Mode::Soft)
            };
            let answer = semlith::hook::run(&input, mode, &client);
            if !answer.is_empty() {
                println!("{answer}");
            }
        }

        Command::Mcp => {
            // Every registered store, so a client stanza is `semlith mcp` and
            // nothing else.
            let dirs = semlith::home::all_dirs(&cli.store, &cwd)?;

            // A daemon is the writer for every store it opened, so an agent
            // that opened the store itself could not index while a portal was
            // open. Forwarding removes that: the daemon answers, and it is the
            // one process allowed to write.
            if let Some(upstream) = semlith::proxy::find(&semlith::proxy::candidates(&dirs)) {
                eprintln!(
                    "semlith {}: forwarding to the daemon on 127.0.0.1:{} (found via {})",
                    env!("CARGO_PKG_VERSION"),
                    upstream.port,
                    upstream.via.display(),
                );
                return semlith::proxy::serve(
                    &upstream,
                    std::io::stdin().lock(),
                    std::io::stdout().lock(),
                );
            }

            // A machine with no store registered used to end the process
            // here, before a byte of protocol was written: a fresh install
            // wired into a client produced a server that exited on startup,
            // which the client reports as nothing at all. That is the same
            // absence this release exists to remove, so it is served instead —
            // the handshake and the tool list are answered, and the first
            // `tools/call` carries the message that used to be fatal.
            let mut fleet = match read_fleet(&cli.store, &cwd, true) {
                Ok(fleet) => fleet,
                Err(e) => {
                    eprintln!("semlith: {e}");
                    Fleet::empty()
                }
            };
            fleet.quiet = true;

            // Read once, here, while nothing else holds the fleet. `tools/list`
            // names the open stores, and a fleet's membership is fixed by
            // `Fleet::open` and never added to, so this cannot go stale — which
            // is what lets the handshake be answered without taking the lock.
            let labels = fleet.labels().join(", ");
            let stores = fleet.len();

            // stdout is the protocol, so this goes to stderr — which a stdio
            // client captures. A server opened on the wrong store answers
            // every question with nothing, and this is the only place that is
            // visible before a query comes back empty.
            match stores {
                0 => eprintln!(
                    "semlith {}: serving no store — the tools are listed, and the first one that \
                     needs a corpus will say so",
                    env!("CARGO_PKG_VERSION"),
                ),
                1 => eprintln!(
                    "semlith {}: serving 1 store on {labels}",
                    env!("CARGO_PKG_VERSION"),
                ),
                n => eprintln!(
                    "semlith {}: serving {n} stores on {labels}",
                    env!("CARGO_PKG_VERSION"),
                ),
            }
            let fleet = std::sync::Arc::new(std::sync::Mutex::new(fleet));

            // Behind the handshake, not in front of it. This used to be
            // `fleet.warm()` on this thread, before `serve` read its first
            // line: an ONNX session takes a second or two to build and a model
            // that is not cached yet is a download, and a client with a startup
            // timeout saw neither an `initialize` result nor an error — it saw
            // silence, and reported no server. The cost still has to be paid by
            // somebody; paying it here means it overlaps the client's own
            // startup instead of blocking it, and only a `tools/call` waits.
            if stores > 0 {
                let warming = std::sync::Arc::clone(&fleet);
                std::thread::spawn(move || {
                    let started = std::time::Instant::now();
                    let mut fleet = warming.lock().unwrap_or_else(|e| e.into_inner());
                    match fleet.warm() {
                        Ok(()) => eprintln!(
                            "semlith: embedding model ready in {}ms; searches are warm from here",
                            started.elapsed().as_millis(),
                        ),
                        // Not fatal: every tool that does not embed still works,
                        // and a search will fail with this same reason attached.
                        Err(e) => eprintln!("semlith: could not load the embedding model: {e}"),
                    }
                });
                eprintln!(
                    "semlith: loading the embedding model in the background — the first search \
                     waits for it, everything else does not"
                );
            }

            semlith::mcp::serve(
                &fleet,
                &labels,
                std::io::stdin().lock(),
                std::io::stdout().lock(),
            )?;
        }
    }

    Ok(())
}

/// Turn `--airgap` into the environment variable the loader reads.
///
/// One switch rather than a flag threaded through `Semlith`, `Fleet` and the
/// daemon: the check lives at the single place weights are fetched, and this is
/// how the flag reaches it. Called before any thread starts, which is what
/// makes the `set_var` safe.
fn arm_airgap(on: bool) {
    if on {
        unsafe { std::env::set_var(semlith::embed::AIRGAP_ENV, "1") };
    }
}

/// The stores a read-only command opens.
///
/// `all` is what `mcp` asks for: every registered store, because a client
/// stanza cannot know which directory the agent will be started in. Everything
/// else asks about the store covering the working directory.
/// The registered name of the store that covers `path`, for a daemon route.
fn store_of(flags: &[PathBuf], path: &Path) -> Result<Option<String>> {
    let dir = home::resolve(flags, path, None)?.one()?;
    Ok(dir.file_name().map(|n| n.to_string_lossy().into_owned()))
}

/// Accept one refused file, or keep out a dummy-only one, after showing a
/// person what was found and asking for the file's name (2.5).
fn decide_refused(
    flags: &[PathBuf],
    upstream: Option<&semlith::proxy::Upstream>,
    path: &Path,
    mode: &str,
    yes: bool,
) -> Result<()> {
    let key = semlith::canonical(path).to_string_lossy().into_owned();
    let dir = home::resolve(flags, path, None)?.one()?;
    // Read-only: what the store recorded about the file, to show before asking.
    let row = {
        let store = Semlith::open_existing(&dir)?;
        semlith::store::refusal(store.db(), &key)?
    };
    let Some(row) = row else {
        bail!("{} is not on the not-indexed list", path.display());
    };
    if row.class == semlith::store::class::CREDENTIAL {
        bail!(
            "{} is a credential file and is never accepted; `semlith index --include-secrets` \
             is the only way to index one",
            path.display()
        );
    }
    eprintln!("{}{}{} — {}", bold(), display(path), reset(), row.rule);
    for m in &row.matches {
        eprintln!(
            "  line {} · {} {} · {} % likely real ({})",
            m.get("line").and_then(|v| v.as_u64()).unwrap_or(0),
            m.get("kind").and_then(|v| v.as_str()).unwrap_or(""),
            m.get("masked").and_then(|v| v.as_str()).unwrap_or(""),
            m.get("confidence").and_then(|v| v.as_u64()).unwrap_or(0),
            m.get("signals")
                .and_then(|v| v.as_array())
                .map(|s| s
                    .iter()
                    .map(|x| format!(
                        "{} {}",
                        x.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        if x.get("effect").and_then(|v| v.as_str()) == Some("up") {
                            "↑"
                        } else {
                            "↓"
                        }
                    ))
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default()
        );
    }
    match mode {
        "redacted" => eprintln!(
            "Accepting with redaction replaces each value above with [REDACTED:…]. It covers only \
             what the scanner detected."
        ),
        "as-is" => eprintln!("Accepting as-is indexes the file's full text, values included."),
        _ => eprintln!(
            "Refusing keeps this file out of the store although every match is a test dummy."
        ),
    }
    if !yes {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        eprint!("Type {name} to confirm: ");
        let mut typed = String::new();
        std::io::stdin().read_line(&mut typed)?;
        if typed.trim() != name {
            bail!("not confirmed; nothing changed");
        }
    }
    match upstream {
        Some(daemon) => {
            let body = serde_json::json!({
                "path": key,
                "mode": mode,
                "reviewed": true,
                "source": "cli",
                "store": dir.file_name().map(|n| n.to_string_lossy().into_owned()),
            });
            daemon.post("/api/refused/accept", &body)?;
        }
        None => {
            let mut store = Semlith::open(&dir, None)?;
            store.accept(&key, mode, "cli")?;
            if mode != "refused" {
                store.index_paths(&[PathBuf::from(&key)], |_, _| {})?;
            }
        }
    }
    eprintln!(
        "{} {}",
        if mode == "refused" {
            "kept out"
        } else {
            "accepted and indexed"
        },
        path.display()
    );
    Ok(())
}

/// The not-indexed list as text, reviewable rows first.
fn print_refused(listing: &serde_json::Value) {
    let total = listing.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
    let review = listing.get("review").and_then(|v| v.as_u64()).unwrap_or(0);
    println!("{total} not indexed · {review} need review");
    for store in listing
        .get("stores")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let name = store.get("store").and_then(|v| v.as_str()).unwrap_or("");
        let rows = store
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if rows.is_empty() {
            continue;
        }
        println!("\n{}{name}{}", bold(), reset());
        let mut dummies = Vec::new();
        for row in &rows {
            let class = row.get("class").and_then(|v| v.as_str()).unwrap_or("");
            let path = row.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let rule = row.get("rule").and_then(|v| v.as_str()).unwrap_or("");
            if class == "dummy" {
                dummies.push(row.clone());
                continue;
            }
            let confidence = row
                .get("confidence")
                .and_then(|v| v.as_u64())
                .map(|c| format!(" · {c} % likely real"))
                .unwrap_or_default();
            let accepted = row
                .get("accepted")
                .and_then(|v| v.as_str())
                .map(|m| format!(" · accepted {m}"))
                .unwrap_or_default();
            println!("  {class:<11} {path}{confidence}{accepted}");
            println!("              {rule}");
        }
        if !dummies.is_empty() {
            println!("  let through as test dummies:");
            for row in dummies {
                println!(
                    "    {} · {} % likely real",
                    row.get("path").and_then(|v| v.as_str()).unwrap_or(""),
                    row.get("confidence").and_then(|v| v.as_u64()).unwrap_or(0)
                );
            }
        }
    }
    if review > 0 {
        println!(
            "\nsemlith refused accept <path> --redact | --as-is reviews one file; credential files are never accepted."
        );
    }
}

/// The scan phase's plan, as a person reads it.
fn print_plan(plan: &semlith::Plan) {
    let langs: Vec<String> = plan
        .languages
        .iter()
        .map(|(l, n)| format!("{l} {n}"))
        .collect();
    let not: usize = plan.not_indexed.values().sum();
    let eta = plan
        .eta_ms
        .map(|ms| format!(" · about {} to embed", human_duration(ms as f32 / 1000.0)))
        .unwrap_or_default();
    eprintln!(
        "plan: {} to embed ({}{}) · {} unchanged · {not} not indexed · {} need review{eta} · scanned in {:.2}s",
        plan.embed,
        semlith::human_bytes(plan.embed_bytes as i64),
        if langs.is_empty() {
            String::new()
        } else {
            format!(": {}", langs.join(", "))
        },
        plan.unchanged,
        plan.review.len(),
        plan.seconds,
    );
    for path in &plan.credential {
        eprintln!(
            "  credential file, never indexed: {}",
            display(Path::new(path))
        );
    }
    for item in &plan.review {
        eprintln!(
            "  needs review: {} — {}",
            display(Path::new(&item.path)),
            item.rule
        );
        for m in &item.matches {
            eprintln!(
                "      line {} · {} {} · {} % likely real",
                m.line, m.kind, m.masked, m.confidence
            );
        }
    }
}

/// One prompt per reviewable file, before anything is embedded (2.7).
///
/// There is never an accept-all: the one choice that covers the rest keeps
/// them refused and starts the run.
fn review_plan(store: &mut Semlith, plan: &semlith::Plan) -> Result<()> {
    for (i, item) in plan.review.iter().enumerate() {
        let content = item.class == semlith::store::class::CONTENT;
        eprint!(
            "{}({}/{}) {}{} — {} — ",
            bold(),
            i + 1,
            plan.review.len(),
            display(Path::new(&item.path)),
            reset(),
            item.confidence
                .map(|c| format!("{c} % likely real"))
                .unwrap_or_else(|| item.rule.clone())
        );
        eprint!(
            "{}",
            if content {
                "[r]edact / [a]s-is / [k]eep refused / keep the rest refused and [s]tart? "
            } else {
                "[a]ccept / [k]eep refused / keep the rest refused and [s]tart? "
            }
        );
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        let mode = match answer.trim().to_ascii_lowercase().as_str() {
            "r" | "redact" if content => "redacted",
            "a" | "as-is" | "accept" => "as-is",
            "s" | "start" => break,
            _ => continue,
        };
        store.accept(&item.path, mode, "cli")?;
        eprintln!("  accepted ({mode})");
    }
    Ok(())
}

fn read_fleet(flags: &[PathBuf], cwd: &Path, all: bool) -> Result<Fleet> {
    let dirs = if all {
        home::all_dirs(flags, cwd)?
    } else {
        home::read_dirs(flags, cwd)?
    };
    if dirs.is_empty() {
        bail!(
            "no semlith store covers {} and none is registered — \
             run `semlith index` in a directory to make one, \
             or `semlith adopt ./.semlith` to move an existing one into {}",
            cwd.display(),
            semlith::home::stores_root()?.display(),
        );
    }
    Fleet::open(&dirs)
}

/// Paths are stored absolute; show them relative to the cwd when possible,
/// which is both shorter and directly usable as an editor target.
/// How often an index run says where it has got to.
const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Where the run is, how fast it is going, and how much longer it has.
///
/// The estimate is the rate so far applied to the files not yet reached, which
/// is wrong whenever the rest of the corpus does not look like the part already
/// walked — so it is printed as an approximation and never as a countdown. A
/// wrong estimate is still worth far more than a silent hour.
fn predict(p: semlith::IndexProgress, elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs_f32().max(0.001);
    let rate = p.chunks as f32 / secs;
    let left = p.total.saturating_sub(p.scanned);
    let per_file = secs / p.scanned.max(1) as f32;
    format!(
        "{}/{} files, {} chunks, {rate:.0} chunks/s, ~{} left",
        p.scanned,
        p.total,
        p.chunks,
        human_duration(left as f32 * per_file),
    )
}

fn human_duration(secs: f32) -> String {
    if secs >= 3600.0 {
        format!("{:.1}h", secs / 3600.0)
    } else if secs >= 60.0 {
        format!("{:.0}m", secs / 60.0)
    } else {
        format!("{secs:.0}s")
    }
}

/// The store `semlith add` writes to, when nothing covers the current
/// directory.
///
/// `index` is free to create a store named after wherever it is run, because
/// that directory is the thing being indexed. `add` is not: the URL is what is
/// being added, and the shell's current directory has nothing to do with it. So
/// a `New` choice here would silently create a store named after a directory
/// the user never mentioned — and, because the root recorded is the store's own
/// downloads folder, the next `add` from the same place would not find it and
/// would create another one beside it. Running `semlith add` twice from `/tmp`
/// produced stores called `tmp` and `tmp-2`, neither of which anyone asked for.
///
/// With exactly one store in the home, that is plainly the one meant. With
/// several, or none, say so rather than guess.
fn resolve_for_add(
    choice: home::Choice,
    flags: &[PathBuf],
    name: Option<&str>,
) -> Result<home::Choice> {
    // An explicit --store or --name is the user saying which, so it stands.
    if !matches!(choice, home::Choice::New { .. }) || !flags.is_empty() || name.is_some() {
        return Ok(choice);
    }

    let registry = home::Registry::load()?;
    let mut names = registry.stores.keys();
    match (names.next(), names.next()) {
        (Some(only), None) => {
            let only = only.clone();
            Ok(home::Choice::Registered {
                dir: home::Registry::dir_of(&only)?,
                name: only,
            })
        }
        (None, _) => bail!(
            "there is no store to add to yet — run `semlith index <path>` first, \
             or give this one a name with `semlith add <url> --name <store>`"
        ),
        _ => bail!(
            "nothing indexes this directory, and there are {} stores to choose from: {}. \
             Name one with `--store <dir>` or `--name <store>`, or run this from a \
             directory one of them covers",
            registry.stores.len(),
            registry
                .stores
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// What the ledger calls a retrieval made from the command line.
///
/// One session per invocation is the truth of it: the process starts, asks one
/// thing and exits. Pretending otherwise would make a session count that means
/// nothing.
const CLI_LEDGER: semlith::ledger::Who<'static> = semlith::ledger::Who {
    client: "cli",
    session: "cli",
};

/// Exit quietly when whoever was reading our output goes away.
///
/// `semlith files | head` is the most ordinary thing anyone types, and it
/// printed a panic and a non-zero status (#75). Two halves, because the
/// mechanism differs:
///
/// On unix the Rust runtime sets `SIGPIPE` to `SIG_IGN` before `main`, so the
/// write returns `EPIPE` and `println!` panics on it. Restoring the default
/// makes the process end the way `cat` and `grep` do, and a shell reports the
/// pipeline's status, which is the reader's.
///
/// On Windows there is no `SIGPIPE`: the write fails with `BrokenPipe` and
/// reaches the same panic. The hook turns that one panic — and only that one —
/// into a silent exit 0, which is what the reader closing the pipe means.
fn quiet_on_a_closed_pipe() {
    #[cfg(unix)]
    {
        // SAFETY: called once, at the top of `main`, before any thread is
        // spawned and before anything has been printed. `SIG_DFL` is what the
        // process would have had if the Rust runtime had not changed it.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
        }
    }

    let inherited = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let said = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or_default();
        // The message `std` builds for a failed `print!`, with the OS error
        // spelled differently on each platform — matching the kind rather than
        // the wording is not possible from here, because the error is already
        // formatted into the panic payload.
        let printing = said.starts_with("failed printing to std");
        if printing && (said.contains("Broken pipe") || said.contains("pipe")) {
            std::process::exit(0);
        }
        inherited(info);
    }));
}

/// What a forget of a path no store holds says, on stderr, before exiting 1.
///
/// One sentence rather than a suggestion: the path is either a typo or already
/// gone, and semlith cannot tell which.
fn not_indexed(path: &std::path::Path) -> String {
    format!("nothing to forget: {} is not indexed", path.display())
}

fn display(path: &std::path::Path) -> String {
    // Both sides canonicalised, or neither matches on Windows: the file comes
    // back from `canonicalize` in the verbatim form and the working directory
    // does not, so `strip_prefix` never fired and every hit printed absolute —
    // in a form nothing can open (#74).
    let cwd = semlith::canonical(&std::env::current_dir().unwrap_or_default());
    let real = semlith::canonical(path);
    let shown = real.strip_prefix(&cwd).unwrap_or(&real);
    semlith::plain(&shown.display().to_string())
}

fn bold() -> &'static str {
    if std::io::stdout().is_terminal() {
        "\x1b[1m"
    } else {
        ""
    }
}

fn reset() -> &'static str {
    if std::io::stdout().is_terminal() {
        "\x1b[0m"
    } else {
        ""
    }
}

/// The message for a name the graph has never heard of.
///
/// A store with an empty graph and a store that simply does not contain the
/// symbol are different facts, and only one of them means "run index". Saying
/// "not found" for both is how someone concludes the feature is broken.
/// What to say when the graph has nothing to show for a name.
///
/// Only reached when there is genuinely nothing — no caller, no callee, and
/// nothing hidden. A symbol whose every target lies outside the corpus has
/// something to say and used to be reported here as though it did not exist,
/// which is the confusion between "semlith shows no callees" and "everything
/// this calls is outside the index" that the hidden count exists to end.
fn nothing_known(fleet: &semlith::fleet::Fleet, name: &str) -> String {
    let symbols: i64 = fleet
        .each()
        .map(|(_, store)| {
            semlith::store::graph_stats(store.db())
                .map(|(s, _)| s)
                .unwrap_or(0)
        })
        .sum();
    if symbols == 0 {
        format!(
            "no symbols in this store yet. The graph is built as files are indexed, \
             so run `semlith index` once (or leave `semlith start` running) and \
             {name} will be there if the corpus defines it."
        )
    } else {
        format!("no symbol named {name} in the graph")
    }
}

/// `[store] ` when several stores are open, and nothing when one is.
fn store_prefix(store: &Option<String>) -> String {
    match store {
        Some(label) => format!("[{label}] "),
        None => String::new(),
    }
}

fn print_ends(out: &mut impl Write, heading: &str, ends: &[semlith::store::EdgeEnd]) -> Result<()> {
    writeln!(out, "{}{heading}{} ({})", bold(), reset(), ends.len())?;
    if ends.is_empty() {
        writeln!(out, "  none")?;
        return Ok(());
    }
    for end in ends {
        // A collapsed row stands for every definition of the name, so it says
        // how many rather than pointing at whichever one came back first —
        // which would read as a fact about where the call goes.
        if end.confidence == semlith::graph::AMBIGUOUS {
            writeln!(
                out,
                "  {} via {} ({}) · {} definitions{}",
                end.symbol.name,
                end.kind,
                end.confidence,
                end.definitions,
                semlith::graph::call_site(end, &|p| display(std::path::Path::new(p))),
            )?;
            continue;
        }
        writeln!(
            out,
            "  {} via {} ({})  {}{}:{}{}",
            end.symbol.name,
            end.kind,
            end.confidence,
            store_prefix(&end.symbol.store),
            display(std::path::Path::new(&end.symbol.path)),
            end.symbol.start_line,
            semlith::graph::call_site(end, &|p| display(std::path::Path::new(p))),
        )?;
    }
    Ok(())
}

/// A unix second as a local clock time, for the ledger's rows.
fn human_time(at: i64) -> String {
    // Local, with the offset. It used to be UTC arithmetic with a day number,
    // while the portal printed the browser's local clock, and neither said
    // which zone it meant — so the same retrieval read 13:40:32 in one place
    // and 19:00:18 in the other and nothing on either screen admitted it.
    semlith::clock::local_clock(at)
}

/// The four steps, and the first one that failed.
///
/// A file can say semlith is registered while nothing can launch it. That was
/// true on the machine this release was found on, twice, and every check that
/// read a file passed both times. These lines are what happened when something
/// tried.
fn print_proof(proof: &semlith::doctor::Proof) {
    println!();
    println!("{}Proof{}", bold(), reset());
    let service = semlith::service::status();
    println!(
        "  {:<4} {:<20} {}",
        if service.installed { "ok  " } else { "--  " },
        "login service",
        if service.installed {
            match semlith::service::last_started() {
                Some(at) => format!(
                    "installed ({}), daemon last started {}",
                    service.mechanism,
                    human_time(at)
                ),
                None => format!("installed ({}), no daemon running", service.mechanism),
            }
        } else {
            "not installed — `semlith start --service`".to_string()
        },
    );
    // A definition from before 0.28.0 asks for background priority, which a
    // daemon cannot lift itself out of: the service runs, and indexes on the
    // efficiency cores. Named until `setup` or `upgrade` has rewritten it.
    if service.installed {
        match semlith::service::stale_definition() {
            Some(why) => println!("  {:<4} {:<20} {why}", "FAIL", "service priority"),
            None => println!(
                "  {:<4} {:<20} normal while embedding, background while idle",
                "ok  ", "service priority"
            ),
        }
    }
    println!(
        "  {:<4} {:<20} {}",
        "ok  ", "a client would run", proof.command
    );
    println!(
        "  {:<4} {:<20} {}",
        if proof.launched { "ok  " } else { "FAIL" },
        "launched",
        if proof.launched {
            "answered `initialize` with the environment a service manager gives a job"
        } else {
            "did not answer with a plain PATH"
        },
    );
    match proof.tools {
        Some(n) if n > 0 => println!("  {:<4} {:<20} {n} tools", "ok  ", "listed the tools"),
        _ => println!("  {:<4} {:<20} none", "FAIL", "listed the tools"),
    }
    match &proof.client {
        Some(verdict) => println!(
            "  {:<4} {:<20} {}: {}",
            if verdict.connected { "ok  " } else { "FAIL" },
            "client says",
            verdict.name,
            verdict.says,
        ),
        // Named rather than implied. Every other client is read from its
        // configuration file, which is exactly the check that passed twice
        // while there was no server.
        None => println!(
            "  {:<4} {:<20} no client with a command semlith can ask is installed",
            "n/a ", "client says",
        ),
    }
    if let Some(failed) = &proof.failed {
        println!("  first failure: {failed}");
    }
}

/// One line: whether an agent opening a session here would find semlith.
///
/// Short enough for a shell prompt and specific enough to act on. A green line
/// says the version, the stores and whether a login service is keeping the
/// daemon up; a red one names the first client that cannot reach semlith and
/// why, because "something is wrong" in a prompt is worse than nothing.
fn print_brief(clients: &[semlith::doctor::ClientReport], rules: &[semlith::doctor::Finding]) {
    let service = semlith::service::status();
    // A client switched off for this directory outranks one that was never
    // registered: the first is why the session in front of the reader has no
    // semlith, the second is a client they may not even use.
    let broken = clients
        .iter()
        .find(|c| c.disabled_here)
        .or_else(|| clients.iter().find(|c| c.fault));
    if let Some(broken) = broken {
        // The first sentence only. A prompt has one line, and the rest of
        // `explain` is for the full report.
        let why = match broken.explain.as_deref() {
            Some(explain) => explain
                .split_once(". ")
                .map(|(head, _)| head)
                .unwrap_or(explain)
                .to_string(),
            None => "is installed and cannot reach semlith".to_string(),
        };
        // The command, separated from the reason, and without the trailing
        // comment the full report has room for.
        let fix = broken
            .repair
            .as_deref()
            .map(|r| r.split_once("  #").map(|(cmd, _)| cmd).unwrap_or(r).trim())
            .map(|cmd| format!(" — run: {cmd}"))
            .unwrap_or_default();
        println!("semlith: {} {why}{fix}", broken.name);
        return;
    }
    if let Some(failed) = rules.iter().find(|r| !r.ok) {
        println!("semlith: {}", failed.check);
        return;
    }
    let stores = semlith::home::Registry::load()
        .map(|r| r.stores.len())
        .unwrap_or(0);
    println!(
        "semlith {} · {} store{} · {}",
        env!("CARGO_PKG_VERSION"),
        stores,
        if stores == 1 { "" } else { "s" },
        if service.installed {
            format!("{} service", service.mechanism)
        } else {
            "no login service — `semlith start --service`".to_string()
        },
    );
}

/// One row per lane: whether its vectors match the committed fp32 answers.
fn print_gpu_checks(checks: &[serde_json::Value]) {
    println!("{}Accelerators{}", bold(), reset());
    for check in checks {
        let lane = check["lane"].as_str().unwrap_or("?");
        match check.get("reason").and_then(|r| r.as_str()) {
            Some(reason) => println!("  {:<4} {lane:<7} {reason}", "n/a "),
            None => println!(
                "  {:<4} {lane:<7} {} · {} · cosine {:.4} (min over 32) · {} chunks/s",
                if check["passed"].as_bool() == Some(true) {
                    "ok  "
                } else {
                    "FAIL"
                },
                check["device"].as_str().unwrap_or("?"),
                check["variant"].as_str().unwrap_or("?"),
                check["cosine"].as_f64().unwrap_or(0.0),
                check["chunks_per_s"].as_f64().unwrap_or(0.0),
            ),
        }
    }
}

/// What `--service` installed, and where to look when it misbehaves.
fn print_service(status: &semlith::service::Status) {
    if !status.installed {
        println!(
            "semlith: the {} definition was written but {} does not report it as installed",
            status.mechanism, status.mechanism,
        );
        return;
    }
    if status.started_now {
        println!(
            "semlith: installed as a {} login service — running now, and from every login",
            status.mechanism,
        );
    } else {
        // Not a failure, and worth saying plainly: something was already
        // listening, so this registered the service and left the running
        // daemon alone rather than starting a second one to lose the bind.
        println!(
            "semlith: installed as a {} login service — a daemon is already listening, so this              one starts at the next login",
            status.mechanism,
        );
    }
    if let Some(definition) = &status.definition {
        println!("  definition  {}", definition.display());
    }
    if let Some(log) = &status.log {
        println!("  log         {}", log.display());
    }
    // Stated rather than implied. A Windows logon task restarts a task that
    // failed; it does not supervise one that ended cleanly, and a user who
    // reads "login service" on all three platforms would assume it did.
    if !status.restarts {
        println!(
            "  note        this platform restarts a service that fails, \
             but does not restart one that exits cleanly"
        );
    }
    println!("  remove      semlith start --no-service");
}

/// `semlith doctor`'s human output.
///
/// One line per client, then the rules that are readings of this machine. A
/// client that is not installed says so and is not a fault; a client that is
/// installed and unregistered carries the command that fixes it, because the
/// whole point of this command is that the next person reads the answer instead
/// of bisecting a configuration file.
/// What this client has beyond its registration: the skill, the hook and the
/// rule block, in four states each.
///
/// `None` where a client documents none of the three, which is most of them.
/// A row that said "skill: paste, hook: paste, rules: paste" on twenty clients
/// would be three columns of noise hiding the one client where it matters.
fn steering_line(client: &semlith::doctor::ClientReport) -> Option<String> {
    use semlith::agentfiles::State;
    let word = |state: State| match state {
        State::Present => "linked",
        State::Absent => "absent",
        State::Stale => "stale",
        State::Paste => "paste needed",
    };
    let mut parts = Vec::new();
    if client.skill != State::Paste {
        parts.push(format!("skill {}", word(client.skill)));
    }
    if client.hook != State::Paste {
        parts.push(format!(
            "hook {}",
            match client.hook {
                State::Present => "present",
                State::Absent => "absent",
                State::Stale => "stale",
                State::Paste => "paste needed",
            }
        ));
    }
    if client.rules != State::Paste {
        parts.push(format!(
            "rule {}",
            match client.rules {
                State::Present => "present",
                State::Absent => "absent",
                State::Stale => "stale",
                State::Paste => "paste needed",
            }
        ));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// The savings block: the number, and every denominator that makes it
/// defensible.
///
/// Never the number alone. Coverage says what share of retrievals it is
/// computed over and tier says how the tokens were counted, and a figure
/// printed without both is a figure a reader cannot check — which is how the
/// double-counted README paragraph of 0.17.2 came to exist.
fn print_savings(store: &semlith::Semlith, indent: &str) -> Result<()> {
    let savings = semlith::store::ledger_savings(store.db())?;
    let misses = semlith::store::ledger_misses(store.db())?;
    let clients = semlith::store::ledger_clients(store.db())?;

    println!(
        "{indent}saved {} tokens over {} of {} retrievals — coverage {} %, tier {}",
        savings.net,
        savings.credited,
        savings.total,
        savings.coverage(),
        savings.tier(),
    );
    println!(
        "{indent}what reading those files whole would have cost, counted with the store's tokenizer"
    );
    println!(
        "{indent}refunds {} ({}) · zero-hit {} of {}",
        misses.refunds,
        if misses.measured {
            "measured: a steering hook reports the reads semlith never served"
        } else {
            "a floor: no steering hook has reported here, so reads semlith never \
             served are not counted"
        },
        misses.zero_hit,
        savings.total,
    );
    if !clients.is_empty() {
        println!(
            "{indent}clients: {}",
            clients
                .iter()
                .map(|(name, count)| format!("{name} {count}"))
                .collect::<Vec<_>>()
                .join(" · ")
        );
    }
    Ok(())
}

fn print_doctor(
    clients: &[semlith::doctor::ClientReport],
    rules: &[semlith::doctor::Finding],
    applied: &[anyhow::Result<semlith::doctor::Applied>],
) {
    println!("{}Clients{}", bold(), reset());
    for client in clients {
        // "Not installed" is only an answer for a client that has a CLI to be
        // installed. For the ten that semlith reaches by writing a file, the
        // CLI is not the question and saying it is absent would be reporting a
        // fault that is not one.
        let state = match (client.registered, &client.note, &client.command) {
            (_, Some(_), _) => "cannot register".to_string(),
            // Registered and switched off reads as "registered" everywhere
            // else, which is how it stayed invisible: the row was green while
            // the session had no server.
            (true, _, _) if client.disabled_here => {
                format!("registered ({}), OFF HERE", client.scope.unwrap_or("user"))
            }
            // A row whose entry names a command that cannot launch is not
            // "registered" in the only sense that matters. `explain` carried
            // the truth and `fault` carried the exit code while the one word a
            // reader actually scans stayed optimistic.
            (true, _, _) if client.fault => {
                format!(
                    "registered ({}), CANNOT LAUNCH",
                    client.scope.unwrap_or("user")
                )
            }
            (true, _, _) => format!("registered ({})", client.scope.unwrap_or("user")),
            (false, _, None) => "not registered".to_string(),
            (false, _, Some(_)) if !client.present => "not installed".to_string(),
            (false, _, Some(_)) => match client.scope {
                Some("project") => "registered for one project only".to_string(),
                _ => "installed, not registered".to_string(),
            },
        };
        println!("  {:<28} {state}", client.name);
        if let Some(note) = &client.note {
            println!("      {note}");
        }
        if let Some(explain) = &client.explain {
            println!("      {explain}");
        }
        // Named even when it is not this directory, because a user who
        // switched semlith off in one project and forgot is the next person
        // to file "it works everywhere except one repository".
        for dir in client.disabled_in.iter().filter(|_| !client.disabled_here) {
            println!("      switched off for {dir}");
        }
        // Steering, beside registration. A client can be perfectly registered
        // and still have an agent that never calls semlith, which is the whole
        // reason these three exist — so they are read on the same row rather
        // than on a page somebody has to go and find.
        if let Some(steering) = steering_line(client) {
            println!("      {steering}");
        }
        if let Some(repair) = &client.repair {
            println!("      run: {repair}");
        }
    }

    println!();
    println!("{}Rules{}", bold(), reset());
    for rule in rules {
        // Three states, not two. A rule this platform cannot take a reading
        // for is neither green nor red: Windows has no file mode, and a tick
        // there would be one the platform has not earned.
        let mark = match (rule.applicable, rule.ok) {
            (false, _) => "n/a ",
            (true, true) => "ok  ",
            (true, false) => "FAIL",
        };
        println!("  {mark} {:<20} {}", rule.id, rule.check);
        if let Some(manual) = &rule.manual {
            println!("       run: {manual}");
        }
    }

    if !applied.is_empty() {
        println!();
        println!("{}Applied{}", bold(), reset());
        for result in applied {
            match result {
                Ok(a) => println!(
                    "  {a}{}",
                    if a.rechecked_ok {
                        ""
                    } else {
                        "  (the rule still does not hold)"
                    }
                ),
                Err(e) => println!("  refused: {e}"),
            }
        }
    }
}

/// `semlith brief`, in its own frame.
///
/// `main`'s dispatch is one `match` over every subcommand, and every arm's
/// locals share its stack frame. Windows gives the main thread 1 MiB, and this
/// arm's renderer was enough to overflow it -- the process died before it could
/// print the refusal its own test was asserting on, which is how the release
/// found out. An arm that does real work gets its own function.
#[allow(clippy::too_many_arguments)]
fn brief(
    store: &[PathBuf],
    cwd: &std::path::Path,
    question: String,
    budget: i64,
    path: &[String],
    ext: &[String],
    lang: &[String],
    prefer: &str,
    json: bool,
) -> Result<()> {
    let filter = Filter::new(path, ext, lang)?;
    let prefer = semlith::Prefer::parse(prefer)?;
    if budget < 1 {
        anyhow::bail!("--budget is a number of tokens, so it has to be at least 1");
    }

    let mut fleet = read_fleet(store, cwd, false)?;
    fleet.quiet = json;

    let started = Instant::now();
    let brief = semlith::brief::brief(&mut fleet, None, &question, budget, &filter, prefer)?;
    let elapsed = started.elapsed();

    // Built from the answer rather than from the hits, because the
    // answer is what a brief actually hands over -- a span whose text
    // the budget dropped cost its locator and nothing more, and a row
    // that counted the text would overstate what was saved.
    let rendered = serde_json::to_string(&brief)?;
    semlith::ledger::brief(&fleet, &CLI_LEDGER, &question, &brief, &rendered, elapsed);

    if json {
        println!(
            "{rendered_pretty}",
            rendered_pretty = serde_json::to_string_pretty(&brief)?
        );
        return Ok(());
    }

    if brief.spans.is_empty() {
        eprintln!("{}", fleet.no_match_reason(&filter));
        return Ok(());
    }

    let mut out = std::io::stdout().lock();
    for (i, span) in brief.spans.iter().enumerate() {
        let from = match &span.store {
            Some(label) => format!("[{label}] "),
            None => String::new(),
        };
        // The same one-letter list badges `search` prints, because it
        // is the same hit and a reader should not have to learn the
        // notation twice.
        let via: String = span
            .lists
            .iter()
            .map(|l| match *l {
                "vector" => 'v',
                "keyword" => 'f',
                "image" => 'i',
                _ => 'g',
            })
            .collect();
        let what = match (&span.symbol, &span.symbol_kind) {
            (Some(name), Some(kind)) => format!(" {kind} {name}"),
            (Some(name), None) => format!(" {name}"),
            _ => String::new(),
        };
        writeln!(
            out,
            "{}{}. {via:<3} {from}{}:{}-{}{what}{}",
            bold(),
            i + 1,
            display(std::path::Path::new(&span.path)),
            span.start_line,
            span.end_line,
            reset()
        )?;
        match &span.text {
            Some(text) => {
                for line in text.lines() {
                    writeln!(out, "   {line}")?;
                }
            }
            // Said rather than left blank: a span with no text under it
            // looks like a span with no text in it.
            None if span.over_budget => writeln!(out, "   (text left out for the budget)")?,
            None => writeln!(out, "   (text: top span only)")?,
        }
        writeln!(out)?;
    }

    for symbol in &brief.symbols {
        writeln!(out, "{}{} (graph){}", bold(), symbol.name, reset())?;
        for (label, edges) in [("called by", &symbol.callers), ("calls", &symbol.callees)] {
            for edge in edges {
                writeln!(
                    out,
                    "   {label} {} {}:{}",
                    edge.symbol.name,
                    display(std::path::Path::new(&edge.symbol.path)),
                    edge.symbol.start_line
                )?;
            }
        }
        if symbol.hidden > 0 {
            writeln!(out, "   and {} more edges", symbol.hidden)?;
        }
        writeln!(out)?;
    }

    let mut tail = format!(
        "{} tokens of {budget}, counted with {}",
        brief.tokens, brief.counted_with
    );
    if !brief.cut.is_empty() {
        let mut dropped = Vec::new();
        if brief.cut.span_text > 0 {
            dropped.push(format!("{} spans left without text", brief.cut.span_text));
        }
        if brief.cut.symbols > 0 {
            dropped.push(format!("{} symbols' edges", brief.cut.symbols));
        }
        tail.push_str(&format!(" -- dropped: {}", dropped.join(", ")));
    }
    writeln!(out, "{tail}")?;

    Ok(())
}

/// What `semlith schedule` can be asked to do.
///
/// Four verbs and no `run now`: a schedule is the daemon's work, and a command
/// that generated a report here would be `semlith report` wearing a costume —
/// with the difference that it would not be the run the record then describes.
#[derive(clap::Subcommand, Debug)]
enum ScheduleCommand {
    /// List every schedule, with when it last ran and when it next will.
    List,

    /// Add a schedule.
    Add {
        /// Which report: savings, access, change, health or gaps.
        kind: String,

        /// How often, in seconds. `86400` is daily, `604800` weekly.
        ///
        /// A number rather than a word because that is what the record holds:
        /// the portal's three cadence chips are shortcuts over this, and a
        /// cadence they cannot spell is still a cadence.
        #[arg(long)]
        every: u64,

        /// The directory the report file is written into. Absolute.
        #[arg(long)]
        to: std::path::PathBuf,

        /// markdown, csv, json, html or pdf.
        #[arg(long, short, default_value = "markdown")]
        format: String,

        /// Which model's prices the savings report costs tokens at.
        #[arg(long, default_value = "Sonnet 5")]
        model: String,

        /// Narrow to a period: all, day, week, month or quarter.
        #[arg(long)]
        window: Option<String>,

        /// Narrow to these stores, by label. Repeatable.
        #[arg(long)]
        scope: Vec<String>,
    },

    /// Remove a schedule by its id.
    Remove {
        /// The id `semlith schedule list` prints.
        id: String,
    },

    /// Turn a schedule on or off without losing it.
    Set {
        /// The id `semlith schedule list` prints.
        id: String,

        /// `on` or `off`.
        state: String,
    },
}

/// The portal's Schedules card, in a terminal.
///
/// Both surfaces read and write the one file the daemon owns, so there is no
/// second copy of what a schedule is; what differs is only how it is drawn. A
/// daemon asleep on its timer notices a change here within a minute — see
/// `schedule::IDLE` for why that ceiling exists at all.
fn run_schedule(what: ScheduleCommand) -> anyhow::Result<()> {
    use semlith::schedule::{Schedule, Schedules};

    match what {
        ScheduleCommand::List => {
            let file = Schedules::load()?;
            if file.schedules.is_empty() {
                println!("no schedules — `semlith schedule add` writes one");
                return Ok(());
            }
            for (id, s) in &file.schedules {
                let cadence = every_words(s.every_seconds);
                println!(
                    "{id}  {}  {}  every {cadence}  {}",
                    s.kind,
                    s.format,
                    if s.enabled { "on" } else { "off" }
                );
                // Absolute, not shortened against this terminal's working
                // directory. `display` exists to make a hit's path short where
                // the reader is standing; a schedule is run by the daemon from
                // somewhere else entirely, and a destination that happens to be
                // the directory you are in printed as nothing at all.
                println!("      to {}", schedule_path(&s.dir));
                // Last and next together, because a cadence with no outcome
                // beside it cannot say whether it is working.
                match s.last_run {
                    Some(at) => println!("      last {}", semlith::clock::local_stamp(at)),
                    None => println!("      last never"),
                }
                if let Some(at) = s.next_run.filter(|_| s.enabled) {
                    println!("      next {}", semlith::clock::local_stamp(at));
                }
                if let Some(path) = &s.last_path {
                    println!("      wrote {}", schedule_path(path));
                }
                // In full, and not folded into the line above: a destination
                // that has gone away is the failure this feature will actually
                // meet, and a schedule that says `on` beside a folder that
                // never fills is the outcome this line exists to prevent.
                if let Some(why) = &s.last_error {
                    println!("      failed: {why}");
                }
            }
        }

        ScheduleCommand::Add {
            kind,
            every,
            to,
            format,
            model,
            window,
            scope,
        } => {
            // Absolute here rather than in `check`, so a relative path typed at
            // a terminal means what the person typing it meant. The daemon has
            // no working directory of theirs, which is why the record may not
            // hold a relative one.
            let to = if to.is_absolute() {
                to
            } else {
                std::env::current_dir()?.join(to)
            };
            let mut schedule = Schedule::new(&kind, &format, &model, every, &to);
            schedule.window = window;
            schedule.stores = scope;
            // Refused before anything is written, naming what would have
            // worked: half a schedules file is worse than none.
            schedule.check()?;
            let mut file = Schedules::load()?;
            let id = file.add(schedule)?;
            file.save()?;
            println!(
                "{id} added — {kind} as {format} every {} into {}",
                every_words(every),
                schedule_path(&to)
            );
            println!("a running daemon picks it up within a minute; `semlith start` runs them");
        }

        ScheduleCommand::Remove { id } => {
            let mut file = Schedules::load()?;
            if !file.remove(&id) {
                anyhow::bail!("no schedule {id:?} — `semlith schedule list` prints the ids");
            }
            file.save()?;
            println!("{id} removed");
        }

        ScheduleCommand::Set { id, state } => {
            let enabled = match state.as_str() {
                "on" => true,
                "off" => false,
                other => anyhow::bail!("{other:?} is not on or off"),
            };
            let mut file = Schedules::load()?;
            let Some(schedule) = file.schedules.get_mut(&id) else {
                anyhow::bail!("no schedule {id:?} — `semlith schedule list` prints the ids");
            };
            schedule.enabled = enabled;
            file.save()?;
            println!("{id} is {state}");
        }
    }
    Ok(())
}

/// A schedule's own path, written out in full.
///
/// Every other path this command prints goes through `display`, which strips
/// the working directory so a hit reads as `src/lib.rs` rather than as a line
/// of absolute noise. A schedule is the one case where that is wrong twice
/// over: the daemon runs it from a directory that is not this one, and
/// `--to .` came back as an empty string, which is a destination printed as
/// nothing.
fn schedule_path(path: &std::path::Path) -> String {
    semlith::plain(&path.display().to_string())
}

/// `604800` as `7 days`, for a line a person reads.
///
/// The record holds seconds and says so; this is the one place that translates,
/// so nothing else has to carry a table of cadence words.
fn every_words(seconds: u64) -> String {
    const HOUR: u64 = 3600;
    const DAY: u64 = 86400;
    match seconds {
        s if s % DAY == 0 && s >= DAY => plural(s / DAY, "day"),
        s if s % HOUR == 0 && s >= HOUR => plural(s / HOUR, "hour"),
        s if s % 60 == 0 && s >= 60 => plural(s / 60, "minute"),
        s => format!("{s}s"),
    }
}

fn plural(n: u64, unit: &str) -> String {
    if n == 1 {
        format!("1 {unit}")
    } else {
        format!("{n} {unit}s")
    }
}
