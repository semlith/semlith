//! A machine-wide cache of vectors, so the same chunk is embedded once.
//!
//! An edited repository re-embeds every chunk of every file it touched, though
//! most of those chunks did not change; a second worktree, or a store over the
//! same code, embeds all of it again. Keyed by what the model is shown — the
//! chunk's embedded text — together with everything that decides the vector
//! it gets back: the model and its pinned revision, the precision variant, the
//! chunking rules and the truncation. Change any of those and the key moves,
//! so a stale vector cannot be handed out.
//!
//! One SQLite file under the semlith home, never in a store: a store is
//! something a person may copy or delete, and the cache is shared by all of
//! them. Store compaction never touches it and its eviction never touches a
//! store. Bounded by a size cap with least-recently-used eviction; 0 turns it
//! off. Deleting the file is safe: it is a cache.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::PathBuf;

/// The default cap. At 1 604 bytes a row, about 650 000 vectors: the whole of
/// the 70-repository corpus this release was measured on fits.
pub const DEFAULT_CAP_MB: u64 = 1024;

/// Overrides the cap, in megabytes; 0 turns the cache off.
pub const CAP_ENV: &str = "SEMLITH_VECTOR_CACHE_MB";

/// Bytes a row takes on disk, measured: 1 703 a row over ten thousand
/// vectors, the key, the two indexes and the pages' slack included. What the
/// cap is counted in, so a full cache is the size of its cap.
const ROW_BYTES: u64 = 1_700;

/// The cap in force, in bytes. 0 is off.
pub fn cap_bytes() -> u64 {
    let mb = std::env::var(CAP_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .or_else(|| crate::home::Settings::load().vector_cache_mb)
        .unwrap_or(DEFAULT_CAP_MB);
    mb.saturating_mul(1024 * 1024)
}

pub fn path() -> Result<PathBuf> {
    Ok(crate::home::home_or_error()?
        .join("cache")
        .join("vectors.db"))
}

/// What decides a vector besides its text, folded into every key.
#[derive(Debug, Clone)]
pub struct Scope {
    /// The model's name and the revision its weights are pinned at.
    pub model: String,
    /// The chunking rules, which decide what text a chunk is.
    pub chunker: u32,
    /// The token count the model is cut off at.
    pub truncation: usize,
}

impl Scope {
    pub fn of(model: &crate::embed::Model) -> Self {
        let model = match model {
            crate::embed::Model::Granite => {
                format!(
                    "{}@{}",
                    crate::embed::GRANITE_NAME,
                    crate::embed::GRANITE_REVISION
                )
            }
            other => other.to_string(),
        };
        Self {
            model,
            chunker: crate::store::FORMAT_VERSION,
            truncation: crate::chunk::MAX_CHARS / 2,
        }
    }

    /// The hash of one chunk's embedded text: what the model is shown.
    pub fn text(text: &str) -> [u8; 32] {
        *blake3::hash(text.as_bytes()).as_bytes()
    }

    /// The key for one chunk, from its text's hash, under one variant.
    pub fn key(&self, text: &[u8; 32], variant: &str) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        for part in [
            self.model.as_bytes(),
            variant.as_bytes(),
            &self.chunker.to_le_bytes(),
            &(self.truncation as u64).to_le_bytes(),
        ] {
            hasher.update(&(part.len() as u64).to_le_bytes());
            hasher.update(part);
        }
        hasher.update(text);
        *hasher.finalize().as_bytes()
    }
}

/// A connection to the cache, or nothing when it is off or cannot be opened.
/// A cache that cannot be read is a run that embeds everything, never a run
/// that fails.
pub struct Cache {
    db: Connection,
}

impl Cache {
    pub fn open() -> Option<Self> {
        if cap_bytes() == 0 {
            return None;
        }
        Self::open_at(&path().ok()?).ok()
    }

    pub fn open_at(path: &std::path::Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            crate::home::secure_dir(dir)?;
        }
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        // Before WAL, which fixes it: sixteen-kilobyte pages hold ten vectors
        // where four-kilobyte pages held two and left a quarter empty. Only a
        // new file takes it.
        db.pragma_update(None, "page_size", 16_384)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        // A rowid table: a WITHOUT ROWID row keeps about a kilobyte on its
        // page and moves the rest of a 1.5 KB vector to an overflow page of its
        // own, so each vector took 4.7 KB on disk and a full cache three times
        // its cap.
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS vectors (
                 key BLOB PRIMARY KEY,
                 variant TEXT NOT NULL,
                 vector BLOB NOT NULL,
                 used INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS vectors_used ON vectors(used);
             CREATE TABLE IF NOT EXISTS counts (k TEXT PRIMARY KEY, v INTEGER NOT NULL);",
        )?;
        // The row count is kept, not counted: a COUNT(*) reads every row of a
        // cache that may be a gigabyte, and the portal asks every second.
        // Counted once, for a cache written before the count was kept.
        let kept: Option<i64> = db
            .query_row("SELECT v FROM counts WHERE k = 'rows'", [], |r| r.get(0))
            .optional()?;
        if kept.is_none() {
            db.execute(
                "INSERT OR IGNORE INTO counts (k, v) SELECT 'rows', COUNT(*) FROM vectors",
                [],
            )?;
        }
        crate::home::tighten_file(path);
        Ok(Self { db })
    }

    fn rows(&self) -> Result<u64> {
        count(&self.db, "rows")
    }

    /// The vector for one key, if the cache holds it.
    pub fn get(&self, key: &[u8; 32]) -> Option<Vec<f32>> {
        let bytes: Vec<u8> = self
            .db
            .query_row(
                "SELECT vector FROM vectors WHERE key = ?1",
                params![&key[..]],
                |r| r.get(0),
            )
            .optional()
            .ok()??;
        bytes.len().is_multiple_of(4).then(|| {
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect()
        })
    }

    /// Record what a run put in and what it took out, and evict past the cap.
    /// One transaction: a run's worth of rows, not a commit per row.
    pub fn record(
        &mut self,
        fresh: &[([u8; 32], &'static str, &[f32])],
        hits: &[[u8; 32]],
        lookups: u64,
    ) -> Result<()> {
        let now = crate::now();
        let tx = self.db.transaction()?;
        {
            let mut put = tx.prepare_cached(
                "INSERT OR IGNORE INTO vectors (key, variant, vector, used) VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut touch = tx.prepare_cached("UPDATE vectors SET used = ?2 WHERE key = ?1")?;
            let mut added = 0i64;
            for (key, variant, vector) in fresh {
                let bytes: Vec<u8> = vector.iter().flat_map(|v| v.to_le_bytes()).collect();
                // Already there when another process embedded it meanwhile.
                if put.execute(params![&key[..], variant, bytes, now])? == 0 {
                    touch.execute(params![&key[..], now])?;
                } else {
                    added += 1;
                }
            }
            for key in hits {
                touch.execute(params![&key[..], now])?;
            }
            let mut count = tx.prepare_cached(
                "INSERT INTO counts (k, v) VALUES (?1, ?2)
                 ON CONFLICT(k) DO UPDATE SET v = v + excluded.v",
            )?;
            count.execute(params!["hits", hits.len() as i64])?;
            count.execute(params!["lookups", lookups as i64])?;
            count.execute(params!["rows", added])?;
        }
        tx.commit()?;
        self.evict(cap_bytes())
    }

    /// Drop the least recently used rows until the cache is under 90 % of
    /// `cap`, so a run that just crossed it does not evict again on the next.
    pub fn evict(&mut self, cap: u64) -> Result<()> {
        let rows = self.rows()?;
        if cap == 0 || rows * ROW_BYTES <= cap {
            return Ok(());
        }
        let keep = cap * 9 / 10 / ROW_BYTES;
        let drop = rows.saturating_sub(keep);
        let tx = self.db.transaction()?;
        let gone = tx.execute(
            "DELETE FROM vectors WHERE key IN
                 (SELECT key FROM vectors ORDER BY used ASC, key ASC LIMIT ?1)",
            params![drop as i64],
        )?;
        tx.execute(
            "UPDATE counts SET v = v - ?1 WHERE k = 'rows'",
            params![gone as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Rows, bytes they are counted at, and the lifetime hit rate.
    pub fn stats(&self) -> Result<Stats> {
        stats_of(&self.db)
    }
}

/// One of the kept counts; zero before anything has counted it.
fn count(db: &Connection, k: &str) -> Result<u64> {
    Ok(db
        .query_row("SELECT v FROM counts WHERE k = ?1", params![k], |r| {
            r.get::<_, i64>(0)
        })
        .optional()?
        .unwrap_or(0)
        .max(0) as u64)
}

fn stats_of(db: &Connection) -> Result<Stats> {
    let rows = count(db, "rows")?;
    Ok(Stats {
        rows,
        bytes: rows * ROW_BYTES,
        cap: cap_bytes(),
        hits: count(db, "hits")?,
        lookups: count(db, "lookups")?,
    })
}

/// What `stats`, `semlith_stats` and Machine limits say about the cache.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Stats {
    pub rows: u64,
    pub bytes: u64,
    pub cap: u64,
    pub hits: u64,
    pub lookups: u64,
}

impl Stats {
    /// The cache as one line, or that it is off.
    pub fn line(&self) -> String {
        if self.cap == 0 {
            return "off (vector cache cap is 0)".to_string();
        }
        let rate = if self.lookups > 0 {
            format!(
                ", {:.0} % of lookups hit",
                self.hits as f64 * 100.0 / self.lookups as f64
            )
        } else {
            String::new()
        };
        format!(
            "{} vectors, {} of {}{rate}",
            self.rows,
            crate::human_bytes(self.bytes as i64),
            crate::human_bytes(self.cap as i64)
        )
    }
}

/// The machine's cache, as `stats` shows it. Off, or never written, is zeros
/// with the cap in force.
///
/// Read-only and quick to give up: the portal asks every second while a run
/// writes to the cache, and opening it for writing (its journal mode, its
/// schema) waited on the run's locks for seconds at a time on Windows. A read
/// that finds it busy answers with the last figures this process read.
pub fn stats() -> Stats {
    static LAST: std::sync::Mutex<Option<Stats>> = std::sync::Mutex::new(None);
    let cap = cap_bytes();
    let off = Stats {
        cap,
        ..Stats::default()
    };
    if cap == 0 {
        return off;
    }
    let Ok(path) = path() else { return off };
    if !path.exists() {
        return off;
    }
    let read = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(anyhow::Error::from)
        .and_then(|db| {
            db.busy_timeout(std::time::Duration::from_millis(200))?;
            stats_of(&db)
        });
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    match read {
        Ok(stats) => {
            *last = Some(stats.clone());
            stats
        }
        Err(_) => last.clone().unwrap_or(off),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_moves_with_every_part_that_decides_the_vector() {
        let scope = Scope {
            model: "m@1".into(),
            chunker: 4,
            truncation: 400,
        };
        let text = Scope::text("fn main() {}");
        let base = scope.key(&text, "fp16-ane");
        assert_ne!(base, scope.key(&Scope::text("fn main() {} "), "fp16-ane"));
        assert_ne!(base, scope.key(&text, "int8-cpu"));
        let mut other = scope.clone();
        other.chunker = 5;
        assert_ne!(base, other.key(&text, "fp16-ane"));
        other = scope.clone();
        other.truncation = 512;
        assert_ne!(base, other.key(&text, "fp16-ane"));
        other = scope.clone();
        other.model = "m@2".into();
        assert_ne!(base, other.key(&text, "fp16-ane"));
        assert_eq!(base, scope.key(&text, "fp16-ane"));
    }

    #[test]
    fn vectors_round_trip_byte_for_byte_and_eviction_is_least_recently_used() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = Cache::open_at(&dir.path().join("v.db")).unwrap();
        let scope = Scope {
            model: "m".into(),
            chunker: 1,
            truncation: 1,
        };
        let v: Vec<f32> = (0..384).map(|i| (i as f32).sin()).collect();
        let keys: Vec<[u8; 32]> = (0..10)
            .map(|i| scope.key(&Scope::text(&i.to_string()), "x"))
            .collect();
        let fresh: Vec<([u8; 32], &'static str, &[f32])> =
            keys.iter().map(|k| (*k, "x", v.as_slice())).collect();
        cache.record(&fresh, &[], 10).unwrap();
        cache.record(&fresh, &[], 0).unwrap();
        assert_eq!(
            cache.stats().unwrap().rows,
            10,
            "a key recorded twice is one row"
        );
        let got = cache.get(&keys[3]).unwrap();
        assert_eq!(
            got.iter().map(|f| f.to_bits()).collect::<Vec<_>>(),
            v.iter().map(|f| f.to_bits()).collect::<Vec<_>>()
        );
        // Make key 0 the most recently used, then shrink the cap to 5 rows:
        // 90 % of it keeps 4, and key 0 is one of them.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        cache.record(&[], &[keys[0]], 1).unwrap();
        cache.evict(5 * ROW_BYTES).unwrap();
        let stats = cache.stats().unwrap();
        assert_eq!(stats.rows, 4);
        assert!(cache.get(&keys[0]).is_some());
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.lookups, 11);
    }

    /// The cap is counted in rows at `ROW_BYTES`; the file has to be about
    /// that big, not a multiple of it.
    #[test]
    fn a_row_takes_about_what_the_cap_counts_it_at() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v.db");
        let mut cache = Cache::open_at(&path).unwrap();
        let scope = Scope {
            model: "m".into(),
            chunker: 1,
            truncation: 1,
        };
        let v: Vec<f32> = (0..384).map(|i| (i as f32).cos()).collect();
        let keys: Vec<[u8; 32]> = (0..2_000)
            .map(|i| scope.key(&Scope::text(&i.to_string()), "fp16-ane"))
            .collect();
        let fresh: Vec<([u8; 32], &'static str, &[f32])> = keys
            .iter()
            .map(|k| (*k, "fp16-ane", v.as_slice()))
            .collect();
        cache.record(&fresh, &[], 0).unwrap();
        cache
            .db
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        let per_row = std::fs::metadata(&path).unwrap().len() / keys.len() as u64;
        assert!(
            per_row <= ROW_BYTES * 6 / 5,
            "{per_row} bytes a row on disk against {ROW_BYTES} counted"
        );
    }
}
