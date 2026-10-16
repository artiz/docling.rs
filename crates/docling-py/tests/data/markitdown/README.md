# MarkItDown test files

Copied unchanged from Microsoft MarkItDown's test suite
(`packages/markitdown/tests/test_files/`, microsoft/markitdown@4cc9fa1),
MIT-licensed — see [`LICENSE`](./LICENSE). `tests/test_markitdown.py` runs
MarkItDown's own content checks for these files against the
`docling_rs.markitdown` facade (#674). `test_files.zip` (its ZIP converter's
fixture: `test.docx`, `test.jpg`, `test.pptx`, `test.xlsx` and three HTML
pages) also drives the CLI's `docling-rs bundle.zip` acceptance (#677,
`crates/docling-cli/tests/cli.rs`).
