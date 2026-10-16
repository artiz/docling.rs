//! Port of docling-parse's line-cell sanitizer
//! (`src/parse/page_item_sanitators/cells.h` → `create_line_cells` /
//! `contract_cells_into_lines_v1`). It merges per-glyph char cells into line
//! cells via a 3-pass contraction — left-to-right, right-to-left, then
//! left-to-right with reverse — using corner-distance adjacency and inserting at
//! most one space per merge. This reproduces docling-parse's inter-word spacing
//! (justified double spaces, the space before a `:`, and RTL ordering) that an
//! ad-hoc gap-heuristic reconstruction can't.
//!
//! Geometry uses native PDF coordinates (y increases upward); each cell carries
//! its four transformed corners r0=bottom-left, r1=bottom-right, r2=top-right,
//! r3=top-left, exactly like `page_cell.h`.

use crate::pdfium_backend::{Glyph, TextCell};

// config.h: the factors that actually bind for line cells.
const MERGE: f64 = 1.0; // line_space_width_factor_for_merge (adjacency gate)
const MERGE_WITH_SPACE: f64 = 0.33; // line_space_width_factor_for_merge_with_space

// create_word_cells: words contract under their own, tighter factors — the
// adjacency gate is word_space_width_factor_for_merge (0.33) and the space
// threshold is twice that (2.0 * 0.33), which the 0.33 gate can never exceed,
// so a word cell never contains an inserted space. Space glyphs are hard
// word-boundary barriers during the contraction (`applicable`'s
// `block_spaces`) and are erased only afterwards, as docling-parse does.
const WORD_MERGE: f64 = 0.33; // word_space_width_factor_for_merge
const WORD_MERGE_WITH_SPACE: f64 = 2.0 * WORD_MERGE;
const H_TOL: f64 = 1.0; // horizontal_cell_tolerance (ligature eps_d1 relaxation)

#[derive(Clone)]
struct Cell {
    text: String,
    rx0: f64,
    ry0: f64, // bottom-left
    rx1: f64,
    ry1: f64, // bottom-right
    rx2: f64,
    ry2: f64, // top-right
    rx3: f64,
    ry3: f64, // top-left
    ltr: bool,
    active: bool,
    lig_carry: bool, // last_merged_cell_was_ligature
    font: u64,       // hash of docling-parse's font name (for enforce_same_font)
    // Cached invariants of `text` / the quad, maintained by `build_cells` and
    // `merge_with`. The contraction is quadratic in merge attempts, and
    // recomputing these per attempt (trim/char-count/ligature scans over the
    // *growing* line text, min/max over the quad) dominated large-page
    // parsing — the caches turn every attempt into flag reads.
    blank: bool,   // text.trim().is_empty()
    lig: bool,     // is_ligature(&text)
    fb: bool,      // any char in U+FB00..=U+FB06 (the range half of is_ligature)
    nchars: usize, // text.chars().count()
    b_l: f64,      // bounds(): axis-aligned quad extremes
    b_r: f64,
    b_b: f64,
    b_t: f64,
}

impl Cell {
    /// Length of the bottom edge (baseline advance) — `page_cell.h::length`.
    fn length(&self) -> f64 {
        ((self.rx1 - self.rx0).powi(2) + (self.ry1 - self.ry0).powi(2)).sqrt()
    }

    /// Running mean glyph advance over the whole accumulated cell.
    fn avg_char_width(&self) -> f64 {
        if self.nchars > 0 {
            self.length() / self.nchars as f64
        } else {
            0.0
        }
    }

    /// Distance from this cell's bottom-right corner to `other`'s bottom-left.
    fn gap(&self, other: &Cell) -> f64 {
        ((self.rx1 - other.rx0).powi(2) + (self.ry1 - other.ry0).powi(2)).sqrt()
    }

    /// `is_adjacent_to`: both the bottom-corner gap (`< eps0`) and the top-corner
    /// gap (`< eps1`) must be small. The vertical component keeps different
    /// baselines/lines from merging.
    fn adjacent(&self, other: &Cell, eps0: f64, eps1: f64) -> bool {
        let d0 = self.gap(other);
        let d1 = ((self.rx2 - other.rx3).powi(2) + (self.ry2 - other.ry3).powi(2)).sqrt();
        d0 < eps0 && d1 < eps1
    }

    /// Punctuation/space cells are bidi-neutral bridges.
    fn same_orientation(&self, other: &Cell) -> bool {
        self.ltr == other.ltr || is_punct_or_space(&self.text) || is_punct_or_space(&other.text)
    }

    /// `merge_with`: absorb `other` (which lies to this cell's right). Insert at
    /// most one separator space when the gap exceeds `delta`. RTL prepends.
    ///
    /// `euclidean` picks the gap measure: docling-parse uses the **Euclidean
    /// corner distance** `d0` (the same one `is_adjacent_to` uses). The pure-Rust
    /// parser produces clean advance boxes, so it uses `d0` to match docling
    /// byte-for-byte. pdfium's loose boxes overhang (an `f` extends left and
    /// overlaps its neighbour), which a Euclidean distance reads as a false
    /// positive gap and over-inserts spaces (`Self` → `Sel f`); that path keeps
    /// the **signed horizontal gap** instead.
    fn merge_with(&mut self, other: &Cell, delta: f64, euclidean: bool) {
        let gap = if euclidean {
            self.gap(other)
        } else {
            other.rx0 - self.rx1
        };
        let space = delta < gap;
        if !self.ltr || !other.ltr {
            if space {
                self.text.insert(0, ' ');
            }
            self.text = format!("{}{}", other.text, self.text);
            self.ltr = false;
        } else {
            if space {
                self.text.push(' ');
            }
            self.text.push_str(&other.text);
            self.ltr = true;
        }
        // Extend the right edge to `other`.
        self.rx1 = other.rx1;
        self.ry1 = other.ry1;
        self.rx2 = other.rx2;
        self.ry2 = other.ry2;
        // Cache upkeep. Blankness and the ligature char-range scan distribute
        // over concatenation (the inserted separator is whitespace and not in
        // the range); the equality patterns ("ff", "fi", …) do NOT — "fi" is a
        // ligature cell, "fiction" is not — so a short merged text recomputes
        // exactly and a longer one (which no equality pattern can match) falls
        // back to the distributed range flag alone.
        self.blank = self.blank && other.blank;
        self.nchars += other.nchars + usize::from(space);
        self.fb = self.fb || other.fb;
        self.lig = if self.text.len() <= 3 {
            is_ligature(&self.text)
        } else {
            self.fb
        };
        let (l, r, b, t) = quad_bounds(self);
        self.b_l = l;
        self.b_r = r;
        self.b_b = b;
        self.b_t = t;
    }
}

/// Axis-aligned bounds of a cell's quad, `(l, r, b, t)` in PDF points (y-up),
/// from the cache `merge_with` maintains.
fn bounds(c: &Cell) -> (f64, f64, f64, f64) {
    (c.b_l, c.b_r, c.b_b, c.b_t)
}

/// The cached bounds' source of truth: min/max over the quad corners.
fn quad_bounds(c: &Cell) -> (f64, f64, f64, f64) {
    let xs = [c.rx0, c.rx1, c.rx2, c.rx3];
    let ys = [c.ry0, c.ry1, c.ry2, c.ry3];
    let fold = |it: &[f64], f: fn(f64, f64) -> f64| it.iter().copied().reduce(f).unwrap();
    (
        fold(&xs, f64::min),
        fold(&xs, f64::max),
        fold(&ys, f64::min),
        fold(&ys, f64::max),
    )
}

/// Is another active cell painted inside the horizontal gap between `i` and
/// `j`? The contraction walks cells in **stream** order, and a generator that
/// draws a line's bold runs after its regular text leaves them as later cells
/// — the space tolerance would then stitch `C.[ ]Zur Wahrung …` straight
/// across the hole where the bold `6.` sits, and the stranded token ends up at
/// the line's end ("… wenn Sie die Mitteilung 6."). An occupied gap is not a
/// gap. Space-only cells never block (they *are* the gap), and the scan is
/// skipped entirely for glyph-adjacent merges (no room for anything).
fn gap_occupied(cells: &[Cell], i: usize, j: usize) -> bool {
    let (al, ar, ab, at) = bounds(&cells[i]);
    let (bl, br, bb, bt) = bounds(&cells[j]);
    let (gl, gr) = if ar <= bl { (ar, bl) } else { (br, al) };
    if gr - gl < 0.5 {
        return false; // touching or overlapping — nothing fits in between
    }
    let (band_b, band_t) = (ab.min(bb), at.max(bt));
    cells.iter().enumerate().any(|(k, c)| {
        if k == i || k == j || !c.active || c.blank {
            return false;
        }
        let (cl, cr, cb, ct) = bounds(c);
        // Vertically on this line: most of the candidate inside the pair's band.
        let overlap = (ct.min(band_t) - cb.max(band_b)).max(0.0);
        overlap > 0.5 * (ct - cb).max(f64::EPSILON)
            // Horizontally: real ink inside the gap interval.
            && cr.min(gr) - cl.max(gl) > 0.1
    })
}

/// `applicable_for_merge`: both active and same reading orientation. A different
/// font normally blocks the merge (keeps a bold label and its value as separate
/// line cells). On the clean-box parser path, **punctuation/space cells bridge
/// fonts** so a sentence period set in a separate punctuation font joins its word
/// instead of fragmenting (`العمل .` → `العمل.`); letters still enforce the font.
fn applicable(a: &Cell, b: &Cell, parser: bool, block_spaces: bool) -> bool {
    if !a.active || !b.active {
        return false;
    }
    // Word mode (`block_spaces`): a space glyph is a hard word-boundary barrier
    // that never merges in either direction; the space cells themselves are
    // erased after the contraction (`create_word_cells`).
    if block_spaces && (is_all_space(&a.text) || is_all_space(&b.text)) {
        return false;
    }
    // A lone punctuation glyph (not a space) set in a separate punctuation font
    // bridges fonts so it joins its word — but only next to RTL text. In LTR a
    // different-font punctuation (e.g. a bold `:`) is a real run boundary docling
    // keeps spaced (`Laboratories :`); in Arabic the sentence period sits in a
    // Latin punctuation font yet attaches (`العمل.`). Parser path only.
    let lone_punct = |s: &str| {
        let mut ch = s.chars();
        matches!(ch.next(), Some(c) if c != ' ' && is_punct_or_space(&c.to_string()))
            && ch.next().is_none()
    };
    let punct_bridge =
        parser && ((lone_punct(&a.text) && !b.ltr) || (lone_punct(&b.text) && !a.ltr));
    let font_neutral = a.lig || b.lig || punct_bridge;
    if a.font != 0 && b.font != 0 && a.font != b.font && !font_neutral {
        return false;
    }
    a.same_orientation(b)
}

/// Left-to-right pass: `i` ascending accumulates cells to its right.
fn pass_ltr(cells: &mut [Cell], allow_reverse: bool, euclidean: bool, p: Factors) {
    for i in 0..cells.len() {
        if !cells[i].active {
            continue;
        }
        let mut j = i + 1;
        while j < cells.len() {
            if !applicable(&cells[i], &cells[j], euclidean, p.block_spaces) {
                break;
            }
            let i_lig = cells[i].lig || cells[i].lig_carry;
            let j_lig = cells[j].lig || cells[j].lig_carry;
            let d0 = cells[i].avg_char_width() * p.merge;
            let d1 = cells[i].avg_char_width() * p.merge_with_space;
            let adj_d1 = d0 + if i_lig || j_lig { H_TOL } else { 0.0 };
            if cells[i].adjacent(&cells[j], d0, adj_d1) && !gap_occupied(cells, i, j) {
                let other = cells[j].clone();
                cells[i].merge_with(&other, d1, euclidean);
                cells[i].lig_carry = other.lig;
                cells[j].active = false;
                j += 1; // i keeps absorbing the next cell to its right
            } else if allow_reverse
                && cells[j].adjacent(&cells[i], d0, adj_d1)
                && !gap_occupied(cells, j, i)
            {
                let other = cells[i].clone();
                cells[j].merge_with(&other, d1, euclidean);
                cells[j].lig_carry = other.lig;
                cells[i].active = false;
                break; // i is consumed
            } else {
                break;
            }
        }
    }
}

/// Right-to-left pass: `i` descending; its immediate left neighbour `i-1`
/// absorbs it (then the outer loop continues leftward through the absorber).
fn pass_rtl(cells: &mut [Cell], euclidean: bool, p: Factors) {
    let n = cells.len();
    for k in 0..n {
        let i = n - 1 - k;
        if !cells[i].active || i == 0 {
            continue;
        }
        let j = i - 1;
        if !applicable(&cells[i], &cells[j], euclidean, p.block_spaces) {
            continue;
        }
        let i_lig = cells[i].lig || cells[i].lig_carry;
        let j_lig = cells[j].lig || cells[j].lig_carry;
        let d0 = cells[i].avg_char_width() * p.merge;
        let d1 = cells[i].avg_char_width() * p.merge_with_space;
        let adj_d1 = d0 + if i_lig || j_lig { H_TOL } else { 0.0 };
        if cells[j].adjacent(&cells[i], d0, adj_d1) && !gap_occupied(cells, j, i) {
            let other = cells[i].clone();
            cells[j].merge_with(&other, d1, euclidean);
            cells[j].lig_carry = other.lig;
            cells[i].active = false;
        }
    }
}

/// The contraction's tuning: the adjacency-gate and space-insertion factors
/// (per `sanitize_bbox`'s callers) plus the word mode's space barrier.
#[derive(Clone, Copy)]
struct Factors {
    merge: f64,
    merge_with_space: f64,
    block_spaces: bool,
}

const LINE_FACTORS: Factors = Factors {
    merge: MERGE,
    merge_with_space: MERGE_WITH_SPACE,
    block_spaces: false,
};
const WORD_FACTORS: Factors = Factors {
    merge: WORD_MERGE,
    merge_with_space: WORD_MERGE_WITH_SPACE,
    block_spaces: true,
};

/// True when the cell's text is entirely whitespace (`utils::string::is_space`).
fn is_all_space(s: &str) -> bool {
    !s.is_empty() && s.chars().all(char::is_whitespace)
}

fn contract(cells: &mut Vec<Cell>, euclidean: bool, p: Factors) {
    pass_ltr(cells, false, euclidean, p);
    cells.retain(|c| c.active);
    pass_rtl(cells, euclidean, p);
    cells.retain(|c| c.active);
    pass_ltr(cells, true, euclidean, p);
    cells.retain(|c| c.active);
}

/// Build per-glyph char cells from a page's glyph stream (shared by the line and
/// word paths): drop degenerate spaces, recompose ligatures, init word segments.
fn build_cells(glyphs: &[Glyph], euclidean: bool) -> Vec<Cell> {
    let mut cells: Vec<Cell> = Vec::new();
    for g in glyphs {
        // Use the loose box (uniform font ascent/descent + advance) so adjacent
        // glyphs share a top edge, matching docling-parse's `compute_rect`.
        if !g.ll.is_finite() {
            continue;
        }
        // The char cell's quad: the loose rectangle for upright text, the
        // glyph's own rotated quad otherwise (#528) — the contraction below
        // works on corners and edge lengths, so it reads a 90°/180°/270° line
        // in its own reading order, exactly like docling-parse.
        let q = g.quad.map(|q| q.map(f64::from)).unwrap_or_else(|| {
            let (l, b, r, t) = (g.ll as f64, g.lb as f64, g.lr as f64, g.lt as f64);
            [l, b, r, b, r, t, l, t]
        });
        // Drop *degenerate* space glyphs (zero-width loose box): pdfium's generated
        // spaces get a zero-width box at the wrong baseline that breaks the
        // corner-distance adjacency. Without them the inter-word gap drives
        // `merge_with`'s space insertion. Spaces with a real width are kept (they
        // carry justified double-space information). The width is the baseline
        // edge — a rotated glyph's x extent is its em height.
        if g.ch == ' ' && (q[2] - q[0]).hypot(q[3] - q[1]) < 0.5 {
            continue;
        }
        // Recompose a ligature: pdfium decomposes one font glyph (Latin fi/ffi,
        // Arabic lam-alef) into several chars at the *same* loose box. Append them
        // into one cell so the contraction never inserts a space inside it.
        // "Same box": both baseline corners match as points, upright or
        // rotated. A ligature's chars share one box, baseline included; glyphs
        // stacked one under another at the same x (a chart's y-axis ticks
        // `3`…`8`, each placed by its own `cm`) are separate cells — comparing
        // the x span alone glued those into one `345678` cell carrying the
        // first tick's box (#609), where docling-parse keeps six.
        if let Some(last) = cells.last_mut() {
            let near =
                |ax: f64, ay: f64, bx: f64, by: f64| (ax - bx).abs() < 0.5 && (ay - by).abs() < 0.5;
            if near(last.rx0, last.ry0, q[0], q[1]) && near(last.rx1, last.ry1, q[2], q[3]) {
                // Overprint duplicate: the *same* character re-stamped, offset by a
                // fraction of its width (a kashida/elongation segment re-drawn for
                // weight). docling-parse drops it; appending over-counts
                // (right_to_left_02's `قويووووة` vs `قويوووة`). Require a real offset
                // (> 0.1) so a ligature expansion — which decomposes one glyph into
                // several chars at the *identical* box (`ﬀ`→`ff`, diff ≈ 0) — is still
                // recomposed; real doubled letters sit a full advance apart (> 0.5).
                let offset = match g.quad {
                    None => (q[0] - last.rx0).abs(),
                    Some(_) => (q[0] - last.rx0).hypot(q[1] - last.ry0),
                };
                if euclidean && offset > 0.1 && last.text.ends_with(g.ch) {
                    continue;
                }
                last.text.push(g.ch);
                last.ltr = !is_right_to_left(&last.text);
                last.blank = last.blank && g.ch.is_whitespace();
                last.nchars += 1;
                // Recomposed ligature cells stay tiny; recomputing is exact.
                last.fb = last.fb || (0xFB00..=0xFB06).contains(&(g.ch as u32));
                last.lig = is_ligature(&last.text);
                continue;
            }
        }
        let text = g.ch.to_string();
        let ltr = !is_right_to_left(&text);
        let blank = g.ch.is_whitespace();
        let lig = is_ligature(&text);
        let fb = (0xFB00..=0xFB06).contains(&(g.ch as u32));
        cells.push(Cell {
            text,
            rx0: q[0],
            ry0: q[1],
            rx1: q[2],
            ry1: q[3],
            rx2: q[4],
            ry2: q[5],
            rx3: q[6],
            ry3: q[7],
            ltr,
            active: true,
            lig_carry: false,
            font: g.font,
            blank,
            lig,
            fb,
            nchars: 1,
            b_l: (g.ll as f64).min(g.lr as f64),
            b_r: (g.ll as f64).max(g.lr as f64),
            b_b: (g.lb as f64).min(g.lt as f64),
            b_t: (g.lb as f64).max(g.lt as f64),
        });
    }
    cells
}

/// Build line cells from a page's glyph stream via the docling-parse contraction.
pub(crate) fn line_cells(glyphs: &[Glyph], page_h: f32, euclidean: bool) -> Vec<TextCell> {
    line_and_word_cells(glyphs, page_h, euclidean).0
}

/// Build **word** cells from a page's glyph stream via docling-parse's
/// `create_word_cells`: a second contraction over the same char cells under the
/// word factors — adjacency gate 0.33 (vs the line's 1.0), so a gap wide enough
/// to become a line-internal space still merges glyphs into one spaceless word
/// when it stays under the gate (tight-set Korean: line `1군 감염병`, word
/// `1군감염병`); real space glyphs are hard barriers and are erased afterwards.
/// These are the per-word tokens TableFormer matches table-grid cells against.
pub(crate) fn word_cells(glyphs: &[Glyph], page_h: f32, euclidean: bool) -> Vec<TextCell> {
    line_and_word_cells(glyphs, page_h, euclidean).1
}

/// Build the line cells **and** the word cells from one shared glyph build:
/// the char cells are constructed once and contracted twice, under the line
/// factors and the word factors respectively — exactly docling-parse's
/// `create_line_cells` + `create_word_cells` pair.
pub(crate) fn line_and_word_cells(
    glyphs: &[Glyph],
    page_h: f32,
    euclidean: bool,
) -> (Vec<TextCell>, Vec<TextCell>) {
    let built = build_cells(glyphs, euclidean);
    let to_text_cell = |c: Cell| {
        let l = c.rx0.min(c.rx1).min(c.rx2).min(c.rx3) as f32;
        let r = c.rx0.max(c.rx1).max(c.rx2).max(c.rx3) as f32;
        let top = c.ry0.max(c.ry1).max(c.ry2).max(c.ry3) as f32;
        let bot = c.ry0.min(c.ry1).min(c.ry2).min(c.ry3) as f32;
        TextCell {
            text: c.text,
            l,
            t: page_h - top,
            r,
            b: page_h - bot,
        }
    };
    // Word run: docling-parse's `create_word_cells` order — copy the char
    // cells, contract (`sanitize_bbox`), *then* erase the spaces. The space
    // glyphs stay in the stream during the contraction, where `applicable`'s
    // `block_spaces` makes each one a hard word boundary; they are dropped
    // afterwards by the blank filter below. Filtering them out up front let
    // the 0.33 gate alone decide word breaks, which glues tight-set Latin
    // (`MODE` + `to`: 1.8 pt apart under a 2.0 pt gate → `MODEto`) and the
    // thin-spaced Korean docling-parse ≥ 7 keeps apart (`1군` `감염병`).
    let mut word_run: Vec<Cell> = built.clone();
    let mut cells = built;
    contract(&mut cells, euclidean, LINE_FACTORS);
    let lines = to_text_cells(cells, to_text_cell);
    contract(&mut word_run, euclidean, WORD_FACTORS);
    word_run.retain(|c| !c.text.trim().is_empty());
    let words = to_text_cells(word_run, to_text_cell);
    (lines, words)
}

/// Map the contracted cells into a freshly allocated, exactly sized vector.
///
/// A plain `into_iter().map(..).collect()` would take std's in-place-collect
/// path and hand back the *input's* allocation: one ~150-byte [`Cell`] slot per
/// glyph of the page, reinterpreted as ~4× as many 40-byte [`TextCell`] slots,
/// while the contraction leaves a few glyphs per cell. Callers keep these
/// vectors for the whole document (the text-layer path holds every page), so
/// the dead capacity added up to ~30× the cells' real size — gigabytes on a
/// long table-heavy PDF.
fn to_text_cells(cells: Vec<Cell>, f: impl Fn(Cell) -> TextCell) -> Vec<TextCell> {
    let mut out = Vec::with_capacity(cells.len());
    out.extend(cells.into_iter().map(f));
    out
}

fn is_rtl_char(c: char) -> bool {
    let ch = c as u32;
    (0x0600..=0x06FF).contains(&ch)
        || (0x0750..=0x077F).contains(&ch)
        || (0x08A0..=0x08FF).contains(&ch)
        || (0xFB50..=0xFDFF).contains(&ch)
        || (0xFE70..=0xFEFF).contains(&ch)
        || (0x0590..=0x05FF).contains(&ch)
        || (0xFB1D..=0xFB4F).contains(&ch)
        || (0x0700..=0x074F).contains(&ch)
        || (0x0780..=0x07BF).contains(&ch)
        || (0x07C0..=0x07FF).contains(&ch)
}

/// All codepoints are RTL-script (matches `string.h::is_right_to_left`).
fn is_right_to_left(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_rtl_char)
}

/// A single-codepoint punctuation/space cell (matches `string.h`).
fn is_punct_or_space(s: &str) -> bool {
    let mut chars = s.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return false;
    };
    if matches!(
        c,
        ' ' | '\t'
            | '\n'
            | '\r'
            | '\u{0c}'
            | '\u{0b}'
            | '.'
            | ','
            | ';'
            | ':'
            | '!'
            | '?'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '\''
            | '"'
            | '`'
            | '\u{2018}'
            | '\u{2019}'
            | '\u{201c}'
            | '\u{201d}'
            | '-'
            | '\u{2013}'
            | '\u{2014}'
            | '_'
            | '/'
            | '\\'
            | '|'
            | '@'
            | '#'
            | '%'
            | '&'
            | '*'
            | '+'
            | '='
            | '<'
            | '>'
    ) {
        return true;
    }
    let ch = c as u32;
    (0x2000..=0x206F).contains(&ch)
        || (0x3000..=0x303F).contains(&ch)
        || (0xFE50..=0xFE6F).contains(&ch)
        || (0xFF00..=0xFF0F).contains(&ch)
        || (0xFF1A..=0xFF1F).contains(&ch)
        || (0xFF3B..=0xFF5E).contains(&ch)
}

/// Ligature glyph or its ASCII spelling (matches `string.h::is_ligature`).
fn is_ligature(s: &str) -> bool {
    matches!(s, "ff" | "fi" | "fl" | "ffi" | "ffl")
        || s.chars().any(|c| (0xFB00..=0xFB06).contains(&(c as u32)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(ch: char, l: f32, r: f32) -> Glyph {
        Glyph {
            ch,
            l,
            b: 100.0,
            r,
            t: 110.0,
            ll: l,
            lb: 98.0,
            lr: r,
            lt: 110.0,
            font: 0,
            quad: None,
        }
    }

    /// Tight-set Latin (a table cell in a born-digital manual): the gap between
    /// `MODE` and `to` (1.84 pt) is under the 0.33 × average-width word gate (2.0 pt), so
    /// only the space glyph between them keeps the words apart. docling-parse
    /// erases spaces after the word contraction, not before.
    #[test]
    fn space_glyph_separates_tight_words() {
        let glyphs = [
            glyph('M', 137.66, 143.74),
            glyph('O', 143.74, 149.82),
            glyph('D', 149.82, 155.90),
            glyph('E', 155.90, 161.98),
            glyph(' ', 161.84, 164.09),
            glyph('t', 163.82, 166.60),
            glyph('o', 166.60, 171.60),
        ];
        let (lines, words) = line_and_word_cells(&glyphs, 792.0, true);
        let words: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(words, ["MODE", "to"]);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "MODE to");
    }

    /// #609: a chart's y-axis ticks set one under another at the same x (each
    /// tick placed by its own `cm`, same `Tm`) are six line cells with their
    /// own boxes, as in docling-parse — not one `345678` cell with the first
    /// tick's box. A ligature decomposed at one box still recomposes.
    #[test]
    fn stacked_glyphs_at_one_x_stay_separate_cells() {
        let at = |ch: char, b: f32| Glyph {
            lb: b,
            lt: b + 10.0,
            b,
            t: b + 10.0,
            ..glyph(ch, 98.0, 103.0)
        };
        let ticks: Vec<Glyph> = "345678"
            .chars()
            .enumerate()
            .map(|(k, ch)| at(ch, 316.0 + 25.92 * k as f32))
            .collect();
        let lines = line_cells(&ticks, 792.0, true);
        let texts: Vec<&str> = lines.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, ["3", "4", "5", "6", "7", "8"]);
        assert!(lines.windows(2).all(|w| w[0].t > w[1].t));

        let lig = [glyph('f', 10.0, 16.0), glyph('i', 10.0, 16.0)];
        let lines = line_cells(&lig, 792.0, true);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "fi");
    }

    /// The returned vectors own no more than their cells. Collecting straight
    /// from the per-glyph `Cell` buffer reused its allocation (std's in-place
    /// collect), so a page kept ~4 slots per *glyph* — on a dense table page,
    /// 30× the cells' size, for every page the text-layer path held.
    #[test]
    fn cells_do_not_keep_the_glyph_buffer() {
        // Ten rows of `777 777 …`: abutting glyphs, words split by space glyphs.
        let glyphs: Vec<Glyph> = (0..400)
            .map(|i| {
                let x = (i % 40) as f32 * 6.0;
                let ch = if i % 4 == 3 { ' ' } else { '7' };
                let mut g = glyph(ch, x, x + 6.0);
                let row = (i / 40) as f32 * 14.0;
                (g.b, g.t, g.lb, g.lt) = (100.0 + row, 110.0 + row, 98.0 + row, 110.0 + row);
                g
            })
            .collect();
        let (lines, words) = line_and_word_cells(&glyphs, 792.0, true);
        assert_eq!((lines.len(), words.len()), (10, 100));
        assert_eq!(lines.capacity(), lines.len());
        assert_eq!(words.capacity(), words.len());
    }
}
