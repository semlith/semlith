//! Several stores, one query.
//!
//! A developer's work is not one repository, so a question about "the code" is
//! rarely a question about one store. A [`Fleet`] opens the stores it is given,
//! asks each of them the same question, and merges the answers into one ranked
//! list where every hit says which store it came from.
//!
//! Two properties of the store make this cheap. Chunk ids never leave a store
//! — a hit is resolved to text inside the store that produced it — so no id
//! from one store can be looked up in another. And a search's score is a
//! reciprocal-rank sum, not a distance, so scores produced by two different
//! stores with two different models are still the same unit.

use crate::embed::Model;
use crate::filter::Filter;
use crate::{Hit, Semlith, canonical, chunk, model_cache_dir};
use anyhow::{Context, Result, bail};
use fastembed::TextEmbedding;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct Fleet {
    members: Vec<Member>,
    /// Stores that were named but could not be opened at all — the database
    /// header, the meta row or a page under them is unreadable.
    broken: Vec<Broken>,
    /// Stores that opened and then failed a read, remembered by the readability
    /// probe every selection runs. Interior mutability because every read path
    /// through a fleet takes `&self`, and a store going unreadable mid-session
    /// is an observation about the disk rather than a change to the fleet.
    unreadable: RefCell<Vec<Unreadable>>,
    /// One loaded model per distinct model, shared by every store using it.
    /// Three stores built with the same model cost one copy of the weights.
    /// The tokenizer of the first model this fleet loaded, for counting the
    /// ledger's tokens rather than estimating them.
    ///
    /// It lives here rather than on each store because this is where models are
    /// loaded. `Semlith` has an embedder of its own for the library path, and a
    /// fleet never uses it — which is exactly how the ledger came to label every
    /// row `chars4` while the code claimed to count with the real thing: the
    /// tokenizer was hung off the object that was not doing the work.
    tokenizer: Option<tokenizers::Tokenizer>,
    /// Query embeds performed, which is what the "one embed per model, not per
    /// store" claim is measured against.
    embeds: usize,
    /// Print model-download progress to stderr. Off for the MCP server, where
    /// stdout and stderr are a protocol channel.
    pub quiet: bool,
}

struct Member {
    label: String,
    store: Semlith,
    /// The directories this store indexes, from the registry. Empty for a
    /// store named by `--store` that the registry does not know, which then
    /// answers with absolute paths as it always did.
    roots: Vec<PathBuf>,
}

/// A store directory that holds a `store.db` the process could not open.
struct Broken {
    label: String,
    dir: PathBuf,
    error: String,
}

/// A store an answer could not reach, and what to do about it.
///
/// Serialised beside the answer by every aggregating route, so a page can draw
/// what the healthy stores said and report this rather than showing nothing.
/// The whole error chain is carried, because `disk I/O error` on its own names
/// neither the store nor the file it was read from.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Unreadable {
    pub store: String,
    pub path: String,
    pub error: String,
    pub remedy: String,
}

impl Unreadable {
    fn new(store: &str, dir: &Path, error: String) -> Self {
        Self {
            store: store.to_string(),
            path: crate::plain(&dir.display().to_string()),
            error,
            remedy: format!("semlith drop {store} — or index its root again to rebuild the store"),
        }
    }

    fn line(&self) -> String {
        format!(
            "{} at {}: {} — {}",
            self.store, self.path, self.error, self.remedy
        )
    }
}

/// What a request scoped to nothing but unreadable stores is told.
///
/// There is no partial answer to give, so this is an error rather than an empty
/// one — and it names the store, the path and the remedy rather than repeating
/// SQLite's four words.
fn nothing_readable(failed: &[Unreadable]) -> String {
    let each: Vec<String> = failed.iter().map(Unreadable::line).collect();
    if each.len() == 1 {
        format!("this store cannot be read: {}", each[0])
    } else {
        format!("none of these stores can be read: {}", each.join("; "))
    }
}

/// What every surface says when the filters admit no indexed file at all.
///
/// One sentence in one place: the terminal, `semlith_search` and
/// `semlith_brief` all reach an emptied selection, and three wordings for one
/// condition is three things for a reader to learn.
pub const FILTER_SELECTED_NOTHING: &str =
    "No indexed file matches that path/ext/lang filter. Try again without it.";

impl Fleet {
    /// A fleet with nothing in it.
    ///
    /// For the MCP methods that are about the server rather than about a
    /// corpus — `initialize`, `tools/list`, `ping`. An agent that connects to
    /// a daemon before anything has been indexed should be told which tools
    /// exist, not that the daemon is broken.
    ///
    /// Only the daemon used this until 0.21.0. `semlith mcp` on stdio took the
    /// other path and ended the process with "no semlith store covers …"
    /// before writing a byte of protocol, so a fresh install wired into a
    /// client was a server that exited on startup and a client that reported
    /// nothing at all — the same silent absence, reached from the side nobody
    /// had checked.
    pub fn empty() -> Self {
        Self {
            members: Vec::new(),
            broken: Vec::new(),
            unreadable: RefCell::new(Vec::new()),
            tokenizer: None,
            embeds: 0,
            quiet: true,
        }
    }

    /// Give each member the name its owner already knows it by.
    ///
    /// A store has one name, and until 0.16.0 it had two: the daemon's, from
    /// the registry, which `/api/stores` reports and the portal draws its chips
    /// from; and the fleet's, derived from the store directory's own basename.
    /// They agree for a store in the store home, where the directory *is* the
    /// name, and disagree for every store opened by path — so the portal drew a
    /// chip the search route then refused, naming stores the user had never
    /// heard of. The fleet is the one that gives way: the registry's name is
    /// the one a person typed.
    ///
    /// A directory the caller says nothing about keeps the derived label, so a
    /// fleet opened from the command line is unchanged.
    pub fn name_from(&mut self, named: &[(PathBuf, String)]) {
        for member in &mut self.members {
            if let Some(name) = name_for(member.store.dir(), named) {
                member.label = name;
            }
        }
        // A store that could not be opened is reported by the name its owner
        // knows it by for the same reason: a remedy naming a directory basename
        // the registry never used is a remedy nobody can run.
        for store in &mut self.broken {
            if let Some(name) = name_for(&store.dir, named) {
                store.label = name;
            }
        }
    }

    /// Open every store in `dirs`, which must all already be stores.
    ///
    /// The same store named twice — the flag repeated, a relative path beside
    /// an absolute one, a symlink — is opened once. Merging a store with itself
    /// would give every one of its hits a second copy at the same score and
    /// hand it the whole result list.
    pub fn open(dirs: &[PathBuf]) -> Result<Self> {
        if dirs.is_empty() {
            bail!("no store given");
        }

        let mut keys: Vec<PathBuf> = Vec::new();
        let mut members = Vec::new();
        let mut broken: Vec<Broken> = Vec::new();
        for dir in dirs {
            // A store that does not exist is an error before anything is
            // opened, and `open_existing` is what refuses to create one.
            let store = match Semlith::open_existing(dir) {
                Ok(store) => store,
                // A directory that holds a `store.db` and still cannot be
                // opened is a damaged store, not a mistyped path. It is set
                // aside with its reason rather than taken as a reason to open
                // none of the others: one unplugged drive used to blank every
                // page that reads across stores (#129).
                Err(e) if dir.join("store.db").exists() => {
                    let key = canonical(dir);
                    if !broken.iter().any(|b| b.dir == key) {
                        broken.push(Broken {
                            label: label(&key),
                            dir: key,
                            error: format!("{e:#}"),
                        });
                    }
                    continue;
                }
                Err(e) => return Err(e),
            };
            let key = canonical(dir);
            if keys.contains(&key) {
                continue;
            }
            keys.push(key);
            members.push(Member {
                label: String::new(),
                store,
                roots: Vec::new(),
            });
        }

        // A registry that cannot be read costs relative paths, not the open.
        let registry = crate::home::Registry::load().unwrap_or_default();
        for (member, key) in members.iter_mut().zip(&keys) {
            member.label = label(key);
            member.roots = registry
                .stores
                .iter()
                .find(|(name, _)| {
                    crate::home::Registry::dir_of(name).is_ok_and(|d| canonical(&d) == *key)
                })
                .map(|(_, entry)| entry.roots.clone())
                .unwrap_or_default();
        }
        disambiguate(&mut members, &keys);

        // Every store named is damaged. There is no partial answer to give, so
        // this fails the way it always did — with the store, the path and the
        // remedy instead of SQLite's four words.
        if members.is_empty() && !broken.is_empty() {
            let failed: Vec<Unreadable> = broken
                .iter()
                .map(|b| Unreadable::new(&b.label, &b.dir, b.error.clone()))
                .collect();
            bail!("{}", nothing_readable(&failed));
        }

        Ok(Self {
            members,
            broken,
            unreadable: RefCell::new(Vec::new()),
            tokenizer: None,
            embeds: 0,
            quiet: false,
        })
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// `(label, store)` for each chosen store that can be read.
    pub fn chosen_each(&self, only: Option<&[String]>) -> Result<Vec<(&str, &Semlith)>> {
        Ok(self
            .chosen(only)?
            .into_iter()
            .map(|i| (self.members[i].label.as_str(), &self.members[i].store))
            .collect())
    }

    /// Every root the open stores index, from the registry.
    pub fn roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self
            .members
            .iter()
            .flat_map(|m| m.roots.iter().cloned())
            .collect();
        roots.sort();
        roots.dedup();
        roots
    }

    /// Writes paths relative to the root that holds them, for an answer an
    /// agent reads. See [`crate::Shortener`].
    pub fn shortener(&self) -> crate::Shortener {
        crate::Shortener::new(self.roots())
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// `(label, store)` for each store that can be read, in the order they were
    /// named.
    ///
    /// A store that fails the probe is left out here rather than allowed to
    /// abort the caller's whole loop, and is reported by [`Fleet::failed`].
    /// Together with [`Fleet::chosen`] this is the only way into the members,
    /// which is what makes one guard cover every aggregating caller.
    pub fn each(&self) -> impl Iterator<Item = (&str, &Semlith)> {
        (0..self.members.len())
            .filter(|i| self.readable(*i))
            .map(|i| (self.members[i].label.as_str(), &self.members[i].store))
    }

    /// The stores `only` names, unreadable ones left out.
    ///
    /// What a caller that wants [`Fleet::each`] narrowed to some stores should
    /// ask for: it refuses an unknown name the way every other read does, and
    /// it refuses outright when nothing it was asked for can be read.
    pub fn selected(&self, only: Option<&[String]>) -> Result<Vec<(&str, &Semlith)>> {
        Ok(self
            .chosen(only)?
            .into_iter()
            .map(|i| (self.members[i].label.as_str(), &self.members[i].store))
            .collect())
    }

    /// Every store this fleet cannot read, with the path and the remedy.
    ///
    /// Empty when all of them answered, which is what lets a route put it in
    /// the response only when there is something to say.
    pub fn failed(&self) -> Vec<Unreadable> {
        let mut out: Vec<Unreadable> = self
            .broken
            .iter()
            .map(|b| Unreadable::new(&b.label, &b.dir, b.error.clone()))
            .collect();
        for found in self.unreadable.borrow().iter() {
            if !out.iter().any(|o| o.store == found.store) {
                out.push(found.clone());
            }
        }
        out
    }

    /// Whether member `i` still answers, remembering the answer for
    /// [`Fleet::failed`].
    ///
    /// `stats` is the probe because it counts rows in both of the big tables,
    /// so it notices damage a read of the header or the meta row would sail
    /// past — which is the failure in #129, where the store opened and only
    /// then said `disk I/O error`.
    ///
    // One indexed row out of each large table per store per selection, which
    // is what `store::readable` is. It was `stats()` — three full scans —
    // until that was measured at 2.65 ms against 0.24 ms on a 10 390-chunk
    // store, growing linearly, on a path every search takes.
    fn readable(&self, i: usize) -> bool {
        let member = &self.members[i];
        match member.store.readable() {
            Ok(_) => {
                // A store that has come back — the drive was plugged in again,
                // the index run finished — stops being reported.
                self.unreadable
                    .borrow_mut()
                    .retain(|u| u.store != member.label);
                true
            }
            Err(e) => {
                let found = Unreadable::new(&member.label, member.store.dir(), format!("{e:#}"));
                let mut held = self.unreadable.borrow_mut();
                match held.iter_mut().find(|u| u.store == member.label) {
                    Some(slot) => *slot = found,
                    None => held.push(found),
                }
                false
            }
        }
    }

    pub fn labels(&self) -> Vec<&str> {
        self.members.iter().map(|m| m.label.as_str()).collect()
    }

    /// Chunks held across every store, for the "the corpus has n chunks"
    /// message a query with no results prints.
    pub fn chunks(&self) -> usize {
        self.members.iter().map(|m| m.store.len()).sum()
    }

    /// Files indexed across every store.
    pub fn files(&self) -> Result<i64> {
        let mut total = 0;
        for (_, store) in self.each() {
            total += store.stats()?.0;
        }
        Ok(total)
    }

    /// Query embeds performed since this fleet was opened. One per distinct
    /// model per search, not one per store.
    pub fn query_embeds(&self) -> usize {
        self.embeds
    }

    /// Load every distinct model, so the first query does not pay a cold start.
    ///
    /// The vector indexes are deliberately left alone. They are read when a
    /// query needs them, which for a server an agent leaves open all day may be
    /// never; and a store larger than its memory budget would only load shards
    /// here in order to put them straight back down. The model is the part that
    /// is always needed and always expensive, so it is the part warmed.
    pub fn warm(&mut self) -> Result<()> {
        for i in 0..self.members.len() {
            let model = self.members[i].store.model().clone();
            self.embedder(&model)?;
        }
        Ok(())
    }

    /// Fit every member's indexes to the `MiB per store` in force, shedding
    /// resident shards above it now rather than at the next search.
    pub fn follow_budget(&mut self) {
        for member in &mut self.members {
            member.store.follow_budget();
        }
    }

    /// How many indexed files `filter` selects across every store.
    pub fn matching_files(&self, filter: &Filter) -> Result<i64> {
        let mut total = 0;
        for (_, store) in self.each() {
            total += store.matching_files(filter)?;
        }
        Ok(total)
    }

    /// Why a query came back with nothing.
    ///
    /// A filter that admits no file at all is a different answer from a corpus
    /// that does not discuss the question, and since `--path '!src/**'` exists
    /// the first is now easy to write by accident. Saying which one happened is
    /// the whole difference between "ask something else" and "drop a filter".
    pub fn no_match_reason(&self, filter: &Filter) -> String {
        if !filter.is_empty() && self.matching_files(filter).unwrap_or(1) == 0 {
            return FILTER_SELECTED_NOTHING.to_string();
        }
        format!("no matches (store has {} chunks)", self.chunks())
    }

    pub fn search(&mut self, query: &str, k: usize) -> Result<Vec<Hit>> {
        self.search_filtered(query, k, &Filter::default())
    }

    /// Top-`k` chunks for `query` across every store, best first.
    pub fn search_filtered(&mut self, query: &str, k: usize, filter: &Filter) -> Result<Vec<Hit>> {
        self.search_in(None, query, k, filter)
    }

    /// [`Fleet::search_filtered`] restricted to the stores named in `only`.
    ///
    /// An unknown name is an error rather than an empty result: an agent that
    /// guessed a store name has to be told, or it reads "no matches" as "the
    /// corpus does not discuss this".
    pub fn search_in(
        &mut self,
        only: Option<&[String]>,
        query: &str,
        k: usize,
        filter: &Filter,
    ) -> Result<Vec<Hit>> {
        self.search_preferring(only, query, k, filter, crate::Prefer::default())
    }

    /// [`Fleet::search_in`] with the caller's preference applied.
    ///
    /// The preference reaches each store rather than being applied to the
    /// merged list, because a store that holds only prose should still lift
    /// its best prose under `prefer: docs`, and the merge is by score.
    pub fn search_preferring(
        &mut self,
        only: Option<&[String]>,
        query: &str,
        k: usize,
        filter: &Filter,
        prefer: crate::Prefer,
    ) -> Result<Vec<Hit>> {
        let chosen = self.chosen(only)?;
        // Labels are worth their tokens only when there is something to tell
        // apart. One store means the output is what it was before stores could
        // be combined.
        let label_hits = self.members.len() > 1;

        // One vector per distinct model, not per store.
        let mut vectors: Vec<(Model, Vec<f32>)> = Vec::new();
        let mut queues: Vec<Vec<(Hit, f32)>> = Vec::new();

        for i in chosen {
            let model = self.members[i].store.model().clone();
            let vector = match vectors.iter().find(|(m, _)| *m == model) {
                Some((_, v)) => v.clone(),
                None => {
                    let v = self.embed_query(&model, query)?;
                    vectors.push((model, v.clone()));
                    v
                }
            };

            let hits = self.members[i]
                .store
                .search_preferring(query, &vector, k, filter, prefer)?;
            let label = self.members[i].label.clone();
            queues.push(
                hits.into_iter()
                    .map(|(mut h, similarity)| {
                        if label_hits {
                            h.store = Some(label.clone());
                        }
                        (h, similarity)
                    })
                    .collect(),
            );
        }

        Ok(merge(queues, k))
    }

    /// Indexed paths across the stores `only` selects, capped at `limit`.
    ///
    /// Returns the paths and how many the cap left out, because a truncated
    /// list that does not say it is truncated is how an agent concludes a file
    /// is not indexed.
    pub fn paths_in(
        &self,
        only: Option<&[String]>,
        filter: &Filter,
        limit: usize,
    ) -> Result<(Vec<String>, usize)> {
        let chosen = self.chosen(only)?;
        let label = self.members.len() > 1;
        let mut out = Vec::new();
        let mut total = 0;
        for i in chosen {
            for path in self.members[i].store.matching_paths(filter)? {
                total += 1;
                if out.len() < limit {
                    out.push(if label {
                        format!("[{}] {path}", self.members[i].label)
                    } else {
                        path
                    });
                }
            }
        }
        let left_out = total - out.len();
        Ok((out, left_out))
    }

    /// Every definition of `name` across the stores `only` selects.
    pub fn symbols_in(
        &self,
        only: Option<&[String]>,
        name: &str,
        limit: usize,
    ) -> Result<Vec<crate::store::SymbolRow>> {
        self.graph_in(only, |store| {
            crate::store::symbols_named(store.db(), name, limit)
        })
    }

    /// One structural pattern, run over every chosen store.
    pub fn pattern_in(
        &self,
        only: Option<&[String]>,
        language: &str,
        source: &str,
        filter: &crate::filter::Filter,
        offset: usize,
    ) -> Result<crate::pattern::Matches> {
        let mut out = self.matches_in(only, offset, |db, left| {
            crate::pattern::run(db, language, source, filter, left)
        })?;
        out.language = language.trim().to_ascii_lowercase();
        Ok(out)
    }

    /// Every indexed line matching `source`, over every chosen store.
    pub fn grep_in(
        &self,
        only: Option<&[String]>,
        source: &str,
        filter: &crate::filter::Filter,
        offset: usize,
    ) -> Result<crate::pattern::Matches> {
        self.matches_in(only, offset, |db, left| {
            crate::pattern::grep(db, source, filter, left)
        })
    }

    /// The definitions in one indexed file, from the first chosen store that
    /// holds it: `(start, end, name, kind)`, in line order.
    pub fn outline_in(
        &self,
        only: Option<&[String]>,
        path: &str,
    ) -> Result<Vec<(u32, u32, String, String)>> {
        for i in self.chosen(only)? {
            let db = self.members[i].store.db();
            if let Some(rows) =
                crate::store::symbols_in_files(db, &[path.to_string()])?.remove(path)
            {
                return Ok(rows);
            }
        }
        Ok(Vec::new())
    }

    /// A per-store listing run over every chosen store.
    ///
    /// The matches are labelled and concatenated in store order; the file and
    /// match counts are summed, and `truncated` is true when any store hit its
    /// own budget, because a partial answer from one store is a partial
    /// answer.
    fn matches_in(
        &self,
        only: Option<&[String]>,
        offset: usize,
        run: impl Fn(&rusqlite::Connection, usize) -> Result<crate::pattern::Matches>,
    ) -> Result<crate::pattern::Matches> {
        let chosen = self.chosen(only)?;
        let label_rows = self.members.len() > 1;
        let mut out = crate::pattern::Matches {
            language: String::new(),
            matches: Vec::new(),
            files: 0,
            truncated: false,
            skipped: 0,
        };
        // One offset across the fleet, not one per store. The stores are asked
        // in a fixed order, so what the first one skipped comes off what the
        // second is asked to skip.
        let mut left = offset;
        for i in chosen {
            let part = run(self.members[i].store.db(), left)?;
            left = left.saturating_sub(part.skipped);
            out.skipped += part.skipped;
            out.files += part.files;
            out.truncated |= part.truncated;
            for mut found in part.matches {
                if label_rows {
                    found.store = Some(self.members[i].label.clone());
                }
                out.matches.push(found);
            }
        }
        Ok(out)
    }

    /// One span, from whichever chosen store holds it.
    ///
    /// The first store with an answer wins, and its label rides along when
    /// there is more than one store to tell apart. A name with several
    /// definitions returns the list from the first store that has any, because
    /// a list that mixed two stores' definitions would need a store column the
    /// caller did not ask for.
    pub fn read_in(
        &self,
        only: Option<&[String]>,
        target: &crate::Target,
        filter: &crate::filter::Filter,
    ) -> Result<Option<crate::Read>> {
        let chosen = self.chosen(only)?;
        let label_rows = self.members.len() > 1;
        for i in chosen {
            let roots = &self.members[i].roots;
            let Some(found) = self.members[i].store.read_within(target, filter, roots)? else {
                continue;
            };
            return Ok(Some(match found {
                crate::Read::One(mut span) => {
                    if label_rows {
                        span.store = Some(self.members[i].label.clone());
                    }
                    crate::Read::One(span)
                }
                crate::Read::Choose(mut rows) => {
                    if label_rows {
                        for row in &mut rows {
                            row.store = Some(self.members[i].label.clone());
                        }
                    }
                    crate::Read::Choose(rows)
                }
            }));
        }
        Ok(None)
    }

    /// Every definition of several names, one row each, across the chosen
    /// stores. See [`crate::graph::signatures`].
    pub fn signatures_in(
        &self,
        only: Option<&[String]>,
        names: &[String],
    ) -> Result<Vec<crate::graph::Signature>> {
        let label_rows = self.members.len() > 1;
        let mut out = Vec::new();
        for i in self.chosen(only)? {
            let mut rows = crate::graph::signatures(self.members[i].store.db(), names)?;
            if label_rows {
                for row in &mut rows {
                    row.symbol.store = Some(self.members[i].label.clone());
                }
            }
            out.extend(rows);
        }
        // Grouped by the name asked, in the order asked, so the table reads
        // as answers to the question rather than as one store after another.
        out.sort_by_key(|r| {
            names
                .iter()
                .position(|n| *n == r.asked)
                .unwrap_or(usize::MAX)
        });
        Ok(out)
    }

    /// Everything the chosen stores know about `name`, in one reply.
    ///
    /// Merged the same way neighbours are: each store answers about its own
    /// rows and the lists are joined, because one name defined in two stores
    /// is still one name.
    pub fn evidence_in(
        &self,
        only: Option<&[String]>,
        name: &str,
        kinds: &[String],
        limit: usize,
        all: bool,
    ) -> Result<crate::graph::Evidence> {
        let mut merged: Option<crate::graph::Evidence> = None;
        for part in self.graph_each(only, |s| {
            crate::graph::evidence(s.db(), name, kinds, limit, all)
        })? {
            match &mut merged {
                None => merged = Some(part),
                Some(into) => {
                    into.definitions.extend(part.definitions);
                    into.callers.extend(part.callers);
                    into.callees.extend(part.callees);
                    into.ego.extend(part.ego);
                }
            }
        }
        Ok(merged.unwrap_or_else(|| crate::graph::Evidence {
            name: name.to_string(),
            definitions: Vec::new(),
            callers: Vec::new(),
            callees: Vec::new(),
            ego: Vec::new(),
        }))
    }

    /// Callers and callees of `name`, merged across the chosen stores.
    pub fn neighbours_in(
        &self,
        only: Option<&[String]>,
        name: &str,
        kinds: &[String],
        all: bool,
    ) -> Result<crate::graph::Neighbours> {
        // Each store resolves its own callers against its own definitions —
        // which `Type::method` a call means is a fact inside one store — and
        // the rows are joined after.
        let label_rows = self.members.len() > 1;
        let (mut callers, mut callees, mut unresolved) = (Vec::new(), Vec::new(), Vec::new());
        for i in self.chosen(only)? {
            let mut part = crate::graph::neighbours(self.members[i].store.db(), name, kinds, true)?;
            if label_rows {
                let label = &self.members[i].label;
                for row in part.callers.iter_mut().chain(part.callees.iter_mut()) {
                    row.label(label);
                }
            }
            callers.append(&mut part.callers);
            callees.append(&mut part.callees);
            unresolved.append(&mut part.unresolved);
        }
        // Collapsed after the stores are joined, not inside each of them: one
        // name with two definitions in two stores is still one name.
        Ok(crate::graph::Neighbours {
            callers,
            callees: if all {
                callees
            } else {
                crate::graph::collapse(callees)
            },
            hidden: if all { 0 } else { unresolved.len() },
            unresolved: if all { unresolved } else { Vec::new() },
        })
    }

    /// What a name used to be, across the chosen stores, newest first.
    ///
    /// Empty from a store that has not re-indexed since it began keeping
    /// history, which is indistinguishable from a name that never changed --
    /// and is why `semlith stats` says whether a store keeps any at all.
    pub fn past_in(
        &self,
        only: Option<&[String]>,
        name: &str,
        limit: usize,
    ) -> Result<Vec<crate::store::PastSymbol>> {
        let mut rows = self.graph_in(only, |s| {
            crate::store::symbols_past_named(s.db(), name, limit)
        })?;
        rows.sort_by_key(|r| std::cmp::Reverse(r.retired_at));
        rows.truncate(limit);
        Ok(rows)
    }

    /// What should count this fleet's tokens.
    ///
    /// The first store with a loaded model decides, and its tokenizer counts
    /// for the whole answer. Two stores could in principle be on two models;
    /// mixing two tokenizers inside one ratio would be worse than using one of
    /// them and labelling the row, which is what this does.
    pub fn counter(&self) -> crate::ledger::Counter<'_> {
        self.tokenizer
            .as_ref()
            // A store's own tokenizer, for a caller that drove `Semlith`
            // directly and handed the result here.
            .or_else(|| self.members.iter().find_map(|m| m.store.tokenizer()))
            .map(crate::ledger::Counter::Model)
            .unwrap_or(crate::ledger::Counter::Chars4)
    }

    /// The shortest chain from `from` to `to`, in the first store that has one.
    ///
    /// A path that crossed two stores would be a path through two unrelated
    /// corpora, which is not a fact about anybody's code, so each store is
    /// asked separately and the shortest answer wins.
    pub fn path_in(
        &self,
        only: Option<&[String]>,
        from: &str,
        to: &str,
        depth: u32,
        all_edges: bool,
    ) -> Result<Option<crate::graph::Chain>> {
        let mut best: Option<crate::graph::Chain> = None;
        for i in self.chosen(only)? {
            if let Some(chain) =
                crate::graph::shortest_path(self.members[i].store.db(), from, to, depth, all_edges)?
                && best
                    .as_ref()
                    .is_none_or(|b| chain.steps.len() < b.steps.len())
            {
                best = Some(chain);
            }
        }
        Ok(best)
    }

    /// Everything that reaches `name`, merged across the chosen stores.
    ///
    /// Unlike a path, reverse reachability does join across stores cleanly:
    /// each store answers about its own corpus and the answers are separate
    /// facts about separate trees, so concatenating them says "these are the
    /// places that reach this name", which is the question asked. Hops are
    /// per store and are not compared between them.
    pub fn impact_in(
        &self,
        only: Option<&[String]>,
        name: &str,
        kinds: &[String],
        depth: u32,
        all_edges: bool,
    ) -> Result<crate::graph::Impact> {
        let parts = self.graph_each(only, |s| {
            crate::graph::impact(s.db(), name, kinds, depth, all_edges)
        })?;
        let mut merged = crate::graph::Impact {
            name: name.to_string(),
            depth,
            all_edges,
            definitions: Vec::new(),
            reached: Vec::new(),
            files: Vec::new(),
            hidden: 0,
            unqualified: false,
            extracted: 0,
            resolved: 0,
            inferred: 0,
            ambiguous: 0,
        };
        let mut owned_somewhere = false;
        let mut any_part = false;
        for part in parts {
            any_part = true;
            owned_somewhere |= !part.unqualified;
            merged.definitions.extend(part.definitions);
            merged.reached.extend(part.reached);
            merged.files.extend(part.files);
            merged.hidden += part.hidden;
            merged.extracted += part.extracted;
            merged.resolved += part.resolved;
            merged.inferred += part.inferred;
            merged.ambiguous += part.ambiguous;
        }
        // The qualifier fell back only if it matched in no store at all.
        merged.unqualified = any_part && !owned_somewhere;
        merged.reached.sort_by(|a, b| {
            a.hop
                .cmp(&b.hop)
                .then(
                    crate::graph::confidence_rank(&a.confidence)
                        .cmp(&crate::graph::confidence_rank(&b.confidence)),
                )
                .then(a.path.cmp(&b.path))
                .then(a.line.cmp(&b.line))
        });
        merged.files.sort_by(|a, b| {
            a.nearest
                .cmp(&b.nearest)
                .then(b.symbols.cmp(&a.symbols))
                .then(a.path.cmp(&b.path))
        });
        Ok(merged)
    }

    /// A chain turned into evidence: the sentence, the hops, and one line of
    /// source per hop.
    ///
    /// The path is found once, by [`Fleet::path_in`], and read back here —
    /// there is no second traversal, so the Trace panel and `semlith path`
    /// cannot describe one chain differently.
    pub fn trace_in(
        &self,
        only: Option<&[String]>,
        from: &str,
        to: &str,
        depth: u32,
        all_edges: bool,
        shorten: &dyn Fn(&str) -> String,
    ) -> Result<crate::graph::Trace> {
        let chain = self.path_in(only, from, to, depth, all_edges)?;
        let filter = crate::filter::Filter::default();
        let mut read_line = |path: &str, line: u32| -> Option<String> {
            let target = crate::Target::Span {
                path: path.to_string(),
                start: line,
                end: line,
            };
            match self.read_in(only, &target, &filter) {
                Ok(Some(crate::Read::One(span))) => Some(span.text.trim_end().to_string()),
                // Several files end with that suffix, or the store cannot
                // answer for it. A renderer says nothing rather than quoting
                // a line it is not sure about.
                _ => None,
            }
        };
        Ok(crate::graph::trace(
            from,
            to,
            depth,
            all_edges,
            chain,
            &mut read_line,
            shorten,
        ))
    }

    /// The communities of the chosen stores, with their hubs located.
    ///
    /// One store at a time, because a community spanning two unrelated
    /// corpora is not a fact about anybody's code — the same reason
    /// [`Fleet::path_in`] asks each store separately.
    pub fn communities_in(
        &self,
        only: Option<&[String]>,
        shown: usize,
    ) -> Result<(Vec<crate::graph::Community>, usize, i64)> {
        let mut all: Vec<crate::graph::Community> = Vec::new();
        let mut edges_read = 0usize;
        let mut edges_total = 0i64;
        for i in self.chosen(only)? {
            let db = self.members[i].store.db();
            let (edges, total) = crate::store::community_edges(db, crate::graph::COMMUNITY_EDGES)?;
            edges_read += edges.len();
            edges_total += total;
            let pairs: Vec<(String, String)> = edges.into_iter().map(|e| (e.from, e.to)).collect();
            let mut found = crate::graph::communities(&pairs);
            // Locate only the hubs that will be drawn: a definition lookup
            // per member of every community would read the whole symbol
            // table to fill in rows nobody sees.
            let wanted: Vec<String> = found
                .iter()
                .take(shown)
                .flat_map(|c| c.hubs.iter().map(|h| h.name.clone()))
                .collect();
            let located = crate::store::where_defined(db, &wanted)?;
            for community in found.iter_mut().take(shown) {
                for hub in &mut community.hubs {
                    if let Some((_, path, line)) = located.iter().find(|(n, _, _)| *n == hub.name) {
                        hub.path = path.clone();
                        hub.line = *line;
                    }
                }
            }
            all.extend(found);
        }
        all.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.label.cmp(&b.label)));
        let total = all.len();
        all.truncate(shown);
        let _ = edges_read;
        Ok((all, total, edges_total))
    }

    /// Run a read over the chosen stores and concatenate what comes back,
    /// labelling each row with its store when more than one is open.
    /// One answer per chosen store, unlabelled.
    ///
    /// The sibling of [`Fleet::graph_in`] for a reader that returns one whole
    /// answer per store rather than a list of rows to concatenate.
    fn graph_each<T>(
        &self,
        only: Option<&[String]>,
        read: impl Fn(&Semlith) -> Result<T>,
    ) -> Result<Vec<T>> {
        let chosen = self.chosen(only)?;
        let mut out = Vec::new();
        for i in chosen {
            out.push(read(&self.members[i].store)?);
        }
        Ok(out)
    }

    fn graph_in<T: Labelled>(
        &self,
        only: Option<&[String]>,
        read: impl Fn(&Semlith) -> Result<Vec<T>>,
    ) -> Result<Vec<T>> {
        let chosen = self.chosen(only)?;
        let label_rows = self.members.len() > 1;
        let mut out = Vec::new();
        for i in chosen {
            let label = self.members[i].label.clone();
            for mut row in read(&self.members[i].store)? {
                if label_rows {
                    row.label(&label);
                }
                out.push(row);
            }
        }
        Ok(out)
    }

    /// The one store a write goes to.
    ///
    /// One writer per store is the product's rule, and with several stores open
    /// there is no "the" store — so an unnamed write among several is refused
    /// with the names that exist rather than guessed at. Guessing writes to
    /// somebody's other repository.
    pub fn writable(&mut self, only: &[String]) -> Result<&mut Semlith> {
        let i = match only {
            [] if self.members.len() == 1 => 0,
            [] => bail!(
                "this server has {} stores open, so a write has to name one: {}",
                self.members.len(),
                self.labels().join(", ")
            ),
            [name] => self
                .members
                .iter()
                .position(|m| m.label == *name)
                .with_context(|| {
                    format!(
                        "no store called {name} is open; these are: {}",
                        self.labels().join(", ")
                    )
                })?,
            names => bail!(
                "a write goes to one store, not {}: {}",
                names.len(),
                names.join(", ")
            ),
        };
        Ok(&mut self.members[i].store)
    }

    /// Indices of the stores a query should reach.
    ///
    /// A store that cannot be read is dropped from the selection and reported
    /// through [`Fleet::failed`], so a query over several stores answers from
    /// the ones that work. When nothing the caller asked for can be read there
    /// is no partial answer to give, and this fails naming each store, its path
    /// and its remedy.
    fn chosen(&self, only: Option<&[String]>) -> Result<Vec<usize>> {
        let mut asked = Vec::new();
        // Names that resolved to a store this fleet could not even open. They
        // are not members, so they cannot be selected — but they are what the
        // caller asked about, and the refusal has to be about them.
        let mut blamed: Vec<String> = Vec::new();
        match only.filter(|n| !n.is_empty()) {
            None => {
                asked.extend(0..self.members.len());
                blamed.extend(self.broken.iter().map(|b| b.label.clone()));
            }
            Some(names) => {
                for name in names {
                    if let Some(i) = self.members.iter().position(|m| m.label == *name) {
                        if !asked.contains(&i) {
                            asked.push(i);
                        }
                    } else if self.broken.iter().any(|b| b.label == *name) {
                        if !blamed.contains(name) {
                            blamed.push(name.clone());
                        }
                    } else {
                        bail!(
                            "no store called {name} is open; these are: {}",
                            self.labels().join(", ")
                        );
                    }
                }
            }
        }

        let live: Vec<usize> = asked
            .iter()
            .copied()
            .filter(|i| self.readable(*i))
            .collect();
        if live.is_empty() && !(asked.is_empty() && blamed.is_empty()) {
            let mut failed = self.failed();
            failed.retain(|f| {
                blamed.contains(&f.store) || asked.iter().any(|i| self.members[*i].label == f.store)
            });
            bail!("{}", nothing_readable(&failed));
        }
        Ok(live)
    }

    fn embed_query(&mut self, model: &Model, query: &str) -> Result<Vec<f32>> {
        let text = model.query_text(query);
        let session = self.embedder(model)?;
        let _lifted = crate::priority::embedding();
        let mut out = session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .embed(vec![text], Some(1))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.embeds += 1;
        let mut vector = out.remove(0);
        crate::normalize(&mut vector);
        Ok(vector)
    }

    /// The query session for `model`, shared by every reader in the process
    /// and loaded the first time any of them asks.
    ///
    /// Shared since 0.28.0. The daemon holds two readers — the portal's and
    /// the forwarded MCP calls' — and each held its own session and its own
    /// arena, which is memory a daemon pays for doing nothing. One session,
    /// kept loaded, so search is never charged a model load.
    fn embedder(&mut self, model: &Model) -> Result<Arc<Mutex<TextEmbedding>>> {
        let cache = model_cache_dir()?;
        // Read from the same cache in the same breath as the weights, so the
        // count and the model's own segmentation are the same arithmetic.
        if self.tokenizer.is_none() {
            self.tokenizer = model.tokenizer(&cache);
        }
        let mut shared = QUERY_SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, session)) = shared.iter().find(|(m, _)| m == model) {
            return Ok(Arc::clone(session));
        }
        let loaded = Arc::new(Mutex::new(model.load_variant(
            cache,
            chunk::MAX_CHARS / 2,
            self.quiet,
            crate::embed::embed_threads(),
            crate::embed::query_variant(),
        )?));
        shared.push((model.clone(), Arc::clone(&loaded)));
        Ok(loaded)
    }
}

type QuerySessions = Vec<(Model, Arc<Mutex<TextEmbedding>>)>;

/// Query sessions, one per model, shared by every reader in the process.
static QUERY_SESSIONS: Mutex<QuerySessions> = Mutex::new(Vec::new());

/// How many query sessions are loaded, for `/api/about`.
pub fn query_sessions() -> usize {
    QUERY_SESSIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .len()
}

/// Take the best `k` hits from per-store rankings, best first.
///
/// A merge rather than a sort, because each store's list is already ranked and
/// that order must survive: a store's own answer to the query is not up for
/// re-litigation by another store's numbers.
///
/// The key is the fused score first. Those are reciprocal-rank sums produced by
/// the same formula at the same depth in every store, so they are the one
/// quantity that is the same unit across stores — even across two different
/// models — and a chunk that both halves of its store ranked outranks one only
/// the dense half found.
///
/// The tie is where the work is. Every store has a best hit whether or not it
/// has an answer, and a store whose top result is dense-rank-1 scores exactly
/// what another store's dense-rank-1 scores. Ties therefore go to the higher
/// similarity to the query vector: it is the only evidence available about which
/// of two equally-ranked chunks is actually closer to what was asked. Across two
/// models it compares numbers from two vector spaces, which is approximate — but
/// it only ever decides between hits the rank evidence has already called equal,
/// so the worst case is a reordering among equals rather than a wrong answer.
fn merge(queues: Vec<Vec<(Hit, f32)>>, k: usize) -> Vec<Hit> {
    let mut queues: Vec<VecDeque<(Hit, f32)>> = queues.into_iter().map(VecDeque::from).collect();
    let mut out = Vec::new();

    while out.len() < k {
        let mut best: Option<(usize, f32, f32)> = None;
        for (i, queue) in queues.iter().enumerate() {
            let Some((hit, similarity)) = queue.front() else {
                continue;
            };
            // Strictly better, so a tie leaves the earlier store's hit in
            // front: the order the stores were named in is the last tiebreak
            // rather than an accident of iteration.
            let better = match best {
                None => true,
                Some((_, score, sim)) => {
                    hit.score > score || (hit.score == score && *similarity > sim)
                }
            };
            if better {
                best = Some((i, hit.score, *similarity));
            }
        }
        let Some((i, _, _)) = best else { break };
        let (hit, _) = queues[i].pop_front().expect("the winner has a head");
        out.push(hit);
    }

    out
}

/// What a store is called in output.
///
/// Almost every store is a `.semlith` directory, so its own name says nothing;
/// the directory holding it is what a developer would say out loud. A store
/// directory named something else keeps its own name.
fn label(dir: &Path) -> String {
    let own = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if (own.is_empty() || own.starts_with('.'))
        && let Some(parent) = dir
            .parent()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
    {
        return parent;
    }
    if own.is_empty() {
        dir.display().to_string()
    } else {
        own
    }
}

/// The caller's name for a store directory, if it gave one worth having.
///
/// Split out from [`Fleet::name_from`] so the rule is testable without opening
/// a store: an empty name is not a name and must not blank a good label, and a
/// directory the caller said nothing about keeps what it had.
fn name_for(dir: &Path, named: &[(PathBuf, String)]) -> Option<String> {
    let dir = canonical(dir);
    named
        .iter()
        .find(|(at, _)| canonical(at) == dir)
        .map(|(_, name)| name.clone())
        .filter(|name| !name.is_empty())
}

/// Two checkouts of the same repository produce the same label, and a label
/// that names two stores is worse than a long one.
fn disambiguate(members: &mut [Member], keys: &[PathBuf]) {
    let labels: Vec<String> = members.iter().map(|m| m.label.clone()).collect();
    for (i, member) in members.iter_mut().enumerate() {
        if labels
            .iter()
            .enumerate()
            .any(|(j, l)| j != i && *l == labels[i])
        {
            member.label = keys[i].display().to_string();
        }
    }
}

/// A graph row that can say which store it came from.
///
/// The three graph reads return three different shapes over the same rows, and
/// labelling is the only thing `Fleet` adds to any of them.
pub trait Labelled {
    fn label(&mut self, store: &str);
}

impl Labelled for crate::store::SymbolRow {
    fn label(&mut self, store: &str) {
        self.store = Some(store.to_string());
    }
}

impl Labelled for crate::store::PastSymbol {
    fn label(&mut self, store: &str) {
        self.store = Some(store.to_string());
    }
}

impl Labelled for crate::store::EdgeEnd {
    fn label(&mut self, store: &str) {
        self.symbol.store = Some(store.to_string());
    }
}

impl Labelled for crate::store::Unresolved {
    /// Nothing to label. The row is a name the store does *not* hold, so
    /// saying which store did not hold it would be noise: none of them did.
    fn label(&mut self, _store: &str) {}
}

#[cfg(test)]
mod tests {
    use super::name_for;

    /// A store has one name. The daemon's — the registry's, the one a person
    /// typed and the one the portal's chips are made of — wins over the label
    /// derived from the directory's basename, because the two disagreeing is
    /// how the portal came to draw a chip the search route then refused.
    #[test]
    fn a_daemon_name_replaces_the_label_derived_from_the_directory() {
        let dir = std::env::temp_dir()
            .join("semlith-fleet-name-test")
            .join("store");
        // The directory is called `store`; the registry calls it `work`.
        let named = vec![(dir.clone(), "work".to_string())];
        assert_eq!(name_for(&dir, &named).as_deref(), Some("work"));

        // A directory the caller says nothing about keeps what it had, so a
        // fleet opened from the command line is unchanged.
        let elsewhere = std::env::temp_dir().join("semlith-fleet-name-other");
        assert_eq!(name_for(&elsewhere, &named), None);

        // An empty name is not a name, and must not blank a good label.
        let blank = vec![(dir.clone(), String::new())];
        assert_eq!(name_for(&dir, &blank), None);
    }

    use super::*;

    fn hit(path: &str, score: f32) -> Hit {
        Hit {
            score,
            path: path.into(),
            start_line: 1,
            end_line: 1,
            text: String::new(),
            store: None,
            lists: vec!["vector"],
            image: None,
            fresh: true,
            symbol: None,
            symbol_kind: None,
            symbol_line: None,
            provenance: None,
            copies: Vec::new(),
        }
    }

    /// Every store has a best hit whether or not it has an answer, so two
    /// stores' rank-1 results carry the same fused score. Without the
    /// similarity tiebreak the answer is decided by which store was named
    /// first, which is how an unrelated store takes the top of the list.
    #[test]
    fn equal_scores_are_decided_by_similarity_not_by_store_order() {
        let unrelated = vec![(hit("bread.md", 0.0328), 0.31)];
        let answer = vec![(hit("ownership.md", 0.0328), 0.62)];

        let out = merge(vec![unrelated, answer], 5);
        assert_eq!(out[0].path, "ownership.md");
        assert_eq!(out.len(), 2, "nothing may be dropped, only reordered");
    }

    /// A store's own ranking is not up for re-litigation: within one store the
    /// order it returned survives the merge, ties included.
    #[test]
    fn a_stores_own_order_survives_the_merge() {
        // Same score, and the *second* one is closer to the query. Inside one
        // store that order was already decided.
        let one = vec![
            (hit("first.md", 0.0328), 0.10),
            (hit("second.md", 0.0328), 0.90),
        ];
        let out = merge(vec![one], 5);
        assert_eq!(
            out.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
            vec!["first.md", "second.md"]
        );
    }

    #[test]
    fn k_bounds_the_merged_list_not_each_store() {
        let a = vec![(hit("a1", 0.9), 0.9), (hit("a2", 0.8), 0.8)];
        let b = vec![(hit("b1", 0.7), 0.7), (hit("b2", 0.6), 0.6)];
        let out = merge(vec![a, b], 3);
        assert_eq!(
            out.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
            vec!["a1", "a2", "b1"]
        );
    }

    #[test]
    fn a_store_is_named_after_the_directory_holding_it() {
        assert_eq!(label(Path::new("/home/dev/api/.semlith")), "api");
        // A store directory with a name of its own keeps it.
        assert_eq!(label(Path::new("/home/dev/stores/api-store")), "api-store");
        // Nothing to fall back to: the path itself is the only honest answer.
        assert_eq!(label(Path::new("/.semlith")), ".semlith");
    }

    #[test]
    fn stores_that_would_share_a_label_get_their_paths_instead() {
        let keys = [
            PathBuf::from("/a/api/.semlith"),
            PathBuf::from("/b/api/.semlith"),
            PathBuf::from("/b/cli/.semlith"),
        ];
        let mut labels: Vec<String> = keys.iter().map(|k| label(k)).collect();
        assert_eq!(labels, vec!["api", "api", "cli"]);

        // Same rule as `disambiguate`, over labels alone so the test needs no
        // stores on disk.
        let original = labels.clone();
        for i in 0..labels.len() {
            if original
                .iter()
                .enumerate()
                .any(|(j, l)| j != i && *l == original[i])
            {
                labels[i] = keys[i].display().to_string();
            }
        }
        assert_eq!(
            labels,
            vec!["/a/api/.semlith", "/b/api/.semlith", "cli"],
            "a label that names two stores is worse than a long one"
        );
    }
}
