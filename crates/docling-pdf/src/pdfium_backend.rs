//! pdfium-based text extraction and page rendering.
//!
//! Text is reconstructed the way docling's `docling-parse` does it, so the
//! output spacing matches the groundtruth: the page's **character** stream is
//! grouped into **words** (split at a horizontal gap wider than a fraction of
//! the font height — font-relative, so letter-tracking in display titles does
//! not split a word) and words into **lines** (by baseline). pdfium-render's
//! safe API only exposes whole style runs / `GetBoundedText`, so the character
//! loop is driven through the raw `PdfiumLibraryBindings` FFI on a second handle
//! to the same bytes (no fork; stays publishable).

#[cfg(feature = "ocr-prep")]
use image::RgbImage;
#[cfg(feature = "ml")]
use pdfium_render::prelude::*;

/// A run of text with its bounding box, in PDF points with a **top-left** origin
/// (pdfium's native origin is bottom-left; we flip it to match docling's
/// `BoundingBox(..., origin=TOPLEFT)`).
#[derive(Debug, Clone)]
pub struct TextCell {
    pub text: String,
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
}

/// Pixels-per-point used to render page images. Layout is scale-invariant (it
/// scales normalized boxes by the page point size), but OCR benefits from the
/// extra resolution.
pub const RENDER_SCALE: f32 = 2.0;

/// One page's geometry, extracted text cells, and a rendered RGB image. The
/// image is rendered at [`RENDER_SCALE`] pixels per PDF point; `image px =
/// page point × scale`.
#[derive(Clone)]
pub struct PdfPage {
    pub width: f32,
    pub height: f32,
    pub scale: f32,
    pub cells: Vec<TextCell>,
    /// Same text grouped for code regions: split only at pdfium space glyphs, so
    /// monospace runs keep their source spacing instead of the prose heuristic's.
    pub code_cells: Vec<TextCell>,
    /// Per-word cells (one per word, not joined into lines) for TableFormer cell
    /// matching.
    pub word_cells: Vec<TextCell>,
    /// The rendered page bitmap. Present whenever pixels are available at all
    /// (`ocr-prep` ⊂ `ml`): the native pipeline renders it with pdfium, the
    /// browser pipeline receives it from the host canvas. Picture regions are
    /// cropped out of it.
    #[cfg(feature = "ocr-prep")]
    pub image: RgbImage,
    /// The **scale-1.0** page image the layout model runs on (docling parity:
    /// its layout stage calls `page.get_image(scale=1.0)` — pdfium at 1.5×,
    /// PIL-BICUBIC down to point size — a *different* image from the 2×
    /// OCR/crop bitmap above, and a different resampling regime than
    /// stretching that bitmap). `None` on paths without a pdfium renderer
    /// (browser, METS/TIFF), which fall back to stretching [`Self::image`].
    #[cfg(feature = "ocr-prep")]
    pub image_layout: Option<RgbImage>,
    /// Hyperlink annotations on the page (rect in top-left page coords + target
    /// URI), restricted to web/mail/tel schemes. Used only by strict Markdown.
    pub links: Vec<LinkAnnot>,
    /// The page's `/Rotate` value (0/90/180/270) when it was normalized away
    /// before inference: a scanned page with `/Rotate` displays its raster
    /// rotated, which turns OCR into garbage — so extraction un-rotates the
    /// bitmaps (and swaps `width`/`height`) and records the display rotation
    /// here. Assembly rotates the finished geometry *back* by this many
    /// degrees clockwise, so emitted locations and the page size stay in
    /// display space (matching docling and every PDF viewer). Always 0 for
    /// text-layer pages (their cells live in display space already) and on
    /// paths without a pdfium renderer.
    pub rotation: u16,
}

impl PdfPage {
    /// A page built from recognized cells alone — the browser pipeline's
    /// shape (#157), where the bitmap lives on the JS side. Exists so callers
    /// compile identically with and without the `ml` feature: under a
    /// feature-unified workspace build the struct carries the `image` field,
    /// which a plain literal in a non-`ml` consumer can't spell.
    #[cfg(feature = "ocr-prep")]
    pub fn from_cells(width: f32, height: f32, scale: f32, cells: Vec<TextCell>) -> Self {
        Self {
            width,
            height,
            scale,
            cells,
            code_cells: Vec::new(),
            word_cells: Vec::new(),
            #[cfg(feature = "ocr-prep")]
            image: RgbImage::new(0, 0),
            #[cfg(feature = "ocr-prep")]
            image_layout: None,
            links: Vec::new(),
            rotation: 0,
        }
    }

    /// Same as [`from_cells`](Self::from_cells) but carrying the rendered page
    /// bitmap, so picture regions can be cropped out of it (#157: the browser
    /// pipeline gets the same figure bytes the native one does).
    #[cfg(feature = "ocr-prep")]
    pub fn from_cells_with_image(
        width: f32,
        height: f32,
        scale: f32,
        cells: Vec<TextCell>,
        image: RgbImage,
    ) -> Self {
        Self {
            image,
            ..Self::from_cells(width, height, scale, cells)
        }
    }

    /// Un-rotate the page's bitmaps by `deg` (clockwise 90° steps) and record
    /// the compensating display rotation, composing with any rotation already
    /// recorded: the raster becomes upright for inference while assembly
    /// still maps the finished geometry back into display space. Handles both
    /// `/Rotate` normalization (extraction) and content-detected orientation
    /// (#225) — the two compose additively (axis-aligned 90° rotations
    /// commute through the dimension swaps). Link rectangles follow the
    /// raster; `width`/`height` swap on odd quarter-turns.
    #[cfg(feature = "ocr-prep")]
    pub(crate) fn unrotate(&mut self, deg: u16) {
        if deg == 0 {
            return;
        }
        use image::imageops::{rotate180, rotate270, rotate90};
        // Display = upright rotated `deg`° clockwise, so upright = display
        // rotated the complementary amount clockwise.
        let un = |img: &RgbImage| match deg {
            90 => rotate270(img),
            180 => rotate180(img),
            _ => rotate90(img),
        };
        if self.image.width() > 1 {
            self.image = un(&self.image);
        }
        self.image_layout = self.image_layout.as_ref().map(&un);
        let (width, height) = (self.width, self.height);
        // Link rects follow the raster from display into upright space (the
        // inverse of the geometry rotation assembly applies at the end).
        for l in &mut self.links {
            let (nl, nt, nr, nb) = match deg {
                90 => (l.t, width - l.r, l.b, width - l.l),
                180 => (width - l.r, height - l.b, width - l.l, height - l.t),
                _ => (height - l.b, l.l, height - l.t, l.r),
            };
            (l.l, l.t, l.r, l.b) = (nl, nt, nr, nb);
        }
        if deg != 180 {
            (self.width, self.height) = (height, width);
        }
        self.rotation = (self.rotation + deg) % 360;
    }
}

/// A PDF link annotation: its rectangle (top-left page coordinates, matching
/// [`TextCell`]) and target URI.
#[derive(Debug, Clone)]
pub struct LinkAnnot {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
    pub uri: String,
}

#[cfg(feature = "ml")]
/// A parsed PDF: per-page text cells and page images.
pub struct PdfDocument {
    pub pages: Vec<PdfPage>,
}

/// Whether to use the docling-parse line sanitizer ([`crate::dp_lines`]) for prose
/// reconstruction — the default. Set `DOCLING_LEGACY_LINES` to fall back to the
/// older gap-heuristic `lines_from_glyphs`.
pub(crate) fn use_dp_lines() -> bool {
    !docling_core::env::flag("DOCLING_LEGACY_LINES")
}

/// Whether to source **word** cells from the pure-Rust parser (roadmap item 6),
/// the default. The parser's `word_cells` reproduce docling-parse's word grouping
/// byte-for-byte — the per-word tokens TableFormer matches table-grid cells
/// against — which moves table extraction closer to docling on the heavy
/// multi-column fixtures. Set `DOCLING_PDFIUM_WORDS` to keep pdfium's word cells,
/// or `DOCLING_PDFIUM_TEXT` to fall back to pdfium for all text.
pub(crate) fn use_parser_words() -> bool {
    !docling_core::env::flag("DOCLING_PDFIUM_WORDS")
        && !docling_core::env::flag("DOCLING_PDFIUM_TEXT")
}

/// Whether to source **code** cells from the parser too (the default) — the last
/// text layer to leave pdfium, fully retiring its text path. The parser's
/// gap-based code grouping ([`code_cells_from_glyphs`]) reconstructs monospace
/// spacing from positioning gaps (`function add(a, b) { … }`), so it no longer
/// drops the inter-token spaces the old space-glyph-only grouping lost
/// (`functionadd`). Reverts to pdfium with `DOCLING_PDFIUM_WORDS` (alongside word
/// cells) or `DOCLING_PDFIUM_TEXT` (all text).
pub(crate) fn use_parser_code() -> bool {
    use_parser_words()
}

#[cfg(feature = "ml")]
/// Try binding pdfium from a directory (or a literal library file path):
/// `<dir>/<platform library name>` first, else `<dir>` itself as the file.
fn try_bind_dir(path: &str) -> Option<Box<dyn pdfium_render::prelude::PdfiumLibraryBindings>> {
    let name = Pdfium::pdfium_platform_library_name_at_path(path);
    if let Ok(b) = Pdfium::bind_to_library(&name) {
        return Some(b);
    }
    Pdfium::bind_to_library(path).ok()
}

#[cfg(feature = "ml")]
/// Bind to the pdfium dynamic library. Honors `PDFIUM_DYNAMIC_LIB_PATH` (a
/// directory or file) first; else falls back to `.pdfium/lib` relative to the
/// current directory (the layout `scripts/install/download_dependencies.sh` and
/// `scripts/install/pdf_setup.sh` both produce); else the system library.
fn bind() -> Result<Pdfium, PdfiumError> {
    if let Some(path) = docling_core::env::nonempty("PDFIUM_DYNAMIC_LIB_PATH") {
        if let Some(b) = try_bind_dir(&path) {
            return Ok(Pdfium::new(b));
        }
    }
    // No env var (or it didn't resolve): fall back to `.pdfium/lib` relative to
    // the current directory — mirroring `layout.rs`/`ocr.rs`'s `.models/…`
    // defaults — the layout `scripts/install/download_dependencies.sh` (and
    // `scripts/install/pdf_setup.sh`) produce, so a checkout with the dependencies
    // downloaded next to it needs no env var at all.
    if let Some(b) = try_bind_dir(&crate::resolve_asset(".pdfium/lib")) {
        return Ok(Pdfium::new(b));
    }
    Pdfium::bind_to_system_library().map(Pdfium::new)
}

#[cfg(feature = "ml")]
impl PdfDocument {
    /// Parse a PDF from bytes, optionally decrypting with `password`.
    ///
    /// Note: this materialises **every** page's rendered bitmap in memory at
    /// once. For large documents prefer [`for_each_page`], which streams.
    pub fn open(bytes: &[u8], password: Option<&str>) -> Result<Self, PdfiumError> {
        let pdfium = bind()?;
        let ffi = FfiText::load(pdfium.bindings(), bytes, password);
        let doc = pdfium.load_pdf_from_byte_slice(bytes, password)?;
        let mut rust = rust_parser_cells(bytes);
        let mut pages = Vec::new();
        for (i, page) in doc.pages().iter().enumerate() {
            let rc = rust.as_mut().and_then(|v| v.get_mut(i).map(std::mem::take));
            pages.push(extract_page(&page, &ffi, i as i32, rc, true, true)?);
        }
        Ok(PdfDocument { pages })
    }
}

#[cfg(feature = "ml")]
/// Per-page prose line cells from the pure-Rust text parser. This is the
/// **default** text layer (it matches docling-parse's char geometry and is a
/// strict improvement on byte-conformance — e.g. it recovers the Arabic
/// sentence-period attachment in `right_to_left_01`). Set `DOCLING_PDFIUM_TEXT`
/// to fall back to pdfium's text layer. The parser returns an empty page when a
/// PDF (or a page) has no parseable text layer; the caller keeps pdfium's cells
/// in that case, so scanned/edge-case pages are unaffected.
fn rust_parser_cells(bytes: &[u8]) -> Option<Vec<crate::textparse::PageParserCells>> {
    if docling_core::env::flag("DOCLING_PDFIUM_TEXT") {
        return None;
    }
    Some(crate::timing::timed("textparse", || {
        crate::textparse::pdf_all_cells(bytes)
    }))
}

#[cfg(feature = "ml")]
/// Number of pages in a PDF, without rendering any of them — used to decide
/// whether a document is worth spinning up the parallel worker pool.
pub fn page_count(bytes: &[u8], password: Option<&str>) -> Result<usize, PdfiumError> {
    let pdfium = bind()?;
    let doc = pdfium.load_pdf_from_byte_slice(bytes, password)?;
    Ok(doc.pages().len() as usize)
}

#[cfg(feature = "ml")]
/// Render + extract pages one at a time, handing each (owned) [`PdfPage`] to `f`.
/// Only one page bitmap is resident at a time — a rendered page is ~5 MB, so a
/// large PDF would otherwise hold gigabytes of bitmaps at once. `f` receives the
/// zero-based page index and the total page count.
///
/// `render_image` controls whether the page bitmap is rasterized at all: layout,
/// OCR, TableFormer, and picture cropping all need it, but a caller that skips
/// every one of those (the `no_ocr` fast path) doesn't, and rasterizing +
/// downsampling a page is by far the most expensive step per page — skipping it
/// is most of `no_ocr`'s speedup. `PdfPage::image` is a 1×1 placeholder when
/// `false`; do not read it.
///
/// `extract_text` decodes the page's text layer (parser or pdfium cells); pass
/// `false` when full-page OCR is forced and the cells would be discarded
/// unread (docling#4061).
///
/// `range` restricts the walk to a **0-based inclusive** page window (issue
/// #80's `--pages`); out-of-window pages are skipped *before* text extraction
/// and rasterization, so a 3-page window over a 500-page PDF costs three
/// pages, not five hundred. `f` still receives the absolute page index, so
/// downstream page numbering refers to the source document.
///
/// `E` is the caller's error type; pdfium errors convert into it via `From`.
pub fn for_each_page<E, F>(
    bytes: &[u8],
    password: Option<&str>,
    render_image: bool,
    extract_text: bool,
    range: Option<(usize, usize)>,
    mut f: F,
) -> Result<(), E>
where
    E: From<PdfiumError>,
    F: FnMut(usize, usize, PdfPage) -> Result<(), E>,
{
    let pdfium = bind()?;
    let ffi = FfiText::load(pdfium.bindings(), bytes, password);
    let doc = pdfium.load_pdf_from_byte_slice(bytes, password)?;
    // `extract_text = false` (full-page OCR forced, docling#4061 / 2.122):
    // the text layer would be cleared unread, so neither the pure-Rust parser
    // nor pdfium's text page is decoded at all — on vector-dense pages (CAD
    // drawings as 100k+ path segments) that decode is most of the page cost.
    let mut rust = if extract_text {
        rust_parser_cells(bytes)
    } else {
        None
    };
    let pages = doc.pages();
    let total = pages.len() as usize;
    let (first, last) = range.unwrap_or((0, total.saturating_sub(1)));
    for (i, page) in pages.iter().enumerate() {
        if i < first || i > last {
            continue;
        }
        let rc = rust.as_mut().and_then(|v| v.get_mut(i).map(std::mem::take));
        let extracted = extract_page(&page, &ffi, i as i32, rc, render_image, extract_text)?;
        f(i, total, extracted)?;
    }
    Ok(())
}

/// One rasterized page from [`render_pages`] (#243): the absolute 1-based page
/// number in the source document, the pixel dimensions, and the PNG bytes.
#[cfg(feature = "ml")]
#[derive(Debug, Clone)]
pub struct RenderedPage {
    pub page_no: usize,
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
}

#[cfg(feature = "ml")]
/// Rasterize a PDF's pages to PNG (#243) — the lean path behind serve's
/// `to=images`: pdfium render only, no text extraction, no models, and only
/// one page bitmap resident at a time (each is PNG-encoded and dropped before
/// the next renders). `scale` is pixels per PDF point — 2.0 matches the
/// pipeline's [`RENDER_SCALE`] (144 dpi). Unlike the pipeline's render there
/// is no 1.5× supersample + downsample pass: that dance exists only because
/// TableFormer is pixel-pinned to docling's bitmaps, and nothing downstream
/// of this output is — a single render is nearly twice as fast.
///
/// `range` is a **1-based** inclusive page window (issue #80's `pages`
/// semantics: the end clamps to the document, a start past the end errors).
///
/// pdfium is not thread-safe — callers must serialize this against any other
/// pdfium use (docling-serve holds its pipeline mutex around this call for
/// exactly that reason).
pub fn render_pages(
    bytes: &[u8],
    password: Option<&str>,
    range: Option<(usize, usize)>,
    scale: f32,
) -> Result<Vec<RenderedPage>, crate::PdfError> {
    let pdfium = bind()?;
    let doc = pdfium.load_pdf_from_byte_slice(bytes, password)?;
    let pages = doc.pages();
    let total = pages.len() as usize;
    let (first, last) = match range {
        None => (0, total.saturating_sub(1)),
        Some((first, last)) => {
            if first == 0 || last < first {
                return Err(crate::PdfError::Pdfium(format!(
                    "invalid page range {first}-{last} (pages are 1-based, first <= last)"
                )));
            }
            if first > total {
                return Err(crate::PdfError::Pdfium(format!(
                    "page range {first}-{last} is outside the document ({total} page(s))"
                )));
            }
            (first - 1, last.min(total) - 1)
        }
    };
    let mut out = Vec::with_capacity(last.saturating_sub(first) + 1);
    for (i, page) in pages.iter().enumerate() {
        if i < first || i > last {
            continue;
        }
        // pdfium applies /Rotate itself, so the bitmap is the page as a viewer
        // shows it — no orientation handling needed (the pipeline's scanned-page
        // un-rotation is an OCR-conformance concern, not a display one).
        let tw = (page.width().value * scale).round().max(1.0) as i32;
        let th = (page.height().value * scale).round().max(1.0) as i32;
        let cfg = PdfRenderConfig::new()
            .set_target_width(tw)
            .set_target_height(th);
        let bitmap = crate::timing::timed("pdfium.rasterize", || {
            page.render_with_config(&cfg)
                .map(|b| b.as_image().into_rgb8())
        })?;
        let mut png = Vec::new();
        bitmap
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| crate::PdfError::Pdfium(format!("PNG-encoding page {}: {e}", i + 1)))?;
        out.push(RenderedPage {
            page_no: i + 1,
            width: bitmap.width(),
            height: bitmap.height(),
            png,
        });
    }
    Ok(out)
}

#[cfg(feature = "ml")]
fn extract_page(
    page: &pdfium_render::prelude::PdfPage<'_>,
    ffi: &FfiText<'_>,
    index: i32,
    rust_cells: Option<crate::textparse::PageParserCells>,
    render_image: bool,
    extract_text: bool,
) -> Result<PdfPage, PdfiumError> {
    // pdfium reports the page size (and renders) in the *display* frame —
    // `/Rotate` applied — while every text coordinate (its own text page, the
    // pure-Rust parser's MediaBox-based glyphs, link annotation rects) lives
    // in the unrotated frame (docling#4008, 2.121). Keep the unrotated box
    // around for the y-flips and bring every rect into the display frame.
    let width = page.width().value;
    let height = page.height().value;
    let rotation = match page.rotation() {
        Ok(PdfPageRenderRotation::Degrees90) => 90u16,
        Ok(PdfPageRenderRotation::Degrees180) => 180,
        Ok(PdfPageRenderRotation::Degrees270) => 270,
        _ => 0,
    };
    let (unrot_w, unrot_h) = if rotation == 90 || rotation == 270 {
        (height, width)
    } else {
        (width, height)
    };

    // Default: use the pure-Rust text parser instead of pdfium's text layer
    // (override with `DOCLING_PDFIUM_TEXT`). Prose line cells always come from the
    // parser; word and code cells do too unless `DOCLING_PDFIUM_WORDS` keeps them
    // on pdfium (the parser's word grouping reproduces docling-parse's, which
    // TableFormer matches against — roadmap item 6). A page the parser couldn't
    // read (no text layer) keeps pdfium's cells.
    let rc = rust_cells.unwrap_or_default();
    let need_pdfium_prose = extract_text && rc.prose.is_empty();
    let need_pdfium_words = extract_text && (!use_parser_words() || rc.words.is_empty());
    let need_pdfium_code = extract_text && (!use_parser_code() || rc.code.is_empty());

    // The parser covers prose/words/code from one shared glyph pass, so on the
    // common (parser-succeeded) page all three are already satisfied and this
    // pdfium FFI call — otherwise fully discarded below — is skipped outright.
    let (mut cells, mut code_cells, mut word_cells) =
        if need_pdfium_prose || need_pdfium_words || need_pdfium_code {
            let (mut cells, code_cells, word_cells) =
                crate::timing::timed("ffi.page_cells", || ffi.page_cells(index, unrot_h));
            if cells.is_empty() {
                cells = segment_cells(&page.text()?, unrot_h);
            }
            (cells, code_cells, word_cells)
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
    if !rc.prose.is_empty() {
        cells = rc.prose;
    }
    if use_parser_words() && !rc.words.is_empty() {
        word_cells = rc.words;
    }
    if use_parser_code() && !rc.code.is_empty() {
        code_cells = rc.code;
    }
    if rotation != 0 {
        for c in cells
            .iter_mut()
            .chain(word_cells.iter_mut())
            .chain(code_cells.iter_mut())
        {
            let (l, t, r, b) = to_display_frame((c.l, c.t, c.r, c.b), rotation, unrot_w, unrot_h);
            (c.l, c.t, c.r, c.b) = (l, t, r, b);
        }
    }

    let image = if render_image {
        // docling renders at 1.5× the target scale and downsamples "to make it
        // sharper" (pypdfium2 → PIL BICUBIC). Replicate exactly: the TableFormer
        // model is pixel-sensitive, so the page bitmap must match byte-for-byte.
        // `CatmullRom` is the same a=-0.5 cubic kernel as PIL's BICUBIC.
        const SUPERSAMPLE: f32 = 1.5;
        let tw = (width * RENDER_SCALE * SUPERSAMPLE).round().max(1.0) as i32;
        let th = (height * RENDER_SCALE * SUPERSAMPLE).round().max(1.0) as i32;
        let cfg = PdfRenderConfig::new()
            .set_target_width(tw)
            .set_target_height(th);
        let big = crate::timing::timed("pdfium.render", || {
            page.render_with_config(&cfg)
                .map(|b| b.as_image().into_rgb8())
        })?;
        let dw = (width * RENDER_SCALE).round().max(1.0) as u32;
        let dh = (height * RENDER_SCALE).round().max(1.0) as u32;
        crate::timing::timed("image.resize", || fast_downscale(&big, dw, dh))
    } else {
        RgbImage::new(1, 1)
    };
    // The layout model's input image, built exactly like docling's
    // `get_page_image(scale=1.0)`: a pdfium render at 1.5× (pypdfium2 sizes
    // with `ceil`), PIL-BICUBIC down to the point-size image (PIL `resize`'s
    // default kernel; Python `round` = ties-to-even). Distinct from the 2×
    // bitmap above — resampling 1224→640 and 612→640 are different regimes,
    // and the heron model's borderline scores follow the pixels.
    let image_layout = if render_image {
        let tw = f64::from(width * 1.5).ceil().max(1.0) as i32;
        let th = f64::from(height * 1.5).ceil().max(1.0) as i32;
        let cfg = PdfRenderConfig::new()
            .set_target_width(tw)
            .set_target_height(th);
        let big = crate::timing::timed("pdfium.render_layout", || {
            page.render_with_config(&cfg)
                .map(|b| b.as_image().into_rgb8())
        })?;
        let dw = f64::from(width).round_ties_even().max(1.0) as u32;
        let dh = f64::from(height).round_ties_even().max(1.0) as u32;
        Some(crate::timing::timed("image.resize_layout", || {
            crate::resample::pil_resize(&big, dw, dh, crate::resample::PilFilter::Bicubic)
        }))
    } else {
        None
    };

    let mut links = extract_links(page, unrot_h);
    if rotation != 0 {
        for l in &mut links {
            let (a, t, r, b) = to_display_frame((l.l, l.t, l.r, l.b), rotation, unrot_w, unrot_h);
            (l.l, l.t, l.r, l.b) = (a, t, r, b);
        }
    }

    // `/Rotate` normalization for scanned pages: pdfium renders the page as a
    // viewer displays it — `/Rotate` applied — so a rotated scan hands layout
    // and OCR a sideways/upside-down raster and the recognition output is
    // garbage. A page with a text layer needs none of this (its cells carry
    // the geometry; the models never see its pixels decide text), so the
    // normalization is gated to pages with no cells at all — exactly the set
    // the OCR path fires on. The bitmaps are un-rotated to upright (lossless
    // 90° steps), `width`/`height` swap to the upright box, and the display
    // rotation is recorded so assembly can rotate the finished geometry back
    // into display space (docling reports rotated pages in display coords).
    let scanned = cells.is_empty() && word_cells.is_empty() && code_cells.is_empty();
    let mut page = PdfPage {
        width,
        height,
        scale: RENDER_SCALE,
        image_layout,
        cells,
        code_cells,
        word_cells,
        image,
        links,
        rotation: 0,
    };
    if rotation != 0 && scanned && render_image {
        page.unrotate(rotation);
    }
    Ok(page)
}

#[cfg(feature = "ml")]
/// The supersample→target downscale via `fast_image_resize` (SIMD convolution;
/// the same a=-0.5 Catmull-Rom kernel as `image::imageops::resize(...,
/// CatmullRom)` and PIL BICUBIC — see the render comment above). Set
/// `DOCLING_RS_SLOW_RESIZE=1` to fall back to the `image`-crate scalar resize
/// (byte-parity with the pre-SIMD pipeline, several times slower).
fn fast_downscale(big: &RgbImage, dw: u32, dh: u32) -> RgbImage {
    use fast_image_resize as fir;
    static SLOW: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let slow = *SLOW.get_or_init(|| docling_core::env::flag("DOCLING_RS_SLOW_RESIZE"));
    if !slow {
        if let Some(out) = (|| {
            let src = fir::images::ImageRef::new(
                big.width(),
                big.height(),
                big.as_raw(),
                fir::PixelType::U8x3,
            )
            .ok()?;
            let mut dst = fir::images::Image::new(dw, dh, fir::PixelType::U8x3);
            fir::Resizer::new()
                .resize(
                    &src,
                    &mut dst,
                    &fir::ResizeOptions::new()
                        .resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::CatmullRom)),
                )
                .ok()?;
            RgbImage::from_raw(dw, dh, dst.into_vec())
        })() {
            return out;
        }
        // Unreachable in practice; fall through to the scalar path on any error.
    }
    image::imageops::resize(big, dw, dh, image::imageops::FilterType::CatmullRom)
}

#[cfg(feature = "ml")]
/// Collect web/mail/tel hyperlink annotations on a page, mapping each link's
/// rectangle into top-left page coordinates (like [`TextCell`]). `file://` and
/// in-document destinations are skipped — only externally meaningful targets are
/// rendered. pdfium occasionally lists a link twice; rects are kept as-is and the
/// caller dedupes by resolved anchor text.
fn extract_links(page: &pdfium_render::prelude::PdfPage<'_>, page_h: f32) -> Vec<LinkAnnot> {
    let mut out = Vec::new();
    for link in page.links().iter() {
        let Some(uri) = link
            .action()
            .and_then(|a| a.as_uri_action().and_then(|u| u.uri().ok()))
        else {
            continue;
        };
        let scheme_ok = ["http://", "https://", "mailto:", "tel:"]
            .iter()
            .any(|s| uri.starts_with(s));
        if !scheme_ok {
            continue;
        }
        if let Ok(rect) = link.rect() {
            out.push(LinkAnnot {
                l: rect.left().value,
                t: page_h - rect.top().value,
                r: rect.right().value,
                b: page_h - rect.bottom().value,
                uri,
            });
        }
    }
    out
}

/// Map a top-left-origin rect from a page's unrotated (MediaBox) frame into its
/// `/Rotate`d display frame — the counterpart of docling's pypdfium2
/// `_rect_to_display_frame` (docling#4008) for our y-down coordinates.
/// `unrot_w`/`unrot_h` are the unrotated page box; the display box is the same
/// for 180° and swapped for 90°/270°.
pub(crate) fn to_display_frame(
    (l, t, r, b): (f32, f32, f32, f32),
    rotation: u16,
    unrot_w: f32,
    unrot_h: f32,
) -> (f32, f32, f32, f32) {
    match rotation {
        // Page turned 90° clockwise for display: the unrotated top edge becomes
        // the display right edge, so x' runs from the old bottom edge up.
        90 => (unrot_h - b, l, unrot_h - t, r),
        180 => (unrot_w - r, unrot_h - b, unrot_w - l, unrot_h - t),
        270 => (t, unrot_w - r, b, unrot_w - l),
        _ => (l, t, r, b),
    }
}

#[cfg(feature = "ml")]
/// Fallback line cells from pdfium-render's style segments (one cell per
/// segment). Used only when the raw-FFI text page can't be loaded.
fn segment_cells(text: &PdfPageText, page_h: f32) -> Vec<TextCell> {
    text.segments()
        .iter()
        .filter_map(|seg| {
            let s = seg.text();
            if s.trim().is_empty() {
                return None;
            }
            let r = seg.bounds();
            Some(TextCell {
                text: s,
                l: r.left().value,
                t: page_h - r.top().value,
                r: r.right().value,
                b: page_h - r.bottom().value,
            })
        })
        .collect()
}

#[cfg(feature = "ml")]
/// A second, raw-FFI handle on the same PDF used to drive the character loop
/// (`FPDFText_GetUnicode`/`GetCharBox`) that pdfium-render's safe API doesn't
/// expose. Closes the document on drop.
struct FfiText<'a> {
    bindings: &'a dyn PdfiumLibraryBindings,
    doc: FPDF_DOCUMENT,
}

/// One glyph: codepoint + native (y-up) box edges. `l/b/r/t` is pdfium's *tight*
/// ink box (used by the legacy `lines_from_glyphs`); `ll/lb/lr/lt` is the *loose*
/// box (font ascent/descent + advance — uniform per font/size), which the
/// docling-parse-style sanitizer needs so adjacent glyphs share a top edge.
pub(crate) struct Glyph {
    pub(crate) ch: char,
    pub(crate) l: f32,
    pub(crate) b: f32,
    pub(crate) r: f32,
    pub(crate) t: f32,
    pub(crate) ll: f32,
    pub(crate) lb: f32,
    pub(crate) lr: f32,
    pub(crate) lt: f32,
    /// Hash of the PDF font name + flags (0 when not fetched). The sanitizer uses
    /// it for docling-parse's `enforce_same_font` (keeps a bold label and regular
    /// value as separate line cells, e.g. `LABEL : value`).
    pub(crate) font: u64,
}

#[cfg(feature = "ml")]
impl<'a> FfiText<'a> {
    fn load(bindings: &'a dyn PdfiumLibraryBindings, bytes: &[u8], password: Option<&str>) -> Self {
        let doc = bindings.FPDF_LoadMemDocument(bytes, password);
        FfiText { bindings, doc }
    }

    /// Reconstruct line cells for page `index` (zero-based) via the
    /// chars→words→lines grouping. Returns `(prose_cells, code_cells)` — the same
    /// glyphs grouped two ways (gap-heuristic for prose, space-glyph-only for
    /// code). Both empty on any failure (caller falls back).
    fn page_cells(&self, index: i32, page_h: f32) -> (Vec<TextCell>, Vec<TextCell>, Vec<TextCell>) {
        let empty = || (Vec::new(), Vec::new(), Vec::new());
        if self.doc.is_null() {
            return empty();
        }
        let b = self.bindings;
        let page = b.FPDF_LoadPage(self.doc, index);
        if page.is_null() {
            return empty();
        }
        let tp = b.FPDFText_LoadPage(page);
        let out = if tp.is_null() {
            empty()
        } else {
            let dp = use_dp_lines();
            let g = glyphs(b, tp, dp);
            b.FPDFText_ClosePage(tp);
            // Prose line cells: the docling-parse-style sanitizer (behind a flag
            // while it's validated) or the legacy gap-heuristic reconstruction.
            let prose = if dp {
                crate::dp_lines::line_cells(&g, page_h, false)
            } else {
                lines_from_glyphs(&g, page_h, Grouping::Prose)
            };
            (
                prose,
                lines_from_glyphs(&g, page_h, Grouping::CodeSpaceOnly),
                words_from_glyphs(&g, page_h),
            )
        };
        b.FPDF_ClosePage(page);
        out
    }
}

#[cfg(feature = "ml")]
impl Drop for FfiText<'_> {
    fn drop(&mut self) {
        if !self.doc.is_null() {
            self.bindings.FPDF_CloseDocument(self.doc);
        }
    }
}

#[cfg(feature = "ml")]
/// Read every glyph (codepoint + native box) from the text page, in document
/// order. A space glyph is kept as a word-boundary marker (NaN box, char `' '`);
/// pdfium emits these on most lines and they pin word splits exactly. Hard line
/// breaks are dropped (line structure comes from geometry); the gap heuristic in
/// [`lines_from_glyphs`] is the fallback for the lines pdfium leaves space-less.
/// Debug helper: the raw pdfium glyph stream (codepoint + native bottom-left
/// box) for a page, in pdfium's character order. For comparing against
/// docling-parse's char cells.
pub fn debug_glyphs(bytes: &[u8], index: i32) -> Vec<(char, f32, f32)> {
    let Ok(pdfium) = bind() else {
        return Vec::new();
    };
    let ffi = FfiText::load(pdfium.bindings(), bytes, None);
    if ffi.doc.is_null() {
        return Vec::new();
    }
    let b = ffi.bindings;
    let page = b.FPDF_LoadPage(ffi.doc, index);
    if page.is_null() {
        return Vec::new();
    }
    let tp = b.FPDFText_LoadPage(page);
    let mut out = Vec::new();
    if !tp.is_null() {
        for g in glyphs(b, tp, true) {
            out.push((g.ch, g.ll, g.lr));
        }
        b.FPDFText_ClosePage(tp);
    }
    b.FPDF_ClosePage(page);
    out
}

#[cfg(feature = "ml")]
/// One text object on a page, for the hidden-layer diagnostic.
#[derive(Debug, Clone)]
pub struct DebugTextObject {
    /// True when the object is drawn invisibly (text render mode 3) — the marker of
    /// a hidden duplicate text layer.
    pub invisible: bool,
    /// Bounding box in native PDF points (bottom-left origin).
    pub l: f32,
    pub b: f32,
    pub r: f32,
    pub t: f32,
    /// The object's text (best-effort; empty if it could not be read).
    pub text: String,
}

#[cfg(feature = "ml")]
/// Diagnostic: every text object on page `index`, each tagged visible/invisible
/// (via the object-level [`FPDFTextObj_GetTextRenderMode`], which — unlike the
/// per-character render-mode API — is available on the default pdfium binding).
/// A hidden duplicate text layer shows up as invisible objects repeating the
/// visible text. Used by the `dump_render_modes` example.
///
/// [`FPDFTextObj_GetTextRenderMode`]: pdfium_render::prelude::PdfiumLibraryBindings::FPDFTextObj_GetTextRenderMode
pub fn debug_text_objects(bytes: &[u8], index: i32) -> Vec<DebugTextObject> {
    let Ok(pdfium) = bind() else {
        return Vec::new();
    };
    let ffi = FfiText::load(pdfium.bindings(), bytes, None);
    if ffi.doc.is_null() {
        return Vec::new();
    }
    let b = ffi.bindings;
    let page = b.FPDF_LoadPage(ffi.doc, index);
    if page.is_null() {
        return Vec::new();
    }
    let tp = b.FPDFText_LoadPage(page);
    let mut out = Vec::new();
    let n = b.FPDFPage_CountObjects(page);
    for i in 0..n {
        let obj = b.FPDFPage_GetObject(page, i);
        if obj.is_null() || b.FPDFPageObj_GetType(obj) != FPDF_PAGEOBJ_TEXT as i32 {
            continue;
        }
        let (mut l, mut bot, mut r, mut top) = (0f32, 0f32, 0f32, 0f32);
        if b.FPDFPageObj_GetBounds(obj, &mut l, &mut bot, &mut r, &mut top) == 0 {
            continue;
        }
        let invisible = b.FPDFTextObj_GetTextRenderMode(obj) == INVISIBLE_RENDER_MODE;
        let text = if tp.is_null() {
            String::new()
        } else {
            // FPDFTextObj_GetText returns the count of UTF-16 code units, including
            // the trailing NUL; call once for the size, once to fill.
            let need = b.FPDFTextObj_GetText(obj, tp, std::ptr::null_mut(), 0);
            if need <= 1 {
                String::new()
            } else {
                let mut buf = vec![0u16; need as usize];
                b.FPDFTextObj_GetText(obj, tp, buf.as_mut_ptr(), need);
                if let Some(&0) = buf.last() {
                    buf.pop();
                }
                String::from_utf16_lossy(&buf)
            }
        };
        out.push(DebugTextObject {
            invisible,
            l,
            b: bot,
            r,
            t: top,
            text,
        });
    }
    if !tp.is_null() {
        b.FPDFText_ClosePage(tp);
    }
    b.FPDF_ClosePage(page);
    out
}

#[cfg(feature = "ml")]
/// Hash a glyph's PDF font name + flags, for `enforce_same_font`. 0 if unavailable.
fn font_hash(b: &dyn PdfiumLibraryBindings, tp: FPDF_TEXTPAGE, i: i32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut flags: std::os::raw::c_int = 0;
    let len = b.FPDFText_GetFontInfo(tp, i, std::ptr::null_mut(), 0, &mut flags);
    if len == 0 {
        return 0;
    }
    let mut buf = vec![0u8; len as usize];
    b.FPDFText_GetFontInfo(
        tp,
        i,
        buf.as_mut_ptr() as *mut std::os::raw::c_void,
        len,
        &mut flags,
    );
    let mut h = std::collections::hash_map::DefaultHasher::new();
    buf.hash(&mut h);
    flags.hash(&mut h);
    h.finish()
}

#[cfg(feature = "ml")]
/// A glyph's PDF font name (NUL-trimmed), or empty if unavailable.
fn font_name_bytes(b: &dyn PdfiumLibraryBindings, tp: FPDF_TEXTPAGE, i: i32) -> Vec<u8> {
    let mut flags: std::os::raw::c_int = 0;
    let len = b.FPDFText_GetFontInfo(tp, i, std::ptr::null_mut(), 0, &mut flags);
    if len == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; len as usize];
    b.FPDFText_GetFontInfo(
        tp,
        i,
        buf.as_mut_ptr() as *mut std::os::raw::c_void,
        len,
        &mut flags,
    );
    while buf.last() == Some(&0) {
        buf.pop();
    }
    buf
}

#[cfg(feature = "ml")]
/// Read the text layer's glyph boxes and font styles for the given **1-based**
/// pages — the heading-hierarchy stage's style signal (#302). A separate,
/// on-demand pass over the text pages (no rendering), so the extraction
/// pipeline itself stays byte-identical whether or not the stage runs; pages
/// without a text layer (scans) simply yield no glyphs and the stage falls
/// back to its other signals. Boxes are the *loose* char boxes (font ascent +
/// descent — the font-size proxy), converted to top-left origin.
pub(crate) fn glyph_styles(
    bytes: &[u8],
    password: Option<&str>,
    pages: &[usize],
) -> std::collections::HashMap<usize, Vec<crate::heading_hierarchy::GlyphStyle>> {
    use crate::heading_hierarchy::GlyphStyle;
    let mut out = std::collections::HashMap::new();
    let Ok(pdfium) = bind() else {
        return out;
    };
    let ffi = FfiText::load(pdfium.bindings(), bytes, password);
    if ffi.doc.is_null() {
        return out;
    }
    let b = ffi.bindings;
    // Each distinct font name parses once per document.
    let mut cache: std::collections::HashMap<Vec<u8>, crate::font_style::FontStyle> =
        std::collections::HashMap::new();
    for &page_no in pages {
        if page_no == 0 {
            continue;
        }
        let page = b.FPDF_LoadPage(ffi.doc, (page_no - 1) as i32);
        if page.is_null() {
            continue;
        }
        let page_h = b.FPDF_GetPageHeightF(page);
        let tp = b.FPDFText_LoadPage(page);
        if !tp.is_null() {
            let n = b.FPDFText_CountChars(tp);
            let mut styles = Vec::with_capacity(n.max(0) as usize);
            for i in 0..n {
                let ch = match char::from_u32(b.FPDFText_GetUnicode(tp, i)) {
                    Some(c) => c,
                    None => continue,
                };
                if ch.is_whitespace() {
                    continue;
                }
                let mut lr = FS_RECTF {
                    left: 0.0,
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                };
                if b.FPDFText_GetLooseCharBox(tp, i, &mut lr) == 0 {
                    continue;
                }
                let name = font_name_bytes(b, tp, i);
                let style = *cache.entry(name).or_insert_with_key(|n| {
                    crate::font_style::parse_font_style(&String::from_utf8_lossy(n))
                });
                styles.push(GlyphStyle {
                    l: lr.left,
                    t: page_h - lr.top,
                    r: lr.right,
                    b: page_h - lr.bottom,
                    height: lr.top - lr.bottom,
                    weight_cls: crate::font_style::weight_class(style.weight),
                    italic: style.italic,
                    styled: style.known,
                });
            }
            b.FPDFText_ClosePage(tp);
            out.insert(page_no, styles);
        }
        b.FPDF_ClosePage(page);
    }
    out
}

#[cfg(feature = "ml")]
/// pdfium text render mode 3: the glyph is drawn with neither fill nor stroke —
/// an invisible glyph. Web-to-PDF exporters put a hidden plain-text copy of
/// syntax-highlighted code (and other "copy"/accessibility layers) in this mode,
/// which the char-level text API then extracts as a duplicate of the visible text.
const INVISIBLE_RENDER_MODE: i32 = 3;

#[cfg(feature = "ml")]
fn glyphs(b: &dyn PdfiumLibraryBindings, tp: FPDF_TEXTPAGE, fetch_font: bool) -> Vec<Glyph> {
    let n = b.FPDFText_CountChars(tp);
    let mut out = Vec::with_capacity(n.max(0) as usize);
    for i in 0..n {
        let ch = match char::from_u32(b.FPDFText_GetUnicode(tp, i)) {
            Some(c) => c,
            None => continue,
        };
        if ch == '\r' || ch == '\n' {
            continue;
        }
        // Spaces are font-neutral (0): pdfium's generated spaces carry a default
        // font that would otherwise block every word↔space merge under
        // enforce_same_font; docling-parse's spaces inherit the run's font.
        let font = if fetch_font && !ch.is_whitespace() {
            font_hash(b, tp, i)
        } else {
            0
        };
        let (mut l, mut r, mut bot, mut top) = (0f64, 0f64, 0f64, 0f64);
        let has_box = b.FPDFText_GetCharBox(tp, i, &mut l, &mut r, &mut bot, &mut top) != 0;
        // Loose box: font ascent/descent + glyph advance, uniform per font/size.
        let mut lr = FS_RECTF {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        };
        let (ll, lb, lrt, ltop) = if b.FPDFText_GetLooseCharBox(tp, i, &mut lr) != 0 {
            (lr.left, lr.bottom, lr.right, lr.top)
        } else if has_box {
            (l as f32, bot as f32, r as f32, top as f32)
        } else {
            (f32::NAN, 0.0, 0.0, 0.0)
        };
        if ch.is_whitespace() {
            // Keep the space *with its box* (the docling-parse-style line sanitizer
            // needs literal space glyphs); NaN `l` if pdfium reports no box (the
            // legacy `lines_from_glyphs` ignores the box and only flags a space).
            out.push(Glyph {
                ch: ' ',
                l: if has_box { l as f32 } else { f32::NAN },
                b: if has_box { bot as f32 } else { 0.0 },
                r: if has_box { r as f32 } else { 0.0 },
                t: if has_box { top as f32 } else { 0.0 },
                ll,
                lb,
                lr: lrt,
                lt: ltop,
                font,
            });
            continue;
        }
        if !has_box {
            continue;
        }
        out.push(Glyph {
            ch,
            l: l as f32,
            b: bot as f32,
            r: r as f32,
            t: top as f32,
            ll,
            lb,
            lr: lrt,
            lt: ltop,
            font,
        });
    }
    // pdfium splits the Arabic lam-alef ligature into two chars at the *same* x
    // (it's one glyph) in visual order — `alef-variant, lam`. docling-parse and
    // logical order are `lam, alef-variant`. Detect the ligature by the shared x
    // and swap. The shared-x test reliably distinguishes a true ligature from a
    // genuine `alef + lam` sequence (the article `ال`, or `فعالة`), whose two
    // glyphs sit at different x and must NOT be reordered.
    for i in 0..out.len().saturating_sub(1) {
        let same_x = out[i].l.is_finite()
            && out[i + 1].l.is_finite()
            && (out[i].l - out[i + 1].l).abs() < 1.0;
        if same_x
            && matches!(out[i].ch, '\u{0622}' | '\u{0623}' | '\u{0625}' | '\u{0627}')
            && out[i + 1].ch == '\u{0644}'
        {
            out.swap(i, i + 1);
        }
    }
    // Reconstruct degenerate (zero-width) loose space boxes by spanning the gap to
    // the next glyph on the same line, so the sanitizer keeps them as word
    // separators rather than dropping them (which would merge `Information systems`
    // → `Informationsystems`). pdfium gives generated spaces a zero-width box at a
    // wrong baseline; a wrap (different baseline) or a touching gap is left alone.
    for i in 0..out.len() {
        if out[i].ch != ' ' || (out[i].lr - out[i].ll).abs() >= 0.5 {
            continue;
        }
        let prev = out[..i]
            .iter()
            .rev()
            .find(|g| g.ch != ' ' && g.ll.is_finite())
            .map(|g| (g.lr, g.lb, g.lt));
        let next = out[i + 1..]
            .iter()
            .find(|g| g.ch != ' ' && g.ll.is_finite())
            .map(|g| (g.ll, g.lb));
        if let (Some((plr, plb, plt)), Some((nll, nlb))) = (prev, next) {
            let line_h = (plt - plb).abs().max(1.0);
            if (plb - nlb).abs() < line_h * 0.5 && nll > plr + 0.5 {
                out[i].ll = plr;
                out[i].lr = nll;
                out[i].lb = plb;
                out[i].lt = plt;
            }
        }
    }
    out
}

/// How [`lines_from_glyphs`] splits a line into words.
#[derive(Clone, Copy, PartialEq)]
enum Grouping {
    /// Gap heuristic + punctuation glue (`engines,`, `[37`, `98.5`) — prose.
    Prose,
    /// Split only at literal space glyphs, never glue — pdfium code cells.
    /// pdfium's monospace listings carry a real space glyph at every source space,
    /// and its overhanging loose boxes would make the gap heuristic over-split
    /// (`f un c t i o n`), so honouring just the spaces reproduces the spacing.
    CodeSpaceOnly,
    /// Split on the inter-glyph **gap** (or a space glyph), but never glue — for
    /// the parser's code cells: the parser emits no space glyphs (a source space
    /// is a positioning gap), and its clean advance boxes make the gap reliable.
    /// Unlike [`Grouping::Prose`] there is no punctuation glue, so a real gap
    /// always splits (`et al. 2000`, not `et al.2000`) while genuinely touching
    /// tokens stay joined (`add(a,` / `b)`).
    CodeGap,
}

/// Group glyphs (document order) into words then lines, the way docling-parse
/// does: a new **word** starts where the horizontal gap to the previous glyph
/// exceeds ~0.2 × the font height (a real space is ~0.3 × height; letter
/// tracking is smaller, so titles don't shatter); a new **line** starts where
/// the baseline drops by ~half the font height (a superscript rises without
/// dropping, so it stays on its line). Coordinates are flipped to top-left.
/// See [`Grouping`] for how each mode decides word boundaries.
fn lines_from_glyphs(gs: &[Glyph], page_h: f32, mode: Grouping) -> Vec<TextCell> {
    let mut cells: Vec<TextCell> = Vec::new();
    let mut words: Vec<String> = Vec::new(); // words on the current line
    let mut word = String::new();
    // current line bounding box, native
    let (mut ll, mut lb, mut lr, mut lt) = (
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    );
    // Tallest glyph seen on the current line: the word-gap threshold is relative
    // to it, so a small-font run on the line (a superscript citation) isn't split
    // at its tight digit gaps, while a big display title isn't split at its wider
    // letter tracking. A real inter-word space is ~0.3× the font height.
    let mut line_h: f32 = 0.0;
    let mut prev: Option<&Glyph> = None;
    // A space glyph between non-space glyphs pins a word split the gap heuristic
    // can miss (tight justified spacing); it carries no geometry.
    let mut pending_space = false;

    for g in gs {
        if g.ch == ' ' {
            pending_space = true;
            continue;
        }
        let h = (g.t - g.b).abs().max(1.0);
        let (mut new_word, mut new_line) = (false, false);
        if let Some(p) = prev {
            // A new line drops the baseline *and* resets x leftward; requiring the
            // x-reset avoids a descending comma/semicolon faking a line break. A
            // *large* drop (≥1.5× the line height — a skipped line, e.g. a centered
            // page-number footer below a short last word) is always a new line,
            // even without the x-reset.
            // LTR wraps reset x leftward (`g.l < p.r`); RTL (Arabic) wraps reset
            // rightward (the new line begins at the far right). A large drop
            // (≥1.5× line height) is a new line regardless of x.
            let x_reset = if is_arabic(g.ch) || is_arabic(p.ch) {
                g.l > p.r
            } else {
                g.l < p.r
            };
            new_line = (p.b - g.b > h * 0.5 && x_reset) || (p.b - g.b > line_h.max(h) * 1.5);
            // Don't split before closing punctuation, after opening punctuation, or
            // after a period that runs into a digit/lowercase letter — docling
            // keeps `engines,` / `[37` / `i.e.` / `98.5` together even across a
            // space or gap.
            let glued = is_close_punct(g.ch)
                || is_open_punct(p.ch)
                || (p.ch.is_ascii_digit() && g.ch.is_ascii_digit())
                || (p.ch == '.'
                    && !pending_space
                    && (g.ch.is_ascii_digit() || g.ch.is_ascii_lowercase()));
            let word_gap = line_h.max(h) * 0.25;
            new_word = if mode == Grouping::CodeSpaceOnly {
                new_line || pending_space
            } else if mode == Grouping::CodeGap {
                // Gap-based, no glue: a real gap always splits, touching tokens join.
                new_line || pending_space || g.l - p.r > word_gap
            } else if is_arabic(g.ch) || is_arabic(p.ch) {
                // RTL runs right-to-left, so the inter-word gap is `p.l - g.r`. A
                // real word space has a gap; pdfium also emits spurious zero-gap
                // space glyphs inside words (`التي`), so require the gap rather
                // than trusting a bare space glyph.
                new_line || (p.l - g.r > word_gap && !glued)
            } else {
                new_line || ((pending_space || g.l - p.r > word_gap) && !glued)
            };
        }
        pending_space = false;
        if new_line {
            push_word(&mut word, &mut words);
            push_line(&mut words, (ll, lb, lr, lt), page_h, &mut cells);
            (ll, lb, lr, lt) = (
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            );
            line_h = 0.0;
        } else if new_word {
            push_word(&mut word, &mut words);
        }
        word.push(g.ch);
        ll = ll.min(g.l);
        lb = lb.min(g.b);
        lr = lr.max(g.r);
        lt = lt.max(g.t);
        line_h = line_h.max(h);
        prev = Some(g);
    }
    push_word(&mut word, &mut words);
    push_line(&mut words, (ll, lb, lr, lt), page_h, &mut cells);
    cells
}

/// Code line cells from the **parser**'s glyph stream. Unlike pdfium — whose
/// monospace listings carry explicit space glyphs (so [`Grouping::CodeSpaceOnly`]
/// keeps their spacing) — the parser emits no space glyphs: a source space is a
/// positioning gap. So code cells use [`Grouping::CodeGap`], which splits on the
/// inter-glyph gap (a space wherever it exceeds ~0.25× the line height) but never
/// glues punctuation, so `et al. 2000` keeps its space while `add(a,` / `b)` stay
/// joined. The parser's clean advance boxes make the gap heuristic reliable here,
/// where pdfium's overhanging loose boxes would over-split (`f un c t i o n`).
pub(crate) fn code_cells_from_glyphs(gs: &[Glyph], page_h: f32) -> Vec<TextCell> {
    lines_from_glyphs(gs, page_h, Grouping::CodeGap)
}

/// Per-word cells (each word's text + top-left bbox), using the same word/line
/// splitting as [`lines_from_glyphs`] but emitting one cell per word instead of
/// joining into lines — the legacy gap-heuristic word grouping, kept for the
/// pdfium word path (`DOCLING_PDFIUM_WORDS`). The default parser path uses
/// [`crate::dp_lines::word_cells`] instead.
pub(crate) fn words_from_glyphs(gs: &[Glyph], page_h: f32) -> Vec<TextCell> {
    let mut cells = Vec::new();
    let mut word = String::new();
    let inf = (
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    );
    let (mut wl, mut wb, mut wr, mut wt) = inf;
    let mut line_h: f32 = 0.0;
    let mut prev: Option<&Glyph> = None;
    let mut pending_space = false;
    for g in gs {
        if g.ch == ' ' {
            pending_space = true;
            continue;
        }
        let h = (g.t - g.b).abs().max(1.0);
        let mut new_line = false;
        let mut new_word = false;
        if let Some(p) = prev {
            // LTR wraps reset x leftward (`g.l < p.r`); RTL (Arabic) wraps reset
            // rightward (the new line begins at the far right). A large drop
            // (≥1.5× line height) is a new line regardless of x.
            let x_reset = if is_arabic(g.ch) || is_arabic(p.ch) {
                g.l > p.r
            } else {
                g.l < p.r
            };
            new_line = (p.b - g.b > h * 0.5 && x_reset) || (p.b - g.b > line_h.max(h) * 1.5);
            // No digit-digit glue here (unlike the prose grouping): table cells in
            // adjacent columns are numeric and a column gap must still split them
            // (`0.965` `0.934`, not `0.9650.934`). Intra-number digits have no gap
            // so they stay together regardless.
            let glued = is_close_punct(g.ch)
                || is_open_punct(p.ch)
                || (p.ch == '.'
                    && !pending_space
                    && (g.ch.is_ascii_digit() || g.ch.is_ascii_lowercase()));
            let word_gap = line_h.max(h) * 0.25;
            new_word = new_line || ((pending_space || g.l - p.r > word_gap) && !glued);
        }
        pending_space = false;
        if new_word && !word.is_empty() {
            cells.push(TextCell {
                text: std::mem::take(&mut word),
                l: wl,
                t: page_h - wt,
                r: wr,
                b: page_h - wb,
            });
            (wl, wb, wr, wt) = inf;
        }
        if new_line {
            line_h = 0.0;
        }
        word.push(g.ch);
        wl = wl.min(g.l);
        wb = wb.min(g.b);
        wr = wr.max(g.r);
        wt = wt.max(g.t);
        line_h = line_h.max(h);
        prev = Some(g);
    }
    if !word.is_empty() {
        cells.push(TextCell {
            text: word,
            l: wl,
            t: page_h - wt,
            r: wr,
            b: page_h - wb,
        });
    }
    cells
}

fn is_arabic(c: char) -> bool {
    ('\u{0600}'..='\u{06FF}').contains(&c)
}

fn is_close_punct(c: char) -> bool {
    matches!(
        c,
        ',' | '.' | ';' | '!' | '?' | ')' | ']' | '}' | '%' | '\'' | '\u{2019}' | '\u{2018}'
    )
}

fn is_open_punct(c: char) -> bool {
    // `@` glues to what follows (`mAP @0.5`, `bpf@zurich`, `@decorator`).
    matches!(c, '(' | '[' | '{' | '@')
}

fn push_word(word: &mut String, words: &mut Vec<String>) {
    if !word.is_empty() {
        words.push(std::mem::take(word));
    }
}

fn push_line(
    words: &mut Vec<String>,
    bbox: (f32, f32, f32, f32),
    page_h: f32,
    cells: &mut Vec<TextCell>,
) {
    if words.is_empty() {
        return;
    }
    let text = std::mem::take(words).join(" ");
    let (l, b, r, t) = bbox;
    cells.push(TextCell {
        text,
        l,
        t: page_h - t,
        r,
        b: page_h - b,
    });
}

#[cfg(test)]
mod tests {
    use super::to_display_frame;

    /// A 612×792 portrait page displayed under `/Rotate`: a rect near the
    /// unrotated top-left lands where a viewer shows it (docling#4008).
    #[test]
    fn display_frame_follows_the_page_rotation() {
        let r = (72.0, 63.0, 387.0, 74.0); // top-left origin, unrotated
        assert_eq!(to_display_frame(r, 0, 612.0, 792.0), r);
        // 90° clockwise: the page becomes 792×612; the old top edge is the
        // display right edge, old left edge the display top.
        assert_eq!(
            to_display_frame(r, 90, 612.0, 792.0),
            (718.0, 72.0, 729.0, 387.0)
        );
        // 180°: both axes mirror inside the same box.
        assert_eq!(
            to_display_frame(r, 180, 612.0, 792.0),
            (225.0, 718.0, 540.0, 729.0)
        );
        // 270°: the old top edge is the display left edge, old right edge the
        // display top.
        assert_eq!(
            to_display_frame(r, 270, 612.0, 792.0),
            (63.0, 225.0, 74.0, 540.0)
        );
    }

    #[test]
    fn display_frame_rotations_compose_to_identity() {
        let r = (10.0, 20.0, 110.0, 40.0);
        // 90° then 270° from the intermediate (792×612) box round-trips.
        let once = to_display_frame(r, 90, 612.0, 792.0);
        assert_eq!(to_display_frame(once, 270, 792.0, 612.0), r);
        let twice = to_display_frame(to_display_frame(r, 180, 612.0, 792.0), 180, 612.0, 792.0);
        assert_eq!(twice, r);
    }
}
