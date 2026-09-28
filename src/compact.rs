//! Giving a store's dead bytes back without re-embedding anything.
//!
//! A store only ever updated incrementally never shrinks by itself. Three
//! things hold on to what the store no longer has:
//!
//! - `exact.f32` is append-only: a deleted chunk's full-precision record stays
//!   in the file, and nothing but this module rewrites it.
//! - the shards: turbovec's `remove` swap-removes, so a shard file holds only
//!   live codes, but ids only ascend, so churn leaves a trail of sparse shards
//!   each costing a load at search time.
//! - SQLite: freed pages stay in the file, FTS5 keeps a deleted chunk's terms
//!   until its segments merge, and `symbols_past` has no bound.
//!
//! [`Semlith::compact`] rewrites the sidecar to the live records, packs the
//! shards from those records when the sidecar covers every live vector, prunes
//! retired definitions older than a retention, and vacuums. No live vector's
//! codes change: the shards are re-encoded from the exact values the index was
//! given, and turbovec encodes the same rows to the same bytes however they are
//! batched. Semlith never calibrates, which is what makes that hold.
//!
//! The CLI, the daemon, the portal route and Semlith Cloud all call this one
//! function, so the four cannot disagree about what compacting means.

use crate::index::{self, VectorIndex};
use crate::{BIT_WIDTH, GENERATION, Semlith, generation, image, lock, now, store};
use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

/// How long retired definitions are kept when nobody says otherwise, in days.
pub const RETENTION_DAYS: u64 = 90;

/// The reclaimable share, in percent, past which the daemon compacts an idle
/// store when nobody says otherwise.
pub const AUTO_THRESHOLD_PERCENT: u64 = 25;

/// Below this many reclaimable bytes the daemon leaves a store alone whatever
/// the share says: a store of a few hundred kilobytes is 25 % dead after one
/// edit, and compacting it gives back nothing anyone would notice.
pub const AUTO_FLOOR_BYTES: u64 = 1024 * 1024;

/// The retention in force: the saved setting, else [`RETENTION_DAYS`].
pub fn retention_in_force() -> u64 {
    crate::home::Settings::load()
        .history_retention_days
        .unwrap_or(RETENTION_DAYS)
}

/// The auto-compaction threshold in force: the saved setting, else
/// [`AUTO_THRESHOLD_PERCENT`]. 0 is off.
pub fn threshold_in_force() -> u64 {
    crate::home::Settings::load()
        .compact_threshold_percent
        .unwrap_or(AUTO_THRESHOLD_PERCENT)
}

/// Set in the store's meta table for the length of a vector swap. A reader
/// that sees it, or sees it appear, asks again once the swap is over.
const SWAPPING: &str = "vectors_swapping";

/// Whether a compaction is between the two renames of its swap.
pub(crate) fn swapping(db: &Connection) -> Result<bool> {
    Ok(store::get_meta(db, SWAPPING)?.as_deref() == Some("1"))
}

/// What a store takes on disk, and how much of it a compaction would give
/// back.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct Footprint {
    /// `store.db` with its write-ahead log and shared-memory index.
    pub database: u64,
    /// The full-precision sidecar.
    pub exact: u64,
    /// The quantized shards (or the single `index.tv`), text and images.
    pub vectors: u64,
    /// The part of the three above a compaction would give back: the
    /// sidecar's records for chunks that no longer exist, the database's free
    /// pages, and retired definitions past the retention. The write-ahead log
    /// is not counted: SQLite recycles it by itself.
    pub reclaimable: u64,
}

impl Footprint {
    pub fn total(&self) -> u64 {
        self.database + self.exact + self.vectors
    }

    pub fn live(&self) -> u64 {
        self.total().saturating_sub(self.reclaimable)
    }

    /// The reclaimable share of the total, in whole percent.
    pub fn dead_percent(&self) -> u64 {
        (self.reclaimable * 100)
            .checked_div(self.total())
            .unwrap_or(0)
    }
}

/// What to compact, and how.
#[derive(Debug, Clone, Copy)]
pub struct CompactOptions {
    /// Retired definitions older than this many days are dropped. 0 keeps
    /// every one.
    pub retention_days: u64,
    /// Say what would be given back and change nothing.
    pub dry_run: bool,
}

impl Default for CompactOptions {
    fn default() -> Self {
        Self {
            retention_days: RETENTION_DAYS,
            dry_run: false,
        }
    }
}

/// What a compaction did.
#[derive(Debug, Clone, Serialize)]
pub struct CompactReport {
    pub before: Footprint,
    /// After the compaction; for a dry run, `before` less what it would give
    /// back.
    pub after: Footprint,
    pub dry_run: bool,
    /// Retired definitions dropped by the retention (would be, for a dry run).
    pub history_dropped: u64,
    /// Full-precision records dropped because their chunk no longer exists.
    pub vectors_dropped: u64,
    pub shards_before: usize,
    pub shards_after: usize,
    /// A stop asked for before the swap: the vectors are as they were, and
    /// nothing else was touched.
    pub stopped: bool,
    /// What could not be compacted, and why. Empty when everything was.
    pub notes: Vec<String>,
}

/// The sentence a store that cannot have its vectors compacted gets, the same
/// on every surface.
pub const REINDEX_TO_COMPACT: &str = "re-index to compact vectors";

impl Semlith {
    /// What this store takes on disk and what a compaction with this retention
    /// would give back. Reads file sizes and three small queries; never the
    /// vectors.
    pub fn footprint(&self, retention_days: u64) -> Result<Footprint> {
        let size = |p: std::path::PathBuf| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let database = ["store.db", "store.db-wal", "store.db-shm"]
            .iter()
            .map(|name| size(self.dir.join(name)))
            .sum();
        let exact = if self.exact.exists() {
            self.exact.bytes()
        } else {
            0
        };
        let vectors =
            index::vector_bytes(&self.dir) + index::vector_bytes(&self.dir.join(image::INDEX_DIR));

        let partial = Footprint {
            exact,
            ..Default::default()
        };
        let exact_dead = exact_dead(&partial, self)?;
        let history = match cutoff(retention_days) {
            Some(before) => store::history_bytes_before(&self.db, before)? as u64,
            None => 0,
        };
        let free = store::free_bytes(&self.db)? as u64;
        Ok(Footprint {
            database,
            exact,
            vectors,
            reclaimable: exact_dead + history + free,
        })
    }

    /// Whether a compaction can rewrite this store's vectors: it has the
    /// full-precision sidecar and the sharded layout. A store without either
    /// is compacted as far as it can be and told to re-index.
    pub fn compacts_vectors(&self) -> bool {
        self.exact.exists() && matches!(self.index, VectorIndex::Sharded(_))
    }

    /// Compact this store, taking its writer's lock for the length of it.
    /// A dry run reads and takes no lock, so it can be asked of a store a
    /// writer holds.
    pub fn compact(&mut self, options: &CompactOptions) -> Result<CompactReport> {
        if options.dry_run {
            return self.compact_writing(options, &|| false);
        }
        let _lock = lock::StoreLock::acquire(&self.dir)?;
        self.compact_held(options, &|| false)
    }

    /// [`Semlith::compact`] for a caller that already holds the lock -- the
    /// daemon, which is the writer for every store it opened. `stop` is asked
    /// between the steps that build the new files; once they are swapped in
    /// the rest runs to the end.
    pub fn compact_held(
        &mut self,
        options: &CompactOptions,
        stop: &dyn Fn() -> bool,
    ) -> Result<CompactReport> {
        self.writing(|me| me.compact_writing(options, stop))
    }

    fn compact_writing(
        &mut self,
        options: &CompactOptions,
        stop: &dyn Fn() -> bool,
    ) -> Result<CompactReport> {
        let before = self.footprint(options.retention_days)?;
        let shards_before = self.index.shards();
        let history_due = match cutoff(options.retention_days) {
            Some(c) => store::history_count_before(&self.db, c)? as u64,
            None => 0,
        };
        let mut notes = Vec::new();
        let sharded = matches!(self.index, VectorIndex::Sharded(_));
        if !self.exact.exists() {
            notes.push(format!(
                "no full-precision vectors (written before 0.23.0): {REINDEX_TO_COMPACT}"
            ));
        } else if !sharded {
            notes.push(format!(
                "single-file index (written before 0.7.0): {REINDEX_TO_COMPACT}"
            ));
        }

        if options.dry_run {
            let dead = exact_dead(&before, self)?;
            let after = Footprint {
                database: before.database.saturating_sub(before.reclaimable - dead),
                exact: before.exact - dead,
                vectors: before.vectors,
                reclaimable: 0,
            };
            return Ok(CompactReport {
                before,
                after,
                dry_run: true,
                history_dropped: history_due,
                vectors_dropped: dead / self.exact.record_bytes(),
                shards_before,
                shards_after: shards_before,
                stopped: false,
                notes,
            });
        }

        // A compaction killed part-way is finished or undone first, and what
        // this process holds in memory is made durable, so the files read
        // below are the whole of the store.
        if index::recover(&self.dir)? {
            self.reopen_indexes()?;
        }
        store::set_meta(&self.db, SWAPPING, "0")?;
        self.save()?;
        self.index.evict();
        let stopped = |me: &Self, notes: Vec<String>| -> Result<CompactReport> {
            index::discard_compacted(&me.dir)?;
            Ok(CompactReport {
                before,
                after: before,
                dry_run: false,
                history_dropped: 0,
                vectors_dropped: 0,
                shards_before,
                shards_after: shards_before,
                stopped: true,
                notes,
            })
        };

        let mut vectors_dropped = 0;
        if self.exact.exists() {
            if stop() {
                return stopped(self, notes);
            }
            let live = index::ids_on_disk(&self.dir, self.dim, BIT_WIDTH)?;
            let rewrite = self.exact.build_compacted(&live)?;
            vectors_dropped = rewrite.dropped;
            let covered = rewrite.kept == live.len() as u64;
            let pack = sharded && rewrite.ascending && covered;
            if sharded && !covered {
                notes.push(format!(
                    "{} vectors have no full-precision copy, so the shards were left as they are: \
                     {REINDEX_TO_COMPACT}",
                    live.len() as u64 - rewrite.kept
                ));
            } else if sharded && !rewrite.ascending {
                notes.push(format!(
                    "the full-precision file is out of id order, so the shards were left as they \
                     are: {REINDEX_TO_COMPACT}"
                ));
            }
            if stop() {
                return stopped(self, notes);
            }
            if pack {
                let capacity = self.index.capacity().unwrap_or(index::SHARD_VECTORS);
                index::build_compacted(
                    &self.dir,
                    &self.exact.compacted(),
                    self.dim,
                    BIT_WIDTH,
                    capacity,
                )?;
            }
            if stop() {
                return stopped(self, notes);
            }

            // The swap, bracketed so a reader in another process either sees
            // the old set whole, the new set whole, or asks again.
            store::set_meta(&self.db, SWAPPING, "1")?;
            let swapped = self.exact.swap_compacted().and_then(|()| {
                if pack {
                    index::swap_compacted(&self.dir)
                } else {
                    Ok(())
                }
            });
            self.generation = generation(&self.db)? + 1;
            store::set_meta(&self.db, GENERATION, &self.generation.to_string())?;
            store::set_meta(&self.db, SWAPPING, "0")?;
            swapped?;
            self.reopen_indexes()?;
        }

        let history_dropped = match cutoff(options.retention_days) {
            Some(c) => store::prune_history(&self.db, c)? as u64,
            None => 0,
        };
        store::reclaim(&self.db)?;

        Ok(CompactReport {
            before,
            after: self.footprint(options.retention_days)?,
            dry_run: false,
            history_dropped,
            vectors_dropped,
            shards_before,
            shards_after: self.index.shards(),
            stopped: false,
            notes,
        })
    }
}

/// The sidecar bytes a compaction would drop.
///
/// Conservative: every chunk is assumed to have its record, so a store
/// part-written before the sidecar existed under-reports rather than promising
/// bytes a compaction cannot give back.
fn exact_dead(before: &Footprint, store: &Semlith) -> Result<u64> {
    let chunks = store::chunk_count(&store.db)? as u64;
    Ok(before
        .exact
        .saturating_sub(chunks * store.exact.record_bytes()))
}

/// The unix second before which a definition is past `retention_days`, or
/// `None` when every definition is kept.
fn cutoff(retention_days: u64) -> Option<i64> {
    (retention_days > 0).then(|| now() - (retention_days as i64) * 86_400)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Allowlist;
    use std::path::{Path, PathBuf};

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("semlith-compact-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A unit vector no two seeds share, without a model.
    fn vector(dim: usize, seed: u64) -> Vec<f32> {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut v: Vec<f32> = (0..dim)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state % 2001) as f32 / 1000.0 - 1.0
            })
            .collect();
        crate::normalize(&mut v);
        v
    }

    /// A store with shards of eight, so churn spans several of them.
    fn store(dir: &Path) -> Semlith {
        let mut s = Semlith::open(dir, None).unwrap();
        s.index.set_capacity(8);
        s
    }

    /// Index `n` chunks for `path` the way a pass does: rows, codes and the
    /// sidecar from the same values, then a save.
    fn put(s: &mut Semlith, path: &str, n: usize, seed: u64) {
        s.writing(|me| {
            let file = store::insert_file(&me.db, path, "h", 10, now())?;
            let (mut ids, mut flat) = (Vec::new(), Vec::new());
            for i in 0..n {
                let id = store::insert_chunk(&me.db, file, i, 1, 2, &format!("{path} {i}"))?;
                ids.push(id as u64);
                flat.extend(vector(me.dim, seed + i as u64));
            }
            me.index.add(&flat, &ids)?;
            me.exact.append(&flat, &ids)?;
            me.save()
        })
        .unwrap();
    }

    fn remove(s: &mut Semlith, path: &str) {
        s.writing(|me| {
            me.evict(path)?;
            me.save()
        })
        .unwrap();
    }

    /// Delete, re-add and delete, the way edits and renames churn a store.
    fn churned(dir: &Path) -> Semlith {
        let mut s = store(dir);
        put(&mut s, "/t/a.rs", 10, 100);
        put(&mut s, "/t/b.rs", 10, 200);
        put(&mut s, "/t/c.rs", 10, 300);
        remove(&mut s, "/t/b.rs");
        put(&mut s, "/t/b.rs", 10, 400);
        remove(&mut s, "/t/a.rs");
        s
    }

    /// What a reader sees: ids and code scores for a few queries, and the
    /// full-precision cosine of each hit, which is what rescoring reorders by.
    /// One query's ids, code scores and full-precision cosines.
    type Answer = (Vec<u64>, Vec<f32>, Vec<Option<f32>>);

    fn answers(s: &mut Semlith) -> Vec<Answer> {
        (0..5u64)
            .map(|q| {
                let query = vector(s.dim, 7000 + q);
                let (scores, ids) = s.search_vectors(&query, 12, &Allowlist::All).unwrap();
                let rescored = ids
                    .iter()
                    .map(|id| s.exact.get(*id).map(|v| index::cosine(&query, &v)))
                    .collect();
                (ids, scores, rescored)
            })
            .collect()
    }

    fn shard_sizes(dir: &Path) -> Vec<usize> {
        index::vector_files(dir)
            .unwrap()
            .iter()
            .map(|p| turbovec::IdMapIndex::load(p).unwrap().len())
            .collect()
    }

    /// Every file and a hash of its bytes. SQLite's shared-memory index is
    /// left out: a reader's snapshot marks live there, and it holds no rows.
    fn files_under(dir: &Path) -> Vec<(PathBuf, u64)> {
        use std::hash::{Hash, Hasher};
        let mut out = Vec::new();
        for entry in walkdir(dir) {
            if entry.to_string_lossy().ends_with("-shm") {
                continue;
            }
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::fs::read(&entry).unwrap().hash(&mut h);
            out.push((entry, h.finish()));
        }
        out.sort();
        out
    }

    fn walkdir(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walkdir(&path));
            } else {
                out.push(path);
            }
        }
        out
    }

    #[test]
    fn compaction_keeps_every_answer_and_drops_every_dead_byte() {
        let dir = scratch("identity");
        let mut s = churned(&dir);
        let live = index::ids_on_disk(&dir, s.dim, BIT_WIDTH).unwrap();
        assert_eq!(live.len(), 20);
        let records: Vec<Vec<f32>> = live.iter().map(|id| s.exact.get(*id).unwrap()).collect();
        let before = answers(&mut s);
        let footprint = s.footprint(RETENTION_DAYS).unwrap();
        assert!(footprint.reclaimable > 0, "a churned store has dead bytes");
        assert!(
            shard_sizes(&dir).iter().any(|n| *n < 8),
            "churn left sparse shards"
        );

        let report = s.compact(&CompactOptions::default()).unwrap();
        assert!(report.notes.is_empty(), "{:?}", report.notes);
        assert_eq!(report.vectors_dropped, 20);

        // Same ids, same order, same scores, rescored and not.
        assert_eq!(answers(&mut s), before);
        // Every live record byte for byte, no dead one, ids ascending.
        assert_eq!(index::ids_on_disk(&dir, s.dim, BIT_WIDTH).unwrap(), live);
        for (id, record) in live.iter().zip(&records) {
            assert_eq!(s.exact.get(*id).as_ref(), Some(record));
        }
        assert_eq!(s.exact.bytes(), 20 * s.exact.record_bytes());
        // Packed: every shard but the last is full.
        assert_eq!(shard_sizes(&dir), vec![8, 8, 4]);
        assert_eq!(s.footprint(RETENTION_DAYS).unwrap().reclaimable, 0);
        assert_eq!(report.after, s.footprint(RETENTION_DAYS).unwrap());

        // And a fresh open of the compacted store answers the same.
        drop(s);
        let mut reopened = Semlith::open(&dir, None).unwrap();
        assert_eq!(answers(&mut reopened), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reader_in_another_connection_answers_across_a_compaction() {
        let dir = scratch("reader");
        let mut writer = churned(&dir);
        let mut reader = Semlith::open(&dir, None).unwrap();
        let before = answers(&mut reader);
        writer.compact(&CompactOptions::default()).unwrap();
        assert_eq!(answers(&mut reader), before);
        // A shard the writer creates after the reader opened is seen too.
        put(&mut writer, "/t/d.rs", 10, 500);
        let query = vector(writer.dim, 501);
        let (_, ids) = reader.search_vectors(&query, 1, &Allowlist::All).unwrap();
        let (_, want) = writer.search_vectors(&query, 1, &Allowlist::All).unwrap();
        assert_eq!(
            ids, want,
            "the reader missed a shard created after it opened"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_inside_the_retention_is_kept_and_older_is_dropped() {
        let dir = scratch("history");
        let mut s = store(&dir);
        put(&mut s, "/t/a.rs", 2, 1);
        let day = 86_400;
        s.writing(|me| {
            for (name, age) in [("recent", 10), ("ancient", 100)] {
                me.db.execute(
                    "INSERT INTO symbols_past
                         (path, kind, name, qualified, start_line, end_line, content_hash, retired_at)
                     VALUES ('/t/a.rs', 'function', ?1, ?1, 1, 2, 'h', ?2)",
                    rusqlite::params![name, now() - age * day],
                )?;
            }
            Ok(())
        })
        .unwrap();

        let dry = s
            .compact(&CompactOptions {
                dry_run: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(dry.history_dropped, 1);
        assert_eq!(s.retired_symbols().unwrap(), 2, "a dry run dropped history");

        let kept = s
            .compact(&CompactOptions {
                retention_days: 0,
                dry_run: false,
            })
            .unwrap();
        assert_eq!(kept.history_dropped, 0);
        assert_eq!(s.retired_symbols().unwrap(), 2);

        let report = s.compact(&CompactOptions::default()).unwrap();
        assert_eq!(report.history_dropped, 1);
        assert_eq!(
            store::symbols_past_named(&s.db, "recent", 5).unwrap().len(),
            1
        );
        assert!(
            store::symbols_past_named(&s.db, "ancient", 5)
                .unwrap()
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dry_run_changes_no_byte() {
        let dir = scratch("dry");
        let mut s = churned(&dir);
        let before = files_under(&dir);
        let report = s
            .compact(&CompactOptions {
                dry_run: true,
                ..Default::default()
            })
            .unwrap();
        assert!(report.dry_run);
        assert_eq!(report.vectors_dropped, 20);
        assert_eq!(report.after.exact, 20 * s.exact.record_bytes());
        assert_eq!(files_under(&dir), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Killed after the new set was written and before the swap: the store
    /// answers from the old set, and the next compaction throws the new one
    /// away and does the work again.
    #[test]
    fn a_compaction_killed_before_its_swap_leaves_the_store_answering() {
        let dir = scratch("killed-before");
        let mut s = churned(&dir);
        let before = answers(&mut s);
        let live = index::ids_on_disk(&dir, s.dim, BIT_WIDTH).unwrap();
        s.exact.build_compacted(&live).unwrap();
        index::build_compacted(&dir, &s.exact.compacted(), s.dim, BIT_WIDTH, 8).unwrap();
        drop(s);

        let mut reader = Semlith::open(&dir, None).unwrap();
        assert_eq!(answers(&mut reader), before);
        reader.index.set_capacity(8);
        reader.compact(&CompactOptions::default()).unwrap();
        assert_eq!(answers(&mut reader), before);
        assert!(!dir.join("index.compact").exists());
        assert!(!dir.join("exact.f32.compact").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Killed between the swap's two renames: only the new set is on disk. A
    /// reader answers from it, and the next writer names it `index/`.
    #[test]
    fn a_compaction_killed_inside_its_swap_leaves_the_store_answering() {
        let dir = scratch("killed-inside");
        let mut s = churned(&dir);
        let before = answers(&mut s);
        let live = index::ids_on_disk(&dir, s.dim, BIT_WIDTH).unwrap();
        s.exact.build_compacted(&live).unwrap();
        index::build_compacted(&dir, &s.exact.compacted(), s.dim, BIT_WIDTH, 8).unwrap();
        std::fs::rename(dir.join("index"), dir.join("index.old")).unwrap();
        drop(s);

        let mut reader = Semlith::open(&dir, None).unwrap();
        assert_eq!(answers(&mut reader), before);
        assert!(index::recover(&dir).unwrap(), "the built set was not taken");
        assert!(dir.join("index").is_dir() && !dir.join("index.old").exists());
        let mut after = Semlith::open(&dir, None).unwrap();
        assert_eq!(answers(&mut after), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #148: a file refused for a secret it now holds leaves no byte of that
    /// secret in the database file or its log once the run that evicted it
    /// has scrubbed -- which is what `index_set_writing` does when `scrub` is
    /// set.
    #[test]
    fn an_evicted_secret_leaves_no_bytes_behind() {
        let dir = scratch("scrub");
        let mut s = store(&dir);
        let secret = "zq_live_7fG2kLm9Qx4Rt8Vw1Yb6Nc3Hd5Js0Pa";
        s.writing(|me| {
            let file = store::insert_file(&me.db, "/t/env.rs", "h", 10, now())?;
            let id =
                store::insert_chunk(&me.db, file, 0, 1, 1, &format!("let key = \"{secret}\";"))?;
            let v = vector(me.dim, 9);
            me.index.add(&v, &[id as u64])?;
            me.exact.append(&v, &[id as u64])?;
            me.save()
        })
        .unwrap();
        s.writing(|me| {
            me.evict("/t/env.rs")?;
            me.save()?;
            store::reclaim(&me.db)
        })
        .unwrap();
        for name in ["store.db", "store.db-wal"] {
            let bytes = std::fs::read(dir.join(name)).unwrap_or_default();
            let lower = secret.to_lowercase();
            for needle in [secret.as_bytes(), lower.as_bytes()] {
                assert!(
                    !bytes.windows(needle.len()).any(|w| w == needle),
                    "{name} still holds the secret"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stopped_compaction_leaves_every_file_as_it_was() {
        let dir = scratch("stopped");
        let mut s = churned(&dir);
        s.writing(|me| me.save()).unwrap();
        let before = files_under(&dir);
        let asked = std::cell::Cell::new(0);
        // Stopped at the second question: after the sidecar was rewritten
        // beside the live one and before any shard was built.
        let report = s
            .compact_held(&CompactOptions::default(), &|| {
                asked.set(asked.get() + 1);
                asked.get() >= 2
            })
            .unwrap();
        assert!(report.stopped);
        let after: Vec<_> = files_under(&dir)
            .into_iter()
            .filter(|(p, _)| !p.ends_with("index.lock"))
            .collect();
        let before: Vec<_> = before
            .into_iter()
            .filter(|(p, _)| !p.ends_with("index.lock"))
            .collect();
        assert_eq!(
            after.iter().map(|(p, _)| p).collect::<Vec<_>>(),
            before.iter().map(|(p, _)| p).collect::<Vec<_>>()
        );
        for ((p, a), (_, b)) in after.iter().zip(&before) {
            if !p.to_string_lossy().contains("store.db") {
                assert_eq!(a, b, "{} changed", p.display());
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A store written before the sidecar existed keeps its shards, is told
    /// how to get them compacted, and still has its database reclaimed.
    #[test]
    fn a_store_without_full_precision_vectors_is_compacted_as_far_as_it_can_be() {
        let dir = scratch("no-exact");
        let mut s = churned(&dir);
        std::fs::remove_file(dir.join("exact.f32")).unwrap();
        let shards = shard_sizes(&dir);
        let before = answers(&mut s);
        let report = s.compact(&CompactOptions::default()).unwrap();
        assert!(
            report.notes.iter().any(|n| n.contains(REINDEX_TO_COMPACT)),
            "{:?}",
            report.notes
        );
        assert_eq!(shard_sizes(&dir), shards);
        assert_eq!(answers(&mut s), before);
        assert_eq!(store::free_bytes(&s.db).unwrap(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
