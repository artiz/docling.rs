# Apple iWork fixtures (#213, #318)

iWork documents for the `IworkBackend`, pinned by `crates/docling/tests/iwork.rs`
against `groundtruth/`.

**Pages is a conformance format** (#318): upstream docling reads `.pages` since
2.121 (docling#3934, titles/headings/iWork '09 tables in 2.122, docling#4031),
so the `.pages` groundtruth is Python docling's own output — upstream commits
no groundtruth for its Pages fixtures (its tests are assertion-based), so the
files were generated with docling 2.124.0 / docling-core 2.92.0
(`export_to_markdown()`) and our output matches them byte-for-byte; the JSON
structure (labels, table cells, header flags, body order) was cross-checked the
same way. `proposal_nested_dir.pages` is the exception: upstream rejects the
nested-directory bundle layout ("does not look like a Pages document"), so its
groundtruth is ours. `pages_password_protected.pages` pins the error path
(docling's "password-protected" message); it has no groundtruth.

Numbers and Keynote remain docling.rs extensions (upstream has no reader).

Provenance:

| File | Origin |
|---|---|
| `pages_2013.pages` (`testPages2013.pages`), `pages_iwork09.pages` (`testPages.pages`), `pages_password_protected.pages` | [Apache Tika](https://github.com/apache/tika) test corpus (`tika-parser-apple-module`, Apache License 2.0), via upstream docling's `tests/data/pages/`. The first two are the same source document saved by Pages 2013 (`Index/*.iwa`) and by iWork '09 (`index.xml`) |
| `proposal_simple.pages` (`pages5-file.pages`), `proposal_nested_dir.pages` (`pages5-extra-dir.pages`) | [libetonyek](https://github.com/LibreOffice/libetonyek) test data (MPL 2.0) |
| `two_sheets.numbers` (`test-1.numbers`), `account_statement.numbers` (`test-2.numbers`) | [numbers-parser](https://github.com/masaccio/numbers-parser) test data (MIT) |
| `one_slide.key` (`simple-oneslide.key`), `numeric_table.key` (`table.key`) | [keynote-parser](https://github.com/psobot/keynote-parser) test data (MIT) |

`numeric_table.key` pins the current v1 limitation on purpose: its table is
numeric-only, and numeric cell values live in the tile storage (not the
shared string table), so only the table name extracts until tile decoding
lands.
