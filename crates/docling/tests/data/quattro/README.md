# Quattro Pro fixtures (#216)

All sources come from the Open Preservation Foundation
[format-corpus](https://github.com/openpreserve/format-corpus)
(`office/spreadsheet/*`, Creative Commons CC0 — Johan van der Knijff); the
same files ship as Apache Tika's Quattro Pro test documents.

| File | Origin | Generation |
| --- | --- | --- |
| `sources/formatcorpus_ksbase.wq1` | `wq1/KSBASE.WQ1` | Quattro Pro 1–4 for DOS (BOF 0x5120): integers, numbers, labels |
| `sources/formatcorpus_ks4000.wq2` | `wq2/KS4000.WQ2` | Quattro Pro 5 for DOS (BOF 0x5121): styled cells, formulas with cached values and string results |
| `sources/formatcorpus_test.wb1` | `wb1/testQuattro.wb1` | Quattro Pro 1/5 for Windows (BOF 0x1001), saved by Quattro Pro 6.0 |
| `sources/formatcorpus_test.wb2` | `wb2/testQuattro.wb2` | Quattro Pro 6 for Windows (BOF 0x1002) |
| `sources/formatcorpus_test.wb3` | `wb3/testQUATTRO.wb3` | Quattro Pro 7/8 — OLE `PerfectOffice_MAIN` stream (BOF 0x1007) |
| `sources/formatcorpus_test.qpw` | `qpw/testQUATTRO.qpw` | Quattro Pro 9+ — OLE `NativeContent_MAIN` (`QPW9` zones, string table, column cell runs) |

Python docling has no Quattro Pro reader (it goes through LibreOffice), so
the `expected/` outputs are our own; cell values were cross-checked against
a record walk of the same files. Regenerate after an intentional change with
`DOCLING_RS_REGEN=1 cargo test -p docling --test regression`.
