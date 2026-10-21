# ODF (OpenDocument) fixtures

`mirror.txt` lists the `.odt` / `.ods` / `.odp` files taken from the
repository-root corpus; the files below are this suite's own, and
`upstream/` holds the documents `odf_upstream` checks against Python
docling's output. Each ODT reproducer has a `.docx` twin in
`../docx/sources/` with the same content, the reference for what the
Markdown should read like. Regenerate after an intentional change with
`DOCLING_RS_REGEN=1 cargo test -p docling --test regression`.

| File | Origin | Pins |
| --- | --- | --- |
| `sources/footnotes.odt` | own (#466) | a `text:note`'s citation and body stay out of the citing sentence; the body is a furniture `footnote` |
| `sources/odt_toc_index_body.odt` | `min_odt_toc_with_table.odt` attached to #823 (synthetic ODF 1.2; the twin is `../docx/sources/odt_toc_index_body.docx`) | a `<text:table-of-content>` is transparent: its index title, its entries and the heading, paragraph and table LibreOffice left inside the `<text:index-body>` walk as body blocks, between the surrounding paragraphs — the twin's Markdown byte for byte; the `*-source` template is not content |
