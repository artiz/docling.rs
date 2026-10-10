//! The top-level `DocumentConverter`.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::backend::{
    is_deepseek_markdown, AbwBackend, AsciiDocBackend, CsvBackend, DeclarativeBackend,
    DeepSeekBackend, DocBackend, DoclingJsonBackend, DocxBackend, EbcdicBackend, EmailBackend,
    EpubBackend, InterchangeBackend, JatsBackend, LatexBackend, LotusBackend, MarkdownBackend,
    MhtmlBackend, PptBackend, PptxBackend, QuattroBackend, RtfBackend, StarOffice5Backend,
    UsptoBackend, VisioBackend, WebVttBackend, WpdBackend, WpsBackend, XlsBackend, XlsxBackend,
};

/// Drop the embedded image bytes from every picture — the flat nodes and the
/// item tree alike — for [`DocumentConverter::keep_picture_images`]`(false)`.
/// The picture items stay (caption, OCR text, classification, geometry);
/// only their payload goes, so the JSON writes no `image` and Markdown's
/// embedded mode prints the placeholder.
fn strip_picture_images(document: &mut docling_core::DoclingDocument) {
    document.for_each_picture_mut(&mut |node| {
        if let docling_core::Node::Picture { image, .. } = node {
            *image = None;
        }
    });
    if let Some(tree) = document.tree.as_mut() {
        for item in tree.items.iter_mut() {
            if let docling_core::tree::TreeKind::Picture { image, .. } = &mut item.kind {
                *image = None;
            }
        }
    }
}

/// The picture-OCR reader (#645): the OCR models behind the size floor and
/// the classifier filter, with a by-bytes cache so the same image embedded
/// twice is read once. Built by [`DocumentConverter::picture_reader`].
#[cfg(feature = "pdf")]
struct PictureReader {
    pipeline: docling_pdf::Pipeline,
    min_side: u32,
    classes: Vec<String>,
    cache: std::collections::HashMap<u64, Option<docling_core::PictureDescription>>,
}

#[cfg(feature = "pdf")]
impl PictureReader {
    /// The text read off `img`, as the picture's description annotation;
    /// `None` when the picture is skipped (too small, filtered out by its
    /// class) or carries no text.
    fn read(
        &mut self,
        img: &docling_core::PictureImage,
    ) -> Option<docling_core::PictureDescription> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        img.data.hash(&mut h);
        let key = h.finish();
        if let Some(hit) = self.cache.get(&key) {
            return hit.clone();
        }
        let text = self.read_uncached(img);
        self.cache.insert(key, text.clone());
        text
    }

    fn read_uncached(
        &mut self,
        img: &docling_core::PictureImage,
    ) -> Option<docling_core::PictureDescription> {
        // The declared size is a hint; the decoded one is the truth
        // (a 16 × 16 icon is skipped before any inference).
        if img.width.min(img.height) < self.min_side && img.width * img.height > 0 {
            return None;
        }
        let decoded = match self.pipeline.decode_picture(&img.data) {
            Ok(d) => d,
            Err(e) => {
                docling_core::debug_log!("docling: picture OCR: undecodable image: {e}");
                return None;
            }
        };
        if decoded.width().min(decoded.height()) < self.min_side {
            return None;
        }
        // The loader warned once when the classifier is missing; an
        // unclassifiable picture is read rather than silently skipped.
        if !self.classes.is_empty() {
            if let Some(preds) = self.pipeline.classify_picture(&decoded) {
                let top = preds.first().map(|p| p.class_name.as_str());
                if !top.is_some_and(|t| self.classes.iter().any(|c| c == t)) {
                    return None;
                }
            }
        }
        match self.pipeline.ocr_picture(&decoded) {
            Ok(Some(t)) => Some(docling_core::PictureDescription {
                text: t.text,
                provenance: t.provenance.to_string(),
            }),
            Ok(None) => None,
            Err(e) => {
                eprintln!("warning: picture OCR failed: {e}");
                None
            }
        }
    }
}

/// Without the `pdf` feature there is no reader; the type exists so the
/// callers compile unchanged.
#[cfg(not(feature = "pdf"))]
struct PictureReader;

#[cfg(not(feature = "pdf"))]
impl PictureReader {
    fn read(
        &mut self,
        _img: &docling_core::PictureImage,
    ) -> Option<docling_core::PictureDescription> {
        None
    }
}

/// Whether `text` begins with an XML prolog — an `<?xml …?>` declaration or a
/// non-HTML `<!DOCTYPE …>`. Used to route XML documents that arrived with a
/// text/Markdown extension (e.g. a JATS article saved as `.txt`) to the XML
/// backends. An HTML5 `<!DOCTYPE html>` is deliberately excluded.
fn looks_like_xml(text: &str) -> bool {
    let head = text.trim_start();
    if head.starts_with("<?xml") {
        return true;
    }
    if let Some(rest) = head.get(..9) {
        if rest.eq_ignore_ascii_case("<!doctype") {
            return !head[9..]
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("html");
        }
    }
    false
}

/// Pick the concrete XML backend for a generic `.xml` source by sniffing its
/// DOCTYPE / root element (the first part of the file).
/// docling's `_guess_from_content` JATS rule for an `application/xml` input:
/// the `<!DOCTYPE …>` declaration names a JATS DTD.
fn has_jats_doctype(text: &str) -> bool {
    let Some(start) = text.find("<!DOCTYPE ") else {
        return false;
    };
    let Some(len) = text[start..].find('>') else {
        return false;
    };
    let doctype = &text[start..start + len];
    doctype.contains("JATS-journalpublishing") || doctype.contains("JATS-archive")
}

fn sniff_xml(bytes: &[u8]) -> InputFormat {
    // Lossy over the raw head (docling#4038, 2.122): the window can cut a
    // well-formed file mid-codepoint — a fixed-offset `&str` slice would
    // panic on that char boundary — and an XML document may legitimately
    // declare a non-UTF-8 encoding. Every marker matched below is ASCII, so
    // replacement characters cannot change the outcome.
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4000)]);
    let head = head.as_ref();
    // Case-insensitive: USPTO DOCTYPE/root casing varies in the wild (docling
    // PR #3801 — Grant Full Text v2.5 files were missed on casing).
    let lower = head.to_ascii_lowercase();
    if lower.contains("us-patent")
        || lower.contains("patent-application-publication")
        || lower.contains("patdoc")
        || lower.contains("<pap-v1")
    {
        InputFormat::XmlUspto
    } else if head.contains("<doclang") {
        // A bare DocLang document saved as `.xml` (docling names them
        // `*.dclg.xml`, whose final extension is plain `xml`).
        InputFormat::XmlDoclang
    } else if crate::backend::xbrl::looks_like_xbrl(head) {
        InputFormat::XmlXbrl
    } else {
        InputFormat::XmlJats
    }
}
use crate::error::ConversionError;
use crate::format::InputFormat;
use crate::result::{ConversionResult, ConversionStatus};
use crate::source::SourceDocument;
#[cfg(feature = "pdf")]
use crate::stream::MarkdownStream;
#[cfg(feature = "pdf")]
use docling_core::ImageMode;

/// Routes a [`SourceDocument`] to the backend for its format and returns a
/// [`ConversionResult`].
///
/// The Rust analogue of `docling.document_converter.DocumentConverter`. In
/// Phase 0 the format→backend dispatch is a direct match; the Python notion of
/// per-format `FormatOption` (backend + pipeline + options) arrives with the
/// PDF/ML pipeline in a later phase.
#[derive(Debug, Clone)]
pub struct DocumentConverter {
    allowed_formats: Option<HashSet<InputFormat>>,
    strict: bool,
    fetch_images: bool,
    list_attachments: bool,
    /// Omit empty cells from sparse spreadsheet table grids (#271, XLSX/XLS
    /// family; opt-in docling.rs extension).
    skip_empty_cells: bool,
    /// Emit Markdown tables in the compact `| a | b |` form instead of the
    /// width-padded GitHub serializer (#271, opt-in docling.rs extension).
    compact_tables: bool,
    /// docling-core's `MarkdownParams.page_break_placeholder`: text inserted
    /// between pages in the Markdown export; `None` (default) omits breaks.
    page_break_placeholder: Option<String>,
    /// EBCDIC copybook layout (#252): inline JSON or a file path. `None`
    /// falls back to the `<stem>.layout.json` sidecar.
    ebcdic_layout: Option<String>,
    no_table_former: bool,
    no_text_panels: bool,
    /// The text layer only, no models (#611: `--text-layer-only`, the
    /// pre-2.0 `--no-ocr`).
    text_layer_only: bool,
    /// Skip OCR, keep layout and tables — docling's `do_ocr=False` (#611:
    /// `--no-ocr`, `--skip-ocr`).
    no_ocr: bool,
    /// The password of an encrypted PDF (#611) or Office document (#625).
    password: Option<String>,
    force_full_page_ocr: bool,
    /// OCR mode id (docling's `OcrMode`, #254); parsed at the ML call sites.
    ocr_mode: Option<String>,
    /// OCR engine id (`ppocr` | `tesseract`, #460); parsed at the ML call
    /// sites.
    ocr_engine: Option<String>,
    /// OCR render scale in px/pt (#254); validated at the ML call sites.
    ocr_scale: Option<f32>,
    /// Picture-crop / page-image scale in px/pt (docling's `images_scale`,
    /// #520); `None` = the pipeline's 2.0 render.
    images_scale: Option<f32>,
    /// Keep page renders as the document's page images (docling's
    /// `generate_page_images`, #520).
    generate_page_images: bool,
    /// Infer PDF/image section-header levels after assembly (#302, docling's
    /// `HeadingHierarchyModel`): bookmarks > numbering > font style. Off by
    /// default — heading levels then stay exactly as detected.
    heading_hierarchy: bool,
    use_web_browser: bool,
    /// Named Whisper model preset for audio sources (docling's ASR model
    /// specs, PR #3741): English-only / Distil-Whisper variants under
    /// `.models/asr/<preset>/`. `None` = the default Whisper tiny.
    asr_model: Option<String>,
    asr_lang: Option<String>,
    /// Max sampled frames per video (#138 Phase 2). `None` = the default
    /// ([`DEFAULT_VIDEO_FRAMES`]); `Some(0)` disables frame extraction;
    /// [`ALL_VIDEO_FRAMES`] keeps every distinct cut (#647).
    video_frames: Option<usize>,
    /// ffmpeg's scene score a video frame must exceed to count as a cut
    /// (#647); `None` = `DOCLING_RS_VIDEO_SCENE_THRESHOLD`, else 0.27.
    video_scene_threshold: Option<f32>,
    /// Cap on a sampled frame's longer side in px (#647); `None` =
    /// `DOCLING_RS_VIDEO_FRAME_MAX_SIDE`, else the source resolution.
    video_frame_max_side: Option<u32>,
    /// Hamming distance under which a frame's difference hash makes it a
    /// duplicate of a kept one (#647); `None` = `DOCLING_RS_VIDEO_FRAME_DEDUPE`,
    /// else no de-duplication.
    video_frame_dedupe: Option<u32>,
    /// The directory an XBRL instance's taxonomy is read from (docling's
    /// `XBRLBackendOptions.taxonomy`); `None` = the instance's own directory.
    xbrl_taxonomy: Option<PathBuf>,
    /// Opt-in PDF/image enrichment models (docling's
    /// `do_picture_classification` / `do_code_enrichment` /
    /// `do_formula_enrichment`).
    enrich: crate::EnrichmentOptions,
    /// OCR the embedded pictures of non-PDF documents (#645). See
    /// [`Self::do_picture_ocr`].
    picture_ocr: bool,
    /// DocumentFigureClassifier labels a picture must carry to be OCR'd;
    /// empty = every picture. See [`Self::picture_ocr_classes`].
    picture_ocr_classes: Vec<String>,
    /// Smallest side (px) a picture must have to be OCR'd; `None` = the
    /// environment's default. See [`Self::picture_ocr_min_side`].
    picture_ocr_min_side: Option<u32>,
    /// Keep the embedded image bytes on every picture (the default). See
    /// [`Self::keep_picture_images`].
    keep_picture_images: bool,
    /// The PII redaction pass (#621), when asked for. See [`Self::redact_pii`].
    redact: Option<docling_core::RedactionOptions>,
    /// 1-based inclusive PDF page window (#80). See [`Self::page_range`].
    page_range: Option<(usize, usize)>,
    /// OCR recognition language for scanned PDF/image pages (`en`/`ch`).
    /// `None` = the process default (`DOCLING_RS_OCR_LANG`, else English).
    ocr_lang: Option<String>,
    /// Character encoding for text inputs (docling's
    /// `TextBackendOptions.encoding`); `None` = detect. See [`Self::encoding`].
    encoding: Option<String>,
    /// Directory referenced-mode streaming writes images into (#80).
    /// See [`Self::artifacts_dir`].
    artifacts_dir: String,
    /// Per-document wall-clock budget for the PDF/image pipeline (docling's
    /// `document_timeout`, #497). See [`Self::document_timeout`].
    document_timeout: Option<std::time::Duration>,
    /// Bounds on what a ZIP input may decompress (#557,
    /// [`DocumentConverter::convert_archive`]).
    archive_limits: crate::archive::ArchiveLimits,
}

/// Default cap on sampled frames per video. Scene changes rarely exceed this
/// in short clips, and uniform fallback at 8 keeps JSON/DCLX output (which
/// embeds the PNGs) within sane bounds.
pub const DEFAULT_VIDEO_FRAMES: usize = 8;

/// The `video_frames` count meaning "every distinct cut" (#647): no cap,
/// no even resampling of a cut-less video (its first frame alone), the
/// scene threshold and de-duplication deciding what a distinct frame is.
/// Any count at or above `u32::MAX` reads as this on the JSON surfaces, so
/// a binding whose integers are 32-bit (the npm addon) can ask for it.
pub const ALL_VIDEO_FRAMES: usize = usize::MAX;

/// Parse a user-facing page-range string (issue #80's `--pages`): `"A-B"` for
/// an inclusive 1-based window, or a single `"N"` for one page. Whitespace
/// around the numbers is tolerated. Validation against the actual page count
/// happens at convert time; this only checks the spelling (`first >= 1`,
/// `first <= last`).
pub fn parse_page_range(s: &str) -> Result<(usize, usize), String> {
    let parse_one = |part: &str| {
        part.trim()
            .parse::<usize>()
            .map_err(|_| format!("invalid page number '{}'", part.trim()))
    };
    let (first, last) = match s.split_once('-') {
        Some((a, b)) => (parse_one(a)?, parse_one(b)?),
        None => {
            let n = parse_one(s)?;
            (n, n)
        }
    };
    if first == 0 {
        return Err("pages are 1-based; the range starts at 1".into());
    }
    if last < first {
        return Err(format!("range {first}-{last} is inverted (first <= last)"));
    }
    Ok((first, last))
}

impl Default for DocumentConverter {
    fn default() -> Self {
        Self {
            allowed_formats: None,
            strict: false,
            fetch_images: false,
            list_attachments: false,
            skip_empty_cells: false,
            compact_tables: false,
            page_break_placeholder: None,
            ebcdic_layout: None,
            no_table_former: false,
            no_text_panels: false,
            text_layer_only: false,
            no_ocr: false,
            password: None,
            force_full_page_ocr: false,
            ocr_mode: None,
            ocr_engine: None,
            ocr_scale: None,
            images_scale: None,
            generate_page_images: false,
            heading_hierarchy: false,
            use_web_browser: false,
            asr_model: None,
            asr_lang: None,
            video_frames: None,
            video_scene_threshold: None,
            video_frame_max_side: None,
            video_frame_dedupe: None,
            xbrl_taxonomy: None,
            enrich: crate::EnrichmentOptions::default(),
            picture_ocr: false,
            picture_ocr_classes: Vec::new(),
            picture_ocr_min_side: None,
            keep_picture_images: true,
            redact: None,
            page_range: None,
            ocr_lang: None,
            encoding: None,
            artifacts_dir: "artifacts".to_string(),
            document_timeout: None,
            archive_limits: crate::archive::ArchiveLimits::default(),
        }
    }
}

impl DocumentConverter {
    /// A converter that accepts every supported format.
    pub fn new() -> Self {
        Self::default()
    }

    /// A converter restricted to an explicit set of formats. Sources of any
    /// other format are rejected with [`ConversionError::UnsupportedFormat`].
    pub fn with_allowed_formats(formats: impl IntoIterator<Item = InputFormat>) -> Self {
        Self {
            allowed_formats: Some(formats.into_iter().collect()),
            ..Self::default()
        }
    }

    /// Convert only PDF pages `first..=last` (**1-based** inclusive, the page
    /// numbers a viewer shows — issue #80's `--pages A-B`). Out-of-window pages
    /// are skipped before rasterization, so converting 3 pages of a 500-page
    /// PDF costs 3 pages. `last` clamps to the document; a window that selects
    /// no pages at all errors at convert time. Non-PDF formats ignore the
    /// window (they convert whole).
    pub fn page_range(mut self, first: usize, last: usize) -> Self {
        self.page_range = Some((first, last));
        self
    }

    /// A wall-clock budget per document for the PDF pipeline — docling's
    /// `PipelineOptions.document_timeout` (#497); `None` = unlimited, the
    /// default. The budget starts with the conversion and is checked between
    /// pages: once spent, no further page is rendered or processed, the pages
    /// already finished become the document, and the result is a
    /// [`ConversionStatus::PartialSuccess`] carrying one
    /// [`ErrorItem`](crate::ErrorItem) (`document_backend` / `pipeline`,
    /// "document timeout of Ns exceeded after D of S pages …"). A page in
    /// flight finishes first, so a budget shorter than one page's work still
    /// yields that page. Only the paginated pipeline has pages to stop
    /// between: declarative formats (Office, HTML, …) convert whole, as in
    /// docling, and a single image is never cut. A streaming conversion
    /// emits the pages that fit and ends with
    /// [`ConversionError::Timeout`](crate::ConversionError::Timeout) as its
    /// last item.
    pub fn document_timeout(mut self, timeout: Option<std::time::Duration>) -> Self {
        self.document_timeout = timeout;
        self
    }

    /// OCR recognition language for scanned PDF/image pages: `"en"` (the
    /// default — English PP-OCRv3, proper Latin word spacing) or `"ch"` (the
    /// multilingual model docling conformance is measured with — glues Latin
    /// words). An unknown value warns at conversion time and uses the
    /// default; explicit `DOCLING_OCR_REC_ONNX`/`DOCLING_OCR_DICT` paths win
    /// over this switch. Under [`ocr_engine`](Self::ocr_engine) `tesseract`
    /// it is Tesseract's language list instead — tessdata stems (`"deu"`,
    /// `"eng+fra"`, `"script/Cyrillic"`) or BCP-47 tags mapped onto them
    /// (`"de"`, `"zh-Hant"`), see [`docling_pdf::tesseract_lang_arg`]; `en`
    /// and `ch` keep working there. Formats that never OCR ignore it.
    pub fn ocr_lang(mut self, lang: impl Into<String>) -> Self {
        self.ocr_lang = Some(lang.into());
        self
    }

    /// The configured ML pipeline for one conversion (models load per call —
    /// callers that convert many files hold a warm [`docling_pdf::Pipeline`]
    /// themselves). Grew out of docling-pdf's `convert_with_options` free
    /// functions, whose fixed signatures couldn't take #244's `skip_ocr`.
    #[cfg(feature = "pdf")]
    fn ml_pipeline(&self) -> Result<docling_pdf::Pipeline, docling_pdf::PdfError> {
        Ok(docling_pdf::Pipeline::new()?
            .no_table_former(self.no_table_former)
            // docling-pdf's `Pipeline` keeps its pre-2.0 names: its `no_ocr`
            // is the text-layer fast path, its `skip_ocr` docling's do_ocr=False.
            .no_ocr(self.text_layer_only)
            .skip_ocr(self.no_ocr)
            .no_text_panels(self.no_text_panels)
            .enrichments(self.enrich)
            .ocr_lang(self.ocr_lang_choice())
            .ocr_engine(self.ocr_engine_choice())
            .tesseract_lang(self.tesseract_lang_choice())
            .ocr_mode(self.ocr_mode_choice())
            .ocr_scale(self.ocr_scale_choice())
            .images_scale(self.images_scale)
            .generate_page_images(self.generate_page_images)
            .heading_hierarchy(docling_pdf::HeadingHierarchyOptions::enabled(
                self.heading_hierarchy,
            ))
            .document_timeout(self.document_timeout))
    }

    /// DjVu (#434): the hidden text layer by default; a scan-only DjVu (no
    /// text layer on any selected page) falls back to rasterize + OCR when the
    /// ML pipeline is built and OCR is not disabled, otherwise it degrades to
    /// an empty document with a warning. See [`crate::backend::djvu`].
    fn convert_djvu(
        &self,
        source: &SourceDocument,
    ) -> Result<docling_core::DoclingDocument, ConversionError> {
        use crate::backend::djvu;
        let text = djvu::convert_text_layer(source, self.page_range)?;
        if !djvu::is_text_layer_empty(&text) {
            return Ok(text);
        }
        // No text layer anywhere in the selection.
        #[cfg(feature = "pdf")]
        if !self.text_layer_only && !self.no_ocr {
            let pngs = djvu::rasterize_pages(&source.bytes, self.page_range)?;
            let mut pipeline = self
                .ml_pipeline()
                .map_err(|e| ConversionError::with_source("djvu", e))?;
            let mut doc = docling_core::DoclingDocument::new(&source.name);
            for (i, (page_no, png)) in pngs.iter().enumerate() {
                if i > 0 {
                    doc.push(docling_core::Node::PageBreak);
                }
                let mut page = pipeline
                    .convert_image(png, &source.name)
                    .map_err(|e| ConversionError::with_source("djvu", e))?;
                // An image is its own page 1; the DjVu page keeps its number so
                // a `--pages` window's JSON `pages` matches the text-layer path.
                docling_pdf::assemble::stamp_page_no(&mut page.nodes, *page_no);
                doc.nodes.extend(page.nodes);
                doc.links.extend(page.links);
            }
            return Ok(doc);
        }
        eprintln!(
            "docling: warning: DjVu '{}' has no text layer and OCR is unavailable/disabled; \
             the document is empty",
            source.name
        );
        Ok(text)
    }

    /// The parsed [`Self::ocr_lang`] choice for the ML call sites; a value
    /// that parses to nothing warns here (once per conversion) rather than
    /// erroring — same degradation the env selector applies.
    #[cfg(feature = "pdf")]
    fn ocr_lang_choice(&self) -> Option<docling_pdf::OcrLang> {
        let raw = self.ocr_lang.as_deref()?;
        // Under Tesseract the value is its language list, not a PP-OCR model
        // — see `tesseract_lang_choice`.
        if self.ocr_engine_choice() == Some(docling_pdf::OcrEngine::Tesseract) {
            return None;
        }
        let parsed = docling_pdf::OcrLang::parse(raw);
        if parsed.is_none() {
            eprintln!(
                "docling: ocr_lang {raw:?} names no language the OCR models read ({}); using \
                 the default",
                docling_pdf::OcrLang::ACCEPTED
            );
        }
        parsed
    }

    /// The parsed [`Self::ocr_engine`] choice (#460), with the same
    /// warn-and-default degradation as [`ocr_lang_choice`](Self::ocr_lang_choice).
    #[cfg(feature = "pdf")]
    fn ocr_engine_choice(&self) -> Option<docling_pdf::OcrEngine> {
        let raw = self.ocr_engine.as_deref()?;
        let parsed = docling_pdf::OcrEngine::parse(raw);
        if parsed.is_none() {
            eprintln!(
                "docling: ocr_engine {raw:?} is not {}; using the default",
                docling_pdf::OcrEngine::ACCEPTED
            );
        }
        parsed
    }

    /// Tesseract's `-l` argument from [`Self::ocr_lang`] (#460) when the
    /// engine is Tesseract: an unmappable value warns and leaves Tesseract's
    /// default language.
    #[cfg(feature = "pdf")]
    fn tesseract_lang_choice(&self) -> Option<String> {
        if self.ocr_engine_choice() != Some(docling_pdf::OcrEngine::Tesseract) {
            return None;
        }
        let raw = self.ocr_lang.as_deref()?;
        match docling_pdf::tesseract_lang_arg(raw) {
            Ok(arg) => Some(arg),
            Err(e) => {
                eprintln!("docling: {e}; using Tesseract's default language");
                None
            }
        }
    }

    /// The parsed [`Self::ocr_mode`] choice (#254), with the same
    /// warn-and-default degradation as [`ocr_lang_choice`](Self::ocr_lang_choice).
    #[cfg(feature = "pdf")]
    fn ocr_mode_choice(&self) -> Option<docling_pdf::OcrMode> {
        let raw = self.ocr_mode.as_deref()?;
        let parsed = docling_pdf::OcrMode::parse(raw);
        if parsed.is_none() {
            eprintln!(
                "docling: ocr_mode {raw:?} is not \
                 default|full_page|layout_regions|pdf_aware_layout_regions; using the default"
            );
        }
        parsed
    }

    /// The validated [`Self::ocr_scale`] (#254): non-positive/non-finite
    /// values warn and fall back to the engine default.
    #[cfg(feature = "pdf")]
    fn ocr_scale_choice(&self) -> Option<f32> {
        let s = self.ocr_scale?;
        if !(s.is_finite() && s > 0.0) {
            eprintln!("docling: ocr_scale {s} is not a positive number; using the default");
            return None;
        }
        Some(s)
    }

    /// Where [`ImageMode::Referenced`] streaming writes image files, and the
    /// link prefix used in the Markdown (default `artifacts`, matching the
    /// buffered export's convention). Relative paths resolve against the
    /// process working directory.
    pub fn artifacts_dir(mut self, dir: impl Into<String>) -> Self {
        self.artifacts_dir = dir.into();
        self
    }

    /// Decode text inputs (Markdown, CSV, AsciiDoc, WebVTT, LaTeX, the XML
    /// dialects, …) with this character encoding instead of detecting one —
    /// docling's `TextBackendOptions.encoding`
    /// (`MarkdownBackendOptions(encoding="shift_jis")`). A WHATWG encoding
    /// label (`shift_jis`, `koi8-r`, `windows-1251`, `latin1`; Python codec
    /// spellings with `_` are accepted). Nothing is guessed: bytes the
    /// encoding cannot decode fail the conversion, as does an unknown label.
    /// `None` (default) detects — a byte-order mark, then UTF-8, then
    /// windows-1252 (see [`SourceDocument::text`]). A source that already
    /// carries its own [`SourceDocument::encoding`] keeps it.
    pub fn encoding(mut self, label: Option<String>) -> Self {
        self.encoding = label;
        self
    }

    /// The converter's [`encoding`](Self::encoding) applied to a source that
    /// did not set its own.
    /// Bounds on what a ZIP input may make the converter decompress (#557):
    /// entries considered, per-entry and total uncompressed size, compression
    /// ratio. [`ArchiveLimits::default`](crate::archive::ArchiveLimits) when
    /// not set. Only [`DocumentConverter::convert_archive`] reads them.
    pub fn archive_limits(mut self, limits: crate::archive::ArchiveLimits) -> Self {
        self.archive_limits = limits;
        self
    }

    pub(crate) fn archive_limits_ref(&self) -> crate::archive::ArchiveLimits {
        self.archive_limits
    }

    fn with_encoding(&self, mut source: SourceDocument) -> SourceDocument {
        if source.encoding.is_none() {
            source.encoding = self.encoding.clone();
        }
        source
    }

    /// Cap the number of frames sampled from a video (#138 Phase 2); `0`
    /// disables frame extraction entirely (Phase 1 behavior: transcript only).
    /// Defaults to [`DEFAULT_VIDEO_FRAMES`]. Frames are extracted with the
    /// `ffmpeg` binary when present (`DOCLING_FFMPEG` overrides the path);
    /// without it a video converts to its transcript alone.
    pub fn video_frames(mut self, max: usize) -> Self {
        self.video_frames = Some(max);
        self
    }

    /// Keep every distinct cut of a video (#647) — [`ALL_VIDEO_FRAMES`]:
    /// no cap, so what bounds the picture count is the scene threshold and
    /// the de-duplication. Memory stays one frame at a time whatever the
    /// count ([`do_picture_ocr`](Self::do_picture_ocr) +
    /// [`keep_picture_images`](Self::keep_picture_images)`(false)` reads and
    /// drops each frame before the next is decoded).
    pub fn video_frames_all(self) -> Self {
        self.video_frames(ALL_VIDEO_FRAMES)
    }

    /// ffmpeg's `scene` score (0–1, the normalized difference of a frame to
    /// its predecessor) a frame must exceed to be a cut (#647). Default
    /// 0.27 (`DOCLING_RS_VIDEO_SCENE_THRESHOLD` overrides it): a slide
    /// change in a screen recording scores well above it, a fade or a
    /// lighting change around 0.5, a hard cut near 1.0 — raise it to keep
    /// only hard cuts. Out-of-range values are clamped.
    pub fn video_scene_threshold(mut self, threshold: f32) -> Self {
        self.video_scene_threshold = Some(threshold.clamp(0.0, 1.0));
        self
    }

    /// Downscale each sampled frame inside ffmpeg so its longer side is at
    /// most `px` (#647; the aspect ratio is kept, a smaller frame is left as
    /// it is): a 1080p recording at 640 yields 640×360 PNGs, so neither the
    /// embedded images nor the OCR input ever reach memory at full size.
    /// `0` (the default, `DOCLING_RS_VIDEO_FRAME_MAX_SIDE` overrides it) =
    /// the source resolution.
    pub fn video_frame_max_side(mut self, px: u32) -> Self {
        self.video_frame_max_side = Some(px);
        self
    }

    /// Drop a sampled frame whose perceptual difference hash (64 bits over
    /// a 9×8 grey thumbnail) is within `max_distance` bits of a frame
    /// already kept (#647): the same slide after a fade, or shown again
    /// after a camera cut, becomes one picture. 4–6 is a good distance;
    /// `None` (the default, `DOCLING_RS_VIDEO_FRAME_DEDUPE` overrides it)
    /// keeps every sampled frame.
    pub fn video_frame_dedupe(mut self, max_distance: Option<u32>) -> Self {
        self.video_frame_dedupe = max_distance.map(|d| d.min(64));
        self
    }

    /// The folder holding the taxonomy an XBRL instance refers to (docling's
    /// `XBRLBackendOptions.taxonomy`): the filing's extension schema and
    /// linkbases at the relative paths its `link:schemaRef` names, plus any
    /// taxonomy packages (`.zip` with a `META-INF/catalog.xml`) that map the
    /// base taxonomies' `http(s)` URLs to files for offline use. Without it the
    /// instance's own directory is searched. Nothing is fetched remotely; a
    /// document that cannot be found only costs the fact graph the hierarchy
    /// it would have contributed.
    pub fn xbrl_taxonomy(mut self, dir: impl Into<PathBuf>) -> Self {
        self.xbrl_taxonomy = Some(dir.into());
        self
    }

    /// Select a named ASR model preset for audio sources — the English-only
    /// (`whisper_tiny_en`, `whisper_base_en`, `whisper_small_en`) and
    /// Distil-Whisper (`whisper_distil_small_en`) variants of docling's ASR
    /// model specs, or `parakeet_tdt_0.6b_v3` (NVIDIA's Parakeet TDT 0.6B v3
    /// transducer, 25 European languages detected by the model, #508 — see
    /// `docling_asr::parakeet`). `None` (default) uses Whisper tiny
    /// (multilingual) from `.models/asr/`; presets load from
    /// `.models/asr/<preset>/` (fetch them with `download_dependencies.sh
    /// --asr-model=<preset>`).
    pub fn asr_model(mut self, model: Option<String>) -> Self {
        self.asr_model = model;
        self
    }

    /// Select the ASR transcription language for audio/video sources: a
    /// Whisper code (`en`, `de`, `zh`, …) or `auto`. `None` (default) falls
    /// back to `DOCLING_RS_ASR_LANG`, and — when that is unset too — to
    /// per-file auto-detection from the first 30-second window (docling
    /// 2.116 parity). English-only presets always transcribe English.
    pub fn asr_lang(mut self, lang: Option<String>) -> Self {
        self.asr_lang = lang;
        self
    }

    /// Select the Markdown export mode for documents this converter produces.
    ///
    /// `false` (default) makes [`crate::DoclingDocument::export_to_markdown`]
    /// reproduce docling's legacy output byte-for-byte; `true` makes it emit
    /// cleaner, more conformant Markdown (code-fence languages preserved, no
    /// inline-run spacing artifacts, no entity re-escaping). Rust-only — Python
    /// docling has no such switch.
    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Fetch and embed external `<img>` images for HTML/EPUB/MHTML/JATS sources.
    ///
    /// Off by default (matching docling's `enable_*_fetch=False`), so output is
    /// unchanged unless you opt in. When on, the HTML/EPUB/MHTML backends
    /// resolve each `<img src>` — `data:` URIs, local files (relative to the
    /// source file's directory), `http(s)` URLs, and EPUB/MHTML archive
    /// entries — and the JATS backend reads a `<fig>`'s `<graphic xlink:href>`
    /// from the source file's directory (#392), embedding the bytes so they
    /// survive into JSON `ImageRef`s and
    /// [`crate::DoclingDocument::export_to_markdown_with_images`].
    ///
    /// Remote `http(s)` URLs are fetched over the network; enable only for input
    /// you trust (it can otherwise be used to make the process issue requests).
    pub fn fetch_images(mut self, fetch: bool) -> Self {
        self.fetch_images = fetch;
        self
    }

    /// Append an `Attachments` section to converted emails (`.eml` / `.msg`):
    /// one list item per attachment, `name (content/type)` — names and types
    /// only, the payload is never embedded. docling's opt-in
    /// `EmailBackendOptions.list_attachments` (#251); off by default.
    pub fn list_attachments(mut self, list: bool) -> Self {
        self.list_attachments = list;
        self
    }

    /// Omit empty cells from sparse spreadsheet table grids (#271; XLSX/XLS
    /// family, opt-in — a docling.rs extension, docling materialises the full
    /// bounding box). A ragged region's box is mostly padding on sparse
    /// sheets (~7× output inflation); with this on, each row keeps only its
    /// occupied cells (merge-covered continuations included) and a table
    /// that loses cells drops its span/structure overlay. Off by default —
    /// default output stays byte-for-byte docling.
    pub fn skip_empty_cells(mut self, skip: bool) -> Self {
        self.skip_empty_cells = skip;
        self
    }

    /// Emit Markdown tables in the compact `| a | b |` / `| - | - |` form
    /// instead of docling-core's width-padded GitHub serializer (#271, all
    /// formats; opt-in — a docling.rs extension). On sparse spreadsheets the
    /// padding dominates the output size; compact rendering keeps the grid
    /// semantics and drops only whitespace. Off by default — default output
    /// stays byte-for-byte docling.
    pub fn compact_tables(mut self, compact: bool) -> Self {
        self.compact_tables = compact;
        self
    }

    /// Insert `placeholder` between pages in the Markdown export — docling's
    /// `export_to_markdown(page_break_placeholder=…)` (docling-core's
    /// `MarkdownParams`, docling-serve's `md_page_break_placeholder`). Where
    /// docling marks a break between two items whose `prov.page_no` differ,
    /// the serializer here marks it between two rendered blocks separated by a
    /// page boundary — the `PageInfo` marker opening every PDF page and sheet,
    /// the `PageBreak` between slides and DjVu / DocTags pages — so a break
    /// never leads or trails the document and empty pages collapse into one.
    /// `None` (the default) keeps Markdown free of page breaks, as docling's
    /// default export is. Markdown only; JSON carries pages in `prov`.
    pub fn page_break_placeholder(mut self, placeholder: Option<String>) -> Self {
        self.page_break_placeholder = placeholder;
        self
    }

    /// The document-level settings [`convert`](Self::convert) applies to
    /// every backend's output: the serializer knobs carried on the document
    /// (`strict`, `compact_tables`, `page_break_placeholder`) so each later
    /// Markdown export agrees, and first-class cells for every table. Public
    /// for callers that run a backend themselves — docling-serve converts
    /// PDFs and images on a warm `Pipeline` and finishes
    /// them here, so a request's options reach them as they reach every
    /// other format (#547).
    pub fn finish_document(&self, document: &mut docling_core::DoclingDocument) {
        // Carry the mode so `result.document.export_to_markdown()` reflects it.
        document.strict_markdown = self.strict;
        // Compact tables (#271) is additive: the PDF backend already turns it
        // on for its own corpus; never turn it back off here.
        if self.compact_tables {
            document.compact_tables = true;
        }
        // Page-break placeholder: a serializer knob like `strict`, carried on
        // the document so every Markdown export of it agrees.
        if self.page_break_placeholder.is_some() {
            document.page_break_placeholder = self.page_break_placeholder.clone();
        }
        // First-class cells for every table (#240): backends with page
        // geometry (the PDF TableFormer paths) set them; everything else —
        // declarative tables included — derives them from the grid plus the
        // structure overlay (real spans for DOCX/XLSX merges, HTML `th`
        // headers, ODF covered cells; 1×1 records otherwise), so the repair
        // API and the JSON `table_cells` are populated uniformly.
        for table in document.tables_mut() {
            if table.cells.is_none() {
                table.cells = Some(table.derive_cells());
            }
        }
    }

    /// The picture enrichment pass (#645): OCR the embedded pictures when
    /// [`do_picture_ocr`](Self::do_picture_ocr) asks for it, then drop the
    /// image bytes unless [`keep_picture_images`](Self::keep_picture_images)
    /// keeps them. Runs on every conversion after the backend, before
    /// [`finish_document`](Self::finish_document).
    fn enrich_pictures(&self, document: &mut docling_core::DoclingDocument, format: InputFormat) {
        if self.picture_ocr {
            self.ocr_pictures(document, format);
        }
        if !self.keep_picture_images {
            strip_picture_images(document);
        }
    }

    /// The picture-OCR pass itself: PDF, image and METS pages already went
    /// through the OCR pipeline, so their pictures are left alone, and a
    /// video's frames were read as they were decoded
    /// ([`convert_video`](Self::convert_video)); with OCR disabled, or
    /// without the recognizer, the pictures keep no text and one warning
    /// says why. The same image embedded twice — a DOCX walked into both
    /// the flat nodes and the item tree, a slide master's logo on every
    /// slide — is read once (cached by its bytes).
    fn ocr_pictures(&self, document: &mut docling_core::DoclingDocument, format: InputFormat) {
        use docling_core::{tree::TreeKind, Node};

        if matches!(
            format,
            InputFormat::Pdf | InputFormat::Image | InputFormat::MetsGbs | InputFormat::Video
        ) {
            return;
        }
        let Some(mut reader) = self.picture_reader() else {
            return;
        };
        document.for_each_picture_mut(&mut |node| {
            if let Node::Picture {
                image: Some(img),
                description,
                ..
            } = node
            {
                if description.is_none() {
                    *description = reader.read(img);
                }
            }
        });
        if let Some(tree) = document.tree.as_mut() {
            for item in tree.items.iter_mut() {
                if let TreeKind::Picture {
                    image: Some(img),
                    description,
                    ..
                } = &mut item.kind
                {
                    if description.is_none() {
                        *description = reader.read(img);
                    }
                }
            }
        }
    }

    /// The reader the picture-OCR pass (#645) and the video frame hook
    /// (#647) share: the OCR models only — no layout, no TableFormer, the
    /// caller already knows each whole image is the picture — behind the
    /// size floor and the classifier filter. `None` (after one warning)
    /// when OCR is disabled or the recognizer cannot load.
    #[cfg(feature = "pdf")]
    fn picture_reader(&self) -> Option<PictureReader> {
        static WARNED: std::sync::Once = std::sync::Once::new();
        if self.no_ocr || self.text_layer_only {
            WARNED.call_once(|| {
                eprintln!(
                    "warning: picture OCR requested, but OCR is disabled (no_ocr / \
                     text_layer_only); pictures keep no text"
                );
            });
            return None;
        }
        let pipeline = docling_pdf::Pipeline::new().map(|p| {
            p.no_ocr(true)
                .no_table_former(true)
                .ocr_lang(self.ocr_lang_choice())
                .ocr_engine(self.ocr_engine_choice())
                .tesseract_lang(self.tesseract_lang_choice())
                .ocr_scale(self.ocr_scale_choice())
        });
        let pipeline = match pipeline {
            Ok(p) => p,
            Err(e) => {
                WARNED.call_once(|| {
                    eprintln!("warning: picture OCR unavailable ({e}); pictures keep no text");
                });
                return None;
            }
        };
        let min_side = self
            .picture_ocr_min_side
            .or_else(|| docling_core::env::parse::<u32>("DOCLING_RS_PICTURE_OCR_MIN_SIDE"))
            .unwrap_or(32);
        Some(PictureReader {
            pipeline,
            min_side,
            classes: self.picture_ocr_classes.clone(),
            cache: std::collections::HashMap::new(),
        })
    }

    /// Without the `pdf` feature there is no OCR engine to run: one warning,
    /// the pictures keep no text.
    #[cfg(not(feature = "pdf"))]
    fn picture_reader(&self) -> Option<PictureReader> {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            eprintln!(
                "warning: picture OCR requested, but this build has no OCR engine (rebuild \
                 with the `pdf` feature); pictures keep no text"
            );
        });
        None
    }

    /// The frame sampling a video gets (#647): the builder's settings, else
    /// the environment's, else the defaults.
    #[cfg(feature = "asr")]
    fn frame_options(&self) -> crate::video::FrameOptions {
        use docling_core::env;
        crate::video::FrameOptions {
            max_frames: self.video_frames.unwrap_or(DEFAULT_VIDEO_FRAMES),
            scene_threshold: self
                .video_scene_threshold
                .or_else(|| env::parse::<f32>("DOCLING_RS_VIDEO_SCENE_THRESHOLD"))
                .map_or(crate::video::DEFAULT_SCENE_THRESHOLD, |t| t.clamp(0.0, 1.0)),
            max_side: self
                .video_frame_max_side
                .or_else(|| env::parse::<u32>("DOCLING_RS_VIDEO_FRAME_MAX_SIDE"))
                .unwrap_or(0),
            dedupe: self
                .video_frame_dedupe
                .or_else(|| env::parse::<u32>("DOCLING_RS_VIDEO_FRAME_DEDUPE"))
                .map(|d| d.min(64)),
        }
    }

    /// Video (#138): the audio track transcribes through the ASR path
    /// (Phase 1), and — when the ffmpeg binary is available — sampled
    /// frames interleave with the transcript as timestamped pictures
    /// (Phase 2). Without ffmpeg: transcript only. Each frame goes through
    /// the picture enrichment as soon as it is decoded (#647): read by the
    /// OCR models when [`do_picture_ocr`](Self::do_picture_ocr) asks, its
    /// bytes dropped when [`keep_picture_images`](Self::keep_picture_images)
    /// is off — so however many frames are sampled, one is in memory at a
    /// time. The models load on the first frame; a video without one costs
    /// nothing.
    #[cfg(feature = "asr")]
    fn convert_video(
        &self,
        source: &SourceDocument,
    ) -> Result<docling_core::DoclingDocument, String> {
        use docling_core::Node;
        let opts = self.frame_options();
        let mut reader: Option<Option<PictureReader>> = None;
        let mut on_frame = |node: &mut Node| {
            if self.picture_ocr {
                let reader = reader.get_or_insert_with(|| self.picture_reader());
                if let (
                    Some(reader),
                    Node::Picture {
                        image: Some(img),
                        description,
                        ..
                    },
                ) = (reader.as_mut(), &mut *node)
                {
                    if description.is_none() {
                        *description = reader.read(img);
                    }
                }
            }
            if !self.keep_picture_images {
                if let Node::Picture { image, .. } = node {
                    *image = None;
                }
            }
        };
        crate::video::convert_video(
            &source.bytes,
            &source.name,
            self.asr_model.as_deref(),
            self.asr_lang.as_deref(),
            &opts,
            &mut on_frame,
        )
    }

    /// The copybook layout for EBCDIC sources (#252): docling's
    /// `EbcdicLayout` JSON, inline (a string starting with `{`) or as a file
    /// path. Without it, a path-loaded source looks for a
    /// `<stem>.layout.json` sidecar; converting EBCDIC with neither is an
    /// error — the bytes are meaningless without their copybook.
    pub fn ebcdic_layout(mut self, layout: impl Into<String>) -> Self {
        self.ebcdic_layout = Some(layout.into());
        self
    }

    /// Option-typed variant of [`ebcdic_layout`](Self::ebcdic_layout) for
    /// call sites plumbing an optional flag through (`None` keeps the
    /// sidecar fallback).
    pub fn ebcdic_layout_opt(mut self, layout: Option<String>) -> Self {
        self.ebcdic_layout = layout;
        self
    }

    /// Skip loading and running the TableFormer table-structure model for
    /// PDF/image/METS sources.
    ///
    /// Off by default. When enabled, table regions are still detected and
    /// emitted, but their structure is reconstructed geometrically from cell
    /// positions instead of the ONNX model's predicted structure — no model
    /// load and no per-table inference, at the cost of table fidelity. Useful
    /// when parsing speed matters more than exact table structure, especially
    /// with [`convert_streaming`](Self::convert_streaming).
    pub fn no_table_former(mut self, disable: bool) -> Self {
        self.no_table_former = disable;
        self
    }

    /// PDF/image: keep every detected picture as a picture — disable the
    /// text-panel demotion that turns an uncaptioned, dense text-panel
    /// "picture" into paragraphs (#157). The escape hatch for
    /// image-extraction workflows and for charts the heuristic might still
    /// misjudge on scanned pages (#173).
    pub fn no_text_panels(mut self, disable: bool) -> Self {
        self.no_text_panels = disable;
        self
    }

    /// Infer PDF/image section-header levels after assembly (#302, docling's
    /// `HeadingHierarchyModel` with its default options): the PDF outline
    /// (bookmarks) is authoritative, legal/outline numbering covers headings
    /// without a bookmark match, and font size/weight/slant/case rank the
    /// rest. Off by default (docling parity): every detected heading then
    /// keeps the flat level the assembler emits. Non-PDF/image formats
    /// ignore it (their backends carry real heading levels already).
    pub fn heading_hierarchy(mut self, enable: bool) -> Self {
        self.heading_hierarchy = enable;
        self
    }

    /// Skip layout detection, OCR, and TableFormer entirely for PDF/image/METS
    /// sources — no model load, no inference of any kind (#611; this was
    /// `no_ocr` before 2.0).
    ///
    /// Off by default. When enabled, the PDF's embedded text cells are grouped by
    /// line and emitted as plain paragraphs in reading order: no headings, lists,
    /// tables, code blocks, or pictures, since that structure comes from the
    /// layout model. The fastest possible PDF path, but pages with no embedded
    /// text layer (scanned/image-only PDFs) yield no text at all — convert those
    /// without this flag. Implies [`no_table_former`](Self::no_table_former).
    pub fn text_layer_only(mut self, enable: bool) -> Self {
        self.text_layer_only = enable;
        self
    }

    /// Never run OCR, but keep layout detection and TableFormer — docling's
    /// `do_ocr=False`, its CLI's `--no-ocr` (#244; since 2.0, #611, the
    /// meaning of `no_ocr` too). Unlike
    /// [`text_layer_only`](Self::text_layer_only) (the skip-everything fast
    /// path), structured output — headings, tables, pictures, reading order —
    /// is preserved; only text that exists solely as pixels is lost (scanned
    /// pages come back with empty regions, and the speculative OCR of large
    /// embedded images never runs). The OCR model is never loaded, and
    /// independently of this flag a *missing* OCR model degrades to the same
    /// behavior with a warning instead of failing the conversion. SVG inputs
    /// route to direct `<text>` extraction (their text is native — skipping
    /// OCR must not lose it).
    pub fn no_ocr(mut self, disable: bool) -> Self {
        self.no_ocr = disable;
        self
    }

    /// [`no_ocr`](Self::no_ocr) under its pre-2.0 name, kept as an alias.
    pub fn skip_ocr(self, disable: bool) -> Self {
        self.no_ocr(disable)
    }

    /// The password of an encrypted document: a PDF (docling's
    /// `--pdf-password`, #611) or an Office document (#625, a docling.rs
    /// extension — docling reads none): `.docx`/`.xlsx`/`.pptx` (Agile and
    /// Standard encryption), `.doc`/`.xls`/`.ppt` (RC4 and RC4 CryptoAPI).
    ///
    /// A missing or wrong PDF password fails the conversion with "the PDF is
    /// encrypted: a password is required"; PDF passwords are read on the ML
    /// pipeline only (the `pdf-text` / wasm build reads unencrypted PDFs).
    /// Office documents decrypt in every build. Without a password, or when
    /// it is wrong, the format's default password is tried (Excel's
    /// `VelvetSweatshop`, PowerPoint's for modify-password-only files);
    /// otherwise the error says the document is encrypted, or that the
    /// password is wrong.
    pub fn password(mut self, password: Option<String>) -> Self {
        self.password = password;
        self
    }

    /// [`password`](Self::password) under docling's PDF-only name.
    pub fn pdf_password(self, password: Option<String>) -> Self {
        self.password(password)
    }

    /// OCR every PDF page from its rendered image even when the page carries
    /// an embedded text layer — docling's `force_full_page_ocr`. The escape
    /// hatch for text layers that exist but lie (broken encodings, subset
    /// fonts with garbage mappings, scanned forms with a few typed-in
    /// fields). Off by default; ignored when [`no_ocr`](Self::no_ocr) or
    /// [`text_layer_only`](Self::text_layer_only) is set,
    /// mirroring docling, where it is a sub-option of `do_ocr`. Applies to
    /// PDFs only — standalone images are always OCR'd.
    pub fn force_full_page_ocr(mut self, force: bool) -> Self {
        self.force_full_page_ocr = force;
        self
    }

    /// Which document regions feed the OCR — docling's `OcrMode` (#254):
    /// `default`, `full_page`, `layout_regions`, or
    /// `pdf_aware_layout_regions`. The default is the text-layer-aware
    /// behavior (docling's `pdf_aware_layout_regions`);
    /// `full_page`/`layout_regions` discard the text layer like
    /// [`force_full_page_ocr`](Self::force_full_page_ocr) — see
    /// [`docling_pdf::OcrMode`] for the mapping. An unknown value warns at
    /// conversion time and uses the default. PDF/image ML pipeline only.
    pub fn ocr_mode(mut self, mode: impl Into<String>) -> Self {
        self.ocr_mode = Some(mode.into());
        self
    }

    /// Which OCR engine recognizes text on scanned pages (#460): `"ppocr"`
    /// (the default — the built-in PP-OCRv3 recognizer, the conformance
    /// engine) or `"tesseract"` (the system `tesseract` binary, docling's
    /// `TesseractCliOcrOptions`; needs `tesseract-ocr` with a language pack
    /// installed — `DOCLING_TESSERACT` names the binary,
    /// `DOCLING_RS_TESSERACT_PSM` its page segmentation mode,
    /// `DOCLING_RS_TESSDATA_DIR` / `TESSDATA_PREFIX` its data). Both read
    /// the same layout-region crops; [`ocr_lang`](Self::ocr_lang) is the
    /// engine's language. An unknown value warns at conversion time and uses
    /// the default (`DOCLING_RS_OCR_ENGINE`, else PP-OCR). Formats that never
    /// OCR ignore it.
    pub fn ocr_engine(mut self, engine: impl Into<String>) -> Self {
        self.ocr_engine = Some(engine.into());
        self
    }

    /// OCR render scale in pixels per PDF point — docling's
    /// `OcrOptions.scale` (#254; docling's default 3 = 216 dpi). Unset feeds
    /// the recognizer the pipeline's own 2.0 px/pt page render; a different
    /// value resamples that render for the OCR input only, leaving layout and
    /// TableFormer pixels untouched. Non-positive values warn at conversion
    /// time and are ignored. PDF/image ML pipeline only.
    pub fn ocr_scale(mut self, scale: f32) -> Self {
        self.ocr_scale = Some(scale);
        self
    }

    /// Pixels per PDF point for picture crops and page images — docling's
    /// `images_scale` (#520). Unset delivers crops at the pipeline's own 2.0
    /// px/pt render (144 dpi), as before; another value resamples it (above
    /// 2.0 that upsamples, it does not re-render). The picture's `dpi` in the
    /// JSON export is 72·scale either way (#519). Non-finite or non-positive
    /// values are ignored. PDF/image ML pipeline only.
    pub fn images_scale(mut self, scale: f32) -> Self {
        self.images_scale = Some(scale).filter(|s| s.is_finite() && *s > 0.0);
        self
    }

    /// Keep every page's render as a page image — docling's
    /// `generate_page_images` (#520): the JSON export carries it as
    /// `pages[n].image` at [`Self::images_scale`], which docling-core's
    /// `TableItem.get_image` / `FormulaItem.get_image` crop from. Off by
    /// default (one full-page PNG per page in memory). PDF/image ML pipeline
    /// only; text-layer-only (`text_layer_only`) and streaming conversions get none.
    pub fn generate_page_images(mut self, enabled: bool) -> Self {
        self.generate_page_images = enabled;
        self
    }

    /// Classify each detected picture with the DocumentFigureClassifier model
    /// (docling's `do_picture_classification`). Off by default.
    ///
    /// The full 26-class prediction distribution (bar_chart, logo, signature,
    /// …) lands on the picture item and is serialized into the docling JSON as
    /// the `classification` annotation plus the `meta.classification` field.
    /// Markdown output is unaffected. Needs `.models/picture_classifier.onnx`
    /// (fetched by `scripts/install/download_dependencies.sh`); a missing
    /// model warns once and skips classification.
    pub fn do_picture_classification(mut self, enable: bool) -> Self {
        self.enrich.picture_classification = enable;
        self
    }

    /// OCR the pictures embedded in non-PDF documents (#645) — a DOCX/PPTX
    /// screenshot, an HTML figure, a sampled video frame — with the ML
    /// pipeline's OCR models (the PP-OCR recognizer + the `ocr_det.onnx`
    /// line detector when installed, or Tesseract under
    /// [`ocr_engine`](Self::ocr_engine)), and attach the text to the picture
    /// as docling's `PictureDescriptionData` annotation: Markdown prints it
    /// between the caption and the image placeholder, JSON/DCLX carry it as
    /// `meta.description` + the `description` annotation, the hybrid chunker
    /// puts it in the picture's chunk. Off by default — every default export
    /// is unchanged. Nothing is attached to a picture the engine reads no
    /// text from (lines under `DOCLING_RS_OCR_TEXT_SCORE` are dropped as on a
    /// scanned page). PDF, image and METS inputs are untouched: their pages
    /// go through the OCR pipeline already. With [`no_ocr`](Self::no_ocr) /
    /// [`text_layer_only`](Self::text_layer_only), or without the OCR model
    /// (or the `pdf` feature), one warning and no text. See also
    /// [`picture_ocr_classes`](Self::picture_ocr_classes),
    /// [`picture_ocr_min_side`](Self::picture_ocr_min_side) and
    /// [`keep_picture_images`](Self::keep_picture_images).
    pub fn do_picture_ocr(mut self, enable: bool) -> Self {
        self.picture_ocr = enable;
        self
    }

    /// Only OCR the pictures the DocumentFigureClassifier labels as one of
    /// `labels` (its 26 classes: `screenshot_from_computer`,
    /// `screenshot_from_manual`, `logo`, `photograph`, …), judged by its top
    /// prediction — the way to skip photos and logos on a slide deck while
    /// reading its screenshots. Empty (the default) OCRs every picture that
    /// passes the size floor. Needs `.models/picture_classifier.onnx`; a
    /// missing model warns once and the filter is waived.
    pub fn picture_ocr_classes<I, S>(mut self, labels: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.picture_ocr_classes = labels
            .into_iter()
            .map(|s| s.into().trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        self
    }

    /// The smallest side, in pixels, a picture must have to be OCR'd:
    /// bullets, icons and rules are skipped without decoding or inference.
    /// Default 32 (`DOCLING_RS_PICTURE_OCR_MIN_SIDE` overrides it); `0`
    /// reads everything.
    pub fn picture_ocr_min_side(mut self, px: u32) -> Self {
        self.picture_ocr_min_side = Some(px);
        self
    }

    /// Whether the pictures keep their embedded image bytes (the default,
    /// `true`): `false` drops them from every picture after the enrichment
    /// pass, so a document whose images were only there to be read — a slide
    /// deck of screenshots, the sampled frames of a video — exports a slim
    /// JSON/DCLX and a placeholder-only Markdown while keeping the OCR text.
    pub fn keep_picture_images(mut self, keep: bool) -> Self {
        self.keep_picture_images = keep;
        self
    }

    /// Redact personal data from every converted document (#621): after
    /// the backend builds the [`DoclingDocument`](docling_core::DoclingDocument)
    /// and before any export, chunker or stream reads it, every detected
    /// span — e-mail, phone, card number, IBAN, IP, URL credentials,
    /// national IDs, and names / organizations / locations when the NER
    /// model is installed (`.models/ner/`, the `ner` feature) — is replaced
    /// in place by a placeholder (`[EMAIL]`, `[PERSON_2]` with
    /// [`Replacement::Pseudonym`](docling_core::Replacement::Pseudonym), or
    /// a fixed string), in the flat nodes, the item tree behind the JSON,
    /// the link table, captions, hrefs, code, comments, table cells. Images
    /// are dropped by default
    /// ([`ImageRedaction`](docling_core::ImageRedaction)); `BoxOut` paints
    /// over the OCR'd lines that carry a span, `Keep` leaves them. The
    /// result's `redaction` is the [`RedactionReport`](docling_core::RedactionReport)
    /// — counts per label, never the values unless `return_mapping` asks.
    /// Off by default; unused, the output is byte-identical. A docling.rs
    /// extension — Python docling has no redaction stage — and not a
    /// compliance guarantee: pattern and model recall are what they are.
    pub fn redact_pii(mut self, opts: docling_core::RedactionOptions) -> Self {
        self.redact = Some(opts);
        self
    }

    /// Option-typed variant of [`redact_pii`](Self::redact_pii): `None`
    /// turns the pass off.
    pub fn redact_pii_opt(mut self, opts: Option<docling_core::RedactionOptions>) -> Self {
        self.redact = opts;
        self
    }

    /// Run the PII pass (#621) this converter is configured with over a
    /// document built elsewhere — docling-serve's warm pipeline and the
    /// VLM pipeline bypass [`convert`](Self::convert) — and return its
    /// report; `Ok(None)` when [`redact_pii`](Self::redact_pii) is unset.
    pub fn redact(
        &self,
        document: &mut docling_core::DoclingDocument,
    ) -> Result<Option<docling_core::RedactionReport>, ConversionError> {
        match self.redact.as_ref() {
            Some(opts) => self.redact_document(document, opts).map(Some),
            None => Ok(None),
        }
    }

    /// The PII pass over a converted document (#621): the detectors from
    /// the options, the OCR pipeline for `BoxOut` when this converter may
    /// run OCR (a `no_ocr` / `text_layer_only` converter drops the images
    /// instead, with the warning), then the walk. An invalid custom regex
    /// is a `Parse` error — the conversion cannot honour the request.
    fn redact_document(
        &self,
        document: &mut docling_core::DoclingDocument,
        opts: &docling_core::RedactionOptions,
    ) -> Result<docling_core::RedactionReport, ConversionError> {
        let detector = crate::redact::detector(opts)?;
        #[cfg(feature = "pdf")]
        {
            let mut ocr = None;
            if opts.images == docling_core::ImageRedaction::BoxOut
                && !self.no_ocr
                && !self.text_layer_only
            {
                // The OCR models only — no layout, no TableFormer.
                match docling_pdf::Pipeline::new() {
                    Ok(p) => {
                        ocr = Some(
                            p.no_ocr(true)
                                .no_table_former(true)
                                .ocr_lang(self.ocr_lang_choice())
                                .ocr_engine(self.ocr_engine_choice())
                                .tesseract_lang(self.tesseract_lang_choice())
                                .ocr_scale(self.ocr_scale_choice()),
                        )
                    }
                    Err(e) => crate::redact::warn_box_out_unavailable(&e.to_string()),
                }
            }
            Ok(crate::redact::run(document, opts, &detector, ocr.as_mut()))
        }
        #[cfg(not(feature = "pdf"))]
        {
            Ok(crate::redact::run(document, opts, &detector))
        }
    }

    /// Rewrite detected code blocks with the CodeFormulaV2 VLM (docling's
    /// `do_code_enrichment`). Off by default.
    ///
    /// The model re-reads the code crop at ~120 dpi, emits the clean source
    /// text (line breaks included) and identifies the language, which lands in
    /// the JSON `code_language` field. Needs the `.models/code_formula/` graphs
    /// (fetched by `scripts/install/download_dependencies.sh`); a missing
    /// model warns once and leaves the block as extracted.
    pub fn do_code_enrichment(mut self, enable: bool) -> Self {
        self.enrich.code = enable;
        self
    }

    /// Decode display formulas to LaTeX with the CodeFormulaV2 VLM (docling's
    /// `do_formula_enrichment`). Off by default.
    ///
    /// An enriched formula renders as `$$latex$$` in Markdown and as a
    /// `formula` text item in the JSON, replacing the
    /// `<!-- formula-not-decoded -->` placeholder. Same model artifacts as
    /// [`do_code_enrichment`](Self::do_code_enrichment).
    pub fn do_formula_enrichment(mut self, enable: bool) -> Self {
        self.enrich.formula = enable;
        self
    }

    /// Pre-render HTML-routing input in a headless browser before parsing.
    ///
    /// Off by default. When enabled, HTML sources — and MHTML/EPUB, which
    /// assemble HTML from their archives — are loaded in the system Chromium
    /// (driven from Rust over the DevTools protocol — no Node/Playwright) so the
    /// CSS cascade is resolved: elements the browser computes as `display:none`
    /// (e.g. a stylesheet-collapsed nav menu) are removed before the normal HTML
    /// backend runs. This is the one behaviour a pure-Rust parse can't reproduce;
    /// everything else (structure, tables, KVP, formatting) is still handled in
    /// Rust on the cleaned HTML.
    ///
    /// Requires the crate's `web-browser` Cargo feature; without it, converting
    /// an HTML source with this enabled returns [`ConversionError::Browser`].
    pub fn use_web_browser(mut self, enable: bool) -> Self {
        self.use_web_browser = enable;
        self
    }

    /// Return `html` unchanged, or — when [`use_web_browser`](Self::use_web_browser)
    /// is on — its headless-browser-cleaned form (computed-hidden elements
    /// removed). Borrows in the common (disabled) case; only allocates when the
    /// browser actually runs.
    fn maybe_prerender<'a>(
        &self,
        html: &'a str,
    ) -> Result<std::borrow::Cow<'a, str>, ConversionError> {
        crate::backend::maybe_prerender_html(html, self.use_web_browser)
    }

    /// Convert a source document to Markdown **incrementally**, returning an
    /// iterator of Markdown chunks (with picture placeholders).
    ///
    /// Concatenating every `Ok` chunk reproduces
    /// [`convert`](Self::convert)`(...).document.export_to_markdown()`
    /// byte-for-byte. The win is for PDF, whose pages are processed in parallel:
    /// each page's Markdown is emitted in document order as soon as it is ready, so
    /// output starts before the whole document is converted. Other formats build
    /// their document up front and stream it through the same interface.
    ///
    /// Streaming is Markdown-only — JSON needs the whole node tree, so there is no
    /// streaming JSON. The conversion runs on a background thread; dropping the
    /// returned [`MarkdownStream`] cancels it.
    #[cfg(feature = "pdf")]
    pub fn convert_streaming(
        &self,
        source: SourceDocument,
    ) -> Result<MarkdownStream, ConversionError> {
        self.convert_streaming_images(source, ImageMode::Placeholder)
    }

    /// Like [`convert_streaming`](Self::convert_streaming) but with an explicit
    /// picture [`ImageMode`].
    ///
    /// [`ImageMode::Referenced`] streams too (issue #80): each page's images
    /// are written to [`artifacts_dir`](Self::artifacts_dir) *as the page's
    /// Markdown is emitted* and dropped from memory, so an image-heavy PDF
    /// holds ~one page of images at a time instead of all of them until
    /// export. The chunks and files match the buffered
    /// `export_to_markdown_with_images(ImageMode::Referenced, ..)` output.
    #[cfg(feature = "pdf")]
    pub fn convert_streaming_images(
        &self,
        source: SourceDocument,
        image_mode: ImageMode,
    ) -> Result<MarkdownStream, ConversionError> {
        if let Some(allowed) = &self.allowed_formats {
            if !allowed.contains(&source.format) {
                return Err(ConversionError::UnsupportedFormat(source.format));
            }
        }
        let source = self.with_encoding(source);
        Ok(crate::stream::spawn(self.clone(), source, image_mode))
    }

    /// Whether the heading-hierarchy stage (#302) is enabled — the streaming
    /// front-end buffers PDF conversions when it is (the stage needs the whole
    /// assembled document, and streamed output must stay byte-identical to
    /// buffered output).
    #[cfg(feature = "pdf")]
    pub(crate) fn heading_hierarchy_enabled(&self) -> bool {
        self.heading_hierarchy
    }

    /// Streaming internals ([`crate::stream`]) read the producer's settings
    /// off the converter clone they receive.
    #[cfg(feature = "pdf")]
    pub(crate) fn stream_settings(&self) -> crate::stream::StreamSettings {
        crate::stream::StreamSettings {
            strict: self.strict,
            no_table_former: self.no_table_former,
            no_text_panels: self.no_text_panels,
            text_layer_only: self.text_layer_only,
            no_ocr: self.no_ocr,
            password: self.password.clone(),
            force_full_page_ocr: self.force_full_page_ocr,
            enrich: self.enrich,
            page_range: self.page_range,
            ocr_lang: self.ocr_lang_choice(),
            ocr_engine: self.ocr_engine_choice(),
            tesseract_lang: self.tesseract_lang_choice(),
            ocr_mode: self.ocr_mode_choice(),
            ocr_scale: self.ocr_scale_choice(),
            images_scale: self.images_scale,
            artifacts_dir: self.artifacts_dir.clone(),
            page_break_placeholder: self.page_break_placeholder.clone(),
            compact_tables: self.compact_tables,
            document_timeout: self.document_timeout,
            redact: self.redact.clone(),
        }
    }

    /// Convert a single source document.
    ///
    /// The source converts as its declared format (the extension's). Only if
    /// that fails is the content inspected: when it is evidently another
    /// format — RTF, an OOXML / ODF / EPUB package, an OLE Word / Excel /
    /// PowerPoint / Outlook file, PDF or HTML saved under the wrong extension
    /// (#556) — the conversion is retried once as that format, and the result
    /// reports the format it was converted as. A document that converts as
    /// declared is never inspected; when the retry fails too, or nothing is
    /// recognised, the first attempt's error stands.
    pub fn convert(&self, source: SourceDocument) -> Result<ConversionResult, ConversionError> {
        if let Some(allowed) = &self.allowed_formats {
            if !allowed.contains(&source.format) {
                return Err(ConversionError::UnsupportedFormat(source.format));
            }
        }
        let source = self.with_encoding(source);
        let err = match self.convert_as(&source) {
            Ok(result) => return Ok(result),
            Err(err) => err,
        };
        let Some(format) = crate::sniff::detect(&source.bytes).filter(|f| {
            *f != source.format
                && self
                    .allowed_formats
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(f))
        }) else {
            return Err(err);
        };
        let declared = source.format;
        let retry = SourceDocument { format, ..source };
        // The retry's error replaces the first one only when it says the
        // file is encrypted (#624) — an encrypted `.ppt` named `.pptx` is
        // better reported as encrypted than as a bad ZIP.
        let result = self.convert_as(&retry).map_err(|e| {
            if crate::backend::offcrypto::is_encryption_error(&e) {
                e
            } else {
                err
            }
        })?;
        // Converted, but not as the name said: worth a line, since the
        // mislabelled file is otherwise invisible.
        eprintln!(
            "docling: warning: {}: not a valid {} file; converted as {} (its content)",
            retry.name,
            declared.as_str(),
            format.as_str()
        );
        Ok(result)
    }

    /// Convert `source` as its declared format, no retry.
    fn convert_as(&self, source: &SourceDocument) -> Result<ConversionResult, ConversionError> {
        // Problems a conversion survives (docling's `ConversionResult.errors`):
        // today the PDF pipeline's spent document budget (#497).
        #[cfg_attr(not(feature = "pdf"), allow(unused_mut))]
        let mut errors: Vec<crate::ErrorItem> = Vec::new();
        // An encrypted Office document (#624/#625) is decrypted with the
        // converter's password (or the format's default password) into the
        // file the backend reads; without a key that opens it, the error
        // says the document is encrypted — not "bad zip" or an empty result.
        let unlocked;
        let source = match crate::backend::offcrypto::unlock(source, self.password.as_deref())? {
            Some(bytes) => {
                unlocked = SourceDocument {
                    bytes,
                    ..source.clone()
                };
                &unlocked
            }
            None => source,
        };
        let mut document = match source.format {
            // A legacy APS (Automated Patent System) plain-text patent (`PATN`
            // first record) is reconstructed verbatim, mirroring docling.
            InputFormat::Md if crate::backend::uspto::looks_like_aps(&source.text()?) => {
                crate::backend::uspto::convert_aps(source)?
            }
            // A text/Markdown-typed file that is actually an XML document (e.g. a
            // JATS article saved with a `.txt` extension) routes to the XML
            // backends by content, mirroring docling's content-based detection.
            InputFormat::Md if looks_like_xml(&source.text()?) => match sniff_xml(&source.bytes) {
                InputFormat::XmlUspto => UsptoBackend.convert(source)?,
                InputFormat::XmlXbrl => {
                    crate::backend::xbrl::convert_xbrl(source, self.xbrl_taxonomy.as_deref())?
                }
                // docling's format detection reads an XML-looking `.txt` as
                // `application/xml` and, when its DOCTYPE names a JATS DTD
                // (`JATS-journalpublishing…` / `JATS-archive…`), converts it
                // with the JATS backend like a real `.nxml`; any other XML
                // saved as `.txt` is reconstructed generically
                // (element-by-element).
                _ if has_jats_doctype(&source.text()?) => JatsBackend {
                    fetch_images: self.fetch_images,
                }
                .convert(source)?,
                _ => crate::backend::jats::convert_generic(source)?,
            },
            // DeepSeek-OCR annotated Markdown (VLM token format) is detected by
            // its `<|ref|>…[[bbox]]` annotations and parsed separately.
            InputFormat::Md if is_deepseek_markdown(&source.text()?) => {
                DeepSeekBackend.convert(source)?
            }
            InputFormat::Md => MarkdownBackend {
                strict: self.strict,
            }
            .convert(source)?,
            InputFormat::Csv => CsvBackend.convert(source)?,
            InputFormat::Html => {
                // Optionally resolve the CSS cascade in a headless browser first
                // (strips computed-hidden elements); everything else stays in the
                // Rust HTML backend, which runs on the cleaned HTML.
                // Bytes → text through docling's BeautifulSoup decoding order
                // (#371): BOM, declared charset, UTF-8, windows-1252 — so a
                // legacy windows-1252 page converts instead of failing the
                // UTF-8 check every other text backend applies.
                let decoded = crate::backend::decode_html_bytes(&source.bytes);
                let html = self.maybe_prerender(&decoded)?;
                if self.fetch_images {
                    let resolver = crate::backend::FsImageResolver::new(
                        source.base_dir().map(|p| p.to_path_buf()),
                        source.base_url.clone(),
                    );
                    crate::backend::convert_html(&source.name, &html, &resolver)
                } else {
                    crate::backend::convert_html(&source.name, &html, &crate::backend::NoFetch)
                }
            }
            InputFormat::Asciidoc => AsciiDocBackend {
                fetch_images: self.fetch_images,
            }
            .convert(source)?,
            InputFormat::Xlsx => XlsxBackend {
                skip_empty: self.skip_empty_cells,
            }
            .convert(source)?,
            InputFormat::Pptx => PptxBackend.convert(source)?,
            // RTF (#209): a docling.rs extension — docling reaches RTF only via
            // LibreOffice; here it parses natively (hand-rolled tokenizer).
            InputFormat::Rtf => RtfBackend.convert(source)?,
            InputFormat::Visio => VisioBackend.convert(source)?,
            // AbiWord (#216): docling.rs extension, native AWML parse.
            InputFormat::Abiword => AbwBackend.convert(source)?,
            // WordPerfect 5.x/6.x+ (#216): docling.rs extension, native parse
            // of the ÿWPC function-code stream.
            InputFormat::WordPerfect => WpdBackend.convert(source)?,
            // Microsoft Works word processor (#216): docling.rs extension,
            // native parse after libwps.
            InputFormat::Works => WpsBackend.convert(source)?,
            // StarOffice 5 binaries (#215): docling.rs extension, native CFB
            // parse (docling would go through LibreOffice).
            InputFormat::StarOffice5 => StarOffice5Backend.convert(source)?,
            // DjVu (#434): docling.rs extension, pure-Rust decode (`djvu-rs`).
            // The hidden text layer is the default; a scan-only DjVu falls back
            // to rasterize + OCR when the ML pipeline is built.
            InputFormat::Djvu => self.convert_djvu(source)?,
            // DIF/SYLK/dBase (#216): docling.rs extensions, one content-sniffing
            // backend for the three table relics.
            InputFormat::Dbf | InputFormat::Dif | InputFormat::Sylk => {
                InterchangeBackend.convert(source)?
            }
            // Lotus/Quattro/Works record streams (#216): one BOF-sniffing
            // backend for the whole DOS-era family.
            InputFormat::Lotus => LotusBackend.convert(source)?,
            // Quattro Pro (#216): docling.rs extension, native parse after
            // libwps (DOS/Windows record streams, QPW OLE zones).
            InputFormat::QuattroPro => QuattroBackend.convert(source)?,
            InputFormat::Docx => DocxBackend.convert(source)?,
            // Legacy binary Office (issue #127): parsed natively — docling
            // proper converts these through LibreOffice first (PR #3804).
            InputFormat::Xls => XlsBackend {
                skip_empty: self.skip_empty_cells,
            }
            .convert(source)?,
            InputFormat::Ppt => PptBackend.convert(source)?,
            InputFormat::Doc => DocBackend.convert(source)?,
            InputFormat::Vtt => WebVttBackend.convert(source)?,
            InputFormat::Ebcdic => EbcdicBackend {
                layout: self.ebcdic_layout.clone(),
            }
            .convert(source)?,
            InputFormat::Email => EmailBackend {
                list_attachments: self.list_attachments,
            }
            .convert(source)?,
            InputFormat::Mhtml => MhtmlBackend {
                fetch_images: self.fetch_images,
                use_web_browser: self.use_web_browser,
            }
            .convert(source)?,
            InputFormat::Epub => EpubBackend {
                fetch_images: self.fetch_images,
                use_web_browser: self.use_web_browser,
            }
            .convert(source)?,
            InputFormat::JsonDocling => DoclingJsonBackend.convert(source)?,
            InputFormat::Latex => LatexBackend.convert(source)?,
            // A bare `.xml` defaults to XmlJats; sniff the content to route to the
            // right XML backend (docling distinguishes by DOCTYPE / root element).
            InputFormat::XmlJats | InputFormat::XmlUspto | InputFormat::XmlXbrl => {
                match sniff_xml(&source.bytes) {
                    InputFormat::XmlUspto => UsptoBackend.convert(source)?,
                    InputFormat::XmlXbrl => {
                        crate::backend::xbrl::convert_xbrl(source, self.xbrl_taxonomy.as_deref())?
                    }
                    _ => JatsBackend {
                        fetch_images: self.fetch_images,
                    }
                    .convert(source)?,
                }
            }
            InputFormat::Odt | InputFormat::Ods | InputFormat::Odp => {
                crate::backend::convert_odf(source, self.fetch_images)?
            }
            // DocLang back in: bare XML (`.dclg`/`.dclg.xml`) or the OPC
            // archive `--to dclx` writes.
            InputFormat::XmlDoclang | InputFormat::Dclx => {
                crate::backend::DoclangBackend.convert(source)?
            }
            // Raw DocTags (VLM token markup, #152): the tolerant docling-core
            // parser — never fails, best-effort document out.
            InputFormat::DocTags => {
                let mut doc = docling_core::doctags::parse(&source.text()?);
                doc.name = source.name.clone();
                doc
            }
            #[cfg(feature = "pdf")]
            InputFormat::Pdf => {
                let converted = self
                    .ml_pipeline()
                    .map(|p| {
                        p.force_full_page_ocr(self.force_full_page_ocr)
                            .pages(self.page_range)
                    })
                    .and_then(|mut p| {
                        p.convert_outcome(&source.bytes, self.password.as_deref(), &source.name)
                    })
                    .map_err(|e| ConversionError::with_source("pdf", e))?;
                if let Some(message) = converted.completion.message() {
                    errors.push(crate::ErrorItem::timeout(message));
                }
                converted.document
            }
            // SVG (#212), the ML route: rasterize (resvg, white-backed PNG at
            // ~2048px long side) and ride the image pipeline. `--no-ocr` /
            // `--text-layer-only` short-circuit to direct <text> extraction
            // instead — the SVG carries its text natively, so skipping OCR
            // must not mean losing it.
            #[cfg(feature = "pdf")]
            InputFormat::Svg if !self.text_layer_only && !self.no_ocr => {
                let png = crate::backend::svg::rasterize_png(&source.bytes)?;
                self.ml_pipeline()
                    .and_then(|mut p| p.convert_image(&png, &source.name))
                    .map_err(|e| ConversionError::with_source("svg", e))?
            }
            // SVG without the ML pipeline (pdf-text / wasm builds) or with
            // --no-ocr / --text-layer-only: pure-Rust <text> extraction, flat
            // paragraphs in reading order (the pdf / pdf-text split, applied
            // to SVG) — the SVG carries its text natively, so skipping OCR
            // must not mean losing it.
            InputFormat::Svg => crate::backend::SvgBackend.convert(source)?,
            // Apple iWork (#213): pure-Rust IWA text extraction, all builds.
            InputFormat::Pages | InputFormat::Numbers | InputFormat::Keynote => {
                crate::backend::IworkBackend.convert(source)?
            }
            #[cfg(feature = "pdf")]
            InputFormat::Image => self
                .ml_pipeline()
                .and_then(|mut p| p.convert_image(&source.bytes, &source.name))
                .map_err(|e| ConversionError::with_source("image", e))?,
            #[cfg(feature = "pdf")]
            InputFormat::MetsGbs => self
                .ml_pipeline()
                .and_then(|mut p| {
                    docling_pdf::convert_mets_gbs_with_pipeline(&source.bytes, &source.name, &mut p)
                })
                .map_err(|e| ConversionError::with_source("mets-gbs", e))?,
            // Audio → Whisper ASR (symphonia decode + ONNX inference); each
            // transcribed segment becomes a `[time: start-end] text` paragraph.
            #[cfg(feature = "asr")]
            InputFormat::Audio => docling_asr::convert_audio_with_options(
                &source.bytes,
                &source.name,
                self.asr_model.as_deref(),
                self.asr_lang.as_deref(),
            )
            .map_err(|e| ConversionError::with_source(source.format.as_str(), e))?,
            // Video (#138): the audio track transcribes through the same ASR
            // path (Phase 1), and — when the ffmpeg binary is available —
            // sampled frames interleave with the transcript as timestamped
            // pictures (Phase 2). Without ffmpeg: transcript only.
            #[cfg(feature = "asr")]
            InputFormat::Video => self
                .convert_video(source)
                .map_err(|e| ConversionError::with_source(source.format.as_str(), e))?,
            // Without the full ML pipeline, `pdf-text` still converts a PDF's
            // embedded text layer (pure Rust — the wasm32 path), equivalent to
            // `--text-layer-only`: flat paragraphs, no headings/tables/pictures. A
            // scanned PDF has no text layer, so an empty document means "this
            // needs OCR" — say so instead of returning nothing.
            #[cfg(all(feature = "pdf-text", not(feature = "pdf")))]
            InputFormat::Pdf => {
                let doc = docling_pdf::convert_text_layer_pages(
                    &source.bytes,
                    &source.name,
                    self.page_range,
                )
                .map_err(|e| ConversionError::with_source("pdf", e))?;
                if doc.nodes.is_empty() {
                    return Err(ConversionError::Parse(
                        "PDF has no embedded text layer (scanned/image-only?); OCR needs a \
                         build with the `pdf` feature"
                            .into(),
                    ));
                }
                doc
            }
            // Compiled without the ML pipelines: the formats stay detectable,
            // but converting them needs a build with the matching feature.
            #[cfg(not(any(feature = "pdf", feature = "pdf-text")))]
            InputFormat::Pdf => {
                return Err(ConversionError::Parse(
                    "Pdf conversion is not compiled in (rebuild with the `pdf` feature, or \
                     `pdf-text` for text-layer-only extraction)"
                        .into(),
                ))
            }
            #[cfg(not(feature = "pdf"))]
            InputFormat::Image | InputFormat::MetsGbs => {
                return Err(ConversionError::Parse(format!(
                    "{:?} conversion is not compiled in (rebuild with the `pdf` feature)",
                    source.format
                )))
            }
            #[cfg(not(feature = "asr"))]
            InputFormat::Audio | InputFormat::Video => {
                return Err(ConversionError::Parse(format!(
                    "{} conversion is not compiled in (rebuild with the `asr` feature)",
                    source.format.as_str()
                )))
            }
        };
        self.enrich_pictures(&mut document, source.format);
        // The PII pass (#621) runs on the finished model, before anything
        // reads it — after the backend, before the export-side settings.
        let redaction = self.redact(&mut document)?;
        self.finish_document(&mut document);

        let status = if errors.is_empty() {
            ConversionStatus::Success
        } else {
            ConversionStatus::PartialSuccess
        };
        Ok(ConversionResult {
            document,
            status,
            input_name: source.name.clone(),
            format: source.format,
            redaction,
            errors,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file whose extension lies converts as its content after the
    /// declared format fails (#556); junk keeps its original error, and a
    /// format the converter does not allow is not retried into.
    #[test]
    fn mislabelled_sources_retry_as_their_content() {
        let conv = DocumentConverter::new();
        let as_doc =
            |bytes: &[u8]| SourceDocument::from_bytes("x", InputFormat::Doc, bytes.to_vec());

        let rtf = conv
            .convert(as_doc(b"{\\rtf1\\ansi Hello RTF\\par}"))
            .unwrap();
        assert_eq!(rtf.format, InputFormat::Rtf);
        assert_eq!(rtf.document.export_to_markdown().trim(), "Hello RTF");

        let html = conv
            .convert(as_doc(
                b"<!DOCTYPE html><html><body><p>Hello HTML</p></body></html>",
            ))
            .unwrap();
        assert_eq!(html.format, InputFormat::Html);
        assert_eq!(html.document.export_to_markdown().trim(), "Hello HTML");

        let docx = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/data/docx/sources/docx_moveto_body.docx"),
        )
        .unwrap();
        let docx = conv.convert(as_doc(&docx)).unwrap();
        assert_eq!(docx.format, InputFormat::Docx);
        assert!(docx
            .document
            .export_to_markdown()
            .contains("avantMARK11apres"));

        let err = conv.convert(as_doc(b"neither of them")).unwrap_err();
        assert!(err.to_string().contains("doc"), "{err}");

        let only_doc = DocumentConverter::with_allowed_formats([InputFormat::Doc]);
        assert!(only_doc.convert(as_doc(b"{\\rtf1 Hello\\par}")).is_err());
    }

    /// docling reads an XML-looking `.txt` as `application/xml` and converts
    /// it with the JATS backend when its DOCTYPE names a JATS DTD; other XML
    /// under `.txt` stays the generic element-by-element reconstruction.
    #[test]
    fn jats_doctype_text_file_uses_the_jats_backend() {
        let jats = "<!DOCTYPE article PUBLIC \"-//NLM//DTD JATS (Z39.96) Journal Archiving and Interchange DTD v1.2 20190208//EN\" \"JATS-archivearticle1.dtd\">\n<article><front><article-meta><title-group><article-title>T</article-title></title-group></article-meta></front><body><sec><title>S</title><p>Body.</p></sec></body></article>";
        let src = SourceDocument::from_bytes("a.txt", InputFormat::Md, jats.as_bytes().to_vec());
        let doc = DocumentConverter::new().convert(src).unwrap().document;
        assert!(doc.tree.is_some(), "JATS tree expected");
        assert_eq!(doc.export_to_markdown().trim(), "# T\n\n## S\n\nBody.");
        let other = "<?xml version=\"1.0\"?>\n<article><body><sec><title>S</title><p>Body.</p></sec></body></article>";
        let src = SourceDocument::from_bytes("b.txt", InputFormat::Md, other.as_bytes().to_vec());
        let doc = DocumentConverter::new().convert(src).unwrap().document;
        assert!(doc.tree.is_none(), "generic XML path expected");
    }

    /// docling's `TextBackendOptions.encoding`: the converter's `encoding`
    /// decodes text inputs as named (a source's own setting wins), and an
    /// undecodable byte or unknown label is an error, not a guess.
    #[test]
    fn encoding_option_decodes_text_inputs() {
        let sjis = b"# \x93\xfa\x96\x7b\n".to_vec();
        let md = |c: &DocumentConverter, s: SourceDocument| {
            c.convert(s).map(|r| r.document.export_to_markdown())
        };
        let conv = DocumentConverter::new().encoding(Some("shift_jis".into()));
        let src = || SourceDocument::from_bytes("doc", InputFormat::Md, sjis.clone());
        assert_eq!(md(&conv, src()).unwrap().trim(), "# 日本");
        // Detection reads the same bytes as windows-1252.
        assert_ne!(
            md(&DocumentConverter::new(), src()).unwrap().trim(),
            "# 日本"
        );
        // The source's own encoding takes precedence over the converter's.
        let own = src().with_encoding(Some("shift_jis".into()));
        let latin = DocumentConverter::new().encoding(Some("latin1".into()));
        assert_eq!(md(&latin, own).unwrap().trim(), "# 日本");
        assert!(md(
            &DocumentConverter::new().encoding(Some("utf-8".into())),
            src()
        )
        .is_err());
        assert!(md(
            &DocumentConverter::new().encoding(Some("nope-1".into())),
            src()
        )
        .is_err());
    }

    #[test]
    fn end_to_end_markdown() {
        let src =
            SourceDocument::from_bytes("doc", InputFormat::Md, b"# Hello\n\nWorld.\n".to_vec());
        let result = DocumentConverter::new().convert(src).unwrap();
        assert_eq!(result.status, ConversionStatus::Success);
        assert_eq!(result.document.export_to_markdown(), "# Hello\n\nWorld.\n");
    }

    #[test]
    fn doctags_input_converts() {
        // Raw DocTags markup (#152) — the VLM token stream — as a first-class
        // input format (.doctags/.dt), through the tolerant docling-core
        // parser.
        let markup = b"<doctag><section_header_level_1><loc_1><loc_2><loc_3><loc_4>Intro</section_header_level_1><text>Body.</text></doctag>"
            .to_vec();
        let src = SourceDocument::from_bytes("page.doctags", InputFormat::DocTags, markup);
        let result = DocumentConverter::new().convert(src).unwrap();
        let md = result.document.export_to_markdown();
        assert!(md.contains("## Intro"), "{md}");
        assert!(md.contains("Body."), "{md}");
    }

    #[test]
    fn doclang_xml_round_trips() {
        // Every input format now has a backend; DocLang XML reads back in and
        // re-exports as Markdown.
        let xml = b"<doclang version=\"0.7\">\n  <heading>Title</heading>\n  \
                    <text>Hello <bold>world</bold></text>\n</doclang>"
            .to_vec();
        let src = SourceDocument::from_bytes("doc.dclg", InputFormat::XmlDoclang, xml);
        let result = DocumentConverter::new().convert(src).unwrap();
        let md = result.document.export_to_markdown();
        assert!(md.contains("# Title"), "{md}");
        assert!(md.contains("**world**"), "{md}");
    }

    #[test]
    fn sniffs_uspto_doctype_case_insensitively() {
        // docling PR #3801: Grant Full Text v2.5 files were missed when the
        // DOCTYPE casing differed.
        for head in [
            "<?xml version=\"1.0\"?><!DOCTYPE PATDOC SYSTEM \"ST32-US-Grant-025xml.dtd\"><PATDOC/>",
            "<?xml version=\"1.0\"?><!DOCTYPE patdoc SYSTEM \"st32-us-grant-025xml.dtd\"><patdoc/>",
            "<?xml version=\"1.0\"?><US-PATENT-GRANT-V4/>",
        ] {
            assert_eq!(
                super::sniff_xml(head.as_bytes()),
                InputFormat::XmlUspto,
                "head: {head}"
            );
        }
    }
}
