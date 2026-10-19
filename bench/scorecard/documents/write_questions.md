# Writing document-format questions from sampled passages

You write retrieval questions for a private benchmark that compares search tools on documents of many formats (PDF, Word, PowerPoint, Excel, OpenDocument, EPUB, RTF, Jupyter notebooks, mailing-list archives, CSV). Each question has a known answer: the passage it was written from. Tools are scored on whether they return that passage, so the question must be fair and must have one clear answer.

## Inputs

`~/semlith-bench/accuracy/formats/targets.json` is a list of passages, each with `id`, `path` (relative to `~/semlith-bench/docs-formats/files/`, e.g. `pdf/nist-csf-2-0.pdf`), `start_line`, `end_line` and `text` (the text extracted from the document). The documents are in `~/semlith-bench/docs-formats/files/`. Most are binary formats, so read the passage from `text`, and use `rga` (ripgrep-all, installed: it searches inside PDF, docx, odt, epub, ipynb and mail) or `rg` for plain-text formats to check uniqueness. You are given a range of ids to handle.

## Output

Write exactly one file, the one your brief names (`~/semlith-bench/accuracy/docs/generated/<part>.yaml`; create `generated/` if absent), and touch nothing else:

```yaml
questions:
  - id: fmt-007
    target: fmt-007
    path: pdf/nist-csf-2-0.pdf   # the target's path, unchanged
    shape: topic            # topic | paraphrase
    query: "..."
    note: one sentence on what the passage says that answers the query
```

One question per passage. The shape is fixed by the id: an **odd** number (fmt-001, fmt-003, …) is `topic`, an **even** number is `paraphrase`.

## The two shapes

**topic** asks the way a reader with a term in mind would search the docs: "Where do the docs explain how to configure a reverse proxy timeout?", or "What does the manual say about streaming decompression buffer sizes?". It may use the passage's heading words and key terms.

**paraphrase** asks the way a reader who doesn't know the vocabulary would ask: they describe the problem or the behaviour in their own words. It must NOT use any 4+ letter content word from the passage's first non-empty line (usually its heading), and must not copy four words in a row from the passage. Example: for a passage headed "Connection pooling", don't ask "how does connection pooling work"; ask "how can I avoid opening a new database link for every request?".

## When to skip a passage

Skip it (write nothing for that id) when:
- it is not prose a person would read for information: a table of contents, an index, references or a bibliography, page headers and footers, a list of numbers, legal boilerplate, mail headers with no message body, or a code cell with no explanation;
- you cannot write a question that this passage answers and that no other file in the same repository answers about as well. Check with `rga -i -F '<two or three key terms>' ~/semlith-bench/docs-formats/files/` and against the other passages in targets.json; if another place in the same or another document answers it equally (a repeated table header, a summary that restates the passage, a reply that quotes an earlier mail), skip.

Expect to skip some; quality matters more than count.

## Checks before you write the file

- `path` is exactly the target's path.
- Queries are plain English, 8 to 30 words, with no file paths, no line numbers and no code blocks. They may contain a product or command name where a reader would naturally use one.
- Re-read every paraphrase query against the passage's first line and remove any word of 4+ letters that appears in both.
- No two queries are near-duplicates.

## Constraints

Do not run semlith or any indexing, and do not use any semlith MCP tool: the benchmark measures semlith, so the questions must not be shaped by it. Use only `rg`, Read and your own judgement. Do not modify anything outside your one output file. At the end, report the ids you wrote and the ids you skipped with a short reason each.

## Spreadsheets and CSV

For `.xlsx`, `.ods` and `.csv` passages the text is cells joined row by row. Ask about the content of one row or a small group of rows, as someone looking something up would ("Which control requires an organisation to review audit logs weekly?", "Which vendor's product had the deserialization flaw added to the catalogue in March?"). A question about one row is fine as long as no other row answers it equally.
