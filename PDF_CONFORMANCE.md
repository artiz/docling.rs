# PDF conformance roadmap

How close the Rust PDF pipeline gets to docling's **default** Markdown, measured
byte-for-byte against the committed groundtruth (`tests/data/pdf/groundtruth/*.md`),
and what it would take to close the remaining gap.

> Measure locally with `scripts/pdf_groundtruth.sh` (no docling install needed —
> it diffs against the checked-in reference). The numbers below are the current
> state.

## Current state

**1 / 14 groundtruth PDFs are byte-for-byte exact** (`picture_classification`);
the rest are blocked on one of the categories below. Diff = changed lines vs the
groundtruth (one changed line counts as 2).

| PDF | diff | dominant blocker |
|---|---:|---|
| picture_classification | **exact** | — |
| right_to_left_01 | 4 | RTL/bidi |
| code_and_formula | 6 | inter-run spacing + code fencing |
| right_to_left_02 | 8 | RTL/bidi |
| amt_handbook_sample | 12 | double-spaces, duplicate glyphs, fractions |
| 2305.03393v1-pg9 | 25 | table structure |
| right_to_left_03 | 74 | RTL/bidi |
| multi_page | 76 | inter-run spacing + line-wrap hyphens |
| normal_4pages | 108 | reading order (CJK) |
| 2305.03393v1 | 152 | table structure |
| table_mislabeled_as_picture | 151 | table structure |
| 2203.01017v2 | 346 | table structure (+ inter-run spacing) |
| 2206.01062 | 321 | table structure |
| redp5110_sampled | 342 | table structure |

Shipped in this PR (no regressions; `pdf_conformance` stays 76/76):
de-hyphenation + typography normalization, `<!-- formula-not-decoded -->`,
caption-before-image pairing, and (strict-mode only) punctuation tightening.

Reaching ~50% exact requires the two big items below: **text-stream extraction**
(unlocks the spacing-bound PDFs) and **TableFormer** (unlocks the six
table-bound PDFs).

---

## Blocker 1 — inter-run text spacing (a.k.a. "text-stream extraction")

**Symptom.** pdfium splits a visual line into multiple style *segments* (a
citation's superscripts, a code line's tokens, mixed fonts). We emit one cell
per segment and join them with single spaces, so the real inter-run spacing is
lost: `[ 37 , 36 ]` instead of `[37, 36]`, `function add ( a , b )` instead of
`function add(a, b)`. docling reads text via pypdfium2's `get_text_range`
(`FPDFText_GetText`), which inserts spaces from each glyph's *advance* and so
reproduces the PDF's real spacing.

**What was tried in this PR and why each failed** (all reverted):

1. **Raw char API** (`PdfPageText::chars()` → `unicode_char()` + `loose_bounds()`,
   concatenated per line). pdfium's per-char list is *unreliable*: some lines
   come back with no space characters at all (`Thiscontentisextremelyvaluablefor`)
   and the char order is occasionally scrambled. Net regression.
2. **`inside_rect()`** (`FPDFText_GetBoundedText`) over a whole line's bounding
   box. `GetBoundedText` ≠ `GetText`: it *drops* inter-run spaces on
   multi-segment lines (`{ahn,nli,mly,taa}@zurich` vs docling's
   `{ ahn,nli,mly,taa } @zurich`) and *bleeds* glyphs from vertically adjacent
   lines (`nevertheless exLines of different…`). Net regression.
3. **Hybrid** (segment text for single-segment lines, `inside_rect` only for
   multi-segment lines). Same `GetBoundedText` divergence on exactly the lines
   that need fixing.

**Root cause / the real fix.** `segment.text()` is itself `inside_rect(segment.
bounds())` — i.e. the *only* reliable text unit pdfium-render exposes is a single
style run. What docling uses, `FPDFText_GetText(textpage, start_index, count, …)`
for an arbitrary **character range**, is *not* wrapped by `pdfium-render`
0.8.37. The path forward is to get that call:

- add a thin binding for `FPDFText_GetText` over a char range (upstream PR to
  `pdfium-render`, or call it through the crate's `PdfiumLibraryBindings` handle
  directly), then
- group segments into lines (by vertical band, splitting at column gutters — the
  clustering already prototyped in this PR), map each line to its `[start, count]`
  char range, and read the whole line with `GetText`.

This is the single highest-leverage change for default-mode conformance: it
fixes citations, inline code, fractions, and the justified-text double spaces,
and unblocks `multi_page` and `code_and_formula` (the latter also needs code
regions rendered as fenced blocks). **Stopgap shipped:** `--strict` tightens the
citation/parenthetical spacing at serialization time, so strict Markdown already
reads cleanly even though default mode still mirrors the segment spacing.

Also needed alongside it:
- **Line-wrap de-hyphenation for real hyphens.** We already drop the U+0002 soft
  hyphen; `multi_page` wraps words with a real `-` (`professi-`/`onal`), which
  needs line-end-hyphen detection during the line join.
- **Double-space preservation.** docling keeps the PDF's wide justified spacing
  (`the stainless  steel  nuts`); `clean_text` currently collapses runs of
  whitespace. With `GetText` per line, stop collapsing intra-line spacing.

## Blocker 2 — table structure (TableFormer)

**Symptom.** Six PDFs (`2206.01062`, `2305.03393v1[-pg9]`, `redp5110_sampled`,
`table_mislabeled_as_picture`, and the table on `2203.01017v2`) are dominated by
table differences. We reconstruct grids *geometrically* (cluster cells into
rows/columns); docling runs **TableFormer**, an autoregressive transformer that
predicts the table structure as an OTSL/HTML tag sequence plus per-cell bounding
boxes, which recovers spanning headers and merged cells we cannot.

**Scope of a port** (large — own PR, likely staged over several):

1. **Weights.** TableFormer ships in `docling-ibm-models` (`TableModel04_rs`,
   "accurate"/"fast" variants). Export the encoder + the two decoders to ONNX
   from the published checkpoint; confirm the license permits redistribution of
   a converted model.
2. **Inference loop.** Unlike the layout/OCR models (single `Session::run`),
   TableFormer is **autoregressive**: encode the table-crop image once, then step
   the structure decoder to emit OTSL tokens until `<end>`, feeding each token
   back in. The cell-bbox decoder runs per predicted cell. This is a real
   decoding loop in `fleischwolf-pdf`, not a one-shot call — budget for KV-cache
   handling and a token vocabulary/OTSL grammar.
3. **Cell content.** Map predicted cell bounding boxes back onto the PDF text
   cells (we have these) to fill cell text — the same matching docling does for
   "PDF" tables (it does not OCR programmatic tables).
4. **Serialization.** Convert the predicted OTSL grid (with row/col spans) to the
   `Table` node; the Markdown table serializer already exists but assumes a plain
   grid, so spans need representing.

A cheaper interim improvement (not docling-exact, but closes some diff): better
geometric reconstruction — detect header rows, merge obvious spanning cells, and
handle the multi-line header cells that currently shatter into many columns.

## Blocker 3 — RTL / bidi (Arabic)

`right_to_left_01/02/03`. Two compounding issues: (a) reading order — Latin runs
embedded in RTL text and the overall right-to-left flow are emitted left-to-right
(`Python و ة R` vs `R و Python`); we'd need Unicode bidi reordering of each line.
(b) Arabic shaping — pdfium returns presentation-form / decomposed sequences that
differ from docling's (`اإل` vs `الإ`), needing NFC-ish normalization of the
Arabic block. Both are self-contained but specialized; lower priority than 1–2.

## Smaller items

- **Duplicate glyphs** (`amt_handbook`: `T he`, `F Figure 7-26 6`). pdfium emits
  doubled glyphs for some bold/overlapping text; needs de-duplication of
  overlapping cells.
- **Code regions** → fenced ```` ``` ```` blocks with the caption *after* (code
  captions trail; figure captions lead). Pairs with Blocker 1 for the code text.
