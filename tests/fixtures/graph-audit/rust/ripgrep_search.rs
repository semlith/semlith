// Cut down from ripgrep crates/core/search.rs. Indexed by the graph tests as
// `crates/core/search.rs`, so its module is called `search`, the name of the
// method `searcher.search(&haystack)` in main.rs calls.
pub(crate) struct SearchWorker<W> {
    printer: W,
}

impl<W: WriteColor> SearchWorker<W> {
    /// Execute a search over the given haystack.
    pub(crate) fn search(&mut self, haystack: &Haystack) -> io::Result<SearchResult> {
        self.search_path(haystack.path())
    }
}
