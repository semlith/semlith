//! The vector side of a store, read from disk only when something needs it.
//!
//! A store's quantized vectors are the largest thing it owns, and turbovec
//! keeps two copies of them resident once a search has warmed the index: the
//! packed codes it loaded, and the repacked block layout its SIMD kernel scans.
//! That cost is worth paying to answer a query and worth nothing at all to
//! answer `stats`, list files, or sit in an agent's MCP server waiting to be
//! asked something — which is what an opened store used to pay it for.
//!
//! So nothing here touches the disk until a caller asks for something only the
//! vectors can answer. Opening a store is SQLite and a path.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use turbovec::IdMapIndex;

/// What the vector index is allowed to look at for one query.
pub enum Allowlist {
    /// No filter, or one that selects everything the index holds.
    All,
    /// The filter selects nothing. The query is over before it starts.
    Empty,
    Subset(Vec<u64>),
}

impl Allowlist {
    /// `None` is turbovec's "search everything".
    fn as_slice(&self) -> Option<&[u64]> {
        match self {
            Allowlist::Subset(ids) => Some(ids),
            _ => None,
        }
    }
}

/// A store's vectors: a file on disk, and whatever of it is currently resident.
pub struct VectorIndex {
    path: PathBuf,
    dim: usize,
    bit_width: usize,
    /// `None` until a caller needs the vectors themselves.
    resident: Option<IdMapIndex>,
}

impl VectorIndex {
    /// Name the index without reading it. No I/O happens here, which is the
    /// whole point.
    pub fn open(dir: &Path, dim: usize, bit_width: usize) -> Self {
        Self {
            path: dir.join("index.tv"),
            dim,
            bit_width,
            resident: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// True once the vectors are in memory. Callers use this to answer cheaply
    /// from the resident index when it happens to be there, and from SQLite
    /// when it is not.
    pub fn is_resident(&self) -> bool {
        self.resident.is_some()
    }

    /// How many vectors are resident, or `None` when nothing is loaded.
    pub fn resident_len(&self) -> Option<usize> {
        self.resident.as_ref().map(|i| i.len())
    }

    /// Forget what is loaded, so the next use reads the file again.
    ///
    /// This is how a long-lived reader picks up a writer's new index: the
    /// reload costs nothing until the reader is actually asked something.
    pub fn evict(&mut self) {
        self.resident = None;
    }

    /// The resident index, reading the file on first use.
    fn get(&mut self) -> Result<&mut IdMapIndex> {
        if self.resident.is_none() {
            let loaded = if self.path.exists() {
                let idx = IdMapIndex::load(&self.path)
                    .with_context(|| format!("loading {}", self.path.display()))?;
                if let Some(d) = idx.dim_opt()
                    && d != self.dim
                {
                    bail!(
                        "index is {d}-dimensional but the store's model produces {}; \
                         store is corrupt",
                        self.dim
                    );
                }
                idx
            } else {
                IdMapIndex::new(self.dim, self.bit_width).map_err(|e| anyhow::anyhow!("{e:?}"))?
            };
            self.resident = Some(loaded);
        }
        Ok(self.resident.as_mut().unwrap())
    }

    /// Load the vectors and pay turbovec's repack cost now, so the first query
    /// is not slower than the rest.
    pub fn prepare(&mut self) -> Result<()> {
        self.get()?.prepare();
        Ok(())
    }

    pub fn add(&mut self, vectors: &[f32], ids: &[u64]) -> Result<()> {
        self.get()?
            .add_with_ids(vectors, ids)
            .map_err(|e| anyhow::anyhow!("{e:?}"))
    }

    pub fn remove(&mut self, id: u64) -> Result<()> {
        self.get()?.remove(id);
        Ok(())
    }

    pub fn contains(&mut self, id: u64) -> Result<bool> {
        Ok(self.get()?.contains(id))
    }

    /// How many vectors the index holds, reading it if that is the only way
    /// to know.
    pub fn count(&mut self) -> Result<usize> {
        Ok(self.get()?.len())
    }

    /// `(scores, ids)` for the best `depth` vectors `allowlist` permits.
    pub fn search(
        &mut self,
        vector: &[f32],
        depth: usize,
        allowlist: &Allowlist,
    ) -> Result<(Vec<f32>, Vec<u64>)> {
        let index = self.get()?;
        if index.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        Ok(index.search_with_allowlist(vector, depth, allowlist.as_slice()))
    }

    /// Write the index out via a temp file and a rename, so an interrupted save
    /// cannot leave a half-written index behind.
    pub fn save(&mut self) -> Result<()> {
        let tmp = self.path.with_extension("tv.tmp");
        self.get()?
            .write(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Remove the leavings of a run killed mid-save. The caller holds the store
    /// lock, so there is no live writer whose half-written index this could be.
    pub fn clean(&self) {
        let _ = std::fs::remove_file(self.path.with_extension("tv.tmp"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opening must not read the file. Proved with a file that cannot be read:
    /// if `open` touched it, `open` would be the thing that failed.
    #[test]
    fn opening_reads_nothing() {
        let dir = std::env::temp_dir().join(format!("semlith-index-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.tv"), b"not a turbovec file at all").unwrap();

        let mut index = VectorIndex::open(&dir, 384, 4);
        assert!(index.exists(), "the file is there to be read");
        assert!(!index.is_resident(), "opening loaded it anyway");
        assert_eq!(index.resident_len(), None);

        // And the read, when it finally happens, is the thing that fails.
        assert!(
            index.count().is_err(),
            "a file that is not an index was accepted as one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A store with no index yet answers a search rather than failing, and
    /// still holds nothing until asked.
    #[test]
    fn an_absent_index_is_an_empty_one() {
        let dir = std::env::temp_dir().join(format!("semlith-index-b-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut index = VectorIndex::open(&dir, 8, 4);
        assert!(!index.exists());
        assert!(!index.is_resident());

        let (scores, ids) = index.search(&[0.0; 8], 5, &Allowlist::All).unwrap();
        assert!(scores.is_empty() && ids.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
