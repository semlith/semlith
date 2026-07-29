use anyhow::Result;
use clap::{Parser, Subcommand};
use fastembed::TextEmbedding;
use semlith::{Semlith, default_store_dir, embed, embed::Model, filter::Filter};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::time::Instant;

/// A local semantic cache for AI agents.
#[derive(Parser)]
#[command(name = "semlith", version, about, long_about = None)]
struct Cli {
    /// Store directory (also settable with SEMLITH_STORE).
    #[arg(long, short, global = true)]
    store: Option<PathBuf>,

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
    let dir = cli.store.clone().unwrap_or_else(default_store_dir);

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

        Command::Search {
            query,
            k,
            path,
            ext,
            lang,
            json,
        } => {
            // Built before the store is opened, so an unknown language name
            // fails immediately rather than after a model load.
            let filter = Filter::new(&path, &ext, &lang)?;

            let mut store = Semlith::open(&dir, None)?;
            store.quiet = json;

            // A glob that selects nothing is a different answer from a corpus
            // that does not discuss the query, and only one of them is the
            // user's typo.
            let selected = (!filter.is_empty())
                .then(|| store.matching_files(&filter))
                .transpose()?;
            if selected == Some(0) {
                if json {
                    println!("[]");
                } else {
                    eprintln!(
                        "no files match the filter (store has {} chunks)",
                        store.len()
                    );
                }
                return Ok(());
            }

            let started = Instant::now();
            let hits = store.search_filtered(&query, k, &filter)?;
            let elapsed = started.elapsed();

            if json {
                println!("{}", serde_json::to_string_pretty(&hits)?);
            } else if hits.is_empty() {
                eprintln!("no matches (store has {} chunks)", store.len());
            } else {
                let mut out = std::io::stdout().lock();
                for (i, h) in hits.iter().enumerate() {
                    writeln!(
                        out,
                        "{}{}. {:.3}  {}:{}-{}{}",
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
                match selected {
                    Some(n) => eprintln!(
                        "{} hits in {:?} (filter selected {n} of {} files)",
                        hits.len(),
                        elapsed,
                        store.stats()?.0
                    ),
                    None => eprintln!("{} hits in {:?}", hits.len(), elapsed),
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
            let store = Semlith::open(&dir, None)?;
            let (files, chunks, bytes) = store.stats()?;
            println!("store    {}", dir.display());
            println!("model    {} ({} dim)", store.model(), store.dim());
            println!("files    {files}");
            println!("chunks   {chunks}");
            println!("vectors  {}", store.len());
            println!("indexed  {}", semlith::human_bytes(bytes));
        }

        Command::Files => {
            let store = Semlith::open(&dir, None)?;
            for path in semlith::store::all_paths(store.db())? {
                println!("{}", display(std::path::Path::new(&path)));
            }
        }

        Command::Forget { path } => {
            let mut store = Semlith::open(&dir, None)?;
            let n = store.forget(&path)?;
            eprintln!("removed {n} chunks for {}", path.display());
        }

        Command::Mcp => {
            let mut store = Semlith::open(&dir, None)?;
            // Load the model before the first tool call so an agent does not
            // sit through a cold start mid-conversation.
            store.quiet = true;
            store.warm()?;
            semlith::mcp::serve(
                &mut store,
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
