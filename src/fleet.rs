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
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

pub struct Fleet {
    members: Vec<Member>,
    /// One loaded model per distinct model, shared by every store using it.
    /// Three stores built with the same model cost one copy of the weights.
    embedders: Vec<(Model, TextEmbedding)>,
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
}

impl Fleet {
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
        for dir in dirs {
            // A store that does not exist is an error before anything is
            // opened, and `open_existing` is what refuses to create one.
            let store = Semlith::open_existing(dir)?;
            let key = canonical(dir);
            if keys.contains(&key) {
                continue;
            }
            keys.push(key);
            members.push(Member {
                label: String::new(),
                store,
            });
        }

        for (member, key) in members.iter_mut().zip(&keys) {
            member.label = label(key);
        }
        disambiguate(&mut members, &keys);

        Ok(Self {
            members,
            embedders: Vec::new(),
            embeds: 0,
            quiet: false,
        })
    }

    pub fn len(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// `(label, store)` for each store, in the order they were named.
    pub fn each(&self) -> impl Iterator<Item = (&str, &Semlith)> {
        self.members.iter().map(|m| (m.label.as_str(), &m.store))
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
        for member in &self.members {
            total += member.store.stats()?.0;
        }
        Ok(total)
    }

    /// Query embeds performed since this fleet was opened. One per distinct
    /// model per search, not one per store.
    pub fn query_embeds(&self) -> usize {
        self.embeds
    }

    /// Load every distinct model and warm every index, so the first query is
    /// not slower than the rest.
    pub fn warm(&mut self) -> Result<()> {
        for i in 0..self.members.len() {
            let model = self.members[i].store.model().clone();
            self.embedder(&model)?;
            self.members[i].store.warm_index();
        }
        Ok(())
    }

    /// How many indexed files `filter` selects across every store.
    pub fn matching_files(&self, filter: &Filter) -> Result<i64> {
        let mut total = 0;
        for member in &self.members {
            total += member.store.matching_files(filter)?;
        }
        Ok(total)
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
                .search_ranked(query, &vector, k, filter)?;
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
    fn chosen(&self, only: Option<&[String]>) -> Result<Vec<usize>> {
        let Some(names) = only.filter(|n| !n.is_empty()) else {
            return Ok((0..self.members.len()).collect());
        };
        let mut chosen = Vec::new();
        for name in names {
            match self.members.iter().position(|m| m.label == *name) {
                Some(i) => {
                    if !chosen.contains(&i) {
                        chosen.push(i);
                    }
                }
                None => bail!(
                    "no store called {name} is open; these are: {}",
                    self.labels().join(", ")
                ),
            }
        }
        Ok(chosen)
    }

    fn embed_query(&mut self, model: &Model, query: &str) -> Result<Vec<f32>> {
        let text = model.query_text(query);
        let i = self.embedder(model)?;
        let mut out = self.embedders[i]
            .1
            .embed(vec![text], Some(1))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.embeds += 1;
        let mut vector = out.remove(0);
        crate::normalize(&mut vector);
        Ok(vector)
    }

    /// Index into `embedders` for `model`, loading it the first time.
    fn embedder(&mut self, model: &Model) -> Result<usize> {
        if let Some(i) = self.embedders.iter().position(|(m, _)| m == model) {
            return Ok(i);
        }
        let loaded = model.load(model_cache_dir(), chunk::MAX_CHARS / 2, self.quiet)?;
        self.embedders.push((model.clone(), loaded));
        Ok(self.embedders.len() - 1)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str, score: f32) -> Hit {
        Hit {
            score,
            path: path.into(),
            start_line: 1,
            end_line: 1,
            text: String::new(),
            store: None,
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
