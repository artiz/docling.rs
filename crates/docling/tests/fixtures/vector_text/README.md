# Vector-outlined text on a page with a text layer (#931)

`vector-text-page.pdf` — contributed on #931 by its reporter: a one-page
synthetic document (six copies of one paragraph) whose body text Ghostscript
10.06 converted to filled outlines (`-dNoOutputFonts`), after which a small
native `Page 1` footer was added in Helvetica. The text layer thus holds only
the footer; the paragraph exists as paths alone and has to be OCR'd, as
docling's default `pdf_aware_layout_regions` OCRs every layout cluster with
no programmatic text in it.

SHA-256 `598874b00b77a9977fab06ac6d5cc720d3e80cf7982f905f625364691b271550`.
