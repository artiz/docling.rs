# DOC (Word binary) fixtures

`mirror.txt` lists the `.doc` files taken from the repository-root corpus;
the files below are this suite's own. Python docling has no native `.doc`
reader (it goes through LibreOffice), so their `expected/` outputs are ours,
checked against LibreOffice → DOCX → the DOCX backend where that path reads
the file. Regenerate after an intentional change with
`DOCLING_RS_REGEN=1 cargo test -p docling --test regression`.

| File | Origin | Pins |
| --- | --- | --- |
| `sources/embedded_word_object.doc` | `sample_object_word_with_object_image.DOC` attached to #512 (Word 97–2003, saved by Microsoft Word), with its `Macros` storage (a VBA project inherited from the author's template) removed by rewriting the compound file; every other stream, storage CLSID and the directory order — the embedded document's streams ahead of the root's — are byte-identical to the original | a Word document embedding a Word document (which itself embeds a picture object): `ObjectPool/_<id>/` carries its own `WordDocument`/`1Table`/`Data`, and the root storage's must be the ones read |
| `sources/poi_word95.doc` | Apache POI `test-data/document/Word95.doc` (Apache-2.0) | Word 95 (`nFib` 101), non-complex: the pre-97 FIB's `fcMin`/`ccpText` text path |
| `sources/poi_word6_truncated.doc` | Apache POI `test-data/document/Word6.doc` (Apache-2.0) cut by 8 bytes, attached to #521 | Word 6 (`nFib` 101) whose last CFB sector — the directory's — is short, as pre-97 writers left it: converts exactly like the padded original |
| `sources/doc_header_variants.doc` | `min_hdr_variants.doc` attached to #697 (python-docx made the `.docx`, Word COM saved the `.doc`) | a header and a footer of each kind — default, first-page, even-page — in a section without a title page and a document without facing pages: only the default pair is shown, as a furniture `page header` / `page footer` section of `text` items (the `.docx` twin's shape) |
| `sources/doc_comment.doc` | `min_comment.doc` attached to #699 (Word 97–2003, `nFib` 193; Word COM) | a comment on the body paragraph: the annotation story (`PlcfandRef` / `PlcfandTxt`, `ccpAtn`) as a notes-layer `comment_section` with docling's `[author: Author Anon (AA), time: …]: …` note (the date from `AtrdExtra`) and the `comments` back-reference on the paragraph |
| `sources/word6_lists.doc`, `sources/word6_bullets.doc` | `min_word6_lists.doc` / `min_word6_bullets.doc` attached to #700 (Word 6.0/95, anonymised corpus files; Word COM `.docx` twins) | a six-item numbered list and a four-item bulleted list inside a table cell: the `sprmPAnld` autonumbers print their markers in the cell (`1. … 6.`, `- …`) as the `.docx` twin's rich cell does |
| `sources/doc_inherited_heading_styles.doc` | `min_inherited_headings.doc` attached to #824 (Word 97–2003, Word COM; the `.docx` twin is `../docx/sources/doc_inherited_heading_styles.docx`) | custom paragraph styles based on Heading 1 / Heading 2 (`istdBase`), one two hops up: headings of the inherited level (`##`, `###`), not body text; the two-hop style is a heading here where the `.docx` backend prints docling's bold text |
| `sources/doc_huge_papx_table.doc` | `min_hugepapx_table.doc` attached to #825 (Word 97–2003; the `.docx` twin is `../docx/sources/doc_huge_papx_table.docx`) | a 3×2 table whose second row's PAPX is a `sprmPHugePapx` (`0x6646`) into the Data stream: one table and one paragraph, as the twin's, not two one-row tables around two text paragraphs. The rows are the file's own: its generator gave row 1's end mark `fInTable` without `fTtp` — an empty cell by Word's rules — so rows 1 and 2 read as one five-cell row, as LibreOffice (garbled) reads them too |
| `sources/word6_style_lists.doc` | `min_word6_style_lists.doc` attached to #828 (Word 6.0, `nFib` 101; the `.docx` twin is `../docx/sources/word6_style_lists.docx`) | a three-item bulleted list whose `sprmPAnld` sits in the "List Bullet" style's PAPX UPX, the paragraphs holding only the `istd`: list items, as the twin's, not body text |
| `sources/poi_word6_sections2.doc` | Apache POI `test-data/document/Word6_sections2.doc` (Apache-2.0) | Word 6 (`nFib` 104): Windows-1252 text (curly quotes), tabs, section breaks |

Word 6/95 output reads tables, headings, bold/italic, headers/footers, comments and autonumber lists (see `src/backend/doc.rs`); the
POI files' paragraphs match LibreOffice's text export, except where its
filter flattens Windows-1252 curly quotes to ASCII.
