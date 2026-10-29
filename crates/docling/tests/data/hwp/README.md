# Hancom Hangul fixtures (#216)

HWP 5 files from [hwplib](https://github.com/neolord0/hwplib)'s
`sample_hwp/` (Apache-2.0, revision `6746c27`), HWPX files from
[hwpxlib](https://github.com/neolord0/hwpxlib)'s `testFile/reader_writer/`
(Apache-2.0, revision `f9fd225`), renamed to ASCII.

| File | Origin | What it pins |
| --- | --- | --- |
| `sources/hwplib_table.hwp` | `basic/표.hwp` | a 3 × 3 table with a two-paragraph cell, an empty table |
| `sources/hwplib_para_head.hwp` | `extracting_ParaHead.hwp` | outline (`개요`) paragraphs as headings, numbered paragraphs as an ordered list |
| `sources/hwplib_header_footer.hwp` | `basic/머리글꼬리글.hwp` | page header/footer controls as furniture, `개요 1` / `개요 2` headings |
| `sources/hwplib_text_box.hwp` | `basic/글상자.hwp` | text boxes (drawing objects with a paragraph list) after their anchor paragraph |
| `sources/hwplib_ruby.hwp` | `basic/덧말.hwp` | ruby text (`tdut`) after its base in parentheses |
| `sources/hwplib_notes.hwp` | `basic/각주미주.hwp` | footnote and endnote controls as furniture |
| `sources/hwplib_pictures.hwp` | `basic/이미지추가.hwp` | pictures from `BinData/BIN000N.*`, an uncompressed stream in a compressed document |
| `sources/hwpxlib_table.hwpx` | `SimpleTable.hwpx` | a table with a 2 × 2 and a 1 × 2 span |
| `sources/hwpxlib_sample1.hwpx` | `sample1.hwpx` | click-here fields, italic runs, shapes anchored mid-paragraph |
| `sources/hwpxlib_change_track.hwpx` | `ChangeTrack.hwpx` | tracked deletion and insertion, read accepted |
| `sources/hwpxlib_header_footer.hwpx` | `HeaderFooter.hwpx` | a page header as furniture |
| `sources/hwpxlib_ruby.hwpx` | `SimpleDutmal.hwpx` | ruby text (`hp:dutmal`) |
| `sources/hwpxlib_equation.hwpx` | `SimpleEquation.hwpx` | an equation's script kept as text |

Python docling has no HWP reader, so the `expected/` outputs are our own.
Regenerate after an intentional change with `DOCLING_RS_REGEN=1
DOCLING_RS_REGRESSION_FORMATS=hwp cargo test -p docling --test regression`.
