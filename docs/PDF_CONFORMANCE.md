# PDF conformance

How close the Rust PDF pipeline gets to docling's **default** Markdown, measured
byte-for-byte against the committed groundtruth (`tests/data/pdf/groundtruth/*.md`).
The groundtruth is regenerated from **live published docling**. The numbers in
this document are measured with `scripts/conformance/pdf_groundtruth.sh`,
which pins the conformance model set (fp32 layout/OCR env overrides below);
`scripts/conformance/conformance.sh pdf` runs the *default* (int8, English-OCR)
models over every source PDF, so its totals differ from this table.

> Measure locally with `scripts/conformance/pdf_groundtruth.sh` (diffs the checked-in
> reference; no docling install needed) or `scripts/conformance/conformance.sh pdf` (installs
> docling and diffs against it). Both report two metrics: **strict** (byte-for-byte)
> and **whitespace-normalized** (spacing-only diffs ignored). Diff = changed lines
> vs the groundtruth (one changed line counts as 2).

## Current state

**6 / 14 strict** · **7 / 14 whitespace-normalized.** (The two Korean
image-only pages `skipped_1page`/`skipped_2pages` carry no text groundtruth and
are no longer scored.)

| PDF | diff | dominant remaining blocker |
|---|---:|---|
| picture_classification | **exact** | — |
| multi_page | **exact** | — |
| 2305.03393v1-pg9 | **exact** | — (TableFormer table, cell-for-cell) |
| right_to_left_01 | **exact** | — (RTL period attachment) |
| right_to_left_02 | **exact** | — (kashida dedup + page-number layout) |
| amt_handbook_sample | 2 *(ws-ok)* | docling's spurious fraction double space — ours is more faithful |
| code_and_formula | **exact** | — (flat legacy code, line-preserving `pretty` in strict) |
| 2305.03393v1 | 14 | author-block cluster split + in-figure label clusters (model-level) |
| normal_4pages | 20 | two-column line interleave + section-1 numeral claim |
| table_mislabeled_as_picture | 54 | layout over-detects tables (survey rendered as tables) |
| right_to_left_03 | 60 | RTL bidi + wrapper (form) children order |
| redp5110_sampled | 73 | TOC row structure tails + cover-page ordering |
| 2203.01017v2 | 66 | reference-accent spacing + author-block splits (in-picture table recovered: same grid as docling, different OCR engine noise) |
| 2206.01062 | 82 | author-block cluster splits (model-borderline) + one int8-borderline header rowspan |

The per-fixture numbers above predate docling 2.118's reading-order
dehyphenation (docling#3888, ported in #250): both sides now join a
hard-hyphenated lowercase continuation across a column/page break without the
`word- continuation` artifact, so re-measuring against docling ≥ 2.118 shifts
the text-merge component of these diffs (2203/2305 snapshots and the mirrored
groundtruth already reflect it).

`amt` is the 7th under the whitespace-normalized metric: its only diff is
docling's spurious double space before the `1⁄4` fraction, where our single-spaced
output is the more faithful rendering. The remaining non-exact PDFs are heavy
multi-column / table docs whose gaps are model-level (TableFormer structure,
layout classification, title-page reading order), not text-layer.

The heavy table docs improved with the docling-parse **word-cell** grouping
feeding TableFormer and the #61 layout/reading-order postprocessor
(2305.03393v1 93→30, 2203.01017v2 209→161, 2206.01062 198→92): the parser's
per-word cells reproduced docling-parse's `word_cells` closely enough that
cell-to-grid matching tracked docling much better (the word grouping was later
replaced wholesale by the `create_word_cells` port described below). See
"Text reconstruction" below. The #60 matching work (docling's `MatchingPostProcessor` ported to
`tf_match.rs`, plus docling's exact table-crop rounding chain) took
2203 157→150 and redp5110 204→202 with every other fixture unchanged; the
#62 text fixes (docling-parse's quote-normalization table — every curly
quote → `'` — and joining region cells in docling-parse index order
instead of geometric bands) then took 2203 →130, 2206 92→80, 2305 28→26,
normal_4pages 56→44, redp5110 →194, and table_mislabeled 88→86.

The #157 **text-panel recovery** deliberately trades a few diff lines for
recovered content: an *uncaptioned* `picture` that is really a text panel —
dense, wide, multi-line words, whether from the digital text layer or from
OCR of the crop (docling's `bitmap_area_threshold`: bitmap areas ≥ 5 % of
the page get OCR'd on every page kind) — is demoted into per-paragraph text
regions instead of shipping as pixels. docling drops that text entirely
(cells assigned to a picture cluster are never serialized), so the +2/+6/+2
on 2203/normal_4pages/redp5110 are real recovered words (e.g.
normal_4pages' cover publisher block), not regressions; captioned figures
(the corpus' document screenshots) and sparse-label charts keep their crops
and their byte-exact output — `picture_classification` stays **exact**.

The #165 orphan-claimer fix mirrors docling's regular/special cluster split:
only regular clusters claim cells (`_find_unassigned_cells` walks
`regular_clusters` alone), so a line that merely *straddles* a figure border
no longer loses its cells to the picture's 0.2 claim — it becomes an orphan
text region and is emitted, as docling does. Orphans that end up fully inside
a picture or table are still re-dropped, matching docling's Markdown (a
picture's children never reach `MarkdownPictureSerializer` output; a table's
text renders through the grid). Took 2206 80→76 and table_mislabeled 86→80
with every other fixture byte-identical.

The #200 fix closes the scanned-page gap in that same rule: on OCR'd pages
the speculative in-picture OCR (the pass that feeds text-panel demotion)
emitted *all* of its recognized lines beside a kept picture, so a chart's
axis-tick strings spliced into the body text right next to the image — where
docling's postprocess "Remove regular clusters that are included in wrappers"
walks `SPECIAL_TYPES` (picture included) and silently absorbs any orphan
> 80 % contained in the picture as its child. The same containment drop that
already handled the first orphan wave now re-runs after the in-picture wave,
so only border-straddlers (≤ 80 % containment) surface as text, on scanned
pages exactly as on digital ones. The digital corpus is untouched (6/14
strict, same per-file diffs); 17 scanned/image snapshots shed their leaked
figure-internal text (axis ticks, diagram labels — net −59 lines).

The #265 **table-caption attachment** ports the table arm of docling's
`ReadingOrderPredictor._find_to_captions`: a `caption` region binds to the
table (or `document_index`) **immediately adjacent to it in reading order** —
above-caption and below-caption both, but only when exactly one side holds a
table/picture/code element, and never across intervening text. Adjacency, not
geometry, is the load-bearing choice: a flush-left `Table N:` label pairs with
a centered grid it doesn't horizontally overlap, while a geometrically-nearby
caption in the other column of a two-column page never pairs across the
gutter (a distance-based draft did exactly that mispairing on 2203's page 7).
The paired caption is consumed from its own reading-order slot and rides on
the table node — Markdown prints it above the grid, JSON emits docling's
`TableItem.captions` `$ref`, DocLang its `<caption>`. Caption text is now
also markdown-escaped on all three arms (table/picture/code), matching
docling's `export_to_markdown` post-process — redp5110's `TAX\_ID` and
2203's `&lt; td &gt;` figure caption were silently unescaped before. Took
2203 80→66 and redp5110 75→73, nothing worse; picture pairing (below-caption,
h-overlap-gated) is untouched.

The **cell order & join** are now docling's own, end to end. Serialization
order is pure docling-parse index (`_sort_cells`) — the geometric line
re-sort it replaces measured strictly worse on the corpus: normal_4pages'
big section numerals paint *after* their heading text, so only index order
yields docling's `## 들어가며 1`. The join is `PageAssembleModel.sanitize_text`
ported verbatim: a space after every line except one ending in `-`, which
fuses a wrapped word (alnum on both sides — the dash is dropped) or glues
verbatim when the dash stands alone — 2305's superscript ORCIDs render as
docling's `[0000 -0002 -3723 -6960]`, and its OTSL list keeps the raw en-dash
bullet in the text (`- -"C" cell a new table cell …`; the assembler no longer
strips a leading dash, only the symbol-font bullets docling-parse itself
drops). On top of that, cell assignment is exclusive (`_assign_cells_to_clusters`):
each non-empty cell goes to the single best-overlapping regular region at
> 0.2 intersection-over-self, so a cell under two overlapping boxes emits
once ("Hours Hours" duplicates gone), the orphan pass claims at the same
threshold (the old > 0.5 mirror and its (0.2, 0.5] completeness hole are
structurally closed), and normal_4pages' cover publisher block now lands in
its furniture cluster exactly as docling files it. Together: 2305 24→14,
normal_4pages 44→32, 2203 84→80, 2206 92→90, redp5110 166→164,
table_mislabeled 76→72 — −48 lines, nothing worse.

**Word cells are docling-parse's own `create_word_cells`** — a second
contraction over the shared char cells under the word factors
(`word_space_width_factor_for_merge` 0.33 for the adjacency gate, 2 × 0.33
for the never-firing space threshold), with space glyphs acting as pure
word-boundary barriers dropped from the run up front. The words TableFormer
matches against therefore tokenize exactly as docling's: a thin CJK space
whose neighbors overlap contracts into one spaceless word (docling's
`1군감염병`, where splitting at every line-space manufactured `1군 감염병`),
while a full Latin space's gap exceeds the gate and keeps words apart.
Table-heavy fixtures moved wholesale: redp5110 164→73 (the TOC "OTSL
model-level blocker" was largely tokenization), table_mislabeled 72→54,
normal_4pages 32→20, everything else byte-identical.

**Tables inside pictures serialize now.** 2203's Figure 10 is a *screenshot*
of a table: layout detects a `table` cluster inside the `picture`, TableFormer
reads its grid — but the region had no text layer, its speculative in-picture
OCR lines were discarded on digital pages, and the empty-text gate dropped the
whole element. docling OCRs bitmap-covered areas on every page kind and its
table cluster collects those cells. Mirroring the scanned path for exactly
these tables (text-less, >50 % under a picture, on a digital page): the table
region's word crops are recognized and feed the TableFormer matcher and the
cell set, so the grid serializes with text. The recovered ANOVA grid matches
docling's structure cell-for-cell; the cell *strings* differ where both
engines read noise (PP-OCR vs EasyOCR), which costs 2203 +6 diff lines —
a deliberate completeness-over-metric trade (an ODF presentation fixture's
table screenshot also gains its grid). The remaining 2206 table diff is a
single top-left header rowspan the int8 TableFormer resolves as two cells
where docling's fp32 run predicts one 2-row span — span decoding itself is
exercised by neighboring tables; that one token is quantization-borderline.

**TeX math glyphs decode by their built-in encodings.** The standard TeX
math fonts (`CMSY*`, `CMMI*`) ship no PDF `/Encoding`, no ToUnicode, and a
CFF program this parser does not read — their codes fell through to
StandardEncoding and rendered as the wrong ASCII: 2203's `{ahn,…}` author
line read `f ahn,… g`, `→` read `!`, `∈` read `2`, `|T|` read `jTj`.
A static table of the fixed TeX layouts (TeXbook Appendix F), keyed off the
base font name and applied only when the font dict has no `/Encoding` at
all, restores docling-parse's decode (it reads the same mapping out of the
font program). Took 2203 86→74. Cross-page **paragraph continuations** also
align with docling's `predict_merges`: the head test now accepts a trailing
comma (`.+[a-z,\-\u00AD]`, ASCII-lowercase only — a lone `μ` or an
uppercase OCR fragment no longer stitches), and tables joined the skip
set (docling's skip-labels), so 2206's "…In phase four," resumes across a
caption+table+figure page break. Took 2206 90→82; the remaining author-block
chain differences trace to model-borderline cluster splits (docling's run
splits a name block ours detects whole), not the merge rules.

The **footnote-hyperlink** port (docling's `PageAssembleModel._match_hyperlink`:
the URI whose annotation rects cover ≥ 0.5 of the region box, accumulated per
URI, pydantic-`AnyUrl` trailing-slash normalization) renders 2206's footnote
URLs as docling's `[1 https://…](https://…)` whole-item links. Scope is
footnote-labeled regions only: upstream's assemble stage matches every text
label, but both committed groundtruth generations observably carry the
hyperlink into the document **only for footnote items** (2206 page 1's fully
covered plain-text DOI line has `hyperlink: null` in docling's own JSON while
the equally covered footnotes keep theirs), and the corpus is the reference.
Took 2206 76→72.

**Code blocks flattened in legacy** (docling parity): docling's parser has no
line-preserving code path — its code items carry the lines joined by single
spaces — so `text` now holds that flat form on every byte-conformance surface
(legacy Markdown, JSON, DocLang, chunks) and the line-preserving extraction
moved to the new `pretty` field, which **strict** Markdown prefers. Took
code_and_formula 5→**exact** and redp5110 196→172 (its SQL listings); the
strict output is unchanged.

A **word-completeness audit** (character-level diff of normalized output vs
groundtruth, reorderings filtered out) confirms the extraction itself is
whole: 10 / 14 fixtures lose *zero* groundtruth words; the remaining gaps are
table-cell structure (2203's ANOVA grid, redp5110's authority-matrix rows —
TableFormer), picture-child divergence (2305's HTML/OTSL figure axis labels),
RTL/checkbox forms (right_to_left_03), and docling-parse artifacts we
deliberately don't reproduce (`/tildelow`, `/.notdef` glyph-name leaks).

The audit exposed one systematic hole, closed at the time by raising the
orphan claim to mirror the then-> 0.5 serializer (and later closed
*structurally* by the exclusive > 0.2 assignment above): docling assigns each
cell to its best cluster at > 0.2 overlap and serializes the assigned cells,
while our serializer took cells at > 0.5 — a cell whose best overlap fell in
(0.2, 0.5] was "claimed" but emitted by nobody (right_to_left_03's standalone
`20300`, several Korean labels on the skipped_1/2page scans). The orphan
pass's claim test now mirrors the serializer's criterion, so **every
non-empty text cell either serializes inside a region or becomes an orphan**
— completeness by construction. Checkbox regions
(`checkbox_selected`/`unselected`) are also no longer skipped: they assemble
as docling's task-list items (`- [x] بلی`). right_to_left_03's remaining diff
is ordering, not content — docling wraps that page's fields in `form`
containers whose children serialize in docling-parse cell order, while we
emit the same items in geometric reading order; the full wrapper-children
port (and the bidi run order of `-2-5`-style headings) stays on the
model-level blocker list, together with the title-page cluster splits of
2305/2206 (residual heron score noise — see the docling-exact layout input
below).

The **docling-exact layout input** closed most of the preprocessing gap: the
layout model now runs on the same image docling's stage feeds it — a
dedicated `get_page_image(scale=1.0)` render (pdfium at 1.5×, sized with
pypdfium2's `ceil`, then PIL-BICUBIC down to point size) stretched to
640×640 with **PIL BILINEAR**, both kernels ported byte-exactly from
Pillow's fixed-point `Resample.c` (`resample.rs::pil_resize`, verified
against genuine Pillow reference hashes; `preprocessor_config.json` says
`do_pad: false` — the heron processor stretches, it does not letterbox).
Previously the model saw a Triangle stretch of the 2× OCR bitmap —
resampling 1224→640 and 612→640 are different regimes, and heron's
borderline scores follow the pixels. Riding along: docling's same-label
**picture dedup** (`_remove_overlapping_clusters`, IoU/containment > 0.8
groups, larger box wins within 0.3 confidence), which collapses a figure
detected both whole and as sub-panels into the whole-figure box (2206's
four-thumbnail Figure 1). Net −48 diff lines corpus-wide: 2203 132→84,
normal_4pages 50→44, redp5110 172→166, table_mislabeled 80→76, 2305 26→24,
rtl_03 62→60; 2206 went 72→92 — its author-block clusters shifted at the
model's noise floor (ort-vs-torch fp32 numerics) and the reading-order
merge chains land differently. Every exact fixture stayed exact. The
browser (canvas) and METS/TIFF paths keep the legacy stretch (no pdfium
renderer there); the int8 default graph was calibrated on the old input
distribution — the fp32 low-coverage guard absorbs the borderline pages,
and the quant can be recalibrated with `scripts/install/quantize_models.py`
at the next model release.

## DocLang (`.dclx`) conformance

Separate from the Markdown metric above: how close `--to dclx` gets to docling's
DocLang archive, scored on the extracted `document.xml` against the committed
groundtruth (`tests/data/pdf/groundtruth_dclx/*.dclx`, from published docling
2.112.0). Run `scripts/conformance/dclx_conformance.sh pdf`; sweep the tolerance
with `scripts/conformance/dclx_pdf_tol_sweep.sh`.

**PDF avg similarity: 52 % exact · 63 % at the default ±2-grid-unit tolerance**
(issue #32 target: ≥50 %). The ±2 figure is within a point of the
*geometry-ignored* ceiling (65 %), so essentially all of the coordinate
difference is absorbed by ±2 — a wider tolerance buys almost nothing.

### What the geometry tolerance is, and why it is honest

Every laid-out block in a DocLang archive carries four `<location>` provenance
tokens — its bbox as `round(512·coord/page_dim)` on a 0–511 page grid
(docling_core's `_create_location_tokens_for_bbox`). We emit the same tokens
from our layout cluster boxes (`assemble.rs`, `norm_loc`) for text, headings,
tables, pictures, list items (on `ListItem.location`), code, and the
`page_header`/`page_footer` furniture blocks. Because our heron
layout model is docling's, the boxes agree to **~1 grid unit**; the small
residual is the aspect-ratio-stretch-vs-letterbox preprocessing difference, not a
structural gap. `dclx_diff.py` therefore counts a `<location>` pair as matching
when the two values are within `DCLX_TOL` (default **2**) grid units — **text,
tags, nesting, spans, and every non-geometry line stay byte-exact, and unmatched
lines always count against the score**. The tolerance is applied **only to PDF**,
where the reference geometry comes from docling's own layout run; formats whose
geometry is read from the same source file (OOXML slides/sheets) stay exact
(`DCLX_TOL=0`). `DCLX_TOL=0` reproduces a raw `diff` line-for-line.

### Per-fixture (±2)

Text/list-heavy pages land high (multi_page 82 %, right_to_left_02 82 %,
code_and_formula 81 %, 2305-pg9 78 %, right_to_left_01 75 %, amt 72 %,
normal_4pages 71 %, redp5110 65 %, 2206 61 %); the low ones are **model-level,
not provenance**: the big table papers (2203 51 %, 2305 52 %) diverge in
TableFormer cell structure (2203 alone is ~19 k table-grid diff lines),
table_mislabeled/picture_classification in layout classification, and
skipped_1/2page (Korean image pages) + right_to_left_03 in picture detection /
bidi — the *same* blockers that cap the Markdown metric. The corpus average is
bounded by these, so raising it further is a model problem, not a serialization
one: every laid-out block kind now carries provenance, so the ±2 figure sits at
the geometry-ignored ceiling.

## VLM pipeline conformance (#153/#311 — measured)

The remote-VLM pipeline (#77, `--pipeline vlm`) has its own comparison
harness: `scripts/conformance/vlm_conformance.sh` runs the PDF corpus through
docling.rs *and* Python docling's `VlmPipeline`, both against the same
endpoint (`scripts/dev/granite_vlm_server.py` — the only server class that
keeps granite-docling's DocTags tokens intact), and reports per-fixture
whitespace-normalized similarity plus byte-exactness. Known accepted
asymmetry: each side renders pages at its own scale, so some drift is
render-induced rather than parser-induced — triage before attributing.

**A GPU for the shim is a hard requirement, not a convenience** — measured
while dry-running the harness end-to-end for #311 (v1.23, 4-vCPU container,
fp32 `transformers` CPU inference of granite-docling-258M): a routine
academic page took **12 313 s (3.4 h) to generate 3 385 chars** (~0.1 tok/s;
a near-empty page still decodes at ~0.5 tok/s), while both clients cap a
page request at 600 s — the Rust agent's global timeout and
`vlm_convert.py`'s default alike. On CPU every real page therefore times out
client-side while the single-threaded shim grinds on as an orphan, blocking
the next request. That dry run did validate everything up to the model —
shim serving, both converters driving it, caching, scoring — and is what
shaped the harness fixes (venv `python3`, output-dir creation,
`--timeout`/`VLM_TIMEOUT`, the busy-shim probe hint, no-retry-on-timeout).
Reproduce with:

```bash
python scripts/dev/granite_vlm_server.py            # on a CUDA machine
scripts/conformance/setup-docling.sh
VLM_TIMEOUT=3600 scripts/conformance/vlm_conformance.sh   # prints the table + mean
```

### Measured results (#311)

Measured 2026-09-02 against `ibm-granite/granite-docling-258M` served by the
shim on an RTX 3080 Laptop (CUDA, bf16, greedy) — both sides drove the same
server; pages take roughly 390–590 s each at this model size on that GPU:

| fixture | sim% | byte-exact |
|---|---:|---|
| 2203.01017v2.pdf | 84.2 | no |
| 2206.01062.pdf | 59.2 | no |
| 2305.03393v1-pg9.pdf | 99.8 | no |
| 2305.03393v1.pdf | 58.8 | no |
| amt_handbook_sample.pdf | 98.6 | no |
| base14_fonts.pdf | 100.0 | **yes** |
| code_and_formula.pdf | 95.5 | no |
| docling-rs-demotion-repro.pdf | 94.2 | no |
| multi_page.pdf | 99.7 | no |
| normal_4pages.pdf | 67.5 | no |
| picture_classification.pdf | 100.0 | **yes** |
| redp5110_sampled.pdf | 78.9 | no |
| right_to_left_01.pdf | 97.8 | no |
| right_to_left_02.pdf | 69.1 | no |
| right_to_left_03.pdf | 100.0 | **yes** |
| skipped_1page.pdf | 98.4 | no |
| skipped_2pages.pdf | 95.7 | no |
| table_mislabeled_as_picture.pdf | 80.9 | no |

**Mean 87.7% over 18 fixtures, 3 byte-exact** (whitespace-normalized
character similarity of the two Markdown outputs, rust vs. Python docling).

Reading the numbers:

- Two conversions of the *same* page legitimately differ: each side renders
  at its own scale (docling.rs 144 dpi, Python docling 216 dpi), and the
  model's greedy decode is exquisitely sensitive to the input pixels — most
  of the gap on the mid-range fixtures (59–85%) is the model reading the two
  renders differently (dropped/merged cells in dense tables, different line
  wraps), not a parser divergence. Three byte-exact fixtures show the
  ceiling when the model answers identically.
- The long dense-table papers (2206.01062, 2305.03393v1) sit lowest — every
  page multiplies render-induced drift, and OTSL tables amplify a single
  mis-read cell into many token differences.
- Triage lesson baked into the harness: outputs cached from runs where a
  client timed out mid-corpus proved unreliable (two fixtures initially
  scored 1.3%/0.0% from stale artifacts and re-measured at 100.0/69.1) —
  delete `target/vlm-conformance/` after aborted runs rather than trusting
  survivors. A handful of table entries above (2203.01017v2, 2305.03393v1,
  redp5110_sampled) still carry early-run caches and read as lower bounds.

## Bedrock LLM comparison (speed + fuzzy conformance)

`scripts/conformance/bedrock_conformance.sh` benchmarks docling.rs against an
Amazon Bedrock model (Nova by default) prompted to extract each corpus PDF as
Markdown: docling.rs runs one warm CLI batch (set `DOCLING_RS_EP=cuda` for a
GPU run), Bedrock gets a timed Converse call per PDF, and both outputs score
against the committed groundtruth with a normalized line-similarity
percentage (byte-exactness is meaningless for an LLM, so both sides get the
same fuzzy metric). Needs boto3 plus `AWS_BEDROCK_REGION` /
`AWS_BEDROCK_ACCESS_KEY_ID` / `AWS_BEDROCK_SECRET_ACCESS_KEY` in the env; the
model, token cap and prompt are overridable (see the script header — note
Nova Micro is text-only per AWS docs, so PDF input may require
`AWS_BEDROCK_MODEL_ID=eu.amazon.nova-lite-v1:0`).

Measured baseline (2026-08, `eu.amazon.nova-lite-v1:0` in eu-central-1,
docling.rs on CPU with the conformance model pins) over the 14
groundtruth-scored PDFs:

| | total | per doc | mean similarity | converted |
|---|---|---|---|---|
| docling.rs (cpu) | 59.8 s | 4.3 s | **90.3 %** | 14/14 |
| nova-lite | 273.0 s | 21.0 s | 12.5 % | 13/14 |

Per file docling.rs scores 50.8–100 % (the low end is the same
`right_to_left_03` / `table_mislabeled_as_picture` tail the strict metric
tracks); Nova Lite tops out at 47.2 % (`multi_page`), returns 0 % on the RTL
fixtures, and one document failed with a `ModelErrorException` retry-please
error. A GPU run (`DOCLING_RS_EP=cuda`) shrinks the docling.rs column
roughly another order of magnitude (~0.1 s/page on a consumer RTX 3080).

## Enrichment models (opt-in)

docling's optional enrichment stages are ported behind the same flags
(`--enrich-picture-classes` / `--enrich-code` / `--enrich-formula`, docling's
`do_picture_classification` / `do_code_enrichment` / `do_formula_enrichment`)
and validated by `scripts/conformance/enrich_conformance.sh` against Python
docling 2.112's output on the enrichment fixtures
(`tests/data/pdf/groundtruth-enriched/`):

| Fixture | Check | Result |
|---|---|---|
| code_and_formula.pdf | Markdown, `--enrich-code --enrich-formula` | **byte-exact** (CodeFormulaV2's code rewrite, `JavaScript` language, formula LaTeX) |
| picture_classification.pdf | JSON classification annotation + meta | same class ranking; confidences match to ~3 decimals |

The CodeFormulaV2 export (`scripts/install/export_code_formula.py`) verifies
its three ONNX graphs' greedy decode **token-identical** to
`transformers.generate` before writing them. Its decoder also ships as a
dynamic INT8 quantization (`scripts/install/quantize_models.py
code-formula-decoder`, ~655 → ~165 MB, 4× less decoder RAM) that is preferred
automatically when present (`DOCLING_RS_FP32=1` opts out). Unlike the layout /
TableFormer INT8 models it is *near*-exact rather than byte-exact: greedy VLM
decoding has near-tie tokens that weight rounding can flip — on the fixture
the only drift is one extra blank line in the code block, and per-channel /
fp32-lm_head variants flip it identically, so the smaller per-tensor file is
kept. The conformance script gates fp32 byte-exact and allows the int8 leg
whitespace-only drift. The residual confidence drift on
the classifier comes from the crops: docling re-renders each region through
pdfium at the enrichment scale, while docling.rs resizes from the existing
scale-2 page render — sub-pixel differences the classifier's softmax sees in
the third decimal, and that the VLM's argmax decoding absorbs entirely on the
fixtures.

## How the pipeline works

pdfium extracts the glyph layer and renders each page to a bitmap; an ONNX stack
(layout detection, TableFormer, PaddleOCR) interprets it; regions are assembled in
reading order into a `DoclingDocument`. Note on OCR models: everything in this
document — snapshots, groundtruth, the conformance numbers — is measured with the
multilingual `ch_PP-OCRv3` recognition model (docling parity), which
`scripts/conformance/pdf_*.sh` pin via `DOCLING_OCR_REC_ONNX`/`DOCLING_OCR_DICT`.
The *runtime* default is the English `en_PP-OCRv3` pair (the `ch_` model glues
Latin words together); `DOCLING_RS_OCR_LANG=ch` restores the conformance model. Tables use **TableFormer** (image encoder
+ autoregressive OTSL structure decoder + cell-bbox decoder, ported and exported
to ONNX in `tableformer.rs`) on a cv2-exact preprocessed crop (`resample.rs`); the
structure + matched cell text reproduce docling's padded GitHub tables (2305-pg9
is cell-for-cell exact).

**Heading levels (#302, opt-in).** With `--heading-hierarchy` (off by default —
everything in this document is measured with it off), a post-assembly stage
ports docling's `HeadingHierarchyModel`: section-header levels are assigned
from the PDF outline (bookmarks, fuzzily matched by title + page; a matched
list item is promoted to a heading), else legal/outline numbering, else font
style (glyph-height clustering + the conservative font-name weight/slant
parser). It runs on the assembled node stream (`heading_hierarchy.rs`,
`outline.rs`, `font_style.rs` in docling-pdf) and rewrites only heading
levels, so the default-off output — and every snapshot below — is untouched.

**Rotated scans.** Two normalization passes run before any inference, both only
on pages with no text layer (exactly the OCR set), both mapped back to
display-space geometry at assembly via `PdfPage::rotation`:

1. **`/Rotate` metadata** (`pdfium_backend::extract_page`): pdfium renders the
   page as displayed, so a declared rotation is un-rotated losslessly and
   `width`/`height` swap. Pinned by `crates/docling/tests/scanned.rs` — all
   four `/Rotate` orientations of `ocr_test.pdf` OCR byte-identically.
2. **Content-based orientation** (`orient.rs`, #225): a physically rotated
   raster (`/Rotate 0` — sideways phone photo, landscape-fed sheet) is probed
   with the recognizer itself, the classic OSD trick: segment the page with
   the same projection segmentation OCR uses, recognize up to 6 of the widest
   line crops under each 90° hypothesis, score by Σ(confidence × chars). An
   upright page early-exits after one probe round (≥20 chars at ≥0.90 mean
   confidence); a rotated hypothesis must read real text (≥8 chars at ≥0.55)
   *and* beat upright by 1.2× to win — thin evidence (blank/line-art pages)
   is a no-op, and any probe failure degrades to "assume upright". Scores are
   deterministic (single-threaded rec, fixed probe selection), so snapshots
   hold. `DOCLING_RS_OCR_ORIENTATION=off` disables the pass;
   `DOCLING_RS_DEBUG=1` prints per-hypothesis scores. Pinned by the
   `ocr_test_raster*` fixtures (the same lossless raster physically rotated
   inside the page): all four convert byte-identically.

### Performance / parallelism

Profiling a 14-page document (`DOCLING_RS_TIMING=1` prints an env-gated per-stage
wall-clock breakdown) shows ~80 % of the time is the two ONNX models (layout ~58 %,
TableFormer ~22 %) and ~16 % the page-image downsample — all per-page work that is
independent across pages. A multi-page PDF therefore renders on one thread (pdfium
is not thread-safe) and fans the pages out across a **pool of page-workers**, each
owning its own model set (`ort`'s `Session::run` is `&mut self`, so sessions can't
be shared), reassembled in page order. A bounded channel keeps only a handful of
page bitmaps resident, so the streaming memory profile is preserved; the output is
byte-identical to the serial path (verified across all PDF snapshots). Single-page /
image / METS inputs keep the serial path and load no helper models.

The layout model is **memory-bandwidth bound** (even one model at four intra-op
threads only reaches ~2.1× core utilisation), so the pool defaults to two intra-op
threads per worker with `workers ≈ cores / 2` (capped at 4): two threads sharing one
in-cache copy of the weights beats both one fat model and many single-thread workers.
The speed-up scales with cores and memory bandwidth. Tune per machine with
`DOCLING_RS_PDF_WORKERS` (pool size) and `DOCLING_RS_PDF_INTRA` (intra-op threads
per worker). Each worker layout-detects up to `DOCLING_RS_PDF_LAYOUT_BATCH`
already-rendered pages per inference call (issue #73; default 4 on 8+ cores,
1 below — measured on a 4-core box the batch costs pipeline overlap: 8.1 →
9.3 s/conv on 2206.01062). Output is bit-identical at every batch size, so
the knob is purely about throughput.

### Text reconstruction: a pure-Rust PDF text parser (default)

The byte-exact ceiling was the **text extractor** — pdfium's *rendered* glyph
boxes diverge from docling's own `docling-parse` C++ parser at exactly the points
that drive conformance (generated spaces, combining marks, ligature/fraction
positioning). The pipeline now ships a **pure-Rust text parser** (`textparse.rs`,
on `lopdf`) that reconstructs each glyph's box from the *font's own advance
widths* and the PDF text/graphics matrices — the same information docling-parse
uses. It is the **default** text layer; set `DOCLING_PDFIUM_TEXT=1` to fall back
to pdfium. Pages without a parseable text layer fall back to pdfium
automatically, so scanned/OCR pages are unaffected. The parser supplies **all**
text — prose, the **word cells** TableFormer matches against, and **code cells**
(`DOCLING_PDFIUM_WORDS` reverts words+code to pdfium; `DOCLING_PDFIUM_TEXT`
reverts everything). pdfium now does only page rasterisation + link annotations.

The parser handles Type0/CID + Identity-H and simple Type1/TrueType fonts,
ToUnicode CMaps (`bfchar`/`bfrange`), WinAnsi/MacRoman + `/Differences`
encodings, **Form XObject recursion** (`Do` — bulk body text in heavy PDFs lives
inside a form; 2206 p1 was dropping ~9000 chars), a **glyph-name fallback**
(docling emits an unmappable subset-font name verbatim, `/g115`), and an
**overprint dedup** (a kashida elongation re-stamped on itself — right_to_left_02).
A char-frequency validator (`scripts/test/parser_completeness.py`) confirms nothing is
silently skipped.

Its cells feed the ported **docling-parse line sanitizer** (`dp_lines.rs`, from
`src/parse/page_item_sanitators/cells.h`): a 3-pass corner-distance contraction
(LTR → RTL → LTR-reverse) with `merge_with` space insertion (one space when the
gap exceeds 0.33×avg-char-width, plus literal space glyphs), `enforce_same_font`,
ligature recomposition, and loose-box geometry. On the clean parser boxes it uses
the Euclidean corner gap (matching docling); on pdfium's loose boxes it keeps the
signed horizontal gap.

**Word cells** come from a second contraction over the same char cells
(`create_word_cells`, see the word-cell section above): the word factors
(adjacency gate 0.33, space threshold 2 × 0.33) with space glyphs dropped up
front as pure word-boundary barriers — verified against the installed
docling-parse oracle (redp5110 pages byte-exact). These are the per-word
tokens TableFormer matches against table-grid cells, replacing pdfium's word
cells (roadmap item 6). **Code cells** come from the parser too,
via a gap-based grouping (`Grouping::CodeGap`): the parser emits no space glyphs
(a source space is a positioning gap), so a word breaks wherever the inter-glyph
gap exceeds ~0.25× the line height, with no punctuation glue — `et al. 2000`
keeps its space while `add(a,` / `b)` stay joined. `code_and_formula` is byte-exact
(`function add(a, b) { return a + b; }`). With this, pdfium's text path is fully
retired (rasters + links only).

Other text/serializer/layout fixes matching docling: markdown escaping (`_`→`\_`,
then HTML-escape `&`/`<`/`>`), typographic-punctuation normalization
(`’`→`'`, `–`/`—`→`-`, `“”`→`"`, or `'` for Hangul fonts), `@`-glue
(`mAP @0.5`), wrap dehyphenation, paragraph-continuation merging across
column/page breaks, band-aware two-column reading order, **false-picture
suppression** (empty low-confidence margin boxes on text pages), and
**page-number-first** ordering.

## Remaining blockers (model-level)

These yield smaller or uncertain gains than the text-layer work already shipped.
The issues that tracked them (#60–#63) are **closed**: everything
heuristic-level in them landed, and what remains below is the documented
model-level (or by-design) residual each issue closed with:

1. **TableFormer structure on complex tables**
   ([#60](https://github.com/docling-project/docling.rs/issues/60)). The
   *matching* half is done: docling's `MatchingPostProcessor` (cell-class-aware
   good/bad IOU split, column-median snapping, adjacent-column de-duplication,
   best-intersection word assignment, row/column-band orphan pickup) is ported
   in `tf_match.rs` and is the default word→cell matcher, and the table crop
   reproduces docling's exact rounding chain (`round(bbox) → ×2 → ×1024/h →
   round`, banker's rounding) — 2203 157→150, redp5110 204→202, everything else
   unchanged. The rest is **model-level**: the OTSL tag stream itself differs
   from live docling on the hard crops (redp5110's TOC predicts `ched` where
   docling gets `fcel`; multi-row headers / spans on 2206, 2203), so one
   cell-structure diff still cascades through the padded columns into many row
   diffs (at the time, 2206's ~92 table-row diffs traced to ~4 structure
   diffs; today its one remaining table diff is a single header rowspan). A parity
   harness (`DOCLING_RS_TF_MATCH_DUMP=dir` + `scripts/test/tf_match_reference.py`-style
   replay through docling's Python post-processor) confirmed the ported matcher
   reproduces the reference on identical inputs, isolating the residual to the
   model predictions. `DOCLING_RS_TF_SIMPLE_MATCH=1` reverts to the pre-port
   best-overlap matcher.
2. **Layout classification**
   ([#61](https://github.com/docling-project/docling.rs/issues/61)) — *addressed
   by porting docling's `LayoutPostprocessor`.* The raw RT-DETR detections now go
   through the cleanup docling applies before assembly: per-label confidence
   thresholds (`CONFIDENCE_THRESHOLDS`, stricter than the 0.3 base — a
   picture/table/list needs ≥ 0.5), regular/picture/wrapper **bucketed** overlap
   resolution (a high-score picture no longer suppresses a lower-score table or
   table-of-contents index), the picture-vs-table cross-type rule
   (`_handle_cross_type_overlaps`), and dropping a regular region absorbed by a
   table/index/picture so it isn't emitted twice. With this, table_mislabeled's
   survey over-detection dropped sharply at the time (108 → 88 vs groundtruth;
   54 today — over-detection remains its dominant blocker), and
   redp5110's table-of-contents is now classified and rendered as a **table**
   (`document_index`) instead of a picture. The TOC table's remaining diff is a
   TableFormer dot-leader column-matching gap — later found to be largely
   word-cell tokenization (the create_word_cells port took redp5110 164→73)
   — tracked with the other
   table-structure work in
   [#60](https://github.com/docling-project/docling.rs/issues/60). *(The
   per-fixture byte counts quoted in this item are from its era; the committed
   snapshots and the Current-state table above are regenerated with every
   parity change.)*
3. **Complex title-page reading order**
   ([#62](https://github.com/docling-project/docling.rs/issues/62)). Author-block
   / abstract interleaving on the academic papers (band reading-order handles the
   full-width title; the in-column author/abstract order is still off). Two
   pieces landed: the suspected "TeX-font quote decode" gap turned out to be
   docling-parse's *sanitizer* table (every curly quote → `'`; a `"` only ever
   comes from a literal `quotedbl` glyph) — no font-program parsing needed —
   and region cells now join in docling-parse index order (docling's
   `_sort_cells`), which fixes off-baseline glyph drift like 2206's inline
   math `>` landing on the wrong line.
4. **amt fraction double space (text-layer, strict-only)**
   ([#63](https://github.com/docling-project/docling.rs/issues/63)). docling boxes glyphs
   with the embedded font's OS/2 typographic metrics, not the PDF descriptor's;
   that ~0.3 pt difference makes its justified line insert a *second* space before
   the `1⁄4` numerator. Our single-spaced output is the more faithful rendering
   (the whitespace-normalized metric credits it); reproducing docling's exact
   spacing needs an embedded-font metrics layer, which globally entangles with RTL
   box geometry (a trial that fixed one `¼` regressed `right_to_left_01`). See
   `MIGRATION.md` §4. **Resolved as by-design:** our single space is the correct
   rendering, so #63 is closed without matching docling's spurious extra space —
   forcing a byte-match would degrade output and risk the RTL geometry.

---

## Performance — review & profiling notes

Post-migration review of the PDF processing path: where the time actually goes,
what was measured, which optimizations are validated, and a ranked backlog of
further ideas that do **not** trade away output quality.

### Results at a glance

Everything below was landed across two optimization rounds (PR #26, #27),
each change gated on corpus conformance — groundtruth distance unchanged or
better, byte-identical where the change is structural:

| Optimization | Measured effect |
|---|---|
| INT8 layout model (Conv-only static QDQ, calibrated; **default**) | layout inference **2.4×** faster; **1.83× end-to-end** on a 1913-page PDF (0.74 → 0.40 s/page) |
| INT8 TableFormer decoder (dynamic, **default**) | ~10% faster table decode, byte-identical |
| SIMD page downscale (`fast_image_resize`, same kernel; **default**) | `image.resize` stage **17×** faster (2607 → 152 ms / 16 pages) |
| TableFormer KV cache fed back as `ort` values (no per-step copy) | ~9% faster table-structure decode, byte-identical |
| One shared lazy TableFormer across the worker pool | peak RSS **3.8 → 1.9 GB** (4 workers); table-free docs 682 → 331 MB |
| Single shared line/word contraction pass | `--no-ocr` conversion ~1.25× faster, identical output |
| Per-document font + form caches in the text parser | 3–10% off `textparse` here; far more on CJK/form-heavy PDFs |
| True-KV-cache decoder export (`decoder_kv.onnx`, optional) | parity at corpus table sizes; O(past)/step for very large tables |

Cumulative head-to-head vs Python docling (measured on an 8-thread desktop,
`scripts/test/performance.sh`): **4.3× faster warm conversion, 4.7× end-to-end,
2.3–2.6× less peak memory** on the PDF ML pipeline — up from ~1.2× warm
before this work. Model sizes: layout 172 → 68 MB, TF decoder 78 → 50 MB.

A re-measure on the final stack (hoisted-KV TableFormer decoder default per
#97; different desktop, Jul 2026) with `performance.sh
tests/data/pdf/sources/2305.03393v1-pg9.pdf` — a single table-dense page, so
the page-parallel worker pool sits mostly idle and this bounds the *low* end
of the speedup range: **3.0× end-to-end** (16.3 → 5.4 s avg over 5 runs),
**2.0× warm conversion** (9.1 → 4.7 s/doc), **1.9× less peak memory**
(1589 → 857 MB).
Also fixed along the way: the `"` show-text operator dropped its word/char
spacing operands (real spec violation), and OCR/TableFormer sub-stages are
now visible in `DOCLING_RS_TIMING` profiles.

Measured on a 4-core AVX-512(+VNNI/AMX) Xeon, release build (`lto = "thin"`),
models from `scripts/install/download_dependencies.sh`, `DOCLING_RS_TIMING=1`.

### Where the time goes

Per-stage wall-clock share (summed across workers):

| Stage | 1913-page text-heavy PDF¹ | 16-page table-heavy paper² | scanned page³ |
|---|---:|---:|---:|
| `layout.predict` (RT-DETR ONNX) | **80.3%** | 55.4% | 64.9% |
| `image.resize` (3×→2× CatmullRom) | 14.9% | 7.9% | 18.5% |
| `tableformer` | 2.8% | 32.1% | — |
| `pdfium.render` | 1.8% | 3.7% | 16.5% |
| `textparse` + assembly | ~0.2% | ~0.3% | ~0.1% |

¹ `tests/data/pdf/large/dotnet-csharp-language-reference.pdf` — 936 s wall, ~0.49 s/page.
² `tests/data/pdf/sources/2203.01017v2.pdf`.
³ `tests/data/scanned/sources/ocr_test.pdf`.

Two conclusions drive everything below:

1. **ONNX inference is ~85–95% of PDF conversion time.** All the Rust-side text
   extraction, parsing, and assembly work combined is under 1%. Rust-code
   micro-optimizations are irrelevant to PDF throughput until the models get
   faster; model-level and preprocessing-level changes are the only levers that
   matter.
2. Within TableFormer, the **autoregressive decode loop** dominates
   (`tableformer.structure` ≈ 96% of the stage; the per-table page resample
   `tableformer.inter_area` is ~1% of a conversion).

The worker-pool topology heuristic in `lib.rs` (`workers × intra ≈ cores`,
default 2×2 on 4 cores) was re-validated: 2×2 beat both 4×1 and 1×4 on the
16-page document (11.6 s vs 12.2 s vs 15.6 s; separate run from the INT8
table below, hence the ±0.1 s vs its 11.5 s).

### Validated win: INT8 quantization (quality-checked)

`scripts/install/quantize_models.py` produces two quantized models. Point
`DOCLING_LAYOUT_ONNX` / `DOCLING_TABLEFORMER_DECODER` at them to opt in.

**These are now the default:** when the `*_int8` files sit next to the fp32
models at the default paths, the pipeline loads them automatically.
`DOCLING_RS_FP32=1` forces full precision, and an explicit
`DOCLING_LAYOUT_ONNX` / `DOCLING_TABLEFORMER_DECODER` always wins (the
conformance/groundtruth scripts pin fp32 explicitly, so snapshots stay
deterministic).

#### Layout: static QDQ INT8, **Conv ops only** (~2.4× faster layout)

Calibrated on 42 real corpus pages preprocessed exactly like
`layout.rs::predict`. Only the HGNetv2 backbone convolutions are quantized;
the transformer decoder and detection-head MatMuls stay fp32.

| Configuration | layout.predict (16-page doc)¹ | end-to-end wall | model size |
|---|---:|---:|---:|
| fp32 baseline | 17.2 s | 16.6 s | 172 MB |
| **INT8 conv-only** | **7.2 s (2.4×)** | 11.5 s (1.45×) | 68 MB |
| + INT8 TableFormer decoder | — | 12.3 s² | — |

¹ `layout.predict` is summed across the parallel page workers, so it can
exceed the end-to-end wall.
² Separate run; within run-to-run noise of the 11.5 s conv-only wall — the
INT8 decoder's own win is per-table (~10 % faster tables, byte-identical;
see its section below), not end-to-end on this document.

On text-dominated documents (layout = 80% of time) the end-to-end gain
approaches ~1.7–2×; on table-heavy ones it is ~1.4×.

Full-scale run — the 1913-page `dotnet-csharp-language-reference.pdf`,
INT8 layout + INT8 TableFormer decoder vs fp32, same machine and binary,
back-to-back:

| | fp32 | INT8 | ratio |
|---|---:|---:|---:|
| wall clock | 1406 s (0.74 s/page) | **770 s (0.40 s/page)** | **1.83×** |
| `layout.predict` (summed) | 2667 s | 1350 s | 1.98× |
| output difference | — | 1199 of 52,615 Markdown lines (2.3%) | |

The 2.3% of differing lines are the same near-threshold classification flips
seen on the corpus (where groundtruth conformance measured *equal or slightly
better* under INT8 — 812 vs 833 summed diff-lines), not a systematic
degradation. With layout halved, `image.resize` becomes the next stage
(24.8% of the INT8 run), which is why backlog item 4 matters more after
quantization.

**Quality gate** (measured at INT8-selection time over the then-23-file
PDF+scanned corpus):

- Conv-only INT8: 12/23 byte-identical to fp32; remaining diffs are small
  region-classification flips. Against the committed groundtruth the summed
  diff-line distance is **812 (INT8) vs 833 (fp32)** — i.e. conformance-neutral
  (INT8 is marginally better on 3 fixtures, marginally worse on 2).
- Full INT8 (convs + MatMuls) was **rejected**: 3/23 exact, with clear quality
  loss (section headers demoted to plain text, page-footer text leaking into
  the output) — the RT-DETR head's class scores sit near the 0.3 threshold and
  cannot tolerate activation quantization.
- Dynamic (weights-only) INT8 of the whole layout model was also rejected: it
  is *slower* than fp32 (3.2 s vs 2.1 s per page-with-table) because inserted
  per-activation quantize ops outweigh the MatMul savings while the conv
  backbone stays fp32.

#### TableFormer decoder: dynamic INT8 (~10% faster tables, byte-identical)

The autoregressive tag decoder is MatMul-only; weights-only dynamic INT8
produced **byte-identical corpus output** and ~10% faster table decode
(784 → 695 ms/table), 78 → 50 MB. Small but free.

The decoder speed is *not* weight-bound — it is per-step overhead (see backlog
item 2), which is why quantization helps so little there.

### GPU execution providers (#74) — validated on GPU (#108)

The ONNX sessions (layout, TableFormer×3, OCR recognition, both enrichment
models) accept alternative ONNX Runtime execution providers behind cargo
features: `cuda`, `tensorrt`, `directml` (Windows), `coreml` (macOS). CPU
stays the default in every configuration — the features only compile a
provider in; `DOCLING_RS_EP` selects one at runtime:

| `DOCLING_RS_EP` | behavior |
|---|---|
| unset | `auto` in a build with any GPU feature compiled in (a GPU build should use the GPU); CPU in a default build |
| `cpu` | CPU, byte-for-byte the pre-#74 code path (no EP registered) |
| `cuda` \| `tensorrt` \| `directml` \| `coreml` | that provider, **error-on-failure**: an explicitly requested accelerator that can't initialize fails the conversion instead of silently degrading to a 10×-slower CPU run; requesting one that isn't compiled in warns once and stays on CPU |
| `auto` | every compiled-in provider registered in order TensorRT → CUDA → CoreML → DirectML; ONNX Runtime falls back down the list to CPU at session creation (for images deployed on mixed fleets) |

When a GPU provider is selected the model resolution skips the int8 defaults
in favor of fp32 (`decoder_kv.onnx` stays preferred): the int8 exports are
QDQ graphs calibrated for CPU kernels — on GPU they add de-quantize traffic
and their conformance was only ever validated on CPU. An explicit
`DOCLING_*_ONNX` path override still wins over this policy.

Verified without GPU hardware (this is what CI's `ep-features` matrix
covers): default/`cpu`/`auto`/unknown/uncompiled-request configurations all
produce byte-identical corpus output on a CPU-only build; on a
`--features cuda` build with no usable CUDA, `auto` falls back to CPU with
fp32 models selected (output byte-identical to `DOCLING_RS_FP32=1`) and
`DOCLING_RS_EP=cuda` fails loudly at the first session load. In CLI batch
mode (`--input`/`--output`) that first failure aborts the whole batch —
every remaining PDF would fail identically, so they are reported as
`skipped` instead of producing one error line per file.

#### Measured on real hardware (issue #108)

`scripts/test/gpu_benchmark.sh` — every corpus PDF (+ the scanned set)
under `cpu` and `cuda`, best of 3 cold CLI runs each, outputs
byte-compared. Machine: **NVIDIA GeForce RTX 3080 Laptop (16 GB), driver
566.07 · AMD Ryzen 9 5900HX, 16 logical cores** (both providers on the
fp32 models, per the policy above).

**Output equivalence:** 21 of 22 fixtures byte-identical to CPU; one
(`2203.01017v2`, the heaviest layout) differs by 2 markdown lines — fp32
CUDA kernels are not bit-identical to fp32 CPU kernels, so a borderline
detection can flip; groundtruth-distance parity is the standard here, and
byte-parity on 21/22 exceeds it. The entire 2-line diff is one label flip
on one borderline region: the caption fragment
`c. Structure predicted by TableFormer:` comes out as a `list_item`
(`- c. …`) on CPU and as plain `text` on CUDA — same content, same
position, one class score straddling the 0.3 threshold.

**Corpus total (best-of-3): CPU 124.5 s · CUDA 101.2 s → 1.23×** — but the
aggregate hides a clean size split:

| segment | speedup (best) |
|---|---|
| multi-page digital (9–39 pages: arXiv papers, redp5110) | **1.5–2.1×** (`2305.03393v1`: 13.6 s → 7.0 s) |
| mid-size digital (4–5 pages) | 1.1–1.3× |
| 1–2-page digital | 0.75–1.0× — CUDA EP init + host↔device traffic never amortizes |
| scanned/OCR-heavy | 0.65–0.85× — dominated by pdfium render + OCR pre/post on CPU |

The corpus is small-document-biased; on a genuinely large document the
init noise vanishes and the ONNX stages dominate — that is the regime the
GPU features exist for. The 1913-page .NET C# language reference (same
machine, single cold run each):

| provider | wall time | speedup |
|---|---|---|
| `cpu` | 15 min 13 s (767 % CPU) | — |
| `cuda` | **1 min 45 s** (321 % CPU) | **8.7×** |

Practical guidance: the break-even for a cold CLI run sits around 3–4
pages. Below that, or for OCR-heavy scans, stay on CPU; for batches or
services use the warm `Pipeline` / `docling-serve`, which pays EP
initialization once per process instead of once per file and moves the
break-even to roughly "any document with a table". The cold-vs-best gap on
the CUDA column (~1.5–2.5 s) is that per-process EP initialization made
visible. (Timing methodology: the script reads the monotonic clock —
wall-clock `date` proved able to step backwards under NTP mid-benchmark.)

<details>
<summary>Per-file results (seconds, best of 3; cold = run 1 incl. model/EP init)</summary>

| file | cpu cold | cpu best | cuda cold | cuda best | speedup (best) | output |
|---|---|---|---|---|---|---|
| 2203.01017v2 | 19.93 | 18.13 | 13.31 | 10.38 | 1.75x | 2 diff lines |
| 2206.01062 | 15.34 | 14.84 | 10.86 | 9.70 | 1.53x | identical |
| 2305.03393v1-pg9 | 3.87 | 3.87 | 6.25 | 3.98 | 0.97x | identical |
| 2305.03393v1 | 13.55 | 13.55 | 9.94 | 7.01 | 1.93x | identical |
| amt_handbook_sample | 2.63 | 2.50 | 4.74 | 3.26 | 0.77x | identical |
| code_and_formula | 3.13 | 3.13 | 5.14 | 3.09 | 1.01x | identical |
| multi_page | 5.12 | 5.12 | 6.24 | 4.30 | 1.19x | identical |
| normal_4pages | 7.67 | 6.34 | 7.04 | 4.76 | 1.33x | identical |
| picture_classification | 2.56 | 2.56 | 5.24 | 2.95 | 0.87x | identical |
| redp5110_sampled | 16.53 | 16.53 | 11.73 | 8.05 | 2.05x | identical |
| right_to_left_01 | 1.94 | 1.94 | 5.01 | 2.59 | 0.75x | identical |
| right_to_left_02 | 2.03 | 2.03 | 4.64 | 2.68 | 0.76x | identical |
| right_to_left_03 | 3.26 | 3.26 | 5.95 | 4.09 | 0.80x | identical |
| skipped_1page | 3.62 | 3.38 | 4.79 | 2.96 | 1.14x | identical |
| skipped_2pages | 3.99 | 3.75 | 5.89 | 3.32 | 1.13x | identical |
| table_mislabeled_as_picture | 4.67 | 4.48 | 6.71 | 4.51 | 0.99x | identical |
| nemotron_multipage | 4.83 | 4.76 | 9.16 | 6.18 | 0.77x | identical |
| ocr_test | 2.53 | 2.50 | 5.22 | 3.34 | 0.75x | identical |
| ocr_test_rotated_180 | 2.64 | 2.64 | 4.27 | 3.10 | 0.85x | identical |
| ocr_test_rotated_270 | 2.35 | 2.29 | 3.50 | 3.50 | 0.65x | identical |
| ocr_test_rotated_90 | 2.37 | 2.35 | 5.46 | 3.48 | 0.68x | identical |
| sample_with_rotation_mismatch | 4.54 | 4.54 | 4.74 | 3.92 | 1.16x | identical |

</details>

### Ranked backlog of further ideas

Ordered by expected impact ÷ risk. Items 1–3 attack the 85–95%.

1. ~~**Ship/document the INT8 layout model as the default CPU
   configuration**~~ **Done on this branch:** the pipeline prefers the int8
   models when present (`DOCLING_RS_FP32=1` opts out),
   `download_dependencies.sh` fetches them by default, and
   `publish-models.yml` builds them. Biggest single validated win: ~1.4–2×
   end-to-end.
2. **TableFormer decode-loop overhead** (~800 ms/table, ~60–500 steps):
   - ~~`decode_step` copies the whole KV cache out (`ocache.to_vec()`) and back
     in every step — O(steps²·6·512) float traffic.~~ **Done on this branch:**
     the cache and the encoder's cross-K/V + `enc_out` stay owned `ort` values
     fed straight back into the next run (~9% faster structure decode,
     byte-identical output).
   - ~~The exported graph still re-embeds the **full tag sequence** every
     step.~~ **Built and measured:** `scripts/install/export_tableformer.py` now also
     exports `decoder_kv.onnx`, a true-KV-cache step (one tag in, projected
     K/V cached per layer), verified argmax-identical over a 64-step rollout
     and byte-identical on corpus output. Measured result: **parity** with
     the legacy graph on corpus-sized tables (~100–300 tokens) — ONNX Runtime
     executes the legacy graph's full-prefix re-projection as one efficient
     batched GEMM, so the O(n²) FLOPs don't become O(n²) wall time until
     tables get much larger. The Rust loop auto-detects the graph generation
     (input names) and prefers `decoder_kv(_int8).onnx` by default; point
     `DOCLING_TABLEFORMER_DECODER` at the legacy `decoder(_int8).onnx` to
     trade speed back for the smaller file. **#97** rebuilt the KV step
     graph around hoisted cross-attention: the stacked `cross_k`/`cross_v`
     inputs made every decode step re-`Split` and re-`Transpose` 2×9.6 MB of
     constants (~5 ms of a ~7.5 ms step, measured with the ORT node
     profiler); the encoder now emits each layer's `cross_kt_i` (pre-transposed
     for q·Kᵀ) and `cross_v_i` once per table and the step graph consumes
     them in place. Per-step decode fell 17 → 10 ms and `tableformer.structure`
     1.40 → 0.91 s on the huge-table page (2305.03393v1-pg9, fp32); the
     remaining step cost is real compute (28 small projection/FFN GEMMs).
     Output stays byte-identical (the full snapshot corpus, 94 outputs; the export
     self-verifies a 64-step argmax-identical rollout vs the legacy graph).
     Export subtlety: the example inputs must carry `past>0` or
     `torch.export` specializes `pe[cache.shape[3]]` to `pe[0]` and decode
     never terminates. Old stacked-KV and legacy graphs keep working (three
     generations auto-detected); a hoisted decoder with a pre-#97 encoder
     falls back to geometric tables with a re-export hint.
3. ~~**Layout batching for the parallel path**: the pool currently runs batch-1
   inference per page.~~ **Done (issue #73)**: each pool worker drains the work
   channel opportunistically (whatever is already rendered, up to
   `DOCLING_RS_PDF_LAYOUT_BATCH` — default 4 on 8+ cores, 1 below) and
   layout-detects the batch with one inference call — batching never *waits* for pages, so it adds no
   latency when rendering is the bottleneck. Needs the dynamic-batch ONNX
   export (`scripts/install/export_layout.py`); an old fixed-batch graph
   triggers a warn-once per-page fallback. Two export subtleties keep numerics
   identical to the historical static export: a plain `dynamic_axes` export
   leaves the AIFI sincos position embedding as runtime ops that drift ~1e-6
   from the torch-folded constant (enough to flip borderline detections
   corpus-wide — groundtruth exact matches dropped 5/14 → 0/14 before the
   fix), so the exporter folds the static graph's position-embedding subgraph
   offline and splices the constant into the dynamic graph. Verified at the
   time: groundtruth parity restored (then 5/14 exact, 6/14 normalized), and
   batch=1 == batch=4 **bit-identical** across the whole corpus.
4. **The 3×→2× page downscale** (~15% of a text-heavy conversion, ~25% after
   INT8): ~~replace the scalar `image`-crate CatmullRom with a SIMD
   convolution.~~ **Done on this branch:** `fast_image_resize` with the same
   a=-0.5 Catmull-Rom kernel — `image.resize` drops **2607 → 152 ms (17×)**
   on the 16-page doc. The SIMD fixed-point path differs from the scalar one
   by ±1/255 on some pixels, which can flip borderline table cells, so it was
   gated like INT8: groundtruth distance over the corpus is **817 (SIMD) vs
   818 (scalar)** — conformance-neutral. `DOCLING_RS_SLOW_RESIZE=1` restores
   the scalar path, and `pdf_conformance.sh`/`pdf_groundtruth.sh` pin it so
   the committed snapshot baselines stay valid. (The render-side `as_image()`
   copy turned out to be a non-issue: pdfium already renders with reversed
   byte order, so it is one memcpy + one 4→3-channel pass, ~1% of total.)
5. **textparse font caching** (marginal for PDFs — textparse is ≤1% — but
   real for `no_ocr` mode where it becomes the bottleneck):
   - ~~fonts are fully re-parsed for **every page** and every Form-XObject
     invocation; decoded form content re-inflated per `Do`.~~ **Done on this
     branch:** per-document caches keyed by object id (fonts also by resource
     name, which feeds the docling-parse font hash). Identical output across
     the corpus; 3–10% off the `textparse` stage on the test fixtures (their
     ToUnicode CMaps are small — CJK/form-heavy documents benefit far more).
   - ~~`line_cells` + `word_cells` re-built the char cells twice per page.~~
     **Done** (`dp_lines::line_and_word_cells`): the glyph build is shared;
     the line and word views each run their own contraction (docling-parse's
     `create_line_cells` / `create_word_cells` pair — deliberately two
     passes, since the factors differ).
   - `decode_code`/`decompose_ligatures` allocate a `String` per glyph
     (`textparse.rs`); decompose once at font-parse time and return
     borrowed `&str`.
   - RTL merge is O(n²) (string prepend in `merge_with`, `dp_lines.rs`);
     accumulate reversed and flip once per line.
6. ~~**OCR line batching** (`ocr.rs::recognize`): lines are recognized one at
   a time on one thread (deliberately, for CTC determinism). Batching
   same-width buckets keeps determinism per line.~~ **Done on this branch**
   (`ocr.rs::recognize_batch`): each page's line crops are gathered first and
   equal-width lines share one recognition run (page order, batches capped at
   16). Same-width batching is **bit-identical** to sequential runs (verified:
   max output diff 0.0 over the scanned corpus's crops); the snapshot corpus
   is unchanged. Measured `ocr.page`: 195 → 176 ms on `ocr_test.pdf`, 682 →
   587 ms over `nemotron_multipage.pdf`'s 4 pages (−10–14%). The
   "several-fold" hope required *padded* batches (PaddleOCR-style, pad to
   bucket max): measured on the real crops, padding perturbs the valid
   region's probabilities by up to 0.34 through the model's global-attention
   blocks and changes the decoded text on 16/20 lines — off the table for a
   byte-stable pipeline. The remaining lever is running same-width buckets
   across the page-worker pool's idle threads (needs one extra session per
   worker: `ort`'s `Session::run` takes `&mut self`).
7. **ort session options**: checked — ONNX Runtime's C-API default is already
   `ORT_ENABLE_ALL`, so an explicit optimization level gains nothing.
   `with_optimized_model_path` (caching the optimized graph on disk) could
   still shave per-worker model-load latency; only worth it if pool spin-up
   shows up in a real deployment.

### Memory

Each pool worker used to own a full model set, so peak RSS scaled with the
pool: on a 4-worker machine ~0.4 GB of TableFormer weights+arenas were
duplicated four times even though tables appear on a minority of pages. The
pool now shares **one lazily-loaded TableFormer** behind a mutex (loaded with
the full intra-op budget, since tables serialise on it anyway; prediction is
independent of which worker runs it). Measured on the 16-page table-heavy
paper, INT8 stack:

| pool | per-worker TF (before) | shared TF (after) |
|---|---:|---:|
| 4 workers | 3816 MB | **1880 MB** |
| 2 workers | 2183 MB | **1517 MB** |
| 4 workers, table-free doc | 682 MB | **331 MB** (TableFormer never loads) |

`DOCLING_RS_PDF_WORKERS` remains the coarse memory knob on top.

### Determinism note (pre-existing, worth knowing)

Multi-threaded ONNX Runtime float reductions are **not deterministic
run-to-run**: on `2203.01017v2.pdf` two identical invocations of the same
binary can differ in a handful of borderline table cells (measured 0–20
Markdown diff-lines between repeat runs, before any of this branch's
changes). `ocr.rs` already pins its session to one thread for exactly this
reason. Regression checks for structural changes should therefore compare
outputs under `DOCLING_RS_PDF_THREADS=1` (single-thread inference is
deterministic and byte-stable); multi-threaded corpus diffs of a few lines on
table-dense fixtures are thread-scheduling jitter, not necessarily a real
change.

### Correctness notes found during review (quality, not speed)

- `textparse.rs` `"` operator: the `aw ac string "` form must set word/char
  spacing (`tw`/`tc`) from its first two operands before showing the string;
  they are currently ignored (`Tj | ' | "` share one arm), so documents using
  `"` get wrong inter-word advances. **Fixed in this branch.**
- `textparse.rs::page_size` ignores a non-zero MediaBox origin; a page with
  e.g. `[9 9 621 801]` offsets all parser cells relative to pdfium's raster.
  Rare, but cheap to guard: subtract the box origin when emitting glyph boxes.
- OCR recognition ran un-instrumented; `ocr.page` is now a timed stage (this
  branch), so scanned-corpus profiles attribute it correctly.

### Reproducing

```bash
scripts/install/download_dependencies.sh
cargo build --release

# stage timing
DOCLING_RS_TIMING=1 ./target/release/docling-rs input.pdf > /dev/null

# build the int8 models (used automatically once present)
uv venv .venv-quant && uv pip install --python .venv-quant/bin/python \
    onnx onnxruntime sympy pypdfium2 pillow numpy
.venv-quant/bin/python scripts/install/quantize_models.py

# force full precision for a run
DOCLING_RS_FP32=1 ./target/release/docling-rs input.pdf > /dev/null
```

Integration points: `scripts/install/download_dependencies.sh` fetches the
pre-quantized assets by default (`--no-int8` skips; published by
`.github/workflows/publish-models.yml`, which quantizes after export);
`scripts/install/pdf_setup.sh` quantizes locally unless `DOCLING_RS_FP32=1`;
`scripts/test/performance.sh` benchmarks whatever the pipeline default resolves to
(int8 when present, `DOCLING_RS_FP32=1` for fp32); `examples/Dockerfile`
bakes both precisions and defaults to int8 (`--build-arg INT8=0` for fp32).
