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

    let source = corpus_source();
    let questions = read_questions(&source.join("tests/fixtures/retrieval/questions.yaml"));
    assert!(
        questions.len() >= 30,
        "the question set has shrunk to {} questions; it is the measuring stick and \
         may not be trimmed to suit a result",
        questions.len()
    );

    // The corpus is a snapshot, not the working tree, and that is the other
    // half of #88. Indexing the repository root indexed `target/`, `.git/` and
    // every scratch file beside them, so the corpus moved with the build and
    // with whatever else was open — which is why a byte-identical copy of the
    // tree at another path reported a different bytes-per-answer. Four entries
    // are copied because the question set names four: `src`, `tests`, `docs`
    // and `AGENTS.md`. `tests/measure.rs` already snapshots for the same reason.
    let snapshot = tempfile::tempdir().expect("a temporary corpus");
    // Canonical, because the scoring compares `root.join(span.path)` against
    // the path the store recorded, and the store canonicalizes what it indexes.
    // On macOS a temporary directory is `/var/folders/...` and its canonical
    // form is `/private/var/folders/...`, so without this every span comparison
    // fails and the harness reports 2/47 at every depth — which is what it did.
    let root = snapshot
        .path()
        .canonicalize()
        .expect("the corpus directory resolves");
    for name in ["src", "tests", "docs"] {
        copy_tree(&source.join(name), &root.join(name));
    }
    std::fs::copy(source.join("AGENTS.md"), root.join("AGENTS.md"))
        .expect("AGENTS.md is part of the corpus the question set measures");

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
    semlith
        .index_paths(std::slice::from_ref(&root), |_, _| {})
        .expect("the repository indexes");

    // The question set is not part of the corpus it measures.
    //
    // It lives under `tests/fixtures/`, so indexing the repository indexes it
    // too — and it is a prose-dense document that names every identifier the
    // questions ask about, many times each. The first run of this harness
    // scored 16% at hit@1 and reported `edges_out` as a miss, because the top
    // two hits for `edges_out` were the two places in this file that ask about
    // `edges_out`. A benchmark that indexes its own answer key measures the
    // answer key.
    //
    // Forgotten rather than filtered so the exclusion is a fact about the
    // store rather than an argument every query has to remember to pass.
    let questions_file = root.join("tests/fixtures/retrieval/questions.yaml");
    semlith
        .forget(&questions_file)
        .expect("the question set leaves the corpus it measures");

    let score_all = |semlith: &mut Semlith| {
        let mut report = Report::default();
        for question in &questions {
            match question.tool.as_str() {
                "search" => score_search(semlith, &root, question, &mut report),
                "path" => score_path(semlith, question, &mut report),
                "read" => score_read(semlith, &root, question, &mut report),
                // Scored by the same span rule as a search once they are wired
                // up. Counted as skipped rather than as misses: a metric that
                // punishes the harness for what the harness has not implemented
                // is a metric that rewards deleting questions.
                _ => report.skipped += 1,
            }
        }
        report
    };

    let report = score_all(&mut semlith);

    // The harness asserts its own determinism — issue #88, where three runs of
    // one binary over one corpus gave three different hit@k and the graph-only
    // denominator moved between them.
    //
    // The question set is scored twice against the same store, and the two
    // reports must be identical question by question. That covers every
    // query-time source of the drift, which is where it lived: `graph::expand`
    // walked its frontier through a `HashMap`, whose iteration order is seeded
    // randomly per process, so the `f32` masses summed differently, the
    // confidence a name was labelled with depended on arrival order, and
    // `MAX_NODES` truncated whichever names the order had not reached yet.
    //
    // It deliberately does not re-index. The other source was index-time —
    // ONNX Runtime reducing across its intra-op threads in completion order —
    // and the fix for that is the single thread pinned at the top of this test.
    // Checking it here would mean a second full index on every run, for a
    // property one embedding thread already makes true; it is verified once per
    // release by running this harness three times and comparing the reports.
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

    let census = resolution_census(&semlith);
    let tool_list = semlith::mcp::tool_list_bytes();
    let tool_tokens = tool_list.div_ceil(4);
    report.print(&questions, tool_list, tool_tokens);
    census.print();

    assert!(
        census.share() > 50,
        "only {}% of call edges whose target this corpus holds resolve to one \
         definition; the resolver is the release's central change and this is the \
         number that says whether it works",
        census.share()
    );
    assert_eq!(
        report.wrong_yes, 0,
        "{} path question(s) returned a chain between symbols that are not connected. \
         This is the one metric with a hard gate.",
        report.wrong_yes
    );
    assert!(
        tool_tokens < 1_000,
        "tools/list is {tool_list} bytes, about {tool_tokens} tokens, and every agent \
         pays it once per session"
    );
}

#[derive(Default)]
struct Report {
    scored: usize,
    skipped: usize,
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
}

impl Report {
    fn print(&self, questions: &[Question], tool_list: usize, tool_tokens: usize) {
        let searched = self.scored.max(1);
        println!("\n  retrieval harness — {} questions", questions.len());
        println!("  {} scored, {} skipped\n", self.scored, self.skipped);
        for k in [1usize, 3, 8] {
            let hits = self.hit_at.get(&k).copied().unwrap_or(0);
            println!(
                "  hit@{k}   {hits}/{}  ({}%)",
                self.scored,
                hits * 100 / searched
            );
        }
        let mut bytes = self.bytes.clone();
        bytes.sort_unstable();
        let median = bytes.get(bytes.len() / 2).copied().unwrap_or(0);
        let worst = bytes.last().copied().unwrap_or(0);
        println!("\n  bytes per answer   median {median}, worst {worst}");
        println!(
            "  graph-only hits    {} of {} satisfied a span",
            self.graph_only_hits, self.graph_only
        );
        if self.preferred > 0 {
            println!(
                "  prefer questions   {} of {} landed a span",
                self.preferred_hits, self.preferred
            );
        }
        if !self.read_lines.is_empty() {
            let mut lines = self.read_lines.clone();
            lines.sort_unstable();
            println!(
                "  read answer size   median {} lines, worst {}",
                lines[lines.len() / 2],
                lines.last().copied().unwrap_or(0)
            );
        }
        println!(
            "\n  wrong yes          {}  (path questions whose true answer is \"not connected\")",
            self.wrong_yes
        );
        println!(
            "  chains found       {} of {}, {} at the expected length",
            self.chains_found, self.chains_expected, self.chain_length_matches
        );
        println!("  tools/list         {tool_list} bytes, about {tool_tokens} tokens");
        println!("\n  rank of the first satisfying hit, per question:");
        for (id, rank) in &self.ranks {
            match rank {
                Some(rank) => println!("    {rank:>3}  {id}"),
                None => println!("      -  {id}"),
            }
        }
        if !self.misses.is_empty() {
            println!("\n  missed at k=8:");
            for id in &self.misses {
                println!("    {id}");
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

    match question.connected {
        Some(false) if found.is_some() => {
            report.wrong_yes += 1;
            report.misses.push(format!("{} (wrong yes)", question.id));
        }
        Some(false) => {}
        Some(true) => {
            report.chains_expected += 1;
            match found {
                Some(chain) => {
                    report.chains_found += 1;
                    if question.hops == Some(chain.steps.len()) {
                        report.chain_length_matches += 1;
                    }
                }
                None => report.misses.push(format!("{} (no chain)", question.id)),
            }
        }
        None => {}
    }
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

/// The tree the corpus is taken from: this repository, unless
/// `SEMLITH_MEASURE_CORPUS` names another checkout of it.
///
/// The override is what makes an A/B between two releases mean anything. Every
/// number this harness prints moves with the corpus as well as with the code,
/// and the corpus is this repository, so comparing two releases without holding
/// one tree still compares two codebases over two corpora and calls the
/// difference a ranking change. `tests/measure.rs` reads the same variable for
/// the same reason. The tree named has to be a checkout of this repository: the
/// question set's spans are line ranges in these files.
fn corpus_source() -> PathBuf {
    std::env::var_os("SEMLITH_MEASURE_CORPUS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")))
}

/// Copy `from` into `to`, creating what it needs.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
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
        "depth" | "grep" | "kinds" | "want" | "note" => {}
        other => panic!("line {line}: unknown question field {other:?}"),
    }
}

fn number(value: &str, line: usize) -> u32 {
    value
        .parse()
        .unwrap_or_else(|_| panic!("line {line}: {value:?} is not a number"))
}
