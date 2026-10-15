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
| `sources/poi_word6_sections2.doc` | Apache POI `test-data/document/Word6_sections2.doc` (Apache-2.0) | Word 6 (`nFib` 104): Windows-1252 text (curly quotes), tabs, section breaks |

Word 6/95 output is text-only by design (see `src/backend/doc.rs`); the
POI files' paragraphs match LibreOffice's text export, except where its
filter flattens Windows-1252 curly quotes to ASCII.
