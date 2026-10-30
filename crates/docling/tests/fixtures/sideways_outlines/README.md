# Sideways outlined table page with a text layer (#936)

`rotated-table-page.pdf` — the reporter's script on #936: a landscape table
turned 90° onto a portrait page, its text drawn as vector outlines
(Ghostscript `-dNoOutputFonts`), plus a native upright `Page 1` footer.
The text layer holds only the footer, so the sideways raster has to be
orientation-probed like a scan's before layout and OCR (`gen.py` builds the
body; the `Page 1` text is added with PyMuPDF's `insert_text`).

SHA-256 `afa23af6d366e7193b877c7446e2a4c221a4c727deb1f739e2036f26c78fe1da`.
