# Lotus Word Pro fixtures (#216)

From LibreOffice's `lotuswordpro/qa/cppunit/data/` (MPL-2.0).

| File | Origin | What it pins |
| --- | --- | --- |
| `sources/libreoffice_wordpro.lwp` | `pass/wordpro.lwp` | the body story's paragraphs in order, a page break, table cells after the body, a header and a footnote as furniture |
| `sources/libreoffice_tdf129993.lwp` | `tdf129993.lwp` | Windows-1252 text (`€`, `à`), an address block and a table's cells |
| `sources/libreoffice_ordered_bullets.lwp` | `tdf33787-ordered-bullets.lwp` (its footer: CC0) | numbered and lettered list paragraphs (numbering not read), packed unicode in the header (`’`) |

Python docling has no Word Pro reader, so the `expected/` outputs are our
own. Regenerate after an intentional change with `DOCLING_RS_REGEN=1
DOCLING_RS_REGRESSION_FORMATS=lwp cargo test -p docling --test regression`.
