//! Structure half of the store: symbols, and the edges between them.
//!
//! Extraction is tree-sitter over a file's text, and it hangs off the same
//! changed-file path that drives re-embedding (see [`crate::Semlith::index_set`]).
//! That is the whole freshness story: there is no build step, no separate
//! command, and no artifact that can be out of date with the corpus, because
//! the pass that re-embeds a file is the pass that re-extracts it.
//!
//! Every language in [`crate::filter::LANGUAGES`] carries edges — all
//! forty-six, since 0.17.0 replaced the extractor's own six-language list with
//! a question asked of the grammar table. A row with no grammar is named in
//! [`WITHOUT_GRAMMAR`] with the reason; it is searchable exactly as before and
//! simply has no rows here.
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
pub const KINDS: [&str; 6] = [
    "defines",
    "calls",
    "imports",
    "references",
    "contains",
    "aliases",
];

/// Stored on the edge: the syntax tree said where the target came from.
pub const EXTRACTED: &str = "extracted";
/// Stored on the edge: the target was matched by bare name alone.
pub const INFERRED: &str = "inferred";
/// Computed at query time: the name had several definitions and the hint,
/// the source file, or its imports picked exactly one of them.
pub const RESOLVED: &str = "resolved";
/// Computed at query time: the name had several definitions and nothing in
/// the source narrowed it to one. An answer that crosses one of these is a
/// guess, and every surface says so.
pub const AMBIGUOUS: &str = "ambiguous";

/// Every confidence value a renderer can be handed, strongest first.
///
/// Two are written to `edges.confidence` and two are decided when the edge is
/// read, because a stored `resolved` would go stale the moment a second
/// definition of the name was indexed somewhere else.
pub const CONFIDENCES: [&str; 4] = [EXTRACTED, RESOLVED, INFERRED, AMBIGUOUS];

/// How much a confidence value is worth when something has to be ordered by
/// it: lower is better. An unknown value sorts last rather than panicking, so
/// a store written by a newer binary still renders.
pub fn confidence_rank(confidence: &str) -> usize {
    CONFIDENCES
        .iter()
        .position(|c| *c == confidence)
        .unwrap_or(CONFIDENCES.len())
}

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
    /// What the source text said about where the target lives, when it said
    /// anything: the module of a scoped call, the receiver of a method call,
    /// the object of a qualified one. `None` for a bare call, for an import,
    /// and for every structural edge.
    ///
    /// It is a lead, not an address — `self.index.search` yields `index`,
    /// which may name a field, a module or neither. The resolver treats it as
    /// one ranking signal among several rather than as truth.
    pub hint: Option<String>,
    /// The line the reference sat on in the source file.
    ///
    /// Where the call, import or reference was written — not where either
    /// symbol is defined. "Where is X invoked" is a question about this line,
    /// and before 0.16.0 the only answer available was the enclosing symbol's
    /// definition, which can be hundreds of lines away.
    ///
    /// `None` on every row an older binary wrote, which is why the format
    /// version does not move for it.
    pub line: Option<u32>,
}

/// What one file yielded.
#[derive(Debug, Clone, Default)]
pub struct Extraction {
    pub symbols: Vec<Symbol>,
    pub edges: Vec<Edge>,
}

/// Languages in [`crate::filter::LANGUAGES`] that ship without a graph, and why.
///
/// A grammar is refused when its licence is copyleft or cannot be identified:
/// the parser is compiled into this binary, so a GPL or LGPL grammar would take
/// the whole Apache-2.0 crate with it, and a licence nobody can name is treated
/// as the worst case rather than the best. A row here is a deliberate, stated
/// gap; a row missing from both here and the grammar table is a bug, and
/// `every_advertised_language_has_a_grammar` fails for it.
pub const WITHOUT_GRAMMAR: &[(&str, &str)] = &[];

/// Whether the graph covers a language.
///
/// There is one list, [`crate::filter::LANGUAGES`], and one answer to "does
/// this language have a grammar" — this function. Nothing keeps a second copy
/// of the set, because the six-language copy that used to live here drifted
/// from the forty-six the product advertised for four releases.
pub fn has_graph(lang: &str) -> bool {
    grammar(lang).is_some()
}

/// Every language the graph covers, in the table's own order.
pub fn languages() -> Vec<&'static str> {
    crate::filter::LANGUAGES
        .iter()
        .map(|entry| entry.name)
        .filter(|name| has_graph(name))
        .collect()
}

/// The language label for a path, if this release extracts from it.
///
/// Dispatch is by extension and whole filename, through the same
/// [`crate::filter::language_of_path`] the search filter uses, so a file is in
/// exactly one language no matter which half of the product is asking.
pub fn language_of(path: &Path) -> Option<&'static str> {
    let entry = crate::filter::language_of_path(&path.to_string_lossy())?;
    has_graph(entry.name).then_some(entry.name)
}

/// What each grammar's `TAGS_QUERY` does not give us.
///
/// Kept short on purpose: the bundled query is the source of truth for
/// definitions, and this closes the three gaps it has — imports, which no
/// grammar here captures; calls, which TypeScript and C do not; and constants,
/// which almost none of them do.
///
/// A constant is a definition an agent types by name, and without a symbol row
/// the definition lift in `search_preferring` cannot fire for one. That is why
/// `MAX_NODES`, `RRF_K` and `KEY_GRACE` were the identifier questions 0.22.0
/// could not put in the top three while `edges_out` and `DEPENDENCY_KINDS` — a
/// function and an array the Rust query does tag — sat at rank 1. Rust was the
/// only language 0.22.0 fixed, so the same hole was open in the other
/// forty-five; 0.23.0 closes it wherever the language has a construct for a
/// named constant at all. Where a grammar already makes one a symbol under
/// another name — Dart and Zig call it a variable, Kotlin and Swift a property,
/// Python a constant already — nothing is added here, because a second capture
/// over the same span is how one symbol ends up containing itself.
fn supplement(lang: &str) -> &'static str {
    match lang {
        "rust" => {
            r#"
            (const_item name: (identifier) @name) @definition.constant
            (static_item name: (identifier) @name) @definition.constant
            (use_declaration argument: (_) @reference.import)
            ; `use store::Semlith as Store;` — the alias is a symbol of its own
            ; and the path it stands for is what it reaches. Without the edge a
            ; walk stops at `Store`, which is defined nowhere.
            (use_as_clause
              path: (_) @reference.alias
              alias: (_) @name) @definition.alias
            ; A scoped call names the function and the thing it went through.
            ; Only the function earns an edge: `store::edges_out` is a call to
            ; `edges_out`, and `store` is what says *which* `edges_out`. Until
            ; 0.15.0 the path segment was emitted as a second `calls` edge,
            ; which recorded a call the source does not make and put a module
            ; name into every path the finder walked. It is a hint now.
            (call_expression
              function: (scoped_identifier
                path: (identifier) @hint
                name: (identifier) @name)) @reference.call
            ; A longer path, `crate::store::edges_out`: the hint is the segment
            ; nearest the name, because that is the one that names the module.
            (call_expression
              function: (scoped_identifier
                path: (scoped_identifier name: (identifier) @hint)
                name: (identifier) @name)) @reference.call
            ; A method call: `self.flush()`, `store.db()`, `self.index.search()`.
            ; The receiver is the hint, reduced to its last identifier.
            (call_expression
              function: (field_expression
                value: (_) @hint
                field: (field_identifier) @name)) @reference.call
        "#
        }
        // TypeScript's bundled query matches nothing on ordinary unexported
        // declarations, so its definitions are spelled out here rather than
        // inherited.
        "typescript" => {
            r#"
            (import_statement source: (string) @reference.import)
            ; `import { search as find }` and `export { search as find }`.
            (import_specifier
              name: (_) @reference.alias
              alias: (identifier) @name) @definition.alias
            (export_specifier
              name: (_) @reference.alias
              alias: (identifier) @name) @definition.alias
            (function_declaration name: (identifier) @name) @definition.function
            (class_declaration name: (type_identifier) @name) @definition.class
            (interface_declaration name: (type_identifier) @name) @definition.interface
            (method_definition name: (property_identifier) @name) @definition.method
            (variable_declarator
              name: (identifier) @name
              value: [(arrow_function) (function_expression)]) @definition.function
            ; `const MAX_HOLDERS = 4`. The value is spelled out rather than
            ; left as `(_)` so that the arrow function on the line above stays
            ; one symbol: a `const` bound to a function is a function, and
            ; tagging it twice would make `semlith symbol` report two
            ; definitions of it.
            (lexical_declaration
              "const"
              (variable_declarator
                name: (identifier) @name
                value: [
                  (number) (string) (template_string) (true) (false) (null)
                  (object) (array) (identifier) (member_expression)
                  (unary_expression) (binary_expression)
                  (call_expression) (new_expression)
                ])) @definition.constant
            (call_expression function: (identifier) @name) @reference.call
            ; `store.search(...)`, `this.index.search(...)`: the object is the
            ; hint, reduced to its last identifier.
            (call_expression
              function: (member_expression
                object: (_) @hint
                property: (property_identifier) @name)) @reference.call
        "#
        }
        "python" => {
            r#"
            (import_statement name: (_) @reference.import)
            (import_from_statement module_name: (_) @reference.import)
            ; `import numpy as np`, `from x import search as find`.
            (aliased_import
              name: (_) @reference.alias
              alias: (identifier) @name) @definition.alias
            ; `json.loads(...)`, `self.store.search(...)`: the object names the
            ; module or the attribute the call went through.
            (call
              function: (attribute
                object: (_) @hint
                attribute: (identifier) @name)) @reference.call
        "#
        }
        "go" => {
            r#"
            ; `const MaxHolders = 4`, and every name in a parenthesised
            ; `const (...)` block, which is one `const_spec` each.
            (const_declaration (const_spec name: (identifier) @name)) @definition.constant
            (import_spec path: (interpreted_string_literal) @reference.import)
            ; `import fp "path/filepath"` — the alias is what the file then
            ; writes, and the path is what it stands for.
            (import_spec
              name: (package_identifier) @name
              path: (interpreted_string_literal) @reference.alias) @definition.alias
        "#
        }
        "java" => {
            r#"
            (import_declaration (scoped_identifier) @reference.import)
            ; `static final int MAX_HOLDERS = 4;`. Java has no `const`, so a
            ; constant is a field that says `final`, and the modifiers are the
            ; only place that is written.
            ((field_declaration
               (modifiers) @_modifiers
               declarator: (variable_declarator name: (identifier) @name)) @definition.constant
             (#match? @_modifiers "\\bfinal\\b"))
        "#
        }
        // C++ has C's problem and C's fix: the bundled query tags the declarator,
        // so a function's span stopped at its signature, and it captures no
        // calls at all.
        "cpp" => {
            r#"
            (preproc_include path: (_) @reference.import)
            (call_expression function: (identifier) @reference.call)
            (function_definition
              declarator: (function_declarator
                declarator: (identifier) @name)) @definition.function
            ; `#define MAX_HOLDERS 4` and `constexpr int MAX_WAITERS = 8;`.
            ; C++ has both of C's macro and a real constant declaration, and
            ; the qualifier is what tells the second from a plain variable —
            ; `volatile` sits in the same node and is not one.
            (preproc_def name: (identifier) @name) @definition.constant
            ((declaration
               (type_qualifier) @_qualifier
               declarator: (init_declarator declarator: (identifier) @name)) @definition.constant
             (#any-of? @_qualifier "const" "constexpr" "consteval" "constinit"))
        "#
        }
        "csharp" => {
            r#"
            (using_directive (identifier) @reference.import)
            ; `const int MaxHolders = 4;`. A field is a constant only when it
            ; says so, and `static`, `readonly` and the access modifiers all
            ; arrive in the same node.
            ((field_declaration
               (modifier) @_const
               (variable_declaration
                 (variable_declarator name: (identifier) @name))) @definition.constant
             (#eq? @_const "const"))
            (invocation_expression function: (identifier) @name) @reference.call
            ; `store.Search(...)`: the object is the hint.
            (invocation_expression
              function: (member_access_expression
                expression: (_) @hint
                name: (identifier) @name)) @reference.call
        "#
        }
        // Elm's bundled query captures the name in a type annotation as a
        // reference and the body of a declaration not at all, so `acquire =
        // helper` recorded nothing about `acquire` depending on `helper`.
        "elm" => {
            r#"
            (value_declaration
              functionDeclarationLeft: (function_declaration_left
                (lower_case_identifier) @name)) @definition.function
            (value_expr name: (value_qid (lower_case_identifier) @name)) @reference.call
            (import_clause moduleName: (upper_case_qid) @reference.import)
        "#
        }
        "javascript" => {
            r#"
            (import_statement source: (string) @reference.import)
            (import_specifier
              name: (_) @reference.alias
              alias: (identifier) @name) @definition.alias
            (export_specifier
              name: (_) @reference.alias
              alias: (identifier) @name) @definition.alias
            ; `const MAX_HOLDERS = 4`. The bundled query tags only the
            ; `export module.exports = ...` form, so an ordinary top-level
            ; constant reached the graph as nothing at all. The value is
            ; spelled out so that `const f = () => {}` stays a function.
            (lexical_declaration
              "const"
              (variable_declarator
                name: (identifier) @name
                value: [
                  (number) (string) (template_string) (true) (false) (null)
                  (object) (array) (identifier) (member_expression)
                  (unary_expression) (binary_expression)
                  (call_expression) (new_expression)
                ])) @definition.constant
        "#
        }
        "php" => {
            r#"
            (require_expression (string (string_content) @reference.import))
            (include_expression (string (string_content) @reference.import))
            (function_call_expression function: (name) @name) @reference.call
            (member_call_expression
              object: (_) @hint
              name: (name) @name) @reference.call
            ; `const MAX_HOLDERS = 4;`, whether it stands alone or inside a
            ; class. `define('MAX_HOLDERS', 4)` is a function call and stays
            ; one: the name it binds is a string, not an identifier the
            ; grammar can hand back.
            (const_declaration (const_element (name) @name)) @definition.constant
        "#
        }
        "swift" => {
            r#"
            (import_declaration (identifier) @reference.import)
            (call_expression (simple_identifier) @name) @reference.call
            (call_expression
              (navigation_expression
                target: (_) @hint
                suffix: (navigation_suffix (simple_identifier) @name))) @reference.call
        "#
        }
        "c" => {
            r#"
            (preproc_include path: (_) @reference.import)
            (call_expression function: (identifier) @reference.call)
            ; C's bundled query tags the declarator, so a function's span stops
            ; at its signature and every call inside its body was attributed to
            ; the file instead of to the function that makes it. This spans the
            ; whole definition; the narrower duplicate is dropped by the
            ; contained-definition rule below.
            (function_definition
              declarator: (function_declarator
                declarator: (identifier) @name)) @definition.function
            ; `#define MAX_HOLDERS 4`. This is C's named constant, and not a
            ; stylistic alternative to one: a `const int` is a read-only
            ; object rather than a constant expression, and cannot size an
            ; array or label a case.
            (preproc_def name: (identifier) @name) @definition.constant
        "#
        }
        "ruby" => {
            r#"
            ; Ruby has no `const` keyword. A name that begins with a capital
            ; is a constant, and the grammar says so by parsing it as
            ; `constant` rather than `identifier` — which is why this needs no
            ; predicate and no naming convention of our own.
            (assignment left: (constant) @name) @definition.constant
        "#
        }
        "elixir" => {
            r#"
            ; `@max_holders 4`. A module attribute is the nearest thing
            ; Elixir has to a constant, and the grammar has no node for one:
            ; it is `@` applied to a call with no parentheses. The attributes
            ; the language reserves for documentation and typespecs go
            ; through the same syntax and are not constants.
            ((unary_operator
               "@"
               operand: (call target: (identifier) @name (arguments))) @definition.constant
             (#not-any-of? @name
               "moduledoc" "doc" "typedoc" "spec" "type" "typep" "opaque"
               "callback" "macrocallback" "behaviour" "impl" "deprecated"
               "derive" "enforce_keys" "compile" "dialyzer" "on_definition"
               "before_compile" "after_compile" "external_resource"))
        "#
        }
        "lua" => {
            r#"
            ; `local MAX_HOLDERS <const> = 4`. Lua 5.4's attribute is the only
            ; thing in the language that makes a binding a constant; a local
            ; without one is reassignable and is not.
            ((variable_declaration
               (assignment_statement
                 (variable_list
                   name: (identifier) @name
                   attribute: (attribute (identifier) @_attribute)))) @definition.constant
             (#eq? @_attribute "const"))
        "#
        }
        _ => "",
    }
}

/// The grammar for a language, for a caller outside the extractor.
///
/// `semlith pattern` compiles its own query against the same grammar the
/// extractor uses, read from the same table, so the two halves cannot disagree
/// about what "rust" is.
pub fn language_grammar(lang: &str) -> Option<Language> {
    grammar(lang).map(|(language, _)| language)
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

        // The forty languages 0.17.0 added. Twenty-three of their crates ship a
        // tags query written for `tree-sitter tags`, which is the same
        // arrangement the six above have. The other seventeen ship none, so
        // the query is written here under `queries/<language>/tags.scm` in the
        // same capture vocabulary, and the extractor cannot tell the two apart.
        "clojure" => Some((
            tree_sitter_clojure_orchard::LANGUAGE.into(),
            include_str!("../queries/clojure/tags.scm"),
        )),
        "cpp" => Some((
            tree_sitter_cpp::LANGUAGE.into(),
            tree_sitter_cpp::TAGS_QUERY,
        )),
        "csharp" => Some((
            tree_sitter_c_sharp::LANGUAGE.into(),
            tree_sitter_c_sharp::TAGS_QUERY,
        )),
        "dart" => Some((
            tree_sitter_dart::LANGUAGE.into(),
            tree_sitter_dart::TAGS_QUERY,
        )),
        "dockerfile" => Some((
            tree_sitter_containerfile::LANGUAGE.into(),
            include_str!("../queries/dockerfile/tags.scm"),
        )),
        "elixir" => Some((
            tree_sitter_elixir::LANGUAGE.into(),
            tree_sitter_elixir::TAGS_QUERY,
        )),
        "elm" => Some((
            tree_sitter_elm::LANGUAGE.into(),
            tree_sitter_elm::TAGS_QUERY,
        )),
        "erlang" => Some((
            tree_sitter_erlang::LANGUAGE.into(),
            include_str!("../queries/erlang/tags.scm"),
        )),
        "fortran" => Some((
            tree_sitter_fortran::LANGUAGE.into(),
            include_str!("../queries/fortran/tags.scm"),
        )),
        "graphql" => Some((
            tree_sitter_graphql::LANGUAGE.into(),
            include_str!("../queries/graphql/tags.scm"),
        )),
        "groovy" => Some((
            tree_sitter_groovy::LANGUAGE.into(),
            include_str!("../queries/groovy/tags.scm"),
        )),
        "javascript" => Some((
            tree_sitter_javascript::LANGUAGE.into(),
            tree_sitter_javascript::TAGS_QUERY,
        )),
        "lua" => Some((
            tree_sitter_lua::LANGUAGE.into(),
            tree_sitter_lua::TAGS_QUERY,
        )),
        "nix" => Some((
            tree_sitter_nix::LANGUAGE.into(),
            include_str!("../queries/nix/tags.scm"),
        )),
        "ocaml" => Some((
            tree_sitter_ocaml::LANGUAGE_OCAML.into(),
            tree_sitter_ocaml::TAGS_QUERY,
        )),
        "php" => Some((
            tree_sitter_php::LANGUAGE_PHP.into(),
            tree_sitter_php::TAGS_QUERY,
        )),
        "powershell" => Some((
            tree_sitter_powershell::LANGUAGE.into(),
            include_str!("../queries/powershell/tags.scm"),
        )),
        "proto" => Some((
            tree_sitter_proto::LANGUAGE.into(),
            include_str!("../queries/proto/tags.scm"),
        )),
        "r" => Some((tree_sitter_r::LANGUAGE.into(), tree_sitter_r::TAGS_QUERY)),
        "ruby" => Some((
            tree_sitter_ruby::LANGUAGE.into(),
            tree_sitter_ruby::TAGS_QUERY,
        )),
        "sql" => Some((
            tree_sitter_sequel::LANGUAGE.into(),
            include_str!("../queries/sql/tags.scm"),
        )),
        "swift" => Some((
            tree_sitter_swift::LANGUAGE.into(),
            tree_sitter_swift::TAGS_QUERY,
        )),
        "vue" => Some((
            tree_sitter_vue_next::LANGUAGE.into(),
            include_str!("../queries/vue/tags.scm"),
        )),
        "css" => Some((
            tree_sitter_css::LANGUAGE.into(),
            include_str!("../queries/css/tags.scm"),
        )),
        "haskell" => Some((
            tree_sitter_haskell::LANGUAGE.into(),
            include_str!("../queries/haskell/tags.scm"),
        )),
        "html" => Some((
            tree_sitter_html::LANGUAGE.into(),
            include_str!("../queries/html/tags.scm"),
        )),
        "json" => Some((
            tree_sitter_json::LANGUAGE.into(),
            include_str!("../queries/json/tags.scm"),
        )),
        "julia" => Some((
            tree_sitter_julia::LANGUAGE.into(),
            include_str!("../queries/julia/tags.scm"),
        )),
        "kotlin" => Some((
            tree_sitter_kotlin_ng::LANGUAGE.into(),
            include_str!("../queries/kotlin/tags.scm"),
        )),
        "makefile" => Some((
            tree_sitter_make::LANGUAGE.into(),
            include_str!("../queries/makefile/tags.scm"),
        )),
        "markdown" => Some((
            tree_sitter_md::LANGUAGE.into(),
            include_str!("../queries/markdown/tags.scm"),
        )),
        "objective-c" => Some((
            tree_sitter_objc::LANGUAGE.into(),
            include_str!("../queries/objective-c/tags.scm"),
        )),
        "perl" => Some((
            ts_parser_perl::LANGUAGE.into(),
            include_str!("../queries/perl/tags.scm"),
        )),
        "scala" => Some((
            tree_sitter_scala::LANGUAGE.into(),
            include_str!("../queries/scala/tags.scm"),
        )),
        "shell" => Some((
            tree_sitter_bash::LANGUAGE.into(),
            include_str!("../queries/shell/tags.scm"),
        )),
        "svelte" => Some((
            tree_sitter_svelte_ng::LANGUAGE.into(),
            include_str!("../queries/svelte/tags.scm"),
        )),
        "terraform" => Some((
            tree_sitter_hcl::LANGUAGE.into(),
            include_str!("../queries/terraform/tags.scm"),
        )),
        "toml" => Some((
            tree_sitter_toml_ng::LANGUAGE.into(),
            include_str!("../queries/toml/tags.scm"),
        )),
        "yaml" => Some((
            tree_sitter_yaml::LANGUAGE.into(),
            include_str!("../queries/yaml/tags.scm"),
        )),
        "zig" => Some((
            tree_sitter_zig::LANGUAGE.into(),
            include_str!("../queries/zig/tags.scm"),
        )),

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
    line: u32,
    hint: Option<String>,
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

    // Two queries can tag the same definition at two widths: a bundled query
    // that tags the declarator and a supplement that tags the whole definition
    // are both right about where the symbol is and disagree about where it
    // ends. The wider one is the useful one, because a reference inside the
    // body is only attributed to the symbol whose range covers it — C attributed
    // every call in a function body to the file until this rule existed.
    //
    // Same name *and* same kind is the condition, so a `mod x` containing an
    // `fn x`, or a Java class containing its own constructor, keeps both.
    let contained: Vec<bool> = defs
        .iter()
        .map(|inner| {
            defs.iter().any(|outer| {
                outer.name == inner.name
                    && outer.kind == inner.kind
                    && (outer.start, outer.end) != (inner.start, inner.end)
                    && outer.start <= inner.start
                    && outer.end >= inner.end
            })
        })
        .collect();
    let mut keep = contained.iter();
    defs.retain(|_| !keep.next().copied().unwrap_or(false));

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
                hint: None,
                // A structural edge has no call site, so it carries the line
                // the definition itself starts on rather than nothing: that is
                // where the source says the containment happens.
                line: Some(def.start_line),
            }),
            None => edges.push(Edge {
                from: module.clone(),
                to: def.name.clone(),
                kind: "defines".to_string(),
                confidence: EXTRACTED.to_string(),
                hint: None,
                line: Some(def.start_line),
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
        // The file said where this came from, one way or the other: it
        // imported the name itself (`use ...::edges_out;` then `edges_out()`),
        // or it imported the thing the call went through (`use ...::store;`
        // then `store::edges_out()`). The second is exactly as determined as
        // the first — the module the call names is in the file's own imports —
        // and reading it as inferred is what left Rust at 2% extracted while
        // the source was perfectly explicit.
        let named_by_file = reference.kind == "imports"
            || imported.contains(&reference.name)
            || reference
                .hint
                .as_ref()
                .is_some_and(|h| imported.contains(h));
        let confidence = if named_by_file { EXTRACTED } else { INFERRED };
        edges.push(Edge {
            from,
            to: reference.name.clone(),
            kind: reference.kind.to_string(),
            confidence: confidence.to_string(),
            hint: reference.hint.clone(),
            line: Some(reference.line),
        });
    }

    // One call can be captured by both the bundled query and the supplement,
    // and only the supplement carries a hint. Deduplicating on the triple
    // alone would keep whichever landed first; ordering by confidence and then
    // by whether a hint is present makes the best-informed copy of each edge
    // the one that survives.
    edges.sort_by(|a, b| {
        (
            &a.from,
            &a.to,
            &a.kind,
            confidence_rank(&a.confidence),
            a.hint.is_none(),
            &a.hint,
            a.line.is_none(),
            a.line,
        )
            .cmp(&(
                &b.from,
                &b.to,
                &b.kind,
                confidence_rank(&b.confidence),
                b.hint.is_none(),
                &b.hint,
                b.line.is_none(),
                b.line,
            ))
    });
    edges.dedup_by(|a, b| (&a.from, &a.to, &a.kind) == (&b.from, &b.to, &b.kind));

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
        let mut hint: Option<tree_sitter::Node> = None;
        let mut references: Vec<(&str, tree_sitter::Node)> = Vec::new();
        for capture in m.captures() {
            let capture_name = names[capture.index as usize];
            if let Some(kind) = capture_name.strip_prefix("definition.") {
                span = Some((kind, capture.node));
            } else if capture_name == "name" {
                name = Some(capture.node);
            } else if capture_name == "hint" {
                hint = Some(capture.node);
            } else if let Some(kind) = capture_name.strip_prefix("reference.") {
                references.push((kind, capture.node));
            }
        }

        for (kind, node) in references {
            let kind = match kind {
                "call" => "calls",
                "import" => "imports",
                "alias" => "aliases",
                _ => "references",
            };
            // An import is a literal path and is kept whole. Anything else
            // takes the match's `@name` when it has one, and otherwise
            // descends into whatever node the grammar chose to capture —
            // Rust's query hands back `helper()` for a call and an entire
            // `impl` block for an implementation.
            let target = if kind == "imports" {
                Some(trim_literal(&text[node.byte_range()]).to_string())
            } else if kind == "aliases" {
                // The captured node is the path the alias stands for, and
                // `edges.dst` is a bare name, so `store::Semlith` has to become
                // `Semlith` before it can resolve against anything. `@name`
                // here is the alias, not the target, which is why this does not
                // fall through to the branch below.
                last_segment(&text[node.byte_range()])
            } else {
                match name {
                    Some(n) => Some(text[n.byte_range()].to_string()),
                    None => reference_name(node, text),
                }
            };
            let Some(target) = target.filter(|t| !t.is_empty()) else {
                continue;
            };
            // A macro invocation is recorded under the name the source writes.
            //
            // `format!(...)` is a reference to the macro `format!`, not a call
            // to a function called `format`, and the two are different names.
            // Conflating them made every function that formats a string look
            // like a caller of `store::format`: 229 of them on semlith's own
            // store, against three real ones. The grammar's tags query cannot
            // tell them apart because it captures the identifier without its
            // `!`, so the distinction is restored here.
            let target = if in_macro(node) {
                format!("{target}!")
            } else {
                target
            };
            refs.push(Ref {
                name: target,
                kind,
                at: node.start_byte(),
                line: node.start_position().row as u32 + 1,
                hint: hint.and_then(|h| hint_text(h, text)),
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

/// Whether a captured reference is (or sits directly inside) a macro
/// invocation.
///
/// Only the node itself and its immediate parents are checked, because a
/// grammar captures either the invocation or the identifier naming it, and
/// climbing further would swallow every ordinary call that happens to appear
/// inside a macro's arguments — `format!("{}", helper())` really does call
/// `helper`.
fn in_macro(node: tree_sitter::Node) -> bool {
    const MACRO_KINDS: [&str; 2] = ["macro_invocation", "macro_expression"];
    if MACRO_KINDS.contains(&node.kind()) {
        return true;
    }
    node.parent()
        .is_some_and(|parent| MACRO_KINDS.contains(&parent.kind()))
}

/// The lead a `@hint` capture carries, or `None` when it carries none worth
/// storing.
///
/// The capture is whatever stood to the left of the call — `store`, `self`,
/// `self.index`, `crate::store`, `this.client` — so it is reduced to its last
/// identifier the same way a reference is. Three of them are dropped rather
/// than stored: `self`, `this` and `super` name the file the call is already
/// in, so they rank nothing, and `crate` names the whole tree.
fn hint_text(node: tree_sitter::Node, text: &str) -> Option<String> {
    const USELESS: [&str; 5] = ["self", "this", "super", "crate", "cls"];
    let name = reference_name(node, text)?;
    if name.is_empty() || USELESS.contains(&name.as_str()) {
        return None;
    }
    Some(name)
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

/// The bare name at the end of a path an alias stands for.
///
/// `store::Semlith` is `Semlith`, `"path/filepath"` is `filepath`, `x.y.search`
/// is `search`. `edges.dst` is a name rather than a path, so an alias that kept
/// its path would resolve against nothing.
fn last_segment(raw: &str) -> Option<String> {
    let cleaned = trim_literal(raw);
    cleaned
        .rsplit(['/', '.', ':'])
        .find(|s| !s.is_empty())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
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
/// `aliases` is here from 0.16.0 for the opposite reason to `defines`: a
/// re-export or an aliased import is the only thing standing between the name
/// a caller wrote and the definition it meant, so a walk that refuses to cross
/// one reports "not connected" about code that is connected. It is a
/// dependency in the sense that matters — following it lands on real code.
pub const DEPENDENCY_KINDS: [&str; 4] = ["calls", "imports", "references", "aliases"];

pub fn dependency_kinds() -> Vec<String> {
    DEPENDENCY_KINDS.iter().map(|k| k.to_string()).collect()
}

/// Symbol kinds nothing can point *at*.
///
/// A Markdown heading, a YAML or TOML or JSON key, a CSS selector and an HTML
/// element are navigational: they are what a document is searched by, and they
/// are worth being symbols for exactly that. Nothing references them. No
/// `calls` edge has ever meant a heading.
///
/// That distinction has to be enforced where an edge's target is resolved,
/// because `edges.dst` is a *name* and resolution is a join on it. Without it,
/// 0.17.0's forty new languages put 1 427 navigational symbols into this
/// repository's own store — 933 configuration keys, 264 CSS selectors, 230
/// headings — and 44 of them collided with the name of a real function:
/// `path`, `query`, `symbol`, `graph`, `shape`, `forget`, `error`, `version`.
/// Every one of those names became *ambiguous*, the walk refuses to cross an
/// ambiguous name, and the harness measured the result: hit@1 13 to 10, hit@8
/// 27 to 23 on an unchanged question set.
///
/// They stay symbols. `semlith symbol` finds them, a locate answer names the
/// heading a hit sits under, and `semlith stats` counts them. They simply
/// cannot be what an edge meant.
pub const NAVIGATIONAL_KINDS: [&str; 4] = ["heading", "key", "selector", "element"];

/// The same list as a SQL `NOT IN` list, for the two joins that resolve a name.
pub fn not_navigational(column: &str) -> String {
    let holes = NAVIGATIONAL_KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{column} NOT IN ({holes})")
}

/// How much of a symbol's score flows on to its neighbours each round.
///
/// PageRank's own default, and there is no reason here to disagree with it.
/// The 0.15 that does not flow is what keeps the walk anchored to the chunks
/// the query actually found — the whole difference between a personalised
/// PageRank and a plain one, which would rank the repository's most-called
/// utility first for every query ever asked.
const DAMPING: f32 = 0.85;

/// How many rounds the score is pushed outward from the seeds.
///
/// Three carries mass two hops out and leaves a third of it there. Each extra
/// round costs another pass over the frontier and moves the ordering less than
/// the one before it.
const ROUNDS: usize = 3;

/// Rank the neighbourhood of a set of seed symbols by personalised PageRank.
///
/// `personal` is the seed mass per symbol name — for search, the fusion
/// contribution of the chunks each name was found in. `neighbours` is asked for
/// one name's dependency edges at a time, as `(name, weight, confidence)`, and
/// is asked at most once per name however many rounds run: the walk holds the
/// frontier it has visited and never the whole graph, which is what keeps peak
/// memory flat as the corpus grows.
///
/// Returns the reached names, best first, with the confidence of the best edge
/// that reached each. A name the walk never reached through an edge is not in
/// the result, however much seed mass it started with: the third list is about
/// what the code says, and a seed nothing points at is only what the query
/// said.
///
/// Why this rather than the flat hop it replaces: one hop treats every
/// neighbour of every seed as equally related, so a symbol reached once from a
/// weak seed ranks with one reached from three strong ones. That is not a
/// ranking, it is a set.
///
/// Every map here is a `BTreeMap` rather than a `HashMap`, and that is the
/// whole of issue #88. A `HashMap`'s iteration order is seeded randomly per
/// process, so `for (name, mass) in &score` visited the frontier in a
/// different order on every run. Three things followed. The mass arriving at a
/// name is a sum of `f32`s, and floating-point addition is not associative, so
/// the same walk produced slightly different scores and the ranking moved by a
/// question between identical runs. `tiers` keeps the first edge of equal
/// weight, so the confidence a name was labelled with depended on which edge
/// arrived first. And `edges.len() >= MAX_NODES` cut off whichever names the
/// order had not reached yet, so the set of names visited — and the graph-only
/// denominator the harness reports — moved as well. An ordered map costs a
/// `log n` lookup over a set already bounded by `MAX_NODES` and buys a walk
/// that answers the same thing twice.
pub fn expand(
    personal: &std::collections::BTreeMap<String, f32>,
    mut neighbours: impl FnMut(&str) -> Result<Vec<(String, f32, String)>>,
) -> Result<Vec<(String, f32, String)>> {
    if personal.is_empty() {
        return Ok(Vec::new());
    }

    let mut edges: std::collections::BTreeMap<String, Vec<(String, f32, String)>> =
        std::collections::BTreeMap::new();
    // The best edge that reached each name, which is what it is labelled with.
    let mut tiers: std::collections::BTreeMap<String, (f32, String)> =
        std::collections::BTreeMap::new();
    let mut score = personal.clone();

    for _ in 0..ROUNDS {
        let mut next: std::collections::BTreeMap<String, f32> = personal
            .iter()
            .map(|(name, mass)| (name.clone(), (1.0 - DAMPING) * mass))
            .collect();

        for (name, mass) in &score {
            if *mass <= 0.0 {
                continue;
            }
            if !edges.contains_key(name) {
                // The budget is on names visited, not on rows read, because
                // that is what bounds both the queries and the memory.
                if edges.len() >= MAX_NODES {
                    continue;
                }
                edges.insert(name.clone(), neighbours(name)?);
            }
            let out = &edges[name];
            let total: f32 = out.iter().map(|(_, weight, _)| *weight).sum();
            if total <= 0.0 {
                continue;
            }
            for (to, weight, confidence) in out {
                *next.entry(to.clone()).or_default() += DAMPING * mass * weight / total;
                match tiers.get(to) {
                    Some((best, _)) if *best >= *weight => {}
                    _ => {
                        tiers.insert(to.clone(), (*weight, confidence.clone()));
                    }
                }
            }
        }
        score = next;
    }

    // Reached through an edge, which `tiers` is the record of. A seed the walk
    // never left and came back to is not a neighbour of anything and carries no
    // confidence to label it with; a seed that *is* reached — the query matched
    // a chunk and the code points at it as well — stays, because the graph
    // really did find it and the badge on the hit says so.
    let mut ranked: Vec<(String, f32, String)> = score
        .into_iter()
        .filter_map(|(name, mass)| {
            let (weight, confidence) = tiers.get(&name)?.clone();
            Some((name, mass, (weight, confidence)))
        })
        .map(|(name, mass, (_, confidence))| (name, mass, confidence))
        .collect();
    // Ties break by name so two runs over one store return one order.
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(ranked)
}

/// One edge of a path, as the path finder renders it.
///
/// Both ends carry a file and a line, and that is not decoration. A hop
/// between two names says almost nothing when either name has several
/// definitions: `index -> first_store` is true of some `index`, and until the
/// row says which one, a reader cannot tell whether the chain holds together
/// or quietly changed subject halfway through.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Step {
    pub from: String,
    pub from_path: String,
    pub from_line: u32,
    pub to: String,
    pub to_path: String,
    pub to_line: u32,
    pub kind: String,
    pub confidence: String,
    /// How many definitions of `to` the store holds.
    pub definitions: usize,
    /// The line the call was written on, in `from_path`.
    ///
    /// `None` for an edge written before 0.16.0 and for one whose source file
    /// has not been re-indexed since — the same absence `EdgeEnd::line` has,
    /// carried through rather than guessed at, because the enclosing
    /// definition's line is a different line and often far away.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_line: Option<u32>,
}

/// A chain, with what it is worth.
///
/// Named `Chain` rather than `Path` because this module already works with
/// `std::path::Path` on every line that reads a file.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Chain {
    pub steps: Vec<Step>,
    pub summary: Summary,
}

/// What the chain is made of, counted rather than described.
///
/// Every renderer prints this and none of them computes it, so the CLI, the
/// MCP reply and the portal cannot drift into saying different things about
/// one answer.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Summary {
    pub hops: usize,
    pub extracted: usize,
    pub resolved: usize,
    pub inferred: usize,
    pub ambiguous: usize,
    /// Joins where the chain left one definition of a name and picked up at
    /// another. A seam is not a hop — it is the place two hops were welded
    /// together, and the weld is the part that may not hold.
    pub seams: usize,
    /// The names welded at those seams, with how many definitions each has.
    pub ambiguous_names: Vec<(String, usize)>,
    /// Whether the honest word for this chain is "hypothesis".
    ///
    /// True when the chain has a seam, or when nothing on it was better than
    /// a bare-name match. Either way the renderers say so in one line rather
    /// than leaving a reader to work it out from the badges.
    pub hypothesis: bool,
}

impl Summary {
    /// Read the chain and count it.
    fn of(steps: &[Step]) -> Self {
        let mut summary = Summary {
            hops: steps.len(),
            ..Default::default()
        };
        for step in steps {
            match step.confidence.as_str() {
                EXTRACTED => summary.extracted += 1,
                RESOLVED => summary.resolved += 1,
                AMBIGUOUS => summary.ambiguous += 1,
                _ => summary.inferred += 1,
            }
        }
        // A seam sits between two hops: the first arrives at one definition of
        // a name and the second leaves from another.
        for pair in steps.windows(2) {
            let (before, after) = (&pair[0], &pair[1]);
            if (&before.to_path, before.to_line) == (&after.from_path, after.from_line) {
                continue;
            }
            summary.seams += 1;
            let name = before.to.clone();
            if !summary.ambiguous_names.iter().any(|(n, _)| *n == name) {
                summary
                    .ambiguous_names
                    .push((name, before.definitions.max(2)));
            }
        }
        summary.hypothesis = summary.seams > 0
            || (!steps.is_empty()
                && steps
                    .iter()
                    .all(|s| s.confidence == INFERRED || s.confidence == AMBIGUOUS));
        summary
    }
}

/// What points at a symbol, and what it points at.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Neighbours {
    pub callers: Vec<crate::store::EdgeEnd>,
    pub callees: Vec<crate::store::EdgeEnd>,
    /// Targets the store holds no definition for, listed only when asked.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<crate::store::Unresolved>,
    /// How many of those were left out. Non-zero only when they were, so a
    /// reader is told the list is short rather than left to assume it is
    /// complete.
    #[serde(skip_serializing_if = "is_zero")]
    pub hidden: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// One row per name an edge points at, rather than one row per definition that
/// name could mean.
///
/// Only ambiguous rows are folded. A resolved edge already names one
/// definition, and two resolved edges to the same name from two different
/// definitions of the source are two real calls.
pub fn collapse(callees: Vec<crate::store::EdgeEnd>) -> Vec<crate::store::EdgeEnd> {
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    callees
        .into_iter()
        .filter(|end| {
            if end.confidence != AMBIGUOUS {
                return true;
            }
            seen.insert((end.symbol.name.clone(), end.kind.clone()))
        })
        .collect()
}

impl Chain {
    /// The chain as every surface prints it.
    ///
    /// One renderer, called by the CLI and by the MCP reply, so the two cannot
    /// drift into describing one answer differently. The portal draws the same
    /// four parts from the same JSON.
    ///
    /// `bold` and `reset` are the terminal's escapes, or empty strings for a
    /// surface that has none.
    pub fn render(&self, bold: &str, reset: &str, shorten: &dyn Fn(&str) -> String) -> String {
        let mut out = String::new();
        let width = self
            .steps
            .iter()
            .map(|s| {
                endpoint(&s.from, &shorten(&s.from_path), s.from_line)
                    .chars()
                    .count()
            })
            .max()
            .unwrap_or(0);

        for (i, step) in self.steps.iter().enumerate() {
            let left = endpoint(&step.from, &shorten(&step.from_path), step.from_line);
            let right = endpoint(&step.to, &shorten(&step.to_path), step.to_line);
            let pad = " ".repeat(width.saturating_sub(left.chars().count()));
            out.push_str(&format!(
                "{bold}{}{reset}  {left}{pad}  → {right}  {} · {}\n",
                i + 1,
                step.kind,
                step.confidence,
            ));
            // The seam sits between two hops, because that is where it is: the
            // chain arrived at one definition of a name and leaves from
            // another. Drawing it as a hop of its own, as the first sketch of
            // this did, inflates the hop count and hides the join.
            if let Some(next) = self.steps.get(i + 1)
                && (&step.to_path, step.to_line) != (&next.from_path, next.from_line)
            {
                out.push_str(&format!(
                    "   ── seam · {}: {} definitions · continues from {}\n",
                    step.to,
                    step.definitions.max(2),
                    endpoint(&next.from, &shorten(&next.from_path), next.from_line),
                ));
            }
        }

        out.push_str(&self.trailer());
        out.push('\n');
        if self.summary.hypothesis {
            out.push_str("A hypothesis, not a finding.\n");
        }
        out
    }

    /// The counted line under the chain.
    ///
    /// Every number on it is read off the steps, so it cannot disagree with
    /// what was printed above it.
    pub fn trailer(&self) -> String {
        let s = &self.summary;
        // The four counts sum to the hop count, always. A trailer whose parts
        // do not add up sends the reader looking for the hop it left out.
        let mut line = format!(
            "{} hop{} · {} extracted · {} resolved · {} inferred · {} ambiguous",
            s.hops,
            if s.hops == 1 { "" } else { "s" },
            s.extracted,
            s.resolved,
            s.inferred,
            s.ambiguous,
        );
        if s.seams > 0 {
            let names = s
                .ambiguous_names
                .iter()
                .map(|(name, count)| format!("{name}: {count} definitions"))
                .collect::<Vec<_>>()
                .join(", ");
            line.push_str(&format!(" · {} through ambiguous names ({names})", s.seams));
        }
        line
    }
}

/// Leave a path exactly as the store recorded it.
///
/// What a surface with nothing better to do passes to [`Chain::render`]. The
/// CLI passes something that strips the working directory, because a chain of
/// six absolute paths is six copies of one prefix and one useful suffix.
pub fn verbatim(path: &str) -> String {
    path.to_string()
}

/// Where the call, import or reference this edge records was actually written.
///
/// Empty when the edge carries no line — every row written before 0.16.0, and
/// every row whose source file has not been re-indexed since. A renderer says
/// nothing rather than pointing at the enclosing definition and calling it a
/// call site, because those are different lines and often far apart.
///
/// The two shapes are not cosmetic. From [`crate::store::edges_out`] the row's
/// own `symbol` is the *target*, so the call sits in a different file and has
/// to be named. From [`crate::store::edges_in`] the row's `symbol` is the
/// caller itself and its path is already on the line, so only the line number
/// is new.
pub fn call_site(end: &crate::store::EdgeEnd, shorten: &dyn Fn(&str) -> String) -> String {
    let Some(line) = end.line else {
        return String::new();
    };
    match &end.from_path {
        Some(path) => format!("  · called at {}:{line}", shorten(path)),
        None => format!("  · call at line {line}"),
    }
}

/// `name @ path:line`, or the bare name when the store could not say where.
fn endpoint(name: &str, path: &str, line: u32) -> String {
    if path.is_empty() {
        return name.to_string();
    }
    format!("{name} @ {path}:{line}")
}

/// What to say when there is no chain.
///
/// The two sentences are different answers and must not be confused. Without
/// `all_edges` the search refused to cross names it could not pin down, so the
/// honest report is that nothing *it was willing to walk* connects the two —
/// and it names the flag that widens the question. With `all_edges` it walked
/// everything and still found nothing, which is as close to "no" as this tool
/// gets.
pub fn not_connected(from: &str, to: &str, depth: u32, all_edges: bool) -> String {
    if all_edges {
        format!("{from} and {to} are not connected within {depth} hops by any edge in the store.")
    } else {
        format!(
            "{from} and {to} are not connected within {depth} hops by resolved edges. \
             Ambiguous names were not crossed; --all-edges walks them and labels what it finds."
        )
    }
}

/// One hop in each direction around `name`.
///
/// `all` widens the answer in the two ways it is narrow. Without it, callees
/// to a name with several definitions collapse to one row carrying the count:
/// four rows saying `get` are four different functions, and printing them as
/// four callees says this symbol calls `get` four times, which it does not.
/// And without it, edges pointing outside the corpus are left out, as they
/// always have been — with a count now, so a reader knows the list is short.
///
/// Callers are untouched in both states: an inbound edge came from a symbol
/// id, so it is one definite place in one definite file.
pub fn neighbours(
    db: &rusqlite::Connection,
    name: &str,
    kinds: &[String],
    all: bool,
) -> Result<Neighbours> {
    let callees = crate::store::edges_out(db, name, kinds)?;
    let unresolved = crate::store::unresolved_out(db, name, kinds)?;
    Ok(Neighbours {
        callers: crate::store::edges_in(db, name, kinds)?,
        callees: if all { callees } else { collapse(callees) },
        hidden: if all { 0 } else { unresolved.len() },
        unresolved: if all { unresolved } else { Vec::new() },
    })
}

/// How many second-ring names an evidence block will name.
///
/// The ego graph is context, not an answer: past a dozen it stops telling a
/// reader where they are and starts costing them the tokens they came to save.
const EGO_LIMIT: usize = 12;

/// One name two hops from the centre, and the first-ring name it came through.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Hop {
    /// The first-ring name this was reached through.
    pub via: String,
    pub name: String,
    pub kind: String,
    pub confidence: String,
    /// `caller` or `callee`, relative to `via`.
    pub direction: &'static str,
}

/// Everything the store knows about one symbol, in one reply.
///
/// An agent asking "what is `record_retrieval`" wanted the definition, who
/// calls it, what it calls, and roughly where it sits — and before 0.16.0 that
/// was three tool calls and three replies, two of which it had to make before
/// it knew whether the first was the right symbol.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Evidence {
    pub name: String,
    pub definitions: Vec<crate::store::SymbolRow>,
    pub callers: Vec<crate::store::EdgeEnd>,
    pub callees: Vec<crate::store::EdgeEnd>,
    /// The second ring, reached only through names the first ring settled.
    pub ego: Vec<Hop>,
}

/// The definition, the first ring and the second ring, in one pass.
///
/// The second ring is walked only through first-ring names that are
/// `extracted` or `resolved`. An ambiguous name is several unrelated
/// definitions wearing one label, and expanding through one would put
/// somebody else's callers into this symbol's context — the same wrong answer
/// the path finder used to give, in a different shape.
pub fn evidence(
    db: &rusqlite::Connection,
    name: &str,
    kinds: &[String],
    limit: usize,
    all: bool,
) -> Result<Evidence> {
    let definitions = crate::store::symbols_named(db, name, limit)?;
    let ring = neighbours(db, name, kinds, all)?;

    let mut ego: Vec<Hop> = Vec::new();
    let settled: Vec<(&str, &crate::store::EdgeEnd)> = ring
        .callers
        .iter()
        .map(|e| ("caller", e))
        .chain(ring.callees.iter().map(|e| ("callee", e)))
        .filter(|(_, e)| e.confidence == EXTRACTED || e.confidence == RESOLVED)
        .collect();

    for (_, first) in &settled {
        if ego.len() >= EGO_LIMIT {
            break;
        }
        let out = crate::store::edges_out(db, &first.symbol.name, kinds)?;
        let into = crate::store::edges_in(db, &first.symbol.name, kinds)?;
        for (direction, end) in out
            .into_iter()
            .map(|e| ("callee", e))
            .chain(into.into_iter().map(|e| ("caller", e)))
        {
            if ego.len() >= EGO_LIMIT {
                break;
            }
            // The centre is not two hops from itself, and a name already in
            // the first ring is context the reader has.
            if end.symbol.name == name
                || ring
                    .callers
                    .iter()
                    .chain(ring.callees.iter())
                    .any(|e| e.symbol.name == end.symbol.name)
                || ego.iter().any(|h| h.name == end.symbol.name)
            {
                continue;
            }
            ego.push(Hop {
                via: first.symbol.name.clone(),
                name: end.symbol.name,
                kind: end.kind,
                confidence: end.confidence,
                direction,
            });
        }
    }

    Ok(Evidence {
        name: name.to_string(),
        definitions,
        callers: ring.callers,
        callees: ring.callees,
        ego,
    })
}

impl Evidence {
    /// The block as every surface prints it.
    ///
    /// One renderer for the CLI and the MCP reply, for the reason the path
    /// finder has one: two hand-written copies is how the same answer ends up
    /// described two different ways.
    pub fn render(&self, bold: &str, reset: &str, shorten: &dyn Fn(&str) -> String) -> String {
        let mut out = String::new();
        for symbol in &self.definitions {
            out.push_str(&format!(
                "{bold}{}{reset} {}  {}:{}-{}\n",
                symbol.name,
                symbol.kind,
                shorten(&symbol.path),
                symbol.start_line,
                symbol.end_line,
            ));
        }
        if self.definitions.len() > 1 {
            out.push_str(&format!(
                "{} definitions of this name\n",
                self.definitions.len()
            ));
        }
        for (heading, ends) in [("callers", &self.callers), ("callees", &self.callees)] {
            out.push_str(&format!("\n{bold}{heading}{reset} ({})\n", ends.len()));
            if ends.is_empty() {
                out.push_str("  none\n");
            }
            for end in ends.iter() {
                if end.confidence == AMBIGUOUS {
                    out.push_str(&format!(
                        "  {} via {} ({}) · {} definitions{}\n",
                        end.symbol.name,
                        end.kind,
                        end.confidence,
                        end.definitions,
                        call_site(end, shorten),
                    ));
                    continue;
                }
                out.push_str(&format!(
                    "  {} via {} ({})  {}:{}{}\n",
                    end.symbol.name,
                    end.kind,
                    end.confidence,
                    shorten(&end.symbol.path),
                    end.symbol.start_line,
                    call_site(end, shorten),
                ));
            }
        }
        if !self.ego.is_empty() {
            out.push_str(&format!("\n{bold}two hops out{reset}\n"));
            for hop in &self.ego {
                out.push_str(&format!(
                    "  {} · {} of {} ({})\n",
                    hop.name, hop.direction, hop.via, hop.confidence,
                ));
            }
        }
        out.trim_end().to_string()
    }
}

/// The shortest chain of edges from `from` to `to`, if there is one.
///
/// Breadth-first, so the first path found is a shortest one. `None` means the
/// two are not connected within `depth` by the edges this search was allowed
/// to walk — which is an answer, not a failure, and is reported as one.
///
/// # What it will not walk
///
/// `all_edges` is false by default, and a search in that state refuses to
/// cross an `ambiguous` name: one whose several definitions the source gave it
/// no way to choose between. This is the release's central correction. The
/// 0.14.0 finder crossed those names silently, so `call_tool` reached
/// `record_retrieval` through a `record` that is two unrelated functions, and
/// printed the result in exactly the format a real chain prints in. A wrong
/// answer that looks like a right one is worse than no answer, so by default
/// there is no answer.
///
/// With `all_edges` the old behaviour is available and labelled: the chain
/// comes back with its seams marked, its confidences counted, and the sentence
/// saying it is a hypothesis.
///
/// Within a depth, better-supported edges are expanded first, so a name
/// reachable both ways is reached by the edge worth more. Without that the
/// answer would depend on the order SQLite returned rows in.
pub fn shortest_path(
    db: &rusqlite::Connection,
    from: &str,
    to: &str,
    depth: u32,
    all_edges: bool,
) -> Result<Option<Chain>> {
    if from == to {
        return Ok(Some(Chain {
            steps: Vec::new(),
            summary: Summary::default(),
        }));
    }
    // A node is a definition, not a name. Two functions called `search` are
    // two nodes, and a chain that arrives at one of them may only leave from
    // that one.
    //
    // This is the whole correction. Refusing `ambiguous` edges is not enough
    // on its own: on the semlith store every hop of `call_tool ->
    // record_retrieval` resolves to exactly one definition, and the chain is
    // still false, because hop 3 arrives at `search` in `lib.rs` and hop 4
    // leaves from `search` in `routes.rs`. Each hop is true. The chain is not.
    let mut came_from: std::collections::HashMap<Node, Step> = std::collections::HashMap::new();
    let mut seen: std::collections::HashSet<Node> = std::collections::HashSet::new();
    // The start is every definition of the name, because "does anything called
    // `call_tool` reach this" is the question that was asked.
    let start = Node::any(from);
    seen.insert(start.clone());
    let mut frontier = vec![start];
    let kinds = dependency_kinds();

    for _ in 0..depth {
        // The whole level's edges, ordered by what they are worth, before any
        // of them is taken. Ordering within one node would still let a weak
        // edge out of the first node beat a strong edge out of the second.
        let mut level: Vec<(Node, crate::store::EdgeEnd)> = Vec::new();
        for current in &frontier {
            for edge in crate::store::edges_out(db, &current.name, &kinds)? {
                if !all_edges {
                    if edge.confidence == AMBIGUOUS {
                        continue;
                    }
                    // The edge has to leave from the definition the chain
                    // actually reached. `edges_out` is asked about a name and
                    // answers for every definition of it, which is right for
                    // "what does this name call" and wrong for "what does
                    // *this* function call".
                    if !current.holds(edge.from_path.as_deref(), edge.from_line) {
                        continue;
                    }
                }
                level.push((current.clone(), edge));
            }
        }
        level.sort_by_key(|(_, edge)| confidence_rank(&edge.confidence));

        let mut next = Vec::new();
        for (current, edge) in level {
            // Strict walks definitions; `all_edges` walks names, which is what
            // makes a seam possible and therefore visible. Keying the two the
            // same way would quietly repair the chains this flag exists to
            // show you.
            let reached = if all_edges {
                Node::any(&edge.symbol.name)
            } else {
                Node {
                    name: edge.symbol.name.clone(),
                    path: edge.symbol.path.clone(),
                    line: edge.symbol.start_line,
                }
            };
            if !seen.insert(reached.clone()) {
                continue;
            }
            came_from.insert(
                reached.clone(),
                Step {
                    from: current.name.clone(),
                    // An edge always knows which definition it leaves from.
                    // The fallback is for a row an older store cannot supply,
                    // and it says so rather than inventing a line.
                    from_path: edge.from_path.unwrap_or_default(),
                    from_line: edge.from_line.unwrap_or(0),
                    to: reached.name.clone(),
                    to_path: edge.symbol.path,
                    to_line: edge.symbol.start_line,
                    kind: edge.kind,
                    confidence: edge.confidence,
                    definitions: edge.definitions,
                    call_line: edge.line,
                },
            );
            if reached.name == to {
                let steps = unwind(&came_from, from, &reached);
                let summary = Summary::of(&steps);
                return Ok(Some(Chain { steps, summary }));
            }
            next.push(reached);
            if seen.len() >= MAX_NODES {
                return Ok(None);
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(None)
}

/// One definition, as the traversal addresses it.
///
/// `path` empty means "any definition of this name", which is what the start
/// of a search is and nothing else ever is.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Node {
    name: String,
    path: String,
    line: u32,
}

impl Node {
    fn any(name: &str) -> Self {
        Node {
            name: name.to_string(),
            path: String::new(),
            line: 0,
        }
    }

    /// Whether an edge leaving `(path, line)` leaves from this definition.
    ///
    /// A node with no path is the start of the search and holds every
    /// definition. An edge with no recorded source is one an older store
    /// wrote, and is allowed rather than silently dropped: the store predates
    /// the column, so refusing it would turn every pre-0.15.0 store's path
    /// finder off.
    fn holds(&self, path: Option<&str>, line: Option<u32>) -> bool {
        if self.path.is_empty() {
            return true;
        }
        match (path, line) {
            (Some(path), Some(line)) => path == self.path && line == self.line,
            _ => true,
        }
    }
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
/// The most connected symbol across these stores, for a view nobody has scoped.
///
/// One candidate per store — `symbols_scoped` already orders by degree — and
/// then the one with the most edges of the few that came back. Cheap: a
/// handful of candidates, one edge lookup each.
/// How many candidates the opening view considers before it picks one.
///
/// The first by raw degree is almost always a name like `new`, `get` or `run`:
/// defined in forty places, called from everywhere, and connected to none of it
/// in a way a reader can follow. Looking at a handful of candidates is what
/// makes it possible to prefer a symbol whose edges mean something.
const SEED_CANDIDATES: usize = 40;

/// The symbol the graph opens on when nobody has asked for one.
///
/// Not the busiest. A corpus's busiest name is its most *reused* name, and a
/// name defined in forty files earns its degree from forty unrelated callers
/// that the extractor could only match by spelling — so the first thing a user
/// saw on a freshly indexed project was a star of dashed `inferred` lines
/// between functions that have nothing to do with each other. That is a true
/// picture of a name collision and a useless picture of a codebase.
///
/// So the seed is the symbol with the most *resolved* edges — the ones where
/// the extractor knew which definition was meant — and a name with several
/// definitions is preferred only when nothing unambiguous has any edges at all.
/// The busiest name is still one search away; it is just not what the page
/// opens on.
fn busiest(stores: &[(&str, &crate::Semlith)]) -> Result<Option<String>> {
    let mut best: Option<((usize, usize, usize), String)> = None;
    for (_, store) in stores {
        let db = store.db();
        for row in crate::store::symbols_scoped(db, None, SEED_CANDIDATES)? {
            let ends: Vec<crate::store::EdgeEnd> = crate::store::edges_in(db, &row.name, &[])?
                .into_iter()
                .chain(crate::store::edges_out(db, &row.name, &[])?)
                .collect();
            if ends.is_empty() {
                continue;
            }
            let resolved = ends
                .iter()
                .filter(|e| e.confidence == RESOLVED || e.confidence == EXTRACTED)
                .count();
            // Three keys, in this order. A name defined once in the store
            // cannot be a collision, whatever its degree; among those, the one
            // whose edges the extractor actually resolved; and only then raw
            // degree. The last key is what keeps a store with nothing but
            // inferred edges -- a dynamic language with no import graph --
            // drawing the same thing it drew before rather than nothing at all.
            //
            // The ambiguity that matters is the focus symbol's own. `len` and
            // `new` are defined in dozens of files, so every edge into them is
            // a spelling match between functions that have nothing to do with
            // each other -- which is exactly the star of dashed lines a user
            // met on opening the page.
            let unambiguous =
                usize::from(crate::store::symbols_named(db, &row.name, 2)?.len() <= 1);
            let score = (unambiguous, resolved, ends.len());
            if best.as_ref().is_none_or(|(seen, _)| score > *seen) {
                best = Some((score, row.name.clone()));
            }
        }
    }
    Ok(best.map(|(_, name)| name))
}

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

    // With nothing asked for, the busiest symbol's neighbourhood rather than
    // the busiest `limit` symbols.
    //
    // Only edges with both ends drawn are drawn, and the most connected
    // symbols in a corpus are hubs that each point at many different leaves
    // and almost never at one another — so "the top two hundred by degree"
    // came back as two hundred boxes and nought edges. A field of dots is
    // technically the graph and of no use to anyone looking at it.
    let chosen_focus = match (focus, prefix) {
        (None, None) => busiest(stores)?,
        (focus, _) => focus.map(str::to_string),
    };
    let focus = chosen_focus.as_deref();

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
                let mut ends: Vec<crate::store::EdgeEnd> = crate::store::edges_in(db, name, &[])?
                    .into_iter()
                    .chain(crate::store::edges_out(db, name, &[])?)
                    .collect();
                // The certain edges first. When the budget cuts a hub's
                // neighbourhood -- and it always does -- what is drawn should be
                // the calls the extractor resolved rather than whichever
                // spelling matches happened to sort first. A canvas of dashed
                // `inferred` lines is a picture of a name, not of a program.
                ends.sort_by_key(|end| match end.confidence.as_str() {
                    EXTRACTED | RESOLVED => 0,
                    AMBIGUOUS => 1,
                    _ => 2,
                });
                for end in ends {
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
    //
    // Deduplicated across stores. Every store open is asked about every drawn
    // name, so a symbol two stores both know produced the same edge twice —
    // and a 20-node subgraph reported 836 edges where the whole 43-node graph
    // reported 623, which cannot both be true and is not a number anyone can
    // read.
    let mut seen: std::collections::HashSet<(usize, usize, String)> = Default::default();
    for (from_name, from_index) in &index {
        for (_, store) in stores {
            for end in crate::store::edges_out(store.db(), from_name, &[])? {
                let Some(to_index) = index.get(&end.symbol.name) else {
                    continue;
                };
                if from_index == to_index {
                    continue;
                }
                if !seen.insert((*from_index, *to_index, end.kind.clone())) {
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

fn unwind(came_from: &std::collections::HashMap<Node, Step>, start: &str, end: &Node) -> Vec<Step> {
    let mut chain = Vec::new();
    let mut cursor = end.clone();
    while cursor.name != start {
        // The precise definition first, then the name: a strict walk keys its
        // nodes by definition and an `all_edges` walk keys them by name.
        let Some(step) = came_from
            .get(&cursor)
            .or_else(|| came_from.get(&Node::any(&cursor.name)))
        else {
            break;
        };
        chain.push(step.clone());
        cursor = Node {
            name: step.from.clone(),
            path: step.from_path.clone(),
            line: step.from_line,
        };
        // The first hop leaves from the search's own start, which holds every
        // definition of its name and is keyed that way.
        if cursor.name == start {
            break;
        }
    }
    chain.reverse();
    chain
}

/// How many reached definitions one impact answer will carry.
///
/// Reverse reachability fans out faster than a path does: a utility three
/// hops below `main` reaches most of the tree, and a list that long is a
/// scroll rather than an answer. Past this the answer says how many it left
/// out instead of printing them.
pub const IMPACT_LIMIT: usize = 400;

/// One definition that reaches the symbol asked about.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Reached {
    pub name: String,
    #[serde(serialize_with = "crate::serialize_plain")]
    pub path: String,
    pub line: u32,
    /// Hops from the symbol asked about. One is a direct caller.
    pub hop: u32,
    /// The edge that reached it, and how much that edge is worth.
    pub kind: String,
    pub confidence: String,
    /// The name this one reaches, one hop nearer the centre.
    pub via: String,
}

/// One file that reaches the symbol, and how closely.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReachedFile {
    #[serde(serialize_with = "crate::serialize_plain")]
    pub path: String,
    /// Definitions in this file that reach the symbol.
    pub symbols: usize,
    /// The fewest hops any of them takes.
    pub nearest: u32,
}

/// Everything that reaches one symbol, to a bounded number of hops.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Impact {
    pub name: String,
    pub depth: u32,
    /// Whether ambiguous edges were walked.
    pub all_edges: bool,
    /// The definitions of the name itself, so a reader knows which symbol
    /// this is about when there is more than one.
    pub definitions: Vec<crate::store::SymbolRow>,
    pub reached: Vec<Reached>,
    pub files: Vec<ReachedFile>,
    /// How many reached definitions were left out at [`IMPACT_LIMIT`].
    #[serde(skip_serializing_if = "is_zero")]
    pub hidden: usize,
    /// Counted the same way a chain's summary is counted, so the page and the
    /// terminal cannot disagree about what the answer is made of.
    pub extracted: usize,
    pub resolved: usize,
    pub inferred: usize,
    pub ambiguous: usize,
}

/// Everything that reaches `name`, breadth first, up to `depth` hops.
///
/// The mirror of [`shortest_path`] and it walks the same graph under the same
/// rule: a node is a definition, not a name, and an ambiguous edge is not
/// crossed unless `all_edges` says to. Breadth first, so the hop recorded
/// against a definition is the fewest hops it takes — a caller that reaches
/// the symbol both directly and through three others is a direct caller.
pub fn impact(
    db: &rusqlite::Connection,
    name: &str,
    kinds: &[String],
    depth: u32,
    all_edges: bool,
) -> Result<Impact> {
    // Dependency edges by default, for the reason `shortest_path` walks only
    // those: every symbol is one hop from the file that defines it, so a
    // `defines` edge is true and says nothing about who would notice a
    // change. A caller that names kinds explicitly gets exactly those.
    let dependencies = dependency_kinds();
    let kinds = if kinds.is_empty() {
        dependencies.as_slice()
    } else {
        kinds
    };
    let definitions = crate::store::symbols_named(db, name, 16)?;
    let mut answer = Impact {
        name: name.to_string(),
        depth,
        all_edges,
        definitions,
        reached: Vec::new(),
        files: Vec::new(),
        hidden: 0,
        extracted: 0,
        resolved: 0,
        inferred: 0,
        ambiguous: 0,
    };

    // Seen by definition, so two functions sharing a name are two nodes, and
    // the centre itself is never reported as reaching itself.
    let mut seen: std::collections::HashSet<Node> = std::collections::HashSet::new();
    seen.insert(Node::any(name));
    for row in &answer.definitions {
        seen.insert(Node {
            name: row.name.clone(),
            path: row.path.clone(),
            line: row.start_line,
        });
    }

    let mut frontier: Vec<String> = vec![name.to_string()];
    for hop in 1..=depth {
        let mut next: Vec<String> = Vec::new();
        for target in &frontier {
            for end in crate::store::edges_in(db, target, kinds)? {
                if !all_edges && end.confidence == AMBIGUOUS {
                    continue;
                }
                let node = Node {
                    name: end.symbol.name.clone(),
                    path: end.symbol.path.clone(),
                    line: end.symbol.start_line,
                };
                if !seen.insert(node) {
                    continue;
                }
                if answer.reached.len() >= IMPACT_LIMIT {
                    answer.hidden += 1;
                    continue;
                }
                match end.confidence.as_str() {
                    EXTRACTED => answer.extracted += 1,
                    RESOLVED => answer.resolved += 1,
                    INFERRED => answer.inferred += 1,
                    _ => answer.ambiguous += 1,
                }
                next.push(end.symbol.name.clone());
                answer.reached.push(Reached {
                    name: end.symbol.name,
                    path: end.symbol.path,
                    line: end.symbol.start_line,
                    hop,
                    kind: end.kind,
                    confidence: end.confidence,
                    via: target.clone(),
                });
            }
        }
        if next.is_empty() {
            break;
        }
        next.sort();
        next.dedup();
        frontier = next;
    }

    // Files, nearest first, then by how much of the file is involved. A file
    // that reaches the symbol four ways at six hops is further away than one
    // that reaches it once at one hop, and reading it in that order is how
    // somebody decides what to open.
    let mut by_file: std::collections::BTreeMap<String, (usize, u32)> =
        std::collections::BTreeMap::new();
    for row in &answer.reached {
        let entry = by_file.entry(row.path.clone()).or_insert((0, row.hop));
        entry.0 += 1;
        entry.1 = entry.1.min(row.hop);
    }
    answer.files = by_file
        .into_iter()
        .map(|(path, (symbols, nearest))| ReachedFile {
            path,
            symbols,
            nearest,
        })
        .collect();
    answer.files.sort_by(|a, b| {
        a.nearest
            .cmp(&b.nearest)
            .then(b.symbols.cmp(&a.symbols))
            .then(a.path.cmp(&b.path))
    });

    Ok(answer)
}

impl Impact {
    /// The whole answer as text, for the terminal and for an agent.
    ///
    /// One renderer, as the chain has one: the CLI and the MCP reply differ
    /// only in whether they are allowed to use bold.
    pub fn render(&self, bold: &str, reset: &str, shorten: &dyn Fn(&str) -> String) -> String {
        let mut out = self.headline();
        if self.reached.is_empty() {
            return out;
        }
        let mut hop = 0;
        for row in &self.reached {
            if row.hop != hop {
                hop = row.hop;
                out.push_str(&format!(
                    "\n\n{bold}{hop} hop{}{reset}",
                    if hop == 1 { "" } else { "s" }
                ));
            }
            out.push_str(&format!(
                "\n  {} {}:{} · {} · {} · reaches {}",
                row.name,
                shorten(&row.path),
                row.line,
                row.kind,
                row.confidence,
                row.via
            ));
        }
        out.push_str(&format!("\n\n{bold}files{reset}"));
        for file in &self.files {
            out.push_str(&format!(
                "\n  {} · {} definition{} · nearest {} hop{}",
                shorten(&file.path),
                file.symbols,
                if file.symbols == 1 { "" } else { "s" },
                file.nearest,
                if file.nearest == 1 { "" } else { "s" }
            ));
        }
        if self.hidden > 0 {
            out.push_str(&format!(
                "\n\n{} more left out at the limit of {IMPACT_LIMIT}",
                self.hidden
            ));
        }
        out
    }

    /// The sentence that says what the answer is, printed by every surface so
    /// none of them writes its own."""
    pub fn headline(&self) -> String {
        if self.reached.is_empty() {
            return format!(
                "nothing in this store reaches {} within {} hop{}",
                self.name,
                self.depth,
                if self.depth == 1 { "" } else { "s" }
            );
        }
        let files = self.files.len();
        format!(
            "{} definition{} in {} file{} reach {} within {} hop{}",
            self.reached.len(),
            if self.reached.len() == 1 { "" } else { "s" },
            files,
            if files == 1 { "" } else { "s" },
            self.name,
            self.depth,
            if self.depth == 1 { "" } else { "s" }
        )
    }
}

/// One hop's supporting line, as evidence rather than as a picture.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Support {
    /// `from → to · kind`, the hop this line is evidence for.
    pub hop: String,
    /// Where the line was written: `path:line`, or empty when the store has
    /// no call line for this edge.
    pub at: String,
    /// The line itself, read from the store's own chunks. Empty when the
    /// store cannot supply it, which is said rather than filled in.
    pub code: String,
    /// `supporting fact` or `candidate — corroborate before use`.
    pub mark: &'static str,
}

/// What a chain is worth, in the shape somebody can paste into a review.
///
/// The Trace panel and `semlith trace` are one answer: the sentence, the
/// chain, and one line of source per hop. Nothing here re-walks the graph —
/// it reads a [`Chain`] the finder already produced, so the two can never
/// disagree about what the path was.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Trace {
    pub from: String,
    pub to: String,
    pub depth: u32,
    pub all_edges: bool,
    pub answer: String,
    pub chain: Option<Chain>,
    pub lines: Vec<Support>,
}

/// The mark a hop's supporting line carries.
///
/// An inferred or ambiguous hop was matched by name and nothing more, so the
/// line under it is a candidate for the reader to check. Anything the source
/// or the ranking settled is a fact.
pub fn support_mark(confidence: &str) -> &'static str {
    match confidence {
        EXTRACTED | RESOLVED => "supporting fact",
        _ => "candidate — corroborate before use",
    }
}

/// Turn a chain into evidence.
///
/// `line_text` reads one line of one file out of the store — passed as a
/// closure for the reason [`expand`] takes one: the shape of the answer is
/// testable against a literal map with no store and no model behind it.
pub fn trace(
    from: &str,
    to: &str,
    depth: u32,
    all_edges: bool,
    chain: Option<Chain>,
    mut line_text: impl FnMut(&str, u32) -> Option<String>,
    shorten: &dyn Fn(&str) -> String,
) -> Trace {
    let Some(chain) = chain else {
        return Trace {
            from: from.to_string(),
            to: to.to_string(),
            depth,
            all_edges,
            answer: not_connected(from, to, depth, all_edges),
            chain: None,
            lines: Vec::new(),
        };
    };

    let s = &chain.summary;
    let answer = if chain.steps.is_empty() {
        format!("{from} is {to}.")
    } else if s.hypothesis && s.seams > 0 {
        format!(
            "Not connected by resolved edges. The nearest chain is {} hop{} long and crosses {} ambiguous name{} — treat it as a hypothesis.",
            s.hops,
            if s.hops == 1 { "" } else { "s" },
            s.seams,
            if s.seams == 1 { "" } else { "s" }
        )
    } else if s.hypothesis {
        format!(
            "Not connected by resolved edges. The nearest chain is {} hop{} long and every hop was matched by bare name — treat it as a hypothesis.",
            s.hops,
            if s.hops == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "Connected in {} hop{}: {} extracted, {} resolved. Every hop names its target through something the extractor read or the ranking settled.",
            s.hops,
            if s.hops == 1 { "" } else { "s" },
            s.extracted,
            s.resolved
        )
    };

    let lines = chain
        .steps
        .iter()
        .map(|step| {
            let (at, code) = match step.call_line {
                Some(line) if !step.from_path.is_empty() => (
                    format!("{}:{line}", shorten(&step.from_path)),
                    line_text(&step.from_path, line).unwrap_or_default(),
                ),
                _ => (String::new(), String::new()),
            };
            Support {
                hop: format!("{} → {} · {}", step.from, step.to, step.kind),
                at,
                code,
                mark: support_mark(&step.confidence),
            }
        })
        .collect();

    Trace {
        from: from.to_string(),
        to: to.to_string(),
        depth,
        all_edges,
        answer,
        chain: Some(chain),
        lines,
    }
}

impl Trace {
    /// The whole trace as text, for the terminal and for an agent.
    pub fn render(&self, bold: &str, reset: &str, shorten: &dyn Fn(&str) -> String) -> String {
        let mut out = format!("{bold}answer{reset}\n{}\n", self.answer);
        let Some(chain) = &self.chain else {
            return out;
        };
        if chain.steps.is_empty() {
            return out;
        }
        out.push_str(&format!("\n{bold}chain{reset}\n"));
        out.push_str(&chain.render(bold, reset, shorten));
        out.push_str(&format!("\n{bold}supporting lines{reset}\n"));
        for line in &self.lines {
            if line.at.is_empty() {
                out.push_str(&format!(
                    "  {}  ·  no call line recorded  [{}]\n",
                    line.hop, line.mark
                ));
                continue;
            }
            out.push_str(&format!("  {}   {}\n", line.at, line.code));
            out.push_str(&format!("    {}  [{}]\n", line.hop, line.mark));
        }
        out
    }

    /// What "Copy as evidence" copies, and what `--evidence` prints.
    ///
    /// Plain text with no colour and no box drawing, because it is going into
    /// a review comment or a commit message rather than onto a terminal.
    pub fn evidence(&self, shorten: &dyn Fn(&str) -> String) -> String {
        let mut out = format!("{} → {}\nanswer: {}\n", self.from, self.to, self.answer);
        let Some(chain) = &self.chain else {
            return out;
        };
        for (i, step) in chain.steps.iter().enumerate() {
            out.push_str(&format!(
                "{}. {} @ {}:{} -> {} @ {}:{}  [{}]",
                i + 1,
                step.from,
                shorten(&step.from_path),
                step.from_line,
                step.to,
                shorten(&step.to_path),
                step.to_line,
                step.confidence,
            ));
            if let Some(next) = chain.steps.get(i + 1)
                && (&step.to_path, step.to_line) != (&next.from_path, next.from_line)
            {
                out.push_str(&format!(
                    "  seam: {}: {} definitions · continues from {} @ {}:{}",
                    step.to,
                    step.definitions.max(2),
                    next.from,
                    shorten(&next.from_path),
                    next.from_line,
                ));
            }
            out.push('\n');
        }
        if !self.lines.is_empty() {
            out.push_str("\nsupporting lines\n");
            for line in &self.lines {
                let mark = if line.mark.starts_with("candidate") {
                    "candidate"
                } else {
                    "supporting fact"
                };
                if line.at.is_empty() {
                    out.push_str(&format!(
                        "{}   no call line recorded   [{mark}]\n",
                        line.hop
                    ));
                } else {
                    out.push_str(&format!("{}   {}   [{mark}]\n", line.at, line.code));
                }
            }
        }
        out
    }
}

/// Most edges the community pass will read out of one store.
///
/// The bound that stands in for the one-hop-at-a-time rule every other
/// traversal follows. Twenty thousand edges is the whole of this repository's
/// own graph several times over, and a corpus past it gets a panel that says
/// how much it left out.
pub const COMMUNITY_EDGES: usize = 20_000;

/// Rounds of label propagation.
///
/// Label propagation converges in a handful of rounds on graphs this shape,
/// and a fixed cap is what makes the answer the same twice — which the panel
/// needs more than it needs the last percent of modularity.
const LABEL_ROUNDS: usize = 8;

/// How many communities the panel lists before it says `Shown N of M`.
pub const COMMUNITIES_SHOWN: usize = 12;

/// One hub of a community: a name and where it is defined.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Hub {
    pub name: String,
    #[serde(serialize_with = "crate::serialize_plain")]
    pub path: String,
    pub line: u32,
}

/// A group of names that call each other more than they call anything else.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Community {
    /// The community's busiest member, which is what it gets called.
    pub label: String,
    pub size: usize,
    /// The three busiest members, by degree inside the community.
    pub hubs: Vec<Hub>,
    /// The strongest edge out of this community: the other community's
    /// label and how many settled edges run to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross: Option<(String, usize)>,
}

/// The communities of a graph, by label propagation.
///
/// Deterministic on purpose: nodes are visited in name order and a tie
/// between two labels is broken by name, so the same store gives the same
/// communities twice. That matters more here than modularity does — a panel
/// that regroups a codebase every time it is opened is not a map.
///
/// Takes edges rather than a database for the reason [`expand`] takes a
/// closure: the maths is tested against a literal edge list with no store
/// behind it.
pub fn communities(edges: &[(String, String)]) -> Vec<Community> {
    use std::collections::{BTreeMap, BTreeSet};

    let mut neighbours: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (from, to) in edges {
        neighbours
            .entry(from.as_str())
            .or_default()
            .insert(to.as_str());
        neighbours
            .entry(to.as_str())
            .or_default()
            .insert(from.as_str());
    }
    if neighbours.is_empty() {
        return Vec::new();
    }

    // Everything starts in a community of its own, and takes the commonest
    // label among its neighbours each round.
    let mut label: BTreeMap<&str, &str> = neighbours.keys().map(|n| (*n, *n)).collect();
    for _ in 0..LABEL_ROUNDS {
        let mut moved = false;
        for (node, around) in &neighbours {
            let mut tally: BTreeMap<&str, usize> = BTreeMap::new();
            for other in around {
                *tally.entry(label[other]).or_default() += 1;
            }
            let Some(&top) = tally.values().max() else {
                continue;
            };
            // A node keeps its own label when that label is among the
            // winners, and only moves when another strictly beats it. Plain
            // label propagation without this collapses: two clusters joined
            // by a single edge become one community, because a tie is
            // resolved by an arbitrary rule rather than by staying put.
            let mine = label[node];
            if tally.get(mine).copied().unwrap_or(0) == top {
                continue;
            }
            // Among strict winners, the name that sorts first, so the result
            // does not depend on iteration order.
            let Some((&best, _)) = tally
                .iter()
                .filter(|(_, n)| **n == top)
                .min_by_key(|(name, _)| **name)
            else {
                continue;
            };
            if mine != best {
                label.insert(node, best);
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }

    let mut members: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (node, group) in &label {
        members.entry(group).or_default().push(node);
    }

    // How many settled edges run between each pair of communities, for the
    // "strongest cross-community edge" line.
    let mut between: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for (from, to) in edges {
        let (a, b) = (label[from.as_str()], label[to.as_str()]);
        if a != b {
            *between.entry((a, b)).or_default() += 1;
        }
    }

    let out: Vec<(&str, Community)> = members
        .into_iter()
        .map(|(group, names)| {
            // Degree inside the community decides the hubs and the label: the
            // name everything else in the group talks to is what the group is.
            let inside: BTreeSet<&str> = names.iter().copied().collect();
            let mut ranked: Vec<(usize, &str)> = names
                .iter()
                .map(|name| {
                    let degree = neighbours
                        .get(name)
                        .map(|around| around.iter().filter(|o| inside.contains(*o)).count())
                        .unwrap_or(0);
                    (degree, *name)
                })
                .collect();
            ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
            let cross = between
                .iter()
                .filter(|((a, _), _)| *a == group)
                .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.1.cmp(a.0.1)))
                .map(|((_, b), n)| ((*b).to_string(), *n));
            (
                group,
                Community {
                    label: ranked
                        .first()
                        .map(|(_, name)| (*name).to_string())
                        .unwrap_or_else(|| group.to_string()),
                    size: names.len(),
                    hubs: ranked
                        .iter()
                        .take(3)
                        .map(|(_, name)| Hub {
                            name: (*name).to_string(),
                            path: String::new(),
                            line: 0,
                        })
                        .collect(),
                    cross,
                },
            )
        })
        .collect();

    // `cross` was computed against the group's seed name; a reader needs the
    // other community's own label, which is its busiest member.
    let names: std::collections::BTreeMap<&str, String> = out
        .iter()
        .map(|(group, community)| (*group, community.label.clone()))
        .collect();
    let mut out: Vec<Community> = out
        .into_iter()
        .map(|(_, mut community)| {
            community.cross = community
                .cross
                .map(|(group, n)| (names.get(group.as_str()).cloned().unwrap_or(group), n));
            community
        })
        .collect();
    out.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.label.cmp(&b.label)));
    out
}

#[cfg(test)]
mod tests {
    /// Two clusters joined by one edge come back as two communities, the same
    /// two twice. A map that regroups a codebase each time it is opened is
    /// not a map, which is why the pass is seeded by name order and capped.
    #[test]
    fn communities_are_the_same_twice_and_name_what_joins_them() {
        let edges: Vec<(String, String)> = [
            ("store_open", "store_read"),
            ("store_read", "store_write"),
            ("store_write", "store_open"),
            ("http_get", "http_parse"),
            ("http_parse", "http_send"),
            ("http_send", "http_get"),
            ("http_parse", "store_read"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();

        let first = communities(&edges);
        let second = communities(&edges);
        assert_eq!(first.len(), 2, "two clusters, one bridge: {first:?}");
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(a.label, b.label, "the label moved between two runs");
            assert_eq!(a.size, b.size);
            let names =
                |c: &Community| -> Vec<String> { c.hubs.iter().map(|h| h.name.clone()).collect() };
            assert_eq!(names(a), names(b), "the hubs moved between two runs");
        }
        for community in &first {
            assert_eq!(community.size, 3, "{community:?}");
            assert!(!community.hubs.is_empty(), "a community with no hub");
        }
        // The bridge is directed, so it is a cross edge out of one community
        // and not out of the other. Counting it both ways would say two
        // subsystems call each other when one of them only answers.
        let crossings: Vec<_> = first.iter().filter_map(|c| c.cross.clone()).collect();
        assert_eq!(crossings.len(), 1, "{first:?}");
        assert_eq!(crossings[0].1, 1, "one edge joins them");
        assert!(
            first.iter().any(|c| c.label == crossings[0].0),
            "a cross edge must name another community's label: {first:?}"
        );
    }

    /// Nothing to group is an empty list, not a panic and not one community
    /// holding everything.
    #[test]
    fn a_graph_with_no_edges_has_no_communities() {
        assert!(communities(&[]).is_empty());
    }

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

    /// A macro invocation is not a call to a function of that name.
    ///
    /// `format!(...)` is the single worst case in the corpus: every function
    /// that formats a string looked like a caller of `store::format`, which on
    /// the semlith store meant 229 of them. The name the source writes is
    /// `format!`, and that is a different name from `format` — so that is what
    /// the edge records, and the two stop being confused for one another.
    #[test]
    fn a_macro_invocation_is_recorded_under_the_name_the_source_writes() {
        let e = run(
            "a.rs",
            "fn go() { let s = format!(\"{}\", 1); println!(\"{s}\"); helper(); }\nfn helper() {}\n",
        );
        assert!(
            !has_edge(&e, "go", "format", "calls"),
            "a `format!` is not a call to `fn format`: {:?}",
            e.edges
        );
        assert!(has_edge(&e, "go", "format!", "calls"), "{:?}", e.edges);
        assert!(has_edge(&e, "go", "println!", "calls"), "{:?}", e.edges);
        assert!(
            has_edge(&e, "go", "helper", "calls"),
            "an ordinary call is untouched: {:?}",
            e.edges
        );
    }

    #[test]
    fn a_file_with_no_grammar_yields_nothing_rather_than_failing() {
        // Markdown had no grammar until 0.17.0 and was this test's example of
        // a file the extractor skips. A file in no language at all is the
        // example that stays true.
        assert!(
            extract(&PathBuf::from("notes.txt"), "the lock is acquired")
                .unwrap()
                .is_none()
        );
    }

    /// A `const` and a `static` are definitions. The bundled tags query tags
    /// neither, so without the supplement the definition lift in
    /// `search_preferring` cannot fire for one and `semlith symbol MAX_NODES`
    /// answers nothing.
    #[test]
    fn rust_constants_and_statics_are_symbols() {
        let e = run(
            "limits.rs",
            "/// How many nodes a traversal may visit.\n\
             pub const MAX_NODES: usize = 4000;\n\
             static REGISTRY: &str = \"registry.json\";\n\
             fn helper() {}\n",
        );
        let named = |n: &str| e.symbols.iter().find(|s| s.name == n);
        let max_nodes = named("MAX_NODES")
            .unwrap_or_else(|| panic!("MAX_NODES is not a symbol: {:?}", e.symbols));
        assert_eq!(max_nodes.kind, "constant", "{max_nodes:?}");
        let registry = named("REGISTRY")
            .unwrap_or_else(|| panic!("REGISTRY is not a symbol: {:?}", e.symbols));
        assert_eq!(registry.kind, "constant", "{registry:?}");
        assert!(
            named("helper").is_some(),
            "the supplement broke the tags query"
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

    /// The two halves of the stored confidence column, in one file. The call
    /// is to `create`, and the file imported the `File` it goes through, so
    /// the source named where it came from; `helper` is a bare name and did
    /// not.
    #[test]
    fn an_imported_name_is_extracted_and_a_bare_one_is_inferred() {
        let e = run(
            "a.rs",
            "use std::fs::File;\nfn go() { File::create(\"x\"); helper(); }\nfn helper() {}\n",
        );
        assert_eq!(confidence_of(&e, "create", "calls"), EXTRACTED);
        assert_eq!(confidence_of(&e, "helper", "calls"), INFERRED);
    }

    fn hint_of(e: &Extraction, to: &str, kind: &str) -> Option<String> {
        e.edges
            .iter()
            .find(|x| x.to == to && x.kind == kind)
            .unwrap_or_else(|| panic!("no {kind} edge to {to} in {:?}", e.edges))
            .hint
            .clone()
    }

    /// A scoped call is a call to the function, and the path in front of it is
    /// a lead about which definition of that function was meant. Until 0.15.0
    /// the path segment was a second `calls` edge, which recorded a call the
    /// source never makes and gave the path finder a module to walk through.
    #[test]
    fn a_scoped_rust_call_hints_at_its_module_and_does_not_call_it() {
        let e = run("a.rs", "fn go() { store::edges_out(db, name); }\n");
        assert!(has_edge(&e, "go", "edges_out", "calls"), "{:?}", e.edges);
        assert_eq!(hint_of(&e, "edges_out", "calls").as_deref(), Some("store"));
        assert!(
            !has_edge(&e, "go", "store", "calls"),
            "the module is not called: {:?}",
            e.edges
        );
    }

    /// A longer path hints with the segment nearest the name, because that is
    /// the one that names the module the definition lives in.
    #[test]
    fn a_longer_rust_path_hints_with_the_segment_nearest_the_name() {
        let e = run("a.rs", "fn go() { crate::store::edges_out(db); }\n");
        assert_eq!(hint_of(&e, "edges_out", "calls").as_deref(), Some("store"));
    }

    /// A method call's receiver is a lead too, reduced to its last identifier.
    /// `self` is dropped: it names the file the call is already in, so it
    /// ranks nothing.
    #[test]
    fn a_rust_method_call_hints_with_its_receiver() {
        let e = run(
            "a.rs",
            "fn go(&self) { self.index.search(v); self.flush(); }\n",
        );
        assert_eq!(hint_of(&e, "search", "calls").as_deref(), Some("index"));
        assert_eq!(hint_of(&e, "flush", "calls"), None);
    }

    #[test]
    fn python_and_typescript_qualified_calls_carry_the_object_as_a_hint() {
        let py = run("m.py", "def go():\n    json.loads(raw)\n");
        assert_eq!(hint_of(&py, "loads", "calls").as_deref(), Some("json"));

        let ts = run("app.ts", "function go(){ store.search(q); }\n");
        assert_eq!(hint_of(&ts, "search", "calls").as_deref(), Some("store"));
    }

    /// One call reaches the extractor twice — once from the bundled query,
    /// once from the supplement — and only one of the two carries the hint.
    /// The edge that survives has to be the informed one.
    #[test]
    fn the_hinted_copy_of_a_doubly_captured_call_is_the_one_kept() {
        let e = run("a.rs", "fn go() { store::edges_out(db); }\n");
        let calls: Vec<&Edge> = e
            .edges
            .iter()
            .filter(|x| x.to == "edges_out" && x.kind == "calls")
            .collect();
        assert_eq!(calls.len(), 1, "{:?}", e.edges);
        assert_eq!(calls[0].hint.as_deref(), Some("store"));
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

    /// One language's fixture and what the extractor must find in it.
    ///
    /// The fixture is a real file on disk under `tests/fixtures/graph/`, so it
    /// is written in the language rather than in a Rust string literal, and an
    /// editor, a formatter and a human all read it as what it is.
    struct Fixture {
        language: &'static str,
        /// Path under `tests/fixtures/graph/`.
        file: &'static str,
        /// Symbols that must be extracted, as (name, kind).
        symbols: &'static [(&'static str, &'static str)],
        /// Edges that must be extracted, as (from, to, kind).
        edges: &'static [(&'static str, &'static str, &'static str)],
    }

    /// What every language must yield, declared rather than discovered.
    ///
    /// A language with no row here fails
    /// `every_advertised_language_has_a_fixture`, so a grammar cannot be wired
    /// up and left unproven: "it compiled" is not "it extracts anything". The
    /// expectations are written first and the extractor is made to meet them —
    /// loosening a row to match what the code happened to produce is the one
    /// move this table exists to prevent.
    const FIXTURES: &[Fixture] = &[
        Fixture {
            language: "rust",
            file: "rust/lock.rs",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[
                ("acquire", "helper", "calls"),
                ("lock", "std::fs::File", "imports"),
            ],
        },
        Fixture {
            language: "typescript",
            file: "typescript/app.ts",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls"), ("app", "./m", "imports")],
        },
        Fixture {
            language: "python",
            file: "python/run.py",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls"), ("run", "os", "imports")],
        },
        Fixture {
            language: "go",
            file: "go/serve.go",
            symbols: &[
                ("MaxHolders", "constant"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls"), ("serve", "fmt", "imports")],
        },
        Fixture {
            language: "java",
            file: "java/Main.java",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("Main", "class"),
                ("helper", "method"),
                ("acquire", "method"),
            ],
            edges: &[
                ("acquire", "helper", "calls"),
                ("Main", "helper", "contains"),
            ],
        },
        Fixture {
            language: "c",
            file: "c/main.c",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "clojure",
            file: "clojure/lock.clj",
            symbols: &[
                ("max-holders", "constant"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "cpp",
            file: "cpp/lock.cpp",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("MAX_WAITERS", "constant"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "csharp",
            file: "csharp/Lock.cs",
            symbols: &[
                ("MaxHolders", "constant"),
                ("Lock", "class"),
                ("Helper", "method"),
                ("Acquire", "method"),
            ],
            edges: &[("Acquire", "Helper", "calls")],
        },
        Fixture {
            language: "css",
            file: "css/lock.css",
            symbols: &[("acquire", "selector"), ("lock", "selector")],
            edges: &[("lock", "base.css", "imports")],
        },
        Fixture {
            language: "dart",
            file: "dart/lock.dart",
            symbols: &[
                ("maxHolders", "variable"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "dockerfile",
            file: "dockerfile/Dockerfile",
            symbols: &[("build", "stage"), ("runtime", "stage")],
            edges: &[
                ("build", "debian", "imports"),
                ("runtime", "build", "imports"),
            ],
        },
        Fixture {
            language: "elixir",
            file: "elixir/lock.ex",
            symbols: &[
                ("max_holders", "constant"),
                ("Lock", "module"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "elm",
            file: "elm/Lock.elm",
            symbols: &[("helper", "function"), ("acquire", "function")],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "erlang",
            file: "erlang/lock.erl",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("lock", "module"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "fortran",
            file: "fortran/lock.f90",
            symbols: &[
                ("max_holders", "constant"),
                ("lock", "module"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "graphql",
            file: "graphql/lock.graphql",
            symbols: &[
                ("Lock", "type"),
                ("Holder", "type"),
                ("Acquire", "operation"),
            ],
            edges: &[("Lock", "holder", "contains")],
        },
        Fixture {
            language: "groovy",
            file: "groovy/lock.groovy",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "haskell",
            file: "haskell/Lock.hs",
            symbols: &[("helper", "function"), ("acquire", "function")],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "html",
            file: "html/lock.html",
            symbols: &[("acquire", "element")],
            edges: &[
                ("lock", "base.css", "imports"),
                ("lock", "app.js", "imports"),
            ],
        },
        Fixture {
            language: "javascript",
            file: "javascript/app.js",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls"), ("app", "./m", "imports")],
        },
        Fixture {
            language: "json",
            file: "json/lock.json",
            symbols: &[("name", "key"), ("acquire", "key"), ("timeout", "key")],
            edges: &[("acquire", "timeout", "contains")],
        },
        Fixture {
            language: "julia",
            file: "julia/lock.jl",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("Lock", "module"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "kotlin",
            file: "kotlin/Lock.kt",
            symbols: &[
                ("MAX_HOLDERS", "property"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "lua",
            file: "lua/lock.lua",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "makefile",
            file: "makefile/Makefile",
            symbols: &[("helper", "target"), ("acquire", "target")],
            edges: &[
                ("acquire", "helper", "references"),
                ("Makefile", "config.mk", "imports"),
            ],
        },
        Fixture {
            language: "markdown",
            file: "markdown/notes.md",
            symbols: &[("Lock", "heading"), ("Acquire", "heading")],
            edges: &[("Lock", "Acquire", "contains")],
        },
        Fixture {
            language: "nix",
            file: "nix/lock.nix",
            symbols: &[("helper", "binding"), ("acquire", "binding")],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "objective-c",
            file: "objective-c/Lock.m",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("Lock", "class"),
                ("helper", "method"),
                ("acquire", "method"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "ocaml",
            file: "ocaml/lock.ml",
            symbols: &[("helper", "function"), ("acquire", "function")],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "perl",
            file: "perl/lock.pl",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "php",
            file: "php/lock.php",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "powershell",
            file: "powershell/Lock.ps1",
            symbols: &[("Get-Helper", "function"), ("Get-Acquire", "function")],
            edges: &[("Get-Acquire", "Get-Helper", "calls")],
        },
        Fixture {
            language: "proto",
            file: "proto/lock.proto",
            symbols: &[("Lock", "message"), ("Acquire", "service"), ("Hold", "rpc")],
            edges: &[("lock", "base.proto", "imports")],
        },
        Fixture {
            language: "r",
            file: "r/lock.r",
            symbols: &[("helper", "function"), ("acquire", "function")],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "ruby",
            file: "ruby/lock.rb",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("helper", "method"),
                ("acquire", "method"),
            ],
            edges: &[("lock", "helper", "defines")],
        },
        Fixture {
            language: "scala",
            file: "scala/Lock.scala",
            symbols: &[
                ("MaxHolders", "constant"),
                ("Lock", "object"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "shell",
            file: "shell/lock.sh",
            symbols: &[
                ("MAX_HOLDERS", "constant"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[
                ("acquire", "helper", "calls"),
                ("lock", "./config.sh", "imports"),
            ],
        },
        Fixture {
            language: "sql",
            file: "sql/lock.sql",
            symbols: &[("lock_holder", "table"), ("acquire", "view")],
            edges: &[("acquire", "lock_holder", "references")],
        },
        Fixture {
            language: "svelte",
            file: "svelte/Lock.svelte",
            symbols: &[("lock", "element")],
            edges: &[("Lock", "lock", "defines")],
        },
        Fixture {
            language: "swift",
            file: "swift/Lock.swift",
            symbols: &[
                ("maxHolders", "property"),
                ("helper", "function"),
                ("acquire", "function"),
            ],
            edges: &[("acquire", "helper", "calls")],
        },
        Fixture {
            language: "terraform",
            file: "terraform/lock.tf",
            symbols: &[("region", "block"), ("base", "block"), ("lock", "block")],
            edges: &[("lock", "region", "defines")],
        },
        Fixture {
            language: "toml",
            file: "toml/lock.toml",
            symbols: &[("name", "key"), ("acquire", "table")],
            edges: &[("acquire", "timeout", "contains")],
        },
        Fixture {
            language: "vue",
            file: "vue/Lock.vue",
            symbols: &[("lock", "element")],
            edges: &[("Lock", "lock", "defines")],
        },
        Fixture {
            language: "yaml",
            file: "yaml/lock.yaml",
            symbols: &[("name", "key"), ("acquire", "key"), ("timeout", "key")],
            edges: &[("acquire", "timeout", "contains")],
        },
        Fixture {
            language: "zig",
            file: "zig/lock.zig",
            symbols: &[
                ("MAX_HOLDERS", "variable"),
                ("helper", "function"),
                ("acquire", "function"),
                ("std", "variable"),
            ],
            // `const std = @import("std")` binds a name equal to the module
            // it imports, so the import is a self-edge and is dropped: the
            // binding itself is the fact worth recording, and it is a symbol.
            edges: &[("acquire", "helper", "calls")],
        },
    ];

    /// Languages whose fixture declares no symbol of kind `constant`, and why.
    ///
    /// Two different reasons live here, and each row says which it is, because
    /// "this language has nothing to extract" and "this language extracts it
    /// already" are not the same fact and a reader who conflates them will
    /// delete the wrong row.
    ///
    /// The first group has no construct for a named constant at all. A plain
    /// assignment is not one: a Makefile variable can be overridden on the
    /// command line, a CSS custom property can be redefined by a later rule,
    /// and a shell script that says `MAX=4` can say `MAX=5` on the next line —
    /// which is exactly why `shell` is not in this list and `readonly` is what
    /// its query matches. The second group is languages whose grammar already
    /// makes a constant a symbol under another kind. Nothing is added for
    /// those: a second capture over the same span leaves two rows for one
    /// declaration, each the other's innermost enclosing definition, and
    /// `semlith symbol` would answer a single `const val` with two definitions.
    const WITHOUT_CONSTANTS: &[(&str, &str)] = &[
        (
            "css",
            "a custom property is a variable a later rule can redefine",
        ),
        (
            "dart",
            "already a symbol; the bundled query calls it a variable",
        ),
        (
            "dockerfile",
            "ARG and ENV are build variables, overridable at build time",
        ),
        (
            "elm",
            "every binding is immutable, so none of them is the constant",
        ),
        ("graphql", "a schema names types and fields, never a value"),
        (
            "haskell",
            "every binding is immutable, so none of them is the constant",
        ),
        ("html", "markup, with no declaration of any kind"),
        (
            "json",
            "data; a member is a key and a value, not a declaration",
        ),
        (
            "kotlin",
            "already a symbol; `const val` is a property to the grammar",
        ),
        (
            "makefile",
            "a variable can be overridden on the command line",
        ),
        ("markdown", "prose"),
        (
            "nix",
            "a `let` or attribute binding is already a `binding` symbol",
        ),
        (
            "ocaml",
            "every `let` binding is immutable, so none of them is the constant",
        ),
        (
            "powershell",
            "a constant is `Set-Variable -Option Constant`, a call and not syntax",
        ),
        (
            "proto",
            "an enum value is a member of an enum, not a declared constant",
        ),
        (
            "r",
            "the language has no constant construct; `<-` binds and rebinds",
        ),
        (
            "sql",
            "there is no `CREATE CONSTANT`; a literal is written where it is used",
        ),
        (
            "svelte",
            "the grammar reads the markup and hands back the script as raw text",
        ),
        (
            "swift",
            "already a symbol; the bundled query calls `let` a property",
        ),
        (
            "terraform",
            "a local or a variable is a value HCL evaluates, not a constant",
        ),
        ("toml", "data; a key and a value, not a declaration"),
        (
            "vue",
            "the grammar reads the template and hands back the script as raw text",
        ),
        ("yaml", "data; a key and a value, not a declaration"),
        (
            "zig",
            "already a symbol; our own query calls `const` a variable",
        ),
    ];

    fn fixture_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/graph")
    }

    /// Prints each fixture's parse tree, for writing a tags query against a
    /// grammar whose node names nobody has memorised.
    ///
    /// ```sh
    /// cargo test --lib dump_fixture_trees -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "a tool for writing queries, not a gate"]
    fn dump_fixture_trees() {
        let root = fixture_root();
        let mut languages: Vec<_> = crate::filter::LANGUAGES.iter().map(|e| e.name).collect();
        languages.sort_unstable();
        for name in languages {
            let dir = root.join(name);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                let text = std::fs::read_to_string(&path).unwrap();
                let Some((language, _)) = grammar(name) else {
                    continue;
                };
                let mut parser = Parser::new();
                parser.set_language(&language).unwrap();
                let tree = parser.parse(&text, None).unwrap();
                println!(
                    "===== {name} :: {} =====\n{}",
                    path.file_name().unwrap().to_string_lossy(),
                    tree.root_node().to_sexp()
                );
            }
        }
    }

    /// Prints what each fixture actually extracts, for comparing against the
    /// row that declares what it should.
    ///
    /// ```sh
    /// cargo test --lib dump_fixture_extractions -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "a tool for writing queries, not a gate"]
    fn dump_fixture_extractions() {
        let root = fixture_root();
        for fixture in FIXTURES {
            let text = std::fs::read_to_string(root.join(fixture.file)).unwrap();
            let e = extract(std::path::Path::new(fixture.file), &text)
                .unwrap()
                .unwrap();
            println!("===== {} =====", fixture.language);
            for s in &e.symbols {
                println!(
                    "  symbol {:14} {:24} {}-{}",
                    s.kind, s.name, s.start_line, s.end_line
                );
            }
            for x in &e.edges {
                println!(
                    "  edge   {:14} {} -> {} (line {:?})",
                    x.kind, x.from, x.to, x.line
                );
            }
        }
    }

    /// Every language the filter advertises has a fixture, so no grammar is
    /// wired up without something proving it extracts.
    #[test]
    fn every_advertised_language_has_a_fixture() {
        for entry in crate::filter::LANGUAGES {
            if WITHOUT_GRAMMAR.iter().any(|(name, _)| *name == entry.name) {
                continue;
            }
            assert!(
                FIXTURES.iter().any(|f| f.language == entry.name),
                "{} has a grammar but no fixture asserting what it extracts",
                entry.name
            );
        }
    }

    /// Every language with a construct for a named constant extracts one, and
    /// proves it from a fixture written in that language.
    ///
    /// Until 0.23.0 this held for Rust alone. The other forty-five advertised
    /// a graph and then answered `semlith symbol MAX_HOLDERS` with nothing, so
    /// the definition lift could not fire for the one kind of name an agent is
    /// most likely to type verbatim. The escape hatch is [`WITHOUT_CONSTANTS`],
    /// and it costs a sentence saying which language and why — "there was no
    /// time" is not a row anybody can write here.
    #[test]
    fn every_language_with_a_constant_construct_extracts_one() {
        for entry in crate::filter::LANGUAGES {
            let fixture = FIXTURES
                .iter()
                .find(|f| f.language == entry.name)
                .unwrap_or_else(|| panic!("{} has no fixture", entry.name));
            let declares = fixture.symbols.iter().any(|(_, kind)| *kind == "constant");
            match WITHOUT_CONSTANTS
                .iter()
                .find(|(name, _)| *name == entry.name)
            {
                Some((_, why)) => assert!(
                    !declares,
                    "{} is listed as extracting no constant ({why}) but its fixture declares one",
                    entry.name
                ),
                None => assert!(
                    declares,
                    "{} has a constant construct but no fixture proves one is extracted",
                    entry.name
                ),
            }
        }
    }

    /// Each language's fixture yields the symbols and edges its row declares.
    #[test]
    fn every_fixture_yields_what_it_declares() {
        let root = fixture_root();
        let mut wrong: Vec<String> = Vec::new();
        for fixture in FIXTURES {
            let path = root.join(fixture.file);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{} is missing: {e}", path.display()));
            let extraction = extract(std::path::Path::new(fixture.file), &text)
                .unwrap_or_else(|e| panic!("{}: extraction failed: {e}", fixture.language))
                .unwrap_or_else(|| {
                    panic!(
                        "{}: no extractor reached {} — the extension is not in the table",
                        fixture.language, fixture.file
                    )
                });

            for (name, kind) in fixture.symbols {
                if !extraction
                    .symbols
                    .iter()
                    .any(|s| s.name == *name && s.kind == *kind)
                {
                    wrong.push(format!(
                        "{}: no {kind} named {name}; found {:?}",
                        fixture.language,
                        extraction
                            .symbols
                            .iter()
                            .map(|s| (&s.kind, &s.name))
                            .collect::<Vec<_>>()
                    ));
                }
            }

            for (from, to, kind) in fixture.edges {
                if !extraction
                    .edges
                    .iter()
                    .any(|e| e.from == *from && e.to == *to && e.kind == *kind)
                {
                    wrong.push(format!(
                        "{}: no {kind} edge {from} -> {to}; found {:?}",
                        fixture.language,
                        extraction
                            .edges
                            .iter()
                            .map(|e| (&e.from, &e.kind, &e.to))
                            .collect::<Vec<_>>()
                    ));
                }
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    /// Every language the product advertises really parses, so the About page's
    /// list cannot claim a language the binary does not carry.
    ///
    /// This is the one-table rule as a test. The set is
    /// `filter::LANGUAGES` — what `--language` accepts and what the About page
    /// prints — and the only permitted gap is a row named in
    /// [`WITHOUT_GRAMMAR`] with its reason. Adding a language to the filter
    /// without a grammar fails here rather than shipping a language that
    /// filters but does not graph.
    #[test]
    fn every_advertised_language_has_a_working_grammar() {
        // Collected rather than asserted one at a time: with forty-six
        // languages, failing on the first one turns a ten-minute fix into
        // forty-six runs.
        let mut broken: Vec<String> = Vec::new();
        for entry in crate::filter::LANGUAGES {
            let lang = entry.name;
            if let Some((_, why)) = WITHOUT_GRAMMAR.iter().find(|(name, _)| *name == lang) {
                assert!(
                    grammar(lang).is_none(),
                    "{lang} is listed as having no grammar ({why}) but has one"
                );
                continue;
            }
            let Some((language, tags)) = grammar(lang) else {
                broken.push(format!("{lang}: advertised but has no grammar"));
                continue;
            };
            let mut parser = Parser::new();
            if let Err(e) = parser.set_language(&language) {
                broken.push(format!("{lang}: grammar is ABI-incompatible: {e}"));
                continue;
            }
            if let Err(e) = Query::new(&language, tags) {
                broken.push(format!("{lang}: tags query does not compile: {e:?}"));
            }
            let supplement = supplement(lang);
            if !supplement.is_empty()
                && let Err(e) = Query::new(&language, supplement)
            {
                broken.push(format!("{lang}: supplement does not compile: {e:?}"));
            }
        }
        assert!(broken.is_empty(), "{}", broken.join("\n"));
    }

    /// A file being saved mid-edit is the normal case under the watcher.
    #[test]
    fn a_file_that_does_not_parse_yields_what_it_could_read() {
        let e = run("a.rs", "fn good() {}\nfn broken( {\n");
        assert!(e.symbols.iter().any(|s| s.name == "good"));
    }

    /// The line an edge carries is the line the call was written on, not the
    /// line the function that contains it starts on. Those are the same only
    /// for a one-line function, which is why the fixture's call sits four
    /// lines below its `fn`.
    #[test]
    fn a_reference_edge_carries_the_line_the_call_was_written_on() {
        let e = run(
            "a.rs",
            "use other::helper;\n\nfn caller() {\n    let x = 1;\n    let _ = x;\n    helper();\n}\n",
        );
        let call = e
            .edges
            .iter()
            .find(|x| x.kind == "calls" && x.to == "helper")
            .unwrap_or_else(|| panic!("no call edge: {:?}", e.edges));
        assert_eq!(call.line, Some(6), "{:?}", e.edges);

        let import = e
            .edges
            .iter()
            .find(|x| x.kind == "imports")
            .unwrap_or_else(|| panic!("no import edge: {:?}", e.edges));
        assert_eq!(import.line, Some(1), "{:?}", e.edges);

        // Nothing this release extracts may leave the column empty: a NULL is
        // reserved for a row an older binary wrote.
        assert!(e.edges.iter().all(|x| x.line.is_some()), "{:?}", e.edges);
    }

    /// A re-export is the only thing between the name a caller wrote and the
    /// definition it meant. Each language records the alias as a symbol of its
    /// own and an `aliases` edge from it to the bare name it stands for.
    #[test]
    fn an_alias_becomes_a_symbol_and_an_edge_to_what_it_stands_for() {
        let rust = run("a.rs", "pub use store::Semlith as Store;\n");
        assert!(
            rust.symbols.iter().any(|s| s.name == "Store"),
            "{:?}",
            rust.symbols
        );
        assert!(
            has_edge(&rust, "Store", "Semlith", "aliases"),
            "{:?}",
            rust.edges
        );

        let py = run("a.py", "from x import search as find\n");
        assert!(has_edge(&py, "find", "search", "aliases"), "{:?}", py.edges);

        let ts = run("a.ts", "import { search as find } from './x';\n");
        assert!(has_edge(&ts, "find", "search", "aliases"), "{:?}", ts.edges);

        let go = run("a.go", "package m\nimport fp \"path/filepath\"\n");
        assert!(has_edge(&go, "fp", "filepath", "aliases"), "{:?}", go.edges);
    }

    /// A re-export that does not rename has nothing to cross: the alias and
    /// the definition are one name, and a self-edge would be noise.
    #[test]
    fn a_re_export_that_does_not_rename_adds_no_alias_edge() {
        let e = run("a.rs", "pub use store::Semlith;\n");
        assert!(
            !e.edges.iter().any(|x| x.kind == "aliases"),
            "{:?}",
            e.edges
        );
    }

    /// The defect the flat hop had: `near` is one hop from a single weak seed
    /// and `hub` is two hops from three strong ones, and one hop ranked them
    /// the same because both were simply "reached". The walk has to prefer the
    /// one the query's own hits agree about.
    #[test]
    fn a_symbol_three_seeds_agree_on_outranks_one_a_single_seed_reached() {
        let adjacency: std::collections::HashMap<&str, Vec<&str>> = [
            ("a", vec!["mid_a"]),
            ("b", vec!["mid_b"]),
            ("c", vec!["mid_c"]),
            ("mid_a", vec!["hub"]),
            ("mid_b", vec!["hub"]),
            ("mid_c", vec!["hub"]),
            ("weak", vec!["near"]),
            ("hub", vec![]),
            ("near", vec![]),
        ]
        .into_iter()
        .collect();

        let personal: std::collections::BTreeMap<String, f32> = [
            ("a".to_string(), 1.0),
            ("b".to_string(), 1.0),
            ("c".to_string(), 1.0),
            ("weak".to_string(), 0.2),
        ]
        .into_iter()
        .collect();

        let ranked = expand(&personal, |name| {
            Ok(adjacency
                .get(name)
                .into_iter()
                .flatten()
                .map(|to| (to.to_string(), 1.0, EXTRACTED.to_string()))
                .collect())
        })
        .unwrap();

        let place = |name: &str| ranked.iter().position(|(n, _, _)| n == name);
        let hub = place("hub").unwrap_or_else(|| panic!("hub was not reached: {ranked:?}"));
        let near = place("near").unwrap_or_else(|| panic!("near was not reached: {ranked:?}"));
        assert!(
            hub < near,
            "two hops from three seeds must outrank one hop from a weak seed: {ranked:?}"
        );
        // Nothing points at a seed in this fixture, so no seed is reached and
        // none is in the result. A seed the code *does* point at would stay —
        // see `a_seed_the_code_points_at_is_still_reached`.
        assert!(
            ranked.iter().all(|(n, _, _)| !personal.contains_key(n)),
            "no edge reaches a seed in this fixture: {ranked:?}"
        );
    }

    /// The query matched a chunk and the code points at it as well. That is
    /// two independent reasons to return it, and the hit has to say so — a
    /// two-file corpus where every name is a seed is exactly where an
    /// exclude-the-seeds rule silently empties the third list.
    #[test]
    fn a_seed_the_code_points_at_is_still_reached() {
        let personal: std::collections::BTreeMap<String, f32> =
            [("knead".to_string(), 1.0), ("autolyse".to_string(), 1.0)]
                .into_iter()
                .collect();
        let ranked = expand(&personal, |name| {
            Ok(match name {
                "knead" => vec![("autolyse".to_string(), 1.0, EXTRACTED.to_string())],
                _ => Vec::new(),
            })
        })
        .unwrap();
        assert!(
            ranked.iter().any(|(n, _, _)| n == "autolyse"),
            "a seed an edge reaches is still a neighbour: {ranked:?}"
        );
        assert!(
            ranked.iter().all(|(n, _, _)| n != "knead"),
            "nothing points at knead, so the walk did not reach it: {ranked:?}"
        );
    }

    /// A name is asked about once however many rounds run. Without the cache
    /// the walk would re-query the whole frontier every round, which is the
    /// cost that would have made this unshippable.
    #[test]
    fn the_walk_asks_about_each_name_once() {
        let mut asked: Vec<String> = Vec::new();
        let personal: std::collections::BTreeMap<String, f32> =
            [("a".to_string(), 1.0)].into_iter().collect();
        expand(&personal, |name| {
            asked.push(name.to_string());
            Ok(match name {
                "a" => vec![("b".to_string(), 1.0, EXTRACTED.to_string())],
                "b" => vec![("c".to_string(), 1.0, INFERRED.to_string())],
                _ => Vec::new(),
            })
        })
        .unwrap();
        let mut unique = asked.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(asked.len(), unique.len(), "asked twice: {asked:?}");
    }

    /// The label on a reached name is the best edge that reached it, not the
    /// last one read.
    #[test]
    fn a_reached_name_is_labelled_by_the_best_edge_that_reached_it() {
        let personal: std::collections::BTreeMap<String, f32> =
            [("weak".to_string(), 1.0), ("strong".to_string(), 1.0)]
                .into_iter()
                .collect();
        let ranked = expand(&personal, |name| {
            Ok(match name {
                "weak" => vec![("target".to_string(), 0.5, INFERRED.to_string())],
                "strong" => vec![("target".to_string(), 1.0, EXTRACTED.to_string())],
                _ => Vec::new(),
            })
        })
        .unwrap();
        let (_, _, tier) = ranked
            .iter()
            .find(|(n, _, _)| n == "target")
            .expect("target reached");
        assert_eq!(tier, EXTRACTED);
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
            crate::store::insert_edge(&db, id[from], to, "calls", EXTRACTED, None, None).unwrap();
        }
        db
    }

    /// The block is one reply where 0.15.0 needed three, and the second ring
    /// is walked only through names the first ring settled — an ambiguous name
    /// is several unrelated definitions, and expanding one would put somebody
    /// else's callers into this symbol's context.
    #[test]
    fn the_evidence_block_carries_the_definition_both_rings_and_no_ambiguity() {
        let db = chain();
        let found = evidence(&db, "b", &dependency_kinds(), 20, false).unwrap();

        assert_eq!(found.name, "b");
        assert!(!found.definitions.is_empty(), "{found:?}");
        assert!(
            found.callers.iter().any(|e| e.symbol.name == "a"),
            "{:?}",
            found.callers
        );
        assert!(
            found.callees.iter().any(|e| e.symbol.name == "c"),
            "{:?}",
            found.callees
        );
        // `d` is two hops from `b`, through `c`.
        assert!(
            found.ego.iter().any(|h| h.name == "d" && h.via == "c"),
            "{:?}",
            found.ego
        );
        // The centre and the first ring are not in the second ring.
        assert!(
            found
                .ego
                .iter()
                .all(|h| h.name != "b" && h.name != "a" && h.name != "c"),
            "{:?}",
            found.ego
        );
        assert!(
            found.ego.len() <= EGO_LIMIT,
            "the ring is context, not an answer: {:?}",
            found.ego
        );

        let text = found.render("", "", &verbatim);
        assert!(text.contains("callers"), "{text}");
        assert!(text.contains("callees"), "{text}");
        assert!(text.contains("two hops out"), "{text}");
    }

    #[test]
    fn neighbours_separates_the_two_directions() {
        let db = chain();
        let n = neighbours(&db, "d", &[], false).unwrap();
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
        crate::store::insert_edge(&db, d, "a", "calls", EXTRACTED, None, None).unwrap();
        // d -> a -> b -> c -> d is now a cycle; a large depth must still return.
        let path = shortest_path(&db, "a", "d", 50, false).unwrap();
        assert!(path.is_some(), "a still reaches d");
        assert!(
            path.unwrap().steps.len() <= 4,
            "a cycle inflated the shortest path"
        );
    }

    #[test]
    fn the_shortest_path_is_the_short_one_and_names_every_edge() {
        let db = chain();
        let path = shortest_path(&db, "a", "d", 10, false)
            .unwrap()
            .expect("a reaches d");
        let hops: Vec<(&str, &str)> = path
            .steps
            .iter()
            .map(|s| (s.from.as_str(), s.to.as_str()))
            .collect();
        assert_eq!(hops, [("a", "b"), ("b", "c"), ("c", "d")]);
        assert!(path.steps.iter().all(|s| s.kind == "calls"));
        assert!(path.steps.iter().all(|s| s.confidence == EXTRACTED));
        assert!(path.steps.iter().all(|s| !s.to_path.is_empty()));
        assert_eq!(path.summary.extracted, 3);
        assert_eq!(path.summary.seams, 0);
        assert!(!path.summary.hypothesis, "every hop is extracted");
    }

    /// The shape the release exists for: `start` calls `record`, and `record`
    /// is two unrelated functions in two files. One of them calls `finish`.
    ///
    /// The 0.14.0 finder walked out of one `record` and into the other without
    /// a word, and printed `start -> record -> finish` in the same format a
    /// real chain prints in.
    fn forked_name() -> rusqlite::Connection {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::prepare_for_tests(&db);
        let define = |db: &rusqlite::Connection, path: &str, name: &str, line: u32| {
            let file = crate::store::insert_file(db, path, path, 1, 0).unwrap_or_else(|_| {
                db.query_row("SELECT id FROM files WHERE path = ?1", [path], |r| r.get(0))
                    .unwrap()
            });
            let symbol = Symbol {
                kind: "function".to_string(),
                name: name.to_string(),
                qualified: name.to_string(),
                start_line: line,
                end_line: line + 1,
            };
            crate::store::insert_symbol(db, file, None, &symbol).unwrap()
        };
        let start = define(&db, "src/start.rs", "start", 10);
        define(&db, "src/one.rs", "record", 20);
        let second = define(&db, "src/two.rs", "record", 30);
        define(&db, "src/finish.rs", "finish", 40);

        // `start` calls a `record`, and nothing in its source says which.
        crate::store::insert_edge(&db, start, "record", "calls", INFERRED, None, None).unwrap();
        // Only the second `record` reaches `finish`.
        crate::store::insert_edge(&db, second, "finish", "calls", INFERRED, None, None).unwrap();
        db
    }

    /// By default the finder will not cross a name it cannot pin down, so the
    /// answer is that there is no chain. This is the central correction of the
    /// release: no answer beats a wrong answer dressed as a right one.
    #[test]
    fn a_chain_through_an_ambiguous_name_is_not_walked_by_default() {
        let db = forked_name();
        assert!(
            shortest_path(&db, "start", "finish", 6, false)
                .unwrap()
                .is_none(),
            "the only route crosses a name with two definitions"
        );
    }

    /// With `all_edges` the chain comes back, and every part of it says what
    /// it is worth: the hop is ambiguous, the join is a seam, the trailer
    /// counts it, and the sentence names it a hypothesis.
    #[test]
    fn the_same_chain_with_all_edges_is_labelled_rather_than_asserted() {
        let db = forked_name();
        let chain = shortest_path(&db, "start", "finish", 6, true)
            .unwrap()
            .expect("all_edges walks the ambiguous name");

        assert_eq!(chain.steps.len(), 2);
        assert_eq!(chain.steps[0].confidence, AMBIGUOUS);
        assert_eq!(chain.steps[0].definitions, 2);
        assert_eq!(chain.summary.seams, 1, "{:?}", chain.steps);
        assert_eq!(
            chain.summary.ambiguous_names,
            vec![("record".to_string(), 2)]
        );
        assert!(chain.summary.hypothesis);

        let rendered = chain.render("", "", &verbatim);
        assert!(rendered.contains("seam"), "{rendered}");
        assert!(rendered.contains("record: 2 definitions"), "{rendered}");
        assert!(
            rendered.contains("A hypothesis, not a finding."),
            "{rendered}"
        );
        assert!(
            rendered.contains("src/start.rs:10"),
            "every hop shows both endpoints: {rendered}"
        );
    }

    /// A chain with nothing to doubt says nothing about doubt. The trailer is
    /// still printed, because a reader should not have to know that silence
    /// means "all extracted".
    #[test]
    fn an_all_extracted_chain_carries_no_hypothesis_line() {
        let db = chain();
        let rendered = shortest_path(&db, "a", "d", 10, false)
            .unwrap()
            .expect("a reaches d")
            .render("", "", &verbatim);
        assert!(!rendered.contains("hypothesis"), "{rendered}");
        assert!(!rendered.contains("seam"), "{rendered}");
        assert!(rendered.contains("3 hops"), "{rendered}");
        assert!(rendered.contains("3 extracted"), "{rendered}");
    }

    /// The refusal and the failure are different sentences, because they are
    /// different answers, and only one of them has a flag that widens it.
    #[test]
    fn the_refusal_names_the_flag_that_widens_the_question() {
        let strict = not_connected("a", "b", 6, false);
        assert!(strict.contains("by resolved edges"), "{strict}");
        assert!(strict.contains("--all-edges"), "{strict}");

        let walked = not_connected("a", "b", 6, true);
        assert!(walked.contains("any edge in the store"), "{walked}");
        assert!(!walked.contains("--all-edges"), "{walked}");
    }

    /// Not connected is an answer, and so is not connected *within this depth*.
    #[test]
    fn an_unconnected_pair_returns_nothing_rather_than_erroring() {
        let db = chain();
        assert!(
            shortest_path(&db, "a", "lonely", 10, false)
                .unwrap()
                .is_none()
        );
        assert!(
            shortest_path(&db, "a", "d", 2, false).unwrap().is_none(),
            "d is three hops away, so a depth of two does not reach it"
        );
    }
}
