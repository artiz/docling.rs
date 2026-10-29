# Print-file fixtures (#216)

Both files are our own (no public `.prn` corpus exists).

| File | What it pins |
| --- | --- |
| `sources/lotus_report.prn` | a spreadsheet's "print to file" report in CP437: prose blocks read through, the column block kept preformatted, the form feed a page break |
| `sources/laserjet_letter.prn` | a LaserJet stream: the PJL job header and trailer, PCL resets, font selections and a raster `W` data block taken out, a backspace-underlined word |

Regenerate after an intentional change with `DOCLING_RS_REGEN=1
DOCLING_RS_REGRESSION_FORMATS=print_file cargo test -p docling --test regression`.
