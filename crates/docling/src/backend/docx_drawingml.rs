//! DOCX DrawingML shapes to an SVG picture payload (#858), in-process.
//!
//! A flowchart, an architecture sketch or a labelled box in a Word file is a
//! DrawingML drawing without a bitmap: a group (`wpg:wgp`) of shapes
//! (`wps:wsp`) or one lone shape, each a preset geometry (`a:prstGeom`) or
//! a custom path, a fill, an outline with arrowheads, and a text body
//! (`wps:txbx`). docling renders such a drawing through LibreOffice into a
//! PNG; this port has no office suite, and the picture used to come out
//! with no payload at all — an empty `<!-- image -->` in Markdown, an
//! `Image` with an empty target in the Pandoc AST. Here the drawing is
//! translated into an SVG document the way `metafile.rs` (#536) translates
//! an EMF: the geometry subset below, the group transform, theme colours,
//! the labels as `<text>`. The payload is the SVG itself (vector, tiny,
//! byte-deterministic — no host font ever rasterizes it), carried as
//! `image/svg+xml` in docling's `ImageRef`.
//!
//! Covered: `rect`, `roundRect`, `ellipse`, `diamond`, `triangle`,
//! `rtTriangle`, `parallelogram`, `trapezoid`, `hexagon`, `octagon`,
//! `pentagon`, `line`, the straight and bent connectors, `can`,
//! `foldedCorner`, `bevel`, the flowchart presets (`flowChartProcess`,
//! `…Decision`, `…Terminator`, `…Connector`, `…Document`,
//! `…Multidocument`, `…Preparation`, `…Data`, `…Predefined`,
//! `…InputOutput`, `…MagneticDisk`, `…Alternate`), the block arrows
//! (`rightArrow`, `leftArrow`, `upArrow`, `downArrow`, `homePlate`,
//! `chevron`), the rectangular callouts, `custGeom` paths (`moveTo`,
//! `lnTo`, `cubicBezTo`, `quadBezTo`, `close`); solid and theme fills,
//! outlines with width, dash and the `triangle` / `stealth` / `arrow` /
//! `oval` / `diamond` line ends; `rot`, `flipH`, `flipV`; `wpg:grpSp`
//! nesting. A gradient is drawn as its first stop, an unknown preset as its
//! bounding box, both with a diagnostic (`DOCLING_RS_DEBUG`). Text is one
//! `<text>` per paragraph — size, bold, colour and alignment from the runs,
//! no line wrapping — the labels stay as text items too, as docling keeps
//! them (its `_handle_textbox` runs before `_handle_drawingml`).
//!
//! Several anchored drawings in one paragraph become one picture, as
//! upstream renders them (one `_handle_drawingml` per paragraph), placed by
//! their `wp:posOffset`s when every one is positioned against the same
//! frame, else stacked.
//!
//! The shape model this builds ([`Shape`]: identity, text, connection sites
//! and arrow ends) is what the opt-in Mermaid extraction reads.

use std::collections::HashMap;
use std::fmt::Write as _;

use docling_core::PictureImage;
use roxmltree::Node as XmlNode;

use super::docx::attr;

/// 96 px to the inch: 914 400 / 96.
pub(super) const EMU_PER_PX: f64 = 9525.0;
const EMU_PER_INCH: f64 = 914_400.0;
/// Word's default text size in a shape, points.
const DEFAULT_FONT_PT: f64 = 10.0;
const DEFAULT_LINE_HEIGHT: f64 = 1.2;

// ---------------------------------------------------------------------------
// Theme.
// ---------------------------------------------------------------------------

/// `word/theme/theme1.xml`: the colour scheme (`accent1`, `dk1`, …) and the
/// outline widths of the format scheme's line style list, which a shape's
/// `wps:style/a:lnRef idx` indexes.
#[derive(Debug, Clone)]
pub(super) struct Theme {
    colors: HashMap<String, String>,
    /// `a:lnStyleLst` widths in EMU, in order (`lnRef idx="1"` → the first).
    line_widths: Vec<f64>,
    /// The document's default run size in points (`w:docDefaults` in
    /// `styles.xml`; OOXML's 10 pt when unset) — what a label without a
    /// `w:sz` of its own is set in.
    default_size_pt: f64,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            colors: HashMap::new(),
            line_widths: Vec::new(),
            default_size_pt: DEFAULT_FONT_PT,
        }
    }
}

impl Theme {
    pub(super) fn parse(xml: &str) -> Theme {
        let mut theme = Theme::default();
        let Ok(dom) = roxmltree::Document::parse(xml) else {
            return theme;
        };
        if let Some(scheme) = dom.descendants().find(|n| n.has_tag_name("clrScheme")) {
            for slot in scheme.children().filter(|n| n.is_element()) {
                let hex = slot.children().filter(|n| n.is_element()).find_map(|c| {
                    match c.tag_name().name() {
                        "srgbClr" => attr(c, "val").map(str::to_uppercase),
                        "sysClr" => attr(c, "lastClr").map(str::to_uppercase),
                        _ => None,
                    }
                });
                if let Some(hex) = hex {
                    theme.colors.insert(slot.tag_name().name().to_string(), hex);
                }
            }
        }
        if let Some(lines) = dom.descendants().find(|n| n.has_tag_name("lnStyleLst")) {
            theme.line_widths = lines
                .children()
                .filter(|n| n.has_tag_name("ln"))
                .map(|ln| attr(ln, "w").and_then(|w| w.parse().ok()).unwrap_or(9525.0))
                .collect();
        }
        theme
    }

    /// The default run size from `styles.xml`'s `w:docDefaults` (half
    /// points), when it names one.
    pub(super) fn with_styles(mut self, styles_xml: &str) -> Theme {
        if let Ok(dom) = roxmltree::Document::parse(styles_xml) {
            if let Some(sz) = dom
                .descendants()
                .find(|n| n.has_tag_name("docDefaults"))
                .and_then(|d| d.descendants().find(|n| n.has_tag_name("sz")))
                .and_then(|n| attr(n, "val"))
                .and_then(|v| v.parse::<f64>().ok())
            {
                if sz > 0.0 {
                    self.default_size_pt = sz / 2.0;
                }
            }
        }
        self
    }

    /// A scheme colour by its DrawingML name: `tx1`/`bg1`/`tx2`/`bg2` are
    /// the aliases Word writes for `dk1`/`lt1`/`dk2`/`lt2`. `phClr` (the
    /// style matrix placeholder) resolves to nothing here; the caller
    /// substitutes the shape's own reference colour.
    fn scheme(&self, name: &str) -> Option<&str> {
        let key = match name {
            "tx1" => "dk1",
            "bg1" => "lt1",
            "tx2" => "dk2",
            "bg2" => "lt2",
            other => other,
        };
        self.colors.get(key).map(String::as_str)
    }
}

// ---------------------------------------------------------------------------
// The shape model.
// ---------------------------------------------------------------------------

/// A line end decoration (`a:headEnd` / `a:tailEnd` `type`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum LineEnd {
    #[default]
    None,
    /// `triangle`, `stealth`, `arrow`: a directional arrowhead.
    Arrow,
    Oval,
    Diamond,
}

impl LineEnd {
    fn parse(kind: Option<&str>) -> LineEnd {
        match kind {
            Some("triangle" | "stealth" | "arrow") => LineEnd::Arrow,
            Some("oval") => LineEnd::Oval,
            Some("diamond") => LineEnd::Diamond,
            _ => LineEnd::None,
        }
    }

    /// Whether the end implies a direction of flow.
    pub(super) fn is_arrow(self) -> bool {
        self == LineEnd::Arrow
    }
}

/// One shape of a drawing as the Mermaid extraction sees it.
#[derive(Debug, Clone, Default)]
pub(super) struct Shape {
    /// `wps:cNvPr id` — what a connector's `a:stCxn` / `a:endCxn` name.
    pub(super) id: Option<u32>,
    /// `wps:cNvPr name`.
    pub(super) name: Option<String>,
    /// Declared with `wps:cNvCnPr`: a connector, never a node.
    pub(super) connector: bool,
    /// The preset geometry name, or `custGeom`.
    pub(super) geometry: String,
    /// The text body's paragraphs, trimmed, empty ones dropped.
    pub(super) text: Vec<String>,
    /// The connection sites: the shape ids a connector starts and ends at.
    pub(super) start: Option<u32>,
    pub(super) end: Option<u32>,
    /// The decoration at the line's start (`headEnd`) and end (`tailEnd`).
    pub(super) head: LineEnd,
    pub(super) tail: LineEnd,
}

/// A drawing rendered to SVG with the shapes it holds.
pub(super) struct Rendered {
    pub(super) image: PictureImage,
    pub(super) shapes: Vec<Shape>,
    /// What was approximated, one line each — logged under
    /// `DOCLING_RS_DEBUG`, and what the acceptance tests pin.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) diagnostics: Vec<String>,
}

// ---------------------------------------------------------------------------
// Rendering.
// ---------------------------------------------------------------------------

/// Offset + scale from a shape's coordinate space to the canvas (EMU → EMU):
/// `x' = ox + x * sx`. A group composes its children's with its own.
#[derive(Debug, Clone, Copy)]
struct Xform {
    ox: f64,
    oy: f64,
    sx: f64,
    sy: f64,
}

impl Xform {
    fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.ox + x * self.sx, self.oy + y * self.sy)
    }

    /// The transform of a group's children: `a:xfrm` with `off`/`ext` (the
    /// group in the parent's space) and `chOff`/`chExt` (the children's own
    /// space), `(x - chOff) / chExt * ext + off`, then the parent's. A
    /// missing or empty child extent means the children are in the group's
    /// own space already (scale 1).
    fn child(&self, xfrm: Option<XmlNode>) -> Xform {
        let Some(xfrm) = xfrm else {
            return *self;
        };
        let pair = |name: &str, a: &str, b: &str| -> Option<(f64, f64)> {
            let n = xfrm.children().find(|c| c.has_tag_name(name))?;
            Some((emu(n, a)?, emu(n, b)?))
        };
        let (ox, oy) = pair("off", "x", "y").unwrap_or((0.0, 0.0));
        let (ex, ey) = pair("ext", "cx", "cy").unwrap_or((0.0, 0.0));
        let (cox, coy) = pair("chOff", "x", "y").unwrap_or((0.0, 0.0));
        let (cex, cey) = pair("chExt", "cx", "cy").unwrap_or((0.0, 0.0));
        let sx = if cex > 0.0 && ex > 0.0 { ex / cex } else { 1.0 };
        let sy = if cey > 0.0 && ey > 0.0 { ey / cey } else { 1.0 };
        // Child → group space: (x - chOff) * s + off; then this transform.
        Xform {
            ox: self.ox + (ox - cox * sx) * self.sx,
            oy: self.oy + (oy - coy * sy) * self.sy,
            sx: self.sx * sx,
            sy: self.sy * sy,
        }
    }
}

fn emu(node: XmlNode, name: &str) -> Option<f64> {
    attr(node, name)?.trim().parse::<f64>().ok()
}

/// A drawing's frame in the paragraph: its `wp:extent` and, for an anchored
/// one, its `wp:posOffset`s against `relativeFrom`.
struct Frame<'a, 'i> {
    holder: XmlNode<'a, 'i>,
    cx: f64,
    cy: f64,
    pos: Option<(f64, f64)>,
    relative: (String, String),
}

fn frame<'a, 'i>(drawing: XmlNode<'a, 'i>) -> Option<Frame<'a, 'i>> {
    let holder = drawing
        .children()
        .find(|c| c.has_tag_name("inline") || c.has_tag_name("anchor"))?;
    let extent = holder.children().find(|c| c.has_tag_name("extent"))?;
    let (cx, cy) = (emu(extent, "cx")?, emu(extent, "cy")?);
    if cx <= 0.0 || cy <= 0.0 {
        return None;
    }
    let offset = |name: &str| -> Option<(String, f64)> {
        let pos = holder.children().find(|c| c.has_tag_name(name))?;
        let relative = attr(pos, "relativeFrom").unwrap_or("").to_string();
        let off = pos.children().find(|c| c.has_tag_name("posOffset"))?;
        Some((relative, off.text()?.trim().parse::<f64>().ok()?))
    };
    let (h, v) = (offset("positionH"), offset("positionV"));
    let pos = match (&h, &v) {
        (Some((_, x)), Some((_, y))) => Some((*x, *y)),
        _ => None,
    };
    let relative = (
        h.map(|(r, _)| r).unwrap_or_default(),
        v.map(|(r, _)| r).unwrap_or_default(),
    );
    Some(Frame {
        holder,
        cx,
        cy,
        pos,
        relative,
    })
}

/// The SVG under construction.
struct Canvas<'t> {
    theme: &'t Theme,
    body: String,
    shapes: Vec<Shape>,
    diagnostics: Vec<String>,
}

/// Render the paragraph's blip-less drawings into one picture. `None` when
/// none of them has a frame or a shape to draw — the picture then stays the
/// payload-less placeholder it was.
pub(super) fn render(drawings: &[XmlNode], theme: &Theme) -> Option<Rendered> {
    let frames: Vec<Frame> = drawings.iter().filter_map(|d| frame(*d)).collect();
    if frames.is_empty() {
        return None;
    }
    // Where each drawing lands on the canvas: by its offsets when every
    // drawing is anchored against the same frames (Word positions them in
    // the same space then), else stacked top to bottom.
    let positioned = frames.len() == 1
        || (frames.iter().all(|f| f.pos.is_some())
            && frames.iter().all(|f| f.relative == frames[0].relative));
    let mut placed: Vec<(f64, f64)> = Vec::with_capacity(frames.len());
    let mut y_cursor = 0.0;
    for f in &frames {
        if positioned {
            placed.push(f.pos.unwrap_or((0.0, 0.0)));
        } else {
            placed.push((0.0, y_cursor));
            y_cursor += f.cy;
        }
    }
    let min_x = placed.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let min_y = placed.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let width = frames
        .iter()
        .zip(&placed)
        .map(|(f, p)| p.0 - min_x + f.cx)
        .fold(0.0, f64::max);
    let height = frames
        .iter()
        .zip(&placed)
        .map(|(f, p)| p.1 - min_y + f.cy)
        .fold(0.0, f64::max);

    let mut canvas = Canvas {
        theme,
        body: String::new(),
        shapes: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut drawn = 0usize;
    for (f, p) in frames.iter().zip(&placed) {
        let root = Xform {
            ox: p.0 - min_x,
            oy: p.1 - min_y,
            sx: 1.0,
            sy: 1.0,
        };
        let Some(data) = f
            .holder
            .descendants()
            .find(|n| n.has_tag_name("graphicData"))
        else {
            continue;
        };
        for child in data.children().filter(|n| n.is_element()) {
            drawn += canvas.element(child, &root);
        }
    }
    if drawn == 0 {
        return None;
    }
    for d in &canvas.diagnostics {
        docling_core::debug_log!("docx drawingml: {d}");
    }
    let (w_px, h_px) = (width / EMU_PER_PX, height / EMU_PER_PX);
    let mut svg = String::with_capacity(canvas.body.len() + 256);
    let _ = writeln!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\">",
        px(w_px),
        px(h_px),
        px(w_px),
        px(h_px)
    );
    svg.push_str(&canvas.body);
    svg.push_str("</svg>\n");
    Some(Rendered {
        image: PictureImage {
            mimetype: "image/svg+xml".into(),
            width: w_px.round().max(1.0) as u32,
            height: h_px.round().max(1.0) as u32,
            data: svg.into_bytes(),
            dpi: 96,
            display_size: Some([
                (width / EMU_PER_INCH) as f32,
                (height / EMU_PER_INCH) as f32,
            ]),
        },
        shapes: canvas.shapes,
        diagnostics: canvas.diagnostics,
    })
}

impl Canvas<'_> {
    /// Draw a `graphicData` child: a group, a nested group, or a lone
    /// shape. Returns how many shapes were drawn.
    fn element(&mut self, el: XmlNode, xform: &Xform) -> usize {
        match el.tag_name().name() {
            "wgp" | "grpSp" => {
                let xfrm = el
                    .children()
                    .find(|c| c.has_tag_name("grpSpPr"))
                    .and_then(|p| p.children().find(|c| c.has_tag_name("xfrm")));
                let inner = xform.child(xfrm);
                el.children()
                    .filter(|n| n.is_element())
                    .map(|c| self.element(c, &inner))
                    .sum()
            }
            "wsp" => {
                self.shape(el, xform);
                1
            }
            // A picture inside a group has a blip and goes through the
            // picture path; anything else (a chart frame, SmartArt) is
            // not drawn here.
            _ => 0,
        }
    }

    fn shape(&mut self, wsp: XmlNode, xform: &Xform) {
        let props = wsp.children().find(|c| c.has_tag_name("cNvPr"));
        let mut shape = Shape {
            id: props
                .and_then(|p| attr(p, "id"))
                .and_then(|v| v.parse().ok()),
            name: props
                .and_then(|p| attr(p, "name"))
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string),
            connector: wsp.children().any(|c| c.has_tag_name("cNvCnPr")),
            ..Shape::default()
        };
        if let Some(cn) = wsp.children().find(|c| c.has_tag_name("cNvCnPr")) {
            let site = |name: &str| {
                cn.children()
                    .find(|c| c.has_tag_name(name))
                    .and_then(|c| attr(c, "id"))
                    .and_then(|v| v.parse::<u32>().ok())
            };
            shape.start = site("stCxn");
            shape.end = site("endCxn");
        }
        let sp_pr = wsp.children().find(|c| c.has_tag_name("spPr"));
        let style = wsp.children().find(|c| c.has_tag_name("style"));
        let label = shape
            .name
            .clone()
            .or_else(|| shape.id.map(|i| format!("#{i}")))
            .unwrap_or_else(|| "shape".into());

        // --- geometry and placement ---
        let xfrm = sp_pr.and_then(|p| p.children().find(|c| c.has_tag_name("xfrm")));
        let pair = |name: &str, a: &str, b: &str| -> Option<(f64, f64)> {
            let n = xfrm?.children().find(|c| c.has_tag_name(name))?;
            Some((emu(n, a)?, emu(n, b)?))
        };
        let (off_x, off_y) = pair("off", "x", "y").unwrap_or((0.0, 0.0));
        let (ext_x, ext_y) = pair("ext", "cx", "cy").unwrap_or((0.0, 0.0));
        let rot = xfrm
            .and_then(|x| emu(x, "rot"))
            .map(|r| r / 60_000.0)
            .unwrap_or(0.0);
        let flip_h = xfrm.and_then(|x| attr(x, "flipH")) == Some("1");
        let flip_v = xfrm.and_then(|x| attr(x, "flipV")) == Some("1");
        let (x0, y0) = xform.apply(off_x, off_y);
        let (x1, y1) = xform.apply(off_x + ext_x, off_y + ext_y);
        let (x, y, w, h) = (
            x0.min(x1) / EMU_PER_PX,
            y0.min(y1) / EMU_PER_PX,
            (x1 - x0).abs() / EMU_PER_PX,
            (y1 - y0).abs() / EMU_PER_PX,
        );

        let prst = sp_pr
            .and_then(|p| p.children().find(|c| c.has_tag_name("prstGeom")))
            .and_then(|g| attr(g, "prst"))
            .map(str::to_string);
        let cust = sp_pr.and_then(|p| p.children().find(|c| c.has_tag_name("custGeom")));
        shape.geometry = prst
            .clone()
            .or_else(|| cust.map(|_| "custGeom".to_string()))
            .unwrap_or_else(|| "rect".into());
        let adjust: HashMap<String, f64> = sp_pr
            .and_then(|p| p.children().find(|c| c.has_tag_name("prstGeom")))
            .and_then(|g| g.children().find(|c| c.has_tag_name("avLst")))
            .map(|av| {
                av.children()
                    .filter(|c| c.has_tag_name("gd"))
                    .filter_map(|gd| {
                        let name = attr(gd, "name")?;
                        let val = attr(gd, "fmla")?
                            .strip_prefix("val ")?
                            .trim()
                            .parse()
                            .ok()?;
                        Some((name.to_string(), val))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let geom = match cust {
            Some(c) => custom_geometry(c, w, h),
            None => preset_geometry(prst.as_deref().unwrap_or("rect"), w, h, &adjust),
        };
        if !geom.supported {
            self.diagnostics.push(format!(
                "{label}: geometry `{}` is not supported, drawn as its bounding box",
                shape.geometry
            ));
        }

        // --- fill and outline ---
        let style_ref = |name: &str| -> Option<(u32, Option<String>)> {
            let r = style?.children().find(|c| c.has_tag_name(name))?;
            let idx = attr(r, "idx").and_then(|v| v.parse().ok()).unwrap_or(0);
            Some((idx, self.color_of(r, None)))
        };
        let fill_ref = style_ref("fillRef");
        let line_ref = style_ref("lnRef");
        let font_ref = style_ref("fontRef");
        let fill = match sp_pr.and_then(|p| {
            p.children().find(|c| {
                matches!(
                    c.tag_name().name(),
                    "solidFill" | "gradFill" | "noFill" | "pattFill" | "blipFill"
                )
            })
        }) {
            Some(f) if f.has_tag_name("noFill") => None,
            Some(f) if f.has_tag_name("solidFill") => self.color_of(f, fill_ref_color(&fill_ref)),
            Some(f) if f.has_tag_name("gradFill") => {
                self.diagnostics
                    .push(format!("{label}: gradient fill drawn as its first stop"));
                f.descendants()
                    .find(|n| n.has_tag_name("gs"))
                    .and_then(|gs| self.color_of(gs, fill_ref_color(&fill_ref)))
            }
            Some(f) => {
                self.diagnostics.push(format!(
                    "{label}: `{}` fill is not supported, drawn without a fill",
                    f.tag_name().name()
                ));
                None
            }
            // No fill of its own: the style matrix's, when it names one
            // (idx 0 = no fill). A connector or a line has no fill.
            None => match &fill_ref {
                Some((idx, color)) if *idx > 0 && !geom.is_line => color.clone(),
                _ => None,
            },
        };
        let ln = sp_pr.and_then(|p| p.children().find(|c| c.has_tag_name("ln")));
        let mut stroke: Option<String> = None;
        let mut stroke_w = 0.0;
        let mut dash: Option<&str> = None;
        match ln {
            Some(ln) => {
                let no_fill = ln.children().any(|c| c.has_tag_name("noFill"));
                if !no_fill {
                    stroke = ln
                        .children()
                        .find(|c| c.has_tag_name("solidFill"))
                        .and_then(|f| self.color_of(f, line_ref_color(&line_ref)))
                        .or_else(|| line_ref_color(&line_ref));
                    stroke_w = emu(ln, "w")
                        .or_else(|| self.theme_line_width(&line_ref))
                        .unwrap_or(9525.0)
                        / EMU_PER_PX;
                }
                dash = ln
                    .children()
                    .find(|c| c.has_tag_name("prstDash"))
                    .and_then(|d| attr(d, "val"));
                let end = |name: &str| {
                    LineEnd::parse(
                        ln.children()
                            .find(|c| c.has_tag_name(name))
                            .and_then(|e| attr(e, "type")),
                    )
                };
                shape.head = end("headEnd");
                shape.tail = end("tailEnd");
            }
            None => {
                if let Some((idx, _)) = &line_ref {
                    if *idx > 0 {
                        stroke = line_ref_color(&line_ref);
                        stroke_w = self.theme_line_width(&line_ref).unwrap_or(9525.0) / EMU_PER_PX;
                    }
                }
            }
        }
        if geom.is_line && stroke.is_none() {
            stroke = Some(line_ref_color(&line_ref).unwrap_or_else(|| "000000".into()));
            if stroke_w == 0.0 {
                stroke_w = 0.75;
            }
        }
        if shape.head != LineEnd::None || shape.tail != LineEnd::None {
            // Ends on a shape that is not a line are decoration Word does
            // not draw.
            if !geom.is_line {
                shape.head = LineEnd::None;
                shape.tail = LineEnd::None;
            }
        }

        // --- text ---
        let text = text_body(wsp, self.theme.default_size_pt);
        shape.text = text.iter().map(|p| p.lines.join(" ")).collect();
        let body_pr = wsp.children().find(|c| c.has_tag_name("bodyPr"));

        // --- emit ---
        let cx = x + w / 2.0;
        let cy = y + h / 2.0;
        let mut transform = String::new();
        if rot != 0.0 || flip_h || flip_v {
            let _ = write!(transform, "translate({} {})", px(cx), px(cy));
            if rot != 0.0 {
                let _ = write!(transform, " rotate({})", px(rot));
            }
            if flip_h || flip_v {
                let _ = write!(
                    transform,
                    " scale({} {})",
                    if flip_h { -1 } else { 1 },
                    if flip_v { -1 } else { 1 }
                );
            }
            let _ = write!(transform, " translate({} {})", px(-cx), px(-cy));
        }
        let _ = write!(self.body, "  <g");
        if !transform.is_empty() {
            let _ = write!(self.body, " transform=\"{transform}\"");
        }
        if let Some(name) = &shape.name {
            let _ = write!(self.body, " data-name=\"{}\"", escape(name));
        }
        self.body.push_str(">\n");
        // The geometry, in canvas coordinates.
        let _ = write!(self.body, "    <path d=\"{}\"", geom.path.translated(x, y));
        match &fill {
            Some(c) => {
                let _ = write!(self.body, " fill=\"#{c}\"");
            }
            None => self.body.push_str(" fill=\"none\""),
        }
        match &stroke {
            Some(c) if stroke_w > 0.0 => {
                let _ = write!(
                    self.body,
                    " stroke=\"#{c}\" stroke-width=\"{}\"",
                    px(stroke_w)
                );
                if let Some(d) = dash.filter(|d| *d != "solid") {
                    let _ = write!(
                        self.body,
                        " stroke-dasharray=\"{}\"",
                        dash_array(d, stroke_w)
                    );
                }
                if geom.is_line {
                    self.body
                        .push_str(" stroke-linecap=\"round\" stroke-linejoin=\"round\"");
                }
            }
            _ => self.body.push_str(" stroke=\"none\""),
        }
        self.body.push_str("/>\n");
        // The line ends: `headEnd` decorates the start of the path (its
        // first point), `tailEnd` the end — the arrow of a flow `A → B`
        // is a `tailEnd` on a connector from A to B (#858 §6.2).
        if let (Some((p0, p1)), Some(c)) = (geom.ends, &stroke) {
            let size = (stroke_w * 4.0).max(6.0);
            let (p0, p1) = ((p0.0 + x, p0.1 + y), (p1.0 + x, p1.1 + y));
            if shape.head != LineEnd::None {
                self.line_end(shape.head, p1, p0, size, c);
            }
            if shape.tail != LineEnd::None {
                self.line_end(shape.tail, p0, p1, size, c);
            }
        }
        // The text body: one line per paragraph, anchored per `bodyPr`,
        // never mirrored (the group's flip applies to the geometry alone,
        // so the text is drawn in a counter-flipped group).
        if !text.is_empty() {
            let font_color = font_ref
                .as_ref()
                .and_then(|(_, c)| c.clone())
                .unwrap_or_else(|| "000000".into());
            self.text(
                &text,
                body_pr,
                x,
                y,
                w,
                h,
                &font_color,
                flip_h,
                flip_v,
                cx,
                cy,
            );
        }
        self.body.push_str("  </g>\n");
        self.shapes.push(shape);
    }

    fn theme_line_width(&self, line_ref: &Option<(u32, Option<String>)>) -> Option<f64> {
        let (idx, _) = line_ref.as_ref()?;
        if *idx == 0 {
            return None;
        }
        self.theme.line_widths.get(*idx as usize - 1).copied()
    }

    /// The colour a `solidFill` / `gs` / `*Ref` element declares:
    /// `srgbClr`, a `schemeClr` through the theme (a `phClr` placeholder
    /// takes `placeholder`), a `sysClr`'s last colour or a preset name —
    /// with the `lumMod` / `lumOff` / `shade` / `tint` modifiers Word
    /// writes for its theme tints.
    fn color_of(&self, holder: XmlNode, placeholder: Option<String>) -> Option<String> {
        let c = holder.children().find(|c| {
            matches!(
                c.tag_name().name(),
                "srgbClr" | "schemeClr" | "sysClr" | "prstClr" | "scrgbClr"
            )
        })?;
        let base = match c.tag_name().name() {
            "srgbClr" => attr(c, "val").map(str::to_uppercase),
            "sysClr" => attr(c, "lastClr").map(str::to_uppercase),
            "prstClr" => attr(c, "val").and_then(preset_color).map(str::to_string),
            "schemeClr" => match attr(c, "val") {
                Some("phClr") => placeholder,
                Some(name) => self.theme.scheme(name).map(str::to_string),
                None => None,
            },
            _ => None,
        }?;
        let mut rgb = parse_hex(&base)?;
        for m in c.children().filter(|n| n.is_element()) {
            let val = attr(m, "val")
                .and_then(|v| v.parse::<f64>().ok())
                .map(|v| v / 100_000.0);
            let Some(v) = val else {
                continue;
            };
            match m.tag_name().name() {
                "lumMod" => rgb = with_luminance(rgb, |l| l * v),
                "lumOff" => rgb = with_luminance(rgb, |l| l + v),
                "shade" => rgb = rgb.map(|ch| ch * v),
                "tint" => rgb = rgb.map(|ch| ch * v + (1.0 - v)),
                _ => {}
            }
        }
        Some(format_hex(rgb))
    }

    /// An arrowhead (or oval / diamond end) at `tip`, pointing away from
    /// `from`, in the stroke colour.
    fn line_end(
        &mut self,
        kind: LineEnd,
        from: (f64, f64),
        tip: (f64, f64),
        size: f64,
        color: &str,
    ) {
        let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-6 {
            return;
        }
        let (ux, uy) = (dx / len, dy / len);
        let (nx, ny) = (-uy, ux);
        match kind {
            LineEnd::Arrow => {
                let base = (tip.0 - ux * size, tip.1 - uy * size);
                let half = size * 0.45;
                let _ = writeln!(
                    self.body,
                    "    <path d=\"M{} {} L{} {} L{} {} Z\" fill=\"#{color}\" stroke=\"none\"/>",
                    px(tip.0),
                    px(tip.1),
                    px(base.0 + nx * half),
                    px(base.1 + ny * half),
                    px(base.0 - nx * half),
                    px(base.1 - ny * half)
                );
            }
            LineEnd::Oval => {
                let r = size * 0.35;
                let _ = writeln!(
                    self.body,
                    "    <circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"#{color}\" stroke=\"none\"/>",
                    px(tip.0 - ux * r),
                    px(tip.1 - uy * r),
                    px(r)
                );
            }
            LineEnd::Diamond => {
                let half = size * 0.5;
                let mid = (tip.0 - ux * half, tip.1 - uy * half);
                let back = (tip.0 - ux * size, tip.1 - uy * size);
                let _ = writeln!(
                    self.body,
                    "    <path d=\"M{} {} L{} {} L{} {} L{} {} Z\" fill=\"#{color}\" stroke=\"none\"/>",
                    px(tip.0),
                    px(tip.1),
                    px(mid.0 + nx * half * 0.6),
                    px(mid.1 + ny * half * 0.6),
                    px(back.0),
                    px(back.1),
                    px(mid.0 - nx * half * 0.6),
                    px(mid.1 - ny * half * 0.6)
                );
            }
            LineEnd::None => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        paras: &[TextPara],
        body_pr: Option<XmlNode>,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        default_color: &str,
        flip_h: bool,
        flip_v: bool,
        cx: f64,
        cy: f64,
    ) {
        let inset = |name: &str, default: f64| {
            body_pr.and_then(|b| emu(b, name)).unwrap_or(default) / EMU_PER_PX
        };
        let (l, t, r, b) = (
            inset("lIns", 91_440.0),
            inset("tIns", 45_720.0),
            inset("rIns", 91_440.0),
            inset("bIns", 45_720.0),
        );
        let anchor = body_pr.and_then(|b| attr(b, "anchor")).unwrap_or("t");
        // Word wraps a shape's text at its inset box (`wrap="square"`, the
        // default); `wrap="none"` lets it run.
        let wrap = body_pr.and_then(|b| attr(b, "wrap")) != Some("none");
        let inner_w = (w - l - r).max(0.0);
        // Lay the lines out first: the block's height places it.
        let mut lines: Vec<(String, f64, &TextPara)> = Vec::new();
        for p in paras {
            let size_px = p.size_pt * 96.0 / 72.0;
            for seg in &p.lines {
                for line in wrap_line(seg, size_px, p.bold, if wrap { inner_w } else { 0.0 }) {
                    lines.push((line, size_px, p));
                }
            }
        }
        let total: f64 = lines.iter().map(|(_, s, _)| s * DEFAULT_LINE_HEIGHT).sum();
        let inner_h = (h - t - b).max(0.0);
        let mut line_y = y
            + t
            + match anchor {
                "ctr" => ((inner_h - total) / 2.0).max(0.0),
                "b" => (inner_h - total).max(0.0),
                _ => 0.0,
            };
        if flip_h || flip_v {
            // Undo the group's flip around the shape's centre for the text.
            let _ = writeln!(
                self.body,
                "    <g transform=\"translate({} {}) scale({} {}) translate({} {})\">",
                px(cx),
                px(cy),
                if flip_h { -1 } else { 1 },
                if flip_v { -1 } else { 1 },
                px(-cx),
                px(-cy)
            );
        }
        for (line, size_px, p) in lines {
            let line_h = size_px * DEFAULT_LINE_HEIGHT;
            // The baseline sits a little under the line's middle.
            let baseline = line_y + line_h / 2.0 + size_px * 0.35;
            let (tx, anchor_attr) = match p.align.as_str() {
                "center" => ((x + w / 2.0), "middle"),
                "right" | "end" => ((x + w - r), "end"),
                _ => ((x + l), "start"),
            };
            let _ = write!(
                self.body,
                "    <text x=\"{}\" y=\"{}\" font-family=\"Calibri, Arial, Helvetica, sans-serif\" font-size=\"{}\" text-anchor=\"{anchor_attr}\" fill=\"#{}\"",
                px(tx),
                px(baseline),
                px(size_px),
                p.color.as_deref().unwrap_or(default_color)
            );
            if p.bold {
                self.body.push_str(" font-weight=\"bold\"");
            }
            if p.italic {
                self.body.push_str(" font-style=\"italic\"");
            }
            let _ = writeln!(self.body, ">{}</text>", escape(&line));
            line_y += line_h;
        }
        if flip_h || flip_v {
            self.body.push_str("    </g>\n");
        }
    }
}

fn fill_ref_color(fill_ref: &Option<(u32, Option<String>)>) -> Option<String> {
    fill_ref.as_ref().and_then(|(_, c)| c.clone())
}

fn line_ref_color(line_ref: &Option<(u32, Option<String>)>) -> Option<String> {
    line_ref.as_ref().and_then(|(_, c)| c.clone())
}

// ---------------------------------------------------------------------------
// Mermaid (#858 phase 2).
// ---------------------------------------------------------------------------

/// The flowchart of a drawing's shapes as Mermaid, when its connectivity is
/// unambiguous — `extract_diagrams=mermaid`, the default. The nodes are the
/// labelled shapes, the edges come **only** from connectors whose
/// `a:stCxn` / `a:endCxn` name two of them; a `tailEnd` arrow is `A --> B`,
/// a `headEnd` arrow `B --> A`, both `A <--> B`, neither `A --- B` (an
/// `oval` or `diamond` end implies no direction). Any connector or line
/// without both sites — a line laid by eye — makes the graph ambiguous and
/// the result `Err` with the reason: nothing is invented from positions.
pub(super) fn mermaid(shapes: &[Shape]) -> Result<String, String> {
    let is_line = |s: &Shape| {
        s.connector
            || matches!(s.geometry.as_str(), "line" | "straightConnector1")
            || s.geometry.starts_with("bentConnector")
            || s.geometry.starts_with("curvedConnector")
    };
    let nodes: Vec<&Shape> = shapes
        .iter()
        .filter(|s| !is_line(s) && !s.text.is_empty())
        .collect();
    let lines: Vec<&Shape> = shapes.iter().filter(|s| is_line(s)).collect();
    if lines.is_empty() {
        return Err("no connectors".into());
    }
    if nodes.is_empty() {
        return Err("no labelled shapes".into());
    }
    // Node identifiers: the shape's name when it is one, else `S<id>`,
    // made unique.
    let mut ids: Vec<String> = Vec::with_capacity(nodes.len());
    for n in &nodes {
        let base: String = n
            .name
            .as_deref()
            .unwrap_or("")
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let mut id = if base.is_empty() || !base.starts_with(|c: char| c.is_ascii_alphabetic()) {
            format!(
                "S{}",
                n.id.map_or_else(|| ids.len().to_string(), |i| i.to_string())
            )
        } else {
            base
        };
        let mut k = 2;
        while ids.contains(&id) {
            id = format!("{id}_{k}");
            k += 1;
        }
        ids.push(id);
    }
    let node_at = |shape_id: Option<u32>| -> Option<usize> {
        let id = shape_id?;
        nodes.iter().position(|n| n.id == Some(id))
    };
    let label = |i: usize| {
        format!(
            "{}[\"{}\"]",
            ids[i],
            mermaid_label(&nodes[i].text.join(" "))
        )
    };
    let mut out = String::from("flowchart TD\n");
    let mut linked = vec![false; nodes.len()];
    for line in &lines {
        let name = line
            .name
            .clone()
            .or_else(|| line.id.map(|i| format!("#{i}")))
            .unwrap_or_else(|| "a line".into());
        let (Some(a), Some(b)) = (node_at(line.start), node_at(line.end)) else {
            return Err(format!(
                "connector `{name}` has no connection sites to labelled shapes"
            ));
        };
        let arrow = match (line.head.is_arrow(), line.tail.is_arrow()) {
            (false, true) => "-->",
            (true, false) => "<--",
            (true, true) => "<-->",
            (false, false) => "---",
        };
        let (a, b) = if arrow == "<--" { (b, a) } else { (a, b) };
        let arrow = if arrow == "<--" { "-->" } else { arrow };
        let _ = writeln!(out, "  {} {arrow} {}", label(a), label(b));
        linked[a] = true;
        linked[b] = true;
    }
    for (i, l) in linked.iter().enumerate() {
        if !l {
            let _ = writeln!(out, "  {}", label(i));
        }
    }
    // A code item's text carries no trailing newline: the fence closes
    // right after the last line.
    out.truncate(out.trim_end().len());
    Ok(out)
}

/// The code item `extract_diagrams` adds after a drawing's picture:
/// `(language, text)`, or `None` when the extraction is off or the graph
/// is ambiguous (a `DOCLING_RS_DEBUG` diagnostic says why).
pub(super) fn diagram(
    rendered: &Rendered,
    format: crate::DiagramFormat,
) -> Option<(String, String)> {
    match format {
        crate::DiagramFormat::None => None,
        crate::DiagramFormat::Mermaid => match mermaid(&rendered.shapes) {
            Ok(text) => Some(("mermaid".to_string(), text)),
            Err(reason) => {
                docling_core::debug_log!(
                    "docx drawingml: no mermaid graph extracted — {reason}; the picture stays"
                );
                None
            }
        },
    }
}

/// A node label inside `["…"]`: Mermaid's entity codes for the quote and
/// the hash (`#quot;`, `#35;`), a line break as a space.
fn mermaid_label(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' => out.push_str("#quot;"),
            '#' => out.push_str("#35;"),
            '\n' | '\r' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Text bodies.
// ---------------------------------------------------------------------------

struct TextPara {
    /// The paragraph's text, one entry per hard break (`w:br`).
    lines: Vec<String>,
    size_pt: f64,
    bold: bool,
    italic: bool,
    color: Option<String>,
    align: String,
}

/// The `wps:txbx/w:txbxContent` paragraphs of a shape: each one's text
/// (runs joined, tabs as spaces, a `w:br` starting a new line, a list
/// paragraph led by a bullet), the first run's size, weight, slant and
/// colour, the paragraph's justification. Empty paragraphs are dropped
/// (they only take vertical space; a label is what matters).
fn text_body(wsp: XmlNode, default_size_pt: f64) -> Vec<TextPara> {
    let Some(txbx) = wsp.children().find(|c| c.has_tag_name("txbx")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for p in txbx
        .descendants()
        .filter(|n| n.has_tag_name("p") && n.ancestors().any(|a| a.has_tag_name("txbxContent")))
    {
        let mut lines: Vec<String> = vec![String::new()];
        let mut first_rpr: Option<XmlNode> = None;
        for n in p.descendants() {
            let in_run = n.parent().is_some_and(|r| r.has_tag_name("r"));
            match n.tag_name().name() {
                "t" if in_run => {
                    if first_rpr.is_none() {
                        first_rpr = n
                            .parent()
                            .and_then(|r| r.children().find(|c| c.has_tag_name("rPr")));
                    }
                    lines.last_mut().unwrap().push_str(n.text().unwrap_or(""));
                }
                "tab" if in_run => lines.last_mut().unwrap().push(' '),
                "br" | "cr" if in_run => lines.push(String::new()),
                _ => {}
            }
        }
        let ppr = p.children().find(|c| c.has_tag_name("pPr"));
        let mut lines: Vec<String> = lines
            .iter()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            continue;
        }
        if ppr.is_some_and(|pp| pp.children().any(|c| c.has_tag_name("numPr"))) {
            lines[0] = format!("\u{2022} {}", lines[0]);
        }
        let rpr =
            first_rpr.or_else(|| ppr.and_then(|pp| pp.children().find(|c| c.has_tag_name("rPr"))));
        let has = |name: &str| {
            rpr.is_some_and(|r| {
                r.children().any(|c| {
                    c.has_tag_name(name) && attr(c, "val").is_none_or(|v| v != "0" && v != "false")
                })
            })
        };
        let size_pt = rpr
            .and_then(|r| r.children().find(|c| c.has_tag_name("sz")))
            .and_then(|s| attr(s, "val"))
            .and_then(|v| v.parse::<f64>().ok())
            .map(|half| half / 2.0)
            .unwrap_or(default_size_pt);
        let color = rpr
            .and_then(|r| r.children().find(|c| c.has_tag_name("color")))
            .and_then(|c| attr(c, "val"))
            .filter(|v| *v != "auto" && v.len() == 6)
            .map(str::to_uppercase);
        let align = ppr
            .and_then(|pp| pp.children().find(|c| c.has_tag_name("jc")))
            .and_then(|j| attr(j, "val"))
            .unwrap_or("left")
            .to_string();
        out.push(TextPara {
            lines,
            size_pt,
            bold: has("b"),
            italic: has("i"),
            color,
            align,
        });
    }
    out
}

/// An estimate of a line's width in a proportional sans font at `size_px`
/// — per-character widths of the common classes, without a font on hand.
/// Good enough to wrap where Word wraps, give or take a word.
fn text_width(text: &str, size_px: f64, bold: bool) -> f64 {
    let em: f64 = text
        .chars()
        .map(|c| match c {
            ' ' => 0.28,
            'i' | 'j' | 'l' | 't' | 'f' | 'r' | 'I' | '.' | ',' | ';' | ':' | '\'' | '!' | '|'
            | '(' | ')' | '[' | ']' => 0.3,
            'm' | 'w' | 'M' | 'W' => 0.85,
            c if c.is_ascii_uppercase() => 0.66,
            c if c.is_ascii_digit() => 0.55,
            c if (c as u32) >= 0x2E80 => 1.0,
            _ => 0.52,
        })
        .sum();
    em * size_px * if bold { 1.06 } else { 1.0 }
}

/// `text` wrapped greedily at `max_w` pixels (none when `max_w` is 0): a
/// word that does not fit alone still stays whole.
fn wrap_line(text: &str, size_px: f64, bold: bool, max_w: f64) -> Vec<String> {
    if max_w <= 0.0 || text_width(text, size_px, bold) <= max_w {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let candidate = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if line.is_empty() || text_width(&candidate, size_px, bold) <= max_w {
            line = candidate;
        } else {
            lines.push(std::mem::replace(&mut line, word.to_string()));
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

// ---------------------------------------------------------------------------
// Geometry.
// ---------------------------------------------------------------------------

/// An SVG path in shape-local pixels (the shape's box is `0 0 w h`).
struct Path(Vec<Seg>);

enum Seg {
    M(f64, f64),
    L(f64, f64),
    C(f64, f64, f64, f64, f64, f64),
    Q(f64, f64, f64, f64),
    /// An elliptical arc to a point: `rx ry x-rotation large-arc sweep x y`.
    A(f64, f64, bool, bool, f64, f64),
    Z,
}

impl Path {
    fn translated(&self, dx: f64, dy: f64) -> String {
        let mut s = String::new();
        for seg in &self.0 {
            match *seg {
                Seg::M(x, y) => {
                    let _ = write!(s, "M{} {}", px(x + dx), px(y + dy));
                }
                Seg::L(x, y) => {
                    let _ = write!(s, " L{} {}", px(x + dx), px(y + dy));
                }
                Seg::C(a, b, c, d, x, y) => {
                    let _ = write!(
                        s,
                        " C{} {} {} {} {} {}",
                        px(a + dx),
                        px(b + dy),
                        px(c + dx),
                        px(d + dy),
                        px(x + dx),
                        px(y + dy)
                    );
                }
                Seg::Q(a, b, x, y) => {
                    let _ = write!(
                        s,
                        " Q{} {} {} {}",
                        px(a + dx),
                        px(b + dy),
                        px(x + dx),
                        px(y + dy)
                    );
                }
                Seg::A(rx, ry, large, sweep, x, y) => {
                    let _ = write!(
                        s,
                        " A{} {} 0 {} {} {} {}",
                        px(rx),
                        px(ry),
                        u8::from(large),
                        u8::from(sweep),
                        px(x + dx),
                        px(y + dy)
                    );
                }
                Seg::Z => s.push_str(" Z"),
            }
        }
        s
    }
}

struct Geom {
    path: Path,
    /// A preset this module draws faithfully.
    supported: bool,
    /// A line-like geometry: no fill, arrowheads at `ends`.
    is_line: bool,
    /// The start and end point of a line, shape-local.
    ends: Option<((f64, f64), (f64, f64))>,
}

fn polygon(points: &[(f64, f64)]) -> Path {
    let mut segs = Vec::with_capacity(points.len() + 1);
    for (i, &(x, y)) in points.iter().enumerate() {
        segs.push(if i == 0 { Seg::M(x, y) } else { Seg::L(x, y) });
    }
    segs.push(Seg::Z);
    Path(segs)
}

fn ellipse(x: f64, y: f64, w: f64, h: f64) -> Vec<Seg> {
    let (rx, ry) = (w / 2.0, h / 2.0);
    vec![
        Seg::M(x, y + ry),
        Seg::A(rx, ry, false, true, x + w, y + ry),
        Seg::A(rx, ry, false, true, x, y + ry),
        Seg::Z,
    ]
}

fn round_rect(w: f64, h: f64, r: f64) -> Path {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    if r <= 0.0 {
        return polygon(&[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]);
    }
    Path(vec![
        Seg::M(r, 0.0),
        Seg::L(w - r, 0.0),
        Seg::A(r, r, false, true, w, r),
        Seg::L(w, h - r),
        Seg::A(r, r, false, true, w - r, h),
        Seg::L(r, h),
        Seg::A(r, r, false, true, 0.0, h - r),
        Seg::L(0.0, r),
        Seg::A(r, r, false, true, r, 0.0),
        Seg::Z,
    ])
}

/// The shape of a preset geometry, in its `w × h` box. `adj` holds the
/// `avLst` guides (`adj`, `adj1`, …, in 100 000ths), the presets' own
/// defaults applying otherwise.
fn preset_geometry(prst: &str, w: f64, h: f64, adj: &HashMap<String, f64>) -> Geom {
    let a = |name: &str, default: f64| adj.get(name).copied().unwrap_or(default) / 100_000.0;
    let shortest = w.min(h);
    let closed = |path: Path| Geom {
        path,
        supported: true,
        is_line: false,
        ends: None,
    };
    match prst {
        "rect" | "flowChartProcess" | "wedgeRectCallout" | "borderCallout1" | "borderCallout2"
        | "borderCallout3" | "accentCallout1" | "textNoShape" => {
            closed(polygon(&[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]))
        }
        "roundRect"
        | "round1Rect"
        | "round2SameRect"
        | "round2DiagRect"
        | "snipRoundRect"
        | "wedgeRoundRectCallout"
        | "flowChartAlternateProcess" => closed(round_rect(w, h, shortest * a("adj", 16_667.0))),
        "flowChartTerminator" => closed(round_rect(w, h, h / 2.0)),
        "ellipse"
        | "flowChartConnector"
        | "flowChartSummingJunction"
        | "flowChartOr"
        | "wedgeEllipseCallout" => closed(Path(ellipse(0.0, 0.0, w, h))),
        "diamond" | "flowChartDecision" => closed(polygon(&[
            (w / 2.0, 0.0),
            (w, h / 2.0),
            (w / 2.0, h),
            (0.0, h / 2.0),
        ])),
        "triangle" | "flowChartExtract" => {
            let apex = w * a("adj", 50_000.0);
            closed(polygon(&[(apex, 0.0), (w, h), (0.0, h)]))
        }
        "flowChartMerge" => closed(polygon(&[(0.0, 0.0), (w, 0.0), (w / 2.0, h)])),
        "rtTriangle" => closed(polygon(&[(0.0, 0.0), (w, h), (0.0, h)])),
        "parallelogram" | "flowChartInputOutput" | "flowChartData" => {
            let d = shortest * a("adj", 25_000.0);
            closed(polygon(&[(d, 0.0), (w, 0.0), (w - d, h), (0.0, h)]))
        }
        "trapezoid" | "flowChartManualOperation" => {
            let d = shortest * a("adj", 25_000.0);
            closed(polygon(&[(d, 0.0), (w - d, 0.0), (w, h), (0.0, h)]))
        }
        "flowChartManualInput" => closed(polygon(&[(0.0, h * 0.2), (w, 0.0), (w, h), (0.0, h)])),
        "hexagon" | "flowChartPreparation" => {
            let d = shortest * a("adj", 25_000.0);
            closed(polygon(&[
                (d, 0.0),
                (w - d, 0.0),
                (w, h / 2.0),
                (w - d, h),
                (d, h),
                (0.0, h / 2.0),
            ]))
        }
        "octagon" => {
            let d = shortest * a("adj", 29_289.0);
            closed(polygon(&[
                (d, 0.0),
                (w - d, 0.0),
                (w, d),
                (w, h - d),
                (w - d, h),
                (d, h),
                (0.0, h - d),
                (0.0, d),
            ]))
        }
        "pentagon" => closed(polygon(&[
            (w / 2.0, 0.0),
            (w, h * 0.38),
            (w * 0.81, h),
            (w * 0.19, h),
            (0.0, h * 0.38),
        ])),
        "homePlate" => {
            let d = shortest * a("adj", 50_000.0);
            closed(polygon(&[
                (0.0, 0.0),
                (w - d, 0.0),
                (w, h / 2.0),
                (w - d, h),
                (0.0, h),
            ]))
        }
        "chevron" => {
            let d = shortest * a("adj", 50_000.0);
            closed(polygon(&[
                (0.0, 0.0),
                (w - d, 0.0),
                (w, h / 2.0),
                (w - d, h),
                (0.0, h),
                (d, h / 2.0),
            ]))
        }
        "rightArrow" | "leftArrow" | "upArrow" | "downArrow" => {
            // adj1: the shaft's share of the thickness; adj2: the head's
            // length as a share of the shortest side.
            let shaft = a("adj1", 50_000.0);
            let head = shortest * a("adj2", 50_000.0);
            let pts: Vec<(f64, f64)> = match prst {
                "rightArrow" => {
                    let (t0, t1) = (h * (1.0 - shaft) / 2.0, h * (1.0 + shaft) / 2.0);
                    vec![
                        (0.0, t0),
                        (w - head, t0),
                        (w - head, 0.0),
                        (w, h / 2.0),
                        (w - head, h),
                        (w - head, t1),
                        (0.0, t1),
                    ]
                }
                "leftArrow" => {
                    let (t0, t1) = (h * (1.0 - shaft) / 2.0, h * (1.0 + shaft) / 2.0);
                    vec![
                        (w, t0),
                        (head, t0),
                        (head, 0.0),
                        (0.0, h / 2.0),
                        (head, h),
                        (head, t1),
                        (w, t1),
                    ]
                }
                "upArrow" => {
                    let (t0, t1) = (w * (1.0 - shaft) / 2.0, w * (1.0 + shaft) / 2.0);
                    vec![
                        (t0, h),
                        (t0, head),
                        (0.0, head),
                        (w / 2.0, 0.0),
                        (w, head),
                        (t1, head),
                        (t1, h),
                    ]
                }
                _ => {
                    let (t0, t1) = (w * (1.0 - shaft) / 2.0, w * (1.0 + shaft) / 2.0);
                    vec![
                        (t0, 0.0),
                        (t0, h - head),
                        (0.0, h - head),
                        (w / 2.0, h),
                        (w, h - head),
                        (t1, h - head),
                        (t1, 0.0),
                    ]
                }
            };
            closed(polygon(&pts))
        }
        "can" | "flowChartMagneticDisk" => {
            // A cylinder: the body with an elliptical bottom, and the top
            // ellipse drawn over it.
            let ry = (h * a("adj", 25_000.0) / 2.0).min(h / 2.0);
            let rx = w / 2.0;
            let mut segs = vec![
                Seg::M(0.0, ry),
                Seg::L(0.0, h - ry),
                Seg::A(rx, ry, false, false, w, h - ry),
                Seg::L(w, ry),
                Seg::A(rx, ry, false, false, 0.0, ry),
                Seg::Z,
            ];
            segs.extend(ellipse(0.0, 0.0, w, ry * 2.0));
            closed(Path(segs))
        }
        "foldedCorner" => {
            let d = shortest * a("adj", 16_667.0);
            closed(Path(vec![
                Seg::M(0.0, 0.0),
                Seg::L(w, 0.0),
                Seg::L(w, h - d),
                Seg::L(w - d, h),
                Seg::L(0.0, h),
                Seg::Z,
                Seg::M(w - d, h),
                Seg::L(w - d * 0.85, h - d * 0.85),
                Seg::L(w, h - d),
            ]))
        }
        "bevel" => {
            let d = shortest * a("adj", 12_500.0);
            closed(Path(vec![
                Seg::M(0.0, 0.0),
                Seg::L(w, 0.0),
                Seg::L(w, h),
                Seg::L(0.0, h),
                Seg::Z,
                Seg::M(d, d),
                Seg::L(w - d, d),
                Seg::L(w - d, h - d),
                Seg::L(d, h - d),
                Seg::Z,
                Seg::M(0.0, 0.0),
                Seg::L(d, d),
                Seg::M(w, 0.0),
                Seg::L(w - d, d),
                Seg::M(w, h),
                Seg::L(w - d, h - d),
                Seg::M(0.0, h),
                Seg::L(d, h - d),
            ]))
        }
        "flowChartDocument" => closed(Path(vec![
            Seg::M(0.0, 0.0),
            Seg::L(w, 0.0),
            Seg::L(w, h * 0.8),
            Seg::C(w * 0.75, h * 0.6, w * 0.25, h * 1.1, 0.0, h * 0.85),
            Seg::Z,
        ])),
        "flowChartMultidocument" => {
            // Three stacked documents, each a step up and right.
            let (sx, sy) = (w * 0.08, h * 0.08);
            let mut segs = Vec::new();
            for i in (0..3).rev() {
                let (ox, oy) = (sx * i as f64, sy * (2 - i) as f64);
                let (dw, dh) = (w - 2.0 * sx, h - 2.0 * sy);
                segs.extend([
                    Seg::M(ox, oy),
                    Seg::L(ox + dw, oy),
                    Seg::L(ox + dw, oy + dh * 0.8),
                    Seg::C(
                        ox + dw * 0.75,
                        oy + dh * 0.6,
                        ox + dw * 0.25,
                        oy + dh * 1.1,
                        ox,
                        oy + dh * 0.85,
                    ),
                    Seg::Z,
                ]);
            }
            closed(Path(segs))
        }
        "flowChartPredefinedProcess" => {
            let d = w * 0.12;
            closed(Path(vec![
                Seg::M(0.0, 0.0),
                Seg::L(w, 0.0),
                Seg::L(w, h),
                Seg::L(0.0, h),
                Seg::Z,
                Seg::M(d, 0.0),
                Seg::L(d, h),
                Seg::M(w - d, 0.0),
                Seg::L(w - d, h),
            ]))
        }
        "flowChartDelay" => closed(Path(vec![
            Seg::M(0.0, 0.0),
            Seg::L(w - h / 2.0, 0.0),
            Seg::A(h / 2.0, h / 2.0, false, true, w - h / 2.0, h),
            Seg::L(0.0, h),
            Seg::Z,
        ])),
        "flowChartOffpageConnector" => closed(polygon(&[
            (0.0, 0.0),
            (w, 0.0),
            (w, h * 0.8),
            (w / 2.0, h),
            (0.0, h * 0.8),
        ])),
        "line" | "straightConnector1" => Geom {
            path: Path(vec![Seg::M(0.0, 0.0), Seg::L(w, h)]),
            supported: true,
            is_line: true,
            ends: Some(((0.0, 0.0), (w, h))),
        },
        "bentConnector2" => Geom {
            path: Path(vec![Seg::M(0.0, 0.0), Seg::L(w, 0.0), Seg::L(w, h)]),
            supported: true,
            is_line: true,
            ends: Some(((0.0, 0.0), (w, h))),
        },
        "bentConnector3" => {
            let mid = w * a("adj1", 50_000.0);
            Geom {
                path: Path(vec![
                    Seg::M(0.0, 0.0),
                    Seg::L(mid, 0.0),
                    Seg::L(mid, h),
                    Seg::L(w, h),
                ]),
                supported: true,
                is_line: true,
                ends: Some(((0.0, 0.0), (w, h))),
            }
        }
        "bentConnector4" | "bentConnector5" | "curvedConnector2" | "curvedConnector3"
        | "curvedConnector4" | "curvedConnector5" => Geom {
            path: Path(vec![Seg::M(0.0, 0.0), Seg::C(w, 0.0, 0.0, h, w, h)]),
            supported: false,
            is_line: true,
            ends: Some(((0.0, 0.0), (w, h))),
        },
        _ => Geom {
            path: polygon(&[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]),
            supported: false,
            is_line: false,
            ends: None,
        },
    }
}

/// A `custGeom`: every `a:path` of its `pathLst`, its `w`/`h` coordinate
/// space scaled to the shape's box.
fn custom_geometry(cust: XmlNode, w: f64, h: f64) -> Geom {
    let mut segs = Vec::new();
    let mut is_line = true;
    for path in cust.descendants().filter(|n| n.has_tag_name("path")) {
        let pw = emu(path, "w")
            .filter(|v| *v > 0.0)
            .unwrap_or(w * EMU_PER_PX);
        let ph = emu(path, "h")
            .filter(|v| *v > 0.0)
            .unwrap_or(h * EMU_PER_PX);
        let (sx, sy) = (w / pw, h / ph);
        let pts = |n: XmlNode| -> Vec<(f64, f64)> {
            n.children()
                .filter(|c| c.has_tag_name("pt"))
                .filter_map(|p| Some((emu(p, "x")? * sx, emu(p, "y")? * sy)))
                .collect()
        };
        for cmd in path.children().filter(|n| n.is_element()) {
            let p = pts(cmd);
            match (cmd.tag_name().name(), p.as_slice()) {
                ("moveTo", [a]) => segs.push(Seg::M(a.0, a.1)),
                ("lnTo", [a]) => segs.push(Seg::L(a.0, a.1)),
                ("cubicBezTo", [a, b, c]) => segs.push(Seg::C(a.0, a.1, b.0, b.1, c.0, c.1)),
                ("quadBezTo", [a, b]) => segs.push(Seg::Q(a.0, a.1, b.0, b.1)),
                ("close", _) => {
                    segs.push(Seg::Z);
                    is_line = false;
                }
                _ => {}
            }
        }
    }
    if segs.is_empty() {
        return Geom {
            path: polygon(&[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]),
            supported: false,
            is_line: false,
            ends: None,
        };
    }
    Geom {
        path: Path(segs),
        supported: true,
        is_line,
        ends: None,
    }
}

// ---------------------------------------------------------------------------
// Small helpers.
// ---------------------------------------------------------------------------

/// A pixel value for an attribute: two decimals, trailing zeros dropped —
/// the same bytes on every host.
fn px(v: f64) -> String {
    let v = if v.abs() < 0.005 { 0.0 } else { v };
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.to_string()
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// `prstDash` → `stroke-dasharray`, in multiples of the stroke width as
/// DrawingML defines them.
fn dash_array(kind: &str, w: f64) -> String {
    let w = w.max(0.5);
    let pattern: &[f64] = match kind {
        "dot" | "sysDot" => &[1.0, 3.0],
        "dash" | "sysDash" => &[4.0, 3.0],
        "lgDash" => &[8.0, 3.0],
        "dashDot" | "sysDashDot" => &[4.0, 3.0, 1.0, 3.0],
        "lgDashDot" => &[8.0, 3.0, 1.0, 3.0],
        "lgDashDotDot" | "sysDashDotDot" => &[8.0, 3.0, 1.0, 3.0, 1.0, 3.0],
        _ => &[4.0, 3.0],
    };
    pattern
        .iter()
        .map(|m| px(m * w))
        .collect::<Vec<_>>()
        .join(" ")
}

fn preset_color(name: &str) -> Option<&'static str> {
    Some(match name {
        "black" => "000000",
        "white" => "FFFFFF",
        "red" => "FF0000",
        "green" => "008000",
        "lime" => "00FF00",
        "blue" => "0000FF",
        "yellow" => "FFFF00",
        "gray" | "grey" => "808080",
        "silver" => "C0C0C0",
        "navy" => "000080",
        "orange" => "FFA500",
        "purple" => "800080",
        "maroon" => "800000",
        "teal" => "008080",
        "olive" => "808000",
        "aqua" | "cyan" => "00FFFF",
        "fuchsia" | "magenta" => "FF00FF",
        "dkGray" | "darkGray" => "A9A9A9",
        "ltGray" | "lightGray" => "D3D3D3",
        _ => return None,
    })
}

fn parse_hex(hex: &str) -> Option<[f64; 3]> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let ch = |i: usize| {
        u8::from_str_radix(&hex[i..i + 2], 16)
            .ok()
            .map(|v| v as f64 / 255.0)
    };
    Some([ch(0)?, ch(2)?, ch(4)?])
}

fn format_hex(rgb: [f64; 3]) -> String {
    let ch = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("{:02X}{:02X}{:02X}", ch(rgb[0]), ch(rgb[1]), ch(rgb[2]))
}

/// Apply `f` to the HSL luminance of `rgb` (Word's `lumMod` / `lumOff`).
fn with_luminance(rgb: [f64; 3], f: impl Fn(f64) -> f64) -> [f64; 3] {
    let (r, g, b) = (rgb[0], rgb[1], rgb[2]);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    let (h, s) = if d < 1e-9 {
        (0.0, 0.0)
    } else {
        let s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };
        let h = if max == r {
            ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
        } else if max == g {
            ((b - r) / d + 2.0) / 6.0
        } else {
            ((r - g) / d + 4.0) / 6.0
        };
        (h, s)
    };
    let l = f(l).clamp(0.0, 1.0);
    if s < 1e-9 {
        return [l, l, l];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hue = |mut t: f64| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A development aid, not a check: render every `drawingml_*.docx`
    /// fixture's pictures to `$DOCLING_RS_SVG_DUMP/<fixture>-<n>.svg` (and
    /// `.png` through resvg when the build has it), to look at.
    /// `DOCLING_RS_SVG_DUMP=/tmp/svg cargo test -p docling --lib -- --ignored dump_fixture_renders`
    #[test]
    #[ignore]
    fn dump_fixture_renders() {
        use crate::backend::DeclarativeBackend;
        let Some(dir) = std::env::var_os("DOCLING_RS_SVG_DUMP") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sources =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/docx/sources");
        let root =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/data/docx/sources");
        let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(&sources)
            .unwrap()
            .chain(std::fs::read_dir(&root).unwrap())
            .map(|e| e.unwrap().path())
            .filter(|p| {
                let n = p.file_name().unwrap().to_string_lossy();
                n.ends_with(".docx") && (n.starts_with("drawingml") || n.starts_with("textbox"))
            })
            .collect();
        paths.sort();
        for path in paths {
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let src = crate::SourceDocument::from_file(&path).unwrap();
            let doc = super::super::docx::DocxBackend::default()
                .convert(&src)
                .unwrap();
            let mut n = 0;
            for node in &doc.nodes {
                if let docling_core::Node::Picture {
                    image: Some(img), ..
                } = node
                {
                    if img.mimetype != "image/svg+xml" {
                        continue;
                    }
                    let svg = dir.join(format!("{stem}-{n}.svg"));
                    std::fs::write(&svg, &img.data).unwrap();
                    #[cfg(feature = "pdf")]
                    {
                        use resvg::{tiny_skia, usvg};
                        let mut db = usvg::fontdb::Database::new();
                        db.load_system_fonts();
                        let opt = usvg::Options {
                            fontdb: std::sync::Arc::new(db),
                            ..Default::default()
                        };
                        let tree = usvg::Tree::from_data(&img.data, &opt).unwrap();
                        let scale = 2.0;
                        let (w, h) = (
                            (tree.size().width() * scale).ceil() as u32,
                            (tree.size().height() * scale).ceil() as u32,
                        );
                        let mut pixmap = tiny_skia::Pixmap::new(w.max(1), h.max(1)).unwrap();
                        pixmap.fill(tiny_skia::Color::WHITE);
                        resvg::render(
                            &tree,
                            tiny_skia::Transform::from_scale(scale, scale),
                            &mut pixmap.as_mut(),
                        );
                        std::fs::write(
                            dir.join(format!("{stem}-{n}.png")),
                            pixmap.encode_png().unwrap(),
                        )
                        .unwrap();
                    }
                    n += 1;
                }
            }
            eprintln!("{stem}: {n} svg picture(s)");
        }
    }

    fn fixture(name: &str) -> docling_core::DoclingDocument {
        use crate::backend::DeclarativeBackend;
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/docx/sources")
            .join(name);
        let src = crate::SourceDocument::from_file(&path).unwrap();
        super::super::docx::DocxBackend::default()
            .convert(&src)
            .unwrap()
    }

    fn svg_pictures(doc: &docling_core::DoclingDocument) -> Vec<&PictureImage> {
        doc.nodes
            .iter()
            .filter_map(|n| match n {
                docling_core::Node::Picture {
                    image: Some(img), ..
                } if img.mimetype == "image/svg+xml" => Some(img),
                _ => None,
            })
            .collect()
    }

    /// The paragraph's drawings of a fixture, rendered through the shape
    /// model (what the Mermaid extraction reads).
    fn rendered(name: &str) -> Rendered {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data/docx/sources")
            .join(name);
        let bytes = std::fs::read(&path).unwrap();
        let mut pkg = super::super::ooxml::Package::open(&bytes).unwrap();
        let xml = pkg.read("word/document.xml").unwrap();
        let theme = pkg
            .read("word/theme/theme1.xml")
            .map(|t| Theme::parse(&t))
            .unwrap_or_default()
            .with_styles(&pkg.read("word/styles.xml").unwrap_or_default());
        let xml = super::super::mc::resolve_alternate_content(&xml);
        let dom = roxmltree::Document::parse(&xml).unwrap();
        let drawings: Vec<XmlNode> = dom
            .descendants()
            .filter(|n| n.has_tag_name("drawing"))
            .collect();
        render(&drawings, &theme).expect("a rendered drawing")
    }

    /// #858 acceptance A/B: the supported fixture is one SVG picture of the
    /// drawing's extent, its three nodes and two connectors read with their
    /// identities, connection sites and arrow ends, with nothing
    /// approximated; the tree chain carries the same payload.
    #[test]
    fn supported_flowchart_renders_fully() {
        let doc = fixture("drawingml_flowchart_supported.docx");
        let pics = svg_pictures(&doc);
        assert_eq!(pics.len(), 1);
        let img = pics[0];
        assert_eq!((img.width, img.height), (525, 210));
        assert_eq!(img.dpi, 96);
        let svg = std::str::from_utf8(&img.data).unwrap();
        assert!(
            svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"524.93\""),
            "{svg}"
        );
        assert_eq!(svg.matches("<text ").count(), 3, "{svg}");
        assert!(
            svg.contains("fill=\"#E8F1F8\" stroke=\"#0F4761\" stroke-width=\"1.33\""),
            "{svg}"
        );
        // The labels stay text items, as docling keeps them.
        let md = doc.export_to_markdown();
        assert!(
            md.contains("Start Task\n\nDecision Process\n\nEnd Task\n\n```mermaid\n"),
            "{md}"
        );
        // The item tree's picture carries the same bytes.
        let json = doc.export_to_json();
        assert!(json.contains("data:image/svg+xml;base64,"), "{json}");

        let r = rendered("drawingml_flowchart_supported.docx");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let names: Vec<&str> = r.shapes.iter().filter_map(|s| s.name.as_deref()).collect();
        assert_eq!(
            names,
            [
                "StartTask",
                "DecisionProcess",
                "EndTask",
                "Connector1",
                "Connector2"
            ]
        );
        let c1 = &r.shapes[3];
        assert!(c1.connector);
        assert_eq!((c1.start, c1.end), (Some(10), Some(20)));
        assert_eq!((c1.head, c1.tail), (LineEnd::None, LineEnd::Arrow));
        assert_eq!(r.shapes[1].text, ["Decision Process"]);
        assert_eq!(r.shapes[1].geometry, "roundRect");
    }

    /// #858 acceptance C: the complex fixture renders what it can and says
    /// what it approximated — the `bevel` pair and the multidocument are
    /// drawn as their own presets, the four gradients as their first stop,
    /// nothing is invented — and keeps all twelve labels.
    #[test]
    fn complex_flowchart_renders_with_diagnostics() {
        let r = rendered("drawingml_flowchart_complex.docx");
        assert_eq!(r.shapes.len(), 25);
        let labels: Vec<&str> = r
            .shapes
            .iter()
            .filter(|s| !s.text.is_empty())
            .map(|s| s.text[0].as_str())
            .collect();
        assert_eq!(labels.len(), 12, "{labels:?}");
        assert!(labels.contains(&"Dispatch to Work Table or Error Table"));
        assert_eq!(
            r.diagnostics
                .iter()
                .filter(|d| d.contains("gradient"))
                .count(),
            4,
            "{:?}",
            r.diagnostics
        );
        assert!(
            r.diagnostics.iter().all(|d| d.contains("gradient")),
            "{:?}",
            r.diagnostics
        );
        // No connector declares its sites: nothing for a graph to read.
        assert!(r
            .shapes
            .iter()
            .all(|s| s.start.is_none() && s.end.is_none()));
        let lines = r.shapes.iter().filter(|s| s.geometry == "line").count();
        assert_eq!(lines, 12);
        let svg = std::str::from_utf8(&r.image.data).unwrap();
        // The flipped lines carry their transform; the frame rect draws
        // nothing (no fill, no line).
        assert!(
            svg.contains("scale(-1 1)") && svg.contains("scale(1 -1)"),
            "{svg}"
        );
        assert!(svg.contains("fill=\"none\" stroke=\"none\""), "{svg}");
        // A long label wraps inside its box.
        assert!(
            svg.contains(">Dispatch to Work Table<") && svg.contains(">or Error Table<"),
            "{svg}"
        );
    }

    /// #858 acceptance B: `headEnd` decorates the connector's start, so an
    /// arrow there points into Node A; `tailEnd` points into Node B.
    #[test]
    fn arrow_ends_follow_the_connector() {
        let tail = rendered("drawingml_arrow_tail.docx");
        let c = tail.shapes.iter().find(|s| s.connector).unwrap();
        assert_eq!(
            (c.start, c.end, c.head, c.tail),
            (Some(10), Some(20), LineEnd::None, LineEnd::Arrow)
        );
        let head = rendered("drawingml_arrow_head.docx");
        let c = head.shapes.iter().find(|s| s.connector).unwrap();
        assert_eq!((c.head, c.tail), (LineEnd::Arrow, LineEnd::None));
        // The arrowhead polygon sits at the line's start (x = 136.48, the
        // connector's left end) for `headEnd`, at its end for `tailEnd`.
        let svg_head = std::str::from_utf8(&head.image.data).unwrap();
        let svg_tail = std::str::from_utf8(&tail.image.data).unwrap();
        // The arrowhead is the polygon filled with the stroke colour.
        let arrow_x = |svg: &str| -> String {
            let end = svg.find("Z\" fill=\"#0F4761\" stroke=\"none\"/>").unwrap();
            let i = svg[..end].rfind("<path d=\"M").unwrap();
            svg[i + 10..].split(' ').next().unwrap().to_string()
        };
        assert_eq!(arrow_x(svg_head), "136.48", "{svg_head}");
        assert_eq!(arrow_x(svg_tail), "262.47", "{svg_tail}");
    }

    /// #858 acceptance D3: the ticket's transform oracle through a real
    /// drawing — `chOff` (100000, 200000), `chExt` 2 500 000 × 1 000 000
    /// into `ext` 5 000 000 × 2 000 000: the first node lands at (0, 63 px)
    /// and 252 px wide.
    #[test]
    fn group_transform_oracle() {
        let r = rendered("drawingml_group_transform.docx");
        let svg = std::str::from_utf8(&r.image.data).unwrap();
        assert!(
            svg.contains("<path d=\"M0 62.99 L251.97 62.99 L251.97 188.98 L0 188.98 Z\""),
            "{svg}"
        );
    }

    /// #858 acceptance A2 / B / C2: the supported fixture's exact
    /// two-edge graph; the directional pair; the complex fixture declines
    /// (its twelve lines name no connection sites) rather than guessing.
    #[test]
    fn mermaid_from_explicit_connectors_only() {
        let r = rendered("drawingml_flowchart_supported.docx");
        assert_eq!(
            mermaid(&r.shapes).unwrap(),
            "flowchart TD\n  StartTask[\"Start Task\"] --> DecisionProcess[\"Decision Process\"]\n  DecisionProcess[\"Decision Process\"] --> EndTask[\"End Task\"]"
        );
        let tail = rendered("drawingml_arrow_tail.docx");
        assert_eq!(
            mermaid(&tail.shapes).unwrap(),
            "flowchart TD\n  NodeA[\"Node A\"] --> NodeB[\"Node B\"]"
        );
        let head = rendered("drawingml_arrow_head.docx");
        assert_eq!(
            mermaid(&head.shapes).unwrap(),
            "flowchart TD\n  NodeB[\"Node B\"] --> NodeA[\"Node A\"]"
        );
        let complex = rendered("drawingml_flowchart_complex.docx");
        let err = mermaid(&complex.shapes).unwrap_err();
        assert!(err.contains("no connection sites"), "{err}");
        let transform = rendered("drawingml_group_transform.docx");
        assert!(mermaid(&transform.shapes).is_ok());
    }

    /// Marker neutrality and escaping on a synthetic graph: `oval` and
    /// `diamond` ends are undirected, both arrows bidirectional, a quote
    /// and a hash in a label become entity codes, a nameless node gets a
    /// stable id, an unlinked node is listed on its own.
    #[test]
    fn mermaid_edges_and_escaping() {
        let node = |id: u32, name: Option<&str>, text: &str| Shape {
            id: Some(id),
            name: name.map(str::to_string),
            geometry: "rect".into(),
            text: vec![text.into()],
            ..Shape::default()
        };
        let link = |start: u32, end: u32, head: LineEnd, tail: LineEnd| Shape {
            id: None,
            connector: true,
            geometry: "line".into(),
            start: Some(start),
            end: Some(end),
            head,
            tail,
            ..Shape::default()
        };
        let shapes = vec![
            node(1, Some("A"), "Say \"hi\" #1"),
            node(2, None, "B [x]"),
            node(3, Some("C"), "C"),
            node(4, Some("Lonely"), "Alone"),
            link(1, 2, LineEnd::Oval, LineEnd::Diamond),
            link(2, 3, LineEnd::Arrow, LineEnd::Arrow),
        ];
        assert_eq!(
            mermaid(&shapes).unwrap(),
            "flowchart TD\n  A[\"Say #quot;hi#quot; #35;1\"] --- S2[\"B [x]\"]\n  S2[\"B [x]\"] <--> C[\"C\"]\n  Lonely[\"Alone\"]"
        );
        // A line laid by eye (no sites) makes the graph ambiguous.
        let mut by_eye = shapes.clone();
        by_eye.push(Shape {
            geometry: "line".into(),
            ..Shape::default()
        });
        assert!(mermaid(&by_eye).is_err());
        assert_eq!(mermaid(&shapes[..4]).unwrap_err(), "no connectors");
    }

    /// The whole way through: the default `extract_diagrams=mermaid` adds
    /// the graph as a `mermaid` code item after the picture, in both
    /// chains, and the Markdown fence carries the language even in the
    /// legacy mode; `none` turns it off; the ambiguous fixture gets no
    /// code item.
    #[test]
    fn extract_diagrams_adds_a_code_item() {
        use crate::backend::DeclarativeBackend;
        let convert = |name: &str, on: bool| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/data/docx/sources")
                .join(name);
            let src = crate::SourceDocument::from_file(&path).unwrap();
            let backend = if on {
                super::super::docx::DocxBackend::default()
            } else {
                super::super::docx::DocxBackend {
                    extract_diagrams: crate::DiagramFormat::None,
                    ..Default::default()
                }
            };
            backend.convert(&src).unwrap()
        };
        let doc = convert("drawingml_flowchart_supported.docx", true);
        let md = doc.export_to_markdown();
        // The graph takes the placeholder's place in Markdown; the JSON
        // keeps the picture (its SVG payload) and the code item.
        assert!(
            md.contains("End Task\n\n```mermaid\nflowchart TD\n  StartTask[\"Start Task\"] --> DecisionProcess[\"Decision Process\"]\n  DecisionProcess[\"Decision Process\"] --> EndTask[\"End Task\"]\n```\n"),
            "{md}"
        );
        assert!(!md.contains("<!-- image -->"), "{md}");
        let json = doc.export_to_json();
        assert!(json.contains("\"label\": \"code\""), "{json}");
        assert!(json.contains("flowchart TD"), "{json}");
        assert!(json.contains("data:image/svg+xml;base64,"), "{json}");
        assert!(!convert("drawingml_flowchart_supported.docx", false)
            .export_to_markdown()
            .contains("mermaid"));
        assert!(!convert("drawingml_flowchart_complex.docx", true)
            .export_to_markdown()
            .contains("mermaid"));
    }

    #[test]
    fn pixels_are_deterministic_and_short() {
        assert_eq!(px(0.0), "0");
        assert_eq!(px(-0.001), "0");
        assert_eq!(px(12.5), "12.5");
        assert_eq!(px(12.345), "12.35");
        assert_eq!(px(100.0), "100");
    }

    #[test]
    fn theme_tints_follow_word() {
        // accent1 5B9BD5 with lumMod 75% is Word's "Blue, Accent 1, Darker 25%".
        let rgb = parse_hex("5B9BD5").unwrap();
        assert_eq!(format_hex(with_luminance(rgb, |l| l * 0.75)), "2E75B6");
        // shade 50% halves every channel.
        assert_eq!(format_hex(rgb.map(|c| c * 0.5)), "2E4E6B");
    }

    #[test]
    fn group_transform_maps_child_space_to_the_canvas() {
        // The ticket's numeric oracle (#858 §7.4): chOff (100000, 200000),
        // chExt 2500000 × 1000000 into ext 5000000 × 2000000 — a child at
        // (100000, 500000) lands at (0, 600000), width 1200000 → 2400000.
        let xml = r#"<a:xfrm xmlns:a="x"><a:off x="0" y="0"/><a:ext cx="5000000" cy="2000000"/><a:chOff x="100000" y="200000"/><a:chExt cx="2500000" cy="1000000"/></a:xfrm>"#;
        let dom = roxmltree::Document::parse(xml).unwrap();
        let root = Xform {
            ox: 0.0,
            oy: 0.0,
            sx: 1.0,
            sy: 1.0,
        };
        let t = root.child(Some(dom.root_element()));
        assert_eq!(t.apply(100_000.0, 500_000.0), (0.0, 600_000.0));
        let (x0, _) = t.apply(100_000.0, 0.0);
        let (x1, _) = t.apply(1_300_000.0, 0.0);
        assert_eq!(x1 - x0, 2_400_000.0);
    }

    #[test]
    fn line_ends_read_like_the_ticket() {
        assert_eq!(LineEnd::parse(Some("triangle")), LineEnd::Arrow);
        assert_eq!(LineEnd::parse(Some("stealth")), LineEnd::Arrow);
        assert_eq!(LineEnd::parse(Some("oval")), LineEnd::Oval);
        assert_eq!(LineEnd::parse(Some("none")), LineEnd::None);
        assert!(!LineEnd::Diamond.is_arrow());
    }
}
