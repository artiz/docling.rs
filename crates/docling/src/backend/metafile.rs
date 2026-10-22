//! Windows metafiles (EMF / WMF) to PNG (#536).
//!
//! Word, Visio, older clip art and pasted screenshots store pictures as
//! Windows metafiles: a list of GDI drawing records rather than pixels. The
//! `image` crate reads neither format, so such a picture used to reach the
//! document without an image; docling upstream renders it through
//! LibreOffice. Here the records are translated into an SVG document
//! (shapes, pens and brushes, paths, embedded DIB bitmaps, text) that
//! [resvg] rasterizes — the same rasterizer and host fonts SVG input uses —
//! into the PNG payload docling's `ImageRef` carries.
//!
//! Covered: the EMF records Office writes for drawings (polygons, poly-lines
//! and Béziers in both 16- and 32-bit forms, rectangles, rounded rectangles,
//! ellipses, arcs / chords / pies, path brackets, `MoveTo` / `LineTo`,
//! window / viewport / world transforms, `SaveDC` / `RestoreDC`, pens,
//! brushes, stock objects, `ExtTextOutW`, and the bitmap records
//! `StretchDIBits` / `BitBlt` / `StretchBlt` / `SetDIBitsToDevice`), and the
//! equivalent WMF records (with or without the placeable header). Not
//! covered: clipping regions (the picture's own frame still clips), raster
//! operations beyond copy / pattern fill, hatch and pattern brushes (drawn
//! solid), and the EMF+ records of a dual EMF+ file — its plain-EMF
//! fallback records are what is drawn. A metafile that draws nothing
//! recognisable yields `None`, and the picture stays a placeholder as before.
//!
//! [resvg]: https://docs.rs/resvg

// Without the `pdf` feature (wasm) there is no rasterizer: `render` returns
// `None` up front and the interpreter is unused.
#![cfg_attr(not(feature = "pdf"), allow(dead_code))]

use std::fmt::Write as _;

use docling_core::PictureImage;

/// The longest side a rendered metafile gets, in pixels.
const MAX_SIDE: f64 = 2048.0;

/// Render an EMF or WMF to a PNG [`PictureImage`]. `size_hint` is the
/// picture's size in pixels when the metafile cannot say (a Word 97 WMF
/// BLIP drops the placeable header). `None` when `data` is not a metafile,
/// draws nothing, or the build has no rasterizer (no `pdf` feature).
pub(crate) fn render(data: &[u8], size_hint: Option<(f64, f64)>) -> Option<PictureImage> {
    if cfg!(not(feature = "pdf")) {
        return None;
    }
    let svg = to_svg(data, size_hint)?;
    rasterize(&svg)
}

/// Whether `data` is an EMF (`EMR_HEADER` with the `" EMF"` signature).
pub(crate) fn is_emf(data: &[u8]) -> bool {
    u32_at(data, 0) == Some(1) && data.get(40..44) == Some(b" EMF")
}

/// Whether `data` is a WMF: the placeable key `9AC6CDD7`, or a bare
/// `META_HEADER` (memory / disk type, 9-word header, version 0x0100/0x0300).
pub(crate) fn is_wmf(data: &[u8]) -> bool {
    if u32_at(data, 0) == Some(0x9AC6_CDD7) {
        return true;
    }
    matches!(u16_at(data, 0), Some(1 | 2))
        && u16_at(data, 2) == Some(9)
        && matches!(u16_at(data, 4), Some(0x0100 | 0x0300))
}

/// A metafile as an SVG document plus the pixel size it should render at.
pub(crate) struct Svg {
    pub(crate) doc: String,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

pub(crate) fn to_svg(data: &[u8], size_hint: Option<(f64, f64)>) -> Option<Svg> {
    if is_emf(data) {
        emf_to_svg(data)
    } else if is_wmf(data) {
        wmf_to_svg(data, size_hint)
    } else {
        None
    }
}

#[cfg(feature = "pdf")]
fn rasterize(svg: &Svg) -> Option<PictureImage> {
    use resvg::{tiny_skia, usvg};
    use std::sync::{Arc, OnceLock};

    // System fonts load once per process: a document can hold hundreds of
    // metafiles.
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    let fonts = FONTS.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    });
    let opt = usvg::Options {
        fontdb: fonts.clone(),
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(&svg.doc, &opt).ok()?;
    let (pw, ph) = (
        svg.width.round().clamp(1.0, MAX_SIDE) as u32,
        svg.height.round().clamp(1.0, MAX_SIDE) as u32,
    );
    let mut pixmap = tiny_skia::Pixmap::new(pw, ph)?;
    // Office composes a metafile on the page's white.
    pixmap.fill(tiny_skia::Color::WHITE);
    let size = tree.size();
    let transform = tiny_skia::Transform::from_scale(
        pw as f32 / size.width().max(1e-3),
        ph as f32 / size.height().max(1e-3),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let data = pixmap.encode_png().ok()?;
    Some(PictureImage {
        dpi: PictureImage::DEFAULT_DPI,
        mimetype: "image/png".into(),
        width: pw,
        height: ph,
        data,
        display_size: None,
    })
}

#[cfg(not(feature = "pdf"))]
fn rasterize(_svg: &Svg) -> Option<PictureImage> {
    None
}

/// The pixel size for a picture `w` × `h` points-ish units already in
/// 96-dpi pixels, scaled into `1..=MAX_SIDE` on the long side.
fn fit(w: f64, h: f64) -> (f64, f64) {
    let (w, h) = (w.abs().max(1.0), h.abs().max(1.0));
    let long = w.max(h);
    let s = if long > MAX_SIDE {
        MAX_SIDE / long
    } else {
        1.0
    };
    ((w * s).max(1.0), (h * s).max(1.0))
}

// ---------------------------------------------------------------------------
// GDI state shared by both formats

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rgb(u8, u8, u8);

impl Rgb {
    fn from_colorref(v: u32) -> Self {
        Rgb(v as u8, (v >> 8) as u8, (v >> 16) as u8)
    }
    fn css(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

#[derive(Clone, Debug)]
struct Pen {
    color: Rgb,
    /// Logical width; 0 = cosmetic (one device pixel).
    width: f64,
    null: bool,
}

#[derive(Clone, Debug)]
struct Brush {
    color: Rgb,
    null: bool,
}

#[derive(Clone, Debug)]
struct Font {
    /// Logical height (negative = character height, positive = cell height).
    height: f64,
    weight: u32,
    italic: bool,
    face: String,
    /// Tenths of a degree, counter-clockwise.
    escapement: f64,
}

#[derive(Clone, Debug)]
enum Obj {
    Pen(Pen),
    Brush(Brush),
    Font(Font),
    /// Palettes, regions, … — they take an object slot but draw nothing.
    Other,
}

/// An affine map `(x, y) → (a·x + c·y + e, b·x + d·y + f)`.
#[derive(Clone, Copy, Debug)]
struct Xf {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Xf {
    const ID: Xf = Xf {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };
    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }
    /// `self` then `o`.
    fn then(&self, o: &Xf) -> Xf {
        Xf {
            a: self.a * o.a + self.b * o.c,
            b: self.a * o.b + self.b * o.d,
            c: self.c * o.a + self.d * o.c,
            d: self.c * o.b + self.d * o.d,
            e: self.e * o.a + self.f * o.c + o.e,
            f: self.e * o.b + self.f * o.d + o.f,
        }
    }
}

#[derive(Clone, Debug)]
struct Dc {
    pen: Pen,
    brush: Brush,
    font: Font,
    text_color: Rgb,
    text_align: u32,
    /// `ALTERNATE` (even-odd) vs `WINDING` polygon fill.
    winding: bool,
    map_mode: u32,
    win_org: (f64, f64),
    win_ext: (f64, f64),
    vp_org: (f64, f64),
    vp_ext: (f64, f64),
    world: Xf,
    cur: (f64, f64),
}

impl Dc {
    fn new() -> Self {
        Dc {
            pen: Pen {
                color: Rgb(0, 0, 0),
                width: 0.0,
                null: false,
            },
            brush: Brush {
                color: Rgb(255, 255, 255),
                null: false,
            },
            font: Font {
                height: -12.0,
                weight: 400,
                italic: false,
                face: String::new(),
                escapement: 0.0,
            },
            text_color: Rgb(0, 0, 0),
            text_align: 0,
            winding: false,
            map_mode: 1,
            win_org: (0.0, 0.0),
            win_ext: (1.0, 1.0),
            vp_org: (0.0, 0.0),
            vp_ext: (1.0, 1.0),
            world: Xf::ID,
            cur: (0.0, 0.0),
        }
    }
}

/// The record interpreter: GDI state in, SVG elements out. `page` maps the
/// page space (after the world transform) to the SVG's user space — the
/// format decides it (EMF: window → viewport → the frame's device pixels;
/// WMF: the window itself is the picture's space).
struct Gdi {
    dc: Dc,
    stack: Vec<Dc>,
    objects: Vec<Option<Obj>>,
    out: String,
    drawn: usize,
    /// An open path bracket (`BeginPath` … `EndPath`): its SVG path data.
    path: Option<String>,
    /// The bracket's finished path, waiting for `FillPath` / `StrokePath`.
    closed_path: Option<String>,
    /// MM_* fixed-unit modes scale to device pixels by this (px per mm).
    px_per_mm: (f64, f64),
    /// WMF: the logical space is the SVG's (no window → viewport step).
    wmf: bool,
}

impl Gdi {
    fn new(wmf: bool, px_per_mm: (f64, f64)) -> Self {
        Gdi {
            dc: Dc::new(),
            stack: Vec::new(),
            objects: Vec::new(),
            out: String::new(),
            drawn: 0,
            path: None,
            closed_path: None,
            px_per_mm,
            wmf,
        }
    }

    /// Logical → SVG user space.
    fn xf(&self) -> Xf {
        let dc = &self.dc;
        let page = if self.wmf {
            // The picture's space is the window: origin at its corner, the
            // extent's sign flipping an axis (y-up WMFs).
            let sx = if dc.win_ext.0 < 0.0 { -1.0 } else { 1.0 };
            let sy = if dc.win_ext.1 < 0.0 { -1.0 } else { 1.0 };
            Xf {
                a: sx,
                b: 0.0,
                c: 0.0,
                d: sy,
                e: -dc.win_org.0 * sx,
                f: -dc.win_org.1 * sy,
            }
        } else {
            match dc.map_mode {
                // MM_ISOTROPIC / MM_ANISOTROPIC: window → viewport.
                7 | 8 => {
                    let sx = dc.vp_ext.0 / nonzero(dc.win_ext.0);
                    let mut sy = dc.vp_ext.1 / nonzero(dc.win_ext.1);
                    if dc.map_mode == 7 {
                        // Isotropic: one scale, the smaller, signs kept.
                        let s = sx.abs().min(sy.abs());
                        sy = s * sy.signum();
                        let sx2 = s * sx.signum();
                        return self.page_xf(sx2, sy);
                    }
                    return self.page_xf(sx, sy);
                }
                // Fixed units, y up: LOMETRIC 0.1 mm, HIMETRIC 0.01 mm,
                // LOENGLISH 0.01", HIENGLISH 0.001", TWIPS 1/1440".
                2..=6 => {
                    let mm = match dc.map_mode {
                        2 => 0.1,
                        3 => 0.01,
                        4 => 0.254,
                        5 => 0.0254,
                        _ => 25.4 / 1440.0,
                    };
                    return self.page_xf(mm * self.px_per_mm.0, -mm * self.px_per_mm.1);
                }
                // MM_TEXT: logical = device, offset by the origins.
                _ => return self.page_xf(1.0, 1.0),
            }
        };
        dc.world.then(&page)
    }

    fn page_xf(&self, sx: f64, sy: f64) -> Xf {
        let dc = &self.dc;
        let page = Xf {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            e: dc.vp_org.0 - dc.win_org.0 * sx,
            f: dc.vp_org.1 - dc.win_org.1 * sy,
        };
        dc.world.then(&page)
    }

    /// The average scale of the current transform (pen widths, font sizes).
    fn scale(&self) -> (f64, f64) {
        let x = self.xf();
        (
            (x.a * x.a + x.b * x.b).sqrt(),
            (x.c * x.c + x.d * x.d).sqrt(),
        )
    }

    fn pt(&self, x: f64, y: f64) -> (f64, f64) {
        self.xf().apply(x, y)
    }

    // ----- objects ---------------------------------------------------------

    /// WMF: a created object takes the lowest free slot.
    fn add_object(&mut self, obj: Obj) {
        if let Some(slot) = self.objects.iter_mut().find(|o| o.is_none()) {
            *slot = Some(obj);
        } else if self.objects.len() < 65_535 {
            self.objects.push(Some(obj));
        }
    }

    /// EMF: an object goes to the slot its record names.
    fn set_object(&mut self, index: u32, obj: Obj) {
        let i = index as usize;
        if i > 65_535 {
            return;
        }
        if self.objects.len() <= i {
            self.objects.resize(i + 1, None);
        }
        self.objects[i] = Some(obj);
    }

    fn select(&mut self, index: u32) {
        if index & 0x8000_0000 != 0 {
            // EMF stock objects.
            match index & 0x7FFF_FFFF {
                0 => self.dc.brush = solid(Rgb(255, 255, 255)),
                1 => self.dc.brush = solid(Rgb(192, 192, 192)),
                2 => self.dc.brush = solid(Rgb(128, 128, 128)),
                3 => self.dc.brush = solid(Rgb(64, 64, 64)),
                4 => self.dc.brush = solid(Rgb(0, 0, 0)),
                5 => self.dc.brush.null = true,
                6 => self.dc.pen = pen(Rgb(255, 255, 255)),
                7 => self.dc.pen = pen(Rgb(0, 0, 0)),
                8 => self.dc.pen.null = true,
                _ => {}
            }
            return;
        }
        match self.objects.get(index as usize).cloned().flatten() {
            Some(Obj::Pen(p)) => self.dc.pen = p,
            Some(Obj::Brush(b)) => self.dc.brush = b,
            Some(Obj::Font(f)) => self.dc.font = f,
            _ => {}
        }
    }

    fn delete(&mut self, index: u32) {
        if let Some(slot) = self.objects.get_mut(index as usize) {
            *slot = None;
        }
    }

    fn save(&mut self) {
        if self.stack.len() < 1024 {
            self.stack.push(self.dc.clone());
        }
    }

    /// `RestoreDC(n)`: negative = relative to the top, positive = absolute.
    fn restore(&mut self, n: i32) {
        let depth = if n < 0 {
            self.stack.len().checked_sub(n.unsigned_abs() as usize)
        } else {
            (n as usize).checked_sub(1)
        };
        if let Some(d) = depth.filter(|&d| d < self.stack.len()) {
            self.dc = self.stack[d].clone();
            self.stack.truncate(d);
        }
    }

    // ----- drawing ---------------------------------------------------------

    fn fill_attrs(&self, fill: bool) -> String {
        if fill && !self.dc.brush.null {
            format!(
                " fill=\"{}\" fill-rule=\"{}\"",
                self.dc.brush.color.css(),
                if self.dc.winding {
                    "nonzero"
                } else {
                    "evenodd"
                }
            )
        } else {
            " fill=\"none\"".into()
        }
    }

    fn stroke_attrs(&self, stroke: bool) -> String {
        if stroke && !self.dc.pen.null {
            let (sx, sy) = self.scale();
            let w = (self.dc.pen.width * (sx + sy) / 2.0).max(1.0);
            format!(
                " stroke=\"{}\" stroke-width=\"{:.3}\" stroke-linejoin=\"round\"",
                self.dc.pen.color.css(),
                w
            )
        } else {
            String::new()
        }
    }

    /// Emit (or, inside a path bracket, collect) path data `d`.
    fn shape(&mut self, d: String, fill: bool, stroke: bool) {
        if d.is_empty() {
            return;
        }
        if let Some(path) = self.path.as_mut() {
            path.push_str(&d);
            return;
        }
        if (!fill || self.dc.brush.null) && (!stroke || self.dc.pen.null) {
            return;
        }
        let attrs = format!("{}{}", self.fill_attrs(fill), self.stroke_attrs(stroke));
        let _ = writeln!(self.out, "<path d=\"{d}\"{attrs}/>");
        self.drawn += 1;
    }

    fn poly_d(&self, pts: &[(f64, f64)], close: bool) -> String {
        let mut d = String::new();
        for (i, &(x, y)) in pts.iter().enumerate() {
            let (px, py) = self.pt(x, y);
            let _ = write!(d, "{}{:.3} {:.3} ", if i == 0 { 'M' } else { 'L' }, px, py);
        }
        if close && !pts.is_empty() {
            d.push_str("Z ");
        }
        d
    }

    fn polygon(&mut self, pts: &[(f64, f64)]) {
        let d = self.poly_d(pts, true);
        self.shape(d, true, true);
    }

    fn polyline(&mut self, pts: &[(f64, f64)]) {
        let d = self.poly_d(pts, false);
        self.shape(d, false, true);
    }

    fn polypolygon(&mut self, polys: &[Vec<(f64, f64)>]) {
        let d: String = polys.iter().map(|p| self.poly_d(p, true)).collect();
        self.shape(d, true, true);
    }

    /// Cubic Béziers: `start` then groups of three points.
    fn bezier(&mut self, start: (f64, f64), pts: &[(f64, f64)], continue_path: bool) {
        let (sx, sy) = self.pt(start.0, start.1);
        let mut d = if continue_path && self.path.is_some() {
            String::new()
        } else {
            format!("M{sx:.3} {sy:.3} ")
        };
        for c in pts.chunks_exact(3) {
            let p: Vec<(f64, f64)> = c.iter().map(|&(x, y)| self.pt(x, y)).collect();
            let _ = write!(
                d,
                "C{:.3} {:.3} {:.3} {:.3} {:.3} {:.3} ",
                p[0].0, p[0].1, p[1].0, p[1].1, p[2].0, p[2].1
            );
        }
        if let Some(&last) = pts.chunks_exact(3).last().map(|c| &c[2]) {
            self.dc.cur = last;
        }
        self.shape(d, false, true);
    }

    fn line_to(&mut self, x: f64, y: f64) {
        let (x0, y0) = self.dc.cur;
        let xf = self.xf();
        if let Some(path) = &mut self.path {
            let (px, py) = xf.apply(x, y);
            let (sx, sy) = xf.apply(x0, y0);
            if path.is_empty() {
                let _ = write!(path, "M{sx:.3} {sy:.3} ");
            }
            let _ = write!(path, "L{px:.3} {py:.3} ");
        } else {
            let d = self.poly_d(&[(x0, y0), (x, y)], false);
            self.shape(d, false, true);
        }
        self.dc.cur = (x, y);
    }

    fn move_to(&mut self, x: f64, y: f64) {
        self.dc.cur = (x, y);
        let (px, py) = self.xf().apply(x, y);
        if let Some(path) = self.path.as_mut() {
            let _ = write!(path, "M{px:.3} {py:.3} ");
        }
    }

    fn rect(&mut self, l: f64, t: f64, r: f64, b: f64) {
        self.polygon(&[(l, t), (r, t), (r, b), (l, b)]);
    }

    /// An ellipse in the box, as four Béziers (transform-safe).
    fn ellipse_d(&self, l: f64, t: f64, r: f64, b: f64) -> String {
        let (cx, cy, rx, ry) = ((l + r) / 2.0, (t + b) / 2.0, (r - l) / 2.0, (b - t) / 2.0);
        let k = 0.552_284_75;
        let p = |x: f64, y: f64| self.pt(x, y);
        let pts = [
            p(cx + rx, cy),
            p(cx + rx, cy + ry * k),
            p(cx + rx * k, cy + ry),
            p(cx, cy + ry),
            p(cx - rx * k, cy + ry),
            p(cx - rx, cy + ry * k),
            p(cx - rx, cy),
            p(cx - rx, cy - ry * k),
            p(cx - rx * k, cy - ry),
            p(cx, cy - ry),
            p(cx + rx * k, cy - ry),
            p(cx + rx, cy - ry * k),
            p(cx + rx, cy),
        ];
        let mut d = format!("M{:.3} {:.3} ", pts[0].0, pts[0].1);
        for c in pts[1..].chunks_exact(3) {
            let _ = write!(
                d,
                "C{:.3} {:.3} {:.3} {:.3} {:.3} {:.3} ",
                c[0].0, c[0].1, c[1].0, c[1].1, c[2].0, c[2].1
            );
        }
        d.push_str("Z ");
        d
    }

    fn ellipse(&mut self, l: f64, t: f64, r: f64, b: f64) {
        let d = self.ellipse_d(l, t, r, b);
        self.shape(d, true, true);
    }

    fn round_rect(&mut self, l: f64, t: f64, r: f64, b: f64, w: f64, h: f64) {
        // Corners as straight cuts would lose the look; a polygon through
        // each quarter-ellipse's midpoints is close enough at icon sizes.
        let (rx, ry) = ((w / 2.0).min((r - l) / 2.0), (h / 2.0).min((b - t) / 2.0));
        let k = 1.0 - std::f64::consts::FRAC_1_SQRT_2;
        let pts = [
            (l + rx, t),
            (r - rx, t),
            (r - rx * k, t + ry * k),
            (r, t + ry),
            (r, b - ry),
            (r - rx * k, b - ry * k),
            (r - rx, b),
            (l + rx, b),
            (l + rx * k, b - ry * k),
            (l, b - ry),
            (l, t + ry),
            (l + rx * k, t + ry * k),
        ];
        self.polygon(&pts);
    }

    /// `Arc` / `Chord` / `Pie`: the ellipse in the box from the radial
    /// through `start` counter-clockwise to the one through `end`, sampled.
    #[allow(clippy::too_many_arguments)]
    fn arc(&mut self, b: [f64; 4], start: (f64, f64), end: (f64, f64), kind: u8) {
        let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        let (rx, ry) = ((b[2] - b[0]) / 2.0, (b[3] - b[1]) / 2.0);
        let a0 = (-(start.1 - cy)).atan2(start.0 - cx);
        let mut a1 = (-(end.1 - cy)).atan2(end.0 - cx);
        if a1 <= a0 {
            a1 += std::f64::consts::TAU;
        }
        let n = 32;
        let mut pts: Vec<(f64, f64)> = (0..=n)
            .map(|i| {
                let a = a0 + (a1 - a0) * i as f64 / n as f64;
                (cx + rx * a.cos(), cy - ry * a.sin())
            })
            .collect();
        match kind {
            0 => self.polyline(&pts),
            1 => self.polygon(&pts),
            _ => {
                pts.push((cx, cy));
                self.polygon(&pts);
            }
        }
    }

    /// Text at logical `(x, y)` per the current font, colour and alignment.
    fn text(&mut self, x: f64, y: f64, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        if text.trim().is_empty() {
            return;
        }
        let (x, y) = if self.dc.text_align & 1 != 0 {
            self.dc.cur // TA_UPDATECP
        } else {
            (x, y)
        };
        let (px, py) = self.pt(x, y);
        let (_, sy) = self.scale();
        let f = &self.dc.font;
        let mut size = f.height.abs() * sy;
        if f.height > 0.0 {
            // Cell height: the character part is roughly 80 % of it.
            size *= 0.8;
        }
        if size < 0.5 {
            size = 12.0 * sy.max(1e-3);
        }
        let v = self.dc.text_align & 24;
        let baseline = match v {
            24 => py,              // TA_BASELINE
            8 => py - size * 0.22, // TA_BOTTOM
            _ => py + size * 0.88, // TA_TOP
        };
        let anchor = match self.dc.text_align & 6 {
            6 => "middle",
            2 => "end",
            _ => "start",
        };
        let family = font_family(&f.face);
        let mut attrs = format!(
            "x=\"{px:.3}\" y=\"{baseline:.3}\" font-size=\"{size:.3}\" font-family=\"{family}\" fill=\"{}\" text-anchor=\"{anchor}\"",
            self.dc.text_color.css()
        );
        if f.weight >= 600 {
            attrs.push_str(" font-weight=\"bold\"");
        }
        if f.italic {
            attrs.push_str(" font-style=\"italic\"");
        }
        if f.escapement.abs() > 0.5 {
            let _ = write!(
                attrs,
                " transform=\"rotate({:.2} {px:.3} {py:.3})\"",
                -f.escapement / 10.0
            );
        }
        let _ = writeln!(self.out, "<text {attrs}>{}</text>", xml_escape(&text));
        self.drawn += 1;
    }

    /// A bitmap (`dib` = BITMAPINFO + bits) into the logical rectangle;
    /// `src` = the part of the bitmap to show, when not all of it.
    fn bitmap(&mut self, dest: [f64; 4], dib: &[u8], bits: &[u8], src: Option<[i64; 4]>) {
        let Some(mut img) = decode_dib(dib, bits) else {
            return;
        };
        if let Some([sx, sy, sw, sh]) = src {
            let (iw, ih) = (img.width() as i64, img.height() as i64);
            if sw > 0 && sh > 0 && (sx, sy, sw, sh) != (0, 0, iw, ih) {
                let (x, y) = (sx.clamp(0, iw), sy.clamp(0, ih));
                let (w, h) = (sw.min(iw - x), sh.min(ih - y));
                if w > 0 && h > 0 {
                    img = img.crop_imm(x as u32, y as u32, w as u32, h as u32);
                }
            }
        }
        let mut png = std::io::Cursor::new(Vec::new());
        if img.write_to(&mut png, image::ImageFormat::Png).is_err() {
            return;
        }
        let corners = [
            self.pt(dest[0], dest[1]),
            self.pt(dest[0] + dest[2], dest[1] + dest[3]),
        ];
        let (x0, x1) = (
            corners[0].0.min(corners[1].0),
            corners[0].0.max(corners[1].0),
        );
        let (y0, y1) = (
            corners[0].1.min(corners[1].1),
            corners[0].1.max(corners[1].1),
        );
        if x1 - x0 < 1e-6 || y1 - y0 < 1e-6 {
            return;
        }
        let _ = writeln!(
            self.out,
            "<image x=\"{x0:.3}\" y=\"{y0:.3}\" width=\"{:.3}\" height=\"{:.3}\" preserveAspectRatio=\"none\" href=\"data:image/png;base64,{}\"/>",
            x1 - x0,
            y1 - y0,
            docling_core::base64::encode(&png.into_inner())
        );
        self.drawn += 1;
    }

    /// A pattern / solid raster op without a source bitmap over the
    /// logical rectangle (`PATCOPY` with the brush, `BLACKNESS`,
    /// `WHITENESS`; anything else is skipped).
    fn pattern_blt(&mut self, dest: [f64; 4], rop: u32) {
        let color = match rop {
            0x00F0_0021 if !self.dc.brush.null => self.dc.brush.color,
            0x0000_0042 => Rgb(0, 0, 0),
            0x00FF_0062 => Rgb(255, 255, 255),
            _ => return,
        };
        let saved = (self.dc.brush.clone(), self.dc.pen.clone());
        self.dc.brush = solid(color);
        self.dc.pen.null = true;
        self.rect(dest[0], dest[1], dest[0] + dest[2], dest[1] + dest[3]);
        (self.dc.brush, self.dc.pen) = saved;
    }

    fn finish(self, view: [f64; 4], size: (f64, f64)) -> Option<Svg> {
        if self.drawn == 0 || view[2].abs() < 1e-9 || view[3].abs() < 1e-9 {
            return None;
        }
        let (w, h) = fit(size.0, size.1);
        let doc = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.3}\" height=\"{h:.3}\" viewBox=\"{:.3} {:.3} {:.3} {:.3}\" preserveAspectRatio=\"none\">\n{}</svg>\n",
            view[0], view[1], view[2], view[3], self.out
        );
        Some(Svg {
            doc,
            width: w,
            height: h,
        })
    }
}

fn nonzero(v: f64) -> f64 {
    if v.abs() < 1e-12 {
        1.0
    } else {
        v
    }
}

fn solid(color: Rgb) -> Brush {
    Brush { color, null: false }
}

fn pen(color: Rgb) -> Pen {
    Pen {
        color,
        width: 0.0,
        null: false,
    }
}

/// `LOGBRUSH`: style 0 solid, 1 null (hollow), 2 hatched (drawn solid in
/// its colour), 3+ pattern / DIB (drawn as mid grey).
fn brush(style: u32, color: u32) -> Brush {
    match style {
        1 => Brush {
            color: Rgb(255, 255, 255),
            null: true,
        },
        0 | 2 => solid(Rgb::from_colorref(color)),
        _ => solid(Rgb(160, 160, 160)),
    }
}

/// `PenStyle` 5 = `PS_NULL`.
fn pen_of(style: u32, width: f64, color: u32) -> Pen {
    Pen {
        color: Rgb::from_colorref(color),
        width,
        null: style & 0x0F == 5,
    }
}

/// A CSS font stack for a GDI face name, so a host without the face still
/// draws a look-alike.
fn font_family(face: &str) -> String {
    let lower = face.to_ascii_lowercase();
    let generic = if lower.contains("courier") || lower.contains("mono") || lower.contains("consol")
    {
        "'Liberation Mono', 'DejaVu Sans Mono', monospace"
    } else if lower.contains("times")
        || lower.contains("roman")
        || lower.contains("serif") && !lower.contains("sans")
        || lower.contains("georgia")
        || lower.contains("garamond")
    {
        "'Liberation Serif', 'DejaVu Serif', serif"
    } else {
        "Arial, 'Liberation Sans', 'DejaVu Sans', sans-serif"
    };
    let face: String = face
        .chars()
        .filter(|c| !c.is_control() && *c != '\'' && *c != '"' && *c != '<' && *c != '&')
        .collect();
    if face.trim().is_empty() {
        generic.to_string()
    } else {
        format!("'{}', {generic}", face.trim())
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A packed DIB (`BITMAPINFO` + colour table, then `bits`) decoded through
/// the `image` crate's BMP reader, behind a synthesized file header whose
/// pixel offset accounts for the colour table.
fn decode_dib(info: &[u8], bits: &[u8]) -> Option<image::DynamicImage> {
    let header_size = u32_at(info, 0)? as usize;
    if !(12..=124).contains(&header_size) || info.len() < header_size {
        return None;
    }
    let (bit_count, compression, clr_used) = if header_size == 12 {
        (u16_at(info, 10)? as u32, 0, 0)
    } else {
        (
            u16_at(info, 14)? as u32,
            u32_at(info, 16)?,
            u32_at(info, 32).unwrap_or(0),
        )
    };
    let entry = if header_size == 12 { 3 } else { 4 };
    let colors = if clr_used > 0 {
        clr_used as usize
    } else if bit_count <= 8 {
        1usize << bit_count
    } else {
        0
    };
    // BI_BITFIELDS with a 40-byte header: three masks follow it.
    let masks = if header_size == 40 && compression == 3 {
        12
    } else {
        0
    };
    let table = header_size + masks + colors * entry;
    let info = info.get(..table.min(info.len()))?;
    let mut bmp = Vec::with_capacity(14 + info.len() + bits.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((14 + info.len() + bits.len()) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&((14 + info.len()) as u32).to_le_bytes());
    bmp.extend_from_slice(info);
    bmp.extend_from_slice(bits);
    let img = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp).ok()?;
    if bit_count == 32 && compression == 0 {
        if let Some(rgba) = dib_alpha(&img, bits, i32_at(info, 8)? < 0) {
            return Some(image::DynamicImage::ImageRgba8(rgba));
        }
    }
    Some(img)
}

/// The alpha of a 32-bpp `BI_RGB` DIB. GDI ignores the fourth byte, but the
/// writers that turn transparent PNGs into EMFs fill it, and LibreOffice and
/// GDI+ honour it when it is not uniformly 0 — black corners otherwise frame
/// every such picture. Premultiplied pixels (every channel ≤ alpha, as
/// `AlphaBlend` wants them) are un-premultiplied. `None` when the channel
/// carries nothing (all 0 or all 255).
fn dib_alpha(img: &image::DynamicImage, bits: &[u8], top_down: bool) -> Option<image::RgbaImage> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let px = bits.get(..w.checked_mul(h)?.checked_mul(4)?)?;
    let alphas = || px.chunks_exact(4).map(|p| p[3]);
    if alphas().all(|a| a == 0) || alphas().all(|a| a == 255) {
        return None;
    }
    let premultiplied = px.chunks_exact(4).all(|p| p[0].max(p[1]).max(p[2]) <= p[3]);
    let mut out = img.to_rgba8();
    for (y, row) in px.chunks_exact(w * 4).enumerate() {
        let oy = if top_down { y } else { h - 1 - y };
        for (x, p) in row.chunks_exact(4).enumerate() {
            let a = p[3];
            let px = out.get_pixel_mut(x as u32, oy as u32);
            if premultiplied && a > 0 && a < 255 {
                for c in 0..3 {
                    px[c] =
                        ((u32::from(px[c]) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
                }
            }
            px[3] = a;
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// EMF

fn emf_to_svg(data: &[u8]) -> Option<Svg> {
    let rect = |o: usize| -> Option<[f64; 4]> {
        Some([
            i32_at(data, o)? as f64,
            i32_at(data, o + 4)? as f64,
            i32_at(data, o + 8)? as f64,
            i32_at(data, o + 12)? as f64,
        ])
    };
    let bounds = rect(8)?;
    let frame = rect(24)?; // 0.01 mm
    let dev = (i32_at(data, 72)? as f64, i32_at(data, 76)? as f64);
    let mm = (i32_at(data, 80)? as f64, i32_at(data, 84)? as f64);
    let px_per_mm = if dev.0 > 0.0 && dev.1 > 0.0 && mm.0 > 0.0 && mm.1 > 0.0 {
        (dev.0 / mm.0, dev.1 / mm.1)
    } else {
        (96.0 / 25.4, 96.0 / 25.4)
    };
    // The picture: the frame in device pixels (the bounds when the frame is
    // empty), rendered at 96 dpi.
    let view = if frame[2] > frame[0] && frame[3] > frame[1] {
        [
            frame[0] / 100.0 * px_per_mm.0,
            frame[1] / 100.0 * px_per_mm.1,
            (frame[2] - frame[0]) / 100.0 * px_per_mm.0,
            (frame[3] - frame[1]) / 100.0 * px_per_mm.1,
        ]
    } else {
        [
            bounds[0],
            bounds[1],
            bounds[2] - bounds[0] + 1.0,
            bounds[3] - bounds[1] + 1.0,
        ]
    };
    // Its 96-dpi size — or the reference device's pixels when those are
    // finer (an EMF wrapping a bitmap records it at the bitmap's own
    // resolution; rendering it smaller would throw pixels away).
    let size = if frame[2] > frame[0] && frame[3] > frame[1] {
        let dpi96 = (
            (frame[2] - frame[0]) / 2540.0 * 96.0,
            (frame[3] - frame[1]) / 2540.0 * 96.0,
        );
        if view[2] * view[3] > dpi96.0 * dpi96.1 {
            (view[2], view[3])
        } else {
            dpi96
        }
    } else {
        (view[2], view[3])
    };

    let mut g = Gdi::new(false, px_per_mm);
    let mut off = 0usize;
    let mut count = 0usize;
    while let (Some(kind), Some(size)) = (u32_at(data, off), u32_at(data, off + 4)) {
        let size = size as usize;
        if size < 8 || !size.is_multiple_of(4) || off + size > data.len() || count > 2_000_000 {
            break;
        }
        count += 1;
        let r = &data[off..off + size];
        emf_record(&mut g, kind, r);
        if kind == 14 {
            break; // EMR_EOF
        }
        off += size;
    }
    g.finish(view, size)
}

fn emf_points16(r: &[u8], at: usize, n: usize) -> Vec<(f64, f64)> {
    (0..n)
        .map_while(|i| {
            Some((
                i16_at(r, at + i * 4)? as f64,
                i16_at(r, at + i * 4 + 2)? as f64,
            ))
        })
        .collect()
}

fn emf_points32(r: &[u8], at: usize, n: usize) -> Vec<(f64, f64)> {
    (0..n)
        .map_while(|i| {
            Some((
                i32_at(r, at + i * 8)? as f64,
                i32_at(r, at + i * 8 + 4)? as f64,
            ))
        })
        .collect()
}

fn emf_record(g: &mut Gdi, kind: u32, r: &[u8]) {
    let u = |o: usize| u32_at(r, o).unwrap_or(0);
    let i = |o: usize| i32_at(r, o).unwrap_or(0) as f64;
    let f = |o: usize| f32_at(r, o).unwrap_or(0.0) as f64;
    // Point arrays: bounds (16) then a count at 24.
    let count = || (u(24) as usize).min(r.len() / 4);
    match kind {
        // Polygons and poly-lines, 32-bit (2–8) and 16-bit (0x55–0x5B).
        2 | 0x55 => {
            let pts = if kind == 2 {
                emf_points32(r, 28, count())
            } else {
                emf_points16(r, 28, count())
            };
            if let Some((&first, rest)) = pts.split_first() {
                g.bezier(first, rest, false);
            }
        }
        3 | 0x56 => {
            let pts = if kind == 3 {
                emf_points32(r, 28, count())
            } else {
                emf_points16(r, 28, count())
            };
            g.polygon(&pts);
        }
        4 | 0x57 => {
            let pts = if kind == 4 {
                emf_points32(r, 28, count())
            } else {
                emf_points16(r, 28, count())
            };
            g.polyline(&pts);
        }
        5 | 0x58 => {
            let pts = if kind == 5 {
                emf_points32(r, 28, count())
            } else {
                emf_points16(r, 28, count())
            };
            let start = g.dc.cur;
            g.bezier(start, &pts, true);
        }
        6 | 0x59 => {
            let pts = if kind == 6 {
                emf_points32(r, 28, count())
            } else {
                emf_points16(r, 28, count())
            };
            for (x, y) in pts {
                g.line_to(x, y);
            }
        }
        // PolyPolyline / PolyPolygon: bounds, nPolys, total points, counts.
        7 | 8 | 0x5A | 0x5B => {
            let n_polys = (u(24) as usize).min(r.len() / 4);
            let total = (u(28) as usize).min(r.len() / 4);
            let counts: Vec<usize> = (0..n_polys).map(|k| u(32 + k * 4) as usize).collect();
            let at = 32 + n_polys * 4;
            let pts = if kind <= 8 {
                emf_points32(r, at, total)
            } else {
                emf_points16(r, at, total)
            };
            let mut polys = Vec::new();
            let mut k = 0usize;
            for c in counts {
                let end = (k + c).min(pts.len());
                polys.push(pts[k..end].to_vec());
                k = end;
            }
            if matches!(kind, 8 | 0x5B) {
                g.polypolygon(&polys);
            } else {
                for p in polys {
                    g.polyline(&p);
                }
            }
        }
        9 => g.dc.win_ext = (i(8), i(12)),
        10 => g.dc.win_org = (i(8), i(12)),
        11 => g.dc.vp_ext = (i(8), i(12)),
        12 => g.dc.vp_org = (i(8), i(12)),
        17 => g.dc.map_mode = u(8),
        19 => g.dc.winding = u(8) == 2,
        22 => g.dc.text_align = u(8),
        24 => g.dc.text_color = Rgb::from_colorref(u(8)),
        27 => g.move_to(i(8), i(12)),
        33 => g.save(),
        34 => g.restore(i32_at(r, 8).unwrap_or(-1)),
        // SetWorldTransform / ModifyWorldTransform.
        35 | 36 => {
            let m = Xf {
                a: f(8),
                b: f(12),
                c: f(16),
                d: f(20),
                e: f(24),
                f: f(28),
            };
            if kind == 35 {
                g.dc.world = m;
            } else {
                match u(32) {
                    1 => g.dc.world = Xf::ID,
                    2 => g.dc.world = m.then(&g.dc.world),
                    3 => g.dc.world = g.dc.world.then(&m),
                    4 => g.dc.world = m,
                    _ => {}
                }
            }
        }
        37 => g.select(u(8)),
        // CreatePen: ihPen, style, width (POINT x), colour.
        38 => {
            let p = pen_of(u(12), i(16), u(24));
            g.set_object(u(8), Obj::Pen(p));
        }
        39 => {
            let b = brush(u(12), u(16));
            g.set_object(u(8), Obj::Brush(b));
        }
        40 => g.delete(u(8)),
        // Ellipse / Rectangle / RoundRect: a RECTL at 8.
        42 => g.ellipse(i(8), i(12), i(16), i(20)),
        43 => g.rect(i(8), i(12), i(16), i(20)),
        44 => g.round_rect(i(8), i(12), i(16), i(20), i(24), i(28)),
        // Arc / Chord / Pie: box, start, end.
        45..=47 => g.arc(
            [i(8), i(12), i(16), i(20)],
            (i(24), i(28)),
            (i(32), i(36)),
            (kind - 45) as u8,
        ),
        54 => g.line_to(i(8), i(12)),
        // BeginPath / EndPath / CloseFigure / FillPath / StrokeAndFillPath /
        // StrokePath / AbortPath.
        59 => {
            g.path = Some(String::new());
            g.closed_path = None;
        }
        60 => g.closed_path = g.path.take(),
        61 => {
            if let Some(p) = g.path.as_mut() {
                p.push_str("Z ");
            }
        }
        62..=64 => {
            let d = g
                .closed_path
                .take()
                .or_else(|| g.path.take())
                .unwrap_or_default();
            let (fill, stroke) = match kind {
                62 => (true, false),
                63 => (true, true),
                _ => (false, true),
            };
            g.shape(d, fill, stroke);
        }
        68 => {
            g.path = None;
            g.closed_path = None;
        }
        // BitBlt: bounds, dest x y cx cy, rop, src x y, xform, bk colour,
        // usage, offBmi, cbBmi, offBits, cbBits.
        76 => {
            let dest = [i(24), i(28), i(32), i(36)];
            let (off_bmi, cb_bmi, off_bits, cb_bits) = (u(84), u(88), u(92), u(96));
            if cb_bmi == 0 {
                g.pattern_blt(dest, u(40));
            } else if let (Some(bmi), Some(bits)) =
                (slice(r, off_bmi, cb_bmi), slice(r, off_bits, cb_bits))
            {
                g.bitmap(dest, bmi, bits, None);
            }
        }
        // StretchBlt: as BitBlt, plus src cx cy at 100.
        77 => {
            let dest = [i(24), i(28), i(32), i(36)];
            let (off_bmi, cb_bmi, off_bits, cb_bits) = (u(84), u(88), u(92), u(96));
            if cb_bmi == 0 {
                g.pattern_blt(dest, u(40));
            } else if let (Some(bmi), Some(bits)) =
                (slice(r, off_bmi, cb_bmi), slice(r, off_bits, cb_bits))
            {
                let src = [i(44) as i64, i(48) as i64, i(100) as i64, i(104) as i64];
                g.bitmap(dest, bmi, bits, Some(src));
            }
        }
        // SetDIBitsToDevice: bounds, dest x y, src x y cx cy, offBmi, cbBmi,
        // offBits, cbBits.
        80 => {
            let (cx, cy) = (i(40), i(44));
            let dest = [i(24), i(28), cx, cy];
            if let (Some(bmi), Some(bits)) = (slice(r, u(48), u(52)), slice(r, u(56), u(60))) {
                g.bitmap(dest, bmi, bits, None);
            }
        }
        // StretchDIBits: bounds, dest x y, src x y cx cy, offBmi, cbBmi,
        // offBits, cbBits, usage, rop, dest cx cy.
        81 => {
            let dest = [i(24), i(28), i(72), i(76)];
            if let (Some(bmi), Some(bits)) = (slice(r, u(48), u(52)), slice(r, u(56), u(60))) {
                let src = [i(32) as i64, i(36) as i64, i(40) as i64, i(44) as i64];
                g.bitmap(dest, bmi, bits, Some(src));
            }
        }
        // ExtCreateFontIndirectW: ihFont, LOGFONTW.
        82 => {
            let face: String = utf16(r.get(40..104).unwrap_or(&[]));
            let font = Font {
                height: i(12),
                weight: u(28),
                italic: r.get(32).copied().unwrap_or(0) != 0,
                face,
                escapement: i(20),
            };
            g.set_object(u(8), Obj::Font(font));
        }
        // ExtTextOutA / W: bounds, mode, scales, EMRTEXT (ref x y, nChars,
        // offString, options, rect, offDx).
        83 | 84 => {
            let (x, y) = (i(36), i(40));
            let n = u(44) as usize;
            let off = u(48) as usize;
            let text = if kind == 84 {
                utf16(r.get(off..off + n * 2).unwrap_or(&[]))
            } else {
                r.get(off..off + n)
                    .map(|b| b.iter().map(|&c| c as char).collect())
                    .unwrap_or_default()
            };
            g.text(x, y, &text);
        }
        // ExtCreatePen: ihPen, offBmi, cbBmi, offBits, cbBits, style, width,
        // brush style, colour.
        95 => {
            let p = pen_of(u(24), u(28) as f64, u(36));
            g.set_object(u(8), Obj::Pen(p));
        }
        // Pattern / DIB / mono brushes and palettes take a slot.
        48 | 49 | 93 | 94 => g.set_object(u(8), Obj::Brush(brush(3, 0))),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// WMF

fn wmf_to_svg(data: &[u8], size_hint: Option<(f64, f64)>) -> Option<Svg> {
    // The placeable header: key, hmf, bbox (l t r b, i16), units per inch.
    let (start, placeable) = if u32_at(data, 0) == Some(0x9AC6_CDD7) {
        let bbox = [
            i16_at(data, 6)? as f64,
            i16_at(data, 8)? as f64,
            i16_at(data, 10)? as f64,
            i16_at(data, 12)? as f64,
        ];
        let inch = u16_at(data, 14)? as f64;
        (
            22usize,
            Some((bbox, if inch > 0.0 { inch } else { 1440.0 })),
        )
    } else {
        (0, None)
    };
    let header_words = u16_at(data, start + 2)? as usize;
    let mut off = start + header_words * 2;
    // A writer that declares the 18-byte header (9 words) but omits
    // `mtNoParameters` — the #832 reproducer's Word 6 generator — puts the
    // first record two bytes early. Take it from there when the declared
    // offset holds no record but that one does (LibreOffice reads it too).
    let record_at = |o: usize| {
        u32_at(data, o).is_some_and(|w| w >= 3 && o + w as usize * 2 <= data.len())
            && u16_at(data, o + 4).is_some_and(|f| f != 0)
    };
    if off >= 2 && !record_at(off) && record_at(off - 2) {
        off -= 2;
    }

    let mut g = Gdi::new(true, (96.0 / 25.4, 96.0 / 25.4));
    if let Some((b, _)) = placeable {
        g.dc.win_org = (b[0], b[1]);
        g.dc.win_ext = (b[2] - b[0], b[3] - b[1]);
    }
    // The picture's extent: the first window set (or the placeable box).
    let mut window: Option<(f64, f64)> = None;
    let mut count = 0usize;
    while let (Some(words), Some(func)) = (u32_at(data, off), u16_at(data, off + 4)) {
        let size = words as usize * 2;
        if size < 6 || off + size > data.len() || count > 2_000_000 {
            break;
        }
        count += 1;
        let r = &data[off..off + size];
        if func == 0 {
            break; // META_EOF
        }
        wmf_record(&mut g, func, r, &mut window);
        off += size;
    }
    let (ex, ey) = window.unwrap_or(g.dc.win_ext);
    // In the picture space the window starts at the origin (see `Gdi::xf`).
    let view = [0.0, 0.0, ex.abs(), ey.abs()];
    let size = if let Some((b, inch)) = placeable {
        (
            (b[2] - b[0]).abs() / inch * 96.0,
            (b[3] - b[1]).abs() / inch * 96.0,
        )
    } else if let Some(hint) = size_hint {
        hint
    } else {
        (ex.abs(), ey.abs())
    };
    g.finish(view, size)
}

/// The record's `i`-th 16-bit parameter (after size and function).
fn wp(r: &[u8], i: usize) -> f64 {
    i16_at(r, 6 + i * 2).unwrap_or(0) as f64
}

fn wpu(r: &[u8], i: usize) -> u32 {
    u16_at(r, 6 + i * 2).unwrap_or(0) as u32
}

fn wmf_points(r: &[u8], at: usize, n: usize) -> Vec<(f64, f64)> {
    (0..n)
        .map_while(|k| {
            Some((
                i16_at(r, at + k * 4)? as f64,
                i16_at(r, at + k * 4 + 2)? as f64,
            ))
        })
        .collect()
}

fn wmf_record(g: &mut Gdi, func: u16, r: &[u8], window: &mut Option<(f64, f64)>) {
    let colorref = |i: usize| wpu(r, i) | (wpu(r, i + 1) << 16);
    match func {
        0x0201 => {} // SetBkColor
        0x0103 => g.dc.map_mode = wpu(r, 0),
        0x0106 => g.dc.winding = wpu(r, 0) == 2,
        0x0209 => g.dc.text_color = Rgb::from_colorref(colorref(0)),
        0x012E => g.dc.text_align = wpu(r, 0),
        // SetWindowOrg / SetWindowExt: y then x.
        0x020B => g.dc.win_org = (wp(r, 1), wp(r, 0)),
        0x020C => {
            g.dc.win_ext = (wp(r, 1), wp(r, 0));
            window.get_or_insert(g.dc.win_ext);
        }
        0x0213 => g.line_to(wp(r, 1), wp(r, 0)),
        0x0214 => g.move_to(wp(r, 1), wp(r, 0)),
        // Rectangle / Ellipse: bottom, right, top, left.
        0x041B => g.rect(wp(r, 3), wp(r, 2), wp(r, 1), wp(r, 0)),
        0x0418 => g.ellipse(wp(r, 3), wp(r, 2), wp(r, 1), wp(r, 0)),
        // RoundRect: height, width, bottom, right, top, left.
        0x061C => g.round_rect(wp(r, 5), wp(r, 4), wp(r, 3), wp(r, 2), wp(r, 1), wp(r, 0)),
        // Arc / Pie / Chord: yEnd xEnd yStart xStart bottom right top left.
        0x0817 | 0x081A | 0x0830 => {
            let b = [wp(r, 7), wp(r, 6), wp(r, 5), wp(r, 4)];
            let kind = match func {
                0x0817 => 0,
                0x0830 => 1,
                _ => 2,
            };
            g.arc(b, (wp(r, 3), wp(r, 2)), (wp(r, 1), wp(r, 0)), kind);
        }
        0x0324 | 0x0325 => {
            let n = wpu(r, 0) as usize;
            let pts = wmf_points(r, 8, n);
            if func == 0x0324 {
                g.polygon(&pts);
            } else {
                g.polyline(&pts);
            }
        }
        0x0538 => {
            let n = wpu(r, 0) as usize;
            let counts: Vec<usize> = (0..n).map(|k| wpu(r, 1 + k) as usize).collect();
            let mut at = 8 + n * 2;
            let mut polys = Vec::new();
            for c in counts {
                polys.push(wmf_points(r, at, c));
                at += c * 4;
            }
            g.polypolygon(&polys);
        }
        // TextOut: length, string (padded to words), y, x.
        0x0521 => {
            let n = wpu(r, 0) as usize;
            let s = r.get(8..8 + n).unwrap_or(&[]);
            let after = 8 + n.div_ceil(2) * 2;
            let y = i16_at(r, after).unwrap_or(0) as f64;
            let x = i16_at(r, after + 2).unwrap_or(0) as f64;
            g.text(x, y, &cp1252(s));
        }
        // ExtTextOut: y, x, count, options, [rect], string.
        0x0A32 => {
            let (y, x, n, opts) = (wp(r, 0), wp(r, 1), wpu(r, 2) as usize, wpu(r, 3));
            let mut at = 14;
            if opts & 0x0006 != 0 {
                at += 8; // ETO_OPAQUE / ETO_CLIPPED rectangle
            }
            let s = r.get(at..at + n).unwrap_or(&[]);
            g.text(x, y, &cp1252(s));
        }
        0x001E => g.save(),
        0x0127 => g.restore(wp(r, 0) as i32),
        0x012D => g.select(wpu(r, 0)),
        0x01F0 => g.delete(wpu(r, 0)),
        // CreatePenIndirect: style, width (POINTS x, y), colour.
        0x02FA => {
            let p = pen_of(wpu(r, 0), wp(r, 1), colorref(3));
            g.add_object(Obj::Pen(p));
        }
        // CreateBrushIndirect: style, colour, hatch.
        0x02FC => {
            let b = brush(wpu(r, 0), colorref(1));
            g.add_object(Obj::Brush(b));
        }
        // CreateFontIndirect: height, width, escapement, orientation,
        // weight, italic|underline, strikeout|charset, …, face (32 bytes).
        0x02FB => {
            let face_bytes = r.get(6 + 18..6 + 18 + 32).unwrap_or(&[]);
            let end = face_bytes
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(face_bytes.len());
            let font = Font {
                height: wp(r, 0),
                weight: wpu(r, 4),
                italic: r.get(6 + 10).copied().unwrap_or(0) != 0,
                face: cp1252(&face_bytes[..end]),
                escapement: wp(r, 2),
            };
            g.add_object(Obj::Font(font));
        }
        // Palettes, pattern brushes, regions: a slot each.
        0x00F7 | 0x01F9 | 0x06FF => g.add_object(Obj::Other),
        0x0142 => g.add_object(Obj::Brush(brush(3, 0))),
        // StretchDIB: rop (2 words), usage, src h w y x, dest h w y x, DIB.
        0x0F43 => {
            let dest = [wp(r, 10), wp(r, 9), wp(r, 8), wp(r, 7)];
            let src = [
                wp(r, 6) as i64,
                wp(r, 5) as i64,
                wp(r, 4) as i64,
                wp(r, 3) as i64,
            ];
            if let Some(dib) = r.get(6 + 22..) {
                wmf_dib(g, dest, dib, Some(src));
            }
        }
        // DIBStretchBlt: rop (2 words), src h w y x, dest h w y x, DIB —
        // or, bitmap-less (the record is then exactly `(func >> 8) + 3`
        // words), a reserved word before the destination.
        0x0B41 => {
            if r.len() / 2 != usize::from(func >> 8) + 3 {
                let dest = [wp(r, 9), wp(r, 8), wp(r, 7), wp(r, 6)];
                let src = [
                    wp(r, 5) as i64,
                    wp(r, 4) as i64,
                    wp(r, 3) as i64,
                    wp(r, 2) as i64,
                ];
                if let Some(dib) = r.get(6 + 20..) {
                    wmf_dib(g, dest, dib, Some(src));
                }
            } else {
                g.pattern_blt([wp(r, 10), wp(r, 9), wp(r, 8), wp(r, 7)], colorref(0));
            }
        }
        // DIBBitBlt: rop (2 words), src y x, dest h w y x, DIB — likewise
        // with a reserved word when bitmap-less.
        0x0940 => {
            if r.len() / 2 != usize::from(func >> 8) + 3 {
                let dest = [wp(r, 7), wp(r, 6), wp(r, 5), wp(r, 4)];
                if let Some(dib) = r.get(6 + 16..) {
                    wmf_dib(g, dest, dib, None);
                }
            } else {
                g.pattern_blt([wp(r, 8), wp(r, 7), wp(r, 6), wp(r, 5)], colorref(0));
            }
        }
        // SetDIBitsToDevice: usage, scanCount, startScan, src y x, h w,
        // dest y x, DIB.
        0x0D33 => {
            let dest = [wp(r, 8), wp(r, 7), wp(r, 6), wp(r, 5)];
            if let Some(dib) = r.get(6 + 18..) {
                wmf_dib(g, dest, dib, None);
            }
        }
        _ => {}
    }
}

/// A WMF DIB record's bitmap: header + colour table + bits back to back.
fn wmf_dib(g: &mut Gdi, dest: [f64; 4], dib: &[u8], src: Option<[i64; 4]>) {
    g.bitmap(dest, dib, &[], src);
}

/// A slice of a record by `(offset, length)`, `None` when out of range.
fn slice(r: &[u8], off: u32, len: u32) -> Option<&[u8]> {
    let (off, len) = (off as usize, len as usize);
    r.get(off..off.checked_add(len)?)
}

fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

fn cp1252(b: &[u8]) -> String {
    encoding_rs::WINDOWS_1252.decode(b).0.into_owned()
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn i16_at(d: &[u8], o: usize) -> Option<i16> {
    Some(i16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn i32_at(d: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn f32_at(d: &[u8], o: usize) -> Option<f32> {
    Some(f32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An EMF record: type, size, then the payload padded to 4 bytes.
    fn emr(kind: u32, payload: &[u8]) -> Vec<u8> {
        let mut p = payload.to_vec();
        while !p.len().is_multiple_of(4) {
            p.push(0);
        }
        let mut r = Vec::with_capacity(8 + p.len());
        r.extend_from_slice(&kind.to_le_bytes());
        r.extend_from_slice(&(8 + p.len() as u32).to_le_bytes());
        r.extend_from_slice(&p);
        r
    }

    fn le(vals: &[i32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// A 100 × 50 px EMF (at 96 dpi on a 96-dpi reference device) built
    /// from `body`, with the header and `EMR_EOF` around it.
    fn emf(body: &[Vec<u8>]) -> Vec<u8> {
        // Bounds, frame (0.01 mm), signature, version, bytes, records,
        // handles, reserved, nDescription, offDescription, nPalEntries,
        // device px, device mm.
        let mut h = le(&[0, 0, 99, 49, 0, 0, 2646, 1323]);
        h.extend_from_slice(b" EMF");
        h.extend(le(&[0x10000, 0, body.len() as i32 + 2, 4]));
        h.extend(le(&[0, 0, 0, 0, 1920, 1080, 508, 286]));
        let mut out = emr(1, &h);
        for r in body {
            out.extend_from_slice(r);
        }
        out.extend(emr(14, &le(&[0, 16, 20])));
        let total = out.len() as u32;
        out[48..52].copy_from_slice(&total.to_le_bytes());
        out
    }

    /// A bottom-up 2 × 2 24-bpp `BITMAPINFOHEADER` and its bits, one colour.
    fn dib_2x2(bgr: [u8; 3]) -> (Vec<u8>, Vec<u8>) {
        let mut info = le(&[40, 2, 2]);
        info.extend_from_slice(&1u16.to_le_bytes());
        info.extend_from_slice(&24u16.to_le_bytes());
        info.extend(le(&[0, 16, 2835, 2835, 0, 0]));
        let row = [bgr[0], bgr[1], bgr[2], bgr[0], bgr[1], bgr[2], 0, 0];
        (info, [row, row].concat())
    }

    fn emf_sample() -> Vec<u8> {
        let red = 0x0000_00ff;
        let (info, bits) = dib_2x2([0xff, 0, 0]); // blue
                                                  // EMR_STRETCHDIBITS: bounds, dest x y, src x y cx cy, offBmi,
                                                  // cbBmi, offBits, cbBits, usage, rop, dest cx cy, then the DIB.
        let mut sdib = le(&[0, 0, 0, 0, 60, 10, 0, 0, 2, 2]);
        sdib.extend(le(&[80, info.len() as i32, 80 + info.len() as i32]));
        sdib.extend(le(&[bits.len() as i32, 0, 0x00CC_0020, 30, 30]));
        sdib.extend_from_slice(&info);
        sdib.extend_from_slice(&bits);
        // ExtCreateFontIndirectW: ih, LOGFONTW (height … face).
        let mut font = le(&[2, -12, 0, 0, 0, 400, 0, 0]);
        let mut face = [0u8; 64];
        for (k, c) in "Arial".encode_utf16().enumerate() {
            face[k * 2..k * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }
        font.extend_from_slice(&face);
        // ExtTextOutW: bounds, mode, scales, EMRTEXT, then "Hi".
        let mut text = le(&[0, 0, 0, 0, 1, 0, 0, 5, 45, 2, 76, 0, 0, 0, 0, 0, 0]);
        text.extend(
            "Hi".encode_utf16()
                .flat_map(|c| c.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        emf(&[
            emr(39, &le(&[1, 0, red, 0])),          // CreateBrushIndirect
            emr(37, &le(&[1])),                     // SelectObject brush
            emr(37, &le(&[0x8000_0008u32 as i32])), // NULL_PEN
            emr(43, &le(&[0, 0, 50, 50])),          // Rectangle
            emr(81, &sdib[..]),                     // StretchDIBits
            emr(82, &font),
            emr(37, &le(&[2])),
            emr(84, &text),
        ])
    }

    #[test]
    fn emf_records_become_svg() {
        let data = emf_sample();
        assert!(is_emf(&data) && !is_wmf(&data));
        let svg = to_svg(&data, None).expect("drawn");
        assert_eq!((svg.width.round(), svg.height.round()), (100.0, 50.0));
        assert!(svg.doc.contains("fill=\"#ff0000\""), "{}", svg.doc);
        assert!(svg.doc.contains("<image "), "{}", svg.doc);
        assert!(svg.doc.contains(">Hi</text>"), "{}", svg.doc);
        assert!(svg.doc.contains("Arial"), "{}", svg.doc);
    }

    #[test]
    fn header_only_emf_draws_nothing() {
        assert!(to_svg(&emf(&[]), None).is_none());
        assert!(render(b"\x89PNG\r\n\x1a\n", None).is_none());
        assert!(render(&[], None).is_none());
    }

    /// A WMF record: size in words, function, then the 16-bit parameters.
    fn wmr(func: u16, params: &[i16]) -> Vec<u8> {
        let mut r = (3 + params.len() as u32).to_le_bytes().to_vec();
        r.extend_from_slice(&func.to_le_bytes());
        for p in params {
            r.extend_from_slice(&p.to_le_bytes());
        }
        r
    }

    /// A WMF (placeable when `bbox_inch` is given) around `body`.
    pub(crate) fn wmf(bbox_inch: Option<([i16; 4], u16)>, body: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some((b, inch)) = bbox_inch {
            out.extend_from_slice(&0x9AC6_CDD7u32.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            for v in b {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&inch.to_le_bytes());
            out.extend_from_slice(&[0; 6]); // reserved + checksum
        }
        // META_HEADER: type, header words, version, size, objects, max, 0.
        for w in [1u16, 9, 0x0300, 0, 0, 4, 0, 0, 0] {
            out.extend_from_slice(&w.to_le_bytes());
        }
        for r in body {
            out.extend_from_slice(r);
        }
        out.extend(wmr(0, &[]));
        out
    }

    pub(crate) fn wmf_body() -> Vec<Vec<u8>> {
        vec![
            wmr(0x020B, &[0, 0]),                      // SetWindowOrg
            wmr(0x020C, &[500, 1000]),                 // SetWindowExt (y, x)
            wmr(0x02FC, &[0, 0xff00u16 as i16, 0, 0]), // green brush (COLORREF 0x00ff00)
            wmr(0x012D, &[0]),
            wmr(0x02FA, &[5, 0, 0, 0, 0]), // PS_NULL pen
            wmr(0x012D, &[1]),
            // Polygon: n, then points — a triangle on the left half.
            wmr(0x0324, &[3, 0, 0, 500, 0, 0, 500]),
            // TextOut: length, "Ab", y, x.
            wmr(0x0521, &[2, i16::from_le_bytes(*b"Ab"), 400, 600]),
        ]
    }

    #[test]
    fn placeable_wmf_becomes_svg() {
        let data = wmf(Some(([0, 0, 1000, 500], 1000)), &wmf_body());
        assert!(is_wmf(&data) && !is_emf(&data));
        let svg = to_svg(&data, None).expect("drawn");
        // 1000 × 500 units at 1000 per inch = 96 × 48 px.
        assert_eq!((svg.width.round(), svg.height.round()), (96.0, 48.0));
        assert!(svg.doc.contains(">Ab</text>"), "{}", svg.doc);
    }

    #[test]
    fn bare_wmf_takes_the_size_hint() {
        let data = wmf(None, &wmf_body());
        assert!(is_wmf(&data));
        let svg = to_svg(&data, Some((200.0, 100.0))).expect("drawn");
        assert_eq!((svg.width.round(), svg.height.round()), (200.0, 100.0));
        // Without a hint the window extent is the size.
        let svg = to_svg(&data, None).expect("drawn");
        assert_eq!((svg.width.round(), svg.height.round()), (1000.0, 500.0));
    }

    #[test]
    fn dib_alpha_is_honoured_when_present() {
        // 2 × 1 32-bpp: an opaque red pixel and a transparent one.
        let mut info = le(&[40, 2, 1]);
        info.extend_from_slice(&1u16.to_le_bytes());
        info.extend_from_slice(&32u16.to_le_bytes());
        info.extend(le(&[0, 8, 0, 0, 0, 0]));
        let img = decode_dib(&info, &[0, 0, 0xff, 0xff, 0, 0, 0, 0]).expect("dib");
        let rgba = img.to_rgba8();
        assert_eq!(rgba.get_pixel(0, 0).0, [0xff, 0, 0, 0xff]);
        assert_eq!(rgba.get_pixel(1, 0).0[3], 0);
        // An all-zero alpha byte is GDI's "no alpha": opaque.
        let img = decode_dib(&info, &[0, 0, 0xff, 0, 0, 0xff, 0, 0]).expect("dib");
        assert_eq!(img.to_rgba8().get_pixel(1, 0).0, [0, 0xff, 0, 0xff]);
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn metafiles_rasterize_to_png() {
        let img = render(&emf_sample(), None).expect("png");
        assert_eq!(img.mimetype, "image/png");
        assert_eq!((img.width, img.height), (100, 50));
        let px = image::load_from_memory(&img.data).unwrap().to_rgb8();
        assert_eq!(px.get_pixel(25, 25).0, [0xff, 0, 0], "the red rectangle");
        assert_eq!(px.get_pixel(75, 25).0, [0, 0, 0xff], "the blue bitmap");
        assert_eq!(px.get_pixel(95, 45).0, [0xff; 3], "white background");

        let img = render(&wmf(Some(([0, 0, 1000, 500], 1000)), &wmf_body()), None).expect("png");
        assert_eq!((img.width, img.height), (96, 48));
        let px = image::load_from_memory(&img.data).unwrap().to_rgb8();
        assert_eq!(px.get_pixel(5, 5).0, [0, 0xff, 0], "the green triangle");
        assert_eq!(px.get_pixel(90, 5).0, [0xff; 3]);
    }
}
