#915 | fixed | claude/issue-915-ocr-cell-confidence | 2026-10-28T23:00Z
#916 | fixed | claude/issue-916-render-omissions | 2026-10-29T06:00Z
#917 | fixed | claude/issue-917-render-feature | 2026-10-29T06:00Z
#918 | fixed | claude/issue-918-ris-amp | 2026-10-29T00:40Z
#919 | fixed | claude/issues-919-923-pdf-text-layer | 2026-10-29T03:55Z
#920 | fixed | claude/issues-919-923-pdf-text-layer | 2026-10-29T03:55Z
#921 | skipped | word gap is docling-parse's avg-char-width*0.33 threshold (dp_lines.rs port); changing it moves every PDF baseline, needs models + fixture (gist unreachable) to validate | 2026-10-29T03:55Z
#922 | fixed | claude/issues-919-923-pdf-text-layer | 2026-10-29T03:55Z
#923 | fixed | claude/issues-919-923-pdf-text-layer | 2026-10-29T03:55Z
#934 | skipped | feature request not a bug: recovery is correct; wants a new public repair-provenance API that lopdf 0.45 discards (Stream::new overwrites /Length) — needs maintainer API design or lopdf patch | 2026-10-30T06:50Z
#936 | fixed | claude/issue-936-sideways-outlines | 2026-10-30T19:50Z
#938 | fixed | claude/issue-938-bounded-repairs | 2026-10-31T03:13Z
#942 | skipped | release-asset provenance/licence manifest request (mutable models-v1 release, upstream revisions unverifiable offline); needs maintainer design, not a code bug | 2026-10-31T07:00Z
#945 | skipped | OCR detector perf tuning; acceptance needs .models + docling-parse shim (absent here) to measure FUNSD recall, snapshots and time/memory; cannot validate | 2026-10-31T12:00Z
#947 | fixed | claude/issue-947-det-box-crop-outside | 2026-10-31T15:27Z
#951 | fixed | claude/issue-951-models-immutable | 2026-11-01T07:00Z
#952 | skipped | audit-report request, no code bug: paste/ttf-parser are unmaintained with no drop-in maintained replacement; swapping deps is a maintainer supply-chain decision | 2026-11-01T08:00Z
#953 | skipped | licence/attribution notice request for zune-inflate (docs/NOTICE; upstream-crate provenance), not a code bug | 2026-11-01T08:00Z
#954 | skipped | provenance/SBOM request about Microsoft's ORT binary; nothing actionable in this repo beyond docs | 2026-11-01T08:00Z
#955 | skipped | model training/conversion lineage records follow-up to #942; needs maintainer/upstream data, docs/manifest only | 2026-11-01T08:00Z
#970 | fixed | claude/issue-970-preserve-order-feature | 2026-11-02T03:46Z
#973 | fixed | claude/issue-973-models-catalog | 2026-11-02T05:57Z
#979 | fixed | claude/issue-979-calamine-0.38 | 2026-11-03T21:46Z
#984 | fixed | claude/issue-984-text-layer-order | 2026-11-05T05:50Z
#987 | fixed | claude/issue-987-hybrid-table-overflow | 2026-11-05T20:17Z
#990 | fixed | claude/issue-990-tableformer-bbox-chunks | 2026-11-06T05:46Z
#994 | fixed | claude/issue-994-form-cache | 2026-11-06T22:13Z
#998 | fixed | claude/issue-998-reading-order | 2026-11-07T20:24Z
#999 | skipped | needs .models (layout/OCR/TableFormer) + scanned fixture to reproduce and validate the auto-mode bitmap OCR merge; none in this container | 2026-11-07T19:19Z
#1000 | fixed | claude/issue-1000-tf-degenerate | 2026-11-07T20:51Z
#1001 | fixed | claude/issue-1001-pdf-text-normalize | 2026-11-07T21:15Z
#1002 | fixed | claude/issue-1002-pua-bullets | 2026-11-07T21:56Z
#1003 | skipped | docling-parse 7 probes the embedded glyph outline (blank -> space, inked -> U+FFFD, text.h add_cell); needs glyph-ink detection for Type1/CFF/TrueType in textparse + baseline refresh, larger than the issue's scope | 2026-11-07T21:21Z
#1004 | fixed | claude/issue-1004-heading-keywords | 2026-11-07T22:26Z
#1005 | skipped | blocked on #997 (assigned): upstream's script runs are an InlineGroup held by the text item (docling-core 2.106 schema); our flat JSON/HTML export renders InlineGroup as plain text, so <sup>/<sub> cannot surface until #997 lands | 2026-11-07T23:18Z
#1006 | fixed | claude/issue-1006-broken-text-layer | 2026-11-08T07:19Z
#1007 | fixed | claude/issue-1007-embedded-picture-images | 2026-11-08T07:30Z
#1008 | skipped | blocked on #997 (assigned): ticket says "After ticket A" — DocLang 0.9 whitespace/inline-group contract comes from A; doing it first would conflict with claude/issue-996-997-inline-contract | 2026-11-08T03:16Z
#1009 | claimed | claude/issues-1009-1010-footnotes | 2026-11-08T03:16Z
#1010 | claimed | claude/issues-1009-1010-footnotes | 2026-11-08T03:16Z
#1010 | skipped | blocked on #997 (assigned): upstream's JATS table notes are empty footnote wrappers over inline runs whose whitespace/printing follows A's InlineGroup contract (docling-core 2.106 prints the runs through the body walk); the footnote plumbing it needs lands with #1009 (claude/issues-1009-1010-footnotes) | 2026-11-08T03:40Z
#1011 | claimed | claude/issue-1011-html-escape-code | 2026-11-08T05:16Z
#1012 | claimed | claude/issue-1012-rich-heading-chunks | 2026-11-08T07:16Z
#1009 | fixed | claude/issue-1009-footnotes | 2026-11-08T07:20Z
#1011 | fixed | claude/issue-1011-html-escape-code | 2026-11-08T06:20Z
#1013 | claimed | claude/issue-1013-latex-title-braces | 2026-11-08T07:21Z
#1013 | fixed | claude/issue-1013-latex-title-braces | 2026-11-08T07:46Z
#1014 | claimed | claude/issues-1014-1042-docling-json-reader | 2026-11-08T07:50Z
#1042 | claimed | claude/issues-1014-1042-docling-json-reader | 2026-11-08T07:50Z
#1012 | fixed | claude/issue-1012-rich-heading-chunks | 2026-11-08T08:06Z
#1026 | claimed | claude/issue-1026-csv-sep-line | 2026-11-08T08:07Z
#1014 | fixed | claude/issues-1014-1042-docling-json-reader | 2026-11-08T08:06Z
#1042 | fixed | claude/issues-1014-1042-docling-json-reader | 2026-11-08T08:06Z
#1026 | fixed | claude/issue-1026-csv-sep-line | 2026-11-08T09:03Z
#1028 | claimed | claude/issue-1028-uspto-sigma | 2026-11-08T09:04Z
#1015 | claimed | claude/issue-1015-jats-content | 2026-11-08T09:16Z
#1015 | fixed | claude/issue-1015-jats-content | 2026-11-08T10:06Z
#1016 | claimed | claude/issue-1016-rich-table-cells | 2026-11-08T10:07Z
#1028 | fixed | claude/issue-1028-uspto-sigma | 2026-11-08T10:36Z
#1017 | claimed | claude/issue-1017-docx-lvloverride | 2026-11-08T11:16Z
#1016 | fixed | claude/issue-1016-rich-table-cells | 2026-11-08T12:03Z
#1017 | fixed | claude/issue-1017-docx-lvloverride | 2026-11-08T12:24Z
#1018 | claimed | claude/issue-1018-pptx-autonum-picfill | 2026-11-08T12:26Z
#1018 | fixed | claude/issue-1018-pptx-autonum-picfill | 2026-11-08T13:07Z
#1020 | claimed | claude/issue-1020-xlsx-errors-chart-cap | 2026-11-08T13:10Z
#1019 | claimed | claude/issue-1019-xlsx-cell-images | 2026-11-08T13:16Z
#1021 | claimed | claude/issue-1021-latex-href | 2026-11-08T13:16Z
