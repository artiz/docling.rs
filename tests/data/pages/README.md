# Apple Pages conformance corpus (#318, #383)

Upstream docling's `tests/data/pages/` mirrored as-is: the four fixtures and
the groundtruth docling commits for them (docling#4062 — `.md` from
`export_to_markdown()`, `.json` from `export_to_dict()`). Pinned by
`crates/docling/tests/iwork.rs`: the Markdown must match byte-for-byte (modulo
the final newline upstream's harness strips) and the JSON structure is
cross-checked (body order, body texts, tables, notes-layer comments and their
back-refs). Never regenerated here — it is upstream's output; refresh it from
upstream when docling's reader changes.

| File | What it pins |
|---|---|
| `pages_2013.pages` (Tika `testPages2013.pages`) | Pages 5+ `Index/*.iwa`: styles → labels, the table placed in the text flow, a floating text box |
| `pages_iwork09.pages` (Tika `testPages.pages`) | The same document saved by iWork '09 (`index.xml`): body paragraphs, `sf:ghost-text` skipped, the `sf:tabular-model` grid |
| `pages_iwork09_comments.pages` | iWork '09 reviewer comments: notes-layer texts with `[author]: text`, `comments` back-refs on the annotated paragraphs |
| `pages_iwork09_formatted.pages` | iWork '09 character styles (underline, bold), a hyperlink inside a footnote, header/footer/footnotes on the furniture layer |

The Tika files are from the [Apache Tika](https://github.com/apache/tika) test
corpus (`tika-parser-apple-module`, Apache License 2.0); all four came through
upstream docling's repository.
