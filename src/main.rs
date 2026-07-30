use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use fastembed::TextEmbedding;
use semlith::{Semlith, embed, embed::Model, filter::Filter, fleet::Fleet, store_dirs};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::time::Instant;

/// A local semantic cache for AI agents.
#[derive(Parser)]
#[command(name = "semlith", version, about, long_about = None)]
struct Cli {
    /// Store directory (also settable with SEMLITH_STORE). Repeatable for
    /// `search`, `stats`, `files` and `mcp`, which read every store named;
    /// `index`, `watch` and `forget` write, and take exactly one.
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

        /// Suppress per-file output and download progress.
        #[arg(long, short)]
        quiet: bool,
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

    /// List available embedding models.
    Models,

    /// List the language names `--lang` accepts, and their extensions.
    Languages,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let dirs = store_dirs(&cli.store);
    // Every command that writes uses this; the read commands open all of them.
    let dir = dirs[0].clone();

    // Reading several stores is a merge; writing several would be several
    // locks with several failure modes, against a store whose rule is one
    // writer. Refuse it here rather than half-way through the second store.
    if dirs.len() > 1
        && matches!(
            cli.command,
            Command::Index { .. } | Command::Watch { .. } | Command::Forget { .. }
        )
    {
        bail!(
            "this command writes, so it takes one store, not {}: {}",
            dirs.len(),
            dirs.iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    match cli.command {
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
            quiet,
        } => {
            let model = model
                .map(|m| m.parse::<Model>().map_err(anyhow::Error::msg))
                .transpose()?;
            let mut store = Semlith::open(&dir, model)?;
            store.quiet = quiet;

            let roots = if paths.is_empty() {
                vec![PathBuf::from(".")]
            } else {
                paths
            };

            let started = Instant::now();
            let report = store.index_paths(&roots, |path| {
                if !quiet {
                    eprintln!("  + {}", display(path));
                }
            })?;

            let (files, chunks, bytes) = store.stats()?;
            eprintln!(
                "indexed {} files ({} chunks) in {:.1}s — {} unchanged, {} skipped, {} removed",
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

            let mut store = Semlith::open(&dir, None)?;
            store.quiet = quiet;

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

            let mut fleet = Fleet::open(&dirs)?;
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
            let fleet = Fleet::open(&dirs)?;
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
            let fleet = Fleet::open(&dirs)?;
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
            let mut store = Semlith::open(&dir, None)?;
            let n = store.forget(&path)?;
            eprintln!("removed {n} chunks for {}", path.display());
        }

        Command::Mcp => {
            let mut fleet = Fleet::open(&dirs)?;
            // Load each distinct model before the first tool call so an agent
            // does not sit through a cold start mid-conversation.
            fleet.quiet = true;
            fleet.warm()?;
            semlith::mcp::serve(
                &mut fleet,
                std::io::stdin().lock(),
                std::io::stdout().lock(),
            )?;
        }
    }

    Ok(())
}

/// Paths are stored absolute; show them relative to the cwd when possible,
/// which is both shorter and directly usable as an editor target.
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
