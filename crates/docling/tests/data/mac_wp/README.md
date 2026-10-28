# Classic Macintosh word-processor fixtures (#216)

All from LibreOffice's `writerperfect/qa/unit/data/writer/libmwaw/pass/`
(MPL-2.0), the libmwaw test corpus, given extensions.

| File | Origin | What it pins |
| --- | --- | --- |
| `sources/libreoffice_macwrite_4.5.mw` | `MacWrite_4.5` | MacWrite version 6: nibble-compressed paragraphs, bold runs, header and footer windows as furniture |
| `sources/libreoffice_word_1.0.mcw` | `MicrosoftWord_1.0` | Word 1.0 (`FE32`): the text from 0x80, headers and footers inline (1.0 keeps them in the text run) |
| `sources/libreoffice_word_3.0.mcw` | `MicrosoftWord_3.0` | Word 3.0 (`FE34`): the text from 0x100, a footnote mark dropped |
| `sources/libreoffice_word_5.0.mcw` | `MicrosoftWord_5.0` | Word 5.0 (`FE37 0023`): a fast-saved file read through its piece table, Mac Roman accents |
| `sources/libreoffice_clarisworks_1.0.cwk` | `ClarisWorks_1.0` | ClarisWorks 1: the main text zone (two chunks) |
| `sources/libreoffice_clarisworks_6.0.cwk` | `ClarisWorks_6.0.cwk` | ClarisWorks 6: the main text, then the text box and the table cells' text zones |

Python docling has no reader for these formats, so the `expected/` outputs
are our own. Regenerate after an intentional change with `DOCLING_RS_REGEN=1
DOCLING_RS_REGRESSION_FORMATS=mac_wp cargo test -p docling --test regression`.
