//! Structure half of the store: symbols, and the edges between them.
//!
//! Extraction is tree-sitter over a file's text, and it hangs off the same
//! changed-file path that drives re-embedding (see [`crate::Semlith::index_set`]).
//! That is the whole freshness story: there is no build step, no separate
//! command, and no artifact that can be out of date with the corpus, because
//! the pass that re-embeds a file is the pass that re-extracts it.
//!
//! Six languages carry edges. The rest are searchable exactly as before and
//! simply have no rows here.
//!
//! # Where the queries come from
//!
//! Every grammar ships a `TAGS_QUERY` written for `tree-sitter tags`, and it is
//! a good source of *definitions* — that is what it was built for. Its
//! reference coverage is uneven and none of it covers imports: of the six here,
//! none capture import statements at all, and TypeScript and C capture no calls
//! either. So each language pairs the bundled query with a short supplementary
//! one below, and the two are run together.
//!
//! # Confidence
//!
//! An edge is `extracted` when the syntax tree says where the target came from
//! — a structural edge, or a call whose name the file also imports — and
//! `inferred` when the target was matched by bare name alone. Two functions
//! called `new` in different modules is the normal case in real code, not a
//! corner case, so an inferred edge is marked and stays marked everywhere it
//! surfaces. Nothing may present one as the other.

use anyhow::Result;

/// How long one file's parse may take before it is given up on.
const PARSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
use std::path::Path;
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

/// Edge kinds, as stored in `edges.kind`.
pub const KINDS: [&str; 5] = ["defines", "calls", "imports", "references", "contains"];

pub const EXTRACTED: &str = "extracted";
pub const INFERRED: &str = "inferred";

/// A symbol found in a file, before it has an id.
#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub kind: String,
    pub name: String,
    pub qualified: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// An edge found in a file, before either end has an id.
///
/// `from` is the name of the symbol the reference sits inside — the file's
/// module symbol when it sits at the top level.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub confidence: String,
}

/// What one file yielded.
#[derive(Debug, Clone, Default)]
pub struct Extraction {
    pub symbols: Vec<Symbol>,
    pub edges: Vec<Edge>,
}

/// The languages that carry edges, as the About page lists them.
pub const LANGUAGES: [&str; 6] = ["rust", "typescript", "python", "go", "java", "c"];

/// Extensions that reach an extractor, and the language label each maps to.
///
/// Dispatch is by extension, like `chunk::extract`, and for the same reason: it
/// decides before any bytes are read.
const EXTENSIONS: &[(&str, &str)] = &[
    ("rs", "rust"),
    ("ts", "typescript"),
    ("tsx", "typescript"),
    ("mts", "typescript"),
    ("cts", "typescript"),
    ("py", "python"),
    ("pyi", "python"),
    ("go", "go"),
    ("java", "java"),
    ("c", "c"),
    ("h", "c"),
];

/// The language label for a path, if this release extracts from it.
pub fn language_of(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    EXTENSIONS
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, lang)| *lang)
}

/// What each grammar's `TAGS_QUERY` does not give us.
///
/// Kept short on purpose: the bundled query is the source of truth for
/// definitions, and this closes the two gaps it has — imports, which no grammar
/// here captures, and calls, which TypeScript and C do not.
fn supplement(lang: &str) -> &'static str {
    match lang {
        "rust" => {
            r#"
            (use_declaration argument: (_) @reference.import)
            ; A scoped call names two things worth an edge: the type or module
            ; it went through, and the function itself. `File::create` is most
            ; useful as an edge to `File`; `store::record_retrieval` is most
            ; useful as an edge to `record_retrieval`. Capture both rather than
            ; guess which kind of path this is.
            (call_expression
              function: (scoped_identifier path: (identifier) @name) @reference.call)
            (call_expression
              function: (scoped_identifier name: (identifier) @name) @reference.call)
            ; A method call: `self.flush()`, `store.db()`.
            (call_expression
              function: (field_expression field: (field_identifier) @name) @reference.call)
        "#
        }
        // TypeScript's bundled query matches nothing on ordinary unexported
        // declarations, so its definitions are spelled out here rather than
        // inherited.
        "typescript" => {
            r#"
            (import_statement source: (string) @reference.import)
            (function_declaration name: (identifier) @name) @definition.function
            (class_declaration name: (type_identifier) @name) @definition.class
            (interface_declaration name: (type_identifier) @name) @definition.interface
            (method_definition name: (property_identifier) @name) @definition.method
            (variable_declarator
              name: (identifier) @name
              value: [(arrow_function) (function_expression)]) @definition.function
            (call_expression function: (identifier) @name) @reference.call
            (call_expression
              function: (member_expression property: (property_identifier) @name)) @reference.call
        "#
        }
        "python" => {
            r#"
            (import_statement name: (_) @reference.import)
            (import_from_statement module_name: (_) @reference.import)
        "#
        }
        "go" => {
            r#"
            (import_spec path: (interpreted_string_literal) @reference.import)
        "#
        }
        "java" => {
            r#"
            (import_declaration (scoped_identifier) @reference.import)
        "#
        }
        "c" => {
            r#"
            (preproc_include path: (_) @reference.import)
            (call_expression function: (identifier) @reference.call)
        "#
        }
        _ => "",
    }
}

fn grammar(lang: &str) -> Option<(Language, &'static str)> {
    match lang {
        "rust" => Some((
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::TAGS_QUERY,
        )),
        "typescript" => Some((
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            tree_sitter_typescript::TAGS_QUERY,
        )),
        "python" => Some((
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::TAGS_QUERY,
        )),
        "go" => Some((tree_sitter_go::LANGUAGE.into(), tree_sitter_go::TAGS_QUERY)),
        "java" => Some((
            tree_sitter_java::LANGUAGE.into(),
            tree_sitter_java::TAGS_QUERY,
        )),
        "c" => Some((tree_sitter_c::LANGUAGE.into(), tree_sitter_c::TAGS_QUERY)),
        _ => None,
    }
}

/// The compiled queries for a language, built once per process.
///
/// Compiling a query is orders of magnitude dearer than running one, and
/// indexing a repository runs these over every file it walks. A grammar whose
/// bundled query does not compile against the grammar version linked here is a
/// packaging problem rather than a user's file, so that half is dropped and the
/// other still runs.
fn queries(lang: &'static str, language: &Language, tags: &str) -> &'static [Query] {
    static CACHE: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<&'static str, &'static [Query]>>,
    > = std::sync::LazyLock::new(Default::default);

    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.entry(lang).or_insert_with(|| {
        let compiled: Vec<Query> = [tags, supplement(lang)]
            .iter()
            .filter(|source| !source.is_empty())
            .filter_map(|source| Query::new(language, source).ok())
            .collect();
        Box::leak(compiled.into_boxed_slice())
    })
}

/// A definition found in the tree, with the byte range that makes nesting
/// decidable.
struct Def {
    kind: String,
    name: String,
    start: usize,
    end: usize,
    start_line: u32,
    end_line: u32,
}

/// A reference found in the tree, with the byte offset that places it inside a
/// definition.
struct Ref {
    name: String,
    kind: &'static str,
    at: usize,
}

/// Symbols and edges for one file.
///
/// `None` when the extension carries no grammar. A file that fails to parse is
/// not an error either: tree-sitter always returns a tree, and a partial parse
/// of a file mid-edit yields the symbols it could read rather than nothing.
pub fn extract(path: &Path, text: &str) -> Result<Option<Extraction>> {
    let Some(lang) = language_of(path) else {
        return Ok(None);
    };
    let Some((language, tags)) = grammar(lang) else {
        return Ok(None);
    };

    let mut parser = Parser::new();
    parser.set_language(&language)?;
    // Bounded. A tree-sitter grammar can take superlinear time on input that
    // is valid and pathological — a deeply nested expression, a file of
    // brackets — and this runs inside the store's write lock, so a parser that
    // does not come back is a store nothing else can write to. Two seconds is
    // far more than any real file takes, and expiry means "no graph for this
    // file", which is what an unsupported language already means.
    let deadline = std::time::Instant::now() + PARSE_TIMEOUT;
    let mut expired = |_: &tree_sitter::ParseState| {
        if std::time::Instant::now() >= deadline {
            std::ops::ControlFlow::Break(())
        } else {
            std::ops::ControlFlow::Continue(())
        }
    };
    let options = tree_sitter::ParseOptions::default().progress_callback(&mut expired);
    let parsed =
        parser.parse_with_options(&mut |at, _| &text.as_bytes()[at..], None, Some(options));
    let Some(tree) = parsed else {
        return Ok(None);
    };

    let mut defs: Vec<Def> = Vec::new();
    let mut refs: Vec<Ref> = Vec::new();

    for query in queries(lang, &language, tags) {
        collect(query, &tree, text, &mut defs, &mut refs);
    }

    // The bundled query and the supplement can capture one item twice — and
    // within the bundled query a method is often both `definition.method` and
    // `definition.function`. Two Defs over one range make each the other's
    // innermost enclosing definition, which is how a symbol ends up containing
    // itself.
    defs.sort_by(|a, b| {
        (a.start, std::cmp::Reverse(a.end), &a.name).cmp(&(
            b.start,
            std::cmp::Reverse(b.end),
            &b.name,
        ))
    });
    defs.dedup_by(|a, b| a.start == b.start && a.end == b.end && a.name == b.name);

    let module = module_name(path);
    let mut symbols = vec![Symbol {
        kind: "module".to_string(),
        name: module.clone(),
        qualified: path.to_string_lossy().to_string(),
        start_line: 1,
        end_line: text.lines().count().max(1) as u32,
    }];
    let mut edges: Vec<Edge> = Vec::new();

    for (i, def) in defs.iter().enumerate() {
        let parent = enclosing(&defs, i, def.start).map(|p| defs[p].name.clone());
        let qualified = match &parent {
            Some(p) => format!("{p}::{}", def.name),
            None => format!("{module}::{}", def.name),
        };
        symbols.push(Symbol {
            kind: def.kind.clone(),
            name: def.name.clone(),
            qualified,
            start_line: def.start_line,
            end_line: def.end_line,
        });
        // Structural edges are read straight off the tree, so both are
        // extracted: a definition is either nested in another or it is not.
        // A self-edge is possible and meaningless: `A.java` takes `A` as its
        // module name and then declares `class A`.
        if parent.as_deref().unwrap_or(&module) == def.name {
            continue;
        }
        match parent {
            Some(p) => edges.push(Edge {
                from: p,
                to: def.name.clone(),
                kind: "contains".to_string(),
                confidence: EXTRACTED.to_string(),
            }),
            None => edges.push(Edge {
                from: module.clone(),
                to: def.name.clone(),
                kind: "defines".to_string(),
                confidence: EXTRACTED.to_string(),
            }),
        }
    }

    // The names this file imports. A call to one of them was resolved by the
    // file itself rather than guessed at, which is the line between extracted
    // and inferred for a reference.
    let imported: Vec<String> = refs
        .iter()
        .filter(|r| r.kind == "imports")
        .flat_map(|r| import_names(&r.name))
        .collect();

    for reference in &refs {
        let from = match enclosing_name(&defs, reference.at) {
            Some(name) => name,
            None => module.clone(),
        };
        if from == reference.name {
            continue; // direct recursion adds a self-edge and no information
        }
        let confidence = if reference.kind == "imports" || imported.contains(&reference.name) {
            EXTRACTED
        } else {
            INFERRED
        };
        edges.push(Edge {
            from,
            to: reference.name.clone(),
            kind: reference.kind.to_string(),
            confidence: confidence.to_string(),
        });
    }

    edges.sort_by(|a, b| (&a.from, &a.to, &a.kind).cmp(&(&b.from, &b.to, &b.kind)));
    edges.dedup();

    Ok(Some(Extraction { symbols, edges }))
}

fn collect(
    query: &Query,
    tree: &tree_sitter::Tree,
    text: &str,
    defs: &mut Vec<Def>,
    refs: &mut Vec<Ref>,
) {
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        // A tags query names its tag with `@name` and spans it with
        // `@definition.X` or `@reference.X`, and the two are separate captures
        // of the same match. Java is the clearest case: `@reference.call` lands
        // on the argument list `()` and the method's identifier arrives beside
        // it as `@name`. So the match is read as a whole, not capture by
        // capture.
        let mut span: Option<(&str, tree_sitter::Node)> = None;
        let mut name: Option<tree_sitter::Node> = None;
        let mut references: Vec<(&str, tree_sitter::Node)> = Vec::new();
        for capture in m.captures() {
            let capture_name = names[capture.index as usize];
            if let Some(kind) = capture_name.strip_prefix("definition.") {
                span = Some((kind, capture.node));
            } else if capture_name == "name" {
                name = Some(capture.node);
            } else if let Some(kind) = capture_name.strip_prefix("reference.") {
                references.push((kind, capture.node));
            }
        }

        for (kind, node) in references {
            let kind = match kind {
                "call" => "calls",
                "import" => "imports",
                _ => "references",
            };
            // An import is a literal path and is kept whole. Anything else
            // takes the match's `@name` when it has one, and otherwise
            // descends into whatever node the grammar chose to capture —
            // Rust's query hands back `helper()` for a call and an entire
            // `impl` block for an implementation.
            let target = if kind == "imports" {
                Some(trim_literal(&text[node.byte_range()]).to_string())
            } else {
                match name {
                    Some(n) => Some(text[n.byte_range()].to_string()),
                    None => reference_name(node, text),
                }
            };
            let Some(target) = target.filter(|t| !t.is_empty()) else {
                continue;
            };
            refs.push(Ref {
                name: target,
                kind,
                at: node.start_byte(),
            });
        }

        if let (Some((kind, node)), Some(name)) = (span, name) {
            let span = definition_span(node, name);
            defs.push(Def {
                kind: kind.to_string(),
                name: text[name.byte_range()].to_string(),
                start: span.start_byte(),
                end: span.end_byte(),
                start_line: span.start_position().row as u32 + 1,
                end_line: span.end_position().row as u32 + 1,
            });
        }
    }
}

/// The identifier an edge should point at, out of whatever node the query
/// captured.
///
/// Grammars disagree about this. Some capture the bare identifier, some the
/// whole call expression, some an entire `impl` block. Descending to the name
/// makes all of them agree, and a capture with no identifier in it at all is
/// dropped rather than stored as a wall of source text.
fn reference_name(node: tree_sitter::Node, text: &str) -> Option<String> {
    if let Some(function) = node.child_by_field_name("function") {
        return reference_name(function, text);
    }
    if let Some(property) = node.child_by_field_name("property") {
        return reference_name(property, text);
    }
    let kind = node.kind();
    if kind.ends_with("identifier") || kind == "type_identifier" || kind == "word" {
        return Some(text[node.byte_range()].to_string());
    }
    if let Some(name) = node.child_by_field_name("name") {
        return reference_name(name, text);
    }
    // A path or qualified name: the last identifier in it is the thing named.
    let mut cursor = node.walk();
    let last = node
        .named_children(&mut cursor)
        .filter(|c| c.kind().ends_with("identifier"))
        .last();
    match last {
        Some(child) => Some(text[child.byte_range()].to_string()),
        None if node.child_count() == 0 => Some(text[node.byte_range()].to_string()),
        None => None,
    }
}

/// The byte range a definition owns, which is what decides what is nested
/// inside it.
///
/// A grammar that captures its definition on the declaration node gives this
/// directly. TypeScript's tags query does not — it captures a node that stops
/// short of the body, so every call inside a function would look like it sat
/// at the top level of the file. Climbing from the name to the nearest
/// declaration-shaped ancestor recovers the real extent.
fn definition_span<'a>(
    captured: tree_sitter::Node<'a>,
    name: tree_sitter::Node<'a>,
) -> tree_sitter::Node<'a> {
    if captured.start_byte() <= name.start_byte() && captured.end_byte() > name.end_byte() {
        return captured;
    }
    let mut best = captured;
    let mut node = name;
    while let Some(parent) = node.parent() {
        if declaration_shaped(parent.kind()) && parent.end_byte() > best.end_byte() {
            best = parent;
        }
        node = parent;
    }
    best
}

fn declaration_shaped(kind: &str) -> bool {
    const MARKERS: [&str; 10] = [
        "declaration",
        "definition",
        "_item",
        "specifier",
        "class",
        "function",
        "method",
        "struct",
        "interface",
        "module",
    ];
    MARKERS.iter().any(|m| kind.contains(m))
}

/// The index of the innermost definition strictly enclosing `at`, skipping
/// `self_index` so a definition is never its own parent.
fn enclosing(defs: &[Def], self_index: usize, at: usize) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, d) in defs.iter().enumerate() {
        if i == self_index || d.start > at || d.end <= at {
            continue;
        }
        if best.is_none_or(|b| d.end - d.start < defs[b].end - defs[b].start) {
            best = Some(i);
        }
    }
    best
}

fn enclosing_name(defs: &[Def], at: usize) -> Option<String> {
    let mut best: Option<&Def> = None;
    for d in defs {
        if d.start > at || d.end <= at {
            continue;
        }
        if best.is_none_or(|b| d.end - d.start < b.end - b.start) {
            best = Some(d);
        }
    }
    best.map(|d| d.name.clone())
}

/// A file's own symbol: what top-level definitions hang off and where
/// top-level references come from.
fn module_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("module")
        .to_string()
}

/// An import's text as the names it could resolve a later reference to.
///
/// `use std::fs::File` imports `File`; `import java.util.List` imports `List`;
/// `from a.b import c` names the module. Taking the last segment covers all of
/// them, and the whole string is kept too so an edge to a module path still
/// resolves.
fn import_names(raw: &str) -> Vec<String> {
    let cleaned = trim_literal(raw);
    let mut out = vec![cleaned.to_string()];
    if let Some(last) = cleaned
        .rsplit(['/', '.', ':'])
        .find(|s| !s.is_empty())
        .filter(|s| *s != cleaned)
    {
        out.push(last.to_string());
    }
    out
}

/// Strip the quoting a literal import path arrives wrapped in.
fn trim_literal(raw: &str) -> &str {
    raw.trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '<' || c == '>')
        .trim()
}

// --------------------------------------------------------------- traversal

/// How many symbols a single traversal will visit before it stops and says so.
///
/// Reverse reachability over a monorepo is unbounded in principle — a utility
/// everything calls reaches everything — and the answer stops being useful long
/// before it stops growing. The budget is what keeps peak memory flat as the
/// corpus grows, which is the property `tests/measure.rs` asserts, and a
/// truncated answer says it was truncated rather than pretending to be whole.
pub const MAX_NODES: usize = 2000;

/// The edge kinds that mean "depends on", which are the only ones a path is
/// about.
///
/// `defines` and `contains` are structural: they say a symbol sits inside a
/// file or another symbol. True, and useless here — every symbol is reached in
/// one hop from the file it lives in, so including them would make a path
/// between two unrelated functions in one file look like a two-hop
/// dependency. Neighbours still show them, because "what is in this" is a
/// question someone asks.
pub const DEPENDENCY_KINDS: [&str; 3] = ["calls", "imports", "references"];

fn dependency_kinds() -> Vec<String> {
    DEPENDENCY_KINDS.iter().map(|k| k.to_string()).collect()
}

/// One edge of a path, as the path finder renders it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Step {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub confidence: String,
}

/// What points at a symbol, and what it points at.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Neighbours {
    pub callers: Vec<crate::store::EdgeEnd>,
    pub callees: Vec<crate::store::EdgeEnd>,
}

/// One hop in each direction around `name`.
pub fn neighbours(db: &rusqlite::Connection, name: &str, kinds: &[String]) -> Result<Neighbours> {
    Ok(Neighbours {
        callers: crate::store::edges_in(db, name, kinds)?,
        callees: crate::store::edges_out(db, name, kinds)?,
    })
}

/// The shortest chain of edges from `from` to `to`, if there is one.
///
/// Breadth-first, so the first path found is a shortest one. `None` means the
/// two are not connected within `depth` — which is an answer, not a failure,
/// and is reported as one.
pub fn shortest_path(
    db: &rusqlite::Connection,
    from: &str,
    to: &str,
    depth: u32,
) -> Result<Option<Vec<Step>>> {
    if from == to {
        return Ok(Some(Vec::new()));
    }
    // name -> the step that first reached it, for walking the chain back.
    let mut came_from: std::collections::HashMap<String, Step> = std::collections::HashMap::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    seen.insert(from.to_string());
    let mut frontier = vec![from.to_string()];
    let kinds = dependency_kinds();

    for _ in 0..depth {
        let mut next = Vec::new();
        for current in &frontier {
            for edge in crate::store::edges_out(db, current, &kinds)? {
                let name = edge.symbol.name.clone();
                if !seen.insert(name.clone()) {
                    continue;
                }
                came_from.insert(
                    name.clone(),
                    Step {
                        from: current.clone(),
                        to: name.clone(),
                        kind: edge.kind,
                        confidence: edge.confidence,
                    },
                );
                if name == to {
                    return Ok(Some(unwind(&came_from, from, to)));
                }
                next.push(name);
                if seen.len() >= MAX_NODES {
                    return Ok(None);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(None)
}

/// The graph a page draws, scoped so it is drawable.
///
/// The payload shape is the contract between the store and any renderer, and
/// is deliberately boring: `nodes` with an id, name, kind, path, lines and
/// store; `edges` with `from`/`to` as indexes into `nodes`, a kind and a
/// confidence; and `total`/`shown` so a truncated drawing can say so rather
/// than quietly looking complete.
///
/// Scope is one of: a symbol and its one-hop neighbourhood, a path prefix, or
/// a whole store — in that order of preference. There is no "everything"
/// scope, because a force layout over a monorepo is neither drawable nor
/// readable, and pretending otherwise just produces a hairball.
pub fn scoped(
    stores: &[(&str, &crate::Semlith)],
    focus: Option<&str>,
    prefix: Option<&str>,
    limit: usize,
    label_stores: bool,
) -> Result<serde_json::Value> {
    let mut nodes: Vec<crate::store::SymbolRow> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut edges: Vec<serde_json::Value> = Vec::new();
    let mut total = 0usize;

    for (label, store) in stores {
        let db = store.db();
        let picked = match focus {
            Some(name) => {
                // The symbol itself, then what touches it. Deduplicated by the
                // caller, and cut at the budget: a name like `new` is called
                // from everywhere, and drawing all of it is a knot rather than
                // a neighbourhood. The rail lists every caller and callee
                // either way, so nothing is hidden by this — only undrawn.
                let mut around = crate::store::symbols_named(db, name, 4)?;
                for end in crate::store::edges_in(db, name, &[])?
                    .into_iter()
                    .chain(crate::store::edges_out(db, name, &[])?)
                {
                    if around.len() >= limit {
                        break;
                    }
                    around.push(end.symbol);
                }
                around
            }
            None => crate::store::symbols_scoped(db, prefix, limit)?,
        };
        total += crate::store::graph_stats(db)?.0 as usize;

        for mut symbol in picked {
            if nodes.len() >= limit {
                break;
            }
            if index.contains_key(&symbol.name) {
                continue;
            }
            if label_stores {
                symbol.store = Some((*label).to_string());
            }
            index.insert(symbol.name.clone(), nodes.len());
            nodes.push(symbol);
        }
    }

    // Only edges whose both ends are drawn. An edge to something off-canvas is
    // not a line anyone can follow.
    for (from_name, from_index) in &index {
        for (_, store) in stores {
            for end in crate::store::edges_out(store.db(), from_name, &[])? {
                let Some(to_index) = index.get(&end.symbol.name) else {
                    continue;
                };
                if from_index == to_index {
                    continue;
                }
                edges.push(serde_json::json!({
                    "from": from_index,
                    "to": to_index,
                    "kind": end.kind,
                    "confidence": end.confidence,
                }));
            }
        }
    }

    Ok(serde_json::json!({
        "nodes": nodes,
        "edges": edges,
        "total": total,
        "shown": nodes.len(),
    }))
}

fn unwind(
    came_from: &std::collections::HashMap<String, Step>,
    start: &str,
    end: &str,
) -> Vec<Step> {
    let mut chain = Vec::new();
    let mut cursor = end.to_string();
    while cursor != start {
        let Some(step) = came_from.get(&cursor) else {
            break;
        };
        chain.push(step.clone());
        cursor = step.from.clone();
    }
    chain.reverse();
    chain
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn run(name: &str, src: &str) -> Extraction {
        extract(&PathBuf::from(name), src)
            .expect("extraction does not fail")
            .expect("the extension carries a grammar")
    }

    fn has_edge(e: &Extraction, from: &str, to: &str, kind: &str) -> bool {
        e.edges
            .iter()
            .any(|x| x.from == from && x.to == to && x.kind == kind)
    }

    fn confidence_of(e: &Extraction, to: &str, kind: &str) -> String {
        e.edges
            .iter()
            .find(|x| x.to == to && x.kind == kind)
            .map(|x| x.confidence.clone())
            .unwrap_or_else(|| panic!("no {kind} edge to {to} in {:?}", e.edges))
    }

    #[test]
    fn a_file_with_no_grammar_yields_nothing_rather_than_failing() {
        assert!(
            extract(&PathBuf::from("notes.md"), "# hi")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rust_carries_every_edge_kind_it_can_express() {
        let e = run(
            "lock.rs",
            "use std::fs::File;\n\
             struct Guard;\n\
             impl Guard {\n    fn open(&self) { File::create(\"x\"); helper(); }\n}\n\
             fn helper() {}\n",
        );
        assert!(e.symbols.iter().any(|s| s.name == "Guard"));
        assert!(e.symbols.iter().any(|s| s.name == "open"));
        assert!(has_edge(&e, "lock", "helper", "defines"), "{:?}", e.edges);
        assert!(has_edge(&e, "open", "helper", "calls"), "{:?}", e.edges);
        assert!(
            e.edges.iter().any(|x| x.kind == "imports"),
            "the supplementary query supplies imports: {:?}",
            e.edges
        );
    }

    /// The two halves of the confidence column, in one file: `File` is
    /// imported by name, `helper` is not.
    #[test]
    fn an_imported_name_is_extracted_and_a_bare_one_is_inferred() {
        let e = run(
            "a.rs",
            "use std::fs::File;\nfn go() { File::create(\"x\"); helper(); }\nfn helper() {}\n",
        );
        assert_eq!(confidence_of(&e, "File", "calls"), EXTRACTED);
        assert_eq!(confidence_of(&e, "helper", "calls"), INFERRED);
    }

    /// A nested definition hangs off its parent, not off the file.
    #[test]
    fn nesting_produces_contains_rather_than_defines() {
        let e = run("m.py", "class Box:\n    def open(self):\n        pass\n");
        assert!(has_edge(&e, "Box", "open", "contains"), "{:?}", e.edges);
        assert!(has_edge(&e, "m", "Box", "defines"), "{:?}", e.edges);
    }

    #[test]
    fn typescript_calls_come_from_the_supplement() {
        let e = run(
            "app.ts",
            "import {a} from './m';\nfunction f(){ helper(); obj.go(); }\nfunction helper(){}\n",
        );
        assert!(has_edge(&e, "f", "helper", "calls"), "{:?}", e.edges);
        assert!(has_edge(&e, "f", "go", "calls"), "{:?}", e.edges);
        assert!(e.edges.iter().any(|x| x.kind == "imports"));
    }

    #[test]
    fn c_calls_and_includes_come_from_the_supplement() {
        let e = run(
            "main.c",
            "#include <stdio.h>\nint helper(){ return 1; }\nint main(){ return helper(); }\n",
        );
        assert!(has_edge(&e, "main", "helper", "calls"), "{:?}", e.edges);
        assert!(
            e.edges
                .iter()
                .any(|x| x.kind == "imports" && x.to == "stdio.h"),
            "{:?}",
            e.edges
        );
    }

    #[test]
    fn python_go_and_java_each_yield_symbols_and_edges() {
        let py = run(
            "m.py",
            "import os\ndef go():\n    helper()\ndef helper():\n    pass\n",
        );
        assert!(has_edge(&py, "go", "helper", "calls"), "{:?}", py.edges);
        assert!(py.edges.iter().any(|x| x.kind == "imports" && x.to == "os"));

        let go = run(
            "m.go",
            "package m\nimport \"fmt\"\nfunc helper() {}\nfunc Run() { helper() }\n",
        );
        assert!(has_edge(&go, "Run", "helper", "calls"), "{:?}", go.edges);
        assert!(
            go.edges
                .iter()
                .any(|x| x.kind == "imports" && x.to == "fmt")
        );

        let java = run(
            "A.java",
            "import java.util.List;\nclass A { void go(){ helper(); } void helper(){} }\n",
        );
        assert!(has_edge(&java, "go", "helper", "calls"), "{:?}", java.edges);
        assert!(java.edges.iter().any(|x| x.kind == "imports"));
        assert!(has_edge(&java, "A", "go", "contains"), "{:?}", java.edges);
    }

    /// Every language reachable from `LANGUAGES` really parses, so the About
    /// page's list cannot claim a language the binary does not carry.
    #[test]
    fn every_advertised_language_has_a_working_grammar() {
        for lang in LANGUAGES {
            let (language, tags) = grammar(lang).expect("advertised language has a grammar");
            let mut parser = Parser::new();
            parser
                .set_language(&language)
                .unwrap_or_else(|e| panic!("{lang} grammar is ABI-incompatible: {e}"));
            Query::new(&language, tags)
                .unwrap_or_else(|e| panic!("{lang} bundled tags query does not compile: {e:?}"));
            let supplement = supplement(lang);
            if !supplement.is_empty() {
                Query::new(&language, supplement)
                    .unwrap_or_else(|e| panic!("{lang} supplement does not compile: {e:?}"));
            }
        }
    }

    /// A file being saved mid-edit is the normal case under the watcher.
    #[test]
    fn a_file_that_does_not_parse_yields_what_it_could_read() {
        let e = run("a.rs", "fn good() {}\nfn broken( {\n");
        assert!(e.symbols.iter().any(|s| s.name == "good"));
    }

    #[test]
    fn recursion_does_not_produce_a_self_edge() {
        let e = run("a.rs", "fn loops() { loops(); }\n");
        assert!(!has_edge(&e, "loops", "loops", "calls"), "{:?}", e.edges);
    }

    // ------------------------------------------------------------ traversal

    /// A chain `a -> b -> c -> d`, plus `e -> d`, in one in-memory store.
    fn chain() -> rusqlite::Connection {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::prepare_for_tests(&db);
        let file = crate::store::insert_file(&db, "a.rs", "h", 1, 0).unwrap();
        let mut id = std::collections::HashMap::new();
        for name in ["a", "b", "c", "d", "e", "lonely"] {
            let symbol = Symbol {
                kind: "function".to_string(),
                name: name.to_string(),
                qualified: name.to_string(),
                start_line: 1,
                end_line: 2,
            };
            id.insert(
                name,
                crate::store::insert_symbol(&db, file, None, &symbol).unwrap(),
            );
        }
        for (from, to) in [("a", "b"), ("b", "c"), ("c", "d"), ("e", "d")] {
            crate::store::insert_edge(&db, id[from], to, "calls", EXTRACTED).unwrap();
        }
        db
    }

    #[test]
    fn neighbours_separates_the_two_directions() {
        let db = chain();
        let n = neighbours(&db, "d", &[]).unwrap();
        let mut callers: Vec<&str> = n.callers.iter().map(|e| e.symbol.name.as_str()).collect();
        callers.sort_unstable();
        assert_eq!(callers, ["c", "e"], "both callers of d");
        assert!(n.callees.is_empty(), "d calls nothing");
    }

    /// A cycle terminates rather than looping: the traversal marks a name seen
    /// before it expands it, so d -> a -> b -> c -> d ends.
    #[test]
    fn a_cycle_terminates_instead_of_looping() {
        let db = chain();
        let file = crate::store::insert_file(&db, "b.rs", "h", 1, 0).unwrap();
        let symbol = Symbol {
            kind: "function".to_string(),
            name: "d".to_string(),
            qualified: "d".to_string(),
            start_line: 1,
            end_line: 2,
        };
        let d = crate::store::insert_symbol(&db, file, None, &symbol).unwrap();
        crate::store::insert_edge(&db, d, "a", "calls", EXTRACTED).unwrap();
        // d -> a -> b -> c -> d is now a cycle; a large depth must still return.
        let path = shortest_path(&db, "a", "d", 50).unwrap();
        assert!(path.is_some(), "a still reaches d");
        assert!(
            path.unwrap().len() <= 4,
            "a cycle inflated the shortest path"
        );
    }

    #[test]
    fn the_shortest_path_is_the_short_one_and_names_every_edge() {
        let db = chain();
        let path = shortest_path(&db, "a", "d", 10)
            .unwrap()
            .expect("a reaches d");
        let hops: Vec<(&str, &str)> = path
            .iter()
            .map(|s| (s.from.as_str(), s.to.as_str()))
            .collect();
        assert_eq!(hops, [("a", "b"), ("b", "c"), ("c", "d")]);
        assert!(path.iter().all(|s| s.kind == "calls"));
        assert!(path.iter().all(|s| s.confidence == EXTRACTED));
    }

    /// Not connected is an answer, and so is not connected *within this depth*.
    #[test]
    fn an_unconnected_pair_returns_nothing_rather_than_erroring() {
        let db = chain();
        assert!(shortest_path(&db, "a", "lonely", 10).unwrap().is_none());
        assert!(
            shortest_path(&db, "a", "d", 2).unwrap().is_none(),
            "d is three hops away, so a depth of two does not reach it"
        );
    }
}
