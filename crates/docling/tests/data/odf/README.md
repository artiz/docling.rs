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
| `sources/odt_style_list_name.odt` | `min_style_list_name.odt` attached to #826 (synthetic ODF 1.2; the twin is `../docx/sources/odt_style_list_name.docx`) | paragraphs numbered through their paragraph style's `style:list-style-name` — set on the style, inherited from a parent style, through an automatic style — with no `<text:list>` around them are list items: one list per run of the same list style, bullets then `1.` `2.` — the twin's Markdown byte for byte |
| `sources/odt_headings_in_list.odt` | `min_headings_in_list.odt` attached to #827 (synthetic ODF 1.2; the twin is `../docx/sources/odt_headings_in_list.docx`) | `<text:h>` headings inside `<text:list-item>`s — Writer's outline numbering — are `##` / `###` headings of their `outline-level`, outside any list, the second list continuing the first's numbering: the twin's Markdown byte for byte |
