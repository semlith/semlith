//! The retrieval harness: every claim this release makes about retrieval,
//! measured rather than asserted.
//!
//! ```sh
//! cargo test --test retrieval -- --ignored --nocapture
//! ```
//!
//! It indexes the repository into a temporary store, runs the fixed question
//! set in `tests/fixtures/retrieval/questions.yaml`, and prints hit@1, hit@3,
//! hit@8, bytes per answer, the graph list's marginal contribution, and the
//! wrong-yes count for `path`.
//!
//! Two of those are gates rather than readings. `wrong yes` must be zero: a
//! path answer that says "connected, here is the chain" about two symbols that
//! are not connected is worse than the tool not existing, because it is
//! indistinguishable from a right answer. And `tools/list` must fit in a
//! thousand tokens, because every agent pays it once per session before it has
//! asked anything.
//!
//! The question set was written and committed before the work it measures, and
//! its header says why. Nothing here may add a question, loosen a span, or
//! skip a shape to make a number look better.

use semlith::Semlith;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One question, as the harness needs it.
#[derive(Debug, Default, Clone)]
struct Question {
    id: String,
    shape: String,
    tool: String,
    query: String,
    name: String,
    from: String,
    to: String,
    connected: Option<bool>,
    hops: Option<usize>,
    k: Option<usize>,
    /// For `tool: read`: `path:start-end` or a symbol name.
    target: String,
    /// For `tool: search`: the preference an agent would pass.
    prefer: String,
    /// For `tool: neighbors`: which half of the ring is scored.
    want: String,
    /// For `tool: neighbors`: edge kinds to restrict to. Empty means all.
    kinds: Vec<String>,
    spans: Vec<Span>,
}

#[derive(Debug, Default, Clone)]
struct Span {
    path: String,
    symbol: String,
    start_line: u32,
    end_line: u32,
}

#[test]
#[ignore = "indexes the repository and downloads an embedding model on first run"]
fn the_retrieval_metrics_are_measured_and_the_gates_hold() {
    // SAFETY: this file holds one test, so nothing else in this process is
    // reading the environment while this writes it, and it runs before any
    // thread is spawned and before a model is loaded.
    //
    // One embedding thread, deliberately, and this is half of issue #88. ONNX
    // Runtime reduces across its intra-op threads in whatever order they
    // finish, so the same text embedded twice on several threads differs in
    // the last bits — and int8 quantization turns a last-bit difference into a
    // rank flip between two near-equal chunks. That is why three runs of one
    // binary over one corpus reported three different hit@k. `tests/measure.rs`
    // has pinned this since the release that found it; this harness, which is
    // the measuring stick for every retrieval claim the repository makes, did
    // not.
    unsafe { std::env::set_var(semlith::embed::THREADS_ENV, "1") };
    assert_eq!(
        semlith::embed::embed_threads(),
        1,
        "the thread pin did not take: this harness would measure ONNX Runtime's \
         reduction jitter as well as the ranking, which is issue #88"
    );

    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/retrieval");
    let all = read_questions(&fixtures.join("questions.yaml"));
    assert!(
        all.len() >= 100,
        "the question set has shrunk to {} questions; it is the measuring stick and \
         may not be trimmed to suit a result",
        all.len()
    );

    // Which half of the set this run is allowed to see.
    //
    // A number tuned against is not a measurement. The set is split by a seed
    // recorded in `split.yaml` before the first ranking change of 0.22.0, and
    // the sealed thirty are scored once, at completion, by the binary the
    // release ships. Development work sees the other seventy and nothing else,
    // so the figure the record states cannot have been fitted to.
    let split = read_split(&fixtures.join("split.yaml"));
    let sealed = std::env::var_os(SEAL_ENV).is_some();
    println!("\n  {} set", if sealed { "sealed" } else { "development" });
    let questions: Vec<Question> = all
        .iter()
        .filter(|q| split.sealed.contains(&q.id) == sealed)
        .cloned()
        .collect();
    if sealed {
        assert_eq!(
            questions.len(),
            split.sealed.len(),
            "{} of the {} sealed ids are not in the question set; an id that was renamed \
             takes its question out of the gate silently",
            split.sealed.len() - questions.len(),
            split.sealed.len()
        );
    }
    assert!(
        !questions.is_empty(),
        "the split left this run no questions to score"
    );
    println!(
        "  {} of {} questions, seed {}",
        questions.len(),
        all.len(),
        split.seed
    );

    // The 0.21.0 figures on this same snapshot, so every print below is a pair
    // rather than a number. Absent until the baseline task has measured it.
    let baseline = read_baseline(&fixtures.join("baseline.yaml"));

    // The corpus is a pinned snapshot checked in beside the question set, and
    // that is the other half of #88.
    //
    // Until 0.22.0 this copied `src`, `tests`, `docs` and `AGENTS.md` out of
    // whatever working tree the harness was compiled in. The copy removed one
    // source of drift — indexing the repository root indexed `target/`, `.git/`
    // and every scratch file beside them — but not the larger one: the content
    // still moved with every commit, so a before/after pair taken across two
    // commits compared two codebases and called the difference a ranking
    // change. `corpus.yaml` says what the snapshot is and when it was taken.
    //
    // Canonical, because the scoring compares `root.join(span.path)` against
    // the path the store recorded, and the store canonicalizes what it indexes.
    // On macOS a temporary directory is `/var/folders/...` and its canonical
    // form is `/private/var/folders/...`, so without this every span comparison
    // fails and the harness reports 2/47 at every depth — which is what it did.
    let manifest = read_manifest(&fixtures.join("corpus.yaml"));
    let root = fixtures
        .join("corpus")
        .canonicalize()
        .expect("the pinned corpus is checked in at tests/fixtures/retrieval/corpus");
    let (files, bytes) = weigh(&root);
    assert_eq!(
        (files, bytes),
        (manifest.files, manifest.bytes),
        "the pinned corpus has been edited: corpus.yaml records {} files and {} bytes at commit {}, \
         and the tree on disk holds {files} files and {bytes} bytes. Every figure this harness \
         prints is a reading of this corpus as much as of the ranking, so an edited corpus is a \
         silently different measurement.",
        manifest.files,
        manifest.bytes,
        manifest.commit
    );
    println!(
        "\n  corpus  {} files, {} bytes, commit {} ({})",
        manifest.files, manifest.bytes, manifest.commit, manifest.tag
    );

    let runs = std::env::var("SEMLITH_RETRIEVAL_RUNS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(3);
    assert!(runs > 0, "a run count of zero measures nothing");

    // Three runs, each its own index.
    //
    // Scoring the same store three times is not three runs: every query-time
    // source of drift is asserted away below, so three scorings of one store
    // are three identical reports and their spread is a decoration. The drift
    // #88 records lived at index time — ONNX Runtime reducing across its
    // intra-op threads in completion order, turning a last-bit difference into
    // a rank flip between two near-equal chunks. A run is therefore an index
    // and a scoring, and the spread across three of them is the noise band
    // every figure in this release is read against.
    let mut reports: Vec<Report> = Vec::new();
    for run in 0..runs {
        let report = index_and_score(&root, &questions, run == 0);
        println!(
            "  run {}/{}   hit@1 {}, hit@3 {}, hit@8 {} of {}",
            run + 1,
            runs,
            report.hit_at.get(&1).copied().unwrap_or(0),
            report.hit_at.get(&3).copied().unwrap_or(0),
            report.hit_at.get(&8).copied().unwrap_or(0),
            report.scored
        );
        reports.push(report);
    }

    let tool_list = semlith::mcp::tool_list_bytes();
    let tool_tokens = tool_list.div_ceil(4);
    let summary = Summary::of(reports);
    summary.print(&questions, tool_list, tool_tokens, baseline.as_ref());

    assert!(
        summary.census_share() > 50,
        "only {}% of call edges whose target this corpus holds resolve to one \
         definition; the resolver is the release's central change and this is the \
         number that says whether it works",
        summary.census_share()
    );
    assert_eq!(
        summary.median(|r| r.wrong_yes),
        0,
        "a path question returned a chain between symbols that are not connected. \
         This is the one metric with a hard gate."
    );
    assert!(
        tool_tokens < 1_000,
        "tools/list is {tool_list} bytes, about {tool_tokens} tokens, and every agent \
         pays it once per session"
    );

    // The identifier promise, asserted on the development set because that is
    // where the contract puts it: an agent that already knows a term must never
    // have to fall back to grep for it.
    if !sealed {
        let stragglers = summary.identifier_stragglers(&questions, 3);
        assert!(
            stragglers.is_empty(),
            "{} identifier question(s) have no satisfying span in the top three: {}. \
             The contract's gate is 100 % of them.",
            stragglers.len(),
            stragglers.join(", ")
        );
    }

    // The gate itself, scored once, on the sealed thirty, by the binary the
    // release ships. Below it there is no release.
    if sealed {
        // Every depth, then one failure carrying all of them.
        //
        // Asserting depth by depth stops at the first one that falls short and
        // says nothing about the rest, so a reader learns hit@8 missed and has
        // to run it again to find out about hit@3 and hit@1. The release record
        // states the whole distance, and so does this.
        let scored = summary.scored();
        let mut against_the_gate: Vec<String> = Vec::new();
        for (k, floor) in [(8usize, 95usize), (3, 85), (1, 70)] {
            let hits = summary.median(|r| r.hit_at.get(&k).copied().unwrap_or(0));
            let percent = hits * 100 / scored.max(1);
            let needed = scored * floor / 100 + usize::from(!(scored * floor).is_multiple_of(100));
            if percent < floor {
                against_the_gate.push(format!(
                    "hit@{k} is {hits} of {scored} ({percent}%), the gate is {floor}% \
                     ({needed} of {scored}), short by {}",
                    needed.saturating_sub(hits)
                ));
            }
        }
        assert!(
            against_the_gate.is_empty(),
            "the sealed set does not meet the gate:\n    {}",
            against_the_gate.join("\n    ")
        );
        let stragglers = summary.identifier_stragglers(&questions, 3);
        assert!(
            stragglers.is_empty(),
            "sealed identifier questions outside the top three: {}",
            stragglers.join(", ")
        );
        if let Some(baseline) = &baseline {
            let bytes = summary.median_bytes();
            assert!(
                bytes <= baseline.bytes,
                "bytes per answer rose from the 0.21.0 baseline's {} to {bytes}; the contract \
                 requires no worse",
                baseline.bytes
            );
        }
    }
}

/// One run: a fresh store, a full index of the pinned corpus, and one scoring
/// of every question.
///
/// `check_determinism` scores twice and compares, which is issue #88's
/// query-time half. It runs on the first run only, because a property that
/// holds is a property that holds — and the index-time half is what the three
/// runs above are for.
fn index_and_score(root: &Path, questions: &[Question], check_determinism: bool) -> Report {
    let store = tempfile::tempdir().expect("a temporary store");
    let mut semlith = Semlith::open(store.path(), None).expect("the store opens");
    semlith.quiet = true;
    // The whole corpus, credential-shaped strings included. 0.19.0's content
    // scan refuses a file that holds one, and this repository holds five of
    // them on purpose — the pattern table's own examples, the fixtures that
    // prove each pattern matches, and the documentation page that explains
    // why a quoted example is refused like any other match. Two of the
    // question set's expected spans are in those files, so a harness that
    // indexed without this flag would report a ranking change that is really
    // a corpus change, and would keep reporting it for ever.
    semlith.boundary = semlith::Boundary {
        roots: None,
        allow_secrets: true,
    };
    // Timed, because the chunking rule is allowed to change the number of
    // chunks and is not allowed to halve the indexing rate — and one corpus on
    // one machine measured by the same harness is the only way that comparison
    // means anything.
    let started = std::time::Instant::now();
    semlith
        .index_paths(std::slice::from_ref(&root.to_path_buf()), |_, _| {})
        .expect("the pinned corpus indexes");
    let indexed_in = started.elapsed();

    let score_all = |semlith: &mut Semlith| {
        let mut report = Report::default();
        for question in questions {
            match question.tool.as_str() {
                "search" => score_search(semlith, root, question, &mut report),
                "path" => score_path(semlith, question, &mut report),
                "read" => score_read(semlith, root, question, &mut report),
                "symbol" => score_symbol(semlith, root, question, &mut report),
                "neighbors" => score_neighbors(semlith, root, question, &mut report),
                other => panic!("{}: unknown tool {other:?}", question.id),
            }
        }
        report
    };

    let mut report = score_all(&mut semlith);
    // What the corpus became. The chunking rule changes how many chunks 146
    // files are, which changes the indexing cost and the precision of every
    // hit, so the number belongs beside the hit@k rather than in a separate
    // measurement nobody runs.
    let (_, chunks, _) = semlith.stats().expect("the store counts its chunks");
    report.chunks = chunks as usize;
    report.index_seconds = indexed_in.as_secs() as usize;

    if check_determinism {
        // The harness asserts its own determinism — issue #88, where three runs
        // of one binary over one corpus gave three different hit@k and the
        // graph-only denominator moved between them.
        //
        // The question set is scored twice against the same store, and the two
        // reports must be identical question by question. That covers every
        // query-time source of the drift, which is where it lived:
        // `graph::expand` walked its frontier through a `HashMap`, whose
        // iteration order is seeded randomly per process, so the `f32` masses
        // summed differently, the confidence a name was labelled with depended
        // on arrival order, and `MAX_NODES` truncated whichever names the order
        // had not reached yet.
        let again = score_all(&mut semlith);
        assert_eq!(
            report.ranks, again.ranks,
            "the harness does not reproduce its own per-question ranks within one run"
        );
        assert_eq!(
            report.hit_at, again.hit_at,
            "the harness does not reproduce its own hit@k within one run"
        );
        assert_eq!(
            (report.graph_only, report.graph_only_hits, report.wrong_yes),
            (again.graph_only, again.graph_only_hits, again.wrong_yes),
            "the harness does not reproduce its own graph-only denominator within one run"
        );
        assert_eq!(
            report.bytes, again.bytes,
            "the harness does not reproduce its own bytes per answer within one run"
        );
    }

    report.census = Some(resolution_census(&semlith));
    report
}

#[derive(Default)]
struct Report {
    scored: usize,
    hit_at: BTreeMap<usize, usize>,
    bytes: Vec<usize>,
    graph_only: usize,
    graph_only_hits: usize,
    wrong_yes: usize,
    /// How many lines each `read` answer came back as. The locate-then-read
    /// claim is about size as much as about correctness.
    read_lines: Vec<usize>,
    /// Search questions that named a preference, and how many of those landed
    /// a span — the number that says whether `prefer` earns its argument.
    preferred: usize,
    preferred_hits: usize,
    chains_found: usize,
    chains_expected: usize,
    chain_length_matches: usize,
    misses: Vec<String>,
    /// Every scored question and the rank its first satisfying hit landed at,
    /// in question order.
    ///
    /// The aggregate hit@k numbers cannot say whether a release helped: adding
    /// ten questions moves every percentage whether or not anything got
    /// better. This table is what one run compares against another, question
    /// by question, and it is why the ids in the question set may never be
    /// reused.
    ranks: Vec<(String, Option<usize>)>,
    /// What share of this run's call edges settled. One census per run, because
    /// the extraction is part of what a run measures.
    census: Option<Census>,
    /// How many chunks the corpus indexed to.
    chunks: usize,
    /// How long the index pass took, in whole seconds.
    index_seconds: usize,
}

/// The environment variable that unseals the sealed thirty.
///
/// A flag rather than a default, because the sealed set is scored once, at
/// completion, and a run that scores it by accident has spent the one
/// measurement the release's outcome rests on.
const SEAL_ENV: &str = "SEMLITH_RETRIEVAL_SEALED";

/// The recorded split: which ids are sealed, and the seed that chose them.
struct Split {
    seed: u64,
    sealed: Vec<String>,
}

fn read_split(path: &Path) -> Split {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("the split is missing at {}: {e}", path.display()));
    let mut seed = None;
    let mut sealed = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        let body = line.trim_start();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        if let Some(rest) = body.strip_prefix("seed: ") {
            seed = Some(rest.trim().parse().expect("the seed is a number"));
        } else if let Some(rest) = body.strip_prefix("- ") {
            sealed.push(rest.trim().to_string());
        }
    }
    let split = Split {
        seed: seed.expect("split.yaml records no seed"),
        sealed,
    };
    assert!(
        !split.sealed.is_empty(),
        "split.yaml seals no questions, which would make the gate a reading of the \
         set the work was tuned against"
    );
    split
}

/// The 0.21.0 figures on this snapshot, if they have been measured yet.
struct Baseline {
    hit_at: BTreeMap<usize, usize>,
    scored: usize,
    bytes: usize,
}

fn read_baseline(path: &Path) -> Option<Baseline> {
    let text = std::fs::read_to_string(path).ok()?;
    let field = |key: &str| -> Option<usize> {
        text.lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(": "))?
            .trim()
            .parse()
            .ok()
    };
    let mut hit_at = BTreeMap::new();
    for k in [1usize, 3, 8] {
        hit_at.insert(k, field(&format!("hit_at_{k}"))?);
    }
    Some(Baseline {
        hit_at,
        scored: field("scored")?,
        bytes: field("bytes")?,
    })
}

/// The runs, and every figure read across them.
///
/// The median is the unit of measurement everywhere in this release, and the
/// spread beside it is what a one-question movement has to clear before it is
/// a result rather than issue #88.
struct Summary {
    runs: Vec<Report>,
}

impl Summary {
    fn of(runs: Vec<Report>) -> Self {
        assert!(!runs.is_empty(), "no runs to summarise");
        Self { runs }
    }

    fn scored(&self) -> usize {
        self.runs[0].scored
    }

    /// The median of one figure across the runs.
    fn median(&self, of: impl Fn(&Report) -> usize) -> usize {
        let mut values: Vec<usize> = self.runs.iter().map(&of).collect();
        values.sort_unstable();
        values[values.len() / 2]
    }

    /// The lowest and highest a figure reached — the run-to-run spread, which
    /// is what issue #88's ±1 means in this release's numbers.
    fn spread(&self, of: impl Fn(&Report) -> usize) -> (usize, usize) {
        let mut values: Vec<usize> = self.runs.iter().map(&of).collect();
        values.sort_unstable();
        (values[0], values[values.len() - 1])
    }

    fn median_bytes(&self) -> usize {
        self.median(|r| {
            let mut bytes = r.bytes.clone();
            bytes.sort_unstable();
            bytes.get(bytes.len() / 2).copied().unwrap_or(0)
        })
    }

    fn census_share(&self) -> usize {
        self.median(|r| r.census.as_ref().map(Census::share).unwrap_or(0))
    }

    /// The median rank a question's first satisfying hit reached, and whether
    /// the runs disagreed about it.
    fn rank_of(&self, id: &str) -> (Option<usize>, bool) {
        let mut ranks: Vec<Option<usize>> = self
            .runs
            .iter()
            .map(|r| {
                r.ranks
                    .iter()
                    .find(|(qid, _)| qid == id)
                    .and_then(|(_, rank)| *rank)
            })
            .collect();
        let moved = ranks.windows(2).any(|w| w[0] != w[1]);
        // A miss sorts after every rank, so the median of two hits and a miss
        // is a hit — which is what "the median run found it" means.
        ranks.sort_by_key(|r| r.unwrap_or(usize::MAX));
        (ranks[ranks.len() / 2], moved)
    }

    /// Identifier questions whose first satisfying hit is outside the top `k`,
    /// by the median run. The contract's gate is that this list is empty.
    fn identifier_stragglers(&self, questions: &[Question], k: usize) -> Vec<String> {
        questions
            .iter()
            .filter(|q| q.shape == "identifier")
            .filter(|q| match self.rank_of(&q.id).0 {
                Some(rank) => rank > k,
                None => true,
            })
            .map(|q| q.id.clone())
            .collect()
    }

    fn print(
        &self,
        questions: &[Question],
        tool_list: usize,
        tool_tokens: usize,
        baseline: Option<&Baseline>,
    ) {
        let scored = self.scored().max(1);
        println!(
            "\n  retrieval harness — {} questions, {} runs, median of {}",
            questions.len(),
            self.runs.len(),
            self.runs.len()
        );
        for k in [1usize, 3, 8] {
            let hits = self.median(|r| r.hit_at.get(&k).copied().unwrap_or(0));
            let (low, high) = self.spread(|r| r.hit_at.get(&k).copied().unwrap_or(0));
            let against = match baseline {
                Some(b) => {
                    let was = b.hit_at.get(&k).copied().unwrap_or(0);
                    format!(
                        "   0.21.0 {was}/{} ({}%)",
                        b.scored,
                        was * 100 / b.scored.max(1)
                    )
                }
                None => String::new(),
            };
            println!(
                "  hit@{k}   {hits}/{}  ({}%)   spread {low}-{high}{against}",
                self.scored(),
                hits * 100 / scored
            );
        }

        let bytes = self.median_bytes();
        let against = match baseline {
            Some(b) => format!("   0.21.0 {}", b.bytes),
            None => String::new(),
        };
        println!("\n  bytes per answer   median {bytes}{against}");
        println!(
            "  graph-only hits    {} of {} satisfied a span",
            self.median(|r| r.graph_only_hits),
            self.median(|r| r.graph_only)
        );
        let preferred = self.median(|r| r.preferred);
        if preferred > 0 {
            println!(
                "  prefer questions   {} of {preferred} landed a span",
                self.median(|r| r.preferred_hits)
            );
        }
        let read_lines = self.median(|r| {
            let mut lines = r.read_lines.clone();
            lines.sort_unstable();
            lines.get(lines.len() / 2).copied().unwrap_or(0)
        });
        if read_lines > 0 {
            println!("  read answer size   median {read_lines} lines");
        }
        println!(
            "\n  wrong yes          {}  (path questions whose true answer is \"not connected\")",
            self.median(|r| r.wrong_yes)
        );
        println!(
            "  chains found       {} of {}, {} at the expected length",
            self.median(|r| r.chains_found),
            self.median(|r| r.chains_expected),
            self.median(|r| r.chain_length_matches)
        );
        println!("  tools/list         {tool_list} bytes, about {tool_tokens} tokens");
        println!(
            "  corpus chunks      {}   indexed in {}s (median)",
            self.median(|r| r.chunks),
            self.median(|r| r.index_seconds)
        );

        // Per shape, because an aggregate hit@k over three kinds of question
        // answers none of them. "Seven concept questions miss" is a sentence
        // somebody can act on; "hit@8 is 87 %" is not, and until this printed
        // it was counted by hand off the miss list.
        println!("\n  by shape, at k=1 / k=3 / k=8:");
        let mut shapes: Vec<&str> = questions.iter().map(|q| q.shape.as_str()).collect();
        shapes.sort_unstable();
        shapes.dedup();
        for shape in shapes {
            let of_this_shape: Vec<&Question> =
                questions.iter().filter(|q| q.shape == shape).collect();
            let at = |k: usize| {
                of_this_shape
                    .iter()
                    .filter(|q| matches!(self.rank_of(&q.id).0, Some(rank) if rank <= k))
                    .count()
            };
            let total = of_this_shape.len();
            println!(
                "    {shape:<11} {:>3} / {:>3} / {:>3}  of {total}",
                at(1),
                at(3),
                at(8)
            );
        }

        if let Some(census) = self.runs[0].census.as_ref() {
            census.print();
        }

        println!("  rank of the first satisfying hit, per question (median of the runs):");
        for question in questions {
            let (rank, moved) = self.rank_of(&question.id);
            let flag = if moved { " *" } else { "" };
            match rank {
                Some(rank) => println!("    {rank:>3}  {}{flag}", question.id),
                None => println!("      -  {}{flag}", question.id),
            }
        }
        println!("    * the runs disagreed about this question's rank");

        // What is still wrong, by shape and by tool, so the record's
        // classification is read off the run rather than reconstructed.
        let misses: Vec<&Question> = questions
            .iter()
            .filter(|q| match self.rank_of(&q.id).0 {
                Some(rank) => rank > 8,
                None => true,
            })
            .collect();
        if !misses.is_empty() {
            println!("\n  missed at k=8 ({}):", misses.len());
            let mut by_shape: BTreeMap<(&str, &str), Vec<&str>> = BTreeMap::new();
            for question in &misses {
                by_shape
                    .entry((question.shape.as_str(), question.tool.as_str()))
                    .or_default()
                    .push(question.id.as_str());
            }
            for ((shape, tool), ids) in by_shape {
                println!("    {shape}/{tool}  {}  {}", ids.len(), ids.join(" "));
            }
        }
        println!();
    }
}

fn score_search(semlith: &mut Semlith, root: &Path, question: &Question, report: &mut Report) {
    let k = question.k.unwrap_or(8);
    let prefer =
        semlith::Prefer::parse(&question.prefer).unwrap_or_else(|e| panic!("{}: {e}", question.id));
    let vector = semlith
        .embed_query(&question.query)
        .unwrap_or_else(|e| panic!("{}: embedding failed: {e}", question.id));
    let hits: Vec<semlith::Hit> = semlith
        .search_preferring(
            &question.query,
            &vector,
            k,
            &semlith::filter::Filter::default(),
            prefer,
        )
        .unwrap_or_else(|e| panic!("{}: search failed: {e}", question.id))
        .into_iter()
        .map(|(hit, _)| hit)
        .collect();
    report.scored += 1;
    if prefer != semlith::Prefer::Any {
        report.preferred += 1;
    }

    // The reply an agent would actually be handed, which is what its cost is.
    report
        .bytes
        .push(semlith::mcp::locate_bytes(&hits, &question.query));

    let mut first: Option<usize> = None;
    for (rank, hit) in hits.iter().enumerate() {
        let satisfies = question.spans.iter().any(|span| satisfied(root, hit, span));
        if satisfies && first.is_none() {
            first = Some(rank + 1);
        }
        // The third list's own contribution: a hit no other list ranked.
        if hit.lists == ["graph"] {
            report.graph_only += 1;
            if satisfies {
                report.graph_only_hits += 1;
            }
        }
    }
    report.ranks.push((question.id.clone(), first));
    match first {
        Some(rank) => {
            for k in [1usize, 3, 8] {
                if rank <= k {
                    *report.hit_at.entry(k).or_default() += 1;
                }
            }
            if prefer != semlith::Prefer::Any {
                report.preferred_hits += 1;
            }
        }
        None => report.misses.push(question.id.clone()),
    }
}

/// Did `read` return the span the question names, and nothing like a file?
///
/// The whole claim of the locate-then-read pair is that the second stage costs
/// one span rather than one file, so the size of what came back is as much the
/// measurement as whether it was right.
fn score_read(semlith: &Semlith, root: &Path, question: &Question, report: &mut Report) {
    report.scored += 1;
    let target = semlith::Target::parse(&question.target);
    let found = semlith
        .read(&target, &semlith::filter::Filter::default())
        .unwrap_or_else(|e| panic!("{}: read failed: {e}", question.id));

    let Some(semlith::Read::One(span)) = found else {
        report.ranks.push((question.id.clone(), None));
        report.misses.push(question.id.clone());
        return;
    };
    report.bytes.push(span.text.len());
    report.read_lines.push(span.text.lines().count());

    let hit = question.spans.iter().any(|want| {
        let same_file =
            span.path.ends_with(&want.path) || root.join(&want.path).to_string_lossy() == span.path;
        let overlaps = span.start_line <= want.end_line && span.end_line >= want.start_line;
        let named = want.symbol.is_empty() || span.symbol.as_deref() == Some(want.symbol.as_str());
        same_file && overlaps && named
    });
    report.ranks.push((question.id.clone(), hit.then_some(1)));
    if hit {
        for k in [1usize, 3, 8] {
            *report.hit_at.entry(k).or_default() += 1;
        }
    } else {
        report.misses.push(question.id.clone());
    }
}

/// Did `symbol` put the definition the question asks about first?
///
/// Scored on the definition list rather than on the whole evidence block: the
/// question "what is `MAX_NODES`" is answered by the row that defines it, and
/// the rings around it are context. A name with four definitions is exactly
/// the case worth measuring — the one the asker meant has to be near the top.
fn score_symbol(semlith: &Semlith, root: &Path, question: &Question, report: &mut Report) {
    report.scored += 1;
    let kinds = semlith::graph::dependency_kinds();
    let evidence = semlith::graph::evidence(semlith.db(), &question.name, &kinds, 8, false)
        .unwrap_or_else(|e| panic!("{}: symbol failed: {e}", question.id));
    report.bytes.push(
        evidence
            .render("", "", &|path: &str| path.to_string())
            .len(),
    );

    let first = evidence.definitions.iter().position(|row| {
        question.spans.iter().any(|span| {
            satisfied_row(
                root,
                &row.path,
                row.start_line,
                row.end_line,
                &row.name,
                span,
            )
        })
    });
    record_rank(report, question, first.map(|i| i + 1));
}

/// Did `neighbors` name the caller or the callee the question asks for?
///
/// The half is scored, not the whole ring: "who calls this" and "what does this
/// call" are different questions, and a tool that answers one by listing the
/// other has not answered.
fn score_neighbors(semlith: &Semlith, root: &Path, question: &Question, report: &mut Report) {
    report.scored += 1;
    let kinds = if question.kinds.is_empty() {
        semlith::graph::dependency_kinds()
    } else {
        question.kinds.clone()
    };
    let ring = semlith::graph::neighbours(semlith.db(), &question.name, &kinds, false)
        .unwrap_or_else(|e| panic!("{}: neighbors failed: {e}", question.id));
    let side = match question.want.as_str() {
        "callers" => &ring.callers,
        "callees" => &ring.callees,
        other => panic!("{}: want is {other:?}, not callers or callees", question.id),
    };

    let first = side.iter().position(|end| {
        question.spans.iter().any(|span| {
            satisfied_row(
                root,
                &end.symbol.path,
                end.symbol.start_line,
                end.symbol.end_line,
                &end.symbol.name,
                span,
            )
        })
    });
    record_rank(report, question, first.map(|i| i + 1));
}

/// The span rule, applied to a symbol row rather than to a search hit.
///
/// Same file, and either the line ranges overlap or the row is the definition
/// the span names — the same two clauses `satisfied` applies to a hit, for the
/// same reason: a moved function must still score.
fn satisfied_row(
    root: &Path,
    path: &str,
    start_line: u32,
    end_line: u32,
    name: &str,
    span: &Span,
) -> bool {
    if Path::new(path) != root.join(&span.path) {
        return false;
    }
    let overlaps = start_line <= span.end_line && end_line >= span.start_line;
    let named = !span.symbol.is_empty() && name == span.symbol;
    overlaps || named
}

/// Record one question's rank in every place the report counts it.
fn record_rank(report: &mut Report, question: &Question, rank: Option<usize>) {
    report.ranks.push((question.id.clone(), rank));
    match rank {
        Some(rank) => {
            for k in [1usize, 3, 8] {
                if rank <= k {
                    *report.hit_at.entry(k).or_default() += 1;
                }
            }
        }
        None => report.misses.push(question.id.clone()),
    }
}

fn score_path(semlith: &Semlith, question: &Question, report: &mut Report) {
    report.scored += 1;
    let found = semlith::graph::shortest_path(
        semlith.db(),
        &question.from,
        &question.to,
        6,
        // The default, which is the behaviour under test. `--all-edges` is a
        // deliberate request for a hypothesis and is not what an agent gets.
        false,
    )
    .unwrap_or_else(|e| panic!("{}: path failed: {e}", question.id));

    // A path question is answered or it is not, and that is its rank.
    //
    // Until 0.22.0 this function recorded no rank at all while still counting
    // the question in `scored`, so every `path` question was a permanent miss
    // in hit@k however well the tool answered it. The run that found this
    // reported "chains found 6 of 7, 6 at the expected length" and "wrong yes
    // 0" in the same breath as ten path questions missing at k=8 — the tool was
    // right and the instrument said it was wrong. With ten of seventy-seven
    // questions unable to score, hit@8 was capped at 87 % against a 95 % gate,
    // so the gate was unreachable by construction rather than by retrieval.
    //
    // There is no rank to speak of here: `shortest_path` returns one answer or
    // none, so a right answer is rank 1 and a wrong one is a miss. The chain's
    // length stays a separate figure rather than a condition of the hit,
    // because `hops` is written against a tree that moves and a chain of a
    // different length is still a chain.
    let correct = match question.connected {
        Some(false) => {
            if found.is_some() {
                report.wrong_yes += 1;
                report.misses.push(format!("{} (wrong yes)", question.id));
                false
            } else {
                true
            }
        }
        Some(true) => {
            report.chains_expected += 1;
            match &found {
                Some(chain) => {
                    report.chains_found += 1;
                    if question.hops == Some(chain.steps.len()) {
                        report.chain_length_matches += 1;
                    }
                    true
                }
                None => {
                    report.misses.push(format!("{} (no chain)", question.id));
                    false
                }
            }
        }
        // A path question with no `connected` is a question with no ground
        // truth, which the set does not contain and must not acquire.
        None => panic!("{}: a path question with no `connected`", question.id),
    };
    record_rank(report, question, correct.then_some(1));
}

/// Whether one hit answers one span.
///
/// Same file, and either the line ranges overlap or the hit sits inside the
/// definition the span names. The second clause is what lets a moved function
/// still score, which is the property the question set was written to have.
fn satisfied(root: &Path, hit: &semlith::Hit, span: &Span) -> bool {
    let wanted = root.join(&span.path);
    if Path::new(&hit.path) != wanted {
        return false;
    }
    let overlaps = hit.start_line <= span.end_line && hit.end_line >= span.start_line;
    let named = !span.symbol.is_empty() && hit.symbol.as_deref() == Some(span.symbol.as_str());
    overlaps || named
}

/// What share of the call edges the corpus can answer for resolve to exactly
/// one definition.
///
/// Measured through `edges_out`, not by reimplementing the ranking in SQL: the
/// question is what the resolver actually returns, and a second implementation
/// of the ladder would measure the second implementation.
///
/// The denominator is deliberately the edges whose target this corpus holds.
/// An edge into the standard library or into a crate nobody indexed cannot be
/// resolved by any ranking, and counting those would report a number that says
/// more about what was indexed than about the resolver.
struct Census {
    resolved: usize,
    extracted: usize,
    ambiguous: usize,
    outside: usize,
}

impl Census {
    fn share(&self) -> usize {
        let answerable = self.resolved + self.extracted + self.ambiguous;
        if answerable == 0 {
            return 0;
        }
        (self.resolved + self.extracted) * 100 / answerable
    }

    fn print(&self) {
        let answerable = self.resolved + self.extracted + self.ambiguous;
        println!("  resolution, over call edges whose target this corpus holds");
        println!(
            "    settled      {} of {answerable}  ({}%)",
            self.resolved + self.extracted,
            self.share()
        );
        println!("      extracted  {}", self.extracted);
        println!("      resolved   {}", self.resolved);
        println!("    ambiguous    {}", self.ambiguous);
        println!("    outside the corpus, not counted   {}\n", self.outside);
    }
}

fn resolution_census(semlith: &Semlith) -> Census {
    let db = semlith.db();
    let mut names: Vec<String> = Vec::new();
    {
        let mut stmt = db
            .prepare("SELECT DISTINCT name FROM symbols")
            .expect("the symbol table reads");
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .expect("symbol names read");
        for name in rows {
            names.push(name.expect("a symbol name"));
        }
    }

    let kinds = vec!["calls".to_string()];
    let mut census = Census {
        resolved: 0,
        extracted: 0,
        ambiguous: 0,
        outside: 0,
    };
    for name in &names {
        // One row per *edge*, not per candidate.
        //
        // `edges_out` returns every candidate for an edge it could not settle,
        // so an ambiguous edge with ten candidates comes back as ten rows and a
        // resolved edge as one. Counting the raw rows makes the ambiguous share
        // a function of how many definitions the ambiguous names happen to
        // have, which is not what "what share of edges resolve" asks — and it
        // reported 35% where the edges say something quite different.
        // `collapse` folds each ambiguous edge back to the one row it is.
        let edges = semlith::graph::collapse(
            semlith::store::edges_out(db, name, &kinds).expect("edges read"),
        );
        for end in edges {
            match end.confidence.as_str() {
                c if c == semlith::graph::EXTRACTED => census.extracted += 1,
                c if c == semlith::graph::RESOLVED => census.resolved += 1,
                _ => census.ambiguous += 1,
            }
        }
        census.outside += semlith::store::unresolved_out(db, name, &kinds)
            .expect("unresolved read")
            .len();
    }
    census
}

/// What `corpus.yaml` records about the pinned snapshot.
///
/// There is no `SEMLITH_MEASURE_CORPUS` override here any more. It existed so
/// that two releases could be compared over one tree, and a snapshot checked in
/// beside the questions gives that for nothing, on every machine, without
/// anybody remembering to set a variable. `tests/measure.rs` still reads the
/// variable, because its corpus is a scale test rather than an answer key.
struct Manifest {
    commit: String,
    tag: String,
    files: usize,
    bytes: u64,
}

fn read_manifest(path: &Path) -> Manifest {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("the corpus manifest is missing at {}: {e}", path.display()));
    let field = |key: &str| -> String {
        text.lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .find_map(|line| line.strip_prefix(key)?.strip_prefix(": "))
            .unwrap_or_else(|| panic!("corpus.yaml has no {key}"))
            .trim()
            .to_string()
    };
    let number = |key: &str| -> u64 {
        field(key)
            .parse()
            .unwrap_or_else(|_| panic!("corpus.yaml's {key} is not a number"))
    };
    Manifest {
        commit: field("commit"),
        tag: field("tag"),
        files: number("files") as usize,
        bytes: number("bytes"),
    }
}

/// How many files the snapshot holds and how many bytes they are.
///
/// Cheap, and it is what makes "the same binary gives the same number on any
/// commit" checkable rather than asserted: a corpus somebody edited fails the
/// run instead of moving every figure by a question.
fn weigh(root: &Path) -> (usize, u64) {
    let mut files = 0;
    let mut bytes = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("the corpus reads").flatten() {
            let meta = entry.metadata().expect("a corpus entry");
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                files += 1;
                bytes += meta.len();
            }
        }
    }
    (files, bytes)
}

/// Read the question set.
///
/// A reader for exactly the subset of YAML this one file uses, because the
/// release adds no dependency for the harness and a general parser is not what
/// is needed: two levels of mapping, one list of records, one nested list of
/// spans, and folded blocks that this ignores.
///
/// It fails loudly on anything it does not recognise. A parser that silently
/// returned an empty set would turn every metric below into a passing zero,
/// which is the one failure mode a measuring stick may not have.
fn read_questions(path: &Path) -> Vec<Question> {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("the question set is missing at {}: {e}", path.display()));

    let mut questions: Vec<Question> = Vec::new();
    let mut in_questions = false;
    let mut in_spans = false;
    // Set while a folded block (`>-`) is being skipped, to the indent of the
    // key that opened it; every deeper line belongs to that block.
    let mut folding: Option<usize> = None;

    for (number, raw) in text.lines().enumerate() {
        let line = raw.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if let Some(opened_at) = folding {
            if indent > opened_at {
                continue;
            }
            folding = None;
        }
        let body = line.trim_start();

        if !in_questions {
            in_questions = body == "questions:";
            continue;
        }

        // A new question, or a new span inside one.
        if let Some(rest) = body.strip_prefix("- ") {
            if indent <= 2 {
                questions.push(Question::default());
                in_spans = false;
            } else {
                let current = questions
                    .last_mut()
                    .unwrap_or_else(|| panic!("line {}: a span before any question", number + 1));
                assert!(
                    in_spans,
                    "line {}: a nested list that is not `spans`",
                    number + 1
                );
                current.spans.push(Span::default());
            }
            assign(&mut questions, in_spans, rest, number + 1, &mut folding);
            continue;
        }

        if body == "spans:" {
            in_spans = true;
            continue;
        }
        if indent <= 4 && body.ends_with(':') && !body.contains(": ") {
            // A key with a block under it that is not `spans`. Nothing here
            // needs one, so it is a change to this file the harness has not
            // been taught about.
            panic!("line {}: unrecognised block {body:?}", number + 1);
        }
        if indent <= 4 {
            in_spans = false;
        }
        assign(&mut questions, in_spans, body, number + 1, &mut folding);
    }

    assert!(
        !questions.is_empty(),
        "the question set parsed to nothing, which would make every metric a passing zero"
    );
    for question in &questions {
        assert!(!question.id.is_empty(), "a question with no id");
        assert!(
            !question.tool.is_empty(),
            "{}: a question with no tool",
            question.id
        );
    }
    questions
}

/// Apply one `key: value` line to the question or span being built.
fn assign(
    questions: &mut [Question],
    in_spans: bool,
    body: &str,
    line: usize,
    folding: &mut Option<usize>,
) {
    let Some((key, value)) = body.split_once(':') else {
        panic!("line {line}: {body:?} is not a key and a value");
    };
    let key = key.trim();
    let value = value.trim();

    // `>-` and `|` open a block that belongs to prose. The harness reads no
    // prose, so the block is skipped rather than half-parsed.
    if value.starts_with('>') || value.starts_with('|') {
        *folding = Some(4);
        return;
    }
    let value = value.trim_matches(|c| c == '"' || c == '\'');
    let question = questions
        .last_mut()
        .unwrap_or_else(|| panic!("line {line}: a field before any question"));

    if in_spans {
        let span = question
            .spans
            .last_mut()
            .unwrap_or_else(|| panic!("line {line}: a span field outside a span"));
        match key {
            "path" => span.path = value.to_string(),
            // `~` is YAML's null, used where a span has no enclosing symbol.
            "symbol" => {
                span.symbol = if value == "~" {
                    String::new()
                } else {
                    value.to_string()
                }
            }
            "start_line" => span.start_line = number(value, line),
            "end_line" => span.end_line = number(value, line),
            "note" => {}
            other => panic!("line {line}: unknown span field {other:?}"),
        }
        return;
    }

    match key {
        "id" => question.id = value.to_string(),
        "shape" => question.shape = value.to_string(),
        "tool" => question.tool = value.to_string(),
        "query" => question.query = value.to_string(),
        "name" => question.name = value.to_string(),
        "from" => question.from = value.to_string(),
        "to" => question.to = value.to_string(),
        "connected" => question.connected = Some(value == "true"),
        "target" => question.target = value.to_string(),
        "prefer" => question.prefer = value.to_string(),
        "hops" => question.hops = Some(number(value, line) as usize),
        "k" => question.k = Some(number(value, line) as usize),
        "want" => question.want = value.to_string(),
        // A YAML flow list, which is the only shape this field takes.
        "kinds" => {
            question.kinds = value
                .trim_matches(|c| c == '[' || c == ']')
                .split(',')
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
                .collect()
        }
        "depth" | "grep" | "note" => {}
        other => panic!("line {line}: unknown question field {other:?}"),
    }
}

fn number(value: &str, line: usize) -> u32 {
    value
        .parse()
        .unwrap_or_else(|_| panic!("line {line}: {value:?} is not a number"))
}
