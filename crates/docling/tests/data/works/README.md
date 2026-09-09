# Microsoft Works word-processor fixtures (#216)

| File | Origin | Generation |
| --- | --- | --- |
| `sources/works2_dos.wps` | LibreOffice `writerperfect/qa/unit/data/writer/libwps/pass/Works_2.00A_DOS.wps` (MPL-2.0) | Works 2.00A for DOS — raw `01 FE` stream, CP 850 text |
| `sources/works3.wps` | LibreOffice `…/libwps/pass/Works_3.0.wps` | Works 3.0 for Windows — OLE `MN0` stream (`04 FE`) |
| `sources/works45.wps` | LibreOffice `…/libwps/pass/Works_4.5.wps` | Works 4.5 — OLE `MN0` stream (`06 FE`) |
| `sources/works5.wps` | LibreOffice `…/libwps/pass/Works_5.0.wps` | Works 2000 — OLE `CONTENTS` stream, `CHNKINK` |
| `sources/works6.wps` | LibreOffice `…/libwps/pass/Works_6.0.wps` | Works 6.0 — OLE `CONTENTS` stream, `CHNKWKS` |

These are LibreOffice's libwps smoke files: structurally complete documents
with (nearly) empty text, so they pin generation detection and the header /
index / text-zone walk rather than content. The content decoding (text codes,
CP 850, character formats, UTF-16 zones, FDPC pages) is pinned by the
synthetic streams in `wps.rs`'s unit tests, whose layouts follow libwps'
`WPS4Text` / `WPS8Text` readers. Python docling has no Works reader (it goes
through LibreOffice), so the `expected/` outputs are our own. Regenerate
after an intentional change with `DOCLING_RS_REGEN=1 cargo test -p docling
--test regression`.
