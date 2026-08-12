//! Port of docling-parse's line-cell sanitizer
//! (`src/parse/page_item_sanitators/cells.h` → `create_line_cells` /
//! `contract_cells_into_lines_v1`). It merges per-glyph char cells into line
//! cells via a 3-pass contraction — left-to-right, right-to-left, then
//! left-to-right with reverse — using corner-distance adjacency and inserting at
//! most one space per merge. This reproduces docling-parse's inter-word spacing
//! (justified double spaces, the space before a `:`, and RTL ordering) that the
//! ad-hoc `lines_from_glyphs` reconstruction can't.
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
// so a word cell never contains an inserted space. Space glyphs are pure
// word-boundary barriers: they are dropped from the word run up front, so a
// thin CJK space's neighbors may still contract into one spaceless word.
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
    font: u64,       // hash of the PDF font name+flags (for enforce_same_font)
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
        // Drop *degenerate* space glyphs (zero-width loose box): pdfium's generated
        // spaces get a zero-width box at the wrong baseline that breaks the
        // corner-distance adjacency. Without them the inter-word gap drives
        // `merge_with`'s space insertion. Spaces with a real width are kept (they
        // carry justified double-space information).
        if g.ch == ' ' && (g.lr - g.ll).abs() < 0.5 {
            continue;
        }
        // Recompose a ligature: pdfium decomposes one font glyph (Latin fi/ffi,
        // Arabic lam-alef) into several chars at the *same* loose box. Append them
        // into one cell so the contraction never inserts a space inside it.
        if let Some(last) = cells.last_mut() {
            if (last.rx0 - g.ll as f64).abs() < 0.5 && (last.rx1 - g.lr as f64).abs() < 0.5 {
                // Overprint duplicate: the *same* character re-stamped, offset by a
                // fraction of its width (a kashida/elongation segment re-drawn for
                // weight). docling-parse drops it; appending over-counts
                // (right_to_left_02's `قويووووة` vs `قويوووة`). Require a real offset
                // (> 0.1) so a ligature expansion — which decomposes one glyph into
                // several chars at the *identical* box (`ﬀ`→`ff`, diff ≈ 0) — is still
                // recomposed; real doubled letters sit a full advance apart (> 0.5).
                let offset = (g.ll as f64 - last.rx0).abs();
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
            rx0: g.ll as f64,
            ry0: g.lb as f64,
            rx1: g.lr as f64,
            ry1: g.lb as f64,
            rx2: g.lr as f64,
            ry2: g.lt as f64,
            rx3: g.ll as f64,
            ry3: g.lt as f64,
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
    // Word run: the space glyphs act as pure word-boundary barriers and never
    // survive into a word cell — with them out of the stream, two glyphs that
    // *overlap* across a thin CJK space (`군`…`감`, 1.5 pt apart under a 2.5 pt
    // gate) contract into one spaceless word (`1군감염병`), while a full Latin
    // space's gap (~0.5 em) exceeds the 0.33 gate and keeps words apart.
    let mut word_run: Vec<Cell> = built
        .iter()
        .filter(|c| !is_all_space(&c.text))
        .cloned()
        .collect();
    let mut cells = built;
    contract(&mut cells, euclidean, LINE_FACTORS);
    let lines: Vec<TextCell> = cells.into_iter().map(to_text_cell).collect();
    contract(&mut word_run, euclidean, WORD_FACTORS);
    let words: Vec<TextCell> = word_run
        .into_iter()
        .filter(|c| !c.text.trim().is_empty())
        .map(to_text_cell)
        .collect();
    (lines, words)
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
