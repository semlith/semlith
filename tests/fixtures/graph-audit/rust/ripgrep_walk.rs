// Cut down from ripgrep crates/ignore/src/walk.rs, lib.rs and
// crates/searcher/src/line_buffer.rs: the methods the audit's G1 method calls
// were really calling or were wrongly resolved to. Indexed by the graph tests
// as `crates/ignore/src/walk.rs`.
pub struct WalkBuilder {
    paths: Vec<PathBuf>,
}

pub struct WalkParallel {
    paths: std::vec::IntoIter<PathBuf>,
}

impl WalkBuilder {
    /// Build a new `WalkParallel` iterator.
    pub fn build_parallel(&self) -> WalkParallel {
        WalkParallel {
            paths: self.paths.clone().into_iter(),
        }
    }
}

impl WalkParallel {
    /// Execute the parallel recursive directory iterator.
    pub fn run<'s, F>(self, mkf: F)
    where
        F: FnMut() -> FnVisitor<'s>,
    {
        self.visit(&mut FnBuilder { builder: mkf })
    }
}

impl Clone for Error {
    fn clone(&self) -> Error {
        Error::Partial(Vec::new())
    }
}

impl LineBuffer {
    /// Reset this buffer, such that it can be used with a new reader.
    fn clear(&mut self) {
        self.pos = 0;
    }
}

impl GramQuery {
    fn unwrap(self) -> Vec<Gram> {
        self.grams
    }
}
