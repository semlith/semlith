//! One writer at a time per store.
//!
//! Indexing writes SQLite rows and rewrites `index.tv`. Two runs against one
//! store interleave those writes and the index stops agreeing with the
//! database, which surfaces later as searches returning nothing for chunks that
//! plainly exist.
//!
//! The lock is an OS advisory lock on a file in the store directory, not the
//! existence of that file. That distinction matters: the kernel drops the lock
//! when the holding process dies, so a run killed with SIGKILL or lost to a
//! power cut leaves nothing to clean up. A lock file that meant "locked by
//! existing" would wedge the store until someone deleted it by hand.

use anyhow::{Context, Result, bail};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const LOCK_FILE: &str = "index.lock";

/// Held for the duration of an index run. Dropping it releases the lock.
#[derive(Debug)]
pub struct StoreLock {
    // Kept alive purely so the lock outlives `acquire`; closing the file
    // releases it.
    _file: File,
    path: PathBuf,
}

impl StoreLock {
    /// Take the store's write lock, or explain who has it.
    pub fn acquire(dir: &Path) -> Result<Self> {
        let path = dir.join(LOCK_FILE);
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;

        if file.try_lock().is_err() {
            // Whoever holds the lock wrote their identity into the file before
            // starting work. Read it back so the error names them rather than
            // saying "busy".
            let mut held_by = String::new();
            let _ = file.read_to_string(&mut held_by);
            let held_by = held_by.trim();
            let who = if held_by.is_empty() {
                "another process".to_string()
            } else {
                held_by.to_string()
            };
            bail!(
                "store {} is being indexed by {who}\n\
                 wait for it to finish, or use --store for a separate store",
                dir.display()
            );
        }

        // Record the holder only after the lock is ours, so this never
        // overwrites a live holder's line.
        file.seek(SeekFrom::Start(0))?;
        file.set_len(0)?;
        write!(file, "{}", describe_holder())?;
        file.flush()?;

        Ok(Self { _file: file, path })
    }

    /// Where the lock lives, for tests and error messages.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        // The lock is released when the file closes. The file itself is left
        // behind on purpose: recreating it every run would race with a waiting
        // process that has already opened it.
        let _ = self.path;
    }
}

fn describe_holder() -> String {
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("pid {} (started at unix {started})", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("semlith-lock-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_second_acquire_is_refused_and_names_the_holder() {
        let dir = tempdir("second");
        let first = StoreLock::acquire(&dir).unwrap();

        let err = StoreLock::acquire(&dir).unwrap_err().to_string();
        assert!(err.contains("being indexed by"), "unhelpful error: {err}");
        assert!(
            err.contains(&format!("pid {}", std::process::id())),
            "error does not name the holder: {err}"
        );

        drop(first);
    }

    #[test]
    fn releasing_lets_the_next_run_in() {
        let dir = tempdir("release");
        let first = StoreLock::acquire(&dir).unwrap();
        drop(first);

        // The lock file still exists; only the OS lock was dropped. Acquiring
        // again must succeed, or a completed run would block the next one.
        let second = StoreLock::acquire(&dir);
        assert!(second.is_ok(), "{:?}", second.err());
    }

    #[test]
    fn a_leftover_lock_file_does_not_block() {
        let dir = tempdir("stale");
        // Simulate what a process killed mid-run leaves behind: the file, with
        // a dead process's identity in it, and no OS lock.
        std::fs::write(dir.join(LOCK_FILE), "pid 999999 (started at unix 1)").unwrap();
        assert!(StoreLock::acquire(&dir).is_ok());
    }
}
