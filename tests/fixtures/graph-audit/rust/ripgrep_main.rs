// Cut down from ripgrep crates/core/main.rs, the input of the 2026-10-14
// audit (G1, G3). Indexed by the graph tests as `crates/core/main.rs`, so the
// file's module is called `main`, as the real one is.
use std::process::ExitCode;

use crate::flags::{HiArgs, SearchMode};

mod index;
mod search;

/// Then, as it was, then again it will be.
fn main() -> ExitCode {
    match run(flags::parse()) {
        Ok(code) => code,
        Err(_) => ExitCode::from(2),
    }
}

/// The main entry point for ripgrep.
fn run(result: crate::flags::ParseResult<HiArgs>) -> anyhow::Result<ExitCode> {
    let args = result.ok();
    let matched = match args.mode() {
        Mode::Search(mode) if args.index() > 0 => index::read(&args, mode)?,
        Mode::Search(mode) if args.threads() == 1 => search(&args, mode)?,
        Mode::Search(mode) => search_parallel(&args, mode)?,
        Mode::Files => files_parallel(&args)?,
    };
    Ok(ExitCode::from(matched as u8))
}

/// The top-level entry point for single-threaded search.
fn search(args: &HiArgs, mode: SearchMode) -> anyhow::Result<bool> {
    Ok(args.quiet())
}

/// The top-level entry point for multi-threaded search.
fn search_parallel(args: &HiArgs, mode: SearchMode) -> anyhow::Result<bool> {
    let stats = args.stats().map(std::sync::Mutex::new);
    let mut searcher = args.search_worker(args.matcher()?, args.searcher()?)?;
    args.walk_builder()?.build_parallel().run(|| {
        let stats = &stats;
        let mut searcher = searcher.clone();

        Box::new(move |result| {
            searcher.printer().get_mut().clear();
            let search_result = searcher.search(&result);
            if let Some(ref locked_stats) = *stats {
                let mut stats = locked_stats.lock().unwrap();
                *stats += search_result.stats().unwrap();
            }
            WalkState::Continue
        })
    });
    Ok(true)
}

/// The top-level entry point for file listing without searching.
fn files_parallel(args: &HiArgs) -> anyhow::Result<bool> {
    let (tx, rx) = mpsc::channel::<crate::haystack::Haystack>();
    args.walk_builder()?.build_parallel().run(|| {
        let tx = tx.clone();
        Box::new(move |result| {
            let _ = tx.send(result);
            WalkState::Continue
        })
    });
    Ok(true)
}
