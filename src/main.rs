use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use fastembed::TextEmbedding;
use semlith::home;
use semlith::{Semlith, embed, embed::Model, filter::Filter, fleet::Fleet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

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
    },

    /// Run the daemon: hold every registered store's write lock, keep them
    /// current as files are saved, and serve the portal on 127.0.0.1. Runs
    /// until interrupted.
    Start {
        /// Stores to open. Defaults to every registered store.
        paths: Vec<PathBuf>,

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
        /// pattern matches anywhere in the tree.
        #[arg(long, short)]
        path: Vec<String>,

        /// Only search files with this extension. Repeatable.
        #[arg(long, short)]
        ext: Vec<String>,

        /// Only search files of this language. Repeatable; see `semlith languages`.
        #[arg(long, short)]
        lang: Vec<String>,

        /// Emit JSON instead of formatted text.
        #[arg(long)]
        json: bool,
    },

    /// Show what the store contains.
    Stats,

    /// List the files currently indexed.
    Files,

    /// Remove a file from the store.
    Forget { path: PathBuf },

    /// Run as an MCP server over stdio, for agents to call as a tool.
    Mcp,

    /// Put semlith on PATH, pre-fetch the embedding model and register it
    /// with the agents you use. Every step is idempotent, so this is also the
    /// repair command.
    Setup {
        /// Take the default at every prompt — add to PATH, download the model,
        /// register no agent — so a script or an agent can run it unattended.
        #[arg(long, short)]
        yes: bool,

        /// Refuse to download model weights. The other steps still run.
        #[arg(long)]
        airgap: bool,
    },

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    match cli.command {
        Command::Setup { yes, airgap } => {
            arm_airgap(airgap);
            semlith::setup::run(yes, airgap)?;
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
        } => {
            arm_airgap(airgap);
            let model = model
                .map(|m| m.parse::<Model>().map_err(anyhow::Error::msg))
                .transpose()?;

            let roots = if paths.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                paths
            };

            // The first path is what the store is about, so it is what names
            // the store and what the registry records as its root. `semlith
            // index ~/work/api` from anywhere means the api store, not a store
            // named after wherever the shell happened to be.
            let choice = home::resolve(&cli.store, &roots[0], name.as_deref())?;
            if let Some(hint) = choice.hint() {
                eprintln!("{hint}");
            }
            let dir = choice.one()?;
            let mut store = Semlith::open(&dir, model)?;
            store.quiet = quiet;

            let started = Instant::now();
            // Throttled, not per file: a corpus large enough to need an
            // estimate is one where a line per file is the noise the estimate
            // is trying to cut through.
            let mut spoke = Instant::now();
            let report = store.index_paths(&roots, |path, p| {
                if quiet {
                    return;
                }
                eprintln!("  + {}", display(path));
                if spoke.elapsed() >= PROGRESS_INTERVAL {
                    spoke = Instant::now();
                    eprintln!("    {}", predict(p, started.elapsed()));
                }
            })?;

            // Recorded after the run, not before it: a registry entry for a
            // store that failed to index is a store the daemon opens and the
            // portal lists with nothing in it.
            let model_name = store.model().to_string();
            home::record(&choice, &roots, &model_name)?;

            let (files, chunks, bytes) = store.stats()?;
            eprintln!(
                "indexed {} files ({} chunks) in {:.1}s — {} already indexed, {} skipped, {} removed",
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

        Command::Search {
            query,
            k,
            path,
            ext,
            lang,
            json,
        } => {
            // Built before any store is opened, so an unknown language name
            // fails immediately rather than after a model load.
            let filter = Filter::new(&path, &ext, &lang)?;

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
                    eprintln!(
                        "no files match the filter (store has {} chunks)",
                        fleet.chunks()
                    );
                }
                return Ok(());
            }

            let started = Instant::now();
            let hits = fleet.search_filtered(&query, k, &filter)?;
            let elapsed = started.elapsed();

            if json {
                println!("{}", serde_json::to_string_pretty(&hits)?);
            } else if hits.is_empty() {
                eprintln!("no matches (store has {} chunks)", fleet.chunks());
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
                    writeln!(
                        out,
                        "{}{}. {:.3}  {from}{}:{}-{}{}",
                        bold(),
                        i + 1,
                        h.score,
                        display(std::path::Path::new(&h.path)),
                        h.start_line,
                        h.end_line,
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

        Command::Languages => {
            for (name, exts) in semlith::filter::LANGUAGES {
                println!(
                    "{name:<12} {}",
                    exts.iter()
                        .map(|e| format!(".{e}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
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
                println!("model    {} ({} dim)", store.model(), store.dim());
                println!("files    {files}");
                println!("chunks   {chunks}");
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

        Command::Files => {
            let fleet = read_fleet(&cli.store, &cwd, false)?;
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

        Command::Forget { path } => {
            let choice = home::resolve(&cli.store, &cwd, None)?;
            if let Some(hint) = choice.hint() {
                eprintln!("{hint}");
            }
            let mut store = Semlith::open(&choice.one()?, None)?;
            let n = store.forget(&path)?;
            eprintln!("removed {n} chunks for {}", path.display());
        }

        Command::Start {
            paths,
            port,
            debounce,
            airgap,
        } => {
            arm_airgap(airgap);
            let dirs = semlith::daemon::stores_to_open(&cli.store, &paths, &cwd)?;
            if dirs.is_empty() {
                // Not an error: the portal's welcome screen exists for exactly
                // this, and telling someone to go index something first is what
                // the screen does better than a bail! does.
                eprintln!("semlith: no store registered yet — the portal will offer to make one");
            }
            semlith::daemon::run(
                &dirs,
                semlith::daemon::port_of(port),
                std::time::Duration::from_millis(debounce),
                semlith::embed::airgap(),
                |line| eprintln!("semlith: {line}"),
            )?;
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

            let mut fleet = read_fleet(&cli.store, &cwd, true)?;
            // Load each distinct model before the first tool call so an agent
            // does not sit through a cold start mid-conversation.
            fleet.quiet = true;
            fleet.warm()?;
            // stdout is the protocol, so this goes to stderr — which a stdio
            // client captures. A server opened on the wrong store answers
            // every question with nothing, and this is the only place that is
            // visible before a query comes back empty.
            eprintln!(
                "semlith {}: serving {} on {}",
                env!("CARGO_PKG_VERSION"),
                match fleet.len() {
                    1 => "1 store".to_string(),
                    n => format!("{n} stores"),
                },
                fleet.labels().join(", "),
            );
            semlith::mcp::serve(
                &mut fleet,
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
            semlith::home::stores_root().display(),
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

fn display(path: &std::path::Path) -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    path.strip_prefix(&cwd)
        .unwrap_or(path)
        .display()
        .to_string()
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
