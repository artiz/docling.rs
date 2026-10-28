# Computer Graphics Metafile fixtures (#216)

| File | Origin | What it pins |
| --- | --- | --- |
| `sources/libreoffice_binary_corvette.cgm` | LibreOffice `sd/qa/unit/data/cgm/pass/binary_corvette.cgm` (MPL-2.0) | binary encoding: the metafile name as the title, three `TEXT` labels of a clip-art drawing |
| `sources/libreoffice_circles.cgm` | LibreOffice `…/cgm/pass/circles.cgm` (MPL-2.0) | binary encoding with real VDCs, 14 pictures and no text: a heading per picture |
| `sources/libreoffice_clear_text.cgm` | LibreOffice `…/cgm/fail/ooo6420-1.cgm` (MPL-2.0; an xfig export LibreOffice's importer rejects) | clear-text encoding (ISO 8632-4): `BEGMF` / `TEXT` statements |

Python docling has no CGM reader, so the `expected/` outputs are our own.
Regenerate after an intentional change with `DOCLING_RS_REGEN=1
DOCLING_RS_REGRESSION_FORMATS=cgm cargo test -p docling --test regression`.
