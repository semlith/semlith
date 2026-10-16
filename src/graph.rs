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

/// What `edges.hint` starts with when the call went through a receiver
/// (`x.run()`, `self.flush()`): the rest is the receiver's type or name, and
/// may be empty. The resolver never settles such an edge on a free function,
/// and settles it at all only on a method of the receiver's stated type.
pub const RECEIVER: &str = ".";
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
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    /// The edge leaves from the file's module symbol: a `defines` edge, or a
    /// reference at the top level. Without it a file and a definition that
    /// share a name — `main.rs` and `fn main` — are one source, and the
    /// module's `defines` edges were written onto `fn main` (2026-10-14
    /// audit, G3).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub from_module: bool,
}

/// What one file yielded.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
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
            ; `crate::search_parallel(args)`, `self::helper()`, `super::run()`:
            ; a path the grammar spells with a keyword, so `(identifier)` above
            ; never matched it and the call was missed (2026-10-14 audit, G2).
            ; The keyword names no file, so there is no hint.
            (call_expression
              function: (scoped_identifier
                path: [(crate) (self) (super)]
                name: (identifier) @name)) @reference.call
            ; A method call: `self.flush()`, `store.db()`, `self.index.search()`.
            ; The receiver is the hint, reduced to its type or last identifier,
            ; and marked as a receiver: a method call never means a free fn.
            (call_expression
              function: (field_expression
                value: (_) @receiver
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
            ; module or the receiver the call went through. A receiver unless
            ; the file imports the name as a module; see `extract`.
            (call
              function: (attribute
                object: (_) @receiver
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
            ; `fmt.Errorf(...)`, `c.validate()`: the operand is the hint, which
            ; is what tells `fmt.Errorf` from a call into this corpus.
            (call_expression
              function: (selector_expression
                operand: (identifier) @hint
                field: (field_identifier) @name)) @reference.call
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
    /// The type a Rust method belongs to — the `impl` or `trait` block around
    /// it — which the tree says and the enclosing-definition rule cannot,
    /// because an `impl` block is not a definition of its own.
    owner: Option<String>,
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
    /// The call went through a receiver (`x.f()`), so it means a method.
    method: bool,
    /// That receiver is `self`, `this` or `cls`: the enclosing class.
    on_self: bool,
}

/// The deepest nesting a block grammar is given. tree-sitter-md and
/// tree-sitter-yaml keep a stack of open blocks that their external scanner
/// serialises into tree-sitter's 1024-byte buffer, and past about 260 levels
/// the C `assert` in `ts_parser__external_scanner_serialize` aborts the
/// process — measured 2026-10-14 for nested blockquotes, nested lists, nested
/// block sequences and `- - -` sequences alike. Half that is deeper than any
/// real document nests; a file past it keeps its text and loses only its graph.
pub const MAX_BLOCK_NESTING: usize = 128;

/// Whether a file in a block grammar nests past [`MAX_BLOCK_NESTING`], counted
/// line by line: each `>` and list marker opens a level, and so does every two
/// columns of indentation in front of them.
/// The text a grammar's C scanner is handed, for the grammars whose scanner
/// calls a narrow `<ctype.h>` function on a full Unicode code point.
///
/// tree-sitter-md's block scanner runs `isdigit` on the character after a
/// digit at a list-marker position (`scanner.c:740`), and tree-sitter-bash's on
/// the character after `{` or `..` in a brace expansion (`scanner.c:1158,
/// 1172`). glibc's `isdigit` indexes a table valid for -128..255 with no
/// check, so a code point past U+00FF reads out of bounds and can fault: a
/// 7-byte Markdown file did under the 0.41.0 fuzzers. Such a character, right
/// after a character those scanners test, is replaced with as many `a` bytes as
/// it has, so every byte offset and line stays the original's and names are
/// still read from the original text. Borrowed when nothing needs replacing.
pub fn ctype_safe<'a>(lang: &str, text: &'a str) -> std::borrow::Cow<'a, str> {
    let triggers: fn(char) -> bool = match lang {
        "markdown" => |c| c.is_ascii_digit(),
        "bash" => |c| c.is_ascii_digit() || c == '{' || c == '.',
        _ => return std::borrow::Cow::Borrowed(text),
    };
    let needs = |prev: Option<char>, c: char| (c as u32) > 0xFF && prev.is_some_and(triggers);
    let mut prev = None;
    if !text.chars().any(|c| {
        let hit = needs(prev, c);
        prev = Some(c);
        hit
    }) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut prev = None;
    for c in text.chars() {
        if needs(prev, c) {
            out.extend(std::iter::repeat_n('a', c.len_utf8()));
            prev = Some('a');
        } else {
            out.push(c);
            prev = Some(c);
        }
    }
    std::borrow::Cow::Owned(out)
}

pub fn too_nested(lang: &str, text: &str) -> bool {
    if !matches!(lang, "markdown" | "yaml") {
        return false;
    }
    // Debug builds only: lets a test hand the real crashing shapes to the
    // parse worker and watch it, not this cap, keep the process up.
    #[cfg(debug_assertions)]
    if std::env::var_os("SEMLITH_NO_NESTING_CAP").is_some() {
        return false;
    }
    text.lines().any(|line| {
        let (mut depth, mut columns) = (0usize, 0usize);
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                ' ' => columns += 1,
                '\t' => columns += 2,
                '>' => depth += 1,
                '-' | '*' | '+' if chars.peek().is_none_or(|n| *n == ' ' || *n == '\t') => {
                    depth += 1
                }
                // Not the end of the prefix: tree-sitter-md goes on nesting
                // past a NUL or another control character, so a line of 100
                // `>`, NULs, then 128 more aborted the parser while this
                // counted 100 (cargo-fuzz chunk_markdown, 0.41.0).
                c if c.is_control() => {}
                _ => break,
            }
            if depth + columns / 2 > MAX_BLOCK_NESTING {
                return true;
            }
        }
        false
    })
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
    if too_nested(lang, text) {
        return Ok(None);
    }
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
    let parse = ctype_safe(lang, text);
    let parsed =
        parser.parse_with_options(&mut |at, _| &parse.as_bytes()[at..], None, Some(options));
    let Some(tree) = parsed else {
        return Ok(None);
    };

    let mut defs: Vec<Def> = Vec::new();
    let mut refs: Vec<Ref> = Vec::new();

    // Rust's field types, read once per file, so a receiver such as
    // `self.members[i].store` can name the type its method belongs to.
    let rust = (lang == "rust").then(|| RustFields::of(tree.root_node(), text));
    for query in queries(lang, &language, tags) {
        collect(query, &tree, text, rust.as_ref(), &mut defs, &mut refs);
    }
    // Markdown's sections come from the one heading scanner the chunker cuts
    // with, which reads setext headings as well as ATX ones.
    if lang == "markdown" {
        defs.retain(|d| d.kind != "heading");
        defs.extend(markdown_sections(text));
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
    let contained = contained(&defs);
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

    let parents = enclosing_each(&defs);
    for (i, def) in defs.iter().enumerate() {
        let parent = parents[i].map(|p| defs[p].name.clone());
        // The owner outranks the enclosing definition: `Fleet::search_preferring`
        // is what the code calls it and what a qualified question asks for,
        // and `fleet::search_preferring` could be either of two methods.
        let qualified = match (&def.owner, &parent) {
            (Some(owner), _) => format!("{owner}::{}", def.name),
            (None, Some(p)) => format!("{p}::{}", def.name),
            (None, None) => format!("{module}::{}", def.name),
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
                from_module: false,
            }),
            None => edges.push(Edge {
                from: module.clone(),
                to: def.name.clone(),
                kind: "defines".to_string(),
                confidence: EXTRACTED.to_string(),
                hint: None,
                line: Some(def.start_line),
                from_module: true,
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

    // The class a definition sits in: the Rust `impl` the extractor read, or
    // the nearest enclosing definition of a class kind for everything else.
    // What `self`, `this` and `cls` are, and what makes a call recursion.
    let class_of = |at: Option<usize>| -> Option<String> {
        let mut at = at;
        if let Some(owner) = at.and_then(|i| defs[i].owner.clone()) {
            return Some(owner);
        }
        while let Some(i) = at {
            if defs[i].kind == "class" {
                return Some(defs[i].name.clone());
            }
            at = parents[i];
        }
        None
    };

    let holders = innermost_at(&defs, refs.iter().map(|r| r.at), None);
    for (reference, holder) in refs.iter().zip(holders) {
        let enclosing = holder.map(|h| &defs[h]);
        let from = match enclosing {
            Some(def) => def.name.clone(),
            None => module.clone(),
        };
        let owner = class_of(holder);
        // A receiver is what the call went through: its type where the
        // source states it, `self`'s class, or the receiver's own name. A
        // Python object the file imported as a module (`import json` then
        // `json.loads()`) is a module path, not a receiver.
        let lead = match (&reference.hint, reference.on_self) {
            (Some(h), _) => Some(h.clone()),
            (None, true) => owner.clone(),
            (None, false) => None,
        };
        let method = reference.method
            && !(lang == "python"
                && !reference.on_self
                && lead.as_ref().is_some_and(|l| {
                    imported.contains(l) || defs.iter().any(|d| d.kind == "alias" && d.name == *l)
                }));
        // Direct recursion adds a self-edge and no information. A call to the
        // same name through anything but the caller's own class is not
        // recursion: `Fleet::search_preferring` calling
        // `store.search_preferring()`, or helpers.py's `make_response` calling
        // `current_app.make_response(args)` (2026-10-14 audit, G2).
        let recursion = from == reference.name
            && match &lead {
                None => true,
                Some(l) => owner.as_ref() == Some(l) || (!method && l == "Self"),
            };
        if recursion {
            continue;
        }
        // The file said where this came from, one way or the other: it
        // imported the name itself (`use ...::edges_out;` then `edges_out()`),
        // or it imported the thing the call went through (`use ...::store;`
        // then `store::edges_out()`, or `use ...::HiArgs;` then a method on an
        // `&HiArgs`). The second is exactly as determined as the first — the
        // module the call names is in the file's own imports — and reading it
        // as inferred is what left Rust at 2% extracted while the source was
        // perfectly explicit.
        let named_by_file = reference.kind == "imports"
            || imported.contains(&reference.name)
            || lead.as_ref().is_some_and(|h| imported.contains(h));
        let confidence = if named_by_file { EXTRACTED } else { INFERRED };
        // A method call's hint carries a leading `.`, so the resolver knows the
        // call went through a receiver and can never mean a free function.
        let hint = if method {
            Some(format!("{RECEIVER}{}", lead.unwrap_or_default()))
        } else {
            reference.hint.clone()
        };
        edges.push(Edge {
            from,
            to: reference.name.clone(),
            kind: reference.kind.to_string(),
            confidence: confidence.to_string(),
            hint,
            line: Some(reference.line),
            from_module: enclosing.is_none(),
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
            a.from_module,
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
                b.from_module,
                confidence_rank(&b.confidence),
                b.hint.is_none(),
                &b.hint,
                b.line.is_none(),
                b.line,
            ))
    });
    edges.dedup_by(|a, b| {
        (&a.from, &a.to, &a.kind, a.from_module) == (&b.from, &b.to, &b.kind, b.from_module)
    });

    Ok(Some(Extraction { symbols, edges }))
}

fn collect(
    query: &Query,
    tree: &tree_sitter::Tree,
    text: &str,
    rust: Option<&RustFields>,
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
        let mut method = false;
        let mut references: Vec<(&str, tree_sitter::Node)> = Vec::new();
        for capture in m.captures() {
            let capture_name = names[capture.index as usize];
            if let Some(kind) = capture_name.strip_prefix("definition.") {
                span = Some((kind, capture.node));
            } else if capture_name == "name" {
                name = Some(capture.node);
            } else if capture_name == "hint" {
                hint = Some(capture.node);
            } else if capture_name == "receiver" {
                hint = Some(capture.node);
                method = true;
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
                // A receiver whose type the source states is a better lead
                // than its variable name: `store.search_preferring()` names a
                // file that defines no such method, and `Semlith` names the
                // one that does.
                hint: hint.and_then(|h| {
                    rust.and_then(|fields| receiver_type(h, text, fields))
                        .or_else(|| hint_text(h, text))
                }),
                method,
                on_self: hint
                    .is_some_and(|h| matches!(&text[h.byte_range()], "self" | "this" | "cls")),
            });
        }

        if let (Some((kind, node)), Some(name)) = (span, name) {
            // An alias is the clause that names it, and its name sits at the
            // clause's very end: climbing from there reached Python's whole
            // `module`, so `BaseResponse` spanned app.py and owned every class
            // in it (2026-10-14 audit, G4 and G5).
            let span = if kind == "alias" {
                node
            } else {
                definition_span(node, name)
            };
            let raw = &text[name.byte_range()];
            defs.push(Def {
                kind: kind.to_string(),
                name: if kind == "heading" {
                    crate::chunk::heading_title(raw)
                } else {
                    raw.to_string()
                },
                owner: rust.and_then(|_| rust_owner(span, text)),
                start: span.start_byte(),
                end: span.end_byte(),
                start_line: span.start_position().row as u32 + 1,
                end_line: span.end_position().row as u32 + 1,
            });
        }
    }
}

/// Each Markdown heading as a definition spanning its section: from its own
/// line to the line before the next heading at its depth or above, or the
/// file's last line.
fn markdown_sections(text: &str) -> Vec<Def> {
    let mut starts = vec![0usize];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    let lines = text.lines().count();
    let byte = |line: usize| starts.get(line).copied().unwrap_or(text.len());
    let found = crate::chunk::headings(text);
    found
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let next = found[i + 1..]
                .iter()
                .find(|n| n.level <= h.level)
                .map_or(lines + 1, |n| n.line as usize);
            let end_line = (next - 1).max(h.line as usize);
            Def {
                kind: "heading".to_string(),
                name: h.title.clone(),
                owner: None,
                start: byte(h.line as usize - 1),
                end: byte(end_line)
                    .saturating_sub(1)
                    .max(byte(h.line as usize - 1) + 1),
                start_line: h.line,
                end_line: end_line as u32,
            }
        })
        .collect()
}

/// A Rust type as far as a method call needs it: its name, and for a
/// container (`Vec<T>`, `Option<T>`, `[T]`) the element a `[i]` or a `for`
/// hands out.
#[derive(Debug, Clone, PartialEq)]
struct Ty {
    name: String,
    elem: Option<String>,
}

/// Standard containers and values: a receiver of one of these is not a
/// type in the corpus, so the variable's name stays the better lead.
const STD_TYPES: [&str; 14] = [
    "Vec", "VecDeque", "Option", "Result", "String", "str", "HashMap", "BTreeMap", "HashSet",
    "BTreeSet", "Path", "PathBuf", "[]", "Self",
];

/// The declared type of every struct field in one Rust file: by owner and
/// field, and by field name alone for a receiver whose owner is unknown.
///
/// A field name declared with two different types in one file maps to
/// nothing by name — `store: Semlith` beside `store: String` — and is then
/// answered only through its owner: `self.members[i].store` is `Member`'s.
struct RustFields {
    by_owner: std::collections::HashMap<(String, String), Ty>,
    by_name: std::collections::HashMap<String, Option<Ty>>,
}

impl RustFields {
    fn of(root: tree_sitter::Node, text: &str) -> RustFields {
        let mut out = RustFields {
            by_owner: Default::default(),
            by_name: Default::default(),
        };
        let mut cursor = root.walk();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if node.kind() == "field_declaration" {
                let owner = node
                    .parent()
                    .and_then(|list| list.parent())
                    .filter(|item| item.kind() == "struct_item")
                    .and_then(|item| item.child_by_field_name("name"))
                    .map(|n| text[n.byte_range()].to_string());
                let name = node.child_by_field_name("name");
                let ty = node
                    .child_by_field_name("type")
                    .and_then(|t| rust_ty(t, text));
                if let (Some(name), Some(ty)) = (name, ty) {
                    let field = text[name.byte_range()].to_string();
                    if let Some(owner) = owner {
                        out.by_owner.insert((owner, field.clone()), ty.clone());
                    }
                    let entry = out.by_name.entry(field).or_insert_with(|| Some(ty.clone()));
                    if entry.as_ref() != Some(&ty) {
                        *entry = None;
                    }
                }
                continue;
            }
            stack.extend(node.named_children(&mut cursor));
        }
        out
    }
}

/// The type a Rust definition is a member of: the nearest `impl` or `trait`
/// block around it. A function nested inside another function, or a module in
/// between, belongs to neither.
fn rust_owner(node: tree_sitter::Node, text: &str) -> Option<String> {
    let mut at = node.parent();
    while let Some(n) = at {
        match n.kind() {
            "impl_item" => return rust_type_name(n.child_by_field_name("type")?, text),
            "trait_item" => {
                return n
                    .child_by_field_name("name")
                    .map(|t| text[t.byte_range()].to_string());
            }
            "function_item" | "mod_item" | "closure_expression" => return None,
            _ => {}
        }
        at = n.parent();
    }
    None
}

/// The type a type node names, through references and the smart pointers a
/// method call sees through: `&mut Fleet`, `Arc<Mutex<Fleet>>` and
/// `crate::fleet::Fleet` all name `Fleet`.
fn rust_type_name(node: tree_sitter::Node, text: &str) -> Option<String> {
    rust_ty(node, text).map(|t| t.name)
}

/// [`rust_type_name`], keeping a container's element.
fn rust_ty(node: tree_sitter::Node, text: &str) -> Option<Ty> {
    const THROUGH: [&str; 7] = ["Box", "Rc", "Arc", "RefCell", "Mutex", "RwLock", "Cell"];
    let plain = |name: String| Some(Ty { name, elem: None });
    match node.kind() {
        "type_identifier" | "primitive_type" => plain(text[node.byte_range()].to_string()),
        "reference_type" | "pointer_type" => rust_ty(node.child_by_field_name("type")?, text),
        "scoped_type_identifier" => rust_ty(node.child_by_field_name("name")?, text),
        "array_type" => Some(Ty {
            name: "[]".to_string(),
            elem: node
                .child_by_field_name("element")
                .and_then(|e| rust_type_name(e, text)),
        }),
        "generic_type" => {
            let base = rust_type_name(node.child_by_field_name("type")?, text)?;
            let first = node.child_by_field_name("type_arguments").and_then(|args| {
                let mut cursor = args.walk();
                args.named_children(&mut cursor).next()
            });
            if THROUGH.contains(&base.as_str()) {
                return rust_ty(first?, text);
            }
            Some(Ty {
                name: base,
                elem: first.and_then(|f| rust_type_name(f, text)),
            })
        }
        _ => None,
    }
}

/// The type of a Rust method call's receiver, where the source states it.
///
/// `self` inside an `impl T` block; a parameter, `let` or `for` binding whose
/// type is declared or constructed (`x: T`, `T::new()`, `T::open(…)?`,
/// `T { … }`, `for x in &self.items`); and a field, through its owner's
/// declared type when the chain says whose field it is (`self.members[i].store`
/// is `Member.store`), or by its name when only one type in the file declares
/// it. A standard container is not a lead, so `None` and the receiver's own
/// name stays the hint, as it was before 0.30.0.
fn receiver_type(receiver: tree_sitter::Node, text: &str, fields: &RustFields) -> Option<String> {
    expr_ty(receiver, text, fields, 0)
        .map(|t| t.name)
        .filter(|name| !STD_TYPES.contains(&name.as_str()))
}

/// The type an expression evaluates to, where the source states it.
fn expr_ty(node: tree_sitter::Node, text: &str, fields: &RustFields, depth: u8) -> Option<Ty> {
    if depth > 8 {
        return None;
    }
    match node.kind() {
        "self" => {
            let mut at = node.parent();
            while let Some(n) = at {
                if n.kind() == "function_item" {
                    return rust_owner(n, text).map(|name| Ty { name, elem: None });
                }
                at = n.parent();
            }
            None
        }
        "identifier" => binding_ty(node, text, fields, depth),
        "field_expression" => {
            let field = text[node.child_by_field_name("field")?.byte_range()].to_string();
            let owner = node
                .child_by_field_name("value")
                .and_then(|v| expr_ty(v, text, fields, depth + 1));
            if let Some(owner) = owner
                && let Some(ty) = fields.by_owner.get(&(owner.name, field.clone()))
            {
                return Some(ty.clone());
            }
            fields.by_name.get(&field).cloned().flatten()
        }
        "index_expression" => {
            let container = expr_ty(node.named_child(0)?, text, fields, depth + 1)?;
            container.elem.map(|name| Ty { name, elem: None })
        }
        "reference_expression" | "parenthesized_expression" | "try_expression" => {
            let inner = node
                .child_by_field_name("value")
                .or_else(|| node.named_child(0))?;
            expr_ty(inner, text, fields, depth + 1)
        }
        _ => None,
    }
}

/// The declared or constructed type of a local named by `ident`: the last
/// parameter, `let` or `for` binding of that name in the enclosing function
/// that comes before the use.
fn binding_ty(ident: tree_sitter::Node, text: &str, fields: &RustFields, depth: u8) -> Option<Ty> {
    let name = &text[ident.byte_range()];
    let mut function = ident.parent();
    while let Some(f) = function {
        if f.kind() == "function_item" {
            break;
        }
        function = f.parent();
    }
    let function = function?;
    // ponytail: walks the whole enclosing function per method call; a
    // per-function binding table if a pathological file ever shows up in
    // the indexing profile.
    let mut found: Option<(usize, Ty)> = None;
    let mut cursor = function.walk();
    let mut stack = vec![function];
    while let Some(node) = stack.pop() {
        if node.start_byte() >= ident.start_byte() {
            continue;
        }
        let names_it = |field: &str| {
            node.child_by_field_name(field)
                .is_some_and(|p| p.kind() == "identifier" && &text[p.byte_range()] == name)
        };
        let ty = match node.kind() {
            "parameter" | "let_declaration" if names_it("pattern") => node
                .child_by_field_name("type")
                .and_then(|t| rust_ty(t, text))
                .or_else(|| {
                    node.child_by_field_name("value")
                        .and_then(|v| constructed_type(v, text))
                        .map(|name| Ty { name, elem: None })
                }),
            // `for x in &self.items`: the element of what is iterated.
            "for_expression" if names_it("pattern") => node
                .child_by_field_name("value")
                .and_then(|v| expr_ty(v, text, fields, depth + 1))
                .and_then(|t| t.elem)
                .map(|name| Ty { name, elem: None }),
            _ => None,
        };
        if let Some(ty) = ty
            && found.as_ref().is_none_or(|(at, _)| node.start_byte() > *at)
        {
            found = Some((node.start_byte(), ty));
        }
        stack.extend(node.named_children(&mut cursor));
    }
    found.map(|(_, ty)| ty)
}

/// `T::new()`, `T::open(…)?`, `T::load(…).unwrap()` and `T { … }` construct a
/// `T`. The upper-case check keeps `store::open()` — a module's function,
/// whose return type the call does not say — out of it.
fn constructed_type(value: tree_sitter::Node, text: &str) -> Option<String> {
    let upper = |t: &str| t.chars().next().is_some_and(char::is_uppercase);
    match value.kind() {
        "try_expression" => constructed_type(value.named_child(0)?, text),
        "struct_expression" => {
            rust_type_name(value.child_by_field_name("name")?, text).filter(|t| upper(t))
        }
        "call_expression" => {
            let function = value.child_by_field_name("function")?;
            match function.kind() {
                "scoped_identifier" => {
                    let path = function.child_by_field_name("path")?;
                    let ty = match path.kind() {
                        "identifier" | "type_identifier" => text[path.byte_range()].to_string(),
                        "scoped_identifier" => {
                            text[path.child_by_field_name("name")?.byte_range()].to_string()
                        }
                        _ => return None,
                    };
                    upper(&ty).then_some(ty)
                }
                // `T::open(…).unwrap()` and `.expect(…)`: the value is what
                // the call before them produced.
                "field_expression" => {
                    let method = &text[function.child_by_field_name("field")?.byte_range()];
                    if matches!(method, "unwrap" | "expect") {
                        constructed_type(function.child_by_field_name("value")?, text)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        _ => None,
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
    // The root is the file, never one definition's extent — Python's is called
    // `module`, which reads as declaration-shaped.
    while let Some(parent) = node.parent().filter(|p| p.parent().is_some()) {
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

// The three passes below answered by comparing every definition with every
// other one, and every reference with every definition: a planted file of
// 450,000 one-line functions was some 10^11 comparisons (audit S18). They are
// sweeps now, with the same answers — `graph::tests` checks them against the
// pairwise versions.

/// For each definition, whether another one of the same name and kind covers
/// it with a different range.
fn contained(defs: &[Def]) -> Vec<bool> {
    let mut out = vec![false; defs.len()];
    let mut groups: std::collections::HashMap<(&str, &str), Vec<usize>> =
        std::collections::HashMap::new();
    for (i, d) in defs.iter().enumerate() {
        groups.entry((&d.name, &d.kind)).or_default().push(i);
    }
    for mut group in groups.into_values() {
        // By start, widest first: whatever could cover a definition sorts
        // before it.
        group.sort_by_key(|&i| (defs[i].start, std::cmp::Reverse(defs[i].end)));
        let mut widest: Option<usize> = None;
        let mut run = 0;
        while run < group.len() {
            let range = (defs[group[run]].start, defs[group[run]].end);
            let mut next = run;
            while next < group.len() && (defs[group[next]].start, defs[group[next]].end) == range {
                next += 1;
            }
            // Only what sorted before this run: a definition with an equal
            // range is the same definition tagged twice, not a cover.
            let covered = widest.is_some_and(|end| end >= range.1);
            for &i in &group[run..next] {
                out[i] = covered;
            }
            widest = Some(widest.map_or(range.1, |end| end.max(range.1)));
            run = next;
        }
    }
    out
}

/// For each position in `at`, the innermost definition whose range holds it:
/// the narrowest, the earliest of equals, skipping `not` for the position at
/// the same index when given. One sweep over the positions in order, holding
/// the definitions open at each by width.
fn innermost_at(
    defs: &[Def],
    at: impl Iterator<Item = usize>,
    not: Option<&dyn Fn(usize) -> usize>,
) -> Vec<Option<usize>> {
    use std::cmp::Reverse;
    use std::collections::{BTreeSet, BinaryHeap};
    let at: Vec<usize> = at.collect();
    let mut order: Vec<usize> = (0..at.len()).collect();
    order.sort_by_key(|&q| at[q]);
    let mut by_start: Vec<usize> = (0..defs.len()).collect();
    by_start.sort_by_key(|&i| defs[i].start);
    let mut open: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut ends: BinaryHeap<Reverse<(usize, usize)>> = BinaryHeap::new();
    let mut next = 0;
    let mut out = vec![None; at.len()];
    for q in order {
        let pos = at[q];
        while next < by_start.len() && defs[by_start[next]].start <= pos {
            let d = &defs[by_start[next]];
            open.insert((d.end - d.start, by_start[next]));
            ends.push(Reverse((d.end, by_start[next])));
            next += 1;
        }
        while let Some(Reverse((end, i))) = ends.peek().copied() {
            if end > pos {
                break;
            }
            ends.pop();
            let d = &defs[i];
            open.remove(&(d.end - d.start, i));
        }
        let skip = not.map(|f| f(q));
        out[q] = open.iter().map(|&(_, i)| i).find(|&i| Some(i) != skip);
    }
    out
}

/// For each definition, the innermost other definition enclosing its start.
fn enclosing_each(defs: &[Def]) -> Vec<Option<usize>> {
    innermost_at(defs, defs.iter().map(|d| d.start), Some(&|q| q))
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

/// Whether an edge target is the language's own: a builtin (`len`, `print`,
/// `Ok`, `println!`), a call through a standard-library namespace
/// (`fmt.Errorf`, `os.path.join`, `std::mem::take`), or an import of one
/// (`"os"`, `std::io::Write`, `import json`).
///
/// For "Called and not found", which is a list of code the corpus does not
/// hold: cobra's was `Errorf` 198 times, `len`, `append` and `os` (2026-10-14
/// audit, item 30 of 0.42.0). A name this answers yes for is only ever asked
/// about when nothing in the store defines it, so a user function of the same
/// name is never hidden.
///
/// `hint` is the edge's stored hint, receiver mark and all. Covers the
/// languages a corpus is mostly made of — Go, Rust, Python, JavaScript and
/// TypeScript; any other answers no.
// ponytail: curated names, not the full standard libraries; widen a list when
// a gaps report shows a standard name.
pub fn is_builtin(language: &str, name: &str, kind: &str, hint: Option<&str>) -> bool {
    let hint = hint.map(|h| h.trim_start_matches(RECEIVER));
    let list = |names: &[&str], n: &str| names.contains(&n);
    match language {
        "go" => {
            const BUILTINS: &[&str] = &[
                "append", "cap", "clear", "close", "complex", "copy", "delete", "imag", "len",
                "make", "max", "min", "new", "panic", "print", "println", "real", "recover",
                "bool", "byte", "error", "float32", "float64", "int", "int8", "int16", "int32",
                "int64", "rune", "string", "uint", "uint8", "uint16", "uint32", "uint64",
                "uintptr", "any",
            ];
            const PACKAGES: &[&str] = &[
                "fmt",
                "os",
                "io",
                "strings",
                "strconv",
                "errors",
                "bytes",
                "sort",
                "sync",
                "time",
                "context",
                "filepath",
                "path",
                "bufio",
                "regexp",
                "reflect",
                "unicode",
                "utf8",
                "math",
                "rand",
                "json",
                "http",
                "url",
                "log",
                "flag",
                "exec",
                "runtime",
                "atomic",
                "template",
                "testing",
                "hex",
                "base64",
                "ioutil",
                "signal",
                "syscall",
                "user",
                "unsafe",
                "slices",
                "maps",
                "cmp",
                "iter",
                "tabwriter",
                "heap",
                "list",
            ];
            if kind == "imports" {
                // A standard package's path has no dot in its first element.
                let path = name.trim_matches('"');
                return !path.split('/').next().unwrap_or(path).contains('.');
            }
            list(BUILTINS, name) || hint.is_some_and(|h| list(PACKAGES, h))
        }
        "rust" => {
            const BUILTINS: &[&str] = &[
                "Ok",
                "Err",
                "Some",
                "Box",
                "drop",
                "panic!",
                "println!",
                "print!",
                "eprintln!",
                "eprint!",
                "format!",
                "vec!",
                "write!",
                "writeln!",
                "assert!",
                "assert_eq!",
                "assert_ne!",
                "debug_assert!",
                "debug_assert_eq!",
                "unreachable!",
                "todo!",
                "unimplemented!",
                "matches!",
                "concat!",
                "env!",
                "include_str!",
                "include_bytes!",
                "stringify!",
                "line!",
                "file!",
                "format_args!",
                "dbg!",
                "cfg!",
                "thread_local!",
                "compile_error!",
                // The standard traits' and types' methods every Rust file calls.
                "clone",
                "unwrap",
                "expect",
                "into",
                "iter",
                "into_iter",
                "iter_mut",
                "collect",
                "map",
                "to_string",
                "to_owned",
                "as_str",
                "as_ref",
                "as_deref",
                "len",
                "is_empty",
                "push",
                "push_str",
                "pop",
                "get",
                "get_mut",
                "insert",
                "remove",
                "contains",
                "contains_key",
                "unwrap_or",
                "unwrap_or_default",
                "unwrap_or_else",
                "ok",
                "err",
                "ok_or",
                "ok_or_else",
                "and_then",
                "filter",
                "filter_map",
                "next",
                "join",
                "split",
                "trim",
                "starts_with",
                "ends_with",
                "lock",
                "borrow",
                "borrow_mut",
                "take",
                "extend",
                "enumerate",
                "zip",
                "chain",
                "rev",
                "sort",
                "sort_by",
                "sort_by_key",
                "dedup",
                "retain",
                "entry",
                "or_insert",
                "or_default",
                "chars",
                "bytes",
                "lines",
                "find",
                "position",
                "any",
                "all",
                "count",
                "sum",
                "first",
                "last",
                "is_some",
                "is_none",
                "is_ok",
                "is_err",
                "from",
                "default",
                "fmt",
                "eq",
                "cmp",
                "hash",
                "with_capacity",
                "map_err",
                "flatten",
                "flat_map",
                "skip",
                "copied",
                "cloned",
                "truncate",
                "clear",
                "to_vec",
                "keys",
                "values",
                "display",
                "load",
                "store",
                "send",
                "recv",
                "spawn",
                "now",
                "elapsed",
            ];
            const NAMESPACES: &[&str] = &[
                "std",
                "core",
                "alloc",
                "String",
                "Vec",
                "Option",
                "Result",
                "Box",
                "Rc",
                "Arc",
                "HashMap",
                "HashSet",
                "BTreeMap",
                "BTreeSet",
                "VecDeque",
                "Path",
                "PathBuf",
                "File",
                "fs",
                "io",
                "env",
                "thread",
                "mem",
                "ptr",
                "fmt",
                "process",
                "Duration",
                "Instant",
                "Mutex",
                "RwLock",
                "AtomicBool",
                "AtomicUsize",
                "AtomicU64",
                "Ordering",
                "Cow",
                "Cell",
                "RefCell",
                "iter",
                "str",
                "char",
                "u8",
                "u32",
                "u64",
                "usize",
                "i32",
                "i64",
                "f64",
                "Self",
            ];
            if kind == "imports" {
                return ["std", "core", "alloc"]
                    .iter()
                    .any(|root| name == *root || name.starts_with(&format!("{root}::")));
            }
            list(BUILTINS, name) || hint.is_some_and(|h| list(NAMESPACES, h))
        }
        "python" => {
            const BUILTINS: &[&str] = &[
                "abs",
                "all",
                "any",
                "ascii",
                "bin",
                "bool",
                "breakpoint",
                "bytearray",
                "bytes",
                "callable",
                "chr",
                "classmethod",
                "compile",
                "complex",
                "delattr",
                "dict",
                "dir",
                "divmod",
                "enumerate",
                "eval",
                "exec",
                "filter",
                "float",
                "format",
                "frozenset",
                "getattr",
                "globals",
                "hasattr",
                "hash",
                "help",
                "hex",
                "id",
                "input",
                "int",
                "isinstance",
                "issubclass",
                "iter",
                "len",
                "list",
                "locals",
                "map",
                "max",
                "memoryview",
                "min",
                "next",
                "object",
                "oct",
                "open",
                "ord",
                "pow",
                "print",
                "property",
                "range",
                "repr",
                "reversed",
                "round",
                "set",
                "setattr",
                "slice",
                "sorted",
                "staticmethod",
                "str",
                "sum",
                "super",
                "tuple",
                "type",
                "vars",
                "zip",
                "__import__",
                "Exception",
                "ValueError",
                "TypeError",
                "KeyError",
                "IndexError",
                "RuntimeError",
                "NotImplementedError",
                "AttributeError",
                "OSError",
                "ImportError",
                "StopIteration",
                "AssertionError",
                "LookupError",
                "FileNotFoundError",
                "PermissionError",
                "UnicodeDecodeError",
                "Warning",
                "DeprecationWarning",
                "UserWarning",
                // The built-in types' methods.
                "append",
                "extend",
                "get",
                "items",
                "keys",
                "values",
                "join",
                "split",
                "strip",
                "rstrip",
                "lstrip",
                "startswith",
                "endswith",
                "replace",
                "lower",
                "upper",
                "pop",
                "update",
                "setdefault",
                "encode",
                "decode",
                "add",
                "remove",
                "insert",
                "sort",
                "copy",
                "count",
                "index",
                "discard",
                "partition",
                "rpartition",
                "splitlines",
                "isdigit",
                "isidentifier",
            ];
            const MODULES: &[&str] = &[
                "os",
                "sys",
                "re",
                "json",
                "typing",
                "t",
                "collections",
                "cabc",
                "functools",
                "itertools",
                "pathlib",
                "io",
                "time",
                "datetime",
                "logging",
                "subprocess",
                "shutil",
                "tempfile",
                "threading",
                "asyncio",
                "inspect",
                "contextlib",
                "dataclasses",
                "enum",
                "abc",
                "copy",
                "math",
                "random",
                "string",
                "textwrap",
                "traceback",
                "warnings",
                "weakref",
                "types",
                "uuid",
                "hashlib",
                "base64",
                "struct",
                "socket",
                "urllib",
                "http",
                "email",
                "unittest",
                "argparse",
                "pickle",
                "csv",
                "glob",
                "fnmatch",
                "operator",
                "queue",
                "signal",
                "platform",
                "importlib",
                "gzip",
                "zipfile",
                "decimal",
                "statistics",
                "secrets",
                "hmac",
                "ssl",
                "errno",
                "codecs",
                "locale",
                "pprint",
                "heapq",
                "bisect",
                "ast",
                "gc",
                "atexit",
                "mimetypes",
                "numbers",
                "__future__",
                "path",
            ];
            if kind == "imports" {
                let top = name
                    .trim_start_matches('.')
                    .split('.')
                    .next()
                    .unwrap_or(name);
                return !name.starts_with('.') && list(MODULES, top);
            }
            list(BUILTINS, name) || hint.is_some_and(|h| list(MODULES, h))
        }
        "javascript" | "typescript" | "tsx" => {
            const BUILTINS: &[&str] = &[
                "require",
                "parseInt",
                "parseFloat",
                "isNaN",
                "isFinite",
                "setTimeout",
                "setInterval",
                "clearTimeout",
                "clearInterval",
                "setImmediate",
                "Promise",
                "Object",
                "Array",
                "JSON",
                "Math",
                "Number",
                "String",
                "Boolean",
                "Date",
                "Error",
                "TypeError",
                "RangeError",
                "Symbol",
                "Map",
                "Set",
                "WeakMap",
                "RegExp",
                "encodeURIComponent",
                "decodeURIComponent",
                "encodeURI",
                "decodeURI",
                "fetch",
                "structuredClone",
                "queueMicrotask",
                "push",
                "pop",
                "shift",
                "unshift",
                "map",
                "filter",
                "forEach",
                "reduce",
                "join",
                "split",
                "slice",
                "splice",
                "indexOf",
                "includes",
                "keys",
                "values",
                "entries",
                "then",
                "catch",
                "finally",
                "log",
                "error",
                "warn",
                "info",
                "debug",
                "toString",
                "hasOwnProperty",
                "call",
                "apply",
                "bind",
                "replace",
                "trim",
                "concat",
                "find",
                "some",
                "every",
                "sort",
                "test",
                "exec",
                "match",
                "startsWith",
                "endsWith",
                "toLowerCase",
                "toUpperCase",
                "stringify",
                "parse",
                "assign",
                "freeze",
                "isArray",
                "resolve",
                "reject",
                "all",
                "get",
                "set",
                "has",
                "delete",
                "emit",
                "on",
            ];
            const MODULES: &[&str] = &[
                "fs",
                "path",
                "http",
                "https",
                "url",
                "util",
                "events",
                "stream",
                "os",
                "crypto",
                "child_process",
                "assert",
                "buffer",
                "querystring",
                "zlib",
                "net",
                "readline",
                "process",
                "console",
                "Object",
                "Array",
                "JSON",
                "Math",
                "Promise",
                "Number",
                "String",
                "Date",
                "Reflect",
            ];
            if kind == "imports" {
                let module = name.trim_matches(|c| c == '"' || c == '\'');
                return module.starts_with("node:") || list(MODULES, module);
            }
            list(BUILTINS, name) || hint.is_some_and(|h| list(MODULES, h))
        }
        _ => false,
    }
}

/// The same list as a SQL `NOT IN` list, for the two joins that resolve a name.
pub fn not_navigational(column: &str) -> String {
    let holes = NAVIGATIONAL_KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{column} NOT IN ({holes})")
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
    // Which of several definitions a caller resolved to: the difference
    // between `Fleet::search_preferring` and `Semlith::search_preferring`
    // is the whole answer to "what calls this one".
    let means = match &end.means {
        Some(m) => format!(" → {m}"),
        None => String::new(),
    };
    let Some(line) = end.line else {
        return means;
    };
    // A structural edge's line is where the definition starts, not a call:
    // printing "call at line 78" for `fn run`'s own line read as a call that
    // does not exist (2026-10-14 audit, G3).
    let what = if matches!(end.kind.as_str(), "defines" | "contains") {
        ("defined at", "defined on line")
    } else {
        ("called at", "call at line")
    };
    match &end.from_path {
        Some(path) => format!("  · {} {}:{line}{means}", what.0, shorten(path)),
        None => format!("  · {} {line}{means}", what.1),
    }
}

/// `1 definition`, `4 definitions`.
pub fn definitions_word(n: usize) -> String {
    format!("{n} definition{}", if n == 1 { "" } else { "s" })
}

/// One edge end as every surface prints it — `semlith neighbors`, `semlith
/// symbol` and their MCP replies — so the CLI and an agent read one line.
pub fn render_end(end: &crate::store::EdgeEnd, shorten: &dyn Fn(&str) -> String) -> String {
    let name = if end.symbol.kind == "module" {
        format!("{} (module)", end.symbol.name)
    } else {
        end.symbol.name.clone()
    };
    let site = call_site(end, shorten);
    // A collapsed row stands for several definitions and must not name one
    // of them: a reader of `get  src/a.rs:12` quotes that file, and it is one
    // of four the call could have meant. A caller (no `from_path`) is one
    // definite place whatever it called, so it keeps its path.
    if end.confidence == AMBIGUOUS && end.from_path.is_some() {
        return format!(
            "  {name} via {} ({}) · {}{site}",
            end.kind,
            end.confidence,
            definitions_word(end.definitions),
        );
    }
    let store = match &end.symbol.store {
        Some(label) => format!("[{label}] "),
        None => String::new(),
    };
    format!(
        "  {name} via {} ({})  {store}{}:{}{site}",
        end.kind,
        end.confidence,
        shorten(&end.symbol.path),
        end.symbol.start_line,
    )
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

/// Which files a graph answer may name: all of them, or the ones a path,
/// extension or language filter selects.
///
/// A store that holds many repositories holds many `run`s and `get`s, and on
/// the 727-question benchmark of 2026-10-01 caller precision was lost to
/// exactly that: definitions and callers from the other repositories. A scope
/// narrows the definitions in SQL (`symbols_by_names` takes the filter's own
/// predicate, so a name defined five hundred times outside the scope is not
/// read at all) and keeps out every caller, callee and reached row whose file
/// the filter does not select. `impact` checks it per hop, so a scoped walk
/// never expands from outside the scope.
#[derive(Debug, Default, Clone)]
pub struct Scope {
    groups: Vec<Vec<String>>,
    paths: Option<std::collections::HashSet<String>>,
}

impl Scope {
    /// Every file in the store.
    pub fn all() -> Self {
        Self::default()
    }

    /// The files `filter` selects in this store; [`Scope::all`] for an empty
    /// filter, with no query.
    pub fn of(db: &rusqlite::Connection, filter: &crate::filter::Filter) -> Result<Self> {
        if filter.is_empty() {
            return Ok(Self::all());
        }
        let paths = crate::store::filtered_paths(db, filter.groups())?;
        Ok(Self {
            groups: filter.groups().to_vec(),
            paths: Some(paths.into_iter().collect()),
        })
    }

    /// Whether a row in `path` belongs in the answer.
    pub fn holds(&self, path: &str) -> bool {
        self.paths.as_ref().is_none_or(|p| p.contains(path))
    }

    /// The definitions of `name` inside the scope, at most `limit`, code
    /// before headings, keys and selectors: ripgrep's twenty YAML `run:` keys
    /// filled `symbol run` ahead of its six Rust `run`s (2026-10-14 audit, G10).
    fn definitions(
        &self,
        db: &rusqlite::Connection,
        name: &str,
        limit: usize,
    ) -> Result<Vec<crate::store::SymbolRow>> {
        Ok(self.definitions_counted(db, name, limit)?.0)
    }

    /// [`Scope::definitions`], and how many there are in all.
    fn definitions_counted(
        &self,
        db: &rusqlite::Connection,
        name: &str,
        limit: usize,
    ) -> Result<(Vec<crate::store::SymbolRow>, usize)> {
        if self.paths.is_none() {
            return crate::store::symbols_named_counted(db, name, limit);
        }
        let mut rows = crate::store::symbols_by_names(
            db,
            std::slice::from_ref(&name.to_string()),
            &self.groups,
        )?;
        rows.sort_by_key(|r| NAVIGATIONAL_KINDS.contains(&r.kind.as_str()));
        let total = rows.len();
        rows.truncate(limit);
        Ok((rows, total))
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
    scope: &Scope,
) -> Result<Neighbours> {
    let (qualifier, bare) = split_qualified(name);
    let definitions = scope.definitions(db, bare, 64)?;
    let scoped = scope.paths.is_some();
    let defined: Vec<i64> = definitions.iter().map(|d| d.id).collect();
    // A qualifier that owns nothing is ignored rather than answered with
    // nothing: the bare name is the nearest true answer.
    let owned: Vec<&crate::store::SymbolRow> = match qualifier {
        Some(q) => definitions.iter().filter(|d| owned_by(d, q)).collect(),
        None => Vec::new(),
    };
    let owned_ids: Vec<i64> = owned.iter().map(|d| d.id).collect();

    let mut callers = Vec::new();
    for incoming in crate::store::edges_in_resolved(db, bare, kinds)? {
        if !owned_ids.is_empty() && !incoming.means.iter().any(|m| owned_ids.contains(m)) {
            continue;
        }
        // In scope means the caller's file is selected and what it called is
        // one of the definitions the scope holds, not a namesake elsewhere.
        if scoped
            && (!scope.holds(&incoming.end.symbol.path)
                || !incoming.means.iter().any(|m| defined.contains(m)))
        {
            continue;
        }
        let mut end = incoming.end;
        // An unsettled caller stays `ambiguous`, the word `impact` uses for
        // the same edge: rewording it `inferred` here gave one edge two
        // confidences (2026-10-14 audit, G8). The caller is still one
        // definition, so the row keeps its path.
        if end.definitions > 1 && incoming.means.len() == 1 {
            end.means = definitions
                .iter()
                .find(|d| d.id == incoming.means[0])
                .map(|d| d.qualified.clone());
        }
        callers.push(end);
    }

    let mut callees = crate::store::edges_out(db, bare, kinds)?;
    if scoped {
        callees.retain(|e| {
            e.from_path.as_deref().is_some_and(|p| scope.holds(p)) && scope.holds(&e.symbol.path)
        });
    }
    if !owned.is_empty() {
        callees.retain(|e| {
            owned.iter().any(|d| {
                e.from_path.as_deref() == Some(d.path.as_str()) && e.from_line == Some(d.start_line)
            })
        });
    }
    let unresolved = crate::store::unresolved_out(db, bare, kinds)?;
    Ok(Neighbours {
        callers,
        callees: if all { callees } else { collapse(callees) },
        hidden: if all { 0 } else { unresolved.len() },
        unresolved: if all { unresolved } else { Vec::new() },
    })
}

/// The most names one definitions table answers.
pub const NAMES_LIMIT: usize = 20;

/// One row of a definitions table: where a definition is, and its first line.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Signature {
    /// The name as it was asked, so a row answers a question.
    pub asked: String,
    #[serde(flatten)]
    pub symbol: crate::store::SymbolRow,
    /// The definition's first line, from the store's own chunks, trimmed. Empty
    /// when the store holds no chunk over that line.
    pub signature: String,
}

/// Every definition of several names, one compact row each, and no rings.
///
/// The question it answers is "where are these defined and what do they look
/// like" — three `semlith_symbol` calls before 0.30.0, each carrying callers,
/// callees and a second ring the question did not ask for. The rows are the
/// ones `semlith symbol` gives for each name alone, code first: leaving the
/// headings out made `symbol a b` drop a README heading `symbol a` shows
/// (2026-10-14 audit, G11).
pub fn signatures(
    db: &rusqlite::Connection,
    names: &[String],
    scope: &Scope,
) -> Result<Vec<Signature>> {
    let mut out = Vec::new();
    for asked in names.iter().take(NAMES_LIMIT) {
        let (qualifier, bare) = split_qualified(asked);
        let mut rows = scope.definitions(db, bare, 64)?;
        if let Some(q) = qualifier
            && rows.iter().any(|r| owned_by(r, q))
        {
            rows.retain(|r| owned_by(r, q));
        }
        for symbol in rows {
            let signature = crate::store::chunks_overlapping(
                db,
                &symbol.path,
                symbol.start_line,
                symbol.start_line,
            )?
            .iter()
            .find_map(|c| {
                let offset = symbol.start_line.checked_sub(c.start_line)? as usize;
                c.text.lines().nth(offset).map(|l| l.trim().to_string())
            })
            .unwrap_or_default();
            out.push(Signature {
                asked: asked.clone(),
                symbol,
                signature,
            });
        }
    }
    Ok(out)
}

/// A definitions table as text, one row per definition, and a last line
/// naming every asked name that has none.
pub fn render_signatures(
    rows: &[Signature],
    asked: &[String],
    shorten: &dyn Fn(&str) -> String,
) -> String {
    const WIDTH: usize = 110;
    let mut out = String::new();
    for row in rows {
        let mut signature = row.signature.clone();
        if signature.chars().count() > WIDTH {
            signature = signature.chars().take(WIDTH).collect::<String>() + "\u{2026}";
        }
        let store = match &row.symbol.store {
            Some(s) => format!("{s} "),
            None => String::new(),
        };
        out.push_str(&format!(
            "{} {} {store}{}:{}-{}  {}\n",
            row.symbol.qualified,
            row.symbol.kind,
            shorten(&row.symbol.path),
            row.symbol.start_line,
            row.symbol.end_line,
            signature
        ));
    }
    let missing: Vec<&str> = asked
        .iter()
        .filter(|n| !rows.iter().any(|r| r.asked == **n))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        out.push_str(&format!("no definition: {}\n", missing.join(", ")));
    }
    out.trim_end().to_string()
}

/// A graph answer's text fitted under [`ANSWER_CHARS`].
///
/// Lines are kept in order while they fit; the rest are counted per file, read
/// off each line's first `path:line`, and a `more:` line says what narrows
/// them. An answer already under the cap comes back untouched.
pub fn fit(text: String, narrow: &str) -> String {
    if text.len() <= ANSWER_CHARS {
        return text;
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::new();
    let mut kept = 0;
    for line in &lines {
        if out.len() + line.len() + 1 + 2_000 > ANSWER_CHARS {
            break;
        }
        out.push_str(line);
        out.push('\n');
        kept += 1;
    }
    let rest = &lines[kept..];
    let mut by_file: Vec<(String, usize)> = Vec::new();
    for line in rest {
        let file = line
            .split_whitespace()
            .find_map(|word| {
                let (path, number) = word.rsplit_once(':')?;
                let number = number.split('-').next()?;
                (!path.is_empty() && number.parse::<u32>().is_ok()).then(|| path.to_string())
            })
            .unwrap_or_else(|| "(no path)".to_string());
        match by_file.iter_mut().find(|(f, _)| *f == file) {
            Some((_, n)) => *n += 1,
            None => by_file.push((file, 1)),
        }
    }
    out.push_str(&format!("\n{} more lines, by file\n", rest.len()));
    let mut left = by_file.len();
    for (file, n) in &by_file {
        let line = format!("  {file} · {n}\n");
        if out.len() + line.len() + 300 > ANSWER_CHARS {
            out.push_str(&format!("  … {left} more files\n"));
            break;
        }
        out.push_str(&line);
        left -= 1;
    }
    out.push_str(&format!(
        "more: {} lines past the {ANSWER_CHARS}-character cap; {narrow}",
        rest.len()
    ));
    out
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
    /// The first `k`, code before headings, keys and selectors.
    pub definitions: Vec<crate::store::SymbolRow>,
    /// How many definitions the name has in all, of which `definitions` is
    /// the first `k`. `symbol run -k 3` said "3 definitions of this name" of
    /// 52 (2026-10-14 audit, G9).
    pub total: usize,
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
    scope: &Scope,
) -> Result<Evidence> {
    let (qualifier, bare) = split_qualified(name);
    let (mut definitions, mut total) = scope.definitions_counted(db, bare, limit)?;
    if let Some(q) = qualifier
        && definitions.iter().any(|d| owned_by(d, q))
    {
        // Every owned one, counted as the bare name is: the qualifier narrows
        // the question, and the count says what it narrowed to.
        let (every, _) = scope.definitions_counted(db, bare, usize::MAX)?;
        let owned: Vec<_> = every.into_iter().filter(|d| owned_by(d, q)).collect();
        total = owned.len();
        definitions = owned.into_iter().take(limit).collect();
    }
    let ring = neighbours(db, name, kinds, all, scope)?;

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
            if !scope.holds(&end.symbol.path) {
                continue;
            }
            // The centre is not two hops from itself, and a name already in
            // the first ring is context the reader has.
            if end.symbol.name == bare
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
        total,
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
        let total = self.total.max(self.definitions.len());
        if total > self.definitions.len() {
            out.push_str(&format!(
                "{total} definitions of this name, {} shown; k {total} lists every one\n",
                self.definitions.len()
            ));
        } else if total > 1 {
            out.push_str(&format!("{total} definitions of this name\n"));
        }
        for (heading, ends) in [("callers", &self.callers), ("callees", &self.callees)] {
            out.push_str(&format!("\n{bold}{heading}{reset} ({})\n", ends.len()));
            if ends.is_empty() {
                out.push_str("  none\n");
            }
            for end in ends.iter() {
                out.push_str(&render_end(end, shorten));
                out.push('\n');
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

/// How many definitions a target may have before the graph page stops
/// settling edges to it: the same cut search's walk makes.
const DRAW_HUB: usize = 32;

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
fn busiest(stores: &[(&str, &rusqlite::Connection)]) -> Result<Option<String>> {
    // Chosen once per store and graph: the choice reads the busiest symbols of
    // the whole store, seconds on a large one, and gives the same answer until
    // the symbol or edge count moves.
    type Chosen = (String, (i64, i64), Option<String>);
    static CHOSEN: std::sync::Mutex<Vec<Chosen>> = std::sync::Mutex::new(Vec::new());
    let mut key = String::new();
    let mut stats = (0i64, 0i64);
    for (_, db) in stores {
        key.push_str(db.path().unwrap_or(""));
        key.push('|');
        let (a, b) = crate::store::graph_stats(db)?;
        stats = (stats.0 + a, stats.1 + b);
    }
    if let Some((_, _, name)) = CHOSEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(k, st, _)| *k == key && *st == stats)
    {
        return Ok(name.clone());
    }
    let name = busiest_uncached(stores)?;
    let mut chosen = CHOSEN.lock().unwrap_or_else(|e| e.into_inner());
    chosen.retain(|(k, _, _)| *k != key);
    chosen.push((key, stats, name.clone()));
    Ok(name)
}

fn busiest_uncached(stores: &[(&str, &rusqlite::Connection)]) -> Result<Option<String>> {
    let mut best: Option<((usize, usize, usize), String)> = None;
    for (_, db) in stores {
        let db: &rusqlite::Connection = db;
        for name in crate::store::busiest_names(db, SEED_CANDIDATES * 4)? {
            // A target nothing in the store defines (a macro, a standard
            // library call) is busy and cannot be opened on.
            let definitions = crate::store::symbols_named(db, &name, 2)?.len();
            if definitions == 0 {
                continue;
            }
            // Counted, not settled: settling every edge of forty candidates
            // joined millions of candidate rows on the 879k-chunk corpus and
            // held the page, and every route behind it, for minutes.
            let (resolved, degree) = crate::store::edge_counts(db, &name)?;
            if degree == 0 {
                continue;
            }
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
            let unambiguous = usize::from(definitions <= 1);
            let score = (unambiguous, resolved, degree);
            if best.as_ref().is_none_or(|(seen, _)| score > *seen) {
                best = Some((score, name.clone()));
            }
        }
    }
    Ok(best.map(|(_, name)| name))
}

/// Takes connections rather than open stores, so the daemon can draw on a
/// read-only connection of its own instead of holding every store's lock for
/// the length of the drawing.
pub fn scoped(
    stores: &[(&str, &rusqlite::Connection)],
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

    for (label, db) in stores {
        let db: &rusqlite::Connection = db;
        let picked = match focus {
            Some(name) => {
                // The symbol itself, then what touches it. Deduplicated by the
                // caller, and cut at the budget: a name like `new` is called
                // from everywhere, and drawing all of it is a knot rather than
                // a neighbourhood. The rail lists every caller and callee
                // either way, so nothing is hidden by this — only undrawn.
                let mut around = crate::store::symbols_named(db, name, 4)?;
                let mut ends: Vec<crate::store::EdgeEnd> =
                    crate::store::edges_in_first(db, name, limit * 2)?
                        .into_iter()
                        .chain(crate::store::edges_out_short_of_hubs(
                            db,
                            name,
                            &[],
                            DRAW_HUB,
                        )?)
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
    let drawn: Vec<String> = index.keys().cloned().collect();
    for (from_name, from_index) in &index {
        for (_, db) in stores {
            for end in crate::store::edges_out_to(db, from_name, &drawn)? {
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

/// The most characters a graph answer's text may take, at any depth.
///
/// Claude Code refuses an MCP reply past its output limit and hands the agent
/// nothing at all, which is what `search_preferring` at the default depth did
/// on 2026-09-25 with 75 012 characters. Past this, rows collapse into
/// per-file counts and a `more:` line says what narrows them.
pub const ANSWER_CHARS: usize = 16_000;

/// One definition that reaches the symbol asked about.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Reached {
    pub name: String,
    #[serde(serialize_with = "crate::serialize_plain")]
    pub path: String,
    /// Where the reaching definition starts.
    pub line: u32,
    /// Where the call, import or reference was written, from `edges.line`.
    /// `None` on an edge an older binary wrote; a renderer then says a
    /// re-index adds it rather than printing the definition's line as though
    /// it were the call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<u32>,
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
    /// The qualifier asked for matched no definition's owner, so the answer
    /// is about the bare name, and says so.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unqualified: bool,
    /// Counted the same way a chain's summary is counted, so the page and the
    /// terminal cannot disagree about what the answer is made of.
    pub extracted: usize,
    pub resolved: usize,
    pub inferred: usize,
    pub ambiguous: usize,
}

/// `Type::method`, `module::function` or `Type.method`, split into the
/// qualifier and the bare name. A name that is not qualified, or whose parts
/// are not identifiers — a Markdown heading with a version in it — comes back
/// whole.
pub fn split_qualified(name: &str) -> (Option<&str>, &str) {
    let ident = |t: &str| !t.is_empty() && t.chars().all(|c| c.is_alphanumeric() || c == '_');
    let split = name.rsplit_once("::").or_else(|| name.rsplit_once('.'));
    match split {
        Some((owner, bare)) => {
            let last = owner.rsplit("::").next().unwrap_or(owner);
            if ident(last) && ident(bare) {
                (Some(last), bare)
            } else {
                (None, name)
            }
        }
        None => (None, name),
    }
}

/// Whether a definition belongs to `qualifier`: its owning type, its module
/// (the file's stem) or a directory it sits in.
pub fn owned_by(row: &crate::store::SymbolRow, qualifier: &str) -> bool {
    crate::store::owner_of(&row.qualified) == Some(qualifier)
        || module_name(Path::new(&row.path)) == qualifier
        || Path::new(&row.path)
            .parent()
            .is_some_and(|p| p.components().any(|c| c.as_os_str() == qualifier))
}

/// Everything that reaches `name`, breadth first, up to `depth` hops.
///
/// The mirror of [`shortest_path`] and it walks the same graph under the same
/// rule: a node is a definition, not a name, and an ambiguous edge is not
/// crossed unless `all_edges` says to. Breadth first, so the hop recorded
/// against a definition is the fewest hops it takes — a caller that reaches
/// the symbol both directly and through three others is a direct caller.
///
/// From 0.30.0 each hop asks what calls *this definition*: an edge into a
/// name is kept only where the resolver says it means the definition being
/// walked from. Before, a caller of any `run` counted as reaching the `run`
/// in `main.rs`, which is most of why one answer was 75 012 characters.
pub fn impact(
    db: &rusqlite::Connection,
    name: &str,
    kinds: &[String],
    depth: u32,
    all_edges: bool,
    scope: &Scope,
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
    let (qualifier, bare) = split_qualified(name);
    let mut definitions = scope.definitions(db, bare, 64)?;
    let mut unqualified = false;
    if let Some(q) = qualifier {
        let owned: Vec<_> = definitions
            .iter()
            .filter(|d| owned_by(d, q))
            .cloned()
            .collect();
        if owned.is_empty() {
            unqualified = !definitions.is_empty();
        } else {
            definitions = owned;
        }
    }
    let mut answer = Impact {
        name: name.to_string(),
        depth,
        all_edges,
        definitions,
        reached: Vec::new(),
        files: Vec::new(),
        hidden: 0,
        unqualified,
        extracted: 0,
        resolved: 0,
        inferred: 0,
        ambiguous: 0,
    };

    // Seen by definition, so two functions sharing a name are two nodes, and
    // the centre itself is never reported as reaching itself.
    let mut seen: std::collections::HashSet<Node> = std::collections::HashSet::new();
    for row in &answer.definitions {
        seen.insert(Node {
            name: row.name.clone(),
            path: row.path.clone(),
            line: row.start_line,
        });
    }
    let asked: std::collections::HashSet<i64> = answer.definitions.iter().map(|d| d.id).collect();

    let mut incoming: std::collections::HashMap<String, Vec<crate::store::Incoming>> =
        std::collections::HashMap::new();
    let mut frontier: Vec<crate::store::SymbolRow> = answer.definitions.clone();
    for hop in 1..=depth {
        let mut next: Vec<crate::store::SymbolRow> = Vec::new();
        for target in &frontier {
            if !incoming.contains_key(&target.name) {
                let found = crate::store::edges_in_resolved(db, &target.name, kinds)?;
                incoming.insert(target.name.clone(), found);
            }
            for edge in &incoming[&target.name] {
                if !edge.means.contains(&target.id) {
                    continue;
                }
                // An edge that could mean several definitions still reaches
                // the question when every one of them is a definition that was
                // asked about: `search_preferring` asked bare means both.
                let within = hop == 1 && edge.means.iter().all(|id| asked.contains(id));
                if !all_edges && edge.end.confidence == AMBIGUOUS && !within {
                    continue;
                }
                let caller = &edge.end.symbol;
                if !scope.holds(&caller.path) {
                    continue;
                }
                let node = Node {
                    name: caller.name.clone(),
                    path: caller.path.clone(),
                    line: caller.start_line,
                };
                if !seen.insert(node) {
                    continue;
                }
                if answer.reached.len() >= IMPACT_LIMIT {
                    answer.hidden += 1;
                    continue;
                }
                match edge.end.confidence.as_str() {
                    EXTRACTED => answer.extracted += 1,
                    RESOLVED => answer.resolved += 1,
                    INFERRED => answer.inferred += 1,
                    _ => answer.ambiguous += 1,
                }
                next.push(caller.clone());
                answer.reached.push(Reached {
                    name: caller.name.clone(),
                    path: caller.path.clone(),
                    line: caller.start_line,
                    at: edge.end.line,
                    hop,
                    kind: edge.end.kind.clone(),
                    confidence: edge.end.confidence.clone(),
                    via: target.name.clone(),
                });
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }

    // Files, nearest first, then by how much of the file is involved. A file
    // that reaches the symbol four ways at six hops is further away than one
    // that reaches it once at one hop, and reading it in that order is how
    // somebody decides what to open.
    answer.files = files_of(&answer.reached);
    Ok(answer)
}

/// The reached rows gathered per file, nearest first.
pub fn files_of(reached: &[Reached]) -> Vec<ReachedFile> {
    let mut by_file: std::collections::BTreeMap<String, (usize, u32)> =
        std::collections::BTreeMap::new();
    for row in reached {
        let entry = by_file.entry(row.path.clone()).or_insert((0, row.hop));
        entry.0 += 1;
        entry.1 = entry.1.min(row.hop);
    }
    let mut files: Vec<ReachedFile> = by_file
        .into_iter()
        .map(|(path, (symbols, nearest))| ReachedFile {
            path,
            symbols,
            nearest,
        })
        .collect();
    files.sort_by(|a, b| {
        a.nearest
            .cmp(&b.nearest)
            .then(b.symbols.cmp(&a.symbols))
            .then(a.path.cmp(&b.path))
    });
    files
}

impl Impact {
    /// The whole answer as text, for the terminal and for an agent.
    ///
    /// One renderer, as the chain has one: the CLI and the MCP reply differ
    /// only in whether they are allowed to use bold. It never passes
    /// [`ANSWER_CHARS`]: rows that do not fit are counted per file instead.
    pub fn render(&self, bold: &str, reset: &str, shorten: &dyn Fn(&str) -> String) -> String {
        let mut out = self.headline();
        if self.unqualified {
            out.push_str(&format!(
                "\nno definition of {} is owned by {}; this is every {}",
                split_qualified(&self.name).1,
                split_qualified(&self.name).0.unwrap_or_default(),
                split_qualified(&self.name).1
            ));
        }
        if !self.definitions.is_empty() {
            let defs: Vec<String> = self
                .definitions
                .iter()
                .map(|d| format!("{}:{}", shorten(&d.path), d.start_line))
                .collect();
            out.push_str(&format!("\ndefined at {}", defs.join(", ")));
        }
        if self.reached.is_empty() {
            return out;
        }
        // Room kept for the files block and the closing lines, so the rows
        // are what gives way.
        let files_block = self.files_block(bold, reset, shorten);
        let reserve = files_block.len().min(ANSWER_CHARS / 4) + 400;
        let mut hop = 0;
        let mut shown = 0usize;
        let mut undated = 0usize;
        for row in &self.reached {
            let mut chunk = String::new();
            if row.hop != hop {
                chunk.push_str(&format!(
                    "\n\n{bold}{} hop{}{reset}",
                    row.hop,
                    if row.hop == 1 { "" } else { "s" }
                ));
            }
            let at = match row.at {
                Some(at) => format!("{}:{at} in {}", shorten(&row.path), row.name),
                None => {
                    undated += 1;
                    format!(
                        "{}:{} {} (definition)",
                        shorten(&row.path),
                        row.line,
                        row.name
                    )
                }
            };
            chunk.push_str(&format!(
                "\n  {at} · {} {} · reaches {}",
                row.kind, row.confidence, row.via
            ));
            if out.len() + chunk.len() + reserve > ANSWER_CHARS {
                break;
            }
            if row.hop != hop {
                hop = row.hop;
            }
            out.push_str(&chunk);
            shown += 1;
        }
        let collapsed = self.reached.len() - shown;
        if collapsed > 0 {
            let rest = files_of(&self.reached[shown..]);
            out.push_str(&format!("\n\n{bold}{collapsed} more, by file{reset}"));
            let mut left = rest.len();
            for file in &rest {
                let line = format!(
                    "\n  {} · {} · nearest {} hop{}",
                    shorten(&file.path),
                    file.symbols,
                    file.nearest,
                    if file.nearest == 1 { "" } else { "s" }
                );
                if out.len() + line.len() + 300 > ANSWER_CHARS {
                    out.push_str(&format!("\n  … {left} more files"));
                    break;
                }
                out.push_str(&line);
                left -= 1;
            }
            let nearer = self.reached[shown].hop.saturating_sub(1).max(1);
            out.push_str(&format!(
                "\nmore: {collapsed} rows past the {ANSWER_CHARS}-character cap; depth: {nearer} lists them, or ask about one of the callers above"
            ));
        } else if out.len() + files_block.len() < ANSWER_CHARS {
            out.push_str(&files_block);
        }
        if undated > 0 {
            out.push_str(&format!(
                "\n{undated} row{} show{} the caller's definition: this store has no call-site lines for {}; a re-index adds them",
                if undated == 1 { "" } else { "s" },
                if undated == 1 { "s" } else { "" },
                if undated == 1 { "it" } else { "them" }
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

    fn files_block(&self, bold: &str, reset: &str, shorten: &dyn Fn(&str) -> String) -> String {
        let mut out = format!("\n\n{bold}files{reset}");
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
        out
    }

    /// The sentence that says what the answer is, printed by every surface so
    /// none of them writes its own.
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
mod nesting_tests {
    use super::*;

    #[test]
    fn the_four_crashing_shapes_are_too_nested_and_real_files_are_not() {
        let d = 260;
        let quotes = "> ".repeat(d) + "x\n";
        let list: String = (0..d).map(|i| "  ".repeat(i) + "- a\n").collect();
        let seq: String = (0..d).map(|i| "  ".repeat(i) + "-\n").collect();
        let flat = "- ".repeat(d) + "x\n";
        assert!(too_nested("markdown", &quotes));
        assert!(too_nested("markdown", &list));
        assert!(too_nested("yaml", &seq));
        assert!(too_nested("yaml", &flat));
        // The same shapes at a depth people write are left to the grammar.
        let list: String = (0..40).map(|i| "  ".repeat(i) + "- a\n").collect();
        assert!(!too_nested("markdown", &list));
        assert!(!too_nested(
            "markdown",
            "> a quote\n>> a reply\n- item\n  - sub\n"
        ));
        assert!(
            !too_nested("rust", &quotes),
            "only the block grammars are capped"
        );
    }
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

    /// The fuzzer's SEGV and bash's twin: a character past U+00FF right after
    /// a character a scanner tests with narrow `isdigit` is replaced, byte for
    /// byte, and nothing else changes.
    #[test]
    fn a_wide_character_after_a_digit_is_kept_from_a_narrow_ctype_scanner() {
        let md = " 1\u{5641A}\u{FFFD}";
        let safe = ctype_safe("markdown", md);
        assert_eq!(safe.len(), md.len());
        assert_eq!(safe, " 1aaaa\u{FFFD}");
        assert_eq!(
            ctype_safe("bash", "echo {é..9} {\u{1F600}"),
            "echo {é..9} {aaaa"
        );
        assert!(matches!(
            ctype_safe("markdown", "1. café 2026 \u{1F600}"),
            std::borrow::Cow::Borrowed(_)
        ));
        assert!(matches!(
            ctype_safe("rust", md),
            std::borrow::Cow::Borrowed(_)
        ));
        // The graph of the original text, names read from the original.
        let g = extract(Path::new("a.md"), "# 1\u{1F600} heading\n\nbody\n")
            .unwrap()
            .unwrap();
        assert!(
            g.symbols.iter().any(|s| s.name.contains('\u{1F600}')),
            "{:?}",
            g.symbols
        );
    }

    /// The fuzzer's crash: nesting carried on past NULs on one line.
    #[test]
    fn nesting_that_continues_past_a_control_character_is_too_deep() {
        let line = format!("{}{}{}", ">".repeat(100), "\0".repeat(100), ">".repeat(130));
        assert!(too_nested("markdown", &line));
        assert!(!too_nested("markdown", &format!("{}\0text", ">".repeat(5))));
    }

    /// The pairwise versions the sweeps replaced (audit S18), as the oracle.
    fn contained_pairwise(defs: &[Def]) -> Vec<bool> {
        defs.iter()
            .map(|inner| {
                defs.iter().any(|outer| {
                    outer.name == inner.name
                        && outer.kind == inner.kind
                        && (outer.start, outer.end) != (inner.start, inner.end)
                        && outer.start <= inner.start
                        && outer.end >= inner.end
                })
            })
            .collect()
    }

    fn innermost_pairwise(defs: &[Def], skip: Option<usize>, at: usize) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (i, d) in defs.iter().enumerate() {
            if Some(i) == skip || d.start > at || d.end <= at {
                continue;
            }
            if best.is_none_or(|b| d.end - d.start < defs[b].end - defs[b].start) {
                best = Some(i);
            }
        }
        best
    }

    #[test]
    fn the_sweeps_answer_what_the_pairwise_passes_did() {
        // A small deterministic generator: nested, crossing, equal and
        // duplicated ranges, a few shared names.
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n) as usize
        };
        for _ in 0..300 {
            let count = 1 + next(40);
            let defs: Vec<Def> = (0..count)
                .map(|_| {
                    let start = next(200);
                    let end = start + 1 + next(80);
                    Def {
                        kind: ["function", "class"][next(2)].to_string(),
                        name: ["a", "b", "c"][next(3)].to_string(),
                        owner: None,
                        start,
                        end,
                        start_line: 1,
                        end_line: 1,
                    }
                })
                .collect();
            assert_eq!(contained(&defs), contained_pairwise(&defs));
            let each = enclosing_each(&defs);
            for (i, d) in defs.iter().enumerate() {
                assert_eq!(each[i], innermost_pairwise(&defs, Some(i), d.start));
            }
            let at: Vec<usize> = (0..60).map(|_| next(300)).collect();
            let got = innermost_at(&defs, at.iter().copied(), None);
            for (q, pos) in at.iter().enumerate() {
                assert_eq!(got[q], innermost_pairwise(&defs, None, *pos));
            }
        }
    }
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

    /// A method call's receiver is a lead too, reduced to its last identifier
    /// and marked as a receiver. `self` outside any `impl` names no type, so
    /// the mark stands alone.
    #[test]
    fn a_rust_method_call_hints_with_its_receiver() {
        let e = run(
            "a.rs",
            "fn go(&self) { self.index.search(v); self.flush(); }\n",
        );
        assert_eq!(hint_of(&e, "search", "calls").as_deref(), Some(".index"));
        assert_eq!(hint_of(&e, "flush", "calls").as_deref(), Some("."));
    }

    /// A Python object the file imports as a module is a module path; any
    /// other object is a receiver, and `self` is the enclosing class.
    #[test]
    fn python_and_typescript_qualified_calls_carry_the_object_as_a_hint() {
        let py = run(
            "m.py",
            "import json\n\nclass Store:\n    def go(self):\n        json.loads(raw)\n        app.render()\n        self.flush()\n",
        );
        assert_eq!(hint_of(&py, "loads", "calls").as_deref(), Some("json"));
        assert_eq!(hint_of(&py, "render", "calls").as_deref(), Some(".app"));
        assert_eq!(hint_of(&py, "flush", "calls").as_deref(), Some(".Store"));

        let ts = run("app.ts", "function go(){ store.search(q); }\n");
        assert_eq!(hint_of(&ts, "search", "calls").as_deref(), Some("store"));
    }

    /// A Rust method's receiver names its type where the source says it, and
    /// a method is qualified by the type it is implemented on (1.11).
    #[test]
    fn rust_receivers_name_their_type_and_methods_their_owner() {
        let src = "struct Member { store: Semlith }\n\
            struct Unreadable { store: String }\n\
            struct Fleet { members: Vec<Member> }\n\
            impl Fleet {\n\
                fn search_in(&self) { self.search_preferring(); }\n\
                fn search_preferring(&self) { self.members[0].store.search_preferring(); }\n\
            }\n\
            fn run(fleet: &mut Fleet) { fleet.open(); }\n\
            fn build() { let s = Semlith::open(p)?; s.index(); let t = Other { a: 1 }; t.go(); }\n";
        let e = run("fleet.rs", src);
        let owner_of = |name: &str| {
            e.symbols
                .iter()
                .find(|s| s.name == name)
                .map(|s| s.qualified.clone())
        };
        assert_eq!(owner_of("search_in").as_deref(), Some("Fleet::search_in"));
        assert_eq!(owner_of("run").as_deref(), Some("fleet::run"));
        let hint = |from: &str, to: &str| {
            e.edges
                .iter()
                .find(|x| x.from == from && x.to == to && x.kind == "calls")
                .and_then(|x| x.hint.clone())
        };
        assert_eq!(
            hint("search_in", "search_preferring").as_deref(),
            Some(".Fleet")
        );
        // Not recursion: the same name, on a field declared `store: Semlith`.
        assert_eq!(
            hint("search_preferring", "search_preferring").as_deref(),
            Some(".Semlith"),
            "{:?}",
            e.edges
        );
        assert_eq!(hint("run", "open").as_deref(), Some(".Fleet"));
        assert_eq!(hint("build", "index").as_deref(), Some(".Semlith"));
        assert_eq!(hint("build", "go").as_deref(), Some(".Other"));
        // A `for` binding is the element of what it iterates.
        let e = run(
            "a.rs",
            "struct Item; impl Item { fn go(&self) {} }\nstruct Bag { items: Vec<Item> }\nimpl Bag { fn all(&self) { for it in &self.items { it.go(); } } }\n",
        );
        let go = e
            .edges
            .iter()
            .find(|x| x.from == "all" && x.to == "go")
            .and_then(|x| x.hint.clone());
        assert_eq!(go.as_deref(), Some(".Item"), "{:?}", e.edges);
    }

    /// Recursion still adds no edge.
    #[test]
    fn rust_recursion_on_self_adds_no_edge() {
        let e = run(
            "a.rs",
            "struct A;\nimpl A { fn go(&self) { self.go(); } }\n",
        );
        assert!(!has_edge(&e, "go", "go", "calls"), "{:?}", e.edges);
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

    /// Two repositories in one store, each with its own `run` and its own
    /// caller of it. Scoped to one, every graph answer names that repository
    /// only -- the definition, the caller, the reached row -- and unscoped it
    /// names both, as before.
    #[test]
    fn a_scope_keeps_a_namesake_in_another_repository_out() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::prepare_for_tests(&db);
        for repo in ["one", "two"] {
            let file =
                crate::store::insert_file(&db, &format!("/w/{repo}/x.rs"), "h", 1, 0).unwrap();
            let mut ids = Vec::new();
            for (name, line) in [("run", 1), (format!("go_{repo}").as_str(), 5)] {
                let symbol = Symbol {
                    kind: "function".to_string(),
                    name: name.to_string(),
                    qualified: name.to_string(),
                    start_line: line,
                    end_line: line + 2,
                };
                ids.push(crate::store::insert_symbol(&db, file, None, &symbol).unwrap());
            }
            crate::store::insert_edge(&db, ids[1], "run", "calls", EXTRACTED, None, None).unwrap();
        }
        let filter = crate::filter::Filter::new(&["one/**".to_string()], &[], &[]).unwrap();
        let scope = Scope::of(&db, &filter).unwrap();
        let names = |ends: &[crate::store::EdgeEnd]| {
            let mut n: Vec<String> = ends.iter().map(|e| e.symbol.name.clone()).collect();
            n.sort();
            n
        };

        let around = neighbours(&db, "run", &[], true, &scope).unwrap();
        assert_eq!(names(&around.callers), ["go_one"]);
        let everywhere = neighbours(&db, "run", &[], true, &Scope::all()).unwrap();
        assert_eq!(names(&everywhere.callers), ["go_one", "go_two"]);

        let reach = impact(&db, "run", &[], 3, false, &scope).unwrap();
        assert!(
            reach.definitions.iter().all(|d| d.path.contains("/one/")),
            "{reach:?}"
        );
        assert!(
            reach.reached.iter().all(|r| r.path.contains("/one/")),
            "{reach:?}"
        );
        assert!(
            reach.reached.iter().any(|r| r.name == "go_one"),
            "{reach:?}"
        );

        let rows = signatures(&db, &["run".to_string()], &scope).unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(rows[0].symbol.path.contains("/one/"));

        let found = evidence(&db, "run", &[], 20, true, &scope).unwrap();
        assert!(found.definitions.iter().all(|d| d.path.contains("/one/")));
        assert_eq!(names(&found.callers), ["go_one"]);
    }

    /// The block is one reply where 0.15.0 needed three, and the second ring
    /// is walked only through names the first ring settled — an ambiguous name
    /// is several unrelated definitions, and expanding one would put somebody
    /// else's callers into this symbol's context.
    #[test]
    fn the_evidence_block_carries_the_definition_both_rings_and_no_ambiguity() {
        let db = chain();
        let found = evidence(&db, "b", &dependency_kinds(), 20, false, &Scope::all()).unwrap();

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
        let n = neighbours(&db, "d", &[], false, &Scope::all()).unwrap();
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

/// The 2026-10-14 audit's graph defects (G1-G11 and item 30 of 0.42.0), each
/// replayed on a cut-down copy of the file the audit read, indexed under the
/// path it had there. Red on 0.41.x; the audit is
/// `full-regression-drive/14-10-2026/findings.md`.
#[cfg(test)]
mod audit_tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/graph");
        std::fs::read_to_string(root.join(name)).unwrap()
    }

    /// A store holding each fixture under the path the audit indexed it at,
    /// written by the writer `index_set` uses.
    fn store(files: &[(&str, &str)]) -> rusqlite::Connection {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::prepare_for_tests(&db);
        for (path, name) in files {
            let text = fixture(name);
            let file = crate::store::insert_file(&db, path, path, text.len() as u64, 0).unwrap();
            let e = extract(Path::new(path), &text).unwrap().unwrap();
            crate::store::write_extraction(&db, &e, file, &[]).unwrap();
        }
        db
    }

    fn ripgrep() -> rusqlite::Connection {
        store(&[
            ("crates/core/main.rs", "rust/ripgrep_main.rs"),
            ("crates/ignore/src/walk.rs", "rust/ripgrep_walk.rs"),
            ("crates/core/index/enabled.rs", "rust/ripgrep_enabled.rs"),
        ])
    }

    fn flask() -> rusqlite::Connection {
        store(&[
            ("src/flask/app.py", "python/flask_app.py"),
            ("src/flask/helpers.py", "python/flask_helpers.py"),
            ("src/flask/__init__.py", "python/flask_init.py"),
            ("tests/test_basic.py", "python/flask_test_basic.py"),
        ])
    }

    /// G1: `.build_parallel().run(||` is `WalkParallel::run`, and the audit got
    /// `main::run` "resolved". A method call never resolves to a free
    /// function, and a receiver nothing types leaves the edge ambiguous —
    /// `clone`, `clear` and `unwrap` the same.
    #[test]
    fn a_method_call_is_never_resolved_to_a_free_function() {
        let db = ripgrep();
        for from in ["search_parallel", "files_parallel"] {
            let out = crate::store::edges_out(&db, from, &dependency_kinds()).unwrap();
            for to in ["run", "clone", "clear", "unwrap"] {
                for e in out.iter().filter(|e| e.symbol.name == to) {
                    assert_eq!(
                        e.confidence, AMBIGUOUS,
                        "{from} -> {to} settled on {}:{} ({})",
                        e.symbol.path, e.symbol.start_line, e.symbol.qualified
                    );
                    assert!(
                        e.symbol.qualified != "main::run",
                        "{from} -> {to} names the free fn main::run"
                    );
                }
            }
        }
    }

    /// G1: on that edge `path files_parallel search_parallel` printed a 2-hop
    /// "resolved" chain and trace cited main.rs:301 as a supporting fact.
    #[test]
    fn no_path_or_trace_crosses_an_untyped_method_call() {
        let db = ripgrep();
        let chain = shortest_path(&db, "files_parallel", "search_parallel", 6, false).unwrap();
        assert!(chain.is_none(), "{chain:?}");
        let t = trace(
            "files_parallel",
            "search_parallel",
            6,
            false,
            chain,
            |_, _| None,
            &verbatim,
        );
        assert!(
            t.lines.iter().all(|l| l.mark != "supporting fact"),
            "{:?}",
            t.lines
        );
    }

    /// G2: `crate::search_parallel(args, mode)` on enabled.rs:14 is a caller.
    #[test]
    fn a_call_through_crate_is_a_caller() {
        let db = ripgrep();
        let n = neighbours(&db, "search_parallel", &[], false, &Scope::all()).unwrap();
        assert!(
            n.callers.iter().any(|c| c.symbol.name == "read"
                && c.symbol.path == "crates/core/index/enabled.rs"
                && c.line == Some(14)),
            "{:?}",
            n.callers
        );
    }

    /// G2: helpers.py `make_response` calls `current_app.make_response(args)`
    /// — another receiver's method of the same name, not itself.
    #[test]
    fn a_namesake_called_on_another_receiver_is_not_recursion() {
        let db = flask();
        let n = neighbours(&db, "Flask.make_response", &[], false, &Scope::all()).unwrap();
        assert!(
            n.callers
                .iter()
                .any(|c| c.symbol.path == "src/flask/helpers.py"
                    && c.symbol.name == "make_response"
                    && c.line == Some(27)),
            "{:?}",
            n.callers
        );
    }

    /// G3: the file `main.rs` and `fn main` share a name, and the module's
    /// `defines` edges were written onto `fn main` and printed as "call at
    /// line N" with N the definition's own line.
    #[test]
    fn a_module_edge_sits_on_the_module_and_is_not_a_call() {
        let db = ripgrep();
        let into =
            crate::store::edges_in(&db, "search_parallel", &["defines".to_string()]).unwrap();
        assert!(!into.is_empty());
        for e in &into {
            assert_eq!(e.symbol.kind, "module", "{e:?}");
            let site = call_site(e, &verbatim);
            assert!(!site.contains("call"), "{site}");
        }
        let main = crate::store::edges_out(&db, "main", &["defines".to_string()]).unwrap();
        assert!(!main.is_empty());
        assert!(main.iter().all(|e| e.from_line == Some(1)), "{main:?}");
    }

    /// G8: one edge, one confidence, whichever command shows it
    /// (test_basic.py `flask.make_response()`: impact said ambiguous,
    /// neighbors said inferred).
    #[test]
    fn neighbors_and_impact_give_one_edge_one_confidence() {
        let db = flask();
        let n = neighbours(&db, "make_response", &[], false, &Scope::all()).unwrap();
        let reach = impact(&db, "make_response", &[], 1, false, &Scope::all()).unwrap();
        let caller = n
            .callers
            .iter()
            .find(|c| c.symbol.name == "test_make_response")
            .expect("test_make_response calls make_response");
        let row = reach
            .reached
            .iter()
            .find(|r| r.name == "test_make_response")
            .expect("impact reaches test_make_response");
        assert_eq!(caller.confidence, row.confidence);
    }

    /// G4: `BaseResponse::Flask`, `t::Config`, `ft::Blueprint`: a top-level
    /// definition is owned by its module, never by an import alias.
    #[test]
    fn a_python_definition_is_not_owned_by_an_import_alias() {
        let e = extract(
            Path::new("src/flask/app.py"),
            &fixture("python/flask_app.py"),
        )
        .unwrap()
        .unwrap();
        let flask = e.symbols.iter().find(|s| s.name == "Flask").unwrap();
        assert_eq!(flask.qualified, "app::Flask");
        let method = e
            .symbols
            .iter()
            .find(|s| s.name == "make_response")
            .unwrap();
        assert_eq!(method.qualified, "Flask::make_response");
        for s in &e.symbols {
            for alias in ["BaseResponse::", "t::", "ft::", "cabc::", "_url_quote::"] {
                assert!(!s.qualified.starts_with(alias), "{s:?}");
            }
        }
        assert!(
            !e.edges
                .iter()
                .any(|x| x.kind == "contains"
                    && ["BaseResponse", "t", "ft"].contains(&x.from.as_str())),
            "{:?}",
            e.edges
        );
    }

    /// G5: an alias row is its import line, not `__init__.py:1-40` for a
    /// 39-line file.
    #[test]
    fn an_alias_row_is_its_import_line() {
        let e = extract(
            Path::new("src/flask/__init__.py"),
            &fixture("python/flask_init.py"),
        )
        .unwrap()
        .unwrap();
        for (name, line) in [("Flask", 2), ("make_response", 17), ("url_for", 22)] {
            let s = e
                .symbols
                .iter()
                .find(|s| s.name == name && s.kind == "alias")
                .unwrap_or_else(|| panic!("no alias {name}"));
            assert_eq!((s.start_line, s.end_line), (line, line), "{s:?}");
        }
    }

    /// G6: setext headings (an underline of `===` or `---`) are headings: the
    /// ripgrep CHANGELOG's gave `1-1871` past the end of the file, and a body
    /// line belonged to no section or the wrong one.
    #[test]
    fn setext_headings_define_their_own_sections() {
        let text = fixture("markdown/ripgrep_changelog.md");
        let lines = text.lines().count() as u32;
        let e = extract(Path::new("CHANGELOG.md"), &text).unwrap().unwrap();
        let headings: Vec<&Symbol> = e.symbols.iter().filter(|s| s.kind == "heading").collect();
        let titles: Vec<&str> = headings.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            titles,
            [
                "TBD",
                "15.2.0 (2026-07-15)",
                "15.1.0 (2025-10-22)",
                "15.0.0 (2025-10-15)",
                "12.1.1 (2020-05-29)"
            ]
        );
        let spans: Vec<(u32, u32)> = headings
            .iter()
            .map(|s| (s.start_line, s.end_line))
            .collect();
        assert_eq!(spans, [(1, 5), (6, 35), (36, 53), (54, 65), (66, lines)]);
        for h in &headings {
            assert_eq!(h.qualified, format!("CHANGELOG::{}", h.name), "{h:?}");
        }
    }

    /// The census `stats` prints counts a call through a receiver the way the
    /// resolver settles it: ripgrep's `searcher.clone()` was "resolved"
    /// because the store held one `clone`, a method of another type.
    #[test]
    fn the_census_counts_an_untyped_method_call_as_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::store::open(&dir.path().join("store.db")).unwrap();
        crate::store::read_only(&db, false).unwrap();
        for (path, text) in [
            ("src/a.rs", "fn caller(t: Thing) { t.go(); }\n"),
            (
                "src/b.rs",
                "struct Other;\nimpl Other {\n    fn go(&self) {}\n}\n",
            ),
        ] {
            let file = crate::store::insert_file(&db, path, path, 1, 0).unwrap();
            let e = extract(Path::new(path), text).unwrap().unwrap();
            crate::store::write_extraction(&db, &e, file, &[]).unwrap();
        }
        let rust = crate::store::coverage_by_language(&db)
            .unwrap()
            .into_iter()
            .find(|r| r.language == "rust")
            .unwrap();
        assert_eq!((rust.resolved, rust.ambiguous), (0, 1), "{rust:?}");
        let out = crate::store::edges_out(&db, "caller", &[]).unwrap();
        assert!(out.iter().all(|e| e.confidence == AMBIGUOUS), "{out:?}");
    }

    /// G9, G10: `symbol run -k 3` said "3 definitions" of 52, with nothing
    /// saying the list was cut, and the default list was YAML `run:` keys
    /// ahead of every Rust `run`.
    #[test]
    fn symbol_counts_every_definition_and_lists_code_first() {
        let db = store(&[
            (".github/workflows/ci.yml", "yaml/ripgrep_ci.yml"),
            ("crates/core/main.rs", "rust/ripgrep_main.rs"),
            ("crates/ignore/src/walk.rs", "rust/ripgrep_walk.rs"),
        ]);
        let total = 7 + 2;
        let found = evidence(&db, "run", &dependency_kinds(), 3, false, &Scope::all()).unwrap();
        let kinds: Vec<&str> = found.definitions.iter().map(|d| d.kind.as_str()).collect();
        assert_eq!(
            &kinds[..2],
            ["function", "method"],
            "{:?}",
            found.definitions
        );
        let text = found.render("", "", &verbatim);
        assert!(
            text.contains(&format!("{total} definitions of this name")),
            "{text}"
        );
        assert!(text.contains("3 shown"), "{text}");
        let all = evidence(&db, "run", &dependency_kinds(), 20, false, &Scope::all()).unwrap();
        assert_eq!(all.definitions.len(), total, "{:?}", all.definitions);
        assert!(!all.render("", "", &verbatim).contains("shown"));
    }

    /// G11: several names at once kept a heading out that one name finds.
    #[test]
    fn several_names_answer_with_the_rows_one_name_does() {
        let db = store(&[
            ("CHANGELOG.md", "markdown/ripgrep_changelog.md"),
            ("crates/core/main.rs", "rust/ripgrep_main.rs"),
        ]);
        let one = evidence(&db, "15.2.0 (2026-07-15)", &[], 20, false, &Scope::all()).unwrap();
        assert_eq!(one.definitions.len(), 1);
        let names = [
            "15.2.0 (2026-07-15)".to_string(),
            "search_parallel".to_string(),
        ];
        let rows = signatures(&db, &names, &Scope::all()).unwrap();
        let asked: Vec<&str> = rows.iter().map(|r| r.asked.as_str()).collect();
        assert_eq!(
            asked,
            ["15.2.0 (2026-07-15)", "search_parallel"],
            "{rows:?}"
        );
    }

    /// G7: `symbol --json` printed two "kind" keys per caller, so a parser
    /// kept the edge's and lost the symbol's.
    #[test]
    fn an_edge_end_serialises_one_kind_key() {
        let db = ripgrep();
        let found = evidence(
            &db,
            "search_parallel",
            &dependency_kinds(),
            20,
            false,
            &Scope::all(),
        )
        .unwrap();
        assert!(!found.callers.is_empty());
        let json = serde_json::to_string(&found.callers[0]).unwrap();
        assert_eq!(json.matches("\"kind\":").count(), 1, "{json}");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["symbol_kind"], "function", "{value}");
        assert_eq!(value["kind"], "calls", "{value}");
    }

    /// Item 30: "Called and not found" was cobra's builtins and standard
    /// library — `Errorf` 198 times, `len`, `append`, `os`.
    #[test]
    fn called_and_not_found_leaves_out_builtins_and_the_standard_library() {
        let db = store(&[("command.go", "go/cobra_command.go")]);
        let (targets, distinct) = crate::store::unresolved_targets(&db, 20).unwrap();
        let names: Vec<&str> = targets.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"validateRequiredFlags"), "{names:?}");
        for builtin in [
            "Errorf",
            "len",
            "append",
            "os",
            "fmt",
            "strings",
            "TrimSpace",
            "Fprintln",
        ] {
            assert!(!names.contains(&builtin), "{builtin} in {names:?}");
        }
        assert_eq!(distinct, names.len() as i64);
        assert!(crate::store::builtin_targets(&db).unwrap() >= 7);
    }
}
