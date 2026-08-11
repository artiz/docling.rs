# Apple iWork fixtures (#213)

Modern (IWA, 2013+) iWork documents for the `IworkBackend` text extraction,
pinned by `crates/docling/tests/iwork.rs` against `groundtruth/`.

Provenance (test corpora of the projects whose reverse engineering the
backend follows):

| File | Origin |
|---|---|
| `proposal_simple.pages` (`pages5-file.pages`), `proposal_nested_dir.pages` (`pages5-extra-dir.pages`) | [libetonyek](https://github.com/LibreOffice/libetonyek) test data (MPL 2.0) |
| `two_sheets.numbers` (`test-1.numbers`), `account_statement.numbers` (`test-2.numbers`) | [numbers-parser](https://github.com/masaccio/numbers-parser) test data (MIT) |
| `one_slide.key` (`simple-oneslide.key`), `numeric_table.key` (`table.key`) | [keynote-parser](https://github.com/psobot/keynote-parser) test data (MIT) |

`numeric_table.key` pins the current v1 limitation on purpose: its table is
numeric-only, and numeric cell values live in the tile storage (not the
shared string table), so only the table name extracts until tile decoding
lands.
