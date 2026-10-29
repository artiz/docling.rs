# T602 fixtures (#216)

| File | Origin | What it pins |
| --- | --- | --- |
| `sources/libreoffice_cti_t602.602` | LibreOffice `sw/qa/core/data/602/pass/cti_t602.602` (MPL-2.0) | `@CT 1` (Latin-2 / CP852), the `@` settings skipped, bold (`0x02`) and wide (`0x0F`) styles, soft-wrapped lines (`0x8D`) and a soft hyphen across a wrap (`0xAD 0x8D`), a frame drawn with box characters kept preformatted, `-` bullets as a list |
| `sources/libreoffice_zak.602` | LibreOffice `sw/qa/core/data/602/pass/zak.602` (MPL-2.0; the Czech travel-expenses act 119/1992) | no header: the Kamenický default, hard-wrapped paragraphs read through |

Python docling has no T602 reader (it reaches the format only through
LibreOffice), so the `expected/` outputs are our own. Regenerate after an
intentional change with `DOCLING_RS_REGEN=1 DOCLING_RS_REGRESSION_FORMATS=t602
cargo test -p docling --test regression`.
