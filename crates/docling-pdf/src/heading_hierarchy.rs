//! Section-header level inference for the PDF/image pipeline (#302 — the
//! port of docling's `HeadingHierarchyModel`).
//!
//! The layout model classifies regions as `section_header` without a level,
//! so every heading the PDF path emits lands at the same depth and the
//! document hierarchy is flattened (Roman-numeral parts and Arabic-numeral
//! subsections collapse together). When enabled, this stage runs on the
//! assembled document — right after reading-order assembly, like docling's —
//! and assigns each heading a level from, in precedence order:
//!
//! 1. **bookmarks** — the PDF outline ([`crate::outline`]), the document's
//!    own declared hierarchy. Bookmarks are fuzzily matched (title + page) to
//!    detected headings; a confidently matched heading takes the bookmark's
//!    depth, and a confidently matched *list item* is promoted to a heading
//!    (layout models often mis-classify a heading as a list item).
//! 2. **numbering** — legal/outline numbering such as `PART I → 1. → 1.1 →
//!    (a) → (i)`. The primary signal for headings without a bookmark match.
//! 3. **style** — the heading's visual style, read from the PDF text layer's
//!    glyphs ([`GlyphStyle`], gathered by the pdfium backend): font size
//!    first — with near-equal sizes merged, since the measured height of the
//!    same font varies with descenders — then weight, slant and letter case.
//!
//! Apart from promoting a confidently bookmark-matched list item, the stage
//! only rewrites heading levels — it never adds, removes or reorders items,
//! and headings with no applicable signal keep their level. Docling's
//! semantic level `N` corresponds to our [`Node::Heading`] `level: N + 1`
//! (docling's Markdown serializer renders `section_header` level 1 as `##`,
//! which is exactly what our assembler already emits for every heading).
//!
//! Divergence from docling noted for the record: style is aggregated over
//! *glyphs* rather than parsed text-line cells (same signal, finer
//! granularity), and OCR-only headings carry no style at all (docling reads
//! OCR cell heights; our glyph pass reads the digital text layer only).

use std::collections::HashMap;

use docling_core::Node;

use crate::outline::OutlineItem;

/// Options for the heading-hierarchy stage (docling's
/// `HeadingHierarchyOptions`, defaults included).
#[derive(Clone, Debug)]
pub struct HeadingHierarchyOptions {
    /// Master switch. Off by default (docling parity): all detected headings
    /// keep the assembler's level and the output is byte-for-byte unchanged.
    pub enabled: bool,
    /// Use the PDF outline (bookmarks/ToC) as the authoritative signal.
    pub use_bookmarks: bool,
    /// Use legal/outline numbering for headings without a bookmark match.
    pub use_numbering: bool,
    /// Use visual style (font size, and below) as the last-resort signal.
    pub use_style: bool,
    /// Refine the style fallback with font weight/slant (from the embedded
    /// font names) and all-caps detection.
    pub use_font_style: bool,
    /// Relative difference below which two heading font sizes count as one
    /// size (absorbs descender-driven measurement noise).
    pub style_size_tolerance: f32,
    /// Maximum semantic heading level to assign; deeper levels clamp.
    pub max_level: u8,
    /// Minimum fuzzy title similarity (0..1) for a bookmark to match a
    /// heading/list item.
    pub bookmark_match_threshold: f32,
    /// Override of the numbering-scheme precedence (highest level first);
    /// known schemes: `part`, `chapter`, `article`, `roman_u`, `arabic`,
    /// `alpha_u`, `alpha_l`, `roman_l`. `None` = the default legal ordering.
    pub numbering_schemes: Option<Vec<String>>,
}

impl Default for HeadingHierarchyOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            use_bookmarks: true,
            use_numbering: true,
            use_style: true,
            use_font_style: true,
            style_size_tolerance: 0.05,
            max_level: 6,
            bookmark_match_threshold: 0.8,
            numbering_schemes: None,
        }
    }
}

impl HeadingHierarchyOptions {
    /// The plumbed-everywhere form: default options with the master switch.
    pub fn enabled(on: bool) -> Self {
        Self {
            enabled: on,
            ..Self::default()
        }
    }
}

/// One text-layer glyph's box and style, in top-left page points — the
/// stage's input for the style signal, gathered per page by the pdfium
/// backend (each glyph counts as one character in the style vote).
#[derive(Clone, Copy, Debug)]
pub(crate) struct GlyphStyle {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
    /// Line height (font ascent + descent at the glyph's size) — the
    /// font-size proxy, matching docling's text-line cell height.
    pub height: f32,
    /// 0 light/regular, 1 medium/semibold, 2 bold+ (from the font name).
    pub weight_cls: u8,
    pub italic: bool,
    /// Whether the font name carried any recognizable style at all.
    pub styled: bool,
}

/// Default precedence of numbering schemes, highest hierarchy level first.
/// `dotted` shares the `arabic` rank and orders below it by segment depth.
const DEFAULT_FAMILY_ORDER: [&str; 8] = [
    "part",    // PART I / TITLE I / BOOK I
    "chapter", // CHAPTER 1
    "article", // ARTICLE 1 / SECTION 1 / Clause / § 1
    "roman_u", // I. II. III.
    "arabic",  // 1. 2. 3.  (and dotted 1.1, 1.1.1 by depth)
    "alpha_u", // A. B. C.
    "alpha_l", // (a) (b) (c)
    "roman_l", // (i) (ii) (iii)
];

// ------------------------------------------------------------------ markers

/// A parsed leading numbering marker.
#[derive(Clone, Debug, PartialEq)]
struct Marker {
    family: &'static str,
    /// Dotted-decimal segment count; 1 for everything else.
    depth: usize,
    /// Raw alpha/Roman token, kept for ambiguity resolution.
    token: Option<String>,
    /// Single-letter Roman/alpha that needs document context to resolve.
    ambiguous: bool,
}

impl Marker {
    fn family(family: &'static str) -> Self {
        Marker {
            family,
            depth: 1,
            token: None,
            ambiguous: false,
        }
    }
}

/// Canonical Roman-numeral validator (1..3999), case-insensitive — the
/// hand-rolled equivalent of `M{0,4}(CM|CD|D?C{0,3})(XC|XL|L?X{0,3})
/// (IX|IV|V?I{0,3})` on a non-empty token.
fn is_roman(token: &str) -> bool {
    if token.is_empty() || !token.is_ascii() {
        return false;
    }
    let s: Vec<u8> = token.bytes().map(|b| b.to_ascii_uppercase()).collect();
    let mut i = 0;
    // M{0,4}
    let mut m = 0;
    while i < s.len() && s[i] == b'M' && m < 4 {
        i += 1;
        m += 1;
    }
    // (CM|CD|D?C{0,3})
    if s[i..].starts_with(b"CM") || s[i..].starts_with(b"CD") {
        i += 2;
    } else {
        if i < s.len() && s[i] == b'D' {
            i += 1;
        }
        let mut c = 0;
        while i < s.len() && s[i] == b'C' && c < 3 {
            i += 1;
            c += 1;
        }
    }
    // (XC|XL|L?X{0,3})
    if s[i..].starts_with(b"XC") || s[i..].starts_with(b"XL") {
        i += 2;
    } else {
        if i < s.len() && s[i] == b'L' {
            i += 1;
        }
        let mut x = 0;
        while i < s.len() && s[i] == b'X' && x < 3 {
            i += 1;
            x += 1;
        }
    }
    // (IX|IV|V?I{0,3})
    if s[i..].starts_with(b"IX") || s[i..].starts_with(b"IV") {
        i += 2;
    } else {
        if i < s.len() && s[i] == b'V' {
            i += 1;
        }
        let mut n = 0;
        while i < s.len() && s[i] == b'I' && n < 3 {
            i += 1;
            n += 1;
        }
    }
    i == s.len()
}

/// Whether `text` starts with `word` as a whole word (ASCII case-insensitive).
fn starts_with_word(text: &str, word: &str) -> bool {
    if text.len() < word.len() || !text[..word.len()].eq_ignore_ascii_case(word) {
        return false;
    }
    text[word.len()..]
        .chars()
        .next()
        .is_none_or(|c| !c.is_alphanumeric())
}

/// Classify a bare alpha/Roman token (`A`, `iv`, `i` …) into a marker.
fn classify_letter(token: &str) -> Option<Marker> {
    let upper = token.chars().all(|c| c.is_uppercase());
    if token.chars().count() == 1 {
        let is_roman_single = token
            .chars()
            .next()
            .is_some_and(|c| "IVXLCDMivxlcdm".contains(c));
        let family = match (is_roman_single, upper) {
            (true, true) => "roman_u",
            (true, false) => "roman_l",
            (false, true) => "alpha_u",
            (false, false) => "alpha_l",
        };
        return Some(Marker {
            family,
            depth: 1,
            token: Some(token.to_string()),
            ambiguous: is_roman_single,
        });
    }
    // Multi-letter tokens only count as numbering if they are valid Roman
    // numerals; otherwise they are plain words ("Summary."), not numbering.
    if is_roman(token) {
        return Some(Marker {
            family: if upper { "roman_u" } else { "roman_l" },
            depth: 1,
            token: Some(token.to_string()),
            ambiguous: false,
        });
    }
    None
}

/// Extract the leading numbering marker from a heading, or `None`.
fn parse_marker(text: &str) -> Option<Marker> {
    let s = text.trim_start();
    if s.is_empty() {
        return None;
    }

    for kw in ["part", "title", "book"] {
        if starts_with_word(s, kw) {
            return Some(Marker::family("part"));
        }
    }
    if starts_with_word(s, "chapter") {
        return Some(Marker::family("chapter"));
    }
    for kw in [
        "article", "section", "clause", "schedule", "annex", "appendix", "rule",
    ] {
        if starts_with_word(s, kw) {
            return Some(Marker::family("article"));
        }
    }
    // § 1 / §§ 1.2
    if s.starts_with('§') {
        let after = s.trim_start_matches('§').trim_start();
        if after.starts_with(|c: char| c.is_ascii_digit()) {
            return Some(Marker::family("article"));
        }
    }

    // Dotted decimal outline (1.1, 1.1.1, …) terminated by space/end/punct.
    if let Some((segments, rest)) = take_dotted(s) {
        if segments >= 2
            && rest
                .chars()
                .next()
                .is_none_or(|c| matches!(c, '.' | ')' | ']') || c.is_whitespace())
        {
            return Some(Marker {
                family: "dotted",
                depth: segments,
                token: None,
                ambiguous: false,
            });
        }
    }
    // Single Arabic index (1. / 2)).
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let rest = &s[digits..];
        if rest.starts_with('.') || rest.starts_with(')') {
            return Some(Marker::family("arabic"));
        }
    }

    // Single/multi letter marker, optionally parenthesized: (a) / A. / (iv).
    let after_paren = s.strip_prefix('(').map(str::trim_start).unwrap_or(s);
    let letters: String = after_paren
        .chars()
        .take_while(|c| c.is_alphabetic())
        .collect();
    if !letters.is_empty() {
        let rest = after_paren[letters.len()..].trim_start();
        if rest.starts_with(')') || rest.starts_with('.') {
            return classify_letter(&letters);
        }
    }
    None
}

/// Parse a leading `N(.N)+` run: `(segment count, rest)`. `None` when the
/// text does not start with at least `N.N`.
fn take_dotted(s: &str) -> Option<(usize, &str)> {
    let mut rest = s;
    let mut segments = 0;
    loop {
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            break;
        }
        segments += 1;
        rest = &rest[digits..];
        match rest.strip_prefix('.') {
            // Only continue when a digit follows the dot — `1.` is arabic,
            // not dotted.
            Some(r) if r.starts_with(|c: char| c.is_ascii_digit()) => rest = r,
            _ => break,
        }
    }
    (segments >= 2).then_some((segments, rest))
}

/// Resolve single-letter Roman/alpha markers in place using document-wide
/// evidence: a lone `I.` is Roman when the document also contains unambiguous
/// Roman markers and alpha when it contains unambiguous alpha markers. When
/// evidence is absent or conflicting, `I`/`i` default to Roman (the common
/// legal case) and other letters to alpha.
fn resolve_ambiguous(markers: &mut [Option<Marker>]) {
    let has = |family: &str, ms: &[Option<Marker>]| {
        ms.iter()
            .flatten()
            .any(|m| !m.ambiguous && m.family == family)
    };
    let upper_roman = has("roman_u", markers);
    let upper_alpha = has("alpha_u", markers);
    let lower_roman = has("roman_l", markers);
    let lower_alpha = has("alpha_l", markers);

    for m in markers.iter_mut().flatten() {
        if !m.ambiguous {
            continue;
        }
        let Some(token) = m.token.as_deref() else {
            continue;
        };
        let upper = token.chars().all(|c| c.is_uppercase());
        let (has_roman, has_alpha) = if upper {
            (upper_roman, upper_alpha)
        } else {
            (lower_roman, lower_alpha)
        };
        let roman = if has_roman && !has_alpha {
            true
        } else if has_alpha && !has_roman {
            false
        } else {
            token == "I" || token == "i"
        };
        m.family = match (roman, upper) {
            (true, true) => "roman_u",
            (true, false) => "roman_l",
            (false, true) => "alpha_u",
            (false, false) => "alpha_l",
        };
        m.ambiguous = false;
    }
}

fn family_rank(family: &str, order: &[String]) -> usize {
    let key = if family == "dotted" { "arabic" } else { family };
    order.iter().position(|f| f == key).unwrap_or(order.len()) // unknown scheme → lowest priority
}

/// Map heading index → level from numbering markers (relative, compressed).
fn infer_from_numbering(
    heading_texts: &[&str],
    options: &HeadingHierarchyOptions,
) -> HashMap<usize, usize> {
    let order: Vec<String> = options
        .numbering_schemes
        .clone()
        .unwrap_or_else(|| DEFAULT_FAMILY_ORDER.iter().map(|s| s.to_string()).collect());
    let mut markers: Vec<Option<Marker>> = heading_texts.iter().map(|t| parse_marker(t)).collect();
    resolve_ambiguous(&mut markers);

    let mut keys: HashMap<usize, (usize, usize)> = HashMap::new();
    for (i, m) in markers.iter().enumerate() {
        if let Some(m) = m {
            keys.insert(i, (family_rank(m.family, &order), m.depth));
        }
    }
    compress_keys(keys)
}

/// Compress the distinct sort keys actually present into contiguous 1-based
/// levels, so a document that starts at "1." is not forced to start deep.
fn compress_keys<K: Ord + Clone + std::hash::Hash>(
    keys: HashMap<usize, K>,
) -> HashMap<usize, usize> {
    let mut distinct: Vec<K> = keys.values().cloned().collect();
    distinct.sort();
    distinct.dedup();
    let level_of: HashMap<K, usize> = distinct
        .into_iter()
        .enumerate()
        .map(|(i, k)| (k, i + 1))
        .collect();
    keys.into_iter().map(|(i, k)| (i, level_of[&k])).collect()
}

// -------------------------------------------------------------------- style

/// Share of a heading's styled characters that must be italic to count.
const ITALIC_RATIO: f32 = 0.6;

/// Whether the text is written in capitals (ignoring digits and punctuation).
fn is_all_caps(text: &str) -> bool {
    let letters: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    letters.len() >= 4 && letters.iter().all(|c| c.is_uppercase())
}

/// The visual style of one heading — the ranking key of the style fallback.
#[derive(Clone, Copy, Debug)]
struct HeadingStyle {
    size: f32,
    weight_cls: u8,
    italic: bool,
    caps: bool,
}

/// Derive a heading's style from the glyphs overlapping its box. Weight and
/// slant are a character-weighted vote (a heading can mix a regular "1.1 "
/// with a bold title); glyphs without font style contribute size only, so the
/// ranking degrades to font size.
fn heading_style(
    bbox: [f32; 4],
    text: &str,
    glyphs: &[GlyphStyle],
    options: &HeadingHierarchyOptions,
) -> Option<HeadingStyle> {
    let [hl, ht, hr, hb] = bbox;
    let mut heights: Vec<f32> = Vec::new();
    let mut weights = [0usize; 3];
    let mut styled_chars = 0usize;
    let mut italic_chars = 0usize;
    for g in glyphs {
        if g.l < hr && g.r > hl && g.t < hb && g.b > ht {
            heights.push(g.height);
            if options.use_font_style && g.styled {
                weights[g.weight_cls.min(2) as usize] += 1;
                styled_chars += 1;
                if g.italic {
                    italic_chars += 1;
                }
            }
        }
    }
    if heights.is_empty() {
        return None;
    }
    heights.sort_by(f32::total_cmp);
    let size = if heights.len() % 2 == 1 {
        heights[heights.len() / 2]
    } else {
        (heights[heights.len() / 2 - 1] + heights[heights.len() / 2]) / 2.0
    };
    if !options.use_font_style {
        return Some(HeadingStyle {
            size,
            weight_cls: 0,
            italic: false,
            caps: false,
        });
    }
    // On a tie, the heavier class wins: emphasis makes a heading stand out.
    let weight_cls = (0u8..3)
        .max_by_key(|&cls| (weights[cls as usize], cls))
        .unwrap_or(0);
    Some(HeadingStyle {
        size,
        weight_cls,
        italic: styled_chars > 0 && italic_chars as f32 / styled_chars as f32 >= ITALIC_RATIO,
        caps: is_all_caps(text),
    })
}

/// Group font sizes into clusters (largest first) and map each size to its
/// cluster index; consecutive sizes within `tolerance` (relative) merge, to
/// absorb descender-driven measurement noise.
fn cluster_sizes(mut sizes: Vec<f32>, tolerance: f32) -> Vec<(f32, usize)> {
    sizes.sort_by(|a, b| b.total_cmp(a));
    sizes.dedup();
    let mut clusters = Vec::with_capacity(sizes.len());
    let mut index = 0usize;
    let mut previous: Option<f32> = None;
    for size in sizes {
        if let Some(prev) = previous {
            if (prev - size) > tolerance * prev {
                index += 1;
            }
        }
        clusters.push((size, index));
        previous = Some(size);
    }
    clusters
}

/// Map heading index → level from heading styles (most prominent = level 1).
fn infer_from_style(
    headings: &[HeadingRef],
    glyph_styles: &HashMap<usize, Vec<GlyphStyle>>,
    options: &HeadingHierarchyOptions,
) -> HashMap<usize, usize> {
    if glyph_styles.is_empty() {
        return HashMap::new();
    }
    let mut styles: HashMap<usize, HeadingStyle> = HashMap::new();
    for (i, h) in headings.iter().enumerate() {
        let Some(glyphs) = glyph_styles.get(&h.page_no) else {
            continue;
        };
        let Some(bbox) = h.bbox_points else { continue };
        if let Some(style) = heading_style(bbox, &h.text, glyphs, options) {
            styles.insert(i, style);
        }
    }
    if styles.is_empty() {
        return HashMap::new();
    }
    let clusters = cluster_sizes(
        styles.values().map(|s| s.size).collect(),
        options.style_size_tolerance,
    );
    let cluster_of = |size: f32| -> usize {
        clusters
            .iter()
            .find(|(s, _)| *s == size)
            .map(|(_, c)| *c)
            .unwrap_or(0)
    };
    // Order by size cluster, then by how much the heading stands out within
    // its size: heavier before lighter, upright before italic, capitals
    // before mixed case.
    let keys: HashMap<usize, (usize, i8, bool, bool)> = styles
        .into_iter()
        .map(|(i, s)| {
            (
                i,
                (cluster_of(s.size), -(s.weight_cls as i8), s.italic, !s.caps),
            )
        })
        .collect();
    compress_keys(keys)
}

// ---------------------------------------------------------------- bookmarks

/// Lower-case, collapse whitespace, trim outer punctuation for matching.
fn norm(text: &str) -> String {
    let collapsed = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    collapsed
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_string()
}

/// Strip one leading numbering marker before fuzzy-matching a title, so a
/// bookmark "Definitions" matches an on-page heading "1.1 Definitions" (the
/// port of docling's `_LEADING_MARKER`).
fn strip_marker(text: &str) -> String {
    let s = text.trim_start();
    let matched_len = leading_marker_len(s);
    match matched_len {
        Some(n) => {
            let rest = &s[n..];
            let trimmed = rest.trim_start_matches(|c: char| {
                c.is_whitespace() || matches!(c, '.' | ':' | ')' | '-')
            });
            trimmed.to_string()
        }
        None => text.to_string(),
    }
}

/// Byte length of the leading marker in `s`, `None` when there is none.
fn leading_marker_len(s: &str) -> Option<usize> {
    // Keyword + optional `[\s.:]*[0-9ivxlcdm]*` tail.
    for kw in [
        "chapter", "article", "section", "clause", "schedule", "annex", "appendix", "rule", "part",
        "title", "book",
    ] {
        if starts_with_word(s, kw) {
            let mut i = kw.len();
            let bytes = s.as_bytes();
            while i < bytes.len()
                && (bytes[i].is_ascii_whitespace() || bytes[i] == b'.' || bytes[i] == b':')
            {
                i += 1;
            }
            while i < bytes.len()
                && (bytes[i].is_ascii_digit() || b"ivxlcdmIVXLCDM".contains(&bytes[i]))
            {
                i += 1;
            }
            return Some(i);
        }
    }
    // §+ \s* [0-9.]+
    if s.starts_with('§') {
        let rest = s.trim_start_matches('§');
        let ws = rest.len() - rest.trim_start().len();
        let rest2 = rest.trim_start();
        let num = rest2
            .bytes()
            .take_while(|b| b.is_ascii_digit() || *b == b'.')
            .count();
        if num > 0 {
            return Some(s.len() - rest.len() + ws + num);
        }
    }
    // \(? \d+(\.\d+)* [).]?
    let (paren, body) = match s.strip_prefix('(') {
        Some(r) => (1, r),
        None => (0, s),
    };
    let digits = body.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits > 0 {
        let mut i = digits;
        let b = body.as_bytes();
        while i < b.len() && b[i] == b'.' {
            let d = body[i + 1..]
                .bytes()
                .take_while(|x| x.is_ascii_digit())
                .count();
            if d == 0 {
                break;
            }
            i += 1 + d;
        }
        if i < b.len() && (b[i] == b')' || b[i] == b'.') {
            i += 1;
        }
        return Some(paren + i);
    }
    // \(? [A-Za-z]{1,2} [).]
    let letters = body.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
    if (1..=2).contains(&letters) {
        let b = body.as_bytes();
        if letters < b.len() && (b[letters] == b')' || b[letters] == b'.') {
            return Some(paren + letters + 1);
        }
    }
    None
}

/// `difflib.SequenceMatcher.ratio()` for two short strings: 2·M / T over the
/// Ratcliff–Obershelp matching blocks (no junk heuristic — titles are short).
fn similarity(a: &str, b: &str) -> f32 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
    for (j, &c) in b.iter().enumerate() {
        b2j.entry(c).or_default().push(j);
    }
    let mut matches = 0usize;
    let mut queue = vec![(0usize, a.len(), 0usize, b.len())];
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        // find_longest_match(alo, ahi, blo, bhi)
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for (i, ch) in a.iter().enumerate().take(ahi).skip(alo) {
            let mut newj2len: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = b2j.get(ch) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j
                        .checked_sub(1)
                        .and_then(|p| j2len.get(&p))
                        .copied()
                        .unwrap_or(0)
                        + 1;
                    newj2len.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = newj2len;
        }
        if bestsize == 0 {
            continue;
        }
        matches += bestsize;
        if besti > alo && bestj > blo {
            queue.push((alo, besti, blo, bestj));
        }
        if besti + bestsize < ahi && bestj + bestsize < bhi {
            queue.push((besti + bestsize, ahi, bestj + bestsize, bhi));
        }
    }
    (2.0 * matches as f32) / (a.len() + b.len()) as f32
}

/// Fuzzy similarity in 0..1 between a detected heading and a bookmark title.
/// Both are compared with and without their leading numbering marker, and
/// containment of one normalized title in the other boosts the score
/// (bookmarks are frequently truncated).
fn match_score(cand_text: &str, bm_title: &str) -> f32 {
    let mut variants_a = vec![norm(cand_text), norm(&strip_marker(cand_text))];
    let mut variants_b = vec![norm(bm_title), norm(&strip_marker(bm_title))];
    variants_a.retain(|v| !v.is_empty());
    variants_b.retain(|v| !v.is_empty());
    variants_a.dedup();
    variants_b.dedup();
    let mut best: f32 = 0.0;
    for a in &variants_a {
        for b in &variants_b {
            best = best.max(similarity(a, b));
            if a.chars().count() >= 4
                && b.chars().count() >= 4
                && (a.contains(b.as_str()) || b.contains(a.as_str()))
            {
                best = best.max(0.92);
            }
        }
    }
    best
}

// ------------------------------------------------------------------- stage

/// A heading (or promotable list item) found in the node stream.
struct HeadingRef {
    /// Index into the node vec.
    node_idx: usize,
    /// Plain text (markers reconstructed for ordered list items).
    text: String,
    /// 1-based page.
    page_no: usize,
    /// `[l, t, r, b]` in top-left page points, when the node carries one.
    bbox_points: Option<[f32; 4]>,
    /// Whether this is a list item (a bookmark match promotes it).
    is_list_item: bool,
}

/// Collect heading/list-item candidates with their page geometry.
fn collect(nodes: &[Node], with_list_items: bool) -> Vec<HeadingRef> {
    let mut out = Vec::new();
    let mut page_no = 0usize;
    let mut page_w = 0f32;
    let mut page_h = 0f32;
    let denorm = |loc: [u16; 4], w: f32, h: f32| -> Option<[f32; 4]> {
        (w > 0.0 && h > 0.0).then(|| {
            [
                loc[0] as f32 / 512.0 * w,
                loc[1] as f32 / 512.0 * h,
                loc[2] as f32 / 512.0 * w,
                loc[3] as f32 / 512.0 * h,
            ]
        })
    };
    for (idx, node) in nodes.iter().enumerate() {
        match node {
            Node::PageInfo {
                page_no: p,
                width,
                height,
            } => {
                page_no = *p;
                page_w = *width;
                page_h = *height;
            }
            Node::Located { location, inner } => {
                if let Node::Heading { text, .. } = inner.as_ref() {
                    out.push(HeadingRef {
                        node_idx: idx,
                        text: text.clone(),
                        page_no,
                        bbox_points: denorm(*location, page_w, page_h),
                        is_list_item: false,
                    });
                }
            }
            Node::Heading { text, .. } => out.push(HeadingRef {
                node_idx: idx,
                text: text.clone(),
                page_no,
                bbox_points: None,
                is_list_item: false,
            }),
            Node::ListItem {
                ordered,
                number,
                text,
                location,
                ..
            } if with_list_items => {
                // Reconstruct the enumeration marker the assembler folded into
                // `number`, so bookmark titles that carry it still match.
                let text = if *ordered {
                    format!("{number}. {text}")
                } else {
                    text.clone()
                };
                out.push(HeadingRef {
                    node_idx: idx,
                    text,
                    page_no,
                    bbox_points: location.and_then(|loc| denorm(loc, page_w, page_h)),
                    is_list_item: true,
                });
            }
            _ => {}
        }
    }
    out
}

/// The 1-based pages that contain headings — what the style glyph pass needs
/// to read. Empty when the stage would have nothing to do.
pub(crate) fn heading_pages(nodes: &[Node]) -> Vec<usize> {
    let mut pages: Vec<usize> = collect(nodes, false).iter().map(|h| h.page_no).collect();
    pages.sort_unstable();
    pages.dedup();
    pages.retain(|&p| p > 0);
    pages
}

/// Match the PDF outline to candidates; returns `candidate index → level`
/// (compressed, 1-based). Mirrors docling's `_infer_from_bookmarks`.
fn infer_from_bookmarks(
    candidates: &[HeadingRef],
    outline: &[OutlineItem],
    options: &HeadingHierarchyOptions,
) -> HashMap<usize, usize> {
    let mut claimed: Vec<bool> = vec![false; candidates.len()];
    let mut matches: Vec<(usize, usize)> = Vec::new(); // (candidate, raw level)

    for bm in outline {
        let title = bm.title.trim();
        if title.is_empty() {
            continue;
        }
        // A cross-page (page-less) match must be stronger.
        let threshold = if bm.page_no.is_none() {
            (options.bookmark_match_threshold + 0.1).min(1.0)
        } else {
            options.bookmark_match_threshold
        };
        let mut best: Option<(usize, f32, f32)> = None; // (idx, score, dist)
        for (idx, cand) in candidates.iter().enumerate() {
            if claimed[idx] {
                continue;
            }
            if let (Some(bp), cp) = (bm.page_no, cand.page_no) {
                if cp != 0 && cp != bp {
                    continue;
                }
            }
            let score = match_score(&cand.text, title);
            if score < threshold {
                continue;
            }
            let dist = match (cand.bbox_points.map(|b| b[1]), bm.y_top) {
                (Some(top), Some(y)) => (top - y).abs(),
                _ => f32::INFINITY,
            };
            let better = match best {
                None => true,
                Some((_, bs, bd)) => score > bs + 1e-6 || ((score - bs).abs() <= 1e-6 && dist < bd),
            };
            if better {
                best = Some((idx, score, dist));
            }
        }
        if let Some((idx, _, _)) = best {
            claimed[idx] = true;
            matches.push((idx, bm.level));
        }
    }
    if matches.is_empty() {
        return HashMap::new();
    }
    // Compress the raw bookmark depths actually used into contiguous levels.
    compress_keys(matches.into_iter().collect())
}

/// Run the stage: assign heading levels in place on the assembled node
/// stream. Precedence: bookmarks > numbering > style; headings with no
/// applicable signal keep their level; a confidently bookmark-matched list
/// item is promoted to a heading. `outline` and `glyph_styles` may be empty
/// (signals degrade individually, docling parity).
pub(crate) fn apply(
    nodes: &mut [Node],
    outline: &[OutlineItem],
    glyph_styles: &HashMap<usize, Vec<GlyphStyle>>,
    options: &HeadingHierarchyOptions,
) {
    if !options.enabled {
        return;
    }

    // Bookmark pass first: it may promote list items, changing the set of
    // headings, so it has to run before the heading list is (re)collected.
    let mut bookmark_levels: HashMap<usize, usize> = HashMap::new(); // node_idx → level
    if options.use_bookmarks && !outline.is_empty() {
        let candidates = collect(nodes, true);
        let matched = infer_from_bookmarks(&candidates, outline, options);
        for (cand_idx, level) in matched {
            let cand = &candidates[cand_idx];
            if cand.is_list_item {
                promote_list_item(nodes, cand.node_idx, &cand.text);
            }
            bookmark_levels.insert(cand.node_idx, level);
        }
    }

    let headings = collect(nodes, false);
    if headings.is_empty() {
        return;
    }

    let mut levels: HashMap<usize, usize> = HashMap::new(); // heading index → level
                                                            // Bookmarks are authoritative: seed first so nothing overrides them.
    for (i, h) in headings.iter().enumerate() {
        if let Some(level) = bookmark_levels.get(&h.node_idx) {
            levels.insert(i, *level);
        }
    }
    if options.use_numbering {
        let texts: Vec<&str> = headings.iter().map(|h| h.text.as_str()).collect();
        for (i, level) in infer_from_numbering(&texts, options) {
            levels.entry(i).or_insert(level);
        }
    }
    if options.use_style && !glyph_styles.is_empty() {
        for (i, level) in infer_from_style(&headings, glyph_styles, options) {
            levels.entry(i).or_insert(level);
        }
    }

    for (i, h) in headings.iter().enumerate() {
        let Some(&level) = levels.get(&i) else {
            continue;
        };
        let semantic = level.clamp(1, options.max_level.max(1) as usize);
        // Our `Node::Heading` level is the rendered Markdown depth: docling's
        // semantic level N serializes as N+1 hashes (`##` for level 1).
        let rendered = (semantic + 1).min(u8::MAX as usize) as u8;
        set_heading_level(&mut nodes[h.node_idx], rendered);
    }
}

fn set_heading_level(node: &mut Node, new_level: u8) {
    match node {
        Node::Heading { level, .. } => *level = new_level,
        Node::Located { inner, .. } => {
            if let Node::Heading { level, .. } = inner.as_mut() {
                *level = new_level;
            }
        }
        _ => {}
    }
}

/// Promote a bookmark-matched list item to a heading in place: same text and
/// position, now a `Heading` (the level is assigned by the caller). The
/// following sibling inherits `first_in_list` when the promoted item opened
/// its list, so adjacent lists keep their separation.
fn promote_list_item(nodes: &mut [Node], idx: usize, text: &str) {
    let Node::ListItem {
        first_in_list,
        location,
        ..
    } = &nodes[idx]
    else {
        return;
    };
    let was_first = *first_in_list;
    let loc = *location;
    let heading = Node::Heading {
        level: 2,
        text: text.to_string(),
    };
    nodes[idx] = match loc {
        Some(location) => Node::Located {
            location,
            inner: Box::new(heading),
        },
        None => heading,
    };
    if was_first {
        if let Some(Node::ListItem { first_in_list, .. }) = nodes.get_mut(idx + 1) {
            *first_in_list = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(loc: [u16; 4], text: &str) -> Node {
        Node::Located {
            location: loc,
            inner: Box::new(Node::Heading {
                level: 2,
                text: text.to_string(),
            }),
        }
    }

    fn page(no: usize) -> Node {
        Node::PageInfo {
            page_no: no,
            width: 512.0,
            height: 512.0,
        }
    }

    fn levels(nodes: &[Node]) -> Vec<u8> {
        nodes
            .iter()
            .filter_map(|n| match n {
                Node::Located { inner, .. } => match inner.as_ref() {
                    Node::Heading { level, .. } => Some(*level),
                    _ => None,
                },
                Node::Heading { level, .. } => Some(*level),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn roman_validator_matches_difflib_regex() {
        for ok in ["I", "iv", "XIV", "MCMXCIX", "iii", "C"] {
            assert!(is_roman(ok), "{ok}");
        }
        for bad in ["", "IIII", "VX", "ABC", "Summary", "IC"] {
            assert!(!is_roman(bad), "{bad}");
        }
    }

    #[test]
    fn markers_parse_the_docling_families() {
        let fam = |t: &str| parse_marker(t).map(|m| (m.family, m.depth));
        assert_eq!(fam("PART I — General"), Some(("part", 1)));
        assert_eq!(fam("Chapter 2: Scope"), Some(("chapter", 1)));
        assert_eq!(fam("Article 5"), Some(("article", 1)));
        assert_eq!(fam("§ 12 Something"), Some(("article", 1)));
        assert_eq!(fam("1. Introduction"), Some(("arabic", 1)));
        assert_eq!(fam("2) Also arabic"), Some(("arabic", 1)));
        assert_eq!(fam("1.1 Scope"), Some(("dotted", 2)));
        assert_eq!(fam("2.3.1 Deep"), Some(("dotted", 3)));
        assert_eq!(fam("A. Annex-ish"), Some(("alpha_u", 1)));
        assert_eq!(fam("(a) item"), Some(("alpha_l", 1)));
        assert_eq!(fam("(iv) sub"), Some(("roman_l", 1)));
        assert_eq!(fam("IV. Chapter"), Some(("roman_u", 1)));
        // Plain words are not numbering.
        assert_eq!(fam("Summary."), None);
        assert_eq!(fam("Overview"), None);
    }

    #[test]
    fn ambiguous_single_letters_resolve_from_document_context() {
        // With unambiguous Roman evidence, a lone "V." reads as Roman.
        let texts = ["I. One", "II. Two", "V. Five"];
        let map = infer_from_numbering(&texts.map(|t| t), &HeadingHierarchyOptions::default());
        // All three land on the same (roman_u) level.
        assert_eq!(map[&0], map[&2]);
        // With alpha evidence instead, "C." reads as alpha.
        let texts = ["B. Bee", "C. Sea", "D. Dee"];
        let map = infer_from_numbering(&texts.map(|t| t), &HeadingHierarchyOptions::default());
        assert_eq!(map[&0], map[&1]);
        assert_eq!(map[&1], map[&2]);
    }

    #[test]
    fn numbering_levels_compress_to_contiguous() {
        // part > dotted-2 > dotted-3: distinct keys → levels 1, 2, 3 even
        // though the arabic family rank sits far from `part`.
        let texts = ["PART I", "1.1 Scope", "1.1.1 Detail", "No marker"];
        let map = infer_from_numbering(&texts.map(|t| t), &HeadingHierarchyOptions::default());
        assert_eq!(map[&0], 1);
        assert_eq!(map[&1], 2);
        assert_eq!(map[&2], 3);
        assert!(!map.contains_key(&3));
    }

    #[test]
    fn similarity_behaves_like_difflib_ratio() {
        assert_eq!(similarity("abc", "abc"), 1.0);
        assert_eq!(similarity("", ""), 1.0);
        assert_eq!(similarity("abc", "xyz"), 0.0);
        // difflib: SequenceMatcher(None, "abcd", "bcde").ratio() == 0.75
        assert!((similarity("abcd", "bcde") - 0.75).abs() < 1e-6);
    }

    #[test]
    fn bookmark_titles_match_with_and_without_markers() {
        assert!(match_score("1.1 Definitions", "Definitions") >= 0.9);
        assert!(match_score("ARTICLE 5 Payment Terms", "Payment Terms") >= 0.9);
        assert!(match_score("Introduction", "Conclusion") < 0.8);
    }

    #[test]
    fn apply_assigns_numbering_levels_end_to_end() {
        let mut nodes = vec![
            page(1),
            heading([10, 10, 200, 20], "1. Introduction"),
            heading([10, 40, 200, 50], "1.1 Scope"),
            heading([10, 70, 200, 80], "Unnumbered"),
        ];
        apply(
            &mut nodes,
            &[],
            &HashMap::new(),
            &HeadingHierarchyOptions::enabled(true),
        );
        // Semantic 1/2 render as Markdown levels 2/3; the unnumbered heading
        // keeps the assembler's level.
        assert_eq!(levels(&nodes), vec![2, 3, 2]);
    }

    #[test]
    fn apply_is_inert_when_disabled() {
        let mut nodes = vec![page(1), heading([10, 10, 200, 20], "1.1.1 Deep")];
        apply(
            &mut nodes,
            &[],
            &HashMap::new(),
            &HeadingHierarchyOptions::default(),
        );
        assert_eq!(levels(&nodes), vec![2]);
    }

    #[test]
    fn bookmarks_win_over_numbering_and_promote_list_items() {
        let outline = vec![
            OutlineItem {
                title: "1. Introduction".into(),
                level: 0,
                page_no: Some(1),
                y_top: None,
            },
            OutlineItem {
                title: "Hidden Heading".into(),
                level: 1,
                page_no: Some(1),
                y_top: None,
            },
        ];
        let mut nodes = vec![
            page(1),
            // Numbering alone would put this on level 1 too, but the bookmark
            // is authoritative and the depths compress from the outline.
            heading([10, 10, 200, 20], "1. Introduction"),
            Node::ListItem {
                ordered: false,
                number: 0,
                first_in_list: true,
                text: "Hidden Heading".into(),
                level: 0,
                marker: None,
                location: Some([10, 40, 200, 50]),
                dclx: None,
                href: None,
                layer: None,
            },
            Node::ListItem {
                ordered: false,
                number: 0,
                first_in_list: false,
                text: "a real item".into(),
                level: 0,
                marker: None,
                location: Some([10, 70, 200, 80]),
                dclx: None,
                href: None,
                layer: None,
            },
        ];
        apply(
            &mut nodes,
            &outline,
            &HashMap::new(),
            &HeadingHierarchyOptions::enabled(true),
        );
        // The matched list item became a level-2 (semantic 2 → rendered 3)
        // heading; the trailing sibling re-opens its list.
        assert_eq!(levels(&nodes), vec![2, 3]);
        match &nodes[3] {
            Node::ListItem {
                first_in_list,
                text,
                ..
            } => {
                assert!(*first_in_list, "sibling re-opens the list");
                assert_eq!(text, "a real item");
            }
            other => panic!("expected the sibling list item, got {other:?}"),
        }
    }

    #[test]
    fn style_ranks_by_size_then_prominence() {
        // Two size clusters (18pt vs 12pt); within 12pt, bold beats regular.
        let glyphs = vec![
            GlyphStyle {
                l: 10.0,
                t: 10.0,
                r: 100.0,
                b: 28.0,
                height: 18.0,
                weight_cls: 2,
                italic: false,
                styled: true,
            },
            GlyphStyle {
                l: 10.0,
                t: 60.0,
                r: 100.0,
                b: 72.0,
                height: 12.0,
                weight_cls: 2,
                italic: false,
                styled: true,
            },
            GlyphStyle {
                l: 10.0,
                t: 110.0,
                r: 100.0,
                b: 122.0,
                height: 12.0,
                weight_cls: 0,
                italic: false,
                styled: true,
            },
        ];
        let mut styles = HashMap::new();
        styles.insert(1usize, glyphs);
        let mut nodes = vec![
            page(1),
            heading([10, 10, 200, 28], "Big Title Words"),
            heading([10, 60, 200, 72], "Bold Twelve"),
            heading([10, 110, 200, 122], "Plain Twelve"),
        ];
        // Page is 512x512 points in these tests, so locations ≈ points.
        apply(
            &mut nodes,
            &[],
            &styles,
            &HeadingHierarchyOptions::enabled(true),
        );
        assert_eq!(levels(&nodes), vec![2, 3, 4]);
    }

    #[test]
    fn strip_marker_removes_leading_numbering() {
        assert_eq!(strip_marker("1.1 Definitions"), "Definitions");
        assert_eq!(strip_marker("ARTICLE 5 - Payment"), "Payment");
        assert_eq!(strip_marker("(a) item"), "item");
        assert_eq!(strip_marker("No marker here"), "No marker here");
    }
}
