//! SVG backend (issue #212) — a docling.rs extension; Python docling does not
//! accept SVG input at all.
//!
//! Mirrors the crate's pdf / pdf-text split:
//!
//! - **ML build** (`pdf` feature): the SVG is rasterized to a white-backed PNG
//!   (resvg — pure Rust) and rides the existing image pipeline, so layout +
//!   OCR see the drawing exactly as a browser renders it and charts/diagrams
//!   come back with structure (headings, tables, pictures).
//! - **No ML** (`pdf-text` / wasm), or `--no-ocr` on any build: this backend
//!   extracts the `<text>` elements directly — lossless for the words, lossy
//!   for structure. Labels are grouped into visual lines (top-to-bottom, then
//!   left-to-right) and each line becomes a flat paragraph; no fonts, no
//!   inference. With native text in hand this beats OCR-less rasterization,
//!   which would yield nothing.
//!
//! Reading order needs geometry, so ancestor `transform` chains are resolved
//! (translate / scale / rotate / matrix — the affine subset); unrendered
//! subtrees (`<defs>`, `<clipPath>`, `<mask>`, `<pattern>`, `<symbol>`,
//! `display:none`) contribute nothing, matching what a viewer paints.

use docling_core::{DoclingDocument, Node};
use roxmltree::{Document, Node as XmlNode};

use crate::backend::DeclarativeBackend;
use crate::error::ConversionError;
use crate::source::SourceDocument;

pub struct SvgBackend;

/// Row-major 2×3 affine: `[a b c d e f]` maps `(x, y)` to
/// `(a·x + c·y + e, b·x + d·y + f)` — SVG's `matrix()` parameter order.
#[derive(Clone, Copy)]
struct Xf([f32; 6]);

impl Xf {
    const IDENTITY: Xf = Xf([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        let m = self.0;
        (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
    }

    /// `self ∘ other` — `other` in the local frame, then `self` (the SVG rule:
    /// a child's transform composes to the right of its ancestors').
    fn then(&self, o: Xf) -> Xf {
        let (a, b) = (self.0, o.0);
        Xf([
            a[0] * b[0] + a[2] * b[1],
            a[1] * b[0] + a[3] * b[1],
            a[0] * b[2] + a[2] * b[3],
            a[1] * b[2] + a[3] * b[3],
            a[0] * b[4] + a[2] * b[5] + a[4],
            a[1] * b[4] + a[3] * b[5] + a[5],
        ])
    }
}

/// Parse an SVG `transform` list (`translate(…) rotate(…) …`, applied
/// left-to-right). Unknown/unsupported entries (`skewX`/`skewY`) are skipped —
/// they shear glyphs but barely move anchor points, and only reading *order*
/// is at stake here.
fn parse_transform(s: &str) -> Xf {
    let mut xf = Xf::IDENTITY;
    let mut rest = s;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let Some(close) = rest[open..].find(')') else {
            break;
        };
        let args: Vec<f32> = rest[open + 1..open + close]
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|t| !t.is_empty())
            .filter_map(|t| t.parse().ok())
            .collect();
        rest = &rest[open + close + 1..];
        let step = match (name, args.as_slice()) {
            ("translate", [tx]) => Xf([1.0, 0.0, 0.0, 1.0, *tx, 0.0]),
            ("translate", [tx, ty]) => Xf([1.0, 0.0, 0.0, 1.0, *tx, *ty]),
            ("scale", [k]) => Xf([*k, 0.0, 0.0, *k, 0.0, 0.0]),
            ("scale", [kx, ky]) => Xf([*kx, 0.0, 0.0, *ky, 0.0, 0.0]),
            ("rotate", [deg]) => rotation(*deg, 0.0, 0.0),
            ("rotate", [deg, cx, cy]) => rotation(*deg, *cx, *cy),
            ("matrix", [a, b, c, d, e, f]) => Xf([*a, *b, *c, *d, *e, *f]),
            _ => continue,
        };
        xf = xf.then(step);
    }
    xf
}

fn rotation(deg: f32, cx: f32, cy: f32) -> Xf {
    let (s, c) = deg.to_radians().sin_cos();
    // translate(cx cy) · rotate(deg) · translate(-cx -cy)
    Xf([c, s, -s, c, cx - c * cx + s * cy, cy - s * cx - c * cy])
}

/// A positioned run of text: the anchor point in viewport coordinates plus the
/// inherited font size (the line-banding tolerance).
struct Label {
    x: f32,
    y: f32,
    fs: f32,
    text: String,
}

/// First numeric token of a coordinate list attribute (`x="10 20 30"` anchors
/// the run at 10; per-glyph positioning beyond that is typography, not order).
fn first_coord(node: XmlNode, attr: &str) -> Option<f32> {
    parse_len(node.attribute(attr)?)
}

/// Parse an SVG length, accepting the units that keep their user-space scale
/// (`px`, `pt` — close enough for ordering) and bare numbers. Percentages and
/// font-relative units need layout context we don't have; `None` skips them.
fn parse_len(s: &str) -> Option<f32> {
    let t = s.trim().trim_end_matches("px").trim_end_matches("pt");
    t.trim().parse().ok()
}

/// The element's own font-size, from the presentation attribute or an inline
/// `style="…font-size:…"` declaration.
fn own_font_size(node: XmlNode) -> Option<f32> {
    if let Some(v) = node.attribute("font-size").and_then(parse_len) {
        return Some(v);
    }
    let style = node.attribute("style")?;
    style.split(';').find_map(|decl| {
        let (k, v) = decl.split_once(':')?;
        (k.trim() == "font-size").then(|| parse_len(v)).flatten()
    })
}

fn is_hidden(node: XmlNode) -> bool {
    if node.attribute("display") == Some("none") || node.attribute("visibility") == Some("hidden") {
        return true;
    }
    node.attribute("style").is_some_and(|s| {
        s.split(';').any(|decl| {
            matches!(
                decl.split_once(':').map(|(k, v)| (k.trim(), v.trim())),
                Some(("display", "none")) | Some(("visibility", "hidden"))
            )
        })
    })
}

/// Containers a viewer does not paint directly; `<title>`/`<desc>` are
/// accessibility strings, not canvas text (the root pair is lifted separately).
const UNRENDERED: &[&str] = &[
    "defs", "clipPath", "mask", "pattern", "symbol", "marker", "filter", "style", "script",
    "metadata", "title", "desc",
];

/// Depth-first walk collecting `<text>` labels, composing transforms and
/// inheriting font-size on the way down.
fn walk(node: XmlNode, xf: Xf, fs: f32, labels: &mut Vec<Label>) {
    for child in node.children().filter(XmlNode::is_element) {
        let tag = child.tag_name().name();
        if UNRENDERED.contains(&tag) || is_hidden(child) {
            continue;
        }
        let cxf = match child.attribute("transform") {
            Some(t) => xf.then(parse_transform(t)),
            None => xf,
        };
        let cfs = own_font_size(child).unwrap_or(fs);
        if tag == "text" {
            collect_text(child, cxf, cfs, labels);
        } else {
            walk(child, cxf, cfs, labels);
        }
    }
}

/// Flatten one `<text>` element into labels. A `<tspan>` with an absolute
/// `x`/`y` starts a new run (that's how multi-line SVG text is authored);
/// `dx`/`dy` nudge the pen; plain nested spans continue the current run.
fn collect_text(text: XmlNode, xf: Xf, fs: f32, labels: &mut Vec<Label>) {
    let mut walk = TextWalk {
        xf,
        pen: (
            first_coord(text, "x").unwrap_or(0.0),
            first_coord(text, "y").unwrap_or(0.0),
        ),
        run: String::new(),
        run_pos: (0.0, 0.0),
        run_fs: fs,
        labels,
    };
    walk.run_pos = walk.pen;
    walk.rec(text, fs);
    walk.flush();
}

/// Pen-and-run state for flattening one `<text>` element (the pen tracks
/// where the next glyph lands, the run accumulates text laid down since the
/// pen last jumped).
struct TextWalk<'a> {
    xf: Xf,
    pen: (f32, f32),
    run: String,
    run_pos: (f32, f32),
    run_fs: f32,
    labels: &'a mut Vec<Label>,
}

impl TextWalk<'_> {
    fn flush(&mut self) {
        let t = normalize_ws(&self.run);
        self.run.clear();
        if !t.is_empty() {
            let (x, y) = self.xf.apply(self.run_pos.0, self.run_pos.1);
            self.labels.push(Label {
                x,
                y,
                fs: self.run_fs,
                text: t,
            });
        }
    }

    fn rec(&mut self, node: XmlNode, fs: f32) {
        for child in node.children() {
            if child.is_text() {
                if self.run.is_empty() {
                    self.run_pos = self.pen;
                    self.run_fs = fs;
                }
                self.run.push_str(child.text().unwrap_or(""));
                continue;
            }
            if !child.is_element() || is_hidden(child) {
                continue;
            }
            let cfs = own_font_size(child).unwrap_or(fs);
            if child.has_attribute("x") || child.has_attribute("y") {
                self.flush();
                if let Some(x) = first_coord(child, "x") {
                    self.pen.0 = x;
                }
                if let Some(y) = first_coord(child, "y") {
                    self.pen.1 = y;
                }
            }
            // Relative nudges: em-valued dy (line-steps) resolves against the
            // current font size; anything unparsable moves nothing.
            for (attr, axis) in [("dx", 0usize), ("dy", 1usize)] {
                if let Some(raw) = child.attribute(attr) {
                    let delta = match raw.trim().strip_suffix("em") {
                        Some(n) => n.trim().parse::<f32>().ok().map(|v| v * cfs),
                        None => parse_len(raw),
                    };
                    if let Some(d) = delta {
                        if d != 0.0 {
                            self.flush();
                            if axis == 0 {
                                self.pen.0 += d;
                            } else {
                                self.pen.1 += d;
                            }
                        }
                    }
                }
            }
            self.rec(child, cfs);
        }
    }
}

/// Collapse whitespace runs (default `xml:space` semantics).
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Extract the document: root `<title>` → level-1 heading, root `<desc>` → a
/// paragraph, then every visual line of canvas text as its own paragraph.
pub(crate) fn extract_text(source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
    let xml = source.text()?;
    let tree = Document::parse(xml).map_err(|e| ConversionError::Parse(format!("svg: {e}")))?;
    let root = tree.root_element();
    if root.tag_name().name() != "svg" {
        return Err(ConversionError::Parse(format!(
            "svg: root element is <{}>, not <svg>",
            root.tag_name().name()
        )));
    }

    let mut doc = DoclingDocument::new(&source.name);
    for child in root.children().filter(XmlNode::is_element) {
        match child.tag_name().name() {
            "title" => {
                let t = normalize_ws(child.text().unwrap_or(""));
                if !t.is_empty() {
                    doc.push(Node::Heading { level: 1, text: t });
                }
            }
            "desc" => {
                let t = normalize_ws(child.text().unwrap_or(""));
                if !t.is_empty() {
                    doc.push(Node::Paragraph { text: t });
                }
            }
            _ => {}
        }
    }

    // 16px: the CSS/SVG initial font-size, the tolerance when nothing sets one.
    let mut labels = Vec::new();
    walk(root, Xf::IDENTITY, 16.0, &mut labels);

    // Band labels into visual lines: sort by y, start a new line when the next
    // anchor drops below the running line by more than ~half a line-height,
    // then read each line left-to-right. (The same banding idea as the Visio
    // backend and the PDF assembler's line grouping.)
    labels.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    let mut i = 0;
    while i < labels.len() {
        let mut j = i + 1;
        let mut band_y = labels[i].y;
        let mut band_fs = labels[i].fs;
        while j < labels.len() && labels[j].y - band_y <= 0.6 * band_fs.max(labels[j].fs) {
            band_y = labels[j].y;
            band_fs = band_fs.max(labels[j].fs);
            j += 1;
        }
        let mut line: Vec<&Label> = labels[i..j].iter().collect();
        line.sort_by(|a, b| a.x.total_cmp(&b.x));
        let text = line
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        doc.push(Node::Paragraph { text });
        i = j;
    }

    if doc.nodes.is_empty() {
        return Err(ConversionError::Parse(
            "SVG contains no text elements; graphical content needs the ML image pipeline \
             (a build with the `pdf` feature, converting without --no-ocr)"
                .into(),
        ));
    }
    Ok(doc)
}

impl DeclarativeBackend for SvgBackend {
    fn convert(&self, source: &SourceDocument) -> Result<DoclingDocument, ConversionError> {
        extract_text(source)
    }
}

/// Rasterize the SVG to a PNG for the image ML pipeline. White-backed (an SVG
/// canvas is transparent by default, but layout/OCR are trained on paper), and
/// scaled so the long side lands near 2048 px — enough for OCR on small
/// labels, bounded for poster-sized drawings.
#[cfg(feature = "pdf")]
pub(crate) fn rasterize_png(bytes: &[u8]) -> Result<Vec<u8>, ConversionError> {
    use resvg::{tiny_skia, usvg};

    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    if opt.fontdb.is_empty() {
        // Degrade loudly: shapes still rasterize, but every <text> silently
        // vanishing from OCR's view is worth a warning.
        eprintln!("docling: warning: no system fonts found; SVG text will not rasterize");
    }
    let tree = usvg::Tree::from_data(bytes, &opt)
        .map_err(|e| ConversionError::Parse(format!("svg: {e}")))?;
    let (w, h) = (tree.size().width(), tree.size().height());
    let long = w.max(h).max(1.0);
    let scale = (2048.0 / long).clamp(0.05, 8.0);
    let (pw, ph) = (
        (w * scale).round().max(1.0) as u32,
        (h * scale).round().max(1.0) as u32,
    );
    let mut pixmap = tiny_skia::Pixmap::new(pw, ph)
        .ok_or_else(|| ConversionError::Parse(format!("svg: bad raster size {pw}x{ph}")))?;
    pixmap.fill(tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap
        .encode_png()
        .map_err(|e| ConversionError::Parse(format!("svg: png encode: {e}")))
}
