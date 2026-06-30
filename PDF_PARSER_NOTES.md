# Pure-Rust PDF text parser — WIP notes & roadmap

Goal: replace pdfium's text-extraction layer with a pure-Rust parser whose
character cells match docling's `docling-parse` C++ parser, so the PDF pipeline
can reach docling byte-conformance (and eventually drop pdfium for text — pdfium
would stay only for page rasterisation).

## Why (the measured case)

docling-parse and pdfium disagree on glyph geometry at exactly the points that
break conformance: pdfium gives generated **spaces a zero-width box**, gives
**combining diacritics a real-width box**, and lands ligature/fraction glyphs at
different x. A ceiling experiment — injecting docling-parse's own cells into our
pipeline (keeping our layout + TableFormer) — measured:

| Cells used | Exact |
|---|---|
| pdfium (baseline) | 4/14 |
| docling-parse cells injected | **6/14** (amt + right_to_left_01 flip to exact) |
| + the one `right_to_left_02` `11`-page-number layout fix | **7/14 = 50%** |

So the text parser is the lever; 50% is reachable.

## What's built (`crates/fleischwolf-pdf/src/textparse.rs`)

Opt-in via `DOCLING_RUST_PARSER=1` (default pipeline is unchanged). Pdfium still
provides page rasters + word/code cells; the parser only replaces prose line
cells, fed through the existing `dp_lines` sanitizer.

- Content-stream interpreter: `cm/q/Q`, `BT/ET`, `Tf/Td/TD/Tm/T*/Tc/Tw/Tz/TL/Ts`,
  `Tj/TJ/'/"` with text + graphics matrices.
- **Advance-width geometry** from the font (spaces get real width; combining
  marks get zero advance) — the whole point.
- Fonts: Type0/CID + Identity-H (`/W`, `/DW`), simple Type1/TrueType
  (`/FirstChar`+`/Widths`, `/MissingWidth`), FontDescriptor ascent/descent.
- Encodings: ToUnicode CMap (`bfchar`, `bfrange` scalar **and** array forms,
  structural tokenizer for back-to-back `<..><..>` hex); WinAnsi + MacRoman base
  encodings; `/Differences` via a small Adobe-glyph-name subset.

## Current result: 3/14 (matches pdfium's text quality)

`code_and_formula`, `multi_page`, `picture_classification` exact; `amt`=2,
`right_to_left_01`=2 (same as pdfium). The parser extracts Latin + Arabic
correctly and no longer regresses any text-exact file.

## Why it isn't 6/14 yet — the next lever is the SANITIZER

amt/rtl_01 are stuck at 2 **identical to pdfium**, because their remaining diffs
(the justified tanwin spacing, the fraction line-wrap double space) are produced
by the `dp_lines` sanitizer, which is shared by both the pdfium and Rust paths.
The 6/14 ceiling used docling-parse's *post-sanitizer* textlines. So reaching it
needs `dp_lines` to match docling-parse's C++ contraction on those cases — a
separate fidelity effort, independent of the parser.

## Roadmap

1. **Sanitizer fidelity** (`dp_lines.rs`): reproduce docling-parse's tanwin /
   combining-mark spacing and the line-wrap double-space → amt + rtl_01 exact → 6/14.
2. **`right_to_left_02` layout**: the top `11` page number is mis-classified as a
   picture and the recovered orphan lands at the bottom; docling labels it `text`
   first → fix → 7/14.
3. Parser hardening for the heavy docs (2203/2206/redp5110): more font edge cases,
   reading order; validate against docling-parse char cells per file.
4. Once the parser matches pdfium everywhere, make it the default and drop
   pdfium's text path (keep pdfium for rasterisation only).

## Tooling (under `scripts/`)

- `dump_parse_cells.py` — docling-parse textline cells → JSON/TSV (the oracle).
- `docling_dump_all.py` — full docling items (label/page/bbox/text) per PDF.
- `textparse_dump` example — the Rust parser's cells; `TSV_OUT=1` emits the
  injection TSV for ceiling experiments.

Also in this branch: `assemble::add_orphan_regions` — docling-parity orphan-cell
clustering (emits text the layout detector missed, e.g. amt's stray `.`).
