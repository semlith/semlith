//! The vector side of a store: read from disk only when something needs it,
//! and in pieces small enough to put down again.
//!
//! A store's quantized vectors are the largest thing it owns, and turbovec
//! keeps two copies of them resident once a search has warmed the index: the
//! packed codes it loaded, and the repacked block layout its SIMD kernel scans.
//! That cost is worth paying to answer a query and worth nothing at all to
//! answer `stats`, list files, or sit in an agent's MCP server waiting to be
//! asked something. So nothing here touches the disk until a caller asks for
//! something only the vectors can answer.
//!
//! # Two layouts
//!
//! [`Single`] is one `index.tv` holding everything, which is what every store
//! written before 0.7.0 has and keeps. Opening one is unchanged, searching one
//! is unchanged, and this release does not migrate it: the vectors in it are
//! quantized and cannot be split back out, so the only honest way onto the new
//! layout is to index again.
//!
//! [`Sharded`] is a directory of fixed-size shards, which is what a store
//! created by 0.7.0 gets, and what `format_version` 2 records. Three things
//! follow from it and none of them are possible with one file:
//!
//! - Searching holds a bounded number of shards, not the whole corpus.
//! - Changing one file rewrites one shard, not the entire index — which is what
//!   `semlith watch` does on every save of every watched file.
//! - A long index run can make its work durable as it goes, so an interruption
//!   costs the last shard rather than the whole run.
//!
//! # The shard directory is the manifest
//!
//! A shard is named for the first chunk id it holds — `index/0000000000000001.tvim`
//! — zero-padded so that sorted by name is sorted by id. Reading the directory
//! is therefore reading the map: shard boundaries are the names, and the shard
//! an id belongs to is the last one whose name does not exceed it. Nothing else
//! records the layout, so nothing else can disagree with it.
//!
//! This works because chunk ids only ever ascend. `chunks.id` is
//! `AUTOINCREMENT` precisely so that SQLite cannot hand a deleted row's id to a
//! new chunk, which would put one id inside two shards' ranges at once.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use turbovec::IdMapIndex;

/// Vectors per shard.
///
/// A shard is the unit of everything here: what a search loads, what a save
/// rewrites, and what an interrupted index run keeps. 65536 vectors of a
/// 384-dimensional model at 4 bits is around 12 MB of packed codes, doubled by
/// turbovec's repacked search copy — small enough that a handful fit in a
/// modest budget, large enough that a corpus of a few hundred thousand chunks
/// is a handful and not a thousand.
pub const SHARD_VECTORS: usize = 65536;

/// Override for [`SHARD_VECTORS`].
///
/// This exists so the tests can build a multi-shard store without embedding
/// hundreds of thousands of chunks, and so the size can be measured rather than
/// argued about. It is not part of the documented environment.
const SHARD_VECTORS_ENV: &str = "SEMLITH_SHARD_VECTORS";

/// Where a sharded store keeps its shards, under the store directory.
const SHARD_DIR: &str = "index";

const SHARD_EXT: &str = "tvim";

fn shard_capacity() -> usize {
    std::env::var(SHARD_VECTORS_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(SHARD_VECTORS)
}

/// What the vector index is allowed to look at for one query.
pub enum Allowlist {
    /// No filter, or one that selects everything the index holds.
    All,
    /// The filter selects nothing. The query is over before it starts.
    Empty,
    Subset(Vec<u64>),
}

/// A store's vectors, in whichever layout the store was written with.
pub enum VectorIndex {
    /// One file. Every store written before 0.7.0. Boxed because a whole
    /// turbovec index sits inside it and the sharded variant is a handful of
    /// pointers.
    Single(Box<Single>),
    /// A directory of shards. Every store created by 0.7.0 and later.
    Sharded(Sharded),
}

impl VectorIndex {
    /// Name the index without reading it. The only I/O is listing the shard
    /// directory, which is what tells the two layouts apart.
    ///
    /// What is on disk decides, not what the store says about itself: shards
    /// present means shards, an `index.tv` means one file, and `prefer_sharded`
    /// — the store's `format_version` — only settles the case where a store has
    /// no vectors yet and one of the two is about to be written. The format key
    /// is how a store warns an older binary off; the directory is how this one
    /// knows what it is reading.
    pub fn open(dir: &Path, dim: usize, bit_width: usize, prefer_sharded: bool) -> Result<Self> {
        let sharded = Sharded::open(dir, dim, bit_width)?;
        if !sharded.shards.is_empty() {
            return Ok(VectorIndex::Sharded(sharded));
        }
        let single = Single::open(dir, dim, bit_width);
        if single.path.exists() || !prefer_sharded {
            return Ok(VectorIndex::Single(Box::new(single)));
        }
        Ok(VectorIndex::Sharded(sharded))
    }

    /// Is there an index on disk at all?
    pub fn exists(&self) -> bool {
        match self {
            VectorIndex::Single(s) => s.path.exists(),
            VectorIndex::Sharded(s) => !s.shards.is_empty(),
        }
    }

    /// True while any vectors are in memory.
    pub fn is_resident(&self) -> bool {
        match self {
            VectorIndex::Single(s) => s.resident.is_some(),
            VectorIndex::Sharded(s) => s.shards.iter().any(|s| s.resident.is_some()),
        }
    }

    /// How many vectors are resident, or `None` when nothing is loaded.
    ///
    /// A partially loaded sharded index answers `None` rather than a number
    /// covering some of itself: a count that means "the shards I happen to have
    /// read" is worse than no count, and the caller has SQLite for the honest
    /// one.
    pub fn resident_len(&self) -> Option<usize> {
        match self {
            VectorIndex::Single(s) => s.resident.as_ref().map(|i| i.len()),
            VectorIndex::Sharded(s) => s.fully_resident_len(),
        }
    }

    /// Forget what is loaded, so the next use reads from disk again.
    pub fn evict(&mut self) {
        match self {
            VectorIndex::Single(s) => s.resident = None,
            VectorIndex::Sharded(s) => s.evict_all(),
        }
    }

    /// Load and pay turbovec's repack cost now, so the first query is not
    /// slower than the rest. A sharded index warms only what its budget allows.
    pub fn prepare(&mut self) -> Result<()> {
        match self {
            VectorIndex::Single(s) => {
                s.get()?.prepare();
            }
            VectorIndex::Sharded(s) => s.prepare()?,
        }
        Ok(())
    }

    pub fn add(&mut self, vectors: &[f32], ids: &[u64]) -> Result<()> {
        match self {
            VectorIndex::Single(s) => s
                .get()?
                .add_with_ids(vectors, ids)
                .map_err(|e| anyhow::anyhow!("{e:?}")),
            VectorIndex::Sharded(s) => s.add(vectors, ids),
        }
    }

    pub fn remove(&mut self, id: u64) -> Result<()> {
        match self {
            VectorIndex::Single(s) => {
                s.get()?.remove(id);
            }
            VectorIndex::Sharded(s) => s.remove(id)?,
        }
        Ok(())
    }

    pub fn contains(&mut self, id: u64) -> Result<bool> {
        match self {
            VectorIndex::Single(s) => Ok(s.get()?.contains(id)),
            VectorIndex::Sharded(s) => s.contains(id),
        }
    }

    /// `(scores, ids)` for the best `depth` vectors `allowlist` permits.
    pub fn search(
        &mut self,
        vector: &[f32],
        depth: usize,
        allowlist: &Allowlist,
    ) -> Result<(Vec<f32>, Vec<u64>)> {
        if matches!(allowlist, Allowlist::Empty) || depth == 0 {
            return Ok((Vec::new(), Vec::new()));
        }
        match self {
            VectorIndex::Single(s) => {
                let index = s.get()?;
                if index.is_empty() {
                    return Ok((Vec::new(), Vec::new()));
                }
                let list = match allowlist {
                    Allowlist::Subset(ids) => Some(ids.as_slice()),
                    _ => None,
                };
                Ok(index.search_with_allowlist(vector, depth, list))
            }
            VectorIndex::Sharded(s) => s.search(vector, depth, allowlist),
        }
    }

    /// Make the index durable. Only what changed is written.
    pub fn save(&mut self) -> Result<()> {
        match self {
            VectorIndex::Single(s) => s.save(),
            VectorIndex::Sharded(s) => s.save(),
        }
    }

    /// How many shards the store holds. One, for a single-file index.
    pub fn shards(&self) -> usize {
        match self {
            VectorIndex::Single(s) => usize::from(s.path.exists()),
            VectorIndex::Sharded(s) => s.shards.len(),
        }
    }

    /// Remove the leavings of a run killed mid-save. The caller holds the store
    /// lock, so there is no live writer whose half-written index this could be.
    pub fn clean(&self) {
        match self {
            VectorIndex::Single(s) => {
                let _ = std::fs::remove_file(s.path.with_extension("tv.tmp"));
            }
            VectorIndex::Sharded(s) => s.clean(),
        }
    }
}

/// One `index.tv` holding every vector — the layout of every store written
/// before 0.7.0, kept exactly as it was.
pub struct Single {
    path: PathBuf,
    dim: usize,
    bit_width: usize,
    resident: Option<IdMapIndex>,
}

impl Single {
    fn open(dir: &Path, dim: usize, bit_width: usize) -> Self {
        Self {
            path: dir.join("index.tv"),
            dim,
            bit_width,
            resident: None,
        }
    }

    /// The resident index, reading the file on first use.
    fn get(&mut self) -> Result<&mut IdMapIndex> {
        if self.resident.is_none() {
            self.resident = Some(load_or_new(&self.path, self.dim, self.bit_width)?);
        }
        Ok(self.resident.as_mut().unwrap())
    }

    fn save(&mut self) -> Result<()> {
        let tmp = self.path.with_extension("tv.tmp");
        let path = self.path.clone();
        self.get()?
            .write(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

/// One shard: a slice of the id space, and whatever of it is resident.
struct Shard {
    /// First chunk id this shard may hold. Its range ends where the next
    /// shard's begins; the last shard's range is open.
    start_id: u64,
    path: PathBuf,
    resident: Option<IdMapIndex>,
    /// Changed since it was last written.
    dirty: bool,
}

/// A directory of shards — the layout of a store created by 0.7.0.
pub struct Sharded {
    dir: PathBuf,
    dim: usize,
    bit_width: usize,
    capacity: usize,
    /// Ascending by `start_id`, which is also ascending by file name.
    shards: Vec<Shard>,
}

impl Sharded {
    fn open(store_dir: &Path, dim: usize, bit_width: usize) -> Result<Self> {
        let dir = store_dir.join(SHARD_DIR);
        let mut shards = Vec::new();
        if dir.is_dir() {
            for entry in std::fs::read_dir(&dir)
                .with_context(|| format!("reading {}", dir.display()))?
                .flatten()
            {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some(SHARD_EXT) {
                    continue;
                }
                let Some(start_id) = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse::<u64>().ok())
                else {
                    // Not one of ours. Leaving it alone is the only safe thing
                    // to do with a file we cannot account for.
                    continue;
                };
                shards.push(Shard {
                    start_id,
                    path,
                    resident: None,
                    dirty: false,
                });
            }
        }
        shards.sort_by_key(|s| s.start_id);
        Ok(Self {
            dir,
            dim,
            bit_width,
            capacity: shard_capacity(),
            shards,
        })
    }

    fn shard_path(&self, start_id: u64) -> PathBuf {
        self.dir.join(format!("{start_id:016}.{SHARD_EXT}"))
    }

    /// The shard whose range covers `id`, if any shard does.
    fn locate(&self, id: u64) -> Option<usize> {
        match self.shards.partition_point(|s| s.start_id <= id) {
            0 => None,
            n => Some(n - 1),
        }
    }

    fn load(&mut self, at: usize) -> Result<&mut IdMapIndex> {
        if self.shards[at].resident.is_none() {
            let loaded = load_or_new(&self.shards[at].path, self.dim, self.bit_width)?;
            self.shards[at].resident = Some(loaded);
        }
        Ok(self.shards[at].resident.as_mut().unwrap())
    }

    fn evict_all(&mut self) {
        for shard in &mut self.shards {
            // A shard with unwritten changes is the one thing that must not be
            // dropped: its vectors exist nowhere else.
            if !shard.dirty {
                shard.resident = None;
            }
        }
    }

    /// Resident count, but only when every shard is loaded — see
    /// [`VectorIndex::resident_len`].
    fn fully_resident_len(&self) -> Option<usize> {
        if self.shards.is_empty() || self.shards.iter().any(|s| s.resident.is_none()) {
            return None;
        }
        Some(
            self.shards
                .iter()
                .map(|s| s.resident.as_ref().map_or(0, |i| i.len()))
                .sum(),
        )
    }

    fn prepare(&mut self) -> Result<()> {
        for at in 0..self.shards.len() {
            self.load(at)?.prepare();
        }
        Ok(())
    }

    /// Append vectors, opening a new shard whenever the last one is full.
    ///
    /// Ids ascend, so appending always means the last shard. A batch that
    /// straddles a boundary is split rather than allowed to overflow: the
    /// capacity is what every memory claim downstream rests on.
    fn add(&mut self, vectors: &[f32], ids: &[u64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let mut offset = 0;
        while offset < ids.len() {
            let at = self.open_shard(ids[offset])?;
            let room = self.capacity.saturating_sub(self.load(at)?.len());
            let take = room.min(ids.len() - offset);
            let dim = self.dim;
            self.load(at)?
                .add_with_ids(
                    &vectors[offset * dim..(offset + take) * dim],
                    &ids[offset..offset + take],
                )
                .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            self.shards[at].dirty = true;
            offset += take;
        }
        Ok(())
    }

    /// The shard the next id goes into, creating one when the last is full or
    /// there is none yet.
    fn open_shard(&mut self, next_id: u64) -> Result<usize> {
        if let Some(last) = self.shards.len().checked_sub(1)
            && self.load(last)?.len() < self.capacity
        {
            return Ok(last);
        }
        let path = self.shard_path(next_id);
        self.shards.push(Shard {
            start_id: next_id,
            path,
            resident: Some(
                IdMapIndex::new(self.dim, self.bit_width).map_err(|e| anyhow::anyhow!("{e:?}"))?,
            ),
            dirty: true,
        });
        Ok(self.shards.len() - 1)
    }

    fn remove(&mut self, id: u64) -> Result<()> {
        let Some(at) = self.locate(id) else {
            return Ok(());
        };
        if self.load(at)?.remove(id) {
            self.shards[at].dirty = true;
        }
        Ok(())
    }

    fn contains(&mut self, id: u64) -> Result<bool> {
        match self.locate(id) {
            Some(at) => Ok(self.load(at)?.contains(id)),
            None => Ok(false),
        }
    }

    /// Search every shard and merge the results into one ranking.
    ///
    /// The quantizer is data-oblivious, so a score from one shard means what it
    /// means in another and the merge is a sort rather than a normalisation.
    fn search(
        &mut self,
        vector: &[f32],
        depth: usize,
        allowlist: &Allowlist,
    ) -> Result<(Vec<f32>, Vec<u64>)> {
        let mut merged: Vec<(f32, u64)> = Vec::new();
        for at in 0..self.shards.len() {
            let start = self.shards[at].start_id;
            let end = self.shards.get(at + 1).map(|s| s.start_id);
            // Narrow the allowlist to this shard before loading it: a shard no
            // permitted id falls inside is a shard the query never reads.
            let subset = match allowlist {
                Allowlist::Empty => return Ok((Vec::new(), Vec::new())),
                Allowlist::All => None,
                Allowlist::Subset(ids) => {
                    let mine: Vec<u64> = ids
                        .iter()
                        .copied()
                        .filter(|id| *id >= start && end.is_none_or(|e| *id < e))
                        .collect();
                    if mine.is_empty() {
                        continue;
                    }
                    Some(mine)
                }
            };

            let index = self.load(at)?;
            if index.is_empty() {
                continue;
            }
            // turbovec panics on an id the index does not hold, and a shard can
            // be missing an id its range covers — a chunk deleted since, or one
            // whose vectors a killed run never made durable.
            let subset = subset.map(|ids| {
                ids.into_iter()
                    .filter(|id| index.contains(*id))
                    .collect::<Vec<u64>>()
            });
            if subset.as_ref().is_some_and(|ids| ids.is_empty()) {
                continue;
            }
            let (scores, ids) = index.search_with_allowlist(vector, depth, subset.as_deref());
            merged.extend(scores.into_iter().zip(ids));
        }

        merged.sort_by(|a, b| b.0.total_cmp(&a.0));
        merged.truncate(depth);
        Ok(merged.into_iter().unzip())
    }

    fn save(&mut self) -> Result<()> {
        if !self.shards.iter().any(|s| s.dirty) {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating {}", self.dir.display()))?;
        for shard in &mut self.shards {
            if !shard.dirty {
                continue;
            }
            let Some(index) = shard.resident.as_ref() else {
                continue;
            };
            let tmp = shard.path.with_extension("tmp");
            index
                .write(&tmp)
                .with_context(|| format!("writing {}", tmp.display()))?;
            std::fs::rename(&tmp, &shard.path)?;
            shard.dirty = false;
        }
        Ok(())
    }

    fn clean(&self) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.path().extension().and_then(|e| e.to_str()) == Some("tmp") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Read an index file, or make an empty one when there is nothing there yet.
fn load_or_new(path: &Path, dim: usize, bit_width: usize) -> Result<IdMapIndex> {
    if !path.exists() {
        return IdMapIndex::new(dim, bit_width).map_err(|e| anyhow::anyhow!("{e:?}"));
    }
    let index = IdMapIndex::load(path).with_context(|| format!("loading {}", path.display()))?;
    if let Some(d) = index.dim_opt()
        && d != dim
    {
        bail!("index is {d}-dimensional but the store's model produces {dim}; store is corrupt");
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("semlith-index-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Opening must not read the file. Proved with a file that cannot be read:
    /// if `open` touched it, `open` would be the thing that failed.
    #[test]
    fn opening_reads_nothing() {
        let dir = scratch("lazy");
        std::fs::write(dir.join("index.tv"), b"not a turbovec file at all").unwrap();

        let mut index = VectorIndex::open(&dir, 384, 4, false).unwrap();
        assert!(index.exists(), "the file is there to be read");
        assert!(!index.is_resident(), "opening loaded it anyway");
        assert_eq!(index.resident_len(), None);

        // And the read, when it finally happens, is the thing that fails.
        assert!(
            index.contains(1).is_err(),
            "a file that is not an index was accepted as one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A store with no index yet answers a search rather than failing.
    #[test]
    fn an_absent_index_is_an_empty_one() {
        let dir = scratch("absent");
        let mut index = VectorIndex::open(&dir, 8, 4, false).unwrap();
        assert!(!index.exists());

        let (scores, ids) = index.search(&[0.0; 8], 5, &Allowlist::All).unwrap();
        assert!(scores.is_empty() && ids.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Unit vectors pointing at one axis each, so the nearest neighbour of axis
    /// `i` is the vector on axis `i` and the ranking is decidable by hand.
    fn axis(dim: usize, i: usize) -> Vec<f32> {
        let mut v = vec![0.0; dim];
        v[i % dim] = 1.0;
        v
    }

    /// The release's structural claim, at a size a test can hold: vectors land
    /// in shards of a fixed size, the shards are files named for their first
    /// id, and a search over them finds what a search over one file would.
    #[test]
    fn vectors_land_in_shards_and_the_shards_are_searchable() {
        let dir = scratch("shards");
        let dim = 8;
        let mut index = VectorIndex::open(&dir, dim, 4, true).unwrap();
        let VectorIndex::Sharded(ref mut sharded) = index else {
            panic!("asked for shards, got one file")
        };
        sharded.capacity = 4;

        // Ten vectors, added in batches of three, so batches straddle shard
        // boundaries the way a real embedding batch does.
        for batch in 0..4 {
            let ids: Vec<u64> = (0..3)
                .map(|i| batch * 3 + i + 1)
                .filter(|id| *id <= 10)
                .collect();
            if ids.is_empty() {
                continue;
            }
            let flat: Vec<f32> = ids
                .iter()
                .flat_map(|id| axis(dim, *id as usize - 1))
                .collect();
            index.add(&flat, &ids).unwrap();
        }
        index.save().unwrap();

        assert_eq!(index.shards(), 3, "10 vectors at 4 per shard is 3 shards");
        let names: Vec<String> = std::fs::read_dir(dir.join(SHARD_DIR))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(SHARD_EXT))
            .collect();
        assert!(
            names.contains(&format!("{:016}.{SHARD_EXT}", 1u64)),
            "a shard is named for its first id: {names:?}"
        );

        // Reopened from the directory alone — nothing else records the layout.
        let mut reopened = VectorIndex::open(&dir, dim, 4, true).unwrap();
        assert_eq!(reopened.shards(), 3);
        for id in 1..=10u64 {
            assert!(reopened.contains(id).unwrap(), "id {id} went missing");
            let (_, hits) = reopened
                .search(&axis(dim, id as usize - 1), 1, &Allowlist::All)
                .unwrap();
            // Axes repeat every `dim`, so the best hit is one of the ids on
            // that axis rather than a specific one.
            assert!(
                hits.iter()
                    .all(|h| (h - 1) % dim as u64 == (id - 1) % dim as u64),
                "id {id} ranked {hits:?}, which is not on its axis"
            );
        }

        // A filter narrows to one shard's worth without reading past it.
        let (_, hits) = reopened
            .search(&axis(dim, 0), 5, &Allowlist::Subset(vec![9]))
            .unwrap();
        assert_eq!(hits, vec![9], "an allowlist of one returned {hits:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Removing a vector must touch its shard and no other file on disk.
    #[test]
    fn removing_rewrites_only_the_shard_holding_it() {
        let dir = scratch("dirty");
        let dim = 8;
        let mut index = VectorIndex::open(&dir, dim, 4, true).unwrap();
        let VectorIndex::Sharded(ref mut sharded) = index else {
            panic!("asked for shards")
        };
        sharded.capacity = 4;

        let ids: Vec<u64> = (1..=8).collect();
        let flat: Vec<f32> = ids
            .iter()
            .flat_map(|id| axis(dim, *id as usize - 1))
            .collect();
        index.add(&flat, &ids).unwrap();
        index.save().unwrap();
        assert_eq!(index.shards(), 2);

        let stamp = |name: u64| {
            std::fs::metadata(dir.join(SHARD_DIR).join(format!("{name:016}.{SHARD_EXT}")))
                .unwrap()
                .modified()
                .unwrap()
        };
        let first_before = stamp(1);
        let second_before = stamp(5);

        // Slept, because a rewrite within the same filesystem timestamp tick
        // would be indistinguishable from no rewrite at all.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let mut reopened = VectorIndex::open(&dir, dim, 4, true).unwrap();
        reopened.remove(6).unwrap();
        reopened.save().unwrap();

        assert_eq!(stamp(1), first_before, "an untouched shard was rewritten");
        assert!(
            stamp(5) > second_before,
            "the touched shard was not written"
        );
        assert!(!reopened.contains(6).unwrap());
        assert!(reopened.contains(5).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
